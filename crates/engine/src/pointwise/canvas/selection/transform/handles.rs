//! Canvas transform handle geometry and reversible drag proposals.
use super::*;
use crate::CanvasSnapGrid;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CanvasTransformOrientation {
    #[default]
    Shape,
    Document,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CanvasTransformHandle {
    TopLeft,
    Top,
    TopRight,
    Left,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
    Rotate,
}

impl CanvasTransformHandle {
    pub const ALL: [Self; 9] = [
        Self::TopLeft,
        Self::Top,
        Self::TopRight,
        Self::Left,
        Self::Right,
        Self::BottomLeft,
        Self::Bottom,
        Self::BottomRight,
        Self::Rotate,
    ];

    fn location(self) -> [f32; 2] {
        match self {
            Self::TopLeft => [-0.5, -0.5],
            Self::Top | Self::Rotate => [0., -0.5],
            Self::TopRight => [0.5, -0.5],
            Self::Left => [-0.5, 0.],
            Self::Right => [0.5, 0.],
            Self::BottomLeft => [-0.5, 0.5],
            Self::Bottom => [0., 0.5],
            Self::BottomRight => [0.5, 0.5],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CanvasTransformFrame([f32; 7]);

impl CanvasTransformFrame {
    pub fn center(self) -> [f32; 2] {
        [self.0[0], self.0[1]]
    }

    fn size(self) -> [f32; 2] {
        [self.0[2], self.0[3]]
    }

    pub(super) fn skew(self) -> [f32; 2] {
        [self.0[4], self.0[5]]
    }

    fn columns(self) -> [[f32; 2]; 2] {
        let [_, _, width, height, skew_x, skew_y, angle] = self.0;
        [
            rotate([width, -0.5 * skew_y], -angle),
            rotate([-0.5 * skew_x, height], -angle),
        ]
    }

    fn vector(self, [x, y]: [f32; 2]) -> [f32; 2] {
        let [u, v] = self.columns();
        std::array::from_fn(|axis| u[axis] * x + v[axis] * y)
    }

    fn inverse_point(self, point: [f32; 2]) -> Option<[f32; 2]> {
        let [u, v] = self.columns();
        let determinant = u[0] * v[1] - u[1] * v[0];
        if determinant == 0. || !determinant.is_finite() {
            return None;
        }
        let [x, y] = subtract(point, self.center());
        Some([
            (x * v[1] - y * v[0]) / determinant,
            (y * u[0] - x * u[1]) / determinant,
        ])
    }

    pub fn handle_point(self, handle: CanvasTransformHandle, rotation_gap: f32) -> [f32; 2] {
        let edge = add(self.center(), self.vector(handle.location()));
        if handle == CanvasTransformHandle::Rotate {
            let vertical = if self.0[3] < 0. {
                rotation_gap
            } else {
                -rotation_gap
            };
            add(edge, rotate([0., vertical], -self.0[6]))
        } else {
            edge
        }
    }
}

/// Ctrl changes the anchor to the center or enables uniform resize during rotation.
/// Shift locks corner proportions, skews from an edge, or snaps rotation to 15°.
#[derive(Clone, Copy, Debug, Default)]
pub struct CanvasHandleModifiers {
    centered: bool,
    constrained: bool,
    grid: Option<CanvasSnapGrid>,
}

impl CanvasHandleModifiers {
    pub fn new(centered: bool, constrained: bool) -> Self {
        Self {
            centered,
            constrained,
            grid: None,
        }
    }

    pub fn with_grid(mut self, spacing: f32) -> Self {
        self.grid = CanvasSnapGrid::regular_spacing(spacing);
        self
    }

    pub fn with_snap_grid(mut self, grid: CanvasSnapGrid) -> Self {
        self.grid = Some(grid);
        self
    }
}

#[derive(Clone, Copy, Debug)]
pub struct CanvasHandleDrag {
    frame: CanvasTransformFrame,
    handle: CanvasTransformHandle,
    press: [f32; 2],
    gap: f32,
}

impl CanvasHandleDrag {
    pub fn new(
        frame: CanvasTransformFrame,
        handle: CanvasTransformHandle,
        press: [f32; 2],
        rotation_gap: f32,
    ) -> Self {
        Self {
            frame,
            handle,
            press,
            gap: rotation_gap,
        }
    }
}

pub struct CanvasHandlePreview<'a, E> {
    edit: &'a CanvasSelectionEdit<E>,
    frame: CanvasTransformFrame,
    proposals: Vec<CanvasShapeValues>,
    label: &'static str,
}

impl<'a, E: 'static> CanvasHandlePreview<'a, E> {
    pub fn frame(&self) -> CanvasTransformFrame {
        self.frame
    }

    pub fn outlines(&self) -> Result<Vec<Vec<[f64; 2]>>, PointwiseError<E>> {
        let mut outlines = Vec::new();
        for ((shape, _), values) in self.edit.shapes.iter().zip(&self.proposals) {
            outlines.extend(shape.outline_with_transform(Some(values), true, true)?);
        }
        Ok(outlines)
    }

    pub fn commit(&self) -> Result<bool, PointwiseError<E>> {
        self.edit.filter.apply_canvas_shape_edits(
            self.edit
                .shapes
                .iter()
                .zip(&self.proposals)
                .map(|((editor, expected), proposed)| (editor, expected, proposed)),
            Some(self.label),
        )
    }

    /// Inserts independently editable copies into the destination layer.
    pub fn commit_copies(
        &self,
        destination: &CanvasLayerEditor<E>,
    ) -> Result<Vec<CanvasShapeEditor<E>>, PointwiseError<E>> {
        let filter = &self.edit.filter;
        if !filter.node().same_reference(&destination.owner.node()) {
            return Err(PointwiseError::InvalidOperation);
        }
        let (before, layer) = destination.current_layer()?;
        let mut items = Vec::with_capacity(self.proposals.len());
        let mut editors = Vec::with_capacity(self.proposals.len());
        for ((editor, expected), proposed) in self.edit.shapes.iter().zip(&self.proposals) {
            let source = editor
                .shape_in(&before)
                .ok_or(PointwiseError::InvalidOperation)?;
            if !expected.same_values(&CanvasShapeValues::from_shape(source)) {
                return Err(PointwiseError::InvalidOperation);
            }
            let mut copy = source.clone();
            if !expected.same_values(proposed)
                && !proposed.apply_transform(&mut copy, expected, true)
            {
                return Err(PointwiseError::Argument);
            }
            copy.identity = Arc::new(());
            let ordinal = self.proposals.len() - items.len();
            editors.push(CanvasShapeEditor::at_layer(destination, &copy, ordinal));
            items.push(CanvasShape::Shape(Box::new(copy)));
        }
        let mut after = (*before).clone();
        // The original inserts each item at slot zero in selection order.
        after.layers[layer]
            .shapes
            .splice(..0, items.into_iter().rev());
        filter.commit_canvas_edit(before, after, "Duplicate Canvas shapes")?;
        Ok(editors)
    }
}

impl<E: 'static> CanvasSelectionEdit<E> {
    pub fn frame(&self) -> CanvasTransformFrame {
        if self.orientation == CanvasTransformOrientation::Shape {
            CanvasTransformFrame(self.shapes[0].1.transform)
        } else {
            let [x, y] = self.center;
            let [w, h] = self.size;
            CanvasTransformFrame([x, y, w, h, 0., 0., 0.])
        }
    }

    /// Translates a selection from its press snapshot, optionally snapping the drag and pivot.
    pub fn preview_move(
        &self,
        start: [f32; 2],
        pointer: [f32; 2],
        constrain: bool,
        grid: Option<f32>,
    ) -> Result<CanvasHandlePreview<'_, E>, PointwiseError<E>> {
        self.preview_move_on_grid(
            start,
            pointer,
            constrain,
            grid.and_then(CanvasSnapGrid::regular_spacing),
        )
    }

    pub fn preview_move_on_grid(
        &self,
        start: [f32; 2],
        pointer: [f32; 2],
        constrain: bool,
        grid: Option<CanvasSnapGrid>,
    ) -> Result<CanvasHandlePreview<'_, E>, PointwiseError<E>> {
        if !start.into_iter().chain(pointer).all(f32::is_finite) {
            return Err(PointwiseError::Argument);
        }
        let aim = if constrain {
            let [x, y] = subtract(pointer, start);
            let angle = f64::from(y)
                .atan2(f64::from(x))
                .to_degrees()
                .rem_euclid(360.);
            let axis = [0_f64, 90., 180., 270., 360.]
                .into_iter()
                .min_by(|a, b| (a - angle).abs().total_cmp(&(b - angle).abs()))
                .unwrap();
            let length = f64::from(x * x + y * y).sqrt();
            let (sin, cos) = axis.to_radians().sin_cos();
            add(start, [(length * cos) as f32, (length * sin) as f32])
        } else {
            pointer
        };
        let mut target = subtract(aim, subtract(start, self.center));
        if start == pointer {
            target = self.center;
        } else if let Some(grid) = grid {
            target = grid.snap(target);
        }
        if !target.into_iter().all(f32::is_finite) {
            return Err(PointwiseError::Argument);
        }
        let offset = subtract(target, self.center);
        let mut frame = self.frame();
        frame.0[..2].copy_from_slice(&target);
        let mut proposals = Vec::with_capacity(self.shapes.len());
        for (_, expected) in &self.shapes {
            let mut moved = expected.clone();
            for (center, offset) in moved.transform[..2].iter_mut().zip(offset) {
                *center += offset;
                if !center.is_finite() {
                    return Err(PointwiseError::Argument);
                }
            }
            proposals.push(moved);
        }
        Ok(CanvasHandlePreview {
            edit: self,
            frame,
            proposals,
            label: "Move Canvas selection",
        })
    }

    pub fn preview_handle(
        &self,
        drag: CanvasHandleDrag,
        pointer: [f32; 2],
        modifiers: CanvasHandleModifiers,
    ) -> Result<CanvasHandlePreview<'_, E>, PointwiseError<E>> {
        if self.frame() != drag.frame || pointer.into_iter().any(|value| !value.is_finite()) {
            return Err(PointwiseError::Argument);
        }
        let shape_space = self.orientation == CanvasTransformOrientation::Shape;
        let handle_point = drag.frame.handle_point(drag.handle, drag.gap);
        let mut point = add(handle_point, subtract(pointer, drag.press));
        if let Some(grid) = modifiers.grid {
            point = grid.snap(point);
        }
        let (frame, action) = if pointer == drag.press {
            (drag.frame, Change::Unchanged)
        } else if drag.handle == CanvasTransformHandle::Rotate {
            drag.rotate(point, modifiers, shape_space)
                .ok_or(PointwiseError::Argument)?
        } else {
            drag.resize(point, modifiers, shape_space)
                .ok_or(PointwiseError::Argument)?
        };
        let proposals: Vec<_> = self
            .shapes
            .iter()
            .map(|(_, values)| {
                let mut proposed = values.clone();
                if !matches!(action, Change::Unchanged) {
                    if shape_space {
                        proposed.transform = frame.0;
                    } else {
                        action.apply_to_document(&mut proposed.transform, drag.frame, frame);
                    }
                }
                proposed
            })
            .collect();
        if frame
            .0
            .into_iter()
            .chain(proposals.iter().flat_map(|values| values.transform))
            .any(|value| !value.is_finite())
        {
            return Err(PointwiseError::Argument);
        }
        Ok(CanvasHandlePreview {
            edit: self,
            frame,
            proposals,
            label: action.label(),
        })
    }
}

