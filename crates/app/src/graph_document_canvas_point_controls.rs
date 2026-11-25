//! Numeric controls and mode commands for the active Canvas points.
use super::super::transform::ValuePrompt;
use super::*;

const POINT_KINDS: [CanvasPointKind; 3] = [
    CanvasPointKind::Corner,
    CanvasPointKind::Smooth,
    CanvasPointKind::Independent,
];

pub(super) type TangentDirections = HashMap<(Vertex, CanvasPathHandle), f64>;

#[derive(Clone, Copy)]
enum Coordinate {
    X,
    Y,
}

impl Coordinate {
    fn index(self) -> usize {
        match self {
            Self::X => 0,
            Self::Y => 1,
        }
    }
}

#[derive(Clone, Copy)]
enum TangentField {
    Offset(Coordinate),
    Length,
    Angle,
}

#[derive(Clone, Copy)]
enum PointField {
    Position(Coordinate),
    Tangent(CanvasPathHandle, TangentField),
}

impl PointField {
    fn labels(self) -> (&'static str, &'static str) {
        match self {
            Self::Position(Coordinate::X) => ("Set point X…", "Point X"),
            Self::Position(Coordinate::Y) => ("Set point Y…", "Point Y"),
            Self::Tangent(_, TangentField::Offset(Coordinate::X)) => {
                ("Set X offset…", "Tangent X offset")
            }
            Self::Tangent(_, TangentField::Offset(Coordinate::Y)) => {
                ("Set Y offset…", "Tangent Y offset")
            }
            Self::Tangent(_, TangentField::Length) => ("Set length…", "Tangent length"),
            Self::Tangent(_, TangentField::Angle) => ("Set angle…", "Tangent angle"),
        }
    }

    fn bounds(self) -> [f32; 2] {
        match self {
            Self::Tangent(_, TangentField::Length) => [0., 10000.],
            Self::Tangent(_, TangentField::Angle) => [-360., 360.],
            _ => [-10000., 10000.],
        }
    }
}

impl Points {
    pub(super) fn vertex(&self, vertex: Vertex) -> Option<&CanvasPathPoint> {
        self.loaded
            .contours()
            .nth(vertex.contour)?
            .1
            .get(vertex.point)
    }

    pub(super) fn handle_connected(&self, vertex: Vertex, handle: CanvasPathHandle) -> bool {
        self.loaded
            .contours()
            .nth(vertex.contour)
            .is_some_and(|(closed, points)| {
                point_handles(closed, vertex.point, points.len()).any(|item| item == handle)
            })
    }

    pub(super) fn direction(&self, vertex: Vertex, handle: CanvasPathHandle) -> f64 {
        self.vertex(vertex).map_or(0., |point| {
            let offset = point.handle_offset(handle);
            if offset == [0.; 2] {
                self.directions
                    .get(&(vertex, handle))
                    .copied()
                    .unwrap_or(0.)
            } else {
                let [x, y] = offset.map(f64::from);
                -y.atan2(x).to_degrees()
            }
        })
    }

    pub(super) fn remember_direction(
        &mut self,
        vertex: Vertex,
        handle: CanvasPathHandle,
        offset: [f32; 2],
        angle: f64,
    ) {
        if offset == [0.; 2] {
            self.directions.insert((vertex, handle), angle);
        } else {
            self.directions.remove(&(vertex, handle));
        }
    }

    fn can_set_kind(&self, kind: CanvasPointKind) -> bool {
        self.drag.is_none()
            && self.active.is_some()
            && self.selected_vertices().all(|vertex| {
                self.loaded
                    .can_set_point_kind(vertex.contour, vertex.point, kind)
            })
    }

    fn set_kind(&self, kind: CanvasPointKind) -> CurveAction {
        if !self.can_set_kind(kind) {
            return CurveAction::None;
        }
        let mut proposed = (*self.loaded).clone();
        let mut vertices: Vec<_> = self.selected_vertices().collect();
        vertices.sort_unstable_by_key(|vertex| (vertex.contour, vertex.point));
        for vertex in vertices {
            proposed.set_point_kind(
                vertex.contour,
                vertex.point,
                kind,
                self.direction(vertex, CanvasPathHandle::Incoming),
            );
        }
        self.change(proposed)
    }

