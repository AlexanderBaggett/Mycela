//! Patches (aki) prepares source masks once and places them over a periodic image.
mod draw;
mod manual_preview;
mod mask;
mod path;

use super::{
    LayerColor, NumericLink, Photo, PropertyAnimations, Source, TextureRender, area, filled, order,
    prepare_photo,
};
use crate::pointwise::{PointwiseError, PointwiseRenderEffects, SharedImageMap};
use crate::{ImageError, ImageGeometry, SeededLattice, seeded_random::SeededRandom};
use mask::PreparedPatch;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub(super) struct Patches {
    pub(super) animations: super::PropertyAnimations<4>,
    pub(super) source: Source,
    pub(super) equalize: bool,
    pub(super) strength: f64,
    pub(super) strength_adjustment: Option<NumericLink>,
    pub(super) selections: Vec<Selection>,
    // Switching off graph input with no remembered photo parks this session’s geometry.
    pub(super) selections_parked: bool,
    pub(super) automatic: Automatic,
    pub(super) manual: Vec<Placement>,
    pub(super) repeat: [i32; 2],
    pub(super) repeat_adjustment: Option<NumericLink>,
}

#[derive(Clone, Debug)]
pub(super) struct Selection {
    pub(super) animations: super::PropertyAnimations<6>,
    pub(super) frequency: i32,
    pub(super) number_links: super::NumberLinks,
    pub(super) feather: f32,
    pub(super) jitter: f32,
    pub(super) jitter_frequency: i32,
    pub(super) shadow_strength: f32,
    pub(super) shadow_feather: f32,
    pub(super) knots: Vec<Knot>,
    pub(super) flipped: [bool; 2],
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Knot {
    pub(super) point: [f32; 2],
    pub(super) incoming: [f32; 2],
    pub(super) outgoing: [f32; 2],
    pub(super) mode: i32,
    pub(super) flags: [bool; 3],
}

#[derive(Clone, Debug)]
pub(super) struct Automatic {
    pub(super) enabled: bool,
    pub(super) frequency: [i32; 2],
    pub(super) frequency_adjustment: Option<NumericLink>,
    pub(super) offset: [f32; 2],
    pub(super) offset_adjustment: Option<NumericLink>,
    pub(super) density: f32,
    pub(super) density_adjustment: Option<NumericLink>,
    pub(super) slide: f32,
    pub(super) slide_adjustment: Option<NumericLink>,
    pub(super) position_jitter: [f32; 2],
    pub(super) position_jitter_adjustments: [Option<NumericLink>; 2],
    pub(super) scale: f32,
    pub(super) scale_adjustment: Option<NumericLink>,
    pub(super) scale_jitter: f32,
    pub(super) scale_jitter_adjustment: Option<NumericLink>,
    pub(super) angle: f32,
    pub(super) angle_adjustment: Option<NumericLink>,
    pub(super) angle_jitter: f32,
    pub(super) angle_jitter_adjustment: Option<NumericLink>,
    pub(super) switcher: i32,
    pub(super) switcher_adjustment: Option<NumericLink>,
    /// Original property order from grid Frequency through Patch Switcher.
    pub(super) animations: PropertyAnimations<11>,
}

pub(super) const PATCH_SWITCHERS: [&str; 4] = ["Random", "Rows", "Columns", "Staggered"];

#[derive(Clone, Debug)]
pub(super) struct Placement {
    pub(super) identity: Arc<()>,
    pub(super) point: [f32; 2],
    pub(super) patch: i32,
    pub(super) sequence: i32,
    pub(super) angle: f32,
    pub(super) size: [f32; 2],
}

pub(super) fn selection_outline(selection: &Selection) -> Option<Vec<[f32; 2]>> {
    let path = path::Path::from_knots(&selection.knots).ok()??;
    Some(
        path.outline()
            .iter()
            .map(|point| point.map(|value| value as f32))
            .collect(),
    )
}

pub(super) fn selection_size(selection: &Selection) -> Option<[f32; 2]> {
    let (_, size) = selection_bounds(selection)?;
    Some(size.map(|value| value as f32))
}

pub(super) fn selection_bounds(selection: &Selection) -> Option<([f64; 2], [f64; 2])> {
    let bounds = path::Path::from_knots(&selection.knots).ok()??.bounds;
    Some((bounds.min, bounds.size))
}

pub(super) fn selection_sampling_margins(
    selection: &Selection,
    size: [f64; 2],
    virtual_source: Option<ImageGeometry>,
) -> Option<[f64; 2]> {
    let [feather, jitter] = [1, 2].map(|index| selection.effective_number(index) as f32 / 2.);
    if feather == 0. && jitter == 0. {
        return Some([0.; 2]);
    }
    let source = virtual_source?;
    let dimensions = [source.width(), source.height()].map(f64::from);
    let bounds = path::Bounds {
        min: [0.; 2],
        size: [0, 1].map(|axis| size[axis] * dimensions[axis]),
    };
    let expanded = expanded_sampling_bounds(bounds, source, feather, jitter);
    let margins =
        [0, 1].map(|axis| (expanded.size[axis] - bounds.size[axis]) / 2. / dimensions[axis]);
    margins
        .into_iter()
        .all(|margin| (0. ..0.5).contains(&margin))
        .then_some(margins)
}

pub(super) fn selection_handles(selection: &Selection, index: usize) -> Option<[[f32; 2]; 2]> {
    path::Path::handles(&selection.knots, index)
}

pub(super) fn insert_selection_point(
    selection: &mut Selection,
    index: usize,
    t: f64,
) -> Option<usize> {
    path::Path::split(&mut selection.knots, index, t)
}

#[derive(Clone, Copy)]
pub(super) struct PatchShapeLimit {
    bounds: path::Bounds,
    source: ImageGeometry,
    other: f32,
    jitter: bool,
}

impl PatchShapeLimit {
    pub(super) fn clamp(self, value: f32) -> f32 {
        let extents = [self.source.width(), self.source.height()];
        let bounds = path::Bounds {
            min: std::array::from_fn(|axis| self.bounds.min[axis] * f64::from(extents[axis])),
            size: std::array::from_fn(|axis| self.bounds.size[axis] * f64::from(extents[axis])),
        };
        let fits = |half: f32| {
            let [feather, jitter] = if self.jitter {
                [self.other, half]
            } else {
                [half, self.other]
            };
            let expanded = expanded_sampling_bounds(bounds, self.source, feather, jitter);
            (0..2).all(|axis| {
                expanded.min[axis] >= -1.
                    && expanded.min[axis] + expanded.size[axis] <= f64::from(extents[axis]) + 2.
            })
        };
        let mut half = value / 2.;
        if !fits(half) {
            loop {
                half = crate::round_decimal_places(f64::from(half - 0.05), 3).unwrap_or(0.) as f32;
                if half <= 0. || fits(half) {
                    break;
                }
            }
        }
        half.max(0.) * 2.
    }
}

/// Expands a contour's source-texel bounds using the two half-percent controls.
fn expanded_sampling_bounds(
    bounds: path::Bounds,
    source: ImageGeometry,
    feather: f32,
    jitter: f32,
) -> path::Bounds {
    let size = bounds.size.map(|value| value.ceil().max(1.) as usize);
    let extents = [source.width(), source.height()];
    let [width, height] = [0, 1].map(|axis| {
        let padding = (extents[axis] as f32 * (jitter / 100.)).ceil() as usize;
        size[axis]
            .saturating_add(padding.saturating_mul(2))
            .min(i32::MAX as usize) as i32
    });
    let feathered = crate::pointwise::blur::periodic_extent(
        ImageGeometry::new(width, height),
        source,
        f64::from(feather) / 100.,
        true,
    );
    bounds.resized(
        size,
        [feathered.width() as usize, feathered.height() as usize],
    )
}

impl Selection {
    /// Fits a template to the source using its Feather and Shape Jitter sampling margins.
    /// `source` uses the source editor's virtual texels (1000 per raster pixel).
    pub(super) fn fit_new_shape(&mut self, source: Option<ImageGeometry>) -> Option<()> {
        let bounds = path::Path::from_knots(&self.knots).ok()??.bounds;
        let [feather, jitter] = [self.feather / 2., self.jitter / 2.];
        if feather == 0. && jitter == 0. {
            // The templates already span the source. Re-fitting cubic bounds
            // can move an edge anchor just outside the editable source.
            return Some(());
        }
        let source = source?;
        let dimensions = [source.width(), source.height()].map(f64::from);
        let texels = path::Bounds {
            min: [0, 1].map(|axis| bounds.min[axis] * dimensions[axis]),
            size: [0, 1].map(|axis| bounds.size[axis] * dimensions[axis]),
        };
        let expanded = expanded_sampling_bounds(texels, source, feather, jitter);
        let percent = [0, 1].map(|axis| {
            ((expanded.size[axis] - texels.size[axis]) * 100. / 2. / dimensions[axis]) as f32
        });
        let start = percent.map(|value| f64::from(value / 100.));
        let size = percent.map(|value| 1. - 2. * value / 100.);
        if size.iter().any(|value| !value.is_finite() || *value <= 0.) {
            return None;
        }
        let scale = [0, 1].map(|axis| f64::from(size[axis]) / bounds.size[axis]);
        for knot in &mut self.knots {
            knot.point = [0, 1].map(|axis| {
                (start[axis] + (f64::from(knot.point[axis]) - bounds.min[axis]) * scale[axis])
                    as f32
            });
            knot.incoming =
                [0, 1].map(|axis| (f64::from(knot.incoming[axis]) * scale[axis]) as f32);
            knot.outgoing =
                [0, 1].map(|axis| (f64::from(knot.outgoing[axis]) * scale[axis]) as f32);
        }
        path::Path::from_knots(&self.knots).ok()??;
        Some(())
    }
}

impl Patches {
    pub(super) fn selection_source_geometry_with_input(
        &self,
        input: Option<ImageGeometry>,
    ) -> Option<ImageGeometry> {
        let source = match &self.source {
            Source::Input => input?,
            Source::Asset(asset) => asset.source_geometry::<()>().ok()?,
            Source::Missing => return None,
        };
        Some(ImageGeometry::new(
            source.width().checked_mul(1000)?,
            source.height().checked_mul(1000)?,
        ))
    }

