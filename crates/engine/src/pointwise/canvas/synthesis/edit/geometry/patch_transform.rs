use super::{SynthesisEdit, patches, scale_placement};
use crate::ImageGeometry;

const MIN_EXTENT: f32 = 0.0001;
const ANCHOR_ROUNDING: f32 = 0.000005;

/// A handle on the signed selection box. After a flip the logical edges exchange sides.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SynthesisPatchResizeHandle {
    NorthWest,
    North,
    NorthEast,
    West,
    East,
    SouthWest,
    South,
    SouthEast,
}

impl SynthesisPatchResizeHandle {
    fn direction(self) -> [f32; 2] {
        match self {
            Self::NorthWest => [1., 1.],
            Self::North => [0., 1.],
            Self::NorthEast => [-1., 1.],
            Self::West => [1., 0.],
            Self::East => [-1., 0.],
            Self::SouthWest => [1., -1.],
            Self::South => [0., -1.],
            Self::SouthEast => [-1., -1.],
        }
    }
}

/// Whole-contour gestures, evaluated relative to the edit at mouse-down.
#[derive(Clone, Copy, Debug)]
pub enum SynthesisPatchTransform {
    Translate {
        delta: [f32; 2],
    },
    Resize {
        handle: SynthesisPatchResizeHandle,
        delta: [f32; 2],
        preserve_aspect: bool,
        from_center: bool,
    },
    /// `pointer` is normalized to the image and `fitted_size` is its displayed pixel size.
    Rotate {
        pointer: [f32; 2],
        fitted_size: [f32; 2],
    },
}

/// A whole-contour Transform control, expressed in source percentages.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SynthesisPatchTransformField {
    CenterX,
    CenterY,
    Width,
    Height,
}

impl SynthesisPatchTransformField {
    /// Input limits before fitting the value against the contour's sampling margin.
    pub const fn limits(self) -> [f64; 2] {
        [if self.is_extent() { -10000. } else { 0. }, 10000.]
    }

    const fn axis(self) -> usize {
        match self {
            Self::CenterX | Self::Width => 0,
            Self::CenterY | Self::Height => 1,
        }
    }

    const fn is_extent(self) -> bool {
        matches!(self, Self::Width | Self::Height)
    }

    const fn counterpart(self) -> Self {
        match self {
            Self::CenterX => Self::Width,
            Self::CenterY => Self::Height,
            Self::Width => Self::CenterX,
            Self::Height => Self::CenterY,
        }
    }

    fn round_input(self, percent: f64) -> f64 {
        let [min, max] = self.limits();
        crate::round_decimal_places(percent.clamp(min, max), 2).expect("two decimal places")
    }
}

/// The signed source box and arithmetic mean of its contour anchors, all normalized.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SynthesisPatchTransformBounds {
    pub origin: [f32; 2],
    pub size: [f32; 2],
    pub pivot: [f32; 2],
}

impl SynthesisPatchTransformBounds {
    /// Reads the two-decimal percentage shown by the Transform control.
    pub fn field_percent(self, field: SynthesisPatchTransformField) -> f64 {
        field_percent(field, self.origin[field.axis()], self.size[field.axis()])
    }
}

impl SynthesisEdit {
    pub fn patch_transform_bounds(
        &self,
        layer: usize,
        patch: usize,
    ) -> Option<SynthesisPatchTransformBounds> {
        let selection = self.patches(layer)?.selections.get(patch)?;
        let bounds = SignedBounds::read(selection)?;
        Some(SynthesisPatchTransformBounds {
            origin: bounds.origin.map(|value| value as f32),
            size: bounds.size.map(|value| value as f32),
            pivot: mean_anchor(selection).map(|value| value as f32),
        })
    }

    /// Projects the pointer gesture without mutating the base edit. The fitted source
    /// size used for sampling margins is the base image rectangle before view zoom.
    pub fn preview_patch_transform(
        &self,
        layer: usize,
        patch: usize,
        transform: SynthesisPatchTransform,
        fit_geometry: Option<ImageGeometry>,
    ) -> Option<Self> {
        let projection = self.patch_transform(layer, patch, transform, fit_geometry, false)?;
        let mut draft = self.clone();
        draft.install_patch_transform(layer, patch, projection);
        Some(draft)
    }

