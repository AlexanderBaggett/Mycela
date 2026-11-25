//! Layout and interaction for the Canvas selection's direct transform handles.
use super::*;
use mycela_engine::{
    CanvasHandleDrag, CanvasHandleModifiers, CanvasHandlePreview, CanvasSelectionEdit,
    CanvasTransformFrame, CanvasTransformHandle as Handle,
    CanvasTransformOrientation as Orientation, PointwiseError,
};

#[derive(Default)]
pub(super) struct Tools {
    orientation: Orientation,
    edit: Option<Rc<CanvasSelectionEdit<ForegroundError>>>,
}

impl Tools {
    pub(super) fn frame(&self) -> Option<CanvasTransformFrame> {
        self.edit.as_ref().map(|edit| edit.frame())
    }

    pub(super) fn begin_drag(
        &self,
        space: TileSpace,
        position: Point<Pixels>,
        rem: Pixels,
    ) -> Option<Drag> {
        let edit = self.edit.as_ref()?;
        let frame = edit.frame();
        let gap = rotation_gap(space, rem);
        let hit_radius = rem * 0.48;
        let (handle, tile) = Handle::ALL
            .into_iter()
            .flat_map(|handle| tile_offsets(frame).map(move |tile| (handle, tile)))
            .filter_map(|(handle, tile)| {
                let projected = translated(frame.handle_point(handle, gap), tile);
                let offset = space.window_point(projected) - position;
                if offset.x.abs() > hit_radius || offset.y.abs() > hit_radius {
                    return None;
                }
                let (x, y) = (offset.x / rem, offset.y / rem);
                Some(((handle, tile), x * x + y * y))
            })
            .min_by(|(_, a), (_, b)| a.total_cmp(b))?
            .0;
        Some(Drag {
            edit: edit.clone(),
            tile,
            drag: CanvasHandleDrag::new(
                frame,
                handle,
                to_authored(space.tile_point(position), tile),
                gap,
            ),
        })
    }
}

pub(super) struct Drag {
    edit: Rc<CanvasSelectionEdit<ForegroundError>>,
    drag: CanvasHandleDrag,
    tile: [f32; 2],
}

impl Drag {
    pub(super) fn preview(
        &self,
        gesture: &ShapeDrag,
    ) -> Result<CanvasHandlePreview<'_, ForegroundError>, PointwiseError<ForegroundError>> {
        let modifiers = gesture.modifiers;
        let mut options = CanvasHandleModifiers::new(modifiers.control, modifiers.shift);
        if modifiers.alt {
            options = options.with_snap_grid(gesture.space.1);
        }
        self.edit.preview_handle(
            self.drag,
            to_authored(gesture.space.tile_point(gesture.position), self.tile),
            options,
        )
    }
}

impl DrawingTool {
    pub(super) fn refresh_handles(&mut self) {
        let orientation = self.transform_orientation();
        if let Some(tools) = &mut self.handles {
            tools.edit = self.target.filter().and_then(|filter| {
                filter
                    .canvas_selection_edit_for(&self.selection.shapes, orientation)
                    .ok()
                    .map(Rc::new)
            });
        }
    }

    pub(super) fn transform_orientation(&self) -> Orientation {
        if self.selection.has_group() {
            return Orientation::Document;
        }
        self.handles
            .as_ref()
            .map_or(Orientation::Shape, |tools| tools.orientation)
    }
}

