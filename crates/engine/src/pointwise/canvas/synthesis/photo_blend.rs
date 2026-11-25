//! Blend Synthesis (xj) prepares a photo crop with Fold Edges or Star Blend.
use super::{
    LayerColor, NumberLinks, NumericLink, Photo, Source, TextureRender, area, filled,
    prepare_photo,
    stamp::{Bilinear, Target, Tile, TileColor},
};
use crate::pointwise::{PointwiseError, PointwiseRenderEffects, SharedImageMap};
use crate::{ImageError, ImageGeometry};
use std::f32::consts::PI;

#[derive(Clone, Debug)]
pub(super) struct PhotoBlend {
    pub(super) animations: super::PropertyAnimations<10>,
    pub(super) source: Source,
    pub(super) rectangle: [f32; 4],
    pub(super) equalize: bool,
    pub(super) strength: f64,
    pub(super) strength_adjustment: Option<NumericLink>,
    pub(super) mode: i32,
    pub(super) stretch: [f64; 2],
    pub(super) edge_feather: [f64; 2],
    pub(super) star_feather: f64,
    pub(super) shaping_links: NumberLinks,
    pub(super) repeat: [i32; 2],
    pub(super) repeat_adjustment: Option<NumericLink>,
}

impl PhotoBlend {
    pub(super) fn render<E: 'static>(
        &self,
        color: &LayerColor,
        request: TextureRender<'_>,
        effects: &mut PointwiseRenderEffects<'_, E>,
    ) -> Result<Vec<[f32; 4]>, PointwiseError<E>> {
        let TextureRender {
            input,
            geometry,
            scale,
            execution,
            ..
        } = request;
        if self.rectangle.iter().any(|value| !value.is_finite()) {
            return Err(PointwiseError::Argument);
        }
        let mut target = filled(area(geometry)?, [0., 0., 0., 1.])?;
        let [mut left, mut top, mut width, mut height] = self.rectangle;
        if width < 0.00001 || height < 0.00001 || !matches!(self.mode, 1 | 2) {
            return Ok(target);
        }
        let extent = [geometry.width(), geometry.height()];
        let factor = super::super::raster::style_scale(scale as f32);
        // The saved pair is X, Y. The original worker reads Y for horizontal
        // repeat and X for vertical repeat.
        let repeats = self.effective_repeat().map(|count| factor * count as f32);
        let [mut columns, mut rows] = [repeats[1], repeats[0]];
        if !columns.is_finite() || !rows.is_finite() {
            return Err(PointwiseError::Argument);
        }
        let tiled = columns > 1. || rows > 1.;
        if tiled {
            columns = columns.round_ties_even().max(1.);
            rows = rows.round_ties_even().max(1.);
            if columns >= i32::MAX as f32 || rows >= i32::MAX as f32 {
                return Err(ImageError::Overflow.into());
            }
        } else {
            if columns < 1. {
                let inset = width * (columns - 1.) / 2.;
                left -= inset;
                width += 2. * inset;
            }
            if rows < 1. {
                let inset = height * (rows - 1.) / 2.;
                top -= inset;
                height += 2. * inset;
            }
        }
        let source_map = match &self.source {
            Source::Missing => return Ok(target),
            Source::Input => input
                .cloned()
                .unwrap_or_else(|| SharedImageMap::new(geometry)),
            Source::Asset(asset) => {
                let size = asset.source_geometry()?;
                let [sw, sh] = [size.width() as f32, size.height() as f32];
                if sw <= 0. || sh <= 0. {
                    return Ok(target);
                }
                let requested = (extent[0] as f32 * 1.2 / (width * columns))
                    .ceil()
                    .max((extent[1] as f32 * 1.2 * sw / (height * rows * sh)).ceil());
                let requested = request.photo_width(requested, size);
                asset.synthesis_source(
                    requested,
                    effects.environment_library(),
                    execution,
                    effects.cancel_check(),
                )?
            }
        };
        let photo = prepare_photo(
            &source_map,
            self.equalize,
            NumericLink::effective_value(self.strength, self.strength_adjustment.as_ref()),
            execution,
            effects,
        )?;
        let photo = Photo::read(&photo)?;
        let origin = [left * photo.size[0] as f32, top * photo.size[1] as f32];
        let crop = [width * photo.size[0] as f32, height * photo.size[1] as f32];
        let pitch = if tiled {
            [extent[0] as f32 / columns, extent[1] as f32 / rows]
        } else {
            extent.map(|side| side as f32)
        };
        let tile_size = pitch.map(|span| span.ceil() as i32);
        let size = tile_size.map(|side| side as usize);
        let color = TileColor::new(color, None)?;
        let [stretch_x, stretch_y, feather_x, feather_y, star_feather] = self.effective_shaping();
        let sample = if self.mode == 2 {
            let star = Star::new(&photo, origin, crop, size, star_feather);
            if star.crop.contains(&0.) {
                return Ok(target);
            }
            Kernel::Star(star)
        } else {
            Kernel::Fold(Fold::new(
                &photo,
                origin,
                crop,
                size,
                [stretch_x, stretch_y],
                [feather_x, feather_y],
            ))
        };
        let mut tile = if tiled {
            filled(
                area(ImageGeometry::new(tile_size[0], tile_size[1]))?,
                [0., 0., 0., 1.],
            )?
        } else {
            std::mem::take(&mut target)
        };
        let cancelled = effects.cancel_check();
        for (y, row) in tile.chunks_exact_mut(size[0]).enumerate() {
            crate::pointwise::check_render_cancelled(cancelled)?;
            for (x, pixel) in row.iter_mut().enumerate() {
                let point = [x as f32, y as f32];
                let [r, g, b, transparency] = match &sample {
                    Kernel::Star(star) => star.sample(point),
                    Kernel::Fold(fold) => fold.sample(point),
                };
                let [r, g, b] = color.apply([r, g, b]);
                *pixel = [r, g, b, transparency];
            }
        }
        if !tiled {
            return Ok(tile);
        }

        let mut weights = filled(target.len(), 0.)?;
        let tile = Tile {
            pixels: &tile,
            mask: None,
            size,
        };
        let mut output = Target {
            pixels: &mut target,
            weights: &mut weights,
            size: extent.map(|side| side as usize),
        };
        let identity = LayerColor::default();
        let mut vertical = 0.;
        for _ in 0..rows as i32 {
            let mut horizontal = 0.;
            for _ in 0..columns as i32 {
                crate::pointwise::check_render_cancelled(cancelled)?;
                tile.place(
                    &mut output,
                    [horizontal, vertical],
                    pitch,
                    &identity,
                    0.,
                    cancelled,
                )?;
                horizontal += pitch[0];
            }
            vertical += pitch[1];
        }
        Ok(target)
    }
}

