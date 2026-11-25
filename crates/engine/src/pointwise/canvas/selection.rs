//! Retained handles for editable Canvas geometry.
use super::{
    CanvasDocument, CanvasLayerEditor, CanvasPath, CanvasPrimitive, CanvasShape, PointwiseError,
    PointwiseFilter, hierarchy::prune_small_groups, shape::Shape,
};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

mod curves;
mod transform;
pub use transform::{
    CanvasAlignment, CanvasHandleDrag, CanvasHandleModifiers, CanvasHandlePreview,
    CanvasSelectionEdit, CanvasSelectionField, CanvasSelectionTransform, CanvasTransformFrame,
    CanvasTransformHandle, CanvasTransformOrientation,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CanvasShapeField {
    CenterX,
    CenterY,
    Width,
    Height,
    SkewX,
    SkewY,
    Angle,
    StrokeWidth,
    Control(&'static str),
}

impl CanvasShapeField {
    const TRANSFORM: [Self; 7] = [
        Self::CenterX,
        Self::CenterY,
        Self::Width,
        Self::Height,
        Self::SkewX,
        Self::SkewY,
        Self::Angle,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::CenterX => "Center X",
            Self::CenterY => "Center Y",
            Self::Width => "Width",
            Self::Height => "Height",
            Self::SkewX => "Skew X",
            Self::SkewY => "Skew Y",
            Self::Angle => "Angle",
            Self::StrokeWidth => "Stroke width",
            Self::Control("Tickness") => "Thickness",
            Self::Control(name) => name,
        }
    }

    pub fn bounds(self) -> [f32; 2] {
        match self {
            Self::CenterX | Self::CenterY => [-10000., 10000.],
            Self::Width | Self::Height | Self::SkewX | Self::SkewY => [-1000., 1000.],
            Self::Angle => [-360., 360.],
            Self::StrokeWidth => [0., 100.],
            Self::Control("Start Angle" | "End Angle") => [0., 360.],
            Self::Control("Sides") => [3., 50.],
            Self::Control("Petals" | "Points" | "Teeth") => [3., 100.],
            Self::Control("Inner Tension" | "Outer Tension" | "Direction" | "Twist") => {
                [-100., 100.]
            }
            Self::Control(_) => [0., 100.],
        }
    }

    pub fn is_integer(self) -> bool {
        matches!(self, Self::Control("Sides" | "Petals" | "Points" | "Teeth"))
    }

    // The position of inherited fields is defined by the GTX wire record.
    fn axis(self) -> Option<usize> {
        Self::TRANSFORM.iter().position(|&field| field == self)
    }
}

#[derive(Clone)]
pub struct CanvasShapeValues {
    transform: [f32; 7],
    controls: Vec<(&'static str, f32)>,
    stroke_width: Option<f32>,
}

impl CanvasShapeValues {
    fn from_shape(shape: &Shape) -> Self {
        let controls = CanvasPrimitive::from_tag(shape.kind)
            .into_iter()
            .flat_map(CanvasPrimitive::default_controls)
            .filter_map(|&(name, _)| {
                shape
                    .base
                    .controls
                    .iter()
                    .rev()
                    .find(|(key, _)| key.iter().copied().eq(name.encode_utf16()))
                    .map(|(_, value)| (name, *value))
            })
            .collect();
        Self {
            transform: shape.base.transform,
            controls,
            stroke_width: shape.open_stroke_width(),
        }
    }

    pub fn fields(&self) -> impl Iterator<Item = (CanvasShapeField, f32)> + '_ {
        CanvasShapeField::TRANSFORM
            .into_iter()
            .zip(self.transform)
            .chain(
                self.stroke_width
                    .map(|value| (CanvasShapeField::StrokeWidth, value)),
            )
            .chain(
                self.controls
                    .iter()
                    .map(|&(name, value)| (CanvasShapeField::Control(name), value)),
            )
    }

    pub fn number(&self, field: CanvasShapeField) -> Option<f32> {
        self.fields()
            .find_map(|(candidate, value)| (candidate == field).then_some(value))
    }

    pub fn set_number(&mut self, field: CanvasShapeField, value: f32) -> bool {
        let [min, max] = field.bounds();
        if !(min..=max).contains(&value) || (field.is_integer() && value.fract() != 0.) {
            return false;
        }
        if let Some(index) = field.axis() {
            self.transform[index] = match field {
                CanvasShapeField::Width | CanvasShapeField::Height if value.abs() < 0.01 => 0.01,
                _ => value,
            };
            return true;
        }
        if field == CanvasShapeField::StrokeWidth
            && let Some(width) = &mut self.stroke_width
        {
            *width = value;
            return true;
        }
        if let CanvasShapeField::Control(name) = field
            && let Some((_, current)) = self.controls.iter_mut().find(|(key, _)| *key == name)
        {
            *current = value;
            return true;
        }
        false
    }

    fn same_values(&self, other: &Self) -> bool {
        self.fields()
            .map(|(key, value)| (key, value.to_bits()))
            .eq(other.fields().map(|(key, value)| (key, value.to_bits())))
    }

    fn apply_transform(&self, shape: &mut Shape, expected: &Self, derived: bool) -> bool {
        if !self
            .fields()
            .map(|(key, _)| key)
            .eq(expected.fields().map(|(key, _)| key))
        {
            return false;
        }
        for ((field, value), (_, old)) in self.fields().zip(expected.fields()) {
            if value.to_bits() == old.to_bits() {
                continue;
            }
            // A snapshot from another shape may carry loaded values outside
            // the authoring bounds. Validate only edits to preserve old records.
            let [min, max] = field.bounds();
            if !value.is_finite()
                || (!(derived && field.axis().is_some()) && !(min..=max).contains(&value))
                || (field.is_integer() && value.fract() != 0.)
            {
                return false;
            }
            if let Some(index) = field.axis() {
                shape.base.transform[index] = value;
            } else if field == CanvasShapeField::StrokeWidth {
                shape.set_path_width(value);
            } else if let CanvasShapeField::Control(name) = field
                && let Some((_, current)) = shape
                    .base
                    .controls
                    .iter_mut()
                    .rev()
                    .find(|(key, _)| key.iter().copied().eq(name.encode_utf16()))
            {
                *current = value;
            }
        }
        // The original setters nudge the last edited skew if the new affine
        // basis is singular. Width and height use their own 0.01 floor.
        let [_, _, width, height, x, y, _] = shape.base.transform;
        if !derived && width * height - 0.25 * x * y == 0. {
            if y != expected.transform[5] {
                shape.base.transform[5] += 0.01;
            } else if x != expected.transform[4] {
                shape.base.transform[4] += 0.01;
            }
        }
        shape.apply_path_transform(expected.transform)
    }
}

pub struct CanvasShapeEditor<E> {
    layer: CanvasLayerEditor<E>,
    identity: Arc<()>,
    label: String,
}

impl<E> Clone for CanvasShapeEditor<E> {
    fn clone(&self) -> Self {
        Self {
            layer: self.layer.clone(),
            identity: self.identity.clone(),
            label: self.label.clone(),
        }
    }
}

impl<E: 'static> CanvasShapeEditor<E> {
    pub fn same_reference(&self, other: &Self) -> bool {
        self.layer.same_reference(&other.layer) && Arc::ptr_eq(&self.identity, &other.identity)
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn identity(&self) -> usize {
        Arc::as_ptr(&self.identity) as usize
    }

    pub fn layer(&self) -> CanvasLayerEditor<E> {
        self.layer.clone()
    }

    pub fn settings(&self) -> Result<CanvasShapeValues, PointwiseError<E>> {
        let canvas = self
            .layer
            .owner
            .canvas_document()
            .ok_or(PointwiseError::InvalidCast)?;
        self.shape_in(&canvas)
            .map(CanvasShapeValues::from_shape)
            .ok_or(PointwiseError::InvalidOperation)
    }

    /// Outline in tile coordinates. A proposed transform can be displayed
    /// without mutating the document or registering a history command.
    pub fn outline(
        &self,
        proposed: Option<&CanvasShapeValues>,
    ) -> Result<Vec<Vec<[f64; 2]>>, PointwiseError<E>> {
        self.outline_with_transform(proposed, false, false)
    }

    pub fn guide_outline(
        &self,
        proposed: Option<&CanvasShapeValues>,
    ) -> Result<Vec<Vec<[f64; 2]>>, PointwiseError<E>> {
        self.outline_with_transform(proposed, false, true)
    }

    fn outline_with_transform(
        &self,
        proposed: Option<&CanvasShapeValues>,
        derived: bool,
        guide: bool,
    ) -> Result<Vec<Vec<[f64; 2]>>, PointwiseError<E>> {
        let canvas = self
            .layer
            .owner
            .canvas_document()
            .ok_or(PointwiseError::InvalidCast)?;
        let shape = self
            .shape_in(&canvas)
            .ok_or(PointwiseError::InvalidOperation)?;
        let outline = |shape: &Shape| {
            if guide {
                shape.guide_outline()
            } else {
                shape.outline()
            }
        };
        if let Some(proposed) = proposed {
            let mut draft = shape.clone();
            if !proposed.apply_transform(&mut draft, &CanvasShapeValues::from_shape(shape), derived)
            {
                return Err(PointwiseError::InvalidOperation);
            }
            outline(&draft)
        } else {
            outline(shape)
        }
    }

    pub fn edit(
        &self,
        expected: &CanvasShapeValues,
        proposed: &CanvasShapeValues,
    ) -> Result<(), PointwiseError<E>> {
        self.layer
            .owner
            .edit_canvas_shapes([(self, expected, proposed)])
    }

    pub fn edit_remove(&self) -> Result<(), PointwiseError<E>> {
        self.layer
            .owner
            .edit_remove_canvas_shapes(std::slice::from_ref(self))
    }

    pub fn edit_insert_before(
        &self,
        primitive: CanvasPrimitive,
        bounds: super::CanvasShapeBounds,
    ) -> Result<Self, PointwiseError<E>> {
        self.insert_before(CanvasShape::primitive(primitive, bounds), primitive.name())
    }

    pub fn edit_insert_path_before(&self, path: CanvasPath) -> Result<Self, PointwiseError<E>> {
        self.insert_before(CanvasShape::from_canvas_path(path)?, "Curve")
    }

    fn insert_before(&self, item: CanvasShape, label: &str) -> Result<Self, PointwiseError<E>> {
        let CanvasShape::Shape(shape) = &item else {
            unreachable!();
        };
        let identity = shape.identity.clone();
        self.layer.edit_item_siblings(
            &self.identity,
            &format!("Add {label}"),
            move |siblings, index| {
                siblings.insert(index, item);
                true
            },
        )?;
        Ok(Self {
            layer: self.layer.clone(),
            identity,
            label: format!("{} / {label}", self.layer.label()),
        })
    }

    pub fn path(&self) -> Option<CanvasPath> {
        let canvas = self.layer.owner.canvas_document()?;
        self.shape_in(&canvas)?.path().cloned()
    }

    pub fn edit_path(
        &self,
        expected: &CanvasPath,
        path: CanvasPath,
    ) -> Result<bool, PointwiseError<E>> {
        let (before, layer) = self.layer.current_layer()?;
        if self.shape_in(&before).and_then(Shape::path) != Some(expected) {
            return Err(PointwiseError::InvalidOperation);
        }
        if expected == &path {
            return Ok(false);
        }
        let mut after = (*before).clone();
        let shape = find_shape_mut(&mut after.layers[layer].shapes, &self.identity)
            .ok_or(PointwiseError::InvalidOperation)?;
        shape.edit_path_points(path)?;
        self.layer
            .owner
            .commit_canvas_edit(before, after, "Edit Canvas curve")?;
        Ok(true)
    }

    /// Removes a Curve after its last surviving segment has been deleted.
    pub fn edit_remove_curve(&self, expected: &CanvasPath) -> Result<(), PointwiseError<E>> {
        let owner = &self.layer.owner;
        let before = owner.canvas_document().ok_or(PointwiseError::InvalidCast)?;
        if self.shape_in(&before).and_then(Shape::path) != Some(expected) {
            return Err(PointwiseError::InvalidOperation);
        }
        owner.remove_canvas_shapes(before, HashSet::from([self.identity()]))
    }

    fn shape_in<'a>(&self, document: &'a CanvasDocument) -> Option<&'a Shape> {
        let layer = &document.layers[self.layer.index_in(document)?];
        find_shape(&layer.shapes, &self.identity)
    }
}

