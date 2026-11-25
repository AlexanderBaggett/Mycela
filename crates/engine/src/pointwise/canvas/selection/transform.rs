//! Shared Canvas transforms around the original document bounds.
use super::*;
use crate::CanvasItem;

mod handles;
pub use handles::{
    CanvasHandleDrag, CanvasHandleModifiers, CanvasHandlePreview, CanvasTransformFrame,
    CanvasTransformHandle, CanvasTransformOrientation,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CanvasSelectionField {
    CenterX,
    CenterY,
    Width,
    Height,
    SkewX,
    SkewY,
    Angle,
}

impl CanvasSelectionField {
    pub fn label(self) -> &'static str {
        self.shape_field().label()
    }

    pub fn bounds(self) -> [f32; 2] {
        self.shape_field().bounds()
    }

    fn shape_field(self) -> CanvasShapeField {
        match self {
            Self::CenterX => CanvasShapeField::CenterX,
            Self::CenterY => CanvasShapeField::CenterY,
            Self::Width => CanvasShapeField::Width,
            Self::Height => CanvasShapeField::Height,
            Self::SkewX => CanvasShapeField::SkewX,
            Self::SkewY => CanvasShapeField::SkewY,
            Self::Angle => CanvasShapeField::Angle,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CanvasAlignment {
    Left,
    HorizontalCenter,
    Right,
    Top,
    VerticalCenter,
    Bottom,
}

impl CanvasAlignment {
    fn axis_and_extent(self) -> (usize, f32) {
        match self {
            Self::Left => (0, -0.5),
            Self::HorizontalCenter => (0, 0.),
            Self::Right => (0, 0.5),
            Self::Top => (1, -0.5),
            Self::VerticalCenter => (1, 0.),
            Self::Bottom => (1, 0.5),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum CanvasSelectionTransform {
    Set(CanvasSelectionField, f32),
    Rotate(f32),
    FlipHorizontal,
    FlipVertical,
    Align(CanvasAlignment),
}

/// A snapshot of the selected shapes for one transform. For multiple shapes, center and size
/// describe their authored outlines in document coordinates. Angle comes from the first shape.
pub struct CanvasSelectionEdit<E> {
    filter: PointwiseFilter<E>,
    shapes: Vec<(CanvasShapeEditor<E>, CanvasShapeValues)>,
    center: [f32; 2],
    size: [f32; 2],
    orientation: CanvasTransformOrientation,
}

impl<E: 'static> PointwiseFilter<E> {
    /// Aligns selected items. A single selected group aligns its immediate children.
    pub fn edit_align_canvas_items(
        &self,
        items: &[(CanvasLayerEditor<E>, CanvasItem)],
        alignment: CanvasAlignment,
    ) -> Result<bool, PointwiseError<E>> {
        let mut units = Vec::new();
        let mut seen = HashSet::new();
        let (axis, edge) = alignment.axis_and_extent();
        for (layer, item) in items {
            if !self.node().same_reference(&layer.owner.node()) {
                return Err(PointwiseError::InvalidOperation);
            }
            let children;
            let members = if items.len() == 1 && item.is_group() {
                children = layer.item_children(item);
                children.as_slice()
            } else {
                std::slice::from_ref(item)
            };
            for unit in members {
                let shapes = layer
                    .item_shape_editors(unit)
                    .into_iter()
                    .map(|shape| {
                        if !seen.insert(shape.identity()) {
                            return Err(PointwiseError::InvalidOperation);
                        }
                        let values = shape.settings()?;
                        Ok((shape, values))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                if shapes.is_empty() {
                    continue;
                }
                let mut low = f32::INFINITY;
                let mut high = f32::NEG_INFINITY;
                for (_, values) in &shapes {
                    let center = values.transform[axis];
                    let half_extent = 0.5 * values.transform[axis + 2].abs();
                    low = low.min(center - half_extent);
                    high = high.max(center + half_extent);
                }
                let anchor = if unit.is_group() {
                    if edge < 0. {
                        low
                    } else if edge > 0. {
                        high
                    } else {
                        (low + high) * 0.5
                    }
                } else {
                    let values = &shapes[0].1;
                    values.transform[axis] + edge * values.transform[axis + 2].abs()
                };
                units.push((anchor, shapes));
            }
        }
        if units.len() < 2 {
            return Ok(false);
        }
        let target = units[0].0;
        let edits: Vec<_> = units
            .into_iter()
            .flat_map(|(anchor, shapes)| {
                shapes.into_iter().map(move |(editor, expected)| {
                    let mut proposed = expected.clone();
                    proposed.transform[axis] += target - anchor;
                    (editor, expected, proposed)
                })
            })
            .collect();
        self.apply_canvas_shape_edits(
            edits
                .iter()
                .map(|(editor, expected, proposed)| (editor, expected, proposed)),
            Some("Align Canvas selection"),
        )
    }

    pub fn canvas_selection_edit(
        &self,
        selection: &[CanvasShapeEditor<E>],
    ) -> Result<CanvasSelectionEdit<E>, PointwiseError<E>> {
        self.canvas_selection_edit_for(selection, CanvasTransformOrientation::Shape)
    }

    pub fn canvas_selection_edit_for(
        &self,
        selection: &[CanvasShapeEditor<E>],
        orientation: CanvasTransformOrientation,
    ) -> Result<CanvasSelectionEdit<E>, PointwiseError<E>> {
        if selection.is_empty()
            || selection
                .iter()
                .any(|editor| !self.node().same_reference(&editor.layer.owner.node()))
        {
            return Err(PointwiseError::Argument);
        }
        let mut pending: HashSet<_> = selection.iter().map(CanvasShapeEditor::identity).collect();
        if pending.len() != selection.len() {
            return Err(PointwiseError::Argument);
        }
        let document_bounds =
            selection.len() > 1 || orientation == CanvasTransformOrientation::Document;
        let mut low = [f32::INFINITY; 2];
        let mut high = [f32::NEG_INFINITY; 2];
        let mut records = HashMap::new();
        let mut failure = None;
        self.visit_canvas_shapes(|editor, shape, _| {
            if !pending.remove(&editor.identity()) {
                return;
            }
            if document_bounds {
                match shape.document_bounds() {
                    Ok([first, last]) => {
                        for axis in 0..2 {
                            low[axis] = low[axis].min(first[axis]);
                            high[axis] = high[axis].max(last[axis]);
                        }
                    }
                    Err(error) => failure = Some(error),
                }
            }
            records.insert(editor.identity(), CanvasShapeValues::from_shape(shape));
        });
        if let Some(error) = failure {
            return Err(error);
        }
        if !pending.is_empty() {
            return Err(PointwiseError::InvalidOperation);
        }
        let shapes: Vec<_> = selection
            .iter()
            .map(|editor| (editor.clone(), records.remove(&editor.identity()).unwrap()))
            .collect();
        let size = if document_bounds {
            std::array::from_fn(|axis| high[axis] - low[axis])
        } else {
            let [_, _, width, height, ..] = shapes[0].1.transform;
            [width, height]
        };
        let center = if document_bounds {
            std::array::from_fn(|axis| low[axis] + 0.5 * size[axis])
        } else {
            let [x, y, ..] = shapes[0].1.transform;
            [x, y]
        };
        if center
            .into_iter()
            .chain(size)
            .any(|value| !value.is_finite())
        {
            return Err(PointwiseError::Argument);
        }
        Ok(CanvasSelectionEdit {
            filter: self.clone(),
            shapes,
            center,
            size,
            orientation: if document_bounds {
                CanvasTransformOrientation::Document
            } else {
                orientation
            },
        })
    }
}

impl<E: 'static> CanvasSelectionEdit<E> {
    pub fn value(&self, field: CanvasSelectionField) -> f32 {
        match field {
            CanvasSelectionField::CenterX => self.center[0],
            CanvasSelectionField::CenterY => self.center[1],
            CanvasSelectionField::Width => self.size[0],
            CanvasSelectionField::Height => self.size[1],
            CanvasSelectionField::SkewX => self.frame().skew()[0],
            CanvasSelectionField::SkewY => self.frame().skew()[1],
            CanvasSelectionField::Angle => self.shapes[0].1.transform[6],
        }
    }

    pub fn apply(&self, operation: CanvasSelectionTransform) -> Result<bool, PointwiseError<E>> {
        let (operation, label) = self.operation(operation)?;
        let proposals: Vec<_> = self
            .shapes
            .iter()
            .map(|(_, values)| {
                let mut proposed = values.clone();
                operation.apply(&mut proposed.transform, self.center);
                proposed
            })
            .collect();
        self.filter.apply_canvas_shape_edits(
            self.shapes
                .iter()
                .zip(&proposals)
                .map(|((editor, expected), proposed)| (editor, expected, proposed)),
            Some(label),
        )
    }

    fn operation(
        &self,
        operation: CanvasSelectionTransform,
    ) -> Result<(Operation, &'static str), PointwiseError<E>> {
        use CanvasSelectionField as Field;
        use CanvasSelectionTransform as Transform;
        let shape_space = self.orientation == CanvasTransformOrientation::Shape;
        let result = match operation {
            Transform::Set(field, value) => {
                let [min, max] = field.bounds();
                if !(min..=max).contains(&value) {
                    return Err(PointwiseError::Argument);
                }
                if value == self.value(field) {
                    return Ok((Operation::Unchanged, "Transform Canvas selection"));
                }
                if shape_space {
                    let index = field.shape_field().axis().unwrap();
                    let value =
                        if matches!(field, Field::Width | Field::Height) && value.abs() < 0.01 {
                            0.01
                        } else {
                            value
                        };
                    (Operation::Local(index, value), "Transform Canvas shape")
                } else {
                    match field {
                        Field::CenterX => (
                            Operation::Translate(0, value - self.center[0]),
                            "Move Canvas selection",
                        ),
                        Field::CenterY => (
                            Operation::Translate(1, value - self.center[1]),
                            "Move Canvas selection",
                        ),
                        Field::Width | Field::Height => {
                            let axis = usize::from(field == Field::Height);
                            if self.size[axis] == 0. {
                                return Err(PointwiseError::Argument);
                            }
                            (
                                Operation::Scale(axis, value / self.size[axis]),
                                "Resize Canvas selection",
                            )
                        }
                        Field::SkewX | Field::SkewY => {
                            let axis = usize::from(field == Field::SkewY);
                            let normal = self.size[1 - axis];
                            if normal == 0. {
                                return Err(PointwiseError::Argument);
                            }
                            (
                                Operation::Shear(axis, -0.5 * value / normal),
                                "Skew Canvas selection",
                            )
                        }
                        Field::Angle => (
                            Operation::Rotate(value - self.value(Field::Angle)),
                            "Rotate Canvas selection",
                        ),
                    }
                }
            }
            Transform::Rotate(delta) if delta.is_finite() => {
                (Operation::Rotate(delta), "Rotate Canvas selection")
            }
            Transform::FlipHorizontal | Transform::FlipVertical => {
                let axis = usize::from(matches!(operation, Transform::FlipVertical));
                let flip = if shape_space {
                    Operation::Local(axis + 2, -self.size[axis])
                } else {
                    Operation::Scale(axis, -1.)
                };
                (flip, "Flip Canvas selection")
            }
            Transform::Align(alignment) if self.shapes.len() > 1 => {
                let (axis, edge) = alignment.axis_and_extent();
                let primary = self.shapes[0].1.transform;
                let target = primary[axis] + edge * primary[axis + 2].abs();
                (
                    Operation::Align { axis, edge, target },
                    "Align Canvas selection",
                )
            }
            _ => return Err(PointwiseError::Argument),
        };
        Ok(result)
    }
}

enum Operation {
    Unchanged,
    Local(usize, f32),
    Translate(usize, f32),
    Scale(usize, f32),
    Shear(usize, f32),
    Rotate(f32),
    Align { axis: usize, edge: f32, target: f32 },
}

impl Operation {
    fn apply(&self, values: &mut [f32; 7], pivot: [f32; 2]) {
        match *self {
            Self::Unchanged => {}
            Self::Local(index, value) => values[index] = value,
            Self::Translate(axis, delta) => values[axis] += delta,
            Self::Align { axis, edge, target } => {
                values[axis] = target - edge * values[axis + 2].abs();
            }
            Self::Rotate(delta) => {
                let relative = [values[0] - pivot[0], values[1] - pivot[1]];
                let rotated = rotate(relative, -delta);
                for axis in 0..2 {
                    values[axis] = pivot[axis] + rotated[axis];
                }
                let angle = values[6] + delta;
                values[6] = if angle.abs() > 360. {
                    angle % 360.
                } else {
                    angle
                };
            }
            Self::Scale(axis, factor) => scale_in_document(values, pivot, axis, factor),
            Self::Shear(axis, factor) => {
                let mut columns = [[1., 0.], [0., 1.]];
                columns[1 - axis][axis] = factor;
                compose_in_document(values, pivot, pivot, columns);
            }
        }
    }
}

fn rotate([x, y]: [f32; 2], angle: f32) -> [f32; 2] {
    let (sin, cos) = f64::from(angle).to_radians().sin_cos();
    let (sin, cos) = (sin as f32, cos as f32);
    [x * cos - y * sin, x * sin + y * cos]
}

fn scale_in_document(values: &mut [f32; 7], pivot: [f32; 2], axis: usize, factor: f32) {
    let mut columns = [[1., 0.], [0., 1.]];
    columns[axis][axis] = factor;
    compose_in_document(values, pivot, pivot, columns);
}

fn compose_in_document(
    values: &mut [f32; 7],
    pivot: [f32; 2],
    target: [f32; 2],
    [u, v]: [[f32; 2]; 2],
) {
    let [_, _, width, height, skew_x, skew_y, angle] = *values;
    let project = |[x, y]: [f32; 2]| std::array::from_fn(|axis| u[axis] * x + v[axis] * y);
    let transform_column = |column| rotate(project(rotate(column, -angle)), angle);
    let [w, y] = transform_column([width, -0.5 * skew_y]);
    let [x, h] = transform_column([-0.5 * skew_x, height]);
    let center = project([values[0] - pivot[0], values[1] - pivot[1]]);
    for axis in 0..2 {
        values[axis] = target[axis] + center[axis];
    }
    [values[2], values[3], values[4], values[5]] = [w, h, -2. * x, -2. * y];
    // iq writes the composed matrix back in the saved angle's basis.
    if w * h - x * y == 0. {
        if w == 0. {
            values[2] = 0.01;
        } else if h == 0. {
            values[3] = 0.01;
        } else {
            values[4] += 0.01;
        }
    }
}
