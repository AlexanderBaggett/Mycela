//! Geometry authoring on the same immutable layer revisions as the texture controls.
use super::{
    Arc, Paint, SynthesisEdit, SynthesisField, SynthesisNumberField, changed_fields, patches,
};
use crate::ImageGeometry;

mod crop;
pub use crop::SynthesisCropHandle;
mod manual_placement;
pub use manual_placement::{
    SynthesisPlacementClipboard, SynthesisPlacementField, SynthesisPlacementGeometry,
    SynthesisPlacementTransform,
};
mod patch_shape;
pub use patch_shape::SynthesisPatchShape;
mod patch_transform;
pub use patch_transform::{
    SynthesisPatchResizeHandle, SynthesisPatchTransform, SynthesisPatchTransformBounds,
    SynthesisPatchTransformField,
};

const MAX_REPEAT_CELLS: usize = 100;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SynthesisGeometry {
    Crop,
    Grid,
    GridPoint(usize),
    Patch { selection: usize, knot: usize },
    Placement(usize),
}

pub(super) fn new_selection() -> patches::Selection {
    let knots = [[0.3, 0.3], [0.7, 0.3], [0.7, 0.7], [0.3, 0.7]]
        .into_iter()
        .enumerate()
        .map(|(ix, point)| {
            let offsets = [[0., 0.1], [-0.1, 0.], [0., -0.1], [0.1, 0.]];
            patches::Knot {
                point,
                incoming: offsets[ix],
                outgoing: offsets[(ix + 1) % 4].map(|value| -value),
                mode: 0,
                flags: [false; 3],
            }
        })
        .collect();
    selection_with_knots(knots)
}

fn selection_with_knots(knots: Vec<patches::Knot>) -> patches::Selection {
    patches::Selection {
        frequency: 100,
        number_links: Default::default(),
        animations: Default::default(),
        feather: 0.,
        jitter: 0.,
        jitter_frequency: 40,
        shadow_strength: 0.,
        shadow_feather: 4.,
        flipped: [false; 2],
        knots,
    }
}

impl SynthesisEdit {
    pub fn repeat_grid(&self, layer: usize) -> Option<([usize; 2], &[[f32; 2]])> {
        let Paint::Repeat(grid) = &self.value.layers.get(layer)?.paint else {
            return None;
        };
        Some(([grid.columns, grid.rows], &grid.points))
    }

    /// Projects the full Repeat grid from its mouse-down state. `indices` use
    /// [`Self::repeat_grid`] point order, and `delta` is in normalized source coordinates.
    pub fn repeat_grid_for_drag(
        &self,
        layer: usize,
        indices: &[usize],
        delta: [f32; 2],
        source: Option<ImageGeometry>,
    ) -> Option<Vec<[f32; 2]>> {
        let Paint::Repeat(grid) = &self.value.layers.get(layer)?.paint else {
            return None;
        };
        if indices.is_empty()
            || !delta.into_iter().all(f32::is_finite)
            || !indices.iter().all(|&index| {
                grid.points
                    .get(index)
                    .is_some_and(|point| point.iter().all(|axis| axis.is_finite()))
            })
        {
            return None;
        }
        let mut projected = grid.points.to_vec();
        if delta == [0.; 2] {
            return Some(projected);
        }
        let (min, max) = repeat_point_bounds(indices.iter().map(|&index| grid.points[index]))?;
        let (area_min, area_max) = repeat_point_bounds(grid.points.iter().copied())?;
        let (padding, _) = repeat_feather_padding(grid, source, area_min, area_max);
        let movement = [0, 1]
            .map(|axis| repeat_axis_movement(min[axis], max[axis], padding[axis], delta[axis]));
        for &index in indices {
            let original = grid.points[index];
            projected[index] = [0, 1].map(|axis| {
                let step = movement[axis];
                if step == 0. {
                    original[axis]
                } else {
                    (f64::from(original[axis]) + step) as f32
                }
            });
        }
        Some(projected)
    }

    /// Moves the selected Repeat points together using the same projection as the guide.
    pub fn move_repeat_grid_points(
        &mut self,
        layer: usize,
        indices: &[usize],
        delta: [f32; 2],
        source: Option<ImageGeometry>,
    ) -> bool {
        let Some(projected) = self.repeat_grid_for_drag(layer, indices, delta, source) else {
            return false;
        };
        let Some((_, before)) = self.repeat_grid(layer) else {
            return false;
        };
        if indices
            .iter()
            .all(|&index| before[index] == projected[index])
        {
            return false;
        }
        let Paint::Repeat(grid) = &mut Arc::make_mut(&mut self.value.layers[layer]).paint else {
            return false;
        };
        grid.points = projected.into_boxed_slice();
        self.changed = true;
        true
    }