    fn field_value(&self, vertex: Vertex, field: PointField) -> Option<f32> {
        let point = self.vertex(vertex)?;
        Some(match field {
            PointField::Position(coordinate) => point.position()[coordinate.index()],
            PointField::Tangent(handle, field) if self.handle_connected(vertex, handle) => {
                let offset = point.handle_offset(handle);
                match field {
                    TangentField::Offset(coordinate) => offset[coordinate.index()],
                    TangentField::Length => offset[0].hypot(offset[1]),
                    TangentField::Angle => self.direction(vertex, handle) as f32,
                }
            }
            PointField::Tangent(_, _) => return None,
        })
    }

    fn set_field(&mut self, vertex: Vertex, field: PointField, value: f32) -> CurveAction {
        let Some(point) = self.vertex(vertex) else {
            return CurveAction::None;
        };
        let mut proposed = (*self.loaded).clone();
        let changed = match field {
            PointField::Position(coordinate) => {
                let mut position = point.position();
                position[coordinate.index()] = value;
                proposed.set_point_position(vertex.contour, vertex.point, position)
            }
            PointField::Tangent(handle, field) => {
                let mut offset = point.handle_offset(handle);
                let old_angle = self.direction(vertex, handle);
                let angle = match field {
                    TangentField::Angle => f64::from(value),
                    _ => old_angle,
                };
                match field {
                    TangentField::Offset(coordinate) => offset[coordinate.index()] = value,
                    TangentField::Length | TangentField::Angle => {
                        let length = match field {
                            TangentField::Length => f64::from(value),
                            _ => {
                                let [x, y] = offset.map(f64::from);
                                (x * x + y * y).sqrt()
                            }
                        };
                        let (sin, cos) = (-angle.to_radians()).sin_cos();
                        offset = [(length * cos) as f32, (length * sin) as f32];
                    }
                }
                let changed = proposed.set_point_tangent(
                    vertex.contour,
                    vertex.point,
                    handle,
                    offset,
                    angle,
                    false,
                );
                if changed {
                    self.remember_direction(vertex, handle, offset, angle);
                }
                changed
            }
        };
        if changed {
            self.change(proposed)
        } else {
            CurveAction::None
        }
    }
}

impl GraphDocumentView {
    pub(super) fn render_canvas_point_properties(
        &self,
        points: &Points,
        disabled: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let vertex = points.active.filter(|_| points.selected.len() == 1);
        let disabled = disabled || points.drag.is_some();
        div()
            .h_flex()
            .flex_wrap()
            .gap_2()
            .children(POINT_KINDS.into_iter().map(|kind| {
                let (id, label) = match kind {
                    CanvasPointKind::Corner => ("texture-canvas-point-corner", "Corner"),
                    CanvasPointKind::Smooth => ("texture-canvas-point-smooth", "Smooth"),
                    CanvasPointKind::Independent => ("texture-canvas-point-cusp", "Cusp"),
                };
                Button::new(id)
                    .small()
                    .ghost()
                    .label(label)
                    .selected(
                        points.active.is_some()
                            && points.selected_vertices().all(|vertex| {
                                points
                                    .vertex(vertex)
                                    .is_some_and(|point| point.kind() == kind)
                            }),
                    )
                    .disabled(disabled || !points.can_set_kind(kind))
                    .on_click(cx.listener(move |view, _, window, cx| {
                        view.edit_canvas_point_kind(kind, window, cx);
                    }))
            }))
            .when_some(vertex, |row, vertex| {
                row.child(point_fields_menu(
                    "texture-canvas-point-coordinates",
                    "Point coordinates…",
                    [
                        PointField::Position(Coordinate::X),
                        PointField::Position(Coordinate::Y),
                    ],
                    disabled,
                    cx,
                ))
                .children(
                    HANDLES
                        .into_iter()
                        .filter(|&handle| points.handle_connected(vertex, handle))
                        .map(|handle| {
                            let (id, label) = match handle {
                                CanvasPathHandle::Incoming => {
                                    ("texture-canvas-incoming-tangent", "Incoming tangent…")
                                }
                                CanvasPathHandle::Outgoing => {
                                    ("texture-canvas-outgoing-tangent", "Outgoing tangent…")
                                }
                            };
                            point_fields_menu(
                                id,
                                label,
                                [
                                    PointField::Tangent(
                                        handle,
                                        TangentField::Offset(Coordinate::X),
                                    ),
                                    PointField::Tangent(
                                        handle,
                                        TangentField::Offset(Coordinate::Y),
                                    ),
                                    PointField::Tangent(handle, TangentField::Length),
                                    PointField::Tangent(handle, TangentField::Angle),
                                ],
                                disabled,
                                cx,
                            )
                        }),
                )
            })
    }