impl CanvasHandleDrag {
    fn resize(
        self,
        point: [f32; 2],
        modifiers: CanvasHandleModifiers,
        shape_space: bool,
    ) -> Option<(CanvasTransformFrame, Change)> {
        let frame = self.frame;
        let location = self.handle.location();
        let local = frame.inverse_point(point)?;
        let size = frame.size();
        let mut next = frame;
        let is_edge = location.contains(&0.);
        if is_edge && modifiers.constrained {
            let normal = usize::from(location[0] == 0.);
            let parallel = 1 - normal;
            let direction = location[normal].signum();
            if shape_space {
                next.0[4 + parallel] -= 100. * direction * local[parallel];
                nudge_shape_frame(&mut next.0, 4 + parallel);
            } else {
                let shear =
                    2. * direction * (point[parallel] - frame.center()[parallel]) / size[normal];
                next.0[4 + parallel] = -2. * size[normal] * shear;
            }
            return Some((next, Change::Skew));
        }

        let mut dimensions = size;
        for axis in 0..2 {
            if location[axis] != 0. {
                let inward = local[axis] * location[axis].signum();
                dimensions[axis] = usable_extent(
                    size[axis]
                        * if modifiers.centered {
                            2. * inward
                        } else {
                            0.5 + inward
                        },
                );
            }
        }
        if modifiers.constrained && !is_edge {
            dimensions[1] = usable_extent(
                (dimensions[0] * size[1] / size[0])
                    .abs()
                    .copysign(dimensions[1]),
            );
        }
        next.0[2..4].copy_from_slice(&dimensions);
        if shape_space {
            nudge_shape_frame(&mut next.0, 4);
        }
        if !modifiers.centered {
            let anchor = add(frame.center(), frame.vector(location.map(|value| -value)));
            let center = add(anchor, next.vector(location));
            next.0[..2].copy_from_slice(&center);
        }
        Some((next, Change::Resize))
    }