    /// Settles the gesture inside the source and changes the patch and its manual copies together.
    pub fn transform_patch(
        &mut self,
        layer: usize,
        patch: usize,
        transform: SynthesisPatchTransform,
        fit_geometry: Option<ImageGeometry>,
    ) -> bool {
        let Some(projection) = self.patch_transform(layer, patch, transform, fit_geometry, true)
        else {
            return false;
        };
        self.install_patch_transform(layer, patch, projection)
    }

    /// Assigns one typed Transform percentage, preserving the other axis. A signed
    /// extent changes about the box center and updates any manual copies together.
    pub fn set_patch_transform_field(
        &mut self,
        layer: usize,
        patch: usize,
        field: SynthesisPatchTransformField,
        percent: f64,
        fit_geometry: Option<ImageGeometry>,
    ) -> bool {
        if !percent.is_finite() {
            return false;
        }
        let projection = (|| {
            let base = self.patch_transform_base(layer, patch, fit_geometry)?;
            let margins = sampling_margins(base.selection, base.bounds, base.source)?;
            let next = base
                .bounds
                .with_field(field, percent, margins[field.axis()]);
            let mut selection = base.selection.clone();
            if next != base.bounds {
                base.bounds.apply(next, &mut selection);
                next.snap_boundary_anchors(&mut selection, std::iter::once(field.axis()));
            }
            Projection::new(selection, base.bounds, next)
        })();
        projection.is_some_and(|projection| self.install_patch_transform(layer, patch, projection))
    }

    fn patch_transform(
        &self,
        layer: usize,
        patch: usize,
        transform: SynthesisPatchTransform,
        geometry: Option<ImageGeometry>,
        settled: bool,
    ) -> Option<Projection> {
        let TransformBase {
            selection: before,
            bounds: original,
            source,
        } = self.patch_transform_base(layer, patch, geometry)?;
        let mut selection = before.clone();
        let mut projected = match transform {
            SynthesisPatchTransform::Translate { delta } => {
                let margins = sampling_margins(before, original, source)?;
                let bounds = original.translated(delta, margins)?;
                original.apply(bounds, &mut selection);
                bounds
            }
            SynthesisPatchTransform::Resize {
                handle,
                delta,
                preserve_aspect,
                from_center,
            } => {
                let margins = sampling_margins(before, original, source)?;
                let bounds =
                    original.resized(handle, delta, preserve_aspect, from_center, margins)?;
                original.apply(bounds, &mut selection);
                bounds
            }
            SynthesisPatchTransform::Rotate {
                pointer,
                fitted_size,
            } => {
                rotate(&mut selection, pointer, fitted_size)?;
                SignedBounds::read(&selection)?
            }
        };
        if settled && (selection.knots != before.knots || selection.flipped != before.flipped) {
            let margins = sampling_margins(&selection, projected, source)?;
            let fitted = projected.fitted(margins);
            projected.apply(fitted, &mut selection);
            // A fitted contour's endpoints lie inside the source. Restore exact
            // edge positions when cubic sampling bounds introduce roundoff.
            fitted.snap_boundary_anchors(&mut selection, 0..2);
            projected = fitted;
        }
        Projection::new(selection, original, projected)
    }

    fn patch_transform_base(
        &self,
        layer: usize,
        patch: usize,
        geometry: Option<ImageGeometry>,
    ) -> Option<TransformBase<'_>> {
        let patches = self.patches(layer)?;
        let selection = patches.selections.get(patch)?;
        let source = match geometry {
            Some(geometry) if geometry.width() > 0 && geometry.height() > 0 => {
                Some(ImageGeometry::new(
                    geometry.width().checked_mul(1000)?,
                    geometry.height().checked_mul(1000)?,
                ))
            }
            Some(_) => return None,
            None => patches.selection_source_geometry_with_input(None),
        };
        Some(TransformBase {
            selection,
            bounds: SignedBounds::read(selection)?,
            source,
        })
    }

    fn install_patch_transform(&mut self, layer: usize, patch: usize, result: Projection) -> bool {
        let Some(patches) = self.patches_mut(layer) else {
            return false;
        };
        let Some(selection) = patches.selections.get_mut(patch) else {
            return false;
        };
        if selection.knots == result.selection.knots
            && selection.flipped == result.selection.flipped
        {
            return false;
        }
        *selection = result.selection;
        let [before, after] = result.sizes;
        // The original placement treats a negative prior extent as uninitialized.
        let after = [0, 1].map(|axis| {
            if before[axis] < 0. {
                before[axis]
            } else {
                after[axis]
            }
        });
        if before != after {
            for placement in patches
                .manual
                .iter_mut()
                .filter(|item| item.patch == patch as i32)
            {
                scale_placement(placement, before, after);
            }
        }
        self.changed = true;
        true
    }
}

