use super::{SynthesisEdit, SynthesisPatchResizeHandle, patches};
use crate::ImageGeometry;
use std::sync::Arc;

mod list;
pub use list::SynthesisPlacementClipboard;

/// Numeric controls for the active Manual copy. Center and size use percent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SynthesisPlacementField {
    CenterX,
    CenterY,
    Width,
    Height,
    Angle,
}

impl SynthesisPlacementField {
    /// The outer numeric editor's limits, including signed copy sizes.
    pub fn limits(self) -> [f64; 2] {
        match self {
            Self::CenterX | Self::CenterY => [0., 10_000.],
            Self::Width | Self::Height => [-10_000., 10_000.],
            Self::Angle => [-360., 360.],
        }
    }
}

/// A Manual placement gesture relative to the placement at mouse-down.
/// Positions and deltas use the square output tile before view zoom.
#[derive(Clone, Copy, Debug)]
pub enum SynthesisPlacementTransform {
    /// Set `wrap` on release outside the editor. Drag previews can cross tile edges.
    Move {
        delta: [f32; 2],
        wrap: bool,
    },
    Resize {
        handle: SynthesisPatchResizeHandle,
        delta: [f32; 2],
        preserve_aspect: bool,
        from_center: bool,
    },
    Rotate {
        pointer: [f32; 2],
    },
}

/// The source contour and authored geometry of one Manual placement.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SynthesisPlacementGeometry {
    identity: usize,
    patch: usize,
    copy: i32,
    center: [f32; 2],
    size: [f32; 2],
    angle: f32,
}

impl SynthesisPlacementGeometry {
    /// Session identity, stable across selecting, editing, and reordering this copy.
    pub fn identity(self) -> usize {
        self.identity
    }

    /// Zero-based source contour index.
    pub fn patch(self) -> usize {
        self.patch
    }

    /// Copy number shown alongside the source contour in the placement list.
    pub fn copy_number(self) -> i32 {
        self.copy
    }

    pub fn center(self) -> [f32; 2] {
        self.center
    }

    /// The saved signed dimensions before correction for the photo's aspect ratio.
    pub fn size(self) -> [f32; 2] {
        self.size
    }

    /// The saved angle in degrees. Positive angles rotate toward the left of the tile.
    pub fn angle(self) -> f32 {
        self.angle
    }

    /// Value shown in the Manual inspector, in percent or degrees.
    pub fn field(self, field: SynthesisPlacementField) -> f64 {
        use SynthesisPlacementField::*;
        let value = self.raw_field(field);
        let value = match field {
            CenterX | CenterY if !(0.0..=100.0).contains(&value) => {
                let wrapped = value.rem_euclid(100.);
                if wrapped == 0. && value > 100. {
                    100.
                } else {
                    wrapped
                }
            }
            _ => value,
        };
        let [min, max] = field.limits();
        round_manual_field(value.clamp(min, max))
    }

    /// The typed value before the inspector wraps its center at a selection refresh.
    pub fn authored_field(self, field: SynthesisPlacementField) -> f64 {
        round_manual_field(self.raw_field(field))
    }

    fn raw_field(self, field: SynthesisPlacementField) -> f64 {
        use SynthesisPlacementField::*;
        match field {
            CenterX => raw_percent(self.center[0]),
            CenterY => raw_percent(self.center[1]),
            Width => raw_percent(self.size[0]),
            Height => raw_percent(self.size[1]),
            Angle => f64::from(self.angle),
        }
    }

    pub fn resize_handle(
        self,
        handle: SynthesisPatchResizeHandle,
        source: ImageGeometry,
    ) -> Option<[f32; 2]> {
        let aspect = source_scale(source)?;
        let direction = outward(handle);
        let offset = [0, 1].map(|axis| direction[axis] * self.size[axis].abs() * aspect[axis] / 2.);
        let (sin, cos) = rotation(self.angle);
        let point = add(self.center, to_tile(offset, sin, cos));
        finite(point).then_some(point)
    }

    /// `gap` is the desired handle offset in normalized tile units.
    pub fn rotation_handle(self, source: ImageGeometry, gap: f32) -> Option<[f32; 2]> {
        if !gap.is_finite() || gap < 0. {
            return None;
        }
        let top = self.resize_handle(SynthesisPatchResizeHandle::North, source)?;
        let (sin, cos) = rotation(self.angle);
        let point = add(top, to_tile([0., -gap], sin, cos));
        finite(point).then_some(point)
    }