fn find_shape<'a>(items: &'a [CanvasShape], identity: &Arc<()>) -> Option<&'a Shape> {
    items.iter().find_map(|item| match item {
        CanvasShape::Shape(shape) if Arc::ptr_eq(&shape.identity, identity) => Some(shape.as_ref()),
        CanvasShape::Group(group) => find_shape(&group.children, identity),
        _ => None,
    })
}

fn find_shape_mut<'a>(items: &'a mut [CanvasShape], identity: &Arc<()>) -> Option<&'a mut Shape> {
    items.iter_mut().find_map(|item| match item {
        CanvasShape::Shape(shape) if Arc::ptr_eq(&shape.identity, identity) => Some(shape.as_mut()),
        CanvasShape::Group(group) => find_shape_mut(&mut group.children, identity),
        _ => None,
    })
}

fn remove_shapes(items: &mut Vec<CanvasShape>, selection: &mut HashSet<usize>) {
    items.retain_mut(|item| match item {
        CanvasShape::Shape(shape) => !selection.remove(&(Arc::as_ptr(&shape.identity) as usize)),
        CanvasShape::Group(group) => {
            remove_shapes(&mut group.children, selection);
            !selection.remove(&(Arc::as_ptr(&group.identity) as usize))
        }
        _ => true,
    });
}