struct TransformBase<'a> {
    selection: &'a patches::Selection,
    bounds: SignedBounds,
    source: Option<ImageGeometry>,
}

struct Projection {
    selection: patches::Selection,
    sizes: [[f32; 2]; 2],
}

impl Projection {
    fn new(
        selection: patches::Selection,
        before: SignedBounds,
        after: SignedBounds,
    ) -> Option<Self> {
        SignedBounds::read(&selection)?;
        Some(Self {
            selection,
            sizes: [before.size, after.size].map(|size| size.map(|value| value as f32)),
        })
    }
}

#[derive(Clone, Copy, PartialEq)]
struct SignedBounds {
    origin: [f64; 2],
    size: [f64; 2],
}

impl SignedBounds {
    fn read(selection: &patches::Selection) -> Option<Self> {
        let (mut origin, mut size) = patches::selection_bounds(selection)?;
        for axis in 0..2 {
            if selection.flipped[axis] {
                origin[axis] += size[axis];
                size[axis] = -size[axis];
            }
        }
        Some(Self { origin, size })
    }

    fn with_field(
        mut self,
        field: SynthesisPatchTransformField,
        percent: f64,
        margin: f64,
    ) -> Self {
        let axis = field.axis();
        let size = self.size[axis] as f32;
        let origin = self.origin[axis] as f32;
        let input = field.round_input(percent);
        if input == field_percent(field, origin, size) {
            return self;
        }
        let margin_percent = (margin * 100.) as f32;
        let counterpart = field_percent(field.counterpart(), origin, size);
        if field.is_extent() {
            let maximum = 100. - (100. - 2. * counterpart).abs() - f64::from(2. * margin_percent);
            let sign = if input < 0. { -1. } else { 1. };
            let limit = sign * (input.abs().min(maximum) as f32).max(0.01);
            let extent = field.round_input(f64::from(limit)) as f32 / 100.;
            self.origin[axis] = f64::from(counterpart as f32 / 100. - extent / 2.);
            self.size[axis] = f64::from(extent);
        } else {
            let half = counterpart.abs() / 2. + f64::from(margin_percent);
            let center = field.round_input(input.max(half).min(100. - half)) as f32 / 100.;
            self.origin[axis] = f64::from(center - size / 2.);
        }
        self
    }

    fn translated(self, delta: [f32; 2], margins: [f64; 2]) -> Option<Self> {
        if !delta.into_iter().all(f32::is_finite) {
            return None;
        }
        if delta == [0.; 2] {
            return Some(self);
        }
        let origin = [0, 1].map(|axis| {
            let span = self.size[axis];
            let left = self.origin[axis] + span.min(0.);
            let margin = margins[axis];
            let movement = f64::from(delta[axis]).min(1. - margin - span.abs() - left);
            self.origin[axis] + movement.max(margin - left)
        });
        Some(Self { origin, ..self })
    }