    fn read(patches: &patches::Patches, placement: usize) -> Option<Self> {
        let value = patches.manual.get(placement)?;
        let patch = usize::try_from(value.patch).ok()?;
        patches.selections.get(patch)?;
        let geometry = Self {
            identity: Arc::as_ptr(&value.identity) as usize,
            patch,
            copy: value.sequence,
            center: value.point,
            size: value.size,
            angle: value.angle,
        };
        geometry.is_finite().then_some(geometry)
    }

    fn is_finite(self) -> bool {
        finite(self.center) && finite(self.size) && self.angle.is_finite()
    }

    fn project(mut self, transform: SynthesisPlacementTransform, aspect: [f32; 2]) -> Option<Self> {
        match transform {
            SynthesisPlacementTransform::Move { delta, wrap } => {
                self.center = add(self.center, delta);
                if wrap {
                    self.center = self.center.map(wrap_center);
                }
            }
            SynthesisPlacementTransform::Resize {
                handle,
                delta,
                preserve_aspect,
                from_center,
            } => self.resize(outward(handle), delta, aspect, preserve_aspect, from_center),
            SynthesisPlacementTransform::Rotate { pointer } => {
                if !finite(pointer) {
                    return None;
                }
                let [x, y] = [0, 1].map(|axis| f64::from(pointer[axis] - self.center[axis]));
                let radius = x.hypot(y);
                if radius == 0. || !radius.is_finite() {
                    return None;
                }
                // Original angle zero points up the tile.
                let angle = ((-y / radius).clamp(-1., 1.)).acos() as f32;
                self.angle = 180. * if x > 0. { -angle } else { angle } / std::f32::consts::PI;
            }
        }
        self.is_finite().then_some(self)
    }

    fn resize(
        &mut self,
        direction: [f32; 2],
        delta: [f32; 2],
        aspect: [f32; 2],
        preserve_aspect: bool,
        from_center: bool,
    ) {
        // Project the pointer onto the unrotated box's local axes. The
        // outward directions use the displayed, unsigned handle positions.
        let (sin, cos) = rotation(self.angle);
        let [dx, dy] = delta;
        let local = [dx * cos - dy * sin, dx * sin + dy * cos];
        let multiplier = if from_center { 2. } else { 1. };
        let mut expansion = [0, 1].map(|axis| local[axis] * direction[axis] * multiplier);
        let ratio = ((self.size[1] * aspect[1]) / (self.size[0] * aspect[0])).abs();
        let corner = !direction.contains(&0.);
        let constrained = preserve_aspect && corner && ratio.is_finite() && ratio > 0.;
        if constrained {
            if expansion[0] * ratio < expansion[1] {
                expansion[1] = expansion[0] * ratio;
            } else {
                expansion[0] = expansion[1] / ratio;
            }
        }
        if !from_center {
            let displacement = if corner && !constrained {
                delta.map(|delta| delta / 2.)
            } else {
                let offset = [0, 1].map(|axis| direction[axis] * expansion[axis] / 2.);
                to_tile(offset, sin, cos)
            };
            self.center = add(self.center, displacement);
        }
        for axis in 0..2 {
            let sign = if self.size[axis] < 0. { -1. } else { 1. };
            self.size[axis] += sign * expansion[axis] / aspect[axis];
        }
    }
}

impl SynthesisEdit {
    /// Edits an individual Manual inspector value without rewriting its peers.
    pub fn set_manual_placement_field(
        &mut self,
        layer: usize,
        index: usize,
        field: SynthesisPlacementField,
        value: f64,
    ) -> bool {
        use SynthesisPlacementField::*;
        if !value.is_finite() {
            return false;
        }
        let [min, max] = field.limits();
        let value = round_manual_field(value.clamp(min, max));
        let Some(geometry) = self.placement_geometry(layer, index) else {
            return false;
        };
        if geometry.authored_field(field) == value {
            return false;
        }
        let Some(copy) = self
            .patches_mut(layer)
            .and_then(|p| p.manual.get_mut(index))
        else {
            return false;
        };
        let result = match field {
            CenterX => (&mut copy.point[0], (value as f32) / 100.),
            CenterY => (&mut copy.point[1], (value as f32) / 100.),
            Width => (&mut copy.size[0], (value / 100.) as f32),
            Height => (&mut copy.size[1], (value / 100.) as f32),
            Angle => (&mut copy.angle, value as f32),
        };
        if *result.0 == result.1 {
            return false;
        }
        *result.0 = result.1;
        self.changed = true;
        true
    }