type ShapeValuesByIdentity<'a> = HashMap<usize, (&'a CanvasShapeValues, &'a CanvasShapeValues)>;

fn apply_shape_edits(
    items: &mut [CanvasShape],
    edits: &mut ShapeValuesByIdentity<'_>,
    changed: &mut bool,
    derived: bool,
) -> bool {
    for item in items {
        match item {
            CanvasShape::Shape(shape) => {
                let Some((expected, proposed)) =
                    edits.remove(&(Arc::as_ptr(&shape.identity) as usize))
                else {
                    continue;
                };
                if !expected.same_values(&CanvasShapeValues::from_shape(shape)) {
                    return false;
                }
                if !expected.same_values(proposed) {
                    if !proposed.apply_transform(shape, expected, derived) {
                        return false;
                    }
                    *changed = true;
                }
            }
            CanvasShape::Group(group) => {
                if !apply_shape_edits(&mut group.children, edits, changed, derived) {
                    return false;
                }
            }
            CanvasShape::Layer(_) => {}
        }
    }
    true
}

pub(super) fn visit_shapes(
    items: &[CanvasShape],
    visible: bool,
    visit: &mut impl FnMut(&Shape, bool),
) {
    for item in items {
        match item {
            CanvasShape::Shape(shape) if shape.editable() => {
                visit(shape, visible && shape.header.visible())
            }
            CanvasShape::Group(group) => visit_shapes(
                &group.children,
                visible && group.header.container_visible(),
                visit,
            ),
            _ => {}
        }
    }
}

