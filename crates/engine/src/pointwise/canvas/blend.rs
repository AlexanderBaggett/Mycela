//! Style blending uses the Environment RGB workers and a separate alpha replacement mode.
use super::{
    raster,
    style::{Blend, BlendRelief},
};
use crate::pointwise::blur;
use crate::{
    EnvironmentBlendMode, ImageError, ImageGeometry, PointwiseError, PointwiseRenderEffects,
    ProceduralExecution, SharedImageMap,
};

pub(super) struct StyleBlend {
    mode: Option<EnvironmentBlendMode>,
    opacity: f32,
    erosion: Option<Erosion>,
}

impl StyleBlend {
    pub(super) fn new<E>(record: &Blend) -> Result<Self, PointwiseError<E>> {
        let opacity = record.opacity as f32 / 100.;
        if record.mode == 0 {
            return Ok(Self {
                mode: None,
                opacity,
                erosion: None,
            });
        }
        let mode = EnvironmentBlendMode::ALL
            .get(record.mode.saturating_sub(1) as usize)
            .copied()
            .ok_or(PointwiseError::UnsupportedCapability("Style blend mode"))?;
        Ok(Self {
            mode: Some(mode),
            opacity,
            erosion: Erosion::new(record),
        })
    }

    pub(super) fn replaces_transparency(&self) -> bool {
        self.mode.is_none()
    }

    pub(super) fn composite(
        &self,
        destination: &mut [f32; 4],
        rgb: [f32; 3],
        coverage: f32,
        replacement_coverage: f32,
        intrinsic: f32,
    ) {
        let mut weight = (1. - (1. - coverage)) * self.opacity;
        if self.mode.is_some() {
            self.blend_rgb(destination, rgb, weight);
            return;
        }
        let old = destination[3];
        let behind = (1. - old) * (1. - weight);
        if weight == 0. && behind == 0. {
            weight = 1.;
        }
        let sum = weight + behind;
        let gain = if sum == 0. { 0. } else { 1. / sum };
        for (bottom, top) in destination.iter_mut().zip(rgb) {
            *bottom = (top * weight + *bottom * behind) * gain;
        }
        let area_transparency = 1. - replacement_coverage;
        if area_transparency < 1. {
            let replacement = old * area_transparency + (1. - area_transparency) * intrinsic;
            destination[3] = replacement * self.opacity + old * (1. - self.opacity);
        }
    }

    fn blend_rgb(&self, destination: &mut [f32; 4], rgb: [f32; 3], mut weight: f32) {
        if let Some(erosion) = &self.erosion {
            weight = erosion.apply(weight, rgb, *destination);
        }
        self.mode
            .expect("RGB blend mode")
            .composite(rgb, destination, weight);
    }
}

pub(super) struct BlendGrid {
    pub(super) geometry: ImageGeometry,
    pub(super) execution: ProceduralExecution,
}

impl BlendGrid {
    pub(super) fn composite<E>(
        &self,
        blend: &StyleBlend,
        record: &Blend,
        destination: &mut [[f32; 4]],
        paint: &mut [[f32; 4]],
        effects: &mut PointwiseRenderEffects<'_, E>,
    ) -> Result<(), PointwiseError<E>> {
        if record.has_spatial_shading() {
            // Both saved relief controls sample the paint's original transparency.
            let source = raster::plane_image(paint, self.geometry, 3)?;
            let [upper, lower] = record.relief;
            self.shade(&upper, &source, paint, true, blend.opacity, effects)?;
            self.shade(&lower, &source, destination, false, blend.opacity, effects)?;
        }
        if record.opacity != 0. {
            for (row, source) in destination
                .chunks_exact_mut(self.geometry.width() as usize)
                .zip(paint.chunks_exact(self.geometry.width() as usize))
            {
                crate::pointwise::check_render_cancelled(effects.cancel_check())?;
                for (pixel, &[r, g, b, transparency]) in row.iter_mut().zip(source) {
                    blend.blend_rgb(pixel, [r, g, b], (1. - transparency) * blend.opacity);
                }
            }
        }
        Ok(())
    }