    /// The visible `[left, top, right, bottom]` boundary and whether to mark its feather.
    /// The supplied control points may include the position of an in-progress drag.
    pub fn repeat_guide_bounds(
        &self,
        layer: usize,
        source: Option<ImageGeometry>,
        points: &[[f32; 2]],
    ) -> Option<([f32; 4], bool)> {
        let Paint::Repeat(grid) = &self.value.layers.get(layer)?.paint else {
            return None;
        };
        let (min, max) = repeat_point_bounds(points.iter().copied())?;
        let (padding, has_feather) = repeat_feather_padding(grid, source, min, max);
        Some((
            [
                min[0] - padding[0],
                min[1] - padding[1],
                max[0] + padding[0],
                max[1] + padding[1],
            ],
            has_feather,
        ))
    }

    pub fn patch_counts(&self, layer: usize) -> Option<(usize, usize)> {
        let Paint::Patches(patches) = &self.value.layers.get(layer)?.paint else {
            return None;
        };
        Some(if patches.selections_parked {
            (0, 0)
        } else {
            (patches.selections.len(), patches.manual.len())
        })
    }

    pub fn patch_knots(
        &self,
        layer: usize,
        patch: usize,
    ) -> Option<impl ExactSizeIterator<Item = [f32; 2]> + '_> {
        Some(
            self.patches(layer)?
                .selections
                .get(patch)?
                .knots
                .iter()
                .map(|knot| knot.point),
        )
    }

    /// Samples each segment at equal parameter intervals, excluding its start.
    /// Segments are ordered by their starting knot, ending with the closing edge.
    pub fn patch_outline(&self, layer: usize, patch: usize) -> Option<Vec<[f32; 2]>> {
        let selection = self.patches(layer)?.selections.get(patch)?;
        patches::selection_outline(selection)
    }

    /// Incoming and outgoing handle positions in selection coordinates.
    pub fn patch_handles(&self, layer: usize, patch: usize, knot: usize) -> Option<[[f32; 2]; 2]> {
        let selection = self.patches(layer)?.selections.get(patch)?;
        let point = selection.knots.get(knot)?.point;
        Some(
            patches::selection_handles(selection, knot)?
                .map(|offset| std::array::from_fn(|axis| point[axis] + offset[axis])),
        )
    }

    /// Move one tangent endpoint, maintaining the angle rule for linked handles.
    pub fn move_patch_handle(
        &mut self,
        layer: usize,
        patch: usize,
        knot: usize,
        incoming: bool,
        point: [f32; 2],
    ) -> bool {
        let Some(patches) = self.patches_mut(layer) else {
            return false;
        };
        let moved = edit_selection(patches, patch, |selection| {
            let anchor = selection.knots.get(knot)?;
            let offset: [f32; 2] = std::array::from_fn(|axis| point[axis] - anchor.point[axis]);
            if offset.iter().any(|coord| !(-2. ..=2.).contains(coord)) {
                return None;
            }
            let effective = if anchor.mode == 0 {
                patches::selection_handles(selection, knot)?
            } else {
                [anchor.incoming, anchor.outgoing]
            };
            if effective[usize::from(!incoming)] == offset {
                return None;
            }
            let knot = &mut selection.knots[knot];
            if knot.mode == 0 {
                [knot.incoming, knot.outgoing] = effective;
                knot.mode = 2;
            }
            let (edited, opposite) = if incoming {
                (&mut knot.incoming, &mut knot.outgoing)
            } else {
                (&mut knot.outgoing, &mut knot.incoming)
            };
            *edited = offset;
            if knot.mode == 1 {
                link_handles(offset, opposite);
            }
            Some(())
        })
        .is_some();
        self.changed |= moved;
        moved
    }

    pub fn placement_points(
        &self,
        layer: usize,
    ) -> Option<impl ExactSizeIterator<Item = [f32; 2]> + '_> {
        Some(
            self.patches(layer)?
                .manual
                .iter()
                .map(|placement| placement.point),
        )
    }

    pub fn geometry_fields(
        &self,
        layer: usize,
        geometry: SynthesisGeometry,
    ) -> Vec<SynthesisField> {
        let Some(layer) = self.value.layers.get(layer) else {
            return Vec::new();
        };
        let mut paint = layer.paint.clone();
        let mut fields = Vec::new();
        visit_geometry(&mut paint, geometry, |field| {
            fields.push(field);
            field.value
        });
        fields
    }

    pub fn set_geometry_fields(
        &mut self,
        layer: usize,
        geometry: SynthesisGeometry,
        values: &[f64],
    ) -> Result<(), &'static str> {
        self.set_geometry_fields_with_input_geometry(layer, geometry, values, None)
    }

    /// Applies the field edit using the raw graph input size captured for this authoring action.
    /// Photo sources use their own dimensions.
    pub fn set_geometry_fields_with_input_geometry(
        &mut self,
        layer: usize,
        geometry: SynthesisGeometry,
        values: &[f64],
        input_geometry: Option<crate::ImageGeometry>,
    ) -> Result<(), &'static str> {
        let fields = self.geometry_fields(layer, geometry);
        let values = self.retain_linked_numbers(Some(layer), &fields, values);
        if !changed_fields(&fields, &values, "The selected geometry changed.")? {
            return Ok(());
        }
        if geometry == SynthesisGeometry::Crop {
            let before = self
                .crop_rectangle(layer)
                .ok_or("The selected crop changed.")?;
            self.set_crop_rectangle(layer, crop::with_transform(before, &values));
            return Ok(());
        }
        let paint = &mut Arc::make_mut(&mut self.value.layers[layer]).paint;
        let before = match (&*paint, geometry) {
            (Paint::Patches(patches), SynthesisGeometry::Patch { selection, .. }) => {
                placement_extent(patches, selection).map(|size| (selection, size))
            }
            _ => None,
        };
        match (geometry, &mut *paint) {
            (SynthesisGeometry::Grid, Paint::Repeat(grid)) => {
                resize_grid(grid, values[0] as usize + 1, values[1] as usize + 1)?;
            }
            _ => {
                let mut values = values.iter();
                visit_geometry(paint, geometry, |_| {
                    *values.next().expect("validated fields")
                });
            }
        }
        if let Paint::Patches(patches) = paint {
            let limit_geometry = patches.selection_source_geometry_with_input(input_geometry);
            for (field, value) in fields.iter().zip(&values) {
                let Some((index, slot @ (1 | 2))) = field
                    .number_field()
                    .and_then(SynthesisNumberField::patch_slot)
                else {
                    continue;
                };
                if field.value() != *value {
                    let selection = &mut patches.selections[index];
                    if let Some(limit) =
                        patches::Patches::shape_limit(limit_geometry, selection, slot)
                    {
                        let raw = if slot == 1 {
                            &mut selection.feather
                        } else {
                            &mut selection.jitter
                        };
                        *raw = limit.clamp(*raw);
                    }
                }
            }
            if let Some((selection, size)) = before {
                resize_placements(patches, selection, size);
            }
        }
        self.changed = true;
        Ok(())
    }

    pub fn move_geometry_point(
        &mut self,
        layer: usize,
        geometry: SynthesisGeometry,
        point: [f32; 2],
    ) -> bool {
        if matches!(geometry, SynthesisGeometry::Grid | SynthesisGeometry::Crop)
            || point
                .iter()
                .any(|coord| !coord.is_finite() || !(0. ..=1.).contains(coord))
        {
            return false;
        }
        let Some(layer) = self.value.layers.get_mut(layer) else {
            return false;
        };
        let move_point = |slot: Option<&mut [f32; 2]>| {
            let slot = slot.filter(|old| **old != point)?;
            *slot = point;
            Some(())
        };
        let moved = match (&mut Arc::make_mut(layer).paint, geometry) {
            (Paint::Repeat(grid), SynthesisGeometry::GridPoint(index)) => {
                move_point(grid.points.get_mut(index))
            }
            (Paint::Patches(patches), SynthesisGeometry::Patch { selection, knot })
                if !patches.selections_parked =>
            {
                edit_selection(patches, selection, |selection| {
                    move_point(selection.knots.get_mut(knot).map(|knot| &mut knot.point))
                })
            }
            (Paint::Patches(patches), SynthesisGeometry::Placement(index))
                if !patches.selections_parked =>
            {
                move_point(
                    patches
                        .manual
                        .get_mut(index)
                        .map(|placement| &mut placement.point),
                )
            }
            _ => None,
        };
        self.changed |= moved.is_some();
        moved.is_some()
    }

    pub fn add_patch(&mut self, layer: usize) -> Option<usize> {
        let patches = self.patches_mut(layer)?;
        let index = patches.selections.len();
        patches.selections.push(new_selection());
        self.changed = true;
        Some(index)
    }

    pub fn remove_patch(&mut self, layer: usize, selection: usize) -> bool {
        let Some(patches) = self.patches_mut(layer) else {
            return false;
        };
        if selection >= patches.selections.len() {
            return false;
        }
        patches.selections.remove(selection);
        let removed = selection as i32;
        patches.manual.retain_mut(|placement| {
            if placement.patch > removed {
                placement.patch -= 1;
                true
            } else {
                placement.patch != removed
            }
        });
        self.changed = true;
        true
    }

    pub fn insert_patch_knot(
        &mut self,
        layer: usize,
        selection: usize,
        knot: usize,
    ) -> Option<usize> {
        self.split_patch_segment(layer, selection, knot, 0.5)
    }

    pub fn split_patch_segment(
        &mut self,
        layer: usize,
        selection: usize,
        knot: usize,
        t: f64,
    ) -> Option<usize> {
        let inserted = edit_selection(self.patches_mut(layer)?, selection, |selection| {
            patches::insert_selection_point(selection, knot, t)
        })?;
        self.changed = true;
        Some(inserted)
    }

    pub fn remove_patch_knot(&mut self, layer: usize, selection: usize, knot: usize) -> bool {
        let removed = self
            .patches_mut(layer)
            .and_then(|patches| {
                edit_selection(patches, selection, |selection| {
                    let knots = &mut selection.knots;
                    (knots.len() > 3 && knot < knots.len()).then(|| knots.remove(knot))
                })
            })
            .is_some();
        self.changed |= removed;
        removed
    }

    pub fn add_placement(&mut self, layer: usize, selection: usize) -> Option<usize> {
        self.create_manual_placement(layer, selection, [0.5; 2])
    }

    pub fn remove_placement(&mut self, layer: usize, placement: usize) -> bool {
        let Some(patches) = self.patches_mut(layer) else {
            return false;
        };
        if placement >= patches.manual.len() {
            return false;
        }
        let removed = patches.manual.remove(placement);
        close_sequence_gap(&mut patches.manual, &removed);
        self.changed = true;
        true
    }

    pub fn flip_patch(&mut self, layer: usize, selection: usize, axis: usize) -> bool {
        let Some(flip) = self
            .patches_mut(layer)
            .and_then(|patches| patches.selections.get_mut(selection))
            .and_then(|selection| selection.flipped.get_mut(axis))
        else {
            return false;
        };
        *flip = !*flip;
        self.changed = true;
        true
    }

    pub fn patch_flips(&self, layer: usize, selection: usize) -> Option<[bool; 2]> {
        Some(self.patches(layer)?.selections.get(selection)?.flipped)
    }

    pub fn patch_knot_mode(&self, layer: usize, selection: usize, knot: usize) -> Option<usize> {
        self.patches(layer)?
            .selections
            .get(selection)?
            .knots
            .get(knot)?
            .mode
            .try_into()
            .ok()
    }

    pub fn set_patch_knot_mode(
        &mut self,
        layer: usize,
        selection: usize,
        knot: usize,
        mode: usize,
    ) -> bool {
        let Some(patches) = self.patches_mut(layer) else {
            return false;
        };
        let changed = edit_selection(patches, selection, |selection| {
            let current = selection.knots.get(knot)?.mode;
            if mode >= 3 || current == mode as i32 {
                return None;
            }
            let effective = (current == 0)
                .then(|| patches::selection_handles(selection, knot))
                .flatten();
            let knot = &mut selection.knots[knot];
            if let Some([incoming, outgoing]) = effective {
                knot.incoming = incoming;
                knot.outgoing = outgoing;
            }
            knot.mode = mode as i32;
            if mode == 1 {
                link_handles(knot.incoming, &mut knot.outgoing);
            }
            Some(())
        })
        .is_some();
        self.changed |= changed;
        changed
    }

    fn patches(&self, layer: usize) -> Option<&patches::Patches> {
        match &self.value.layers.get(layer)?.paint {
            Paint::Patches(patches) if !patches.selections_parked => Some(patches),
            _ => None,
        }
    }

    fn patches_mut(&mut self, layer: usize) -> Option<&mut patches::Patches> {
        match &mut Arc::make_mut(self.value.layers.get_mut(layer)?).paint {
            Paint::Patches(patches) if !patches.selections_parked => Some(patches),
            _ => None,
        }
    }
}