    fn rotate(
        self,
        point: [f32; 2],
        modifiers: CanvasHandleModifiers,
        shape_space: bool,
    ) -> Option<(CanvasTransformFrame, Change)> {
        let center = self.frame.center();
        let [x, y] = subtract(point, center);
        if x == 0. && y == 0. {
            return None;
        }
        let angle = |x, y| f64::from(y).atan2(f64::from(x)).to_degrees();
        let location = self
            .frame
            .handle_point(CanvasTransformHandle::Rotate, self.gap);
        let [start_x, start_y] = subtract(location, center);
        let mut delta = if shape_space {
            (angle(start_x, start_y) - angle(x, y)) as f32
        } else {
            (630. - angle(x, y).rem_euclid(360.)) as f32
        };
        if modifiers.constrained {
            delta = (delta / 15.).round() * 15.;
        }
        let mut next = self.frame;
        if modifiers.centered {
            let [width, height] = self.frame.size();
            let distance = (x * x + y * y).sqrt();
            let new_height = usable_extent(height.signum() * 2. * (distance - self.gap));
            next.0[2] = usable_extent(new_height * width / height);
            next.0[3] = new_height;
            if shape_space {
                nudge_shape_frame(&mut next.0, 4);
            }
        }
        let angle = next.0[6] + delta;
        next.0[6] = if angle.abs() > 360. {
            angle % 360.
        } else {
            angle
        };
        Some((next, Change::Rotate(delta)))
    }
}