    pub fn placement_geometry(
        &self,
        layer: usize,
        placement: usize,
    ) -> Option<SynthesisPlacementGeometry> {
        SynthesisPlacementGeometry::read(self.patches(layer)?, placement)
    }

    /// Projects a Manual placement from the same state used by its pointer gesture.
    pub fn placement_for_drag(
        &self,
        layer: usize,
        placement: usize,
        transform: SynthesisPlacementTransform,
        source: ImageGeometry,
    ) -> Option<SynthesisPlacementGeometry> {
        self.placement_geometry(layer, placement)?
            .project(transform, source_scale(source)?)
    }

    /// Applies the Manual placement geometry, keeping its contour and copy number.
    pub fn transform_placement(
        &mut self,
        layer: usize,
        placement: usize,
        transform: SynthesisPlacementTransform,
        source: ImageGeometry,
    ) -> bool {
        let before = self.placement_geometry(layer, placement);
        let Some(geometry) = self
            .placement_for_drag(layer, placement, transform, source)
            .filter(|&geometry| before != Some(geometry))
        else {
            return false;
        };
        let item = &mut self
            .patches_mut(layer)
            .expect("validated placement layer")
            .manual[placement];
        item.point = geometry.center;
        item.size = geometry.size;
        item.angle = geometry.angle;
        self.changed = true;
        true
    }

    /// Projects the contour as it appears in the Manual output guide.
    /// Supply either the current or an in-progress placement from this layer.
    pub fn placement_outline(
        &self,
        layer: usize,
        geometry: SynthesisPlacementGeometry,
        source: ImageGeometry,
    ) -> Option<Vec<[f32; 2]>> {
        let selection = self.patches(layer)?.selections.get(geometry.patch)?;
        let (mut origin, mut size) = patches::selection_bounds(selection)?;
        if size.contains(&0.) {
            return None;
        }
        // Fitting the source contour uses its signed origin and size, as
        // in aj4.c. The placement independently controls the final flip.
        for axis in 0..2 {
            if selection.flipped[axis] {
                origin[axis] += size[axis];
                size[axis] = -size[axis];
            }
        }
        let aspect = source_scale(source)?;
        let extent = [0, 1].map(|axis| f64::from(geometry.size[axis] * aspect[axis]));
        let (sin, cos) = rotation(geometry.angle);
        patches::selection_outline(selection)?
            .into_iter()
            .map(|point| {
                let offset = [0, 1].map(|axis| {
                    (((f64::from(point[axis]) - origin[axis]) / size[axis] - 0.5) * extent[axis])
                        as f32
                });
                let point = add(geometry.center, to_tile(offset, sin, cos));
                finite(point).then_some(point)
            })
            .collect()
    }
}

fn outward(handle: SynthesisPatchResizeHandle) -> [f32; 2] {
    use SynthesisPatchResizeHandle::*;
    match handle {
        NorthWest => [-1., -1.],
        North => [0., -1.],
        NorthEast => [1., -1.],
        East => [1., 0.],
        SouthEast => [1., 1.],
        South => [0., 1.],
        SouthWest => [-1., 1.],
        West => [-1., 0.],
    }
}

fn source_scale(source: ImageGeometry) -> Option<[f32; 2]> {
    let width = source.width() as f32;
    let height = source.height() as f32;
    if width <= 0. || height <= 0. {
        return None;
    }
    let largest = width.max(height);
    Some([width / largest, height / largest])
}

fn rotation(angle: f32) -> (f32, f32) {
    let (sin, cos) = f64::from(angle).to_radians().sin_cos();
    (sin as f32, cos as f32)
}

fn to_tile([x, y]: [f32; 2], sin: f32, cos: f32) -> [f32; 2] {
    [x * cos + y * sin, -x * sin + y * cos]
}

fn add(left: [f32; 2], right: [f32; 2]) -> [f32; 2] {
    [0, 1].map(|axis| left[axis] + right[axis])
}

fn finite(point: [f32; 2]) -> bool {
    point.into_iter().all(f32::is_finite)
}

fn wrap_center(value: f32) -> f32 {
    // The native loop keeps both endpoints, including 1 after positive
    // whole-tile movement and 0 after negative whole-tile movement.
    if value > 1. {
        let fraction = value.fract();
        if fraction == 0. { 1. } else { fraction }
    } else if value < 0. {
        let remaining = value.fract();
        if remaining == 0. { 0. } else { remaining + 1. }
    } else {
        value
    }
}

fn round_manual_field(value: f64) -> f64 {
    crate::round_decimal_places(value, 2).expect("two decimal places")
}