fn repeat_point_bounds(points: impl Iterator<Item = [f32; 2]>) -> Option<([f32; 2], [f32; 2])> {
    let mut finite = points.filter(|point| point.iter().all(|axis| axis.is_finite()));
    let first = finite.next()?;
    Some(finite.fold((first, first), |(min, max), point| {
        (
            [0, 1].map(|axis| min[axis].min(point[axis])),
            [0, 1].map(|axis| max[axis].max(point[axis])),
        )
    }))
}

fn repeat_feather_padding(
    grid: &super::Pattern,
    source: Option<ImageGeometry>,
    min: [f32; 2],
    max: [f32; 2],
) -> ([f32; 2], bool) {
    let span = [0, 1].map(|axis| f64::from(max[axis] - min[axis]));
    let radius = f64::from((grid.effective_feather() / 2.) as f32) / 100.;
    let has_feather = radius.is_finite() && radius > 0.;
    let padding = source
        .filter(|size| has_feather && size.width() > 0 && size.height() > 0)
        .map_or([0.; 2], |source| repeat_guide_padding(span, source, radius));
    (padding, has_feather)
}

fn repeat_axis_movement(min: f32, max: f32, feather: f32, delta: f32) -> f64 {
    let [min, max, feather, delta] = [min, max, feather, delta].map(f64::from);
    let raw = [-min, 1. - max];
    let inset = [raw[0] + feather, raw[1] - feather];
    if inset[0] <= inset[1] {
        delta.clamp(inset[0], inset[1])
    } else if raw[0] <= raw[1] {
        // Retained grids may already be wider than the feather inset.
        delta.clamp(raw[0], raw[1])
    } else {
        // A rigid move cannot fit an over-wide imported selection into the source.
        0.
    }
}