#[derive(Clone, Copy)]
enum Change {
    Unchanged,
    Resize,
    Skew,
    Rotate(f32),
}

impl Change {
    fn apply_to_document(
        self,
        values: &mut [f32; 7],
        before: CanvasTransformFrame,
        after: CanvasTransformFrame,
    ) {
        let [width, height] = before.size();
        let mut basis = after;
        basis.0[6] = 0.;
        if basis != before {
            let [u, v] = basis.columns();
            compose_in_document(
                values,
                before.center(),
                basis.center(),
                [u.map(|x| x / width), v.map(|y| y / height)],
            );
        }
        if let Self::Rotate(delta) = self {
            Operation::Rotate(delta).apply(values, before.center());
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Unchanged | Self::Resize => "Resize Canvas selection",
            Self::Skew => "Skew Canvas selection",
            Self::Rotate(_) => "Rotate Canvas selection",
        }
    }
}

fn add([x, y]: [f32; 2], [u, v]: [f32; 2]) -> [f32; 2] {
    [x + u, y + v]
}

fn subtract([x, y]: [f32; 2], [u, v]: [f32; 2]) -> [f32; 2] {
    [x - u, y - v]
}

fn usable_extent(value: f32) -> f32 {
    if value.abs() < 0.01 {
        0.01f32.copysign(value)
    } else {
        value
    }
}

fn nudge_shape_frame(values: &mut [f32; 7], skew: usize) {
    if values[2] * values[3] - 0.25 * values[4] * values[5] == 0. {
        values[skew] += 0.01;
    }
}
