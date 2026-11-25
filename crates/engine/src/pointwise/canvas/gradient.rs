//! Scalar gradient fields shared by the paint and Area-mask records.
use super::{area, raster, style::GradientField};
use crate::{
    ImageError, ImageGeometry, PointwiseError, PointwiseRenderEffects, ProceduralExecution,
};

impl GradientField {
    pub(super) fn render<E: 'static>(
        &self,
        prepared: Option<&[f32]>,
        geometry: ImageGeometry,
        scale: f32,
        execution: ProceduralExecution,
        effects: &mut PointwiseRenderEffects<'_, E>,
    ) -> Result<Vec<f32>, PointwiseError<E>> {
        let w = geometry.width();
        let h = geometry.height();
        let len = usize::try_from(geometry.area()).map_err(|_| ImageError::Overflow)?;
        let cancelled = effects.cancel_check();
        let mut values = if self.kind == 0 {
            match prepared {
                Some(field) => field.to_vec(),
                None => raster::filled(len, 1.)?,
            }
        } else {
            let mut array = raster::filled(len, 0.)?;
            match self.kind {
                1 => linear(&mut array, w, h, self.angle, cancelled)?,
                2 | 4 => radial(
                    &mut array,
                    w,
                    h,
                    self.kind == 4,
                    self.center(w, h, scale),
                    cancelled,
                )?,
                3 => {
                    // Original mirrored square-pip ramp. The last write at a
                    // center seam wins on small and odd grids.
                    for y in 0..=h / 2 {
                        crate::pointwise::check_render_cancelled(cancelled)?;
                        let boundary = y * w / h;
                        for x in boundary..=w / 2 {
                            mirror(&mut array, w, h, x, y, y as f32 / (h as f32 / 2.));
                        }
                        for x in 0..boundary {
                            mirror(&mut array, w, h, x, y, x as f32 / (w as f32 / 2.));
                        }
                    }
                }
                _ => return Err(PointwiseError::Argument),
            }
            array
        };
        area::remap(&mut values, geometry, &self.curve, cancelled)?;
        if !matches!(self.kind, 0 | 1) && self.offset != [0.; 2] {
            values = if self.wrap {
                area::translate(
                    values,
                    geometry,
                    self.offset,
                    100. * scale,
                    execution,
                    cancelled,
                )?
            } else if self.kind == 3 {
                offset_with_zeros(values, w, h, self.offset.map(|v| v / f64::from(scale)))?
            } else {
                values
            };
        }
        area::soft_blur(values, geometry, self.blur, scale, execution, effects)
    }

    fn center(&self, w: i32, h: i32, scale: f32) -> [f64; 2] {
        if self.wrap || self.offset == [0.; 2] {
            [f64::from(w / 2), f64::from(h / 2)]
        } else {
            [
                (self.offset[0] / f64::from(scale) * (f64::from(w) / 100.) + f64::from(w) / 2.)
                    .round_ties_even(),
                (-self.offset[1] / f64::from(scale) * (f64::from(h) / 100.) + f64::from(h) / 2.)
                    .round_ties_even(),
            ]
        }
    }
}

fn linear<E>(
    values: &mut [f32],
    w: i32,
    h: i32,
    angle: f64,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), PointwiseError<E>> {
    let (quadrant, theta) = if (0.0..=90.0).contains(&angle) {
        (0, angle)
    } else if (-360.0..=-270.0).contains(&angle) {
        (0, 360. + angle)
    } else if (90.0..=180.0).contains(&angle) {
        (1, 180. - angle)
    } else if (-270.0..=-180.0).contains(&angle) {
        (1, -angle - 180.)
    } else if (180.0..=270.0).contains(&angle) {
        (2, 270. - angle)
    } else if (-180.0..=-90.0).contains(&angle) {
        (2, -angle - 90.)
    } else if (270.0..=360.0).contains(&angle) {
        (3, angle - 270.)
    } else if (-90.0..=0.0).contains(&angle) {
        (3, 90. + angle)
    } else {
        return Ok(());
    };
    let radians = theta * (std::f64::consts::PI / 180.);
    let sn = radians.sin();
    let cs = radians.cos();
    let norm = if quadrant < 2 {
        1. / (f64::from(h) * sn + f64::from(w) * cs) as f32
    } else {
        1. / (f64::from(h) * cs + f64::from(w) * sn) as f32
    };
    let sn = sn as f32;
    let cs = cs as f32;
    for (y, row) in values.chunks_exact_mut(w as usize).enumerate() {
        crate::pointwise::check_render_cancelled(cancelled)?;
        let y = y as i32;
        let start = if quadrant < 2 {
            (h - y) as f32 * sn
        } else {
            y as f32 * cs
        };
        for (x, value) in row.iter_mut().enumerate() {
            let x = x as i32;
            let column = if matches!(quadrant, 1 | 2) { w - x } else { x } as f32;
            *value = (start + column * if quadrant < 2 { cs } else { sn }) * norm;
        }
    }
    Ok(())
}

fn radial<E>(
    values: &mut [f32],
    w: i32,
    h: i32,
    angular: bool,
    [cx, cy]: [f64; 2],
    cancelled: &dyn Fn() -> bool,
) -> Result<(), PointwiseError<E>> {
    let half_width = w as f32 / 2.;
    let half_height = h as f32 / 2.;
    let radius = f64::from(half_width * half_width + half_height * half_height).sqrt() as f32;
    let scale = if angular {
        1. / (2. * std::f32::consts::PI)
    } else {
        1. / radius
    };
    for (y, row) in values.chunks_exact_mut(w as usize).enumerate() {
        crate::pointwise::check_render_cancelled(cancelled)?;
        let dy = y as f64 - cy;
        for (x, value) in row.iter_mut().enumerate() {
            let dx = x as f64 - cx;
            *value = if angular {
                let angle = dy.abs().atan2(dx) as f32 * scale;
                if dy < 0. { 1. - angle } else { angle }
            } else {
                1. - (dx * dx + dy * dy).sqrt() as f32 * scale
            };
        }
    }
    Ok(())
}

fn mirror(array: &mut [f32], w: i32, h: i32, x: i32, y: i32, value: f32) {
    for row in [y, h - 1 - y] {
        for column in [x, w - 1 - x] {
            array[(row * w + column) as usize] = value;
        }
    }
}

fn offset_with_zeros(
    values: Vec<f32>,
    w: i32,
    h: i32,
    [x, y]: [f64; 2],
) -> Result<Vec<f32>, ImageError> {
    let dx = (x * f64::from(w) / 100.).round_ties_even() as i32;
    let dy = ((y * f64::from(h) / 100.).round_ties_even() as i32).saturating_neg();
    let mut shifted = raster::filled(values.len(), 0.)?;
    for row in dy.max(0)..h.min(h.saturating_add(dy)) {
        for col in dx.max(0)..w.min(w.saturating_add(dx)) {
            shifted[(row * w + col) as usize] = values[((row - dy) * w + col - dx) as usize];
        }
    }
    Ok(shifted)
}
