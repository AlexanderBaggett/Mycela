//! Hair's strand projection and shading over the three connected image maps.
use super::{
    PointwiseError, PointwiseFilter, SharedImageMap, check_render_cancelled, parameter_ops,
};
use crate::{FloatBuffer, ImageError, ImageGeometry, framework_cast};

fn density_scale(factor: f64) -> f64 {
    1.14_f64.powf(factor.ln() / 2.0_f64.ln())
}

pub(super) fn commit_scale<E: 'static>(
    filter: &PointwiseFilter<E>,
    scale: f64,
) -> Result<(), PointwiseError<E>> {
    for (index, multiplier) in [
        (6, 1.0 / scale),
        (1, 1.0 / scale),
        (2, density_scale(scale)),
    ] {
        let property = filter.parameter(index)?;
        if parameter_ops::is_animated(|| property.retained_animation())? {
            let bounds = property.limits()?.snapshot();
            property
                .animation_curve(0)?
                .scale_bounds(
                    multiplier as f32,
                    bounds.minimum.to_framework_f32(),
                    bounds.maximum.to_framework_f32(),
                    bounds.increment.to_framework_f32(),
                )
                .map_err(crate::AnimationError::Bounds)?;
        } else {
            property.set_number(property.number() * multiplier)?;
        }
    }
    Ok(())
}

fn plane<T: Clone>(length: usize, value: T) -> Result<Vec<T>, ImageError> {
    let mut result = Vec::new();
    result
        .try_reserve_exact(length)
        .map_err(|_| ImageError::Allocation)?;
    result.resize(length, value);
    Ok(result)
}

fn pixel(values: &[f32], index: usize) -> Result<f32, ImageError> {
    let index = if values.len() == 1 { 0 } else { index };
    values.get(index).copied().ok_or(ImageError::Index)
}

fn density(source: &[f32], area: usize, value: f64) -> Result<Vec<f32>, ImageError> {
    let mut amount = (value / 2.0) as f32;
    if amount >= 2.0 {
        amount = 1.9999;
    }
    let soft = amount >= 1.0;
    amount %= 1.0;
    let threshold = 1.0 - amount;
    let divisor = threshold.max(0.001);
    let reciprocal = 1.0 / divisor;
    let mut density = plane(area, 0.0)?;
    for (index, result) in density.iter_mut().enumerate() {
        let input = pixel(source, index)?;
        *result = if soft {
            let input = if input > 0.0 { input } else { 0.0 };
            if input > threshold {
                1.0
            } else if source.len() == 1 {
                // ah6 uses division for the constant lane and a reciprocal
                // multiplication for the dense worker.
                input / divisor
            } else {
                input * reciprocal
            }
        } else if input < threshold {
            0.0
        } else {
            input * amount
        };
    }
    Ok(density)
}

struct Grid {
    width: usize,
    height: usize,
    surface: Vec<f32>,
    normals: Vec<[f32; 2]>,
    density: Vec<f32>,
}

impl Grid {
    fn new(
        [width, height]: [usize; 2],
        heights: &[f32],
        height_factor: f32,
        direction: [f64; 2],
        density: Vec<f32>,
    ) -> Result<Self, ImageError> {
        let area = density.len();
        let mut surface = plane(area, 0.0)?;
        for (index, value) in surface.iter_mut().enumerate() {
            *value = pixel(heights, index)? * height_factor;
        }
        let mut normals = plane(area, [0.0; 2])?;
        for (index, normal) in normals.iter_mut().enumerate() {
            let (x, y) = (index % width, index / width);
            let here = f64::from(surface[index]);
            let right = f64::from(surface[(x + 1) % width + y * width]);
            // The CLR routine uses height as the stride for the next row.
            let below_index = if y == height - 1 {
                x
            } else {
                x + (y + 1) * height
            };
            let below = f64::from(*surface.get(below_index).ok_or(ImageError::Index)?);
            let nx = (right - here) * width as f64 - direction[0];
            let ny = (below - here) * height as f64 + direction[1];
            let length = 1.0 / (nx * nx + ny * ny + 1.0).sqrt();
            *normal = [(nx * length) as f32, (ny * length) as f32];
        }
        Ok(Self {
            width,
            height,
            surface,
            normals,
            density,
        })
    }