    pub(super) fn shape_limit(
        geometry: Option<ImageGeometry>,
        selection: &Selection,
        slot: usize,
    ) -> Option<PatchShapeLimit> {
        let source = geometry?;
        if !matches!(slot, 1 | 2) || source.width() <= 0 || source.height() <= 0 {
            return None;
        }
        let bounds = path::Path::from_knots(&selection.knots).ok()??.bounds;
        Some(PatchShapeLimit {
            bounds,
            source,
            other: selection.effective_number(3 - slot) as f32 / 2.,
            jitter: slot == 2,
        })
    }

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
        let mut target = filled(area(geometry)?, [0., 0., 0., 1.])?;
        if self.selections.is_empty() || (!self.automatic.enabled && self.manual.is_empty()) {
            return Ok(target);
        }
        let factor = super::super::raster::style_scale(scale as f32);
        if !factor.is_finite() || factor <= 0. {
            return Err(PointwiseError::Argument);
        }
        let photo = match &self.source {
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
                let width =
                    request.photo_width((geometry.width() as f32 * 1.2 / factor).ceil(), size);
                asset.synthesis_source(
                    width,
                    effects.environment_library(),
                    execution,
                    effects.cancel_check(),
                )?
            }
        };
        let strength =
            NumericLink::effective_value(self.strength, self.strength_adjustment.as_ref());
        let photo = prepare_photo(&photo, self.equalize, strength, execution, effects)?;
        let photo = Photo::read(&photo)?;
        let patches = self.prepare_patches(&photo, seed, execution, effects)?;
        let cancelled = effects.cancel_check();
        let size = [geometry.width() as usize, geometry.height() as usize];
        let mut canvas = draw::Target {
            size,
            pixels: &mut target,
            photo: &photo,
            color,
            cancelled,
        };
        let lattice = SeededLattice::new(seed)?;
        if self.automatic.enabled {
            self.stamp_automatic(&mut canvas, &patches, factor, seed, &lattice)?;
        }
        let repeat = self.effective_repeat().map(|count| count as f32 * factor);
        if repeat
            .iter()
            .any(|value| !value.is_finite() || *value >= i32::MAX as f32)
        {
            return Err(ImageError::Overflow.into());
        }
        let period = repeat.map(|count| 1. / count);
        for placement in self.manual.iter().rev() {
            let Some(patch) = patches
                .get(placement.patch as usize)
                .and_then(Option::as_ref)
            else {
                continue;
            };
            let offset: [f32; 2] =
                std::array::from_fn(|i| placement.point[i] * size[i] as f32 * period[i]);
            let stretch = std::array::from_fn(|i| {
                (f64::from(placement.size[i] * period[i]) / patch.signed_extent[i]) as f32
            });
            for y in 0..repeat[1].ceil() as i32 {
                for x in 0..repeat[0].ceil() as i32 {
                    crate::pointwise::check_render_cancelled(cancelled)?;
                    let origin = [
                        offset[0] + period[0] * x as f32 * size[0] as f32,
                        offset[1] + period[1] * y as f32 * size[1] as f32,
                    ];
                    let random = lattice.sample4(x, y, placement.patch, placement.sequence);
                    canvas.place(patch, origin, stretch, placement.angle, f64::from(random))?;
                }
            }
        }
        Ok(target)
    }

    fn prepare_patches<E: 'static>(
        &self,
        photo: &Photo,
        seed: i32,
        execution: crate::ProceduralExecution,
        effects: &mut PointwiseRenderEffects<'_, E>,
    ) -> Result<Vec<Option<PreparedPatch>>, PointwiseError<E>> {
        let mut patches = Vec::new();
        patches
            .try_reserve_exact(self.selections.len())
            .map_err(|_| ImageError::Allocation)?;
        for selection in &self.selections {
            patches.push(PreparedPatch::new(
                selection, photo, seed, execution, effects,
            )?);
        }
        Ok(patches)
    }

    fn stamp_automatic<E>(
        &self,
        canvas: &mut draw::Target<'_>,
        patches: &[Option<PreparedPatch>],
        factor: f32,
        seed: i32,
        lattice: &SeededLattice,
    ) -> Result<(), PointwiseError<E>> {
        let size = canvas.size;
        let cancelled = canvas.cancelled;
        let cells = self
            .automatic
            .cells(factor, &self.selections, seed, lattice, cancelled)?;
        let depth = self.automatic.effective_density();
        let cutoff = match (cells.first(), cells.last()) {
            (Some(first), Some(last)) if depth < 100. => {
                let floor = ((last.weight - first.weight) as f32 * (1. - depth / 100.)
                    + first.weight as f32) as i32;
                Some(floor..last.weight)
            }
            _ => None,
        };
        let count = self.automatic.count(factor);
        let pitch: [f32; 2] = std::array::from_fn(|i| size[i] as f32 / count[i] as f32);
        let [position_jitter_x, position_jitter_y] = self.automatic.effective_position_jitter();
        let angle_jitter = self.automatic.effective_angle_jitter();
        let base_angle = self.automatic.effective_angle();
        let [move_x, move_y] = self.automatic.effective_offset();
        for cell in cells {
            crate::pointwise::check_render_cancelled(cancelled)?;
            if depth <= 99.9
                && cutoff
                    .as_ref()
                    .is_some_and(|range| !range.contains(&cell.weight))
            {
                continue;
            }
            let Some(patch) = patches.get(cell.patch).and_then(Option::as_ref) else {
                continue;
            };
            let [x, y] = cell.index;
            let jitter_x = position_jitter_x / 100. * (lattice.sample3(x, y, 2) * 2. - 1.);
            let jitter_y = position_jitter_y / 100. * (lattice.sample3(x, y, 3) * 2. - 1.);
            let angle =
                base_angle + angle_jitter / 100. * 180. * (lattice.sample3(x, y, 4) * 2. - 1.);
            let origin = [
                (x as f32 + cell.slide + move_x / 100. + jitter_x) * pitch[0],
                (y as f32 + move_y / 100. + jitter_y) * pitch[1],
            ];
            canvas.place(
                patch,
                origin,
                [cell.scale; 2],
                angle,
                f64::from(lattice.sample3(x, y, 10)),
            )?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct Cell {
    index: [i32; 2],
    weight: i32,
    patch: usize,
    slide: f32,
    scale: f32,
}

impl Automatic {
    fn count(&self, factor: f32) -> [i32; 2] {
        self.effective_frequency()
            .map(|count| ((count as f32 * factor) as i32).max(1))
    }

    fn cells<E>(
        &self,
        factor: f32,
        selections: &[Selection],
        seed: i32,
        lattice: &SeededLattice,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<Cell>, PointwiseError<E>> {
        let [columns, rows] = self.count(factor);
        let count = (columns as usize)
            .checked_mul(rows as usize)
            .ok_or(ImageError::Overflow)?;
        let mut cells = Vec::new();
        cells
            .try_reserve_exact(count)
            .map_err(|_| ImageError::Allocation)?;
        let weight_sum: f32 = selections
            .iter()
            .map(|selection| selection.effective_frequency() as f32 / 100.)
            .sum();
        let frequency = |sample| {
            let target = sample * weight_sum;
            let mut cumulative = 0.;
            selections
                .iter()
                .position(|selection| {
                    cumulative += selection.effective_frequency() as f32 / 100.;
                    f64::from(cumulative) >= f64::from(target) - 0.0001
                })
                .unwrap_or(usize::MAX)
        };
        let mut random = SeededRandom::from_seed(seed).map_err(|_| ImageError::Overflow)?;
        let row_step = 1. / rows as f32;
        let slide_step =
            ((self.effective_slide() / 100. / row_step).round_ties_even() as i32) as f32 * row_step;
        let scale_jitter = self.effective_scale_jitter() / 100.;
        let base_scale = 1. + self.effective_scale().clamp(-100., 1000.) / 100.;
        let switcher = self.effective_switcher();
        let mut selection = 0;
        let mut slide = 0.;
        for y in 0..rows {
            crate::pointwise::check_render_cancelled(cancelled)?;
            if switcher == 2 {
                selection = 0;
            }
            for x in 0..columns {
                let patch = if switcher == 0 {
                    let seed = (random.sample() * 10.) as i32 + 17;
                    frequency(lattice.sample3(x, y, seed))
                } else {
                    selection
                };
                let variation = lattice.sample3(x, y, 12) * scale_jitter * 2. + 1.;
                let scale = if lattice.sample3(x, y, 18) > 0.5 {
                    base_scale * variation
                } else {
                    base_scale / variation
                };
                cells.push(Cell {
                    index: [x, y],
                    weight: (lattice.sample3(x, y, 1) * 100000.) as i32,
                    patch,
                    slide,
                    scale: scale / factor,
                });
                if matches!(switcher, 2 | 3) {
                    selection = (selection + 1) % selections.len();
                }
            }
            if matches!(switcher, 1 | 3) {
                selection = (selection + 1) % selections.len();
            }
            slide += slide_step;
        }
        order::sort_by_key(&mut cells, |cell| cell.weight);
        Ok(cells)
    }
}