fn raw_percent(value: f32) -> f64 {
    let percent = value * 100.;
    if percent.is_finite() {
        f64::from(percent)
    } else {
        f64::from(value) * 100.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        SynthesisLayerKind, SynthesisPatchShape, pointwise::canvas::synthesis::SynthesisTexture,
    };
    use std::sync::Arc;

    const LANDSCAPE: ImageGeometry = ImageGeometry::new(200, 100);
    const PORTRAIT: ImageGeometry = ImageGeometry::new(100, 200);

    fn draft() -> SynthesisEdit {
        let mut edit = SynthesisEdit::new(Arc::new(SynthesisTexture::default()), true);
        edit.add_layer(SynthesisLayerKind::Patches);
        assert_eq!(edit.add_placement(0, 0), Some(0));
        edit.patches_mut(0).unwrap().manual[0].size = [0.4, 0.5];
        edit
    }

    fn close(actual: [f32; 2], expected: [f32; 2]) {
        assert!(
            actual
                .into_iter()
                .zip(expected)
                .all(|(a, b)| (a - b).abs() < 1e-5),
            "{actual:?} != {expected:?}"
        );
    }

    fn resize(
        handle: SynthesisPatchResizeHandle,
        delta: [f32; 2],
        shift: bool,
        control: bool,
    ) -> SynthesisPlacementTransform {
        SynthesisPlacementTransform::Resize {
            handle,
            delta,
            preserve_aspect: shift,
            from_center: control,
        }
    }

    #[test]
    fn manual_tile_handles_use_unsigned_extents_and_photo_aspect() {
        use SynthesisPatchResizeHandle::{North, NorthWest, SouthEast};
        let mut edit = draft();
        let initial = edit.placement_geometry(0, 0).unwrap();
        close(
            initial.resize_handle(NorthWest, LANDSCAPE).unwrap(),
            [0.3, 0.375],
        );
        close(
            initial.resize_handle(SouthEast, PORTRAIT).unwrap(),
            [0.6, 0.75],
        );
        close(
            initial.rotation_handle(LANDSCAPE, 0.025).unwrap(),
            [0.5, 0.35],
        );
        let raw = &mut edit.patches_mut(0).unwrap().manual[0];
        raw.size[0] = -raw.size[0];
        raw.angle = 90.;
        let rotated = edit.placement_geometry(0, 0).unwrap();
        close(
            rotated.resize_handle(North, LANDSCAPE).unwrap(),
            [0.375, 0.5],
        );
        close(
            rotated.rotation_handle(LANDSCAPE, 0.025).unwrap(),
            [0.35, 0.5],
        );
        assert_eq!(rotated.size(), [-0.4, 0.5]);
        assert_eq!((rotated.patch(), rotated.copy_number()), (0, 1));
    }

    #[test]
    fn manual_drag_wraps_at_release_and_keeps_the_inclusive_seam() {
        let mut edit = draft();
        let initial = edit.placement_geometry(0, 0).unwrap();
        let delta = [2.5, -1.5];
        let ongoing = SynthesisPlacementTransform::Move { delta, wrap: false };
        let released = SynthesisPlacementTransform::Move { delta, wrap: true };
        assert_eq!(
            edit.placement_for_drag(0, 0, ongoing, LANDSCAPE)
                .unwrap()
                .center(),
            [3., -1.]
        );
        let committed = edit.placement_for_drag(0, 0, released, LANDSCAPE).unwrap();
        assert_eq!(committed.center(), [1., 0.]);
        assert_eq!(edit.placement_geometry(0, 0), Some(initial));
        assert!(edit.transform_placement(0, 0, released, LANDSCAPE));
        assert_eq!(edit.placement_geometry(0, 0), Some(committed));
        assert_eq!((committed.patch(), committed.copy_number()), (0, 1));
        assert_eq!(committed.size(), initial.size());
        assert_eq!(committed.angle(), initial.angle());
        assert!(
            edit.placement_for_drag(0, 0, ongoing, ImageGeometry::new(0, 1))
                .is_none()
        );
        assert!(!edit.transform_placement(
            0,
            0,
            SynthesisPlacementTransform::Move {
                delta: [f32::NAN, 0.],
                wrap: true
            },
            LANDSCAPE
        ));
        assert_eq!(edit.placement_geometry(0, 0), Some(committed));
    }

    #[test]
    fn manual_edge_resize_uses_rotated_axes_and_can_cross_through_zero() {
        use SynthesisPatchResizeHandle::{East, West};
        let mut edit = draft();
        edit.patches_mut(0).unwrap().manual[0].angle = 90.;
        let initial = edit.placement_geometry(0, 0).unwrap();
        let moved = edit
            .placement_for_drag(0, 0, resize(East, [0.06, -0.1], true, false), LANDSCAPE)
            .unwrap();
        close(moved.center(), [0.5, 0.45]);
        close(moved.size(), [0.5, 0.5]);
        close(
            moved.resize_handle(West, LANDSCAPE).unwrap(),
            initial.resize_handle(West, LANDSCAPE).unwrap(),
        );
        let flip = resize(East, [0., 0.6], false, false);
        assert!(edit.transform_placement(0, 0, flip, LANDSCAPE));
        let flipped = edit.placement_geometry(0, 0).unwrap();
        close(flipped.center(), [0.5, 0.8]);
        close(flipped.size(), [-0.2, 0.5]);
        let next = edit
            .placement_for_drag(0, 0, resize(East, [0., -0.1], false, true), LANDSCAPE)
            .unwrap();
        close(next.center(), flipped.center());
        close(next.size(), [-0.4, 0.5]);
    }

    #[test]
    fn manual_corner_shift_locks_visible_aspect_and_control_keeps_center() {
        use SynthesisPatchResizeHandle::{NorthWest, SouthEast};
        let edit = draft();
        let initial = edit.placement_geometry(0, 0).unwrap();
        let delta = [0.1, 0.1];
        let next = edit
            .placement_for_drag(0, 0, resize(SouthEast, delta, true, false), LANDSCAPE)
            .unwrap();
        close(next.size(), [0.5, 0.625]);
        close(next.center(), [0.55, 0.53125]);
        close(
            next.resize_handle(NorthWest, LANDSCAPE).unwrap(),
            initial.resize_handle(NorthWest, LANDSCAPE).unwrap(),
        );
        let centered = edit
            .placement_for_drag(0, 0, resize(SouthEast, delta, true, true), LANDSCAPE)
            .unwrap();
        close(centered.size(), [0.6, 0.75]);
        assert_eq!(centered.center(), initial.center());
    }

    #[test]
    fn manual_rotation_uses_the_center_and_zero_at_top() {
        let edit = draft();
        let initial = edit.placement_geometry(0, 0).unwrap();
        for (pointer, angle) in [
            ([0.5, 0.25], 0.),
            ([0.75, 0.5], -90.),
            ([0.25, 0.5], 90.),
            ([0.5, 0.75], 180.),
        ] {
            let next = edit
                .placement_for_drag(
                    0,
                    0,
                    SynthesisPlacementTransform::Rotate { pointer },
                    LANDSCAPE,
                )
                .unwrap();
            assert!((next.angle() - angle).abs() < 1e-5);
            assert_eq!(next.center(), initial.center());
            assert_eq!(next.size(), initial.size());
        }
        assert!(
            edit.placement_for_drag(
                0,
                0,
                SynthesisPlacementTransform::Rotate {
                    pointer: initial.center()
                },
                LANDSCAPE
            )
            .is_none()
        );
    }

    #[test]
    fn manual_outline_uses_the_destination_flip_independent_of_the_source() {
        let mut edit = draft();
        let triangle = edit
            .add_patch_shape(0, SynthesisPatchShape::Triangle, None, Some(LANDSCAPE))
            .unwrap();
        edit.add_placement(0, triangle).unwrap();
        edit.patches_mut(0).unwrap().manual[0].size = [0.6, 0.4];
        let before = edit.placement_geometry(0, 0).unwrap();
        let outline = edit.placement_outline(0, before, LANDSCAPE).unwrap();
        let source = &mut edit.patches_mut(0).unwrap().selections[triangle];
        source.flipped[1] = true;
        for knot in &mut source.knots {
            knot.point[1] = 1. - knot.point[1];
            knot.incoming[1] *= -1.;
            knot.outgoing[1] *= -1.;
        }
        let projected = edit.placement_outline(0, before, LANDSCAPE).unwrap();
        assert_eq!(outline.len(), projected.len());
        for (expected, actual) in outline.iter().copied().zip(projected) {
            close(actual, expected);
        }
        edit.patches_mut(0).unwrap().manual[0].size[1] = -before.size()[1];
        let reversed = edit.placement_geometry(0, 0).unwrap();
        for (expected, actual) in outline
            .into_iter()
            .zip(edit.placement_outline(0, reversed, LANDSCAPE).unwrap())
        {
            close(actual, [expected[0], 1. - expected[1]]);
        }
    }
}