    fn resized(
        self,
        handle: SynthesisPatchResizeHandle,
        delta: [f32; 2],
        preserve_aspect: bool,
        from_center: bool,
        margins: [f64; 2],
    ) -> Option<Self> {
        if !delta.into_iter().all(f32::is_finite) {
            return None;
        }
        if delta == [0.; 2] {
            return Some(self);
        }
        let signs = self.size.map(f64::signum);
        let direction = handle.direction();
        let inward = [0, 1].map(|axis| f64::from(direction[axis]) * signs[axis]);
        let span = self.size.map(f64::abs);
        let limits = [0, 1].map(|axis| {
            let margin = margins[axis];
            let min = self.origin[axis] + self.size[axis].min(0.);
            let max = min + span[axis];
            let near_min = [margin - min, 1. - margin - min];
            let near_max = [max - (1. - margin), max - margin];
            if inward[axis] == 0. {
                [0.; 2]
            } else if from_center {
                [near_min[0].max(near_max[0]), near_min[1].min(near_max[1])]
            } else if inward[axis] > 0. {
                near_min
            } else {
                near_max
            }
        });
        let mut travel = [0, 1].map(|axis| f64::from(delta[axis]) * inward[axis]);
        let clamp = |value: f64, [low, high]: [f64; 2]| value.min(high).max(low);
        if preserve_aspect && !direction.contains(&0.) {
            let aspect = span[1] / span[0];
            let min = limits[0][0].max(limits[1][0] / aspect);
            let max = limits[0][1].min(limits[1][1] / aspect);
            let x = clamp(travel[0].max(travel[1] / aspect), [min, max]);
            travel = [x, x * aspect];
        } else {
            travel = [0, 1].map(|axis| clamp(travel[axis], limits[axis]));
        }
        let origin = [0, 1].map(|axis| {
            self.origin[axis]
                + if direction[axis] > 0. || from_center {
                    travel[axis] * signs[axis]
                } else {
                    0.
                }
        });
        let multiplier = if from_center { 2. } else { 1. };
        let size = [0, 1].map(|axis| {
            let size = self.size[axis] - multiplier * travel[axis] * signs[axis];
            if size.abs() < f64::from(MIN_EXTENT) {
                f64::from(MIN_EXTENT)
            } else {
                size
            }
        });
        Some(Self { origin, size })
    }

    fn fitted(self, margins: [f64; 2]) -> Self {
        let [x, y] = [0, 1].map(|axis| {
            let origin = self.origin[axis];
            let size = self.size[axis];
            let margin = margins[axis];
            if size < 0. {
                let origin = origin.max(-size + margin).min(1. - margin);
                [origin, size.max(-1. + 2. * margin)]
            } else {
                let origin = origin.min(1. - size - margin).max(margin);
                [origin, size.min(1. - origin - margin)]
            }
        });
        Self {
            origin: [x[0], y[0]],
            size: [x[1], y[1]],
        }
    }

    fn apply(self, next: Self, selection: &mut patches::Selection) {
        if self == next {
            return;
        }
        let scale = [0, 1].map(|axis| next.size[axis] / self.size[axis]);
        for knot in &mut selection.knots {
            knot.point = [0, 1].map(|axis| {
                (next.origin[axis]
                    + (f64::from(knot.point[axis]) - self.origin[axis]) * scale[axis])
                    as f32
            });
            knot.incoming =
                [0, 1].map(|axis| (f64::from(knot.incoming[axis]) * scale[axis]) as f32);
            knot.outgoing =
                [0, 1].map(|axis| (f64::from(knot.outgoing[axis]) * scale[axis]) as f32);
        }
        selection.flipped = next.size.map(|extent| extent < 0.);
    }

    fn snap_boundary_anchors(
        self,
        selection: &mut patches::Selection,
        axes: impl Iterator<Item = usize> + Clone,
    ) {
        let min = [0, 1].map(|axis| self.origin[axis] + self.size[axis].min(0.));
        let max = [0, 1].map(|axis| min[axis] + self.size[axis].abs());
        for knot in &mut selection.knots {
            for axis in axes.clone() {
                let point = knot.point[axis];
                if min[axis] <= f64::EPSILON && point.abs() <= ANCHOR_ROUNDING {
                    knot.point[axis] = 0.;
                } else if max[axis] >= 1. - f64::EPSILON && (1. - point).abs() <= ANCHOR_ROUNDING {
                    knot.point[axis] = 1.;
                }
            }
        }
    }
}

fn field_percent(field: SynthesisPatchTransformField, origin: f32, size: f32) -> f64 {
    let value = if field.is_extent() {
        size
    } else {
        origin + size / 2.
    };
    field.round_input(f64::from(value * 100.))
}

fn sampling_margins(
    selection: &patches::Selection,
    bounds: SignedBounds,
    source: Option<ImageGeometry>,
) -> Option<[f64; 2]> {
    patches::selection_sampling_margins(selection, bounds.size.map(f64::abs), source)
}

fn mean_anchor(selection: &patches::Selection) -> [f64; 2] {
    [0, 1].map(|axis| {
        selection
            .knots
            .iter()
            .map(|knot| f64::from(knot.point[axis]))
            .sum::<f64>()
            / selection.knots.len() as f64
    })
}