fn repeat_guide_padding(span: [f64; 2], source: ImageGeometry, radius: f64) -> [f32; 2] {
    // Editor.p uses thousandths of source texels. The guide's feather is allowed
    // to extend beyond the source; clipping to the image is a viewport concern.
    let virtual_size = [source.width(), source.height()].map(|size| (size as f32 * 1000.) as i32);
    let [width, height] = virtual_size;
    let unpadded = std::array::from_fn::<_, 2, _>(|axis| {
        (span[axis] * f64::from(virtual_size[axis])).ceil().max(1.) as i32
    });
    let padded = crate::pointwise::blur::periodic_extent(
        ImageGeometry::new(unpadded[0], unpadded[1]),
        ImageGeometry::new(width, height),
        radius,
        true,
    );
    let expansion = [padded.width(), padded.height()];
    std::array::from_fn(|axis| {
        (span[axis] * (f64::from(expansion[axis]) / f64::from(unpadded[axis]) - 1.) * 0.5) as f32
    })
}

fn resize_grid(grid: &mut super::Pattern, columns: usize, rows: usize) -> Result<(), &'static str> {
    let old_rows = grid.rows;
    let old_columns = grid.columns;
    // Imported grid dimensions read from the file can exceed the authoring range.
    if !(2..=MAX_REPEAT_CELLS + 1).contains(&columns) || !(2..=MAX_REPEAT_CELLS + 1).contains(&rows)
    {
        return Err("Use 1 to 100 cells per axis to resize the grid.");
    }
    let corners = if old_rows > 0 && old_columns > 0 {
        let right = (old_columns - 1) * old_rows;
        [
            grid.points[0],
            grid.points[old_rows - 1],
            grid.points[right],
            grid.points[right + old_rows - 1],
        ]
    } else {
        [[0.15, 0.15], [0.15, 0.85], [0.85, 0.15], [0.85, 0.85]]
    };
    let at = |first: [f32; 2], last: [f32; 2], index: usize, len: usize| {
        if index == 0 {
            first
        } else if index == len - 1 {
            last
        } else {
            std::array::from_fn(|axis| {
                first[axis] + (last[axis] - first[axis]) / (len - 1) as f32 * index as f32
            })
        }
    };
    let [top_left, bottom_left, top_right, bottom_right] = corners;
    grid.points = (0..columns)
        .flat_map(|x| {
            (0..rows).map(move |y| {
                let left = at(top_left, bottom_left, y, rows);
                let right = at(top_right, bottom_right, y, rows);
                at(left, right, x, columns)
            })
        })
        .collect();
    grid.columns = columns;
    grid.rows = rows;
    Ok(())
}

