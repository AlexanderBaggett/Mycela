//! Repeating Pattern (awf/wi) builds a warped tile, then distributes it periodically.
use super::{LayerColor, NumericLink, Source, TextureRender, area, order, prepare_photo, stamp};
use crate::pointwise::{PointwiseError, PointwiseRenderEffects, SharedImageMap, blur};
use crate::{FloatBuffer, ImageError, ImageGeometry, SeededLattice, seeded_random::SeededRandom};
use std::borrow::Cow;

#[derive(Clone, Debug)]
pub(super) struct Pattern {
    pub(super) animations: super::PropertyAnimations<5>,
    pub(super) source: Source,
    pub(super) columns: usize,
    pub(super) rows: usize,
    pub(super) points: Box<[[f32; 2]]>,
    pub(super) equalize: bool,
    pub(super) strength: f64,
    pub(super) strength_adjustment: Option<NumericLink>,
    pub(super) feather: f64,
    pub(super) feather_adjustment: Option<NumericLink>,
    pub(super) frequency: i32,
    pub(super) frequency_adjustment: Option<NumericLink>,
}

impl Pattern {
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
            seed,
            execution,
            ..
        } = request;
        let mut pixels = super::filled(geometry.area() as usize, [0., 0., 0., 1.])?;
        let frequency = self.effective_frequency();
        if frequency == 0 || self.columns < 2 || self.rows < 2 {
            return Ok(pixels);
        }
        let fallback = (matches!(self.source, Source::Input) && input.is_none())
            .then(|| SharedImageMap::new(geometry));
        let input = input.or(fallback.as_ref());
        let source_size = match &self.source {
            Source::Input => input.ok_or(PointwiseError::NullReference)?.geometry(),
            Source::Asset(asset) => asset.source_geometry()?,
            Source::Missing => return Ok(pixels),
        };
        if source_size.width() <= 0 || source_size.height() <= 0 {
            return Ok(pixels);
        }
        let (turned, step) = self.grid_step();
        if step.iter().any(|value| !value.is_finite()) {
            return Err(PointwiseError::Argument);
        }
        if step.iter().any(|value| *value < 0.0001) {
            return Ok(pixels);
        }
        let extent = [geometry.width() as f32, geometry.height() as f32];
        let mut cell = source_step(step, turned, source_size);
        let frequency = (frequency as f32 * super::super::raster::style_scale(scale as f32))
            .round_ties_even()
            .max(1.);
        let repeats = if cell[0] >= cell[1] {
            [
                frequency,
                (extent[1] * frequency * cell[0] / (extent[0] * cell[1]))
                    .round_ties_even()
                    .max(1.),
            ]
        } else {
            [
                (extent[0] * frequency * cell[1] / (extent[1] * cell[0]))
                    .round_ties_even()
                    .max(1.),
                frequency,
            ]
        };
        if repeats
            .iter()
            .any(|value| !value.is_finite() || *value >= i32::MAX as f32)
        {
            return Err(ImageError::Overflow.into());
        }
        let pitch = [extent[0] / repeats[0], extent[1] / repeats[1]];
        let source = match &self.source {
            Source::Input => Cow::Borrowed(input.ok_or(PointwiseError::NullReference)?),
            Source::Asset(asset) => {
                let [sw, sh] = [source_size.width() as f32, source_size.height() as f32];
                let width = if turned {
                    (pitch[1] * 1.2 / step[1])
                        .ceil()
                        .max((pitch[0] * 1.2 * sw / (step[0] * sh)).ceil())
                } else {
                    (pitch[0] * 1.2 / step[0])
                        .ceil()
                        .max((pitch[1] * 1.2 * sw / (step[1] * sh)).ceil())
                };
                let width = request.photo_width(width, source_size);
                let image = asset.synthesis_source(
                    width,
                    effects.environment_library(),
                    execution,
                    effects.cancel_check(),
                )?;
                cell = source_step(step, turned, image.geometry());
                Cow::Owned(image)
            }
            Source::Missing => return Ok(pixels),
        };
        let strength =
            NumericLink::effective_value(self.strength, self.strength_adjustment.as_ref());
        let photo = prepare_photo(&source, self.equalize, strength, execution, effects)?;
        let segments = [self.columns - 1, self.rows - 1];
        let tile_axis = |axis: usize| {
            let cells = i32::try_from(segments[axis]).map_err(|_| ImageError::Overflow)?;
            (cell[axis].ceil() as i32)
                .checked_mul(cells)
                .ok_or(ImageError::Overflow)
        };
        let unpadded = ImageGeometry::new(tile_axis(0)?, tile_axis(1)?);
        let unpadded_area = area(unpadded)?;
        let photo_size = photo.geometry();
        let limit = if turned {
            ImageGeometry::new(photo_size.height(), photo_size.width())
        } else {
            photo_size
        };
        let feather = self.effective_feather() / 200.;
        if !feather.is_finite() || feather < 0. {
            return Err(PointwiseError::Argument);
        }
        let tile_size = blur::periodic_extent(unpadded, limit, feather, false);
        let tile = self.warp_tile(&photo, tile_size, unpadded, turned, effects.cancel_check())?;
        let mask = if feather > 0. {
            let image = SharedImageMap::new(unpadded);
            image.set_buffer(
                0,
                Some(&FloatBuffer::new(super::filled(unpadded_area, 1.)?)),
            )?;
            let blurred = blur::adaptive(
                &image,
                limit,
                blur::AdaptiveSettings {
                    horizontal: true,
                    vertical: true,
                    radius: feather,
                    factor: 1.,
                    periodic: true,
                    all_channels: false,
                    force_expansion: false,
                },
                execution,
                effects,
            )?;
            // The original expansion retains any already oversized axis.
            // Stamping reads the mask from the start of that flat buffer at
            // the warp's stride, which can be smaller than the mask's stride.
            let samples = blurred.materialize(0)?.values();
            if samples.len() < area(tile_size)? {
                return Err(PointwiseError::Argument);
            }
            Some(samples)
        } else {
            None
        };
        let dimensions = [tile_size.width() as usize, tile_size.height() as usize];
        let pitch_span = [pitch[0] * segments[0] as f32, pitch[1] * segments[1] as f32];
        let original = [unpadded.width() as f32, unpadded.height() as f32];
        let padding = std::array::from_fn::<_, 2, _>(|axis| {
            (dimensions[axis] as f32 - original[axis]) / 2. * (pitch_span[axis] / original[axis])
        });
        let span = std::array::from_fn(|axis| pitch_span[axis] + padding[axis] * 2.);
        let counts = repeats.map(|count| count as i32);
        let lattice = SeededLattice::new(seed)?;
        let mut random =
            SeededRandom::from_seed(seed.wrapping_mul(4)).map_err(|_| ImageError::Overflow)?;
        let mut order = Vec::new();
        let count = (counts[0] as usize)
            .checked_mul(counts[1] as usize)
            .ok_or(ImageError::Overflow)?;
        order
            .try_reserve_exact(count)
            .map_err(|_| ImageError::Allocation)?;
        for x in 0..counts[0] {
            for y in 0..counts[1] {
                order.push(((lattice.sample2(x, y) * 10000.) as i32, [x, y]));
            }
        }
        order::sort_by_key(&mut order, |placement| placement.0);
        let mut weights = super::filled(pixels.len(), 0.)?;
        let tile = stamp::Tile {
            pixels: &tile,
            mask: mask.as_deref(),
            size: dimensions,
        };
        let mut target = stamp::Target {
            pixels: &mut pixels,
            weights: &mut weights,
            size: extent.map(|value| value as usize),
        };
        let cancelled = effects.cancel_check();
        for (_, position) in order {
            crate::pointwise::check_render_cancelled(cancelled)?;
            let origin =
                std::array::from_fn(|axis| position[axis] as f32 * pitch[axis] - padding[axis]);
            tile.place(&mut target, origin, span, color, random.sample(), cancelled)?;
        }
        Ok(pixels)
    }

    fn point(&self, x: usize, y: usize) -> [f32; 2] {
        self.points[x * self.rows + y]
    }

    fn grid_step(&self) -> (bool, [f32; 2]) {
        let mut horizontal = [0.; 2];
        let mut vertical = [0.; 2];
        for y in 0..self.rows {
            for x in 0..self.columns - 1 {
                let (a, b) = (self.point(x, y), self.point(x + 1, y));
                for axis in 0..2 {
                    horizontal[axis] += (b[axis] - a[axis]).abs();
                }
            }
        }
        for x in 0..self.columns {
            for y in 0..self.rows - 1 {
                let (a, b) = (self.point(x, y), self.point(x, y + 1));
                for axis in 0..2 {
                    vertical[axis] += (b[axis] - a[axis]).abs();
                }
            }
        }
        let turned = horizontal[0] < horizontal[1] && vertical[1] < vertical[0];
        let axis = usize::from(turned);
        (
            turned,
            [
                horizontal[axis] / ((self.columns - 1) * self.rows) as f32,
                vertical[1 - axis] / ((self.rows - 1) * self.columns) as f32,
            ],
        )
    }

    fn warp_tile<E>(
        &self,
        photo: &SharedImageMap,
        size: ImageGeometry,
        unpadded: ImageGeometry,
        turned: bool,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<[f32; 4]>, PointwiseError<E>> {
        let mut center = [0.; 2];
        for point in &self.points {
            center[0] += point[0];
            center[1] += point[1];
        }
        center = center.map(|value| value / self.points.len() as f32);
        let mut scale = [
            size.width() as f32 / unpadded.width() as f32,
            size.height() as f32 / unpadded.height() as f32,
        ];
        if turned {
            scale.swap(0, 1);
        }
        let point = |x, y| {
            let point = self.point(x, y);
            std::array::from_fn(|axis| {
                ((point[axis] - center[axis]) * scale[axis] + center[axis]).clamp(0., 1.)
            })
        };
        let dimensions = [size.width() as usize, size.height() as usize];
        let cell = [
            dimensions[0].div_ceil(self.columns - 1),
            dimensions[1].div_ceil(self.rows - 1),
        ];
        let mut tile = super::filled(area(size)?, [0., 0., 0., 1.])?;
        let origin = super::super::raster::read_planes(photo)?;
        let source_size = [
            photo.geometry().width() as usize,
            photo.geometry().height() as usize,
        ];
        for y in 0..self.rows - 1 {
            for x in 0..self.columns - 1 {
                stamp::warp(
                    &origin,
                    source_size,
                    &mut tile,
                    dimensions,
                    stamp::Cell {
                        origin: [x * cell[0], y * cell[1]],
                        size: cell,
                        corners: [
                            point(x, y),
                            point(x + 1, y),
                            point(x, y + 1),
                            point(x + 1, y + 1),
                        ],
                    },
                    cancelled,
                )?;
            }
        }
        Ok(tile)
    }
}

fn source_step(step: [f32; 2], turned: bool, size: ImageGeometry) -> [f32; 2] {
    let mut size = [size.width() as f32, size.height() as f32];
    if turned {
        size.swap(0, 1);
    }
    [step[0] * size[0], step[1] * size[1]]
}
