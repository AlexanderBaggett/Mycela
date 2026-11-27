//! aa5's Double path integration and distinct nearest Regions worker.
use super::{PointwiseError, PointwiseFilter, PointwiseRenderOptions, SharedImageMap};
use super::{motion_sampling::BilinearTaps, transform::*};
use crate::{
    FloatBuffer, ImageError, ImageGeometry, ProceduralExecution, SharedFloatStore,
    image_map::{read_pixel, write_pixel},
    procedural_schedule::for_each_dense_band,
};
struct Displacement<'a> {
    direction: &'a [f32],
    distance: &'a [f32],
    geometry: ImageGeometry,
    steps: i32,
    dx: f64,
    dy: f64,
    aa: bool,
    regions: bool,
}

impl Displacement<'_> {
    // Ordered comparisons deliberately pass NaN through the wrap gate.
    #[allow(clippy::manual_range_contains)]
    fn position<E>(&self, col: i32, row: i32) -> Result<(f64, f64), PointwiseError<E>> {
        let Self {
            direction,
            distance,
            steps,
            dx,
            dy,
            aa,
            regions,
            ..
        } = *self;
        let w = self.geometry.width();
        let h = self.geometry.height();
        let (wf, hf) = (f64::from(w), f64::from(h));
        let turn = std::f64::consts::PI * 2.;
        let (mut x, mut y) = (f64::from(col), f64::from(row));
        for step in 0..steps {
            let at = cast(x).wrapping_add(cast(y).wrapping_mul(w));
            let angle = if direction.len() > 1 {
                if step > 0 && aa && !regions {
                    f64::from(super::motion_sampling::sample_pixels(
                        x as f32, y as f32, direction, w, h, true,
                    )?) * turn
                } else {
                    f64::from(read_pixel(direction, at)?) * turn
                }
            } else {
                f64::from(read_pixel(direction, 0)?) * turn
            };
            let (sx, sy) = if distance.len() > 1 {
                if step > 0 && aa && !regions {
                    let d = f64::from(super::motion_sampling::sample_pixels(
                        x as f32, y as f32, distance, w, h, true,
                    )?);
                    (d * dx, d * dy)
                } else if regions {
                    (
                        f64::from(read_pixel(distance, at)?) * dx,
                        f64::from(read_pixel(distance, at)?) * dy,
                    )
                } else {
                    let d = f64::from(read_pixel(distance, at)?);
                    (d * dx, d * dy)
                }
            } else if regions {
                (
                    f64::from(read_pixel(distance, 0)?) * dx,
                    f64::from(read_pixel(distance, 0)?) * dy,
                )
            } else {
                let d = f64::from(read_pixel(distance, 0)?);
                (d * dx, d * dy)
            };
            let (sin, cos) = mycela_framework_math::sin_cos(angle);
            x -= cos * sx;
            y -= sin * sy;
            if regions {
                x %= wf;
                y %= hf;
                if x < 0. {
                    x += wf;
                }
                if y < 0. {
                    y += hf;
                }
                if x >= wf {
                    x = 0.;
                }
                if y >= hf {
                    y = 0.;
                }
            } else {
                if x < 0. || x >= wf {
                    x %= wf;
                    if x < 0. {
                        x += wf;
                    }
                    if x >= wf {
                        x = 0.;
                    }
                }
                if y < 0. || y >= hf {
                    y %= hf;
                    if y < 0. {
                        y += hf;
                    }
                    if y >= hf {
                        y = 0.;
                    }
                }
            }
        }
        Ok((x, y))
    }

    fn nearest_index(&self, x: f64, y: f64) -> i32 {
        cast(x).wrapping_add(cast(y).wrapping_mul(self.geometry.width()))
    }

    fn render_plane<E>(
        &self,
        source: &FloatBuffer,
        dest: &FloatBuffer,
        execution: ProceduralExecution,
    ) -> Result<(), PointwiseError<E>> {
        let source = source.shared_values();
        let w = self.geometry.width();
        let h = self.geometry.height();
        dest.with_mut_slice(|pixels| {
            crate::procedural_schedule::for_each_chunk(
                w.wrapping_mul(h),
                execution,
                |start, count| {
                    let mut row = start.checked_div(w).ok_or(PointwiseError::DivideByZero)?;
                    let mut col = rem(start, w)?;
                    for cell in start..start.wrapping_add(count) {
                        if col >= w {
                            col = 0;
                            row = row.wrapping_add(1);
                        }
                        let (x, y) = self.position(col, row)?;
                        let value = if self.aa && !self.regions {
                            super::motion_sampling::sample_pixels(
                                x as f32, y as f32, &source, w, h, true,
                            )?
                        } else {
                            read_pixel(&source, self.nearest_index(x, y))?
                        };
                        write_pixel(pixels, cell, value)?;
                        col = col.wrapping_add(1);
                    }
                    Ok(())
                },
            )
        })
    }

    fn render_colors<E>(
        &self,
        source: &SharedImageMap,
        output: &SharedImageMap,
        channels: i32,
        execution: ProceduralExecution,
    ) -> Result<(), PointwiseError<E>> {
        let w = self.geometry.width();
        let h = self.geometry.height();
        let area = w
            .checked_mul(h)
            .filter(|_| w > 0 && h > 0)
            .map(|n| n as usize);
        if let Some(area) = area
            && area >= 512 * 512
            && !matches!(
                execution,
                ProceduralExecution::WorkerPool {
                    processor_count: ..=0
                }
            )
        {
            let mut inputs = Vec::with_capacity(4);
            for ch in 0..channels {
                inputs.push(source.channel_with_effects(ch, false)?);
            }
            if inputs
                .iter()
                .all(|plane| matches!(plane.len(), 1) || plane.len() == area)
            {
                let mut raster_planes = Vec::with_capacity(4);
                for (ch, plane) in inputs.iter().enumerate() {
                    let ch = ch as i32;
                    if plane.len() == 1 {
                        output.copy_indexed_constant(ch, plane.read(0)?, source)?;
                    } else {
                        raster_planes.push((
                            ch,
                            plane.shared_values(),
                            output.allocate_indexed(ch, source)?,
                        ));
                    }
                }
                if !raster_planes.is_empty() {
                    self.render_color_bands(w, h, area, &raster_planes, execution)?;
                    for (ch, _, _) in raster_planes {
                        output.seal_channel(ch)?;
                    }
                }
                return Ok(());
            }
        }
        for ch in 0..channels {
            let values = source.channel_with_effects(ch, false)?;
            if values.len() == 1 {
                output.copy_indexed_constant(ch, values.read(0)?, source)?;
            } else {
                let dest = output.allocate_indexed(ch, source)?;
                self.render_plane(&values, &dest, execution)?;
                output.seal_channel(ch)?;
            }
        }
        Ok(())
    }

    fn render_color_bands<E>(
        &self,
        w: i32,
        h: i32,
        area: usize,
        planes: &[(i32, std::sync::Arc<Vec<f32>>, FloatBuffer)],
        execution: ProceduralExecution,
    ) -> Result<(), PointwiseError<E>> {
        let width = w as usize;
        // Hold one band of interleaved color samples, never a full-image displacement map.
        let rows = (512 * 1024 / width).max(1).min(h as usize);
        let mut scratch = Vec::new();
        scratch
            .try_reserve_exact(width * rows)
            .map_err(|_| ImageError::Allocation)?;
        scratch.resize(width * rows, [0.; 4]);
        for first in (0..area).step_by(scratch.len()) {
            let used = (area - first).min(scratch.len());
            let band = &mut scratch[..used];
            for_each_dense_band(band, width, execution, |start, band| {
                for (offset, row) in band.chunks_exact_mut(width).enumerate() {
                    let y = (first / width + start + offset) as i32;
                    for (x, sample) in row.iter_mut().enumerate() {
                        let (x, y) = self
                            .position::<()>(x as i32, y)
                            .map_err(dense_image_error)?;
                        if self.aa {
                            let taps = BilinearTaps::new::<()>(x as f32, y as f32, w, h, true)
                                .map_err(dense_image_error)?;
                            for (value, (_, source, _)) in sample.iter_mut().zip(planes) {
                                *value = taps.sample_pixels(source)?;
                            }
                        } else {
                            let index = self.nearest_index(x, y);
                            for (value, (_, source, _)) in sample.iter_mut().zip(planes) {
                                *value = read_pixel(source, index)?;
                            }
                        }
                    }
                }
                Ok(())
            })?;
            for (lane, (_, _, output)) in planes.iter().enumerate() {
                output.with_mut_slice(|pixels| {
                    for (target, sample) in pixels[first..first + used].iter_mut().zip(band.iter())
                    {
                        *target = sample[lane];
                    }
                });
            }
        }
        Ok(())
    }
}