impl<E: 'static> PointwiseFilter<E> {
    /// Applies a set of shape transforms in one history command, or leaves every shape unchanged.
    pub fn edit_canvas_shapes<'a>(
        &self,
        edits: impl IntoIterator<
            Item = (
                &'a CanvasShapeEditor<E>,
                &'a CanvasShapeValues,
                &'a CanvasShapeValues,
            ),
        >,
    ) -> Result<(), PointwiseError<E>> {
        self.apply_canvas_shape_edits(edits.into_iter(), None)
            .map(|_| ())
    }

    fn apply_canvas_shape_edits<'a>(
        &self,
        edits: impl Iterator<
            Item = (
                &'a CanvasShapeEditor<E>,
                &'a CanvasShapeValues,
                &'a CanvasShapeValues,
            ),
        >,
        derived_command: Option<&str>,
    ) -> Result<bool, PointwiseError<E>> {
        let mut pending = HashMap::new();
        for (editor, expected, proposed) in edits {
            if !self.node().same_reference(&editor.layer.owner.node())
                || pending
                    .insert(editor.identity(), (expected, proposed))
                    .is_some()
            {
                return Err(PointwiseError::InvalidOperation);
            }
        }
        if pending.is_empty() {
            return Ok(false);
        }
        let command = derived_command.unwrap_or(if pending.len() == 1 {
            "Edit Canvas Shape"
        } else {
            "Edit Canvas Shapes"
        });
        let before = self.canvas_document().ok_or(PointwiseError::InvalidCast)?;
        let mut after = (*before).clone();
        let mut changed = false;
        for layer in &mut after.layers {
            if !apply_shape_edits(
                &mut layer.shapes,
                &mut pending,
                &mut changed,
                derived_command.is_some(),
            ) {
                return Err(PointwiseError::InvalidOperation);
            }
        }
        if !pending.is_empty() {
            return Err(PointwiseError::InvalidOperation);
        }
        if changed {
            self.commit_canvas_edit(before, after, command)?;
        }
        Ok(changed)
    }

    pub fn edit_remove_canvas_shapes(
        &self,
        selection: &[CanvasShapeEditor<E>],
    ) -> Result<(), PointwiseError<E>> {
        let mut pending = HashSet::new();
        for editor in selection {
            if !self.node().same_reference(&editor.layer.owner.node()) {
                return Err(PointwiseError::InvalidOperation);
            }
            pending.insert(editor.identity());
        }
        if pending.is_empty() {
            return Ok(());
        }
        let before = self.canvas_document().ok_or(PointwiseError::InvalidCast)?;
        self.remove_canvas_shapes(before, pending)
    }

    pub(super) fn remove_canvas_shapes(
        &self,
        before: Arc<CanvasDocument>,
        mut pending: HashSet<usize>,
    ) -> Result<(), PointwiseError<E>> {
        let command = if pending.len() == 1 {
            "Delete Canvas Shape"
        } else {
            "Delete Canvas Shapes"
        };
        let mut after = (*before).clone();
        for layer in &mut after.layers {
            remove_shapes(&mut layer.shapes, &mut pending);
        }
        if !pending.is_empty() {
            return Err(PointwiseError::InvalidOperation);
        }
        for layer in &mut after.layers {
            prune_small_groups(&mut layer.shapes);
        }
        self.commit_canvas_edit(before, after, command)
    }

    pub fn canvas_shape_editors(&self) -> Vec<CanvasShapeEditor<E>> {
        let mut editors = Vec::new();
        self.visit_canvas_shapes(|editor, _, _| editors.push(editor));
        editors
    }

    pub fn pick_canvas_shape(&self, point: [f32; 2]) -> Option<CanvasShapeEditor<E>> {
        self.pick_canvas_shape_with_tolerance(point, 0.)
    }

    pub fn pick_canvas_shape_with_tolerance(
        &self,
        point: [f32; 2],
        tolerance: f32,
    ) -> Option<CanvasShapeEditor<E>> {
        let point = point.map(|axis| f64::from(axis).rem_euclid(100.));
        let mut best = None;
        let mut distance = f64::INFINITY;
        let mut stroke = None;
        let mut stroke_distance = f64::INFINITY;
        self.visit_canvas_shapes(|editor, shape, visible| {
            if !visible {
                return;
            }
            let Ok(contours) = shape.outline::<E>() else {
                return;
            };
            let [x, y, ..] = shape.base.transform;
            let center = [x, y].map(|axis| f64::from(axis).rem_euclid(100.));
            let stroke_radius = shape
                .open_stroke_width()
                .map(|width| f64::from((0.5 * width).max(tolerance)));
            for x in [-100., 0., 100.] {
                for y in [-100., 0., 100.] {
                    let sample = [point[0] + x, point[1] + y];
                    if let Some(radius) = stroke_radius {
                        for segment in contours.iter().flat_map(|contour| contour.windows(2)) {
                            let candidate =
                                segment_distance_squared(sample, segment[0], segment[1]);
                            if candidate <= radius * radius && candidate < stroke_distance {
                                stroke_distance = candidate;
                                stroke = Some(editor.clone());
                            }
                        }
                        continue;
                    }
                    let dx = center[0] - sample[0];
                    let dy = center[1] - sample[1];
                    let candidate = dx * dx + dy * dy;
                    if candidate < distance && covers(&contours, sample) {
                        distance = candidate;
                        best = Some(editor.clone());
                    }
                }
            }
        });
        stroke.or(best)
    }

    /// Finds visible editable shapes with all four authored transform corners inside the box.
    pub fn canvas_shapes_in_box(&self, a: [f32; 2], b: [f32; 2]) -> Vec<CanvasShapeEditor<E>> {
        let low: [f32; 2] = std::array::from_fn(|i| a[i].min(b[i]));
        let high: [f32; 2] = std::array::from_fn(|i| a[i].max(b[i]));
        let mut selected = Vec::new();
        self.visit_canvas_shapes(|editor, shape, visible| {
            if visible
                && shape
                    .authored_corners()
                    .into_iter()
                    .all(|corner| (0..2).all(|i| low[i] <= corner[i] && corner[i] < high[i]))
            {
                selected.push(editor);
            }
        });
        selected
    }

    fn visit_canvas_shapes(&self, mut visit: impl FnMut(CanvasShapeEditor<E>, &Shape, bool)) {
        let Some(canvas) = self.canvas_document() else {
            return;
        };
        for (index, layer) in canvas.layers.iter().enumerate() {
            let layer_editor = self.canvas_layer_editor(index, layer);
            let mut ordinal = 0;
            visit_shapes(
                &layer.shapes,
                layer.header.container_visible(),
                &mut |shape, visible| {
                    ordinal += 1;
                    let editor = CanvasShapeEditor::at_layer(&layer_editor, shape, ordinal);
                    visit(editor, shape, visible);
                },
            );
        }
    }
}