enum Kernel<'a> {
    Star(Star<'a>),
    Fold(Fold<'a>),
}

impl Photo {
    fn sample(&self, point: [f32; 2], periodic: bool) -> [f32; 4] {
        let bilinear = Bilinear::new(point, self.size, periodic);
        std::array::from_fn(|channel| {
            let plane = &self.pixels[channel];
            if let [constant] = plane.as_slice() {
                *constant
            } else {
                bilinear.read(|index| plane[index])
            }
        })
    }
}

struct Star<'a> {
    photo: &'a Photo,
    origin: [f32; 2],
    crop: [f32; 2],
    size: [f32; 2],
    step: [f32; 2],
    threshold: f32,
    gain: f32,
}

impl<'a> Star<'a> {
    fn new(
        photo: &'a Photo,
        origin: [f32; 2],
        crop: [f32; 2],
        size: [usize; 2],
        feather: f64,
    ) -> Self {
        let crop = crop.map(f32::trunc);
        let size = size.map(|value| value as f32);
        let inverse = 100. - feather;
        let threshold = inverse as f32 / 200.;
        let gain = if threshold == 0.5 {
            f32::MAX
        } else {
            1. / (1. - 2. * threshold)
        };
        Self {
            photo,
            origin: origin.map(f32::trunc),
            crop,
            size,
            step: std::array::from_fn(|axis| crop[axis] / size[axis]),
            threshold,
            gain,
        }
    }