    fn project(
        &self,
        elevation: f32,
        length: f32,
        coverage: &mut [f32],
        alpha: &mut [f32],
        strands: &mut [i32],
    ) -> Result<(), ImageError> {
        strands.fill(-1);
        for (index, (&surface, &[nx, ny])) in self.surface.iter().zip(&self.normals).enumerate() {
            let difference = elevation - surface;
            if difference > 0.0 {
                let offset = difference / down(nx, ny);
                if -offset < length {
                    let x = (index % self.width) as i32;
                    let y = (index / self.width) as i32;
                    let dx = framework_cast::integer(f64::from(nx * offset * self.width as f32));
                    let dy = framework_cast::integer(f64::from(ny * offset * self.height as f32));
                    let x = x.wrapping_add(dx).rem_euclid(self.width as i32) as usize;
                    let y = y.wrapping_add(dy).rem_euclid(self.height as i32) as usize;
                    // Later strands overwrite earlier projections in the same
                    // sample. The original projection also uses height stride.
                    *strands
                        .get_mut(x + y * self.height)
                        .ok_or(ImageError::Index)? = index as i32;
                }
            } else if coverage[index] < 1.0 {
                alpha[index] = 1.0 - coverage[index];
                coverage[index] = 1.0;
            }
        }
        Ok(())
    }
}

fn down(x: f32, y: f32) -> f32 {
    -(1.0 - f64::from(x * x + y * y)).sqrt() as f32
}

struct Sampling {
    direction_to_light: [f32; 3],
    distance_multiplier: f32,
    distance_step: f32,
    shine: f32,
    scatter: f32,
    length: f32,
}

impl Sampling {
    fn shade<E>(
        &self,
        grid: &Grid,
        colors: &[Vec<f32>],
        output: &mut [Vec<f32>],
        alpha: &mut [f32],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), PointwiseError<E>> {
        let area = grid.surface.len();
        let mut coverage = plane(area, 0.0)?;
        let mut strands = plane(area, -1)?;
        let mut elevation = 0.5_f32;
        loop {
            check_render_cancelled(cancelled)?;
            grid.project(elevation, self.length, &mut coverage, alpha, &mut strands)?;
            let mut pending = false;
            for (index, (&strand, covered)) in strands.iter().zip(&mut coverage).enumerate() {
                if *covered > 0.96 || covered.is_nan() {
                    continue;
                }
                pending = true;
                if strand < 0 {
                    continue;
                }
                let source = strand as usize;
                let weight = grid.density[source] * (1.0 - *covered) * self.distance_multiplier;
                let [nx, ny] = grid.normals[source];
                let [lx, ly, lz] = self.direction_to_light;
                let mut light = 1.0 - (lx * nx + ly * ny + lz * down(nx, ny)).abs() * self.scatter;
                if light > 0.8 {
                    light += (light - 0.8) * self.shine;
                }
                light *= weight;
                for (color, channel) in colors.iter().zip(output.iter_mut()) {
                    channel[index] += pixel(color, source)? * light;
                }
                *covered += weight;
            }
            if !pending {
                break;
            }
            let next = elevation - self.distance_step;
            if next == elevation {
                return Err(PointwiseError::UnsupportedCapability(
                    "Hair sampling did not advance",
                ));
            }
            elevation = next;
        }
        for channel in output {
            for value in channel {
                if *value > 1.0 {
                    *value = 1.0;
                }
            }
        }
        Ok(())
    }
}