impl<E: 'static> CanvasShapeEditor<E> {
    pub(super) fn at_layer(layer: &CanvasLayerEditor<E>, shape: &Shape, ordinal: usize) -> Self {
        let name = String::from_utf16_lossy(&shape.header.name);
        let name = if name.is_empty() {
            CanvasPrimitive::from_tag(shape.kind).map_or("Curve", CanvasPrimitive::name)
        } else {
            &name
        };
        Self {
            layer: layer.clone(),
            identity: shape.identity.clone(),
            label: format!("{} / {ordinal}. {name}", layer.label()),
        }
    }
}

fn segment_distance_squared(point: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let [x, y] = [point[0] - a[0], point[1] - a[1]];
    let [dx, dy] = [b[0] - a[0], b[1] - a[1]];
    let length = dx * dx + dy * dy;
    let t = if length > 0. {
        ((x * dx + y * dy) / length).clamp(0., 1.)
    } else {
        0.
    };
    (x - t * dx).powi(2) + (y - t * dy).powi(2)
}

fn covers(contours: &[Vec<[f64; 2]>], [x, y]: [f64; 2]) -> bool {
    let mut inside = false;
    for contour in contours {
        for (&[x0, y0], &[x1, y1]) in contour.iter().zip(contour.iter().cycle().skip(1)) {
            if ((y0 < y && y <= y1) || (y1 < y && y <= y0))
                && x0 + (y - y0) * (x1 - x0) / (y1 - y0) < x
            {
                inside = !inside;
            }
        }
    }
    inside
}