fn next_sequence(placements: &[patches::Placement], patch: i32) -> i32 {
    placements
        .iter()
        .filter(|item| item.patch == patch)
        .count()
        .saturating_add(1)
        .min(i32::MAX as usize) as i32
}

fn close_sequence_gap(placements: &mut [patches::Placement], removed: &patches::Placement) {
    for item in placements
        .iter_mut()
        .filter(|item| item.patch == removed.patch && item.sequence > removed.sequence)
    {
        item.sequence -= 1;
    }
}

fn edit_selection<T>(
    patches: &mut patches::Patches,
    selection: usize,
    change: impl FnOnce(&mut patches::Selection) -> Option<T>,
) -> Option<T> {
    let before = placement_extent(patches, selection);
    let result = change(patches.selections.get_mut(selection)?)?;
    if let Some(before) = before {
        resize_placements(patches, selection, before);
    }
    Some(result)
}

fn placement_extent(patches: &patches::Patches, selection: usize) -> Option<[f32; 2]> {
    let patch = i32::try_from(selection).ok()?;
    patches
        .manual
        .iter()
        .any(|item| item.patch == patch)
        .then(|| {
            patches
                .selections
                .get(selection)
                .and_then(patches::selection_size)
        })
        .flatten()
}

fn resize_placements(patches: &mut patches::Patches, selection: usize, before: [f32; 2]) {
    let Some(after) = patches
        .selections
        .get(selection)
        .and_then(patches::selection_size)
    else {
        return;
    };
    if before != after {
        for placement in patches
            .manual
            .iter_mut()
            .filter(|item| item.patch == selection as i32)
        {
            scale_placement(placement, before, after);
        }
    }
}

