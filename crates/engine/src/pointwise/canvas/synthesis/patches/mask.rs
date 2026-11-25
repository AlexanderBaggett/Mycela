use super::super::{Photo, area, filled, stamp::Bilinear};
use super::{
    Selection, draw,
    path::{Bounds, Path, PlacementOutline, Point},
};
use crate::pointwise::{PointwiseError, PointwiseRenderEffects, blur};
use crate::{
    FloatBuffer, ImageError, ImageGeometry, ProceduralExecution, SeededLattice, SharedImageMap,
};

pub(super) struct PreparedPatch {
    pub(super) signed_extent: Point,
    pub(super) bounds: Bounds,
    pub(super) outline: PlacementOutline,
    pub(super) color: PatchMask,
    pub(super) shadow: Option<PatchMask>,
}

pub(super) struct PatchMask {
    pub(super) pixels: Vec<f32>,
    pub(super) size: [usize; 2],
    pub(super) origin: Point,
    pub(super) scale: Point,
}

impl PreparedPatch {
    pub(super) fn new<E: 'static>(
        selection: &Selection,
        photo: &Photo,
        seed: i32,
        execution: ProceduralExecution,
        effects: &mut PointwiseRenderEffects<'_, E>,
    ) -> Result<Option<Self>, PointwiseError<E>> {
        let Some(path) = Path::from_knots(&selection.knots)? else {
            return Ok(None);
        };
        let original = Bounds {
            min: std::array::from_fn(|i| path.bounds.min[i] * photo.size[i] as f64),
            size: std::array::from_fn(|i| (path.bounds.size[i] * photo.size[i] as f64).abs()),
        };
        let signed_extent = std::array::from_fn(|i| {
            if selection.flipped[i] {
                -path.bounds.size[i]
            } else {
                path.bounds.size[i]
            }
        });
        let size = original.size.map(|extent| extent.ceil().max(1.) as usize);
        let limit = dimensions(photo.size)?;
        let fitted = path.fit(size)?;
        let mut mask = OwnedMask::rasterize(&fitted, size[1], effects.cancel_check())?;
        let feather = f64::from(selection.effective_number(1) as f32 / 2.) / 100.;
        let jitter = selection.effective_number(2) as f32 / 2. / 100.;
        let jitter_frequency = selection.effective_number(3) as i32;
        let jitter_pad: [usize; 2] = photo
            .size
            .map(|side| (side as f32 * jitter).ceil() as usize);
        let jitter_size = [
            size[0]
                .checked_add(jitter_pad[0].checked_mul(2).ok_or(ImageError::Overflow)?)
                .ok_or(ImageError::Overflow)?,
            size[1]
                .checked_add(jitter_pad[1].checked_mul(2).ok_or(ImageError::Overflow)?)
                .ok_or(ImageError::Overflow)?,
        ];
        if jitter > 0. && jitter_frequency > 1 {
            mask = mask.distort(
                jitter_size,
                photo.size,
                jitter,
                jitter_frequency,
                seed,
                effects.cancel_check(),
            )?;
        }
        let feathered_size = extent(blur::periodic_extent(
            dimensions(jitter_size)?,
            limit,
            feather,
            true,
        ));
        if feather > 0. {
            mask = mask.feather(limit, feather, execution, effects)?;
        }
        let bounds = original.resized(size, feathered_size);
        let mut color = mask.padded(2, 0.)?;
        color.invert();
        let shadow = [4, 5].map(|slot| selection.effective_number(slot) as f32);
        let (coverage, shadow) = if shadow[0] > 0. {
            let shadow_radius = f64::from(shadow[1]) / 100.;
            let shadow_size = extent(blur::periodic_extent(
                dimensions(feathered_size)?,
                limit,
                shadow_radius,
                true,
            ));
            let shadow_bounds = original.resized(size, shadow_size);
            let mask = color
                .shadow(photo, bounds, limit, shadow, execution, effects)?
                .mapping(shadow_bounds);
            (shadow_bounds, Some(mask))
        } else {
            (bounds, None)
        };
        let Some(outline) = PlacementOutline::new(coverage, photo.size)? else {
            return Ok(None);
        };
        Ok(Some(Self {
            signed_extent,
            bounds: coverage,
            outline,
            color: color.mapping(bounds),
            shadow,
        }))
    }
}