    fn shade<E>(
        &self,
        settings: &BlendRelief,
        source: &SharedImageMap,
        pixels: &mut [[f32; 4]],
        upper: bool,
        opacity: f32,
        effects: &mut PointwiseRenderEffects<'_, E>,
    ) -> Result<(), PointwiseError<E>> {
        if settings.mode == 1 {
            return self.shade_normals(settings, source, pixels, effects);
        }
        if settings.mode != 2 {
            return Ok(());
        }
        let shifted = self.blur(source, self.geometry, settings.offset_blur, effects)?;
        let values = shifted.materialize(0)?.values();
        let distance = f64::from(settings.distance as f32);
        let [dx, dy] = settings
            .direction
            .map(|direction| crate::framework_cast::integer(direction * distance));
        let width = self.geometry.width();
        let height = self.geometry.height();
        for (y, row) in pixels.chunks_exact_mut(width as usize).enumerate() {
            crate::pointwise::check_render_cancelled(effects.cancel_check())?;
            let sample_y = (i64::from(dy) + y as i64).rem_euclid(i64::from(height)) as usize;
            for (x, pixel) in row.iter_mut().enumerate() {
                let sample_x = (i64::from(dx) + x as i64).rem_euclid(i64::from(width)) as usize;
                let mut light = values[sample_y * width as usize + sample_x];
                if upper {
                    light = 1. - light;
                }
                let shaded = light + (1. - light) * (1. - opacity);
                for component in &mut pixel[..3] {
                    *component *= shaded;
                }
                pixel[3] = shaded;
            }
        }
        Ok(())
    }

    fn shade_normals<E>(
        &self,
        settings: &BlendRelief,
        source: &SharedImageMap,
        pixels: &mut [[f32; 4]],
        effects: &mut PointwiseRenderEffects<'_, E>,
    ) -> Result<(), PointwiseError<E>> {
        let destination = ImageGeometry::new(
            self.geometry
                .width()
                .checked_add(1)
                .ok_or(ImageError::Overflow)?,
            self.geometry
                .height()
                .checked_add(1)
                .ok_or(ImageError::Overflow)?,
        );
        let field = self.blur(source, destination, settings.normal_blur, effects)?;
        let values = field.materialize(0)?.values();
        let width = field.geometry().width() as usize;
        let height = field.geometry().height() as usize;
        let [x, y] = settings.direction;
        let normal = (x * x + y * y + 1.).sqrt();
        let [x, y, z] = [x / normal, y / normal, -1. / normal];
        let factor = settings.depth / 100.;
        let [sx, sy] = [width as f64 * factor, height as f64 * factor];
        for (y_out, row) in pixels
            .chunks_exact_mut(self.geometry.width() as usize)
            .enumerate()
        {
            crate::pointwise::check_render_cancelled(effects.cancel_check())?;
            let here = y_out * width;
            let below = ((y_out + 1) % height) * width;
            for (column, pixel) in row.iter_mut().enumerate() {
                let sample = f64::from(values[here + column]);
                let dx = (f64::from(values[here + (column + 1) % width]) - sample) * sx;
                let dy = (f64::from(values[below + column]) - sample) * sy;
                let light = ((x * dx + y * dy - z) / (dx * dx + dy * dy + 1.).sqrt()) as f32;
                let light = if light > 0. { light } else { 0. };
                for value in &mut pixel[..3] {
                    *value *= light;
                }
            }
        }
        Ok(())
    }

    fn blur<E>(
        &self,
        source: &SharedImageMap,
        geometry: ImageGeometry,
        amount: f64,
        effects: &mut PointwiseRenderEffects<'_, E>,
    ) -> Result<SharedImageMap, PointwiseError<E>> {
        if amount == 0. {
            return Ok(source.clone());
        }
        blur::adaptive(
            source,
            geometry,
            blur::AdaptiveSettings {
                horizontal: true,
                vertical: true,
                radius: amount / 100.,
                factor: 1.,
                periodic: false,
                all_channels: false,
                force_expansion: false,
            },
            self.execution,
            effects,
        )
    }
}

struct Erosion {
    mode: i32,
    amount: f32,
    gain: f32,
}

impl Erosion {
    fn new(record: &Blend) -> Option<Self> {
        if !(record.erosion > 0. && record.erosion_sharpness > 0.) {
            return None;
        }
        Some(Self {
            mode: record.erosion_mode,
            amount: record.erosion / 100.,
            gain: ((f64::from(record.erosion_sharpness) / 100.).min(0.99999)
                * std::f64::consts::PI
                * 0.5)
                .tan() as f32,
        })
    }

    fn apply(&self, opacity: f32, [r, g, b]: [f32; 3], [dr, dg, db, _]: [f32; 4]) -> f32 {
        let upper = (r + g + b) * (1. / 3.);
        let lower = (dr + dg + db) * (1. / 3.);
        let measure = match self.mode {
            0 => upper,
            1 => 1. - upper,
            2 => lower,
            3 => 1. - lower,
            4 => upper - lower + self.amount,
            5 => lower - upper + self.amount,
            _ => 0.,
        };
        let keep = 1. - (measure - (1. - self.amount)) * self.gain;
        if keep <= 0. {
            0.
        } else if keep >= 1. {
            opacity
        } else {
            opacity * keep
        }
    }
}