fn rotate(selection: &mut patches::Selection, pointer: [f32; 2], size: [f32; 2]) -> Option<()> {
    if !pointer.into_iter().all(f32::is_finite)
        || !size
            .into_iter()
            .all(|value| value.is_finite() && value > 0.)
    {
        return None;
    }
    let pivot = mean_anchor(selection).map(|value| value as f32);
    let [x, y] = [0, 1].map(|axis| pointer[axis] - pivot[axis]);
    if [x, y] == [0.; 2] {
        return None;
    }
    let degrees = 90. + 180. * y.atan2(x) / std::f32::consts::PI;
    if degrees == 0. {
        return Some(());
    }
    let (sin, cos) = f64::from(degrees).to_radians().sin_cos();
    let size = size.map(f64::from);
    let rotate = |[x, y]: [f64; 2]| {
        let (x, y) = (x * size[0], y * size[1]);
        [(cos * x - sin * y) / size[0], (sin * x + cos * y) / size[1]]
    };
    let pivot = pivot.map(f64::from);
    for knot in &mut selection.knots {
        let point = rotate([0, 1].map(|axis| f64::from(knot.point[axis]) - pivot[axis]));
        knot.point = [0, 1].map(|axis| (pivot[axis] + point[axis]) as f32);
        knot.incoming = rotate(knot.incoming.map(f64::from)).map(|value| value as f32);
        knot.outgoing = rotate(knot.outgoing.map(f64::from)).map(|value| value as f32);
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::SynthesisPatchResizeHandle::*;
    use super::*;
    use crate::pointwise::canvas::synthesis::{
        SynthesisLayerKind, SynthesisPatchShape, SynthesisTexture,
    };
    use std::sync::Arc;

    const FIT: Option<ImageGeometry> = Some(ImageGeometry::new(160, 100));

    fn draft() -> SynthesisEdit {
        let mut edit = SynthesisEdit::new(Arc::new(SynthesisTexture::default()), true);
        edit.add_layer(SynthesisLayerKind::Patches);
        edit
    }

    fn near(actual: [f32; 2], expected: [f32; 2]) {
        assert!(
            actual
                .into_iter()
                .zip(expected)
                .all(|(a, b)| (a - b).abs() < 2e-5),
            "expected {expected:?}, got {actual:?}"
        );
    }

    fn resize(
        handle: SynthesisPatchResizeHandle,
        delta: [f32; 2],
        preserve_aspect: bool,
        from_center: bool,
    ) -> SynthesisPatchTransform {
        SynthesisPatchTransform::Resize {
            handle,
            delta,
            preserve_aspect,
            from_center,
        }
    }

    #[test]
    fn whole_patch_translate_clamps_the_padded_contour_and_leaves_placements_in_place() {
        let mut edit = draft();
        edit.patches_mut(0).unwrap().selections[0].jitter = 8.;
        edit.add_placement(0, 0).unwrap();
        let before_knots = edit.patch_knots(0, 0).unwrap().collect::<Vec<_>>();
        let transform = SynthesisPatchTransform::Translate { delta: [2., -2.] };
        let preview = edit.preview_patch_transform(0, 0, transform, FIT).unwrap();
        assert_eq!(
            edit.patch_knots(0, 0).unwrap().collect::<Vec<_>>(),
            before_knots
        );
        assert!(edit.transform_patch(0, 0, transform, FIT));
        let bounds = edit.patch_transform_bounds(0, 0).unwrap();
        near(bounds.origin, [0.56, 0.04]);
        near(bounds.size, [0.4, 0.4]);
        assert_eq!(bounds, preview.patch_transform_bounds(0, 0).unwrap());
        let placement = &edit.patches(0).unwrap().manual[0];
        assert_eq!(placement.point, [0.5; 2]);
        near(placement.size, [0.4; 2]);
        assert!(!edit.transform_patch(0, 0, transform, FIT));
        assert!(!edit.transform_patch(
            0,
            0,
            SynthesisPatchTransform::Translate {
                delta: [f32::NAN, 0.]
            },
            FIT
        ));
        assert!(!edit.transform_patch(0, 0, transform, None));
    }

    #[test]
    fn whole_patch_resize_holds_the_opposite_edge_or_center_and_locks_corner_aspect() {
        let base = draft();
        for (transform, origin, size) in [
            (
                resize(East, [0.13, 0.4], true, false),
                [0.3, 0.3],
                [0.53, 0.4],
            ),
            (
                resize(East, [0.05, 0.], false, true),
                [0.25, 0.3],
                [0.5, 0.4],
            ),
            (
                resize(SouthEast, [0.02, 0.1], true, false),
                [0.3, 0.3],
                [0.42, 0.42],
            ),
            (
                resize(NorthWest, [0.05, 0.02], true, true),
                [0.35, 0.35],
                [0.3, 0.3],
            ),
            (
                resize(NorthWest, [-0.5, -0.5], true, true),
                [0., 0.],
                [1., 1.],
            ),
            (
                resize(West, [0.4, 0.], false, false),
                [0.7, 0.3],
                [MIN_EXTENT, 0.4],
            ),
        ] {
            let mut edit = base.clone();
            assert!(edit.transform_patch(0, 0, transform, FIT));
            let bounds = edit.patch_transform_bounds(0, 0).unwrap();
            near(bounds.origin, origin);
            near(bounds.size, size);
        }
    }

    #[test]
    fn whole_patch_resize_crossing_an_edge_updates_signed_placement_sizes() {
        let mut edit = draft();
        let other = edit.add_patch(0).unwrap();
        edit.add_placement(0, other).unwrap();
        edit.add_placement(0, 0).unwrap();
        edit.patches_mut(0).unwrap().manual[0].size = [0.2, 0.35];
        let original = edit.clone();
        let flip = resize(West, [0.55, 0.], false, false);
        assert!(edit.transform_patch(0, 0, flip, FIT));
        let signed = edit.patch_transform_bounds(0, 0).unwrap();
        near(signed.origin, [0.85, 0.3]);
        near(signed.size, [-0.15, 0.4]);
        assert_eq!(edit.patch_flips(0, 0), Some([true, false]));
        near(edit.patch_knots(0, 0).unwrap().next().unwrap(), [0.85, 0.3]);
        let patches = edit.patches(0).unwrap();
        near(patches.manual[0].size, [-0.075, 0.35]);
        near(patches.manual[1].size, [0.4; 2]);
        near(original.patches(0).unwrap().manual[0].size, [0.2, 0.35]);

        // New placements begin with the signed contour size. The previous
        // negative width is an initialization sentinel during another edit.
        edit.add_placement(0, 0).unwrap();
        near(edit.patches(0).unwrap().manual[0].size, [-0.15, 0.4]);
        assert!(edit.transform_patch(0, 0, resize(West, [-0.35, 0.], false, false), FIT));
        assert_eq!(edit.patch_flips(0, 0), Some([false, false]));
        let patches = edit.patches(0).unwrap();
        near(patches.manual[0].size, [-0.15, 0.4]);
        near(patches.manual[1].size, [-0.075, 0.35]);
    }

    #[test]
    fn whole_patch_rotation_uses_normalized_pointer_angle_and_fitted_pixel_axes() {
        let mut edit = draft();
        let triangle = edit
            .add_patch_shape(0, SynthesisPatchShape::Triangle, None, FIT)
            .unwrap();
        assert!(edit.transform_patch(0, triangle, resize(NorthWest, [0.3, 0.3], false, true), FIT));
        let base = edit.patch_transform_bounds(0, triangle).unwrap();
        near(base.pivot, [0.5, 0.566_666_7]);
        let operation = SynthesisPatchTransform::Rotate {
            pointer: [0.8, base.pivot[1]],
            fitted_size: [200., 100.],
        };
        let preview = edit
            .preview_patch_transform(0, triangle, operation, FIT)
            .unwrap();
        let knots = preview
            .patch_knots(0, triangle)
            .unwrap()
            .collect::<Vec<_>>();
        near(knots[0], [0.433_333_34, 0.166_666_67]);
        near(knots[1], [0.633_333_3, 0.566_666_7]);
        near(knots[2], [0.433_333_34, 0.966_666_7]);
        near(
            preview.patch_handles(0, triangle, 0).unwrap()[1],
            [0.483_333_35, 0.266_666_67],
        );
        assert!(edit.transform_patch(0, triangle, operation, FIT));
        assert_eq!(
            edit.patch_knots(0, triangle).unwrap().collect::<Vec<_>>(),
            knots
        );
        assert_eq!(edit.patch_flips(0, triangle), Some([false, false]));
    }

    #[test]
    fn whole_patch_rotation_settles_inside_the_source_only_on_release() {
        let mut edit = draft();
        let circle = edit
            .add_patch_shape(0, SynthesisPatchShape::Circle, None, FIT)
            .unwrap();
        assert!(edit.transform_patch(0, circle, resize(NorthWest, [0.3, 0.3], false, true), FIT));
        let operation = SynthesisPatchTransform::Rotate {
            pointer: [0.8, 0.5],
            fitted_size: [400., 100.],
        };
        let preview = edit
            .preview_patch_transform(0, circle, operation, FIT)
            .unwrap();
        let rotated = preview.patch_transform_bounds(0, circle).unwrap();
        near(rotated.origin, [0.45, -0.3]);
        near(rotated.size, [0.1, 1.6]);
        near(
            preview.patch_handles(0, circle, 1).unwrap()[0],
            [0.55, 0.058_175_99],
        );
        assert!(edit.transform_patch(0, circle, operation, FIT));
        let settled = edit.patch_transform_bounds(0, circle).unwrap();
        near(settled.origin, [0.45, 0.]);
        near(settled.size, [0.1, 1.]);
        assert_eq!(edit.patch_knots(0, circle).unwrap().next().unwrap()[1], 0.);
        near(
            edit.patch_handles(0, circle, 1).unwrap()[0],
            [0.55, 0.22386],
        );
        assert!(!edit.transform_patch(
            0,
            circle,
            SynthesisPatchTransform::Rotate {
                pointer: [0.8, 0.5],
                fitted_size: [400., 0.]
            },
            FIT
        ));
    }

    #[test]
    fn whole_patch_numeric_fields_match_native_padding_and_signed_placement() {
        use SynthesisPatchTransformField::*;

        let mut edit = draft();
        edit.add_placement(0, 0).unwrap();
        let patches = edit.patches_mut(0).unwrap();
        patches.selections[0].feather = 4.;
        patches.selections[0].jitter = 8.;
        patches.manual[0].size = [0.2, 0.35];
        // Original sr controls and contour transforms, OriginalPatchTransform.cs numeric.
        let cases = [
            (CenterX, 0., 25.31, [0.0531, 0.3], [0.4, 0.4], [0.2, 0.35]),
            (
                CenterY,
                9000.,
                73.9,
                [0.0531, 0.539],
                [0.4, 0.4],
                [0.2, 0.35],
            ),
            (
                Width,
                -8000.,
                -39.99,
                [0.45305, 0.539],
                [-0.3999, 0.4],
                [-0.19995, 0.35],
            ),
            (
                Width,
                17.345,
                17.34,
                [0.1664, 0.539],
                [0.1734, 0.4],
                [-0.19995, 0.35],
            ),
            (
                Height,
                -12.125,
                -12.12,
                [0.1664, 0.7996],
                [0.1734, -0.1212],
                [-0.19995, -0.10605],
            ),
            (
                Height,
                0.,
                0.01,
                [0.1664, 0.73895],
                [0.1734, 0.0001],
                [-0.19995, -0.10605],
            ),
        ];
        for (field, input, accepted, origin, size, placement) in cases {
            let before = edit.patch_transform_bounds(0, 0).unwrap();
            assert!(
                edit.set_patch_transform_field(0, 0, field, input, FIT),
                "{field:?}"
            );
            let after = edit.patch_transform_bounds(0, 0).unwrap();
            near(after.origin, origin);
            near(after.size, size);
            let other = 1 - field.axis();
            assert_eq!(after.origin[other], before.origin[other]);
            assert_eq!(after.size[other], before.size[other]);
            assert_eq!(after.field_percent(field), accepted);
            assert_eq!(edit.patch_flips(0, 0), Some(size.map(|extent| extent < 0.)));
            let manual = &edit.patches(0).unwrap().manual[0];
            near(manual.size, placement);
            assert_eq!(manual.point, [0.5; 2]);
        }
        assert!(!edit.set_patch_transform_field(0, 0, Height, 0.01, FIT));
        assert!(!edit.set_patch_transform_field(0, 0, CenterX, f64::NAN, FIT));
        assert!(!edit.set_patch_transform_field(0, 0, CenterX, 80., None));
    }
}