impl GraphDocumentView {
    pub(super) fn render_canvas_handle_controls(
        &self,
        tool: &DrawingTool,
        busy: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let shown = tool.handles.is_some();
        let document = tool.selection.has_group()
            || tool.selection.shapes.len() > 1
            || tool.transform_orientation() == Orientation::Document;
        let disabled = busy || self.has_field_drafts(cx);
        div()
            .h_flex()
            .flex_wrap()
            .gap_2()
            .child(
                Button::new("texture-canvas-handles")
                    .small()
                    .label("Transform handles")
                    .selected(shown)
                    .disabled(disabled)
                    .on_click(cx.listener(|view, _, _, cx| {
                        if view.drawing_current(cx)
                            && !view.has_field_drafts(cx)
                            && let Some(tool) = view.drawing.as_mut()
                            && tool.is_selecting()
                            && !tool.selection.is_empty()
                        {
                            tool.cancel_drag();
                            tool.handles = tool.handles.is_none().then(Tools::default);
                            tool.refresh_handles();
                            cx.notify();
                        }
                    })),
            )
            .when(shown, |row| {
                row.child(
                    Button::new("texture-canvas-document-orientation")
                        .small()
                        .ghost()
                        .label(if document {
                            "Document orientation"
                        } else {
                            "Shape orientation"
                        })
                        .disabled(disabled || tool.single_selection().is_none())
                        .on_click(cx.listener(|view, _, _, cx| {
                            if view.drawing_current(cx)
                                && !view.has_field_drafts(cx)
                                && let Some(tool) = &mut view.drawing
                                && tool.single_selection().is_some()
                            {
                                tool.cancel_drag();
                                if let Some(handles) = &mut tool.handles {
                                    handles.orientation =
                                        if handles.orientation == Orientation::Shape {
                                            Orientation::Document
                                        } else {
                                            Orientation::Shape
                                        };
                                }
                                tool.refresh_handles();
                                cx.notify();
                            }
                        })),
                )
            })
    }
}

fn rotation_gap(space: TileSpace, rem: Pixels) -> f32 {
    1.5 * rem / space.0.size.width * TILE
}

fn tile_offsets(frame: CanvasTransformFrame) -> impl Iterator<Item = [f32; 2]> {
    let [x, y] = frame
        .center()
        .map(|center| center.rem_euclid(TILE) - center);
    [-TILE, 0., TILE].into_iter().flat_map(move |dx| {
        [-TILE, 0., TILE]
            .into_iter()
            .map(move |dy| [x + dx, y + dy])
    })
}

fn translated([x, y]: [f32; 2], [dx, dy]: [f32; 2]) -> [f32; 2] {
    [x + dx, y + dy]
}

fn to_authored([x, y]: [f32; 2], [dx, dy]: [f32; 2]) -> [f32; 2] {
    [x - dx, y - dy]
}

pub(super) fn paint(
    frame: CanvasTransformFrame,
    space: TileSpace,
    stroke: Pixels,
    (accent, background): (Hsla, Hsla),
    window: &mut Window,
) {
    let rem = window.rem_size();
    let gap = rotation_gap(space, rem);
    for tile in tile_offsets(frame) {
        let at = |handle| space.window_point(translated(frame.handle_point(handle, gap), tile));
        let points = [
            Handle::TopLeft,
            Handle::TopRight,
            Handle::BottomLeft,
            Handle::BottomRight,
            Handle::Rotate,
        ]
        .map(at);
        let bounds = space.0;
        if points.iter().all(|p| p.x < bounds.left() - rem)
            || points.iter().all(|p| p.x > bounds.right() + rem)
            || points.iter().all(|p| p.y < bounds.top() - rem)
            || points.iter().all(|p| p.y > bounds.bottom() + rem)
        {
            continue;
        }
        paint_frame(stroke, (accent, background), window, at);
    }
}

fn paint_frame(
    stroke: Pixels,
    (accent, background): (Hsla, Hsla),
    window: &mut Window,
    at: impl Fn(Handle) -> Point<Pixels>,
) {
    let rem = window.rem_size();
    for (width, color) in [(stroke * 3., background), (stroke, accent)] {
        let mut path = PathBuilder::stroke(width);
        path.move_to(at(Handle::TopLeft));
        for corner in [Handle::TopRight, Handle::BottomRight, Handle::BottomLeft] {
            path.line_to(at(corner));
        }
        path.close();
        path.move_to(at(Handle::Top));
        path.line_to(at(Handle::Rotate));
        if let Ok(path) = path.build() {
            window.paint_path(path, color);
        }
    }
    for handle in Handle::ALL {
        let center = at(handle);
        let width = if handle == Handle::Rotate {
            rem * 0.62
        } else {
            rem * 0.46
        };
        for (width, color) in [(width, background), (width - stroke * 2., accent)] {
            let offset = width / 2.;
            window.paint_quad(fill(
                Bounds::new(center - point(offset, offset), size(width, width)),
                color,
            ));
        }
    }
}