fn scale_placement(placement: &mut patches::Placement, before: [f32; 2], after: [f32; 2]) {
    for (size, (old, new)) in placement.size.iter_mut().zip(before.into_iter().zip(after)) {
        let scaled = *size * (new / old);
        if scaled.is_finite() {
            *size = scaled;
        }
    }
}

fn link_handles(edited: [f32; 2], opposite: &mut [f32; 2]) {
    let length = edited[0].hypot(edited[1]);
    if length > 0. {
        let opposite_length = opposite[0].hypot(opposite[1]);
        *opposite = edited.map(|coord| -coord / length * opposite_length);
    }
}

fn visit_geometry(
    paint: &mut Paint,
    geometry: SynthesisGeometry,
    mut update: impl FnMut(SynthesisField) -> f64,
) {
    macro_rules! fields {
        ($($value:expr => $label:literal, $min:expr, $max:expr $(, $whole:literal)? $(=> $number:expr)?;)*) => {{
            $(
                $value = update(SynthesisField {
                    label: $label,
                    value: $value as f64,
                    range: [$min, $max],
                    whole: fields!(@whole $($whole)?),
                    number_field: fields!(@number $($number)?),
                    automatic_placement: false,
                    pair_field: None,
                    color_target: None,
                }) as _;
            )*
        }};
        (@whole $whole:literal) => { $whole };
        (@whole) => { false };
        (@number $field:expr) => { Some($field) };
        (@number) => { None };
    }
    macro_rules! position {
        ($point:expr) => {
            fields! {
                $point[0] => "Position X", 0., 1.;
                $point[1] => "Position Y", 0., 1.;
            }
        };
    }
    match (paint, geometry) {
        (Paint::Blend(layer), SynthesisGeometry::Crop) => {
            let mut values = crop::transform(layer.rectangle);
            fields! {
                values[0] => "Center X (%)", 0., 100.;
                values[1] => "Center Y (%)", 0., 100.;
                values[2] => "Width (%)", -100., 100.;
                values[3] => "Height (%)", -100., 100.;
            }
        }
        (Paint::Repeat(grid), SynthesisGeometry::Grid) => {
            let mut cells = [grid.columns, grid.rows].map(|points| points.saturating_sub(1));
            fields! {
                cells[0] => "Grid X", 1., MAX_REPEAT_CELLS as f64, true;
                cells[1] => "Grid Y", 1., MAX_REPEAT_CELLS as f64, true;
            }
        }
        (Paint::Repeat(grid), SynthesisGeometry::GridPoint(index)) => {
            if let Some(point) = grid.points.get_mut(index) {
                position!(point);
            }
        }
        (
            Paint::Patches(patches),
            SynthesisGeometry::Patch {
                selection: index,
                knot,
            },
        ) if !patches.selections_parked => {
            let Some(selection) = patches.selections.get_mut(index) else {
                return;
            };
            if let Some(knot) = selection.knots.get_mut(knot) {
                let previous = knot.clone();
                position!(knot.point);
                fields! {
                    knot.incoming[0] => "Incoming handle X", -2., 2.;
                    knot.incoming[1] => "Incoming handle Y", -2., 2.;
                    knot.outgoing[0] => "Outgoing handle X", -2., 2.;
                    knot.outgoing[1] => "Outgoing handle Y", -2., 2.;
                }
                // Match the linked angle rule while preserving the other handle's length.
                if previous.incoming != knot.incoming || previous.outgoing != knot.outgoing {
                    if knot.mode == 0 {
                        knot.mode = 2;
                    } else if knot.mode == 1 {
                        let (edited, opposite) = if previous.outgoing != knot.outgoing {
                            (knot.outgoing, &mut knot.incoming)
                        } else {
                            (knot.incoming, &mut knot.outgoing)
                        };
                        link_handles(edited, opposite);
                    }
                }
            }
            fields! {
                selection.frequency => "Placement weight", 0., 100., true => SynthesisNumberField::PatchFrequency(index);
                selection.feather => "Selection feather", 0., 100. => SynthesisNumberField::PatchFeather(index);
                selection.jitter => "Shape jitter", 0., 100. => SynthesisNumberField::PatchJitterShape(index);
                selection.jitter_frequency => "Shape jitter frequency", 1., 100., true => SynthesisNumberField::PatchJitterFrequency(index);
                selection.shadow_strength => "Shadow strength", 0., 100. => SynthesisNumberField::PatchShadowStrength(index);
                selection.shadow_feather => "Shadow feather", 0., 100. => SynthesisNumberField::PatchShadowFeather(index);
            }
        }
        (Paint::Patches(patches), SynthesisGeometry::Placement(index))
            if !patches.selections_parked =>
        {
            let Some(placement) = patches.manual.get_mut(index) else {
                return;
            };
            let previous = placement.clone();
            let mut patch = i64::from(placement.patch) + 1;
            position!(placement.point);
            fields! {
                patch => "Patch number", 1., patches.selections.len() as f64, true;
                placement.angle => "Angle", -360., 360.;
                placement.size[0] => "Width", 0.001, 16.;
                placement.size[1] => "Height", 0.001, 16.;
            }
            let patch = (patch - 1) as i32;
            if patch != previous.patch {
                placement.patch = patch;
                if placement.size == previous.size
                    && let Some(before) = patches
                        .selections
                        .get(previous.patch as usize)
                        .and_then(patches::selection_size)
                    && let Some(after) = patches
                        .selections
                        .get(patch as usize)
                        .and_then(patches::selection_size)
                {
                    scale_placement(placement, before, after);
                }
                let sequence = next_sequence(&patches.manual, patch) - 1;
                patches.manual[index].sequence = sequence;
                close_sequence_gap(&mut patches.manual, &previous);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod repeat_guide_tests {
    use super::*;
    use crate::pointwise::canvas::synthesis::{SynthesisLayerKind, SynthesisTexture};

    #[test]
    fn repeat_batch_drag_projects_and_commits_one_feather_clamped_translation() {
        let mut edit = SynthesisEdit::new(Arc::new(SynthesisTexture::default()), true);
        edit.add_layer(SynthesisLayerKind::Repeating);
        let indices = [1, 4, 1];
        let delta = [-1., 1.];
        let source = Some(ImageGeometry::new(160, 100));
        let initial = edit.repeat_grid(0).unwrap().1.to_vec();
        let projected = edit
            .repeat_grid_for_drag(0, &indices, delta, source)
            .unwrap();
        assert_eq!(edit.repeat_grid(0).unwrap().1, initial.as_slice());
        for (actual, expected) in projected[1].into_iter().zip([0.0068812, 0.98899016]) {
            assert!((f64::from(actual) - expected).abs() < 2e-7, "{projected:?}");
        }
        assert!((projected[4][0] - projected[1][0] - (initial[4][0] - initial[1][0])).abs() < 1e-7);
        assert_eq!(projected[4][1], projected[1][1]);
        for index in (0..initial.len()).filter(|index| !indices.contains(index)) {
            assert_eq!(projected[index], initial[index]);
        }
        let unpadded = edit.repeat_grid_for_drag(0, &indices, delta, None).unwrap();
        assert_eq!(unpadded[1], [0., 1.]);
        assert!(edit.move_repeat_grid_points(0, &indices, delta, source));
        assert_eq!(edit.repeat_grid(0).unwrap().1, projected.as_slice());
        assert!(!edit.move_repeat_grid_points(0, &indices, [0.; 2], source));
    }

    #[test]
    fn repeat_batch_drag_rejects_stale_indices_and_protects_imported_wide_groups() {
        let mut edit = SynthesisEdit::new(Arc::new(SynthesisTexture::default()), true);
        edit.add_layer(SynthesisLayerKind::Repeating);
        let Paint::Repeat(grid) = &mut Arc::make_mut(&mut edit.value.layers[0]).paint else {
            unreachable!();
        };
        grid.points[0][0] = -0.25;
        grid.points[2][0] = 1.25;
        grid.points[3][0] = f32::NAN;
        let source = Some(ImageGeometry::new(160, 100));
        let delta = [0.2, 0.1];
        for invalid in [&[][..], &[0, 9], &[0, 3]] {
            assert!(
                edit.repeat_grid_for_drag(0, invalid, delta, source)
                    .is_none()
            );
            assert!(!edit.move_repeat_grid_points(0, invalid, delta, source));
        }
        assert!(
            edit.repeat_grid_for_drag(0, &[0], [f32::INFINITY, 0.], source)
                .is_none()
        );
        assert!(edit.move_repeat_grid_points(0, &[0, 2], delta, source));
        let (counts, retained) = edit.repeat_grid(0).unwrap();
        assert_eq!(counts, [3, 3]);
        assert_eq!([retained[0][0], retained[2][0]], [-0.25, 1.25]);
        assert!((retained[0][1] - 0.25).abs() < 1e-7);
        assert!((retained[2][1] - 0.95).abs() < 1e-7);
        assert_eq!(retained[3][0].to_bits(), f32::NAN.to_bits());

        let Paint::Repeat(grid) = &mut Arc::make_mut(&mut edit.value.layers[0]).paint else {
            unreachable!();
        };
        grid.points[0][0] = 0.;
        grid.points[2][0] = 1.;
        let projected = edit
            .repeat_grid_for_drag(0, &[0, 2], delta, source)
            .unwrap();
        assert_eq!([projected[0][0], projected[2][0]], [0., 1.]);
    }

    #[test]
    fn grid_fields_count_cells_without_changing_retained_point_counts() {
        let mut edit = SynthesisEdit::new(Arc::new(SynthesisTexture::default()), true);
        edit.add_layer(SynthesisLayerKind::Repeating);
        let fields = edit.geometry_fields(0, SynthesisGeometry::Grid);
        assert_eq!([fields[0].label(), fields[1].label()], ["Grid X", "Grid Y"]);
        assert_eq!([fields[0].value(), fields[1].value()], [2., 2.]);
        assert_eq!(fields[0].range(), [1., 100.]);
        let (counts, points) = edit.repeat_grid(0).unwrap();
        assert_eq!(counts, [3, 3]);
        let corners = [points[0], points[2], points[6], points[8]];
        edit.set_geometry_fields(0, SynthesisGeometry::Grid, &[100., 1.])
            .unwrap();
        let (counts, resized) = edit.repeat_grid(0).unwrap();
        assert_eq!(counts, [101, 2]);
        assert_eq!(resized.len(), 202);
        assert_eq!(
            [resized[0], resized[1], resized[200], resized[201]],
            corners
        );
        assert!(
            edit.set_geometry_fields(0, SynthesisGeometry::Grid, &[101., 1.])
                .is_err()
        );
    }

    #[test]
    fn repeat_guide_uses_effective_feather_and_all_live_points() {
        let mut edit = SynthesisEdit::new(Arc::new(SynthesisTexture::default()), true);
        edit.add_layer(SynthesisLayerKind::Repeating);
        let source = Some(ImageGeometry::new(160, 100));
        let mut points = edit.repeat_grid(0).unwrap().1.to_vec();
        let (default_bounds, feather) = edit.repeat_guide_bounds(0, source, &points).unwrap();
        assert!(feather);
        let check_native = |actual: [f32; 4], expected: [f64; 4]| {
            for (actual, expected) in actual.into_iter().zip(expected) {
                assert!((f64::from(actual) - expected).abs() < 2e-7);
            }
        };
        // Original au8.c with Editor.p's 160000 × 100000 virtual source.
        check_native(
            default_bounds,
            [0.143118814, 0.138990164, 0.856881261, 0.8610099],
        );

        points[4] = [0.05, 0.95];
        let (moved_bounds, _) = edit.repeat_guide_bounds(0, source, &points).unwrap();
        check_native(
            moved_bounds,
            [0.0431188047, 0.138990015, 0.856881261, 0.9610099],
        );

        let values: Vec<_> = edit
            .fields(Some(0))
            .iter()
            .map(|field| {
                if field.number_field() == Some(SynthesisNumberField::RepeatFeather) {
                    0.
                } else {
                    field.value()
                }
            })
            .collect();
        edit.set_fields(Some(0), &values).unwrap();
        assert_eq!(
            edit.repeat_guide_bounds(0, source, &points),
            Some(([0.05, 0.15, 0.85, 0.95], false))
        );
        assert_eq!(edit.repeat_guide_bounds(0, source, &[]), None);
    }
}