    fn sample(&self, pixel: [f32; 2]) -> [f32; 4] {
        let point = |shift: [f32; 2]| {
            std::array::from_fn(|axis| {
                (pixel[axis] * self.step[axis] + shift[axis]).rem_euclid(self.crop[axis])
                    + self.origin[axis]
            })
        };
        let a = self.photo.sample(point([0.; 2]), true);
        let b = self
            .photo
            .sample(point(self.crop.map(|side| (side / 2.).trunc())), true);
        let angle: [f64; 2] =
            std::array::from_fn(|axis| f64::from(pixel[axis] / self.size[axis] * PI));
        let sine = angle[0].sin() * angle[1].sin();
        let shifted = angle.map(|value| {
            let phase = value + std::f64::consts::FRAC_PI_2;
            if phase >= std::f64::consts::PI {
                phase - std::f64::consts::PI
            } else {
                phase
            }
        });
        let total = sine + shifted[0].sin() * shifted[1].sin();
        let weight = if total == 0. {
            0.
        } else {
            (sine / total) as f32
        };
        let weight = ((weight - self.threshold) * self.gain).clamp(0., 1.);
        std::array::from_fn(|channel| weight * a[channel] + (1. - weight) * b[channel])
    }
}

struct Fold<'a> {
    photo: &'a Photo,
    origin: [f32; 2],
    step: [f32; 2],
    shift: [f32; 2],
    overlap: [f32; 2],
    feather_start: [f32; 2],
}

impl<'a> Fold<'a> {
    fn new(
        photo: &'a Photo,
        origin: [f32; 2],
        crop: [f32; 2],
        size: [usize; 2],
        stretch: [f64; 2],
        feather: [f64; 2],
    ) -> Self {
        let crop = crop.map(|side| side.max(1.));
        let size = size.map(|side| side as f32);
        Self {
            photo,
            origin,
            step: std::array::from_fn(|i| crop[i] / (size[i] * (1. + stretch[i] as f32 / 100.))),
            shift: std::array::from_fn(|i| {
                (f64::from(100. * crop[i]) / (100. + stretch[i])) as f32
            }),
            overlap: std::array::from_fn(|i| (stretch[i] * f64::from(size[i]) / 100.) as f32),
            feather_start: std::array::from_fn(|i| {
                (stretch[i] * (100. - feather[i]) * f64::from(size[i]) / 100. / 100.) as f32
            }),
        }
    }

    fn sample(&self, pixel: [f32; 2]) -> [f32; 4] {
        let origin: [f32; 2] =
            std::array::from_fn(|axis| pixel[axis] * self.step[axis] + self.origin[axis]);
        let feather = |axis| {
            let end = self.overlap[axis];
            let start = self.feather_start[axis];
            if pixel[axis] >= start && end != start {
                (end - pixel[axis]) / (end - start)
            } else {
                1.
            }
        };
        let column = |x| {
            let base = self.photo.sample([x, origin[1]], false);
            if pixel[1] >= self.overlap[1] {
                return base;
            }
            let folded = self.photo.sample([x, origin[1] + self.shift[1]], false);
            overlay(base, folded, (1. - folded[3]) * feather(1))
        };
        let base = column(origin[0]);
        if pixel[0] >= self.overlap[0] {
            return base;
        }
        let folded = column(origin[0] + self.shift[0]);
        let opacity = 1. - (folded[3] + (1. - folded[3]) * (1. - feather(0)));
        overlay(base, folded, opacity)
    }
}

fn overlay(mut base: [f32; 4], above: [f32; 4], opacity: f32) -> [f32; 4] {
    let base_opacity = 1. - base[3];
    let total = opacity + base_opacity * (1. - opacity);
    if total > 0. {
        let reciprocal = 1. / total;
        for channel in 0..3 {
            base[channel] = (above[channel] * opacity
                + base_opacity * base[channel] * (1. - opacity))
                * reciprocal;
        }
    }
    base[3] = 1. - total;
    base
}