fn dense_image_error(error: PointwiseError) -> ImageError {
    match error {
        PointwiseError::Image(error) => error,
        // The dense path admits only positive extents, so wrap divisors cannot fail.
        _ => unreachable!("unexpected dense displacement error {error:?}"),
    }
}

pub(super) fn retained<E: 'static>(
    f: &PointwiseFilter<E>,
    inputs: &[Option<SharedImageMap>],
    geometry: ImageGeometry,
    time: f64,
    execution: ProceduralExecution,
    options: PointwiseRenderOptions,
) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
    let original = get(inputs, 0)?;
    let direction = get(inputs, 1)?;
    let distance = get(inputs, 2)?;
    let source = required(original.as_ref())?;
    if source.is_empty()?
        || required(distance.as_ref())?.is_empty()?
        || (f.parameter(0)?.value_at(0, time)? >= 10000.
            && f.parameter(0)?.value_at(1, time)? >= 10000.)
    {
        return Ok(original);
    }
    let output = SharedImageMap::new(geometry);
    let direction = required(direction.as_ref())?.channel(0, false)?;
    let distance = required(distance.as_ref())?.channel(0, false)?;
    let mut sx = f.parameter(0)?.value_at(0, time)?;
    let mut sy = f.parameter(0)?.value_at(1, time)?;
    if sx == 0. {
        sx = 1E-6;
    }
    if sy == 0. {
        sy = 1E-6;
    }
    let dx = f64::from(geometry.width()) / sx / f.parameter(1)?.effective_value_at(time)?;
    let dy = f64::from(geometry.height()) / sy / f.parameter(1)?.effective_value_at(time)?;
    let steps = cast(f.parameter(1)?.effective_value_at(time)?);
    let _ = direction.read(0)?;
    let _ = distance.read(0)?;
    let _ = distance.read(0)?;
    let aa = super::adaptive::text(f, 2, time, "Yes")?;
    let direction = direction.shared_values();
    let distance = distance.shared_values();
    let displacement = Displacement {
        direction: &direction,
        distance: &distance,
        geometry,
        steps,
        dx,
        dy,
        aa,
        regions: false,
    };
    let channels = count(source, options.is_channel_rendering(), false)?;
    let color_channels = channels.min(4);
    displacement.render_colors(source, &output, color_channels, execution)?;
    for ch in color_channels..channels {
        let values = source.channel_with_effects(ch, false)?;
        if values.len() == 1 {
            output.copy_indexed_constant(ch, values.read(0)?, source)?;
        } else {
            let dest = output.allocate_indexed(ch, source)?;
            displacement.render_plane(&values, &dest, execution)?;
            output.seal_channel(ch)?;
        }
    }
    if let Some(region) = source.region_store() {
        let values = region.data(true)?;
        let dest = super::adaptive::floats(
            i32::try_from(values.len()).map_err(|_| crate::ImageError::Overflow)?,
        )?;
        Displacement {
            aa: false,
            regions: true,
            ..displacement
        }
        .render_plane(&values, &dest, execution)?;
        output.set_region_store(Some(SharedFloatStore::new(dest)?));
    }
    Ok(Some(output))
}