struct OwnedMask {
    size: [usize; 2],
    pixels: Vec<f32>,
}

impl OwnedMask {
    fn rasterize<E>(
        path: &Path,
        height: usize,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Self, PointwiseError<E>> {
        let left_bound = path.bounds.min[0].floor();
        let right_bound = (path.bounds.min[0] + path.bounds.size[0]).ceil();
        let size = [(right_bound - left_bound) as usize, height];
        let mut pixels = filled(area(dimensions(size)?)?, 0.)?;
        let mut crossings = Vec::new();
        let step = path.bounds.size[1] / (path.bounds.size[1] - 1.);
        for (y, row) in pixels.chunks_exact_mut(size[0]).enumerate() {
            crate::pointwise::check_render_cancelled(cancelled)?;
            path.crossings(path.bounds.min[1] + y as f64 * step, &mut crossings);
            for span in crossings.chunks_exact(2) {
                // The width uses the enclosing integer bounds. The destination
                // starts at the truncated crossing, which can fall just below zero.
                let width = span[1].ceil() as i64 - span[0].floor() as i64;
                let start = span[0] as i64;
                for x in start.max(0)..(start + width).min(size[0] as i64) {
                    row[x as usize] = span_coverage(x - start, width, [span[0], span[1]]) as f32;
                }
            }
        }
        Ok(Self { size, pixels })
    }

    fn distort<E>(
        self,
        size: [usize; 2],
        photo: [usize; 2],
        amount: f32,
        frequency: i32,
        seed: i32,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Self, PointwiseError<E>> {
        let old = self.padded_to(size, 0.)?;
        let lattice = SeededLattice::new(seed)?;
        let field_size = [
            frequency as usize,
            (frequency as f32 * photo[1] as f32 / photo[0] as f32).ceil() as usize,
        ];
        let mut field: [Vec<f32>; 2] = std::array::from_fn(|_| Vec::new());
        let length = area(dimensions(field_size)?)?;
        for plane in &mut field {
            plane
                .try_reserve_exact(length)
                .map_err(|_| ImageError::Allocation)?;
        }
        for y in 0..field_size[1] {
            for x in 0..field_size[0] {
                for (channel, plane) in field.iter_mut().enumerate() {
                    plane.push(
                        amount
                            - lattice.sample3(x as i32, y as i32, channel as i32) * (amount * 2.),
                    );
                }
            }
        }
        let factor: [f32; 2] =
            std::array::from_fn(|axis| field_size[axis] as f32 / photo[axis] as f32);
        let center = [
            ((photo[0] as i64 - size[0] as i64) / 2) as f32 * factor[0],
            ((photo[1] as i64 - size[1] as i64) / 2) as f32 * factor[0],
        ];
        let mut pixels = filled(area(dimensions(size)?)?, 0.)?;
        for (y, row) in pixels.chunks_exact_mut(size[0]).enumerate() {
            crate::pointwise::check_render_cancelled(cancelled)?;
            for (x, value) in row.iter_mut().enumerate() {
                let sample = Bilinear::new(
                    [
                        x as f32 * factor[0] + center[0],
                        y as f32 * factor[1] + center[1],
                    ],
                    field_size,
                    false,
                );
                let px = x as f32 + sample.read(|index| field[0][index]) * photo[0] as f32;
                let py = y as f32 + sample.read(|index| field[1][index]) * photo[1] as f32;
                if px >= 0. && px < size[0] as f32 && py >= 0. && py < size[1] as f32 {
                    *value = Bilinear::new([px, py], size, false).read(|index| old.pixels[index]);
                }
            }
        }
        Ok(Self { size, pixels })
    }

    fn padded(self, border: usize, fill: f32) -> Result<Self, ImageError> {
        let size = [
            self.size[0]
                .checked_add(2 * border)
                .ok_or(ImageError::Overflow)?,
            self.size[1]
                .checked_add(2 * border)
                .ok_or(ImageError::Overflow)?,
        ];
        self.padded_to(size, fill)
    }

    fn padded_to(self, size: [usize; 2], fill: f32) -> Result<Self, ImageError> {
        if size == self.size {
            return Ok(self);
        }
        let mut pixels = filled(area(dimensions(size)?)?, fill)?;
        let offset = [(size[0] - self.size[0]) / 2, (size[1] - self.size[1]) / 2];
        for (y, row) in self.pixels.chunks_exact(self.size[0]).enumerate() {
            let start = offset[0] + (y + offset[1]) * size[0];
            pixels[start..start + self.size[0]].copy_from_slice(row);
        }
        Ok(Self { size, pixels })
    }

    fn feather<E: 'static>(
        self,
        limit: ImageGeometry,
        radius: f64,
        execution: ProceduralExecution,
        effects: &mut PointwiseRenderEffects<'_, E>,
    ) -> Result<Self, PointwiseError<E>> {
        let image = SharedImageMap::new(dimensions(self.size)?);
        image.set_buffer(0, Some(&FloatBuffer::new(self.pixels)))?;
        let blurred = blur::adaptive(
            &image,
            limit,
            blur::AdaptiveSettings {
                horizontal: true,
                vertical: true,
                radius,
                factor: 1.,
                periodic: true,
                all_channels: false,
                force_expansion: true,
            },
            execution,
            effects,
        )?;
        Ok(Self {
            pixels: blurred.materialize(0)?.values(),
            size: extent(blurred.geometry()),
        })
    }

    fn invert(&mut self) {
        for pixel in &mut self.pixels {
            *pixel = 1. - *pixel;
        }
    }

    fn mapping(self, bounds: Bounds) -> PatchMask {
        let origin = bounds.min.map(|value| value - 2.);
        let scale =
            std::array::from_fn(|axis| (self.size[axis] - 4) as f64 / bounds.size[axis].max(1.));
        PatchMask {
            pixels: self.pixels,
            size: self.size,
            origin,
            scale,
        }
    }

    fn shadow<E: 'static>(
        &self,
        photo: &Photo,
        bounds: Bounds,
        limit: ImageGeometry,
        [strength, feather]: [f32; 2],
        execution: ProceduralExecution,
        effects: &mut PointwiseRenderEffects<'_, E>,
    ) -> Result<Self, PointwiseError<E>> {
        let mut pixels = filled(self.pixels.len(), 1.)?;
        let [w, h] = self.size;
        let step: Point = std::array::from_fn(|i| bounds.size[i] / (self.size[i] - 4) as f64);
        let origin: Point = std::array::from_fn(|i| (bounds.min[i] + 0.5) * step[i]);
        let mut y_sample = origin[1];
        let cancelled = effects.cancel_check();
        for y in 2..h - 2 {
            crate::pointwise::check_render_cancelled(cancelled)?;
            let mut x_sample = origin[0];
            for x in 2..w - 2 {
                let index = x + y * w;
                let alpha = draw::sample([x_sample, y_sample], photo.size, &photo.pixels[3], true);
                pixels[index] = alpha + self.pixels[index] * (1. - alpha);
                x_sample += step[0];
            }
            y_sample += step[1];
        }
        let mut shadow = Self {
            size: self.size,
            pixels,
        };
        shadow.invert();
        shadow = shadow.feather(limit, f64::from(feather / 100.), execution, effects)?;
        let [minimum, threshold] = if strength < 50. {
            [1. - strength / 50., 0.]
        } else {
            [0., (strength - 50.) * 9. / 500.]
        };
        shadow.invert();
        for value in &mut shadow.pixels {
            let fraction = ((*value - threshold) / (1. - threshold)).clamp(0., 1.);
            *value = (1. - fraction) * minimum + fraction;
        }
        Ok(shadow)
    }
}

fn span_coverage(column: i64, width: i64, span: Point) -> f64 {
    if column == 0 {
        1. - (span[0] - span[0].floor())
    } else if column == width - 1 {
        let weight = span[1] - span[1].floor();
        if weight < f64::from(0.0001_f32) {
            1.
        } else {
            weight
        }
    } else {
        1.
    }
}

fn dimensions(size: [usize; 2]) -> Result<ImageGeometry, ImageError> {
    let width = i32::try_from(size[0]).map_err(|_| ImageError::Overflow)?;
    let height = i32::try_from(size[1]).map_err(|_| ImageError::Overflow)?;
    Ok(ImageGeometry::new(width, height))
}

fn extent(geometry: ImageGeometry) -> [usize; 2] {
    [geometry.width() as usize, geometry.height() as usize]
}