    fn edit_canvas_point_kind(
        &mut self,
        kind: CanvasPointKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.drawing_current(cx)
            && let Some(CurveTool::Points(points)) =
                self.drawing.as_ref().and_then(|tool| tool.curve.as_ref())
        {
            let action = points.set_kind(kind);
            self.commit_curve_action(action, window, cx);
            self.drawing_focus.focus(window, cx);
        }
    }

    fn open_canvas_point_value(
        &mut self,
        field: PointField,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.drawing_current(cx) {
            return;
        }
        let Some(CurveTool::Points(points)) =
            self.drawing.as_ref().and_then(|tool| tool.curve.as_ref())
        else {
            return;
        };
        let Some(vertex) = points
            .active
            .filter(|_| points.selected.len() == 1 && points.drag.is_none())
        else {
            return;
        };
        let Some(initial) = points.field_value(vertex, field) else {
            return;
        };
        let revision = self.revision;
        let editor = points.editor.clone();
        let loaded = points.loaded.clone();
        let (_, label) = field.labels();
        let title = match field {
            PointField::Tangent(handle, _) => format!(
                "{} {}",
                match handle {
                    CanvasPathHandle::Incoming => "Incoming",
                    CanvasPathHandle::Outgoing => "Outgoing",
                },
                label.to_lowercase()
            ),
            PointField::Position(_) => label.into(),
        };
        self.open_canvas_value_dialog(
            ValuePrompt {
                title,
                label,
                bounds: field.bounds(),
                input_id: "canvas-point-value",
                error_id: "canvas-point-value-error",
            },
            initial,
            move |view, value, window, cx| {
                if view.revision == revision
                    && view.drawing_current(cx)
                    && let Some(CurveTool::Points(points)) =
                        view.drawing.as_mut().and_then(|tool| tool.curve.as_mut())
                    && points.editor.same_reference(&editor)
                    && Arc::ptr_eq(&points.loaded, &loaded)
                    && points.active == Some(vertex)
                    && points.selected.len() == 1
                    && points.drag.is_none()
                {
                    let action = points.set_field(vertex, field, value);
                    view.commit_curve_action(action, window, cx);
                    cx.notify();
                }
            },
            window,
            cx,
        );
    }
}

fn point_fields_menu<const N: usize>(
    id: &'static str,
    label: &'static str,
    fields: [PointField; N],
    disabled: bool,
    cx: &Context<GraphDocumentView>,
) -> impl IntoElement + use<N> {
    let owner = cx.weak_entity();
    Button::new(id)
        .small()
        .label(label)
        .disabled(disabled)
        .dropdown_menu(move |mut menu, _, _| {
            for field in fields {
                let current = owner.clone();
                menu = menu.item(PopupMenuItem::new(field.labels().0).on_click(
                    move |_, window, cx| {
                        let _ = current.update(cx, |view, cx| {
                            view.open_canvas_point_value(field, window, cx)
                        });
                    },
                ));
            }
            menu
        })
}