pub(super) fn retained<E: 'static>(
    filter: &PointwiseFilter<E>,
    inputs: &[Option<SharedImageMap>],
    geometry: ImageGeometry,
    time: f64,
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
    let factor = filter.factor();
    let numeric = |index| filter.parameter(index)?.effective_value_at(time);
    let height_factor = (numeric(6)? / factor) as f32;
    let length = numeric(1)? / factor;
    let amount = numeric(2)? * density_scale(factor);
    let [heightmap, density_map, color] = inputs else {
        return Err(PointwiseError::Index);
    };
    let [heightmap, density_map, color] =
        [heightmap, density_map, color].map(|input| input.as_ref());
    let density_map = density_map.ok_or(PointwiseError::NullReference)?;
    let image = SharedImageMap::new(geometry);
    let density_input = density_map.channel(0, false)?.values();
    if density_input == [0.0] {
        image.set_constant(3, 1.0)?;
        return Ok(Some(image));
    }
    let heightmap = heightmap.ok_or(PointwiseError::NullReference)?;
    let color = color.ok_or(PointwiseError::NullReference)?;
    let [width, height] = [geometry.width(), geometry.height()]
        .map(|dimension| usize::try_from(dimension).unwrap_or(0));
    let area = width
        .checked_mul(height)
        .filter(|&area| area <= i32::MAX as usize)
        .ok_or(ImageError::Overflow)?;
    if area == 0 {
        return Err(ImageError::Argument.into());
    }
    let mut direction_to_light = filter
        .parameter(0)?
        .custom_object()?
        .ok_or(PointwiseError::NullReference)?
        .light_position()?
        .map(|point| point as f32);
    direction_to_light[0] = -direction_to_light[0];
    let [lx, ly] = direction_to_light;
    let magnitude = f64::from(lx * lx + ly * ly + 1.0).sqrt() as f32;
    let direction = filter.parameter(3)?;
    let grid = Grid::new(
        [width, height],
        &heightmap.channel(0, false)?.values(),
        height_factor,
        [
            direction.raw_value_at(0, time)? / 10.0,
            direction.raw_value_at(1, time)? / 10.0,
        ],
        density(&density_input, area, amount)?,
    )?;
    let distance_multiplier =
        (1.0 / ((0.875 + width.min(2048) as f64 / 2048.0) * numeric(7)?)) as f32;
    let sampling = Sampling {
        direction_to_light: [lx / magnitude, ly / magnitude, -1.0 / magnitude],
        distance_multiplier,
        distance_step: 0.01 * distance_multiplier,
        shine: (1.0 + numeric(4)?) as f32,
        scatter: (1.0 / (1.0 + numeric(5)?)) as f32,
        length: (length / 2.0) as f32,
    };
    if sampling.distance_step <= 0.0 || !sampling.distance_step.is_finite() {
        return Err(PointwiseError::UnsupportedCapability("Hair sample density"));
    }
    let use_rgb = color.channel_count()? > 1;
    let mut alpha = plane(area, 0.0)?;
    let channels = if use_rgb { 3 } else { 1 };
    let split_rgb = width >= 4000 && use_rgb;
    let batch_size = if split_rgb { 1 } else { channels };
    for first in (0..channels).step_by(batch_size) {
        let colors = (first..first + batch_size)
            .map(|index| {
                color
                    .channel(index as i32, false)
                    .map(|plane| plane.values())
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut output = (0..batch_size)
            .map(|_| plane(area, 0.0))
            .collect::<Result<Vec<_>, _>>()?;
        sampling.shade(&grid, &colors, &mut output, &mut alpha, cancelled)?;
        for (offset, channel) in output.into_iter().enumerate() {
            let index = (first + offset) as i32;
            image.set_buffer(index, Some(&FloatBuffer::new(channel)))?;
            if split_rgb {
                image.seal_channel(index)?;
            }
        }
    }
    if !use_rgb {
        // The source allocates the green and blue destinations even in its
        // single-channel shading pass.
        image.materialize(1)?;
        image.materialize(2)?;
    }
    image.set_buffer(3, Some(&FloatBuffer::new(alpha)))?;
    image.merge_effects(&[
        Some(heightmap.clone()),
        Some(density_map.clone()),
        Some(color.clone()),
    ])?;
    Ok(Some(image))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_density_returns_transparent_black_before_merging_effects() {
        let geometry = ImageGeometry::new(8, 8);
        let height = SharedImageMap::new(geometry);
        let density = SharedImageMap::new(geometry);
        let color = SharedImageMap::new(geometry);
        let effect = crate::SharedNamedChannel::new(
            Some("Height".encode_utf16().collect()),
            Some(crate::SharedFloatStore::new(FloatBuffer::new(vec![0.8])).unwrap()),
        );
        height.add_named_channel(effect).unwrap();
        let filter = PointwiseFilter::<()>::new(super::super::PointwiseKind::Hair).unwrap();
        let inputs = [Some(height), Some(density.clone()), Some(color)];
        let blank = retained(&filter, &inputs, geometry, 0., &|| false)
            .unwrap()
            .unwrap();
        assert_eq!(blank.channel(0, true).unwrap().values(), [0.]);
        assert_eq!(blank.channel(3, true).unwrap().values(), [1.]);
        assert!(blank.named_channels().is_empty());

        density.set_constant(0, 1.).unwrap();
        let strands = retained(&filter, &inputs, geometry, 0., &|| false)
            .unwrap()
            .unwrap();
        assert_eq!(strands.named_channels().len(), 1);
        assert!(strands.uses_plane(1).unwrap());
        assert!(strands.uses_plane(2).unwrap());
    }
}
