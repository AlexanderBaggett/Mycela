//! Interaction on the selected Canvas node's untiled image.
use super::super::inspector::CanvasShape;
use super::{CancelCanvasDrawing, DeleteCanvasShape, FinishCanvasPath, GraphDocumentView, Node};
use crate::ForegroundError;
use gpui_kit::base::{Disableable, Selectable, StyledExt, TestSupportExt};
use gpui_kit::component::{
    ActiveTheme, Sizable,
    button::{Button, ButtonVariants},
    form::{Field, Form},
    input::Input,
    menu::{DropdownMenu, PopupMenuItem},
    slider::{Slider, SliderState},
};
use gpui_kit::{
    AnyElement, Bounds, ContentMask, Context, DispatchPhase, Entity, Hsla, Modifiers, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, PathBuilder, Pixels, Point, Subscription, Window,
    canvas, div, fill, point, prelude::*, px, rems, size,
};
use mycela_engine::{
    CanvasHandlePreview, CanvasItem, CanvasLayerEditor, CanvasPath, CanvasPrimitive,
    CanvasSelectionEdit, CanvasShapeBounds, CanvasSnapGrid, PointwiseKind,
};
use std::{
    cell::Cell,
    collections::{HashMap, HashSet},
    rc::Rc,
    time::{Duration, Instant},
};

const TILE: f32 = 100.;

#[path = "graph_document_canvas_design_plan.rs"]
mod design_plan;
#[path = "graph_document_canvas_grid.rs"]
mod grid;
#[path = "graph_document_canvas_handles.rs"]
mod handles;
#[path = "graph_document_canvas_hierarchy.rs"]
mod hierarchy;
#[path = "graph_document_canvas_path.rs"]
mod path;
#[path = "graph_document_canvas_selection.rs"]
mod selection;
#[path = "graph_document_canvas_transform.rs"]
mod transform;

pub(super) struct DrawingTool {
    target: Node,
    primitive: Option<CanvasPrimitive>,
    destination: Destination,
    drag: Option<ShapeDrag>,
    selection: selection::Selection,
    organizer: hierarchy::Organizer,
    handles: Option<handles::Tools>,
    curve: Option<path::CurveTool>,
    design_plan: design_plan::DesignPlanView,
    optimize_strength: Entity<SliderState>,
    _optimize_observer: Subscription,
}

impl DrawingTool {
    pub(super) fn is_selecting(&self) -> bool {
        self.primitive.is_none()
            && self
                .curve
                .as_ref()
                .and_then(path::CurveTool::drawing_mode)
                .is_none()
    }

    pub(super) fn is_target(&self, node: &Node) -> bool {
        self.target.same_reference(node)
    }

    pub(super) fn cancel_drag(&mut self) -> bool {
        self.drag.take().is_some()
    }

    pub(super) fn refresh(&mut self) {
        self.cancel_drag();
        let items = self.organizer.refresh(&self.target, &self.selection.items);
        if let Some(path::CurveTool::Points(points)) = &mut self.curve
            && !points.refresh()
        {
            self.curve = None;
        }
        match &mut self.curve {
            Some(path::CurveTool::Pen(pen)) => pen.refresh(),
            Some(path::CurveTool::Freehand(freehand)) => freehand.refresh(),
            _ => {}
        }
        let mut shapes = HashMap::new();
        if !self.selection.is_empty() || matches!(self.destination, Destination::Before(_)) {
            shapes.extend(
                self.target
                    .filter()
                    .into_iter()
                    .flat_map(|filter| filter.canvas_shape_editors())
                    .map(|editor| (editor.identity(), editor)),
            );
        }
        self.selection = selection::Selection::from_items(items, &shapes);
        match &mut self.destination {
            Destination::Layer(selected) => {
                if let Some(layer) = canvas_layers(&self.target)
                    .into_iter()
                    .find(|layer| layer.same_reference(selected))
                {
                    *selected = layer;
                } else {
                    self.destination = Destination::NewLayer(None);
                }
            }
            Destination::Group(selected, item, _) => {
                self.destination = canvas_layers(&self.target)
                    .into_iter()
                    .find(|layer| layer.same_reference(selected))
                    .map_or(Destination::NewLayer(None), |layer| {
                        let destination = group_destinations(&layer).find(|destination| {
                            matches!(destination, Destination::Group(_, group, _) if group.identity() == item.identity())
                        });
                        destination.unwrap_or(Destination::Layer(layer))
                    });
            }
            Destination::Before(shape) => {
                self.destination = shapes.remove(&shape.identity()).map_or_else(
                    || {
                        let selected = shape.layer();
                        canvas_layers(&self.target)
                            .into_iter()
                            .find(|layer| layer.same_reference(&selected))
                            .map_or(Destination::NewLayer(None), Destination::Layer)
                    },
                    Destination::Before,
                );
            }
            Destination::NewLayer(input) => {
                if input.is_some_and(|index| {
                    !self
                        .target
                        .reference_inputs()
                        .is_ok_and(|ports| index < ports.len())
                }) {
                    *input = None;
                }
            }
        }
        self.refresh_handles();
    }

    pub(super) fn single_selection(&self) -> Option<CanvasShape> {
        (self.selection.items.len() == 1)
            .then(|| self.selection.active_shape())
            .flatten()
    }
}

type Layer = CanvasLayerEditor<ForegroundError>;

fn canvas_layers(node: &Node) -> Vec<Layer> {
    node.filter()
        .map_or_else(Vec::new, |filter| filter.canvas_layer_editors())
}

#[derive(Clone)]
enum Destination {
    Layer(Layer),
    Group(Layer, CanvasItem, String),
    Before(CanvasShape),
    NewLayer(Option<usize>),
}

enum Placement {
    Primitive(CanvasPrimitive, CanvasShapeBounds),
    Curve(CanvasPath),
}

#[derive(Clone, Copy)]
enum CurveCommand {
    Merge,
    Separate,
    Optimize,
}

impl CurveCommand {
    const IMMEDIATE: [Self; 2] = [Self::Merge, Self::Separate];

    fn button(self) -> (&'static str, &'static str) {
        match self {
            Self::Merge => ("texture-canvas-merge-curves", "Merge curves"),
            Self::Separate => ("texture-canvas-separate-curve", "Separate curve"),
            Self::Optimize => ("texture-canvas-optimize-curves", "Optimize curves"),
        }
    }

    fn available(self, selection: &[CanvasShape]) -> bool {
        let Some(active) = selection.last() else {
            return false;
        };
        match self {
            Self::Merge => {
                active.is_curve()
                    && selection
                        .iter()
                        .filter(|shape| shape.is_curve())
                        .take(2)
                        .count()
                        == 2
            }
            Self::Separate => active.can_separate_curve(),
            Self::Optimize => selection.iter().any(CanvasShape::can_optimize_curve),
        }
    }
}

impl Destination {
    fn label(&self) -> String {
        match self {
            Self::Layer(layer) => layer.label().to_owned(),
            Self::Group(_, _, label) => label.clone(),
            Self::Before(shape) => format!("Before {}", shape.label()),
            Self::NewLayer(None) => "New solid layer".into(),
            Self::NewLayer(Some(input)) => format!("New layer from Input {}", input + 1),
        }
    }

    fn same_choice(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Layer(a), Self::Layer(b)) => a.same_reference(b),
            (Self::Group(_, a, _), Self::Group(_, b, _)) => a.identity() == b.identity(),
            (Self::Before(a), Self::Before(b)) => a.same_reference(b),
            (Self::NewLayer(a), Self::NewLayer(b)) => a == b,
            _ => false,
        }
    }
}

fn group_destinations(layer: &Layer) -> impl Iterator<Item = Destination> + '_ {
    let mut path = Vec::new();
    layer.items().into_iter().filter_map(move |item| {
        path.truncate(item.depth());
        if !item.is_group() {
            return None;
        }
        path.push(format!("{}. {}", item.position() + 1, item.label()));
        let label = format!("{} / {}", layer.label(), path.join(" / "));
        Some(Destination::Group(layer.clone(), item, label))
    })
}

#[derive(Clone, Copy)]
struct TileSpace(Bounds<Pixels>, CanvasSnapGrid);

impl TileSpace {
    fn fit(bounds: Bounds<Pixels>, grid: CanvasSnapGrid) -> Option<Self> {
        let side = bounds.size.width.min(bounds.size.height);
        (side > Pixels::ZERO).then(|| {
            Self(
                Bounds::new(
                    bounds.origin
                        + point(
                            (bounds.size.width - side) / 2.,
                            (bounds.size.height - side) / 2.,
                        ),
                    size(side, side),
                ),
                grid,
            )
        })
    }

    fn tile_point(self, position: Point<Pixels>) -> [f32; 2] {
        let local = position - self.0.origin;
        let side = self.0.size.width;
        [local.x / side * TILE, local.y / side * TILE]
    }

    fn window_point(self, [x, y]: [f32; 2]) -> Point<Pixels> {
        self.0.origin
            + point(
                self.0.size.width * (x / TILE),
                self.0.size.height * (y / TILE),
            )
    }
}

struct ShapeDrag {
    space: TileSpace,
    start: Point<Pixels>,
    position: Point<Pixels>,
    modifiers: Modifiers,
    started: Instant,
    mode: DragMode,
}

enum DragMode {
    Draw,
    Move {
        click: selection::Selection,
        edit: CanvasSelectionEdit<ForegroundError>,
        destination: Layer,
        moved_selection: Option<selection::Selection>,
    },
    Marquee,
    Handle(handles::Drag),
}

impl ShapeDrag {
    fn bounds(&self) -> Option<CanvasShapeBounds> {
        let snap = |point: Point<Pixels>| {
            let raw = self.space.tile_point(point);
            if self.modifiers.alt {
                self.space.1.snap(raw)
            } else {
                raw
            }
        };
        let start = snap(self.start);
        let end = snap(self.position);
        let mut bounds = std::array::from_fn(|i| end[i] - start[i]);
        if self.modifiers.shift {
            let extent = bounds[0].abs().max(bounds[1].abs());
            bounds = bounds.map(|axis| {
                if axis == 0. {
                    0.
                } else {
                    extent.copysign(axis)
                }
            });
        }
        if self.modifiers.control {
            CanvasShapeBounds::new(start, bounds.map(|axis| axis * 2.))
        } else {
            CanvasShapeBounds::new(std::array::from_fn(|i| start[i] + bounds[i] * 0.5), bounds)
        }
    }

    fn move_preview(&self) -> Option<CanvasHandlePreview<'_, ForegroundError>> {
        let DragMode::Move { edit, .. } = &self.mode else {
            return None;
        };
        if self.start == self.position || self.is_click() {
            return None;
        }
        edit.preview_move_on_grid(
            self.space.tile_point(self.start),
            self.space.tile_point(self.position),
            self.modifiers.shift,
            self.modifiers.alt.then_some(self.space.1),
        )
        .ok()
    }

    fn is_click(&self) -> bool {
        let delta = self.position - self.start;
        delta.x.abs() < px(6.)
            && delta.y.abs() < px(6.)
            && self.started.elapsed() < Duration::from_millis(200)
    }

    fn box_bounds(&self) -> Bounds<Pixels> {
        drag_rectangle(self.start, self.position)
    }
}

fn drag_rectangle(a: Point<Pixels>, b: Point<Pixels>) -> Bounds<Pixels> {
    Bounds::new(
        point(a.x.min(b.x), a.y.min(b.y)),
        size((a.x - b.x).abs(), (a.y - b.y).abs()),
    )
}

impl GraphDocumentView {
    fn drawing_current(&self, cx: &Context<Self>) -> bool {
        !self.is_busy()
            && !self.inspector.is_changed(cx)
            && self.drawing.as_ref().is_some_and(|drawing| {
                self.is_original_selection(
                    &self.selection,
                    &Some(drawing.target.clone()),
                    self.revision,
                )
            })
    }

    fn choose_canvas_tool(
        &mut self,
        target: &Node,
        primitive: Option<CanvasPrimitive>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        grid::GridPreferences::initialize(cx);
        let first = self.drawing.is_none();
        let selection = self
            .drawing
            .as_ref()
            .filter(|drawing| primitive.is_none() && drawing.is_selecting())
            .map_or_else(selection::Selection::default, |drawing| {
                drawing.selection.clone()
            });
        let handles = self
            .drawing
            .as_mut()
            .filter(|drawing| primitive.is_none() && drawing.is_selecting())
            .and_then(|drawing| drawing.handles.take());
        let destination = self
            .drawing
            .as_ref()
            .map(|drawing| {
                if drawing.is_selecting()
                    && primitive.is_some()
                    && let Some(active) = drawing.selection.active_shape()
                {
                    Destination::Before(active)
                } else {
                    drawing.destination.clone()
                }
            })
            .or_else(|| {
                self.inspector
                    .canvas_area
                    .as_ref()
                    .map(|area| area.editor.layer())
                    .or_else(|| self.style.as_ref()?.canvas_layer().cloned())
                    .or_else(|| canvas_layers(target).into_iter().next())
                    .map(Destination::Layer)
            })
            .unwrap_or(Destination::NewLayer(None));
        let optimize_strength = self.drawing.as_ref().map_or_else(
            || {
                cx.new(|_| {
                    SliderState::new()
                        .min(0.)
                        .max(100.)
                        .step(1.)
                        .default_value(20.)
                })
            },
            |drawing| drawing.optimize_strength.clone(),
        );
        let optimize_observer = cx.observe(&optimize_strength, |_, _, cx| cx.notify());
        let (design_plan, organizer) = self
            .drawing
            .as_mut()
            .filter(|drawing| drawing.is_target(target))
            .map(|drawing| {
                (
                    std::mem::take(&mut drawing.design_plan),
                    std::mem::take(&mut drawing.organizer),
                )
            })
            .unwrap_or_default();
        self.drawing = Some(DrawingTool {
            target: target.clone(),
            primitive,
            destination,
            drag: None,
            selection,
            organizer,
            handles,
            curve: None,
            design_plan,
            optimize_strength,
            _optimize_observer: optimize_observer,
        });
        if primitive.is_some() {
            self.choose_canvas_shape(None, window, cx);
        }
        self.refresh_canvas_design_plan(cx);
        self.drawing_focus.focus(window, cx);
        if first {
            self.preview = None;
            self.start_preview(cx);
        }
        cx.notify();
    }

    fn choose_canvas_shape(
        &mut self,
        editor: Option<CanvasShape>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.choose_canvas_shapes(editor.into_iter().collect(), window, cx);
    }

    fn choose_canvas_shapes(
        &mut self,
        editors: Vec<CanvasShape>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.choose_canvas_selection(selection::Selection::from_shapes(editors), window, cx);
    }

    fn choose_canvas_selection(
        &mut self,
        selection: selection::Selection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let layer = selection.active_layer();
        let editor = (selection.items.len() == 1)
            .then(|| selection.active_shape())
            .flatten();
        if let Some(tool) = &mut self.drawing {
            tool.cancel_drag();
            if matches!(tool.curve, Some(path::CurveTool::Points(_))) {
                tool.curve = None;
            }
            tool.selection = selection;
            tool.refresh_handles();
        }
        self.error = self
            .inspector
            .select_canvas_shape(editor, window, cx)
            .and_then(|()| {
                if let Some(layer) = layer
                    && self
                        .inspector
                        .canvas_area
                        .as_ref()
                        .is_none_or(|area| !area.editor.layer().same_reference(&layer))
                    && let Some(area) = self
                        .inspector
                        .canvas_areas
                        .iter()
                        .find(|area| area.layer().same_reference(&layer))
                        .cloned()
                {
                    self.inspector.select_canvas_area(area, window, cx)?;
                }
                Ok(())
            })
            .err();
        self.observe_drafts(cx);
        cx.notify();
    }

    fn leave_drawing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.inspector.is_changed(cx) {
            self.error = Some("Correct or reset field text before leaving Canvas.".into());
            cx.notify();
            return;
        }
        if self.drawing.take().is_some() {
            self.choose_canvas_shape(None, window, cx);
            self.preview = None;
            self.focus.focus(window, cx);
            self.start_preview(cx);
        }
    }

    fn delete_canvas_shape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.drawing_current(cx)
            && let Some(tool) = self
                .drawing
                .as_ref()
                .filter(|tool| tool.is_selecting() && !tool.selection.is_empty())
        {
            let selected = tool.selection.items.clone();
            self.apply_canvas_shape_edit(
                false,
                |filter| filter.edit_remove_canvas_items(&selected),
                window,
                cx,
            );
        }
    }

    fn start_shape_drag(
        &mut self,
        space: TileSpace,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.preview_pending || !space.0.contains(&event.position) || !self.drawing_current(cx) {
            return;
        }
        if let Some(curve) = self.drawing.as_mut().and_then(|tool| tool.curve.as_mut()) {
            self.drawing_focus.focus(window, cx);
            curve.press(space, event, cx);
            cx.notify();
            return;
        }
        let Some(tool) = self.drawing.as_ref() else {
            return;
        };
        let handle = tool
            .handles
            .as_ref()
            .filter(|_| !self.has_field_drafts(cx))
            .and_then(|handles| handles.begin_drag(space, event.position, window.rem_size()));
        let mode = if let Some(drag) = handle {
            DragMode::Handle(drag)
        } else if tool.is_selecting() {
            let Some(filter) = self.selected_node().and_then(|node| node.filter()) else {
                return;
            };
            let hit = filter.pick_canvas_shape_with_tolerance(
                space.tile_point(event.position),
                TILE * (window.rem_size() / 4. / space.0.size.width),
            );
            if let Some(hit) = hit {
                let branch = selection::branch(&hit);
                let Some(first) = branch.first() else {
                    return;
                };
                let additive = event.modifiers.shift || event.modifiers.control;
                let available = filter
                    .canvas_shape_editors()
                    .into_iter()
                    .map(|shape| (shape.identity(), shape))
                    .collect();
                let click = selection::Selection::from_items(
                    tool.selection.clicked(&branch, additive),
                    &available,
                );
                let hit_selected = tool
                    .selection
                    .shapes
                    .iter()
                    .any(|shape| shape.same_reference(&hit));
                let moved = if hit_selected {
                    tool.selection.clone()
                } else if additive {
                    click.clone()
                } else {
                    selection::Selection::from_items(vec![first.clone()], &available)
                };
                let orientation = tool.transform_orientation();
                let destination = moved.active_layer().unwrap_or_else(|| hit.layer());
                let Ok(edit) = filter.canvas_selection_edit_for(&moved.shapes, orientation) else {
                    return;
                };
                let moved_selection = (!hit_selected && additive).then(|| moved.clone());
                if !hit_selected && !additive {
                    self.choose_canvas_selection(moved, window, cx);
                }
                DragMode::Move {
                    click,
                    edit,
                    destination,
                    moved_selection,
                }
            } else {
                DragMode::Marquee
            }
        } else {
            DragMode::Draw
        };
        if let Some(tool) = &mut self.drawing {
            self.drawing_focus.focus(window, cx);
            tool.drag = Some(ShapeDrag {
                space,
                start: event.position,
                position: event.position,
                modifiers: event.modifiers,
                started: Instant::now(),
                mode,
            });
            cx.notify();
        }
    }

    fn move_shape_drag(&mut self, event: &MouseMoveEvent, window: &Window, cx: &mut Context<Self>) {
        let can_drag = event.pressed_button == Some(MouseButton::Left)
            && self.drawing_focus.is_focused(window)
            && self.drawing_current(cx);
        if let Some(tool) = &mut self.drawing {
            if !can_drag {
                if tool.cancel_drag() {
                    cx.notify();
                }
            } else if let Some(drag) = &mut tool.drag {
                drag.position = event.position;
                drag.modifiers = event.modifiers;
                cx.notify();
            }
        }
    }

    fn finish_shape_drag(
        &mut self,
        event: &MouseUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let can_commit = self.drawing_focus.is_focused(window) && self.drawing_current(cx);
        if let Some(curve) = self.drawing.as_mut().and_then(|tool| tool.curve.as_mut()) {
            let action = curve.release(event, can_commit);
            self.commit_curve_action(action, window, cx);
            cx.notify();
            return;
        }
        if let Some(tool) = &mut self.drawing
            && let Some(mut drag) = tool.drag.take()
        {
            let (primitive, destination) = (tool.primitive, tool.destination.clone());
            drag.position = event.position;
            drag.modifiers = event.modifiers;
            if can_commit {
                match &drag.mode {
                    DragMode::Draw => {
                        if !drag.is_click()
                            && let (Some(primitive), Some(bounds)) = (primitive, drag.bounds())
                        {
                            self.finish_shape_placement(
                                Placement::Primitive(primitive, bounds),
                                destination,
                                window,
                                cx,
                            );
                        }
                    }
                    DragMode::Move {
                        click,
                        destination,
                        moved_selection,
                        ..
                    } => {
                        if drag.is_click() {
                            self.choose_canvas_selection(click.clone(), window, cx);
                        } else if let Some(preview) = drag.move_preview() {
                            if drag.modifiers.control {
                                let mut copies = Vec::new();
                                if self.apply_canvas_shape_change(
                                    false,
                                    |_| {
                                        copies = preview.commit_copies(destination)?;
                                        Ok(!copies.is_empty())
                                    },
                                    window,
                                    cx,
                                ) {
                                    self.choose_canvas_shapes(copies, window, cx);
                                }
                            } else if self.apply_canvas_shape_change(
                                false,
                                |_| preview.commit(),
                                window,
                                cx,
                            ) && let Some(selection) = moved_selection
                            {
                                self.choose_canvas_selection(selection.clone(), window, cx);
                            }
                        }
                    }
                    DragMode::Marquee => self.finish_marquee(&drag, window, cx),
                    DragMode::Handle(handle) => {
                        if !drag.is_click()
                            && let Ok(preview) = handle.preview(&drag)
                        {
                            self.apply_canvas_shape_change(false, |_| preview.commit(), window, cx);
                        }
                    }
                }
            }
            cx.notify();
        }
    }

    fn finish_marquee(&mut self, drag: &ShapeDrag, window: &mut Window, cx: &mut Context<Self>) {
        if drag.is_click() {
            self.choose_canvas_shape(None, window, cx);
            return;
        }
        let Some(filter) = self.selected_node().and_then(|node| node.filter()) else {
            return;
        };
        let mut enclosed: HashSet<_> = filter
            .canvas_shapes_in_box(
                drag.space.tile_point(drag.start),
                drag.space.tile_point(drag.position),
            )
            .into_iter()
            .map(|shape| shape.identity())
            .collect();
        let previous = self
            .drawing
            .as_ref()
            .filter(|_| drag.modifiers.shift)
            .map(|tool| &tool.selection);
        if let Some(selection) = previous {
            enclosed.extend(selection.shapes.iter().map(CanvasShape::identity));
            enclosed.extend(selection.items.iter().map(|(_, item)| item.identity()));
        }
        let mut items = selection::marquee_items(filter.canvas_layer_editors(), &enclosed);
        if let Some(previous) = previous {
            items.sort_by_key(|(_, item)| {
                previous
                    .items
                    .iter()
                    .position(|(_, old)| old.identity() == item.identity())
                    .unwrap_or(usize::MAX)
            });
        }
        let available = filter
            .canvas_shape_editors()
            .into_iter()
            .map(|shape| (shape.identity(), shape))
            .collect();
        self.choose_canvas_selection(
            selection::Selection::from_items(items, &available),
            window,
            cx,
        );
    }

    fn finish_shape_placement(
        &mut self,
        placement: Placement,
        destination: Destination,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match destination {
            Destination::Layer(layer) => {
                self.apply_canvas_shape_edit(
                    false,
                    |_| match placement {
                        Placement::Primitive(primitive, bounds) => {
                            layer.edit_insert_shape(primitive, bounds)
                        }
                        Placement::Curve(path) => layer.edit_insert_path(path),
                    },
                    window,
                    cx,
                );
            }
            Destination::Group(layer, group, _) => {
                self.apply_canvas_shape_edit(
                    false,
                    |_| match placement {
                        Placement::Primitive(primitive, bounds) => {
                            layer.edit_insert_group_shape(&group, primitive, bounds)
                        }
                        Placement::Curve(path) => layer.edit_insert_group_path(&group, path),
                    },
                    window,
                    cx,
                );
            }
            Destination::Before(shape) => {
                let mut inserted = None;
                let applied = self.apply_canvas_shape_edit(
                    false,
                    |_| {
                        inserted = Some(match placement {
                            Placement::Primitive(primitive, bounds) => {
                                shape.edit_insert_before(primitive, bounds)?
                            }
                            Placement::Curve(path) => shape.edit_insert_path_before(path)?,
                        });
                        Ok(())
                    },
                    window,
                    cx,
                );
                if applied
                    && let Some(tool) = &mut self.drawing
                    && let Some(inserted) = inserted
                {
                    tool.destination = Destination::Before(inserted);
                    tool.refresh();
                }
            }
            Destination::NewLayer(input) => {
                let applied = self.apply_canvas_shape_edit(
                    true,
                    |filter| match placement {
                        Placement::Primitive(primitive, bounds) => {
                            filter.edit_insert_canvas_shape(primitive, bounds, input)
                        }
                        Placement::Curve(path) => filter.edit_insert_canvas_path(path, input),
                    },
                    window,
                    cx,
                );
                if applied
                    && let Some(tool) = self.drawing.as_mut()
                    && let Some(layer) = canvas_layers(&tool.target).into_iter().next()
                {
                    tool.destination = Destination::Layer(layer);
                }
            }
        }
    }

    pub(super) fn render_drawing_tools(
        &self,
        busy: bool,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let target = self.selected_node()?;
        if target.kind() != Some(PointwiseKind::Canvas) || self.lab_sections.editing_nested() {
            return None;
        }
        let selected = self.drawing.as_ref().and_then(|tool| tool.primitive);
        let curve = self.drawing.as_ref().and_then(|tool| tool.curve.as_ref());
        let pen = curve.filter(|tool| tool.is_pen());
        let freehand = match curve {
            Some(path::CurveTool::Freehand(freehand)) => Some(freehand),
            _ => None,
        };
        let selecting = self.drawing.as_ref().is_some_and(DrawingTool::is_selecting);
        let destination = self.drawing.as_ref().map(|tool| tool.destination.clone());
        let count = target.reference_inputs().ok()?.len();
        let disabled = busy || self.inspector.is_changed(cx);
        let owner = cx.weak_entity();
        let selection = self.selection.clone();
        let revision = self.revision;
        let shape_target = target.clone();
        let shape_owner = owner.clone();
        let shape_selection = selection.clone();
        let pen_target = target.clone();
        let freehand_target = target.clone();
        let add_shape = Button::new("texture-canvas-draw-shape")
            .small()
            .disabled(disabled)
            .label(selected.map_or("Draw shape…", CanvasPrimitive::name))
            .dropdown_menu(move |mut menu, _, _| {
                for primitive in CanvasPrimitive::ALL {
                    let owner = shape_owner.clone();
                    let target = shape_target.clone();
                    let selection = shape_selection.clone();
                    menu = menu.item(
                        PopupMenuItem::new(primitive.name())
                            .checked(selected == Some(primitive))
                            .on_click(move |_, window, cx| {
                                let _ = owner.update(cx, |view, cx| {
                                    if view.is_original_selection(
                                        &selection,
                                        &Some(target.clone()),
                                        revision,
                                    ) && !view.is_busy()
                                        && !view.inspector.is_changed(cx)
                                    {
                                        view.choose_canvas_tool(
                                            &target,
                                            Some(primitive),
                                            window,
                                            cx,
                                        );
                                    }
                                });
                            }),
                    );
                }
                menu.scrollable(true)
            });

        let select_target = target.clone();
        let select_shape = Button::new("texture-canvas-select-shape")
            .small()
            .label("Select shapes")
            .selected(selecting)
            .disabled(disabled)
            .on_click(cx.listener(move |view, _, window, cx| {
                if view
                    .selected_node()
                    .is_some_and(|node| node.same_reference(&select_target))
                    && !view.is_busy()
                    && !view.inspector.is_changed(cx)
                {
                    view.choose_canvas_tool(&select_target, None, window, cx);
                }
            }));

        Some(
            div()
                .h_flex()
                .items_center()
                .flex_wrap()
                .gap_2()
                .child(add_shape)
                .child(
                    Button::new("texture-canvas-draw-curve")
                        .small()
                        .label("Draw curve")
                        .selected(pen.is_some())
                        .disabled(disabled)
                        .on_click(cx.listener(move |view, _, window, cx| {
                            view.choose_canvas_curve(
                                &pen_target,
                                path::CurveDrawingMode::Tangents,
                                window,
                                cx,
                            );
                        })),
                )
                .child(
                    Button::new("texture-canvas-freehand")
                        .small()
                        .label("Freehand")
                        .selected(freehand.is_some())
                        .disabled(disabled)
                        .on_click(cx.listener(move |view, _, window, cx| {
                            view.choose_canvas_curve(
                                &freehand_target,
                                path::CurveDrawingMode::Freehand,
                                window,
                                cx,
                            );
                        })),
                )
                .child(select_shape)
                .when(
                    selected.is_some() || pen.is_some() || freehand.is_some(),
                    |row| {
                        let choices: Vec<_> = destination
                            .iter()
                            .filter(|dest| matches!(dest, Destination::Before(_)))
                            .cloned()
                            .chain(canvas_layers(&target).into_iter().flat_map(|layer| {
                                let mut choices = vec![Destination::Layer(layer.clone())];
                                choices.extend(group_destinations(&layer));
                                choices
                            }))
                            .chain(std::iter::once(Destination::NewLayer(None)))
                            .chain((0..count).map(|index| Destination::NewLayer(Some(index))))
                            .collect();
                        row.when_some(destination, |row, destination| {
                            row.child(
                                Button::new("texture-canvas-destination")
                                    .small()
                                    .disabled(disabled || choices.len() < 2)
                                    .label(destination.label())
                                    .dropdown_menu(move |mut menu, _, _| {
                                        for choice in &choices {
                                            let owner = owner.clone();
                                            let selection = selection.clone();
                                            let target = target.clone();
                                            let choice = choice.clone();
                                            menu = menu.item(
                                                PopupMenuItem::new(choice.label())
                                                    .checked(destination.same_choice(&choice))
                                                    .on_click(move |_, window, cx| {
                                                        let _ = owner.update(cx, |view, cx| {
                                                            if view.is_original_selection(
                                                                &selection,
                                                                &Some(target.clone()),
                                                                revision,
                                                            ) && view.drawing_current(cx)
                                                                && let Some(tool) =
                                                                    view.drawing.as_mut()
                                                            {
                                                                tool.destination = choice.clone();
                                                                tool.cancel_drag();
                                                                view.drawing_focus
                                                                    .focus(window, cx);
                                                                cx.notify();
                                                            }
                                                        });
                                                    }),
                                            );
                                        }
                                        menu.scrollable(true)
                                    }),
                            )
                        })
                    },
                )
                .when_some(freehand, |row, freehand| {
                    row.child(freehand.control(disabled, cx))
                })
                .when_some(pen, |row, pen| {
                    row.child(
                        Button::new("texture-canvas-finish-curve")
                            .small()
                            .label("Finish open curve")
                            .disabled(disabled || !pen.can_finish())
                            .on_click(cx.listener(|view, _, window, cx| {
                                view.finish_canvas_curve(window, cx)
                            })),
                    )
                })
                .when(self.drawing.is_some(), |row| {
                    row.child(
                        Button::new("texture-canvas-grid-settings")
                            .small()
                            .ghost()
                            .label("Snap grid…")
                            .disabled(disabled)
                            .on_click(
                                cx.listener(|view, _, window, cx| {
                                    view.open_canvas_grid(window, cx)
                                }),
                            ),
                    )
                    .children(self.render_design_plan_menu(disabled, cx))
                    .child(
                        Button::new("texture-canvas-draw-done")
                            .small()
                            .ghost()
                            .label("Done")
                            .disabled(disabled)
                            .on_click(
                                cx.listener(|view, _, window, cx| view.leave_drawing(window, cx)),
                            ),
                    )
                })
                .into_any_element(),
        )
    }

    pub(super) fn render_canvas_shape_panel(
        &self,
        busy: bool,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let tool = self.drawing.as_ref().filter(|tool| tool.is_selecting())?;
        let target = tool.target.clone();
        let selected: HashSet<_> = tool
            .selection
            .items
            .iter()
            .map(|(_, item)| item.identity())
            .collect();
        let fields = self
            .inspector
            .drafts
            .iter()
            .filter(|draft| draft.canvas_shape_field().is_some())
            .map(|draft| {
                Field::new().label(draft.label.clone()).child(
                    Input::new(&draft.input)
                        .id(draft.id.clone())
                        .aria_label(draft.label.clone())
                        .disabled(busy),
                )
            });
        let owner = cx.weak_entity();
        let revision = self.revision;
        let key = self.selection.clone();
        let label = match tool.selection.items.as_slice() {
            [] => "Select shape".into(),
            [(_, item)] => tool
                .selection
                .active_shape()
                .map_or_else(|| item.label().to_owned(), |shape| shape.label().to_owned()),
            items if tool.selection.has_group() => format!("{} items selected", items.len()),
            items => format!("{} shapes selected", items.len()),
        };
        Some(
            div()
                .v_flex()
                .gap_2()
                .child(div().font_semibold().child("Selection"))
                .child(
                    Button::new("texture-canvas-shape-selection")
                        .small()
                        .label(label)
                        .disabled(busy || self.inspector.is_changed(cx))
                        .dropdown_menu(move |mut menu, _, _| {
                            let available = target
                                .filter()
                                .map_or_else(Vec::new, |filter| filter.canvas_shape_editors());
                            if available.is_empty() {
                                return menu
                                    .item(PopupMenuItem::new("No editable shapes").disabled(true));
                            }
                            for shape in available {
                                let owner = owner.clone();
                                let target = target.clone();
                                let key = key.clone();
                                let current = selected.contains(&shape.identity());
                                menu = menu.item(
                                    PopupMenuItem::new(shape.label().to_owned())
                                        .checked(current)
                                        .on_click(move |_, window, cx| {
                                            let _ = owner.update(cx, |view, cx| {
                                                if view.is_original_selection(
                                                    &key,
                                                    &Some(target.clone()),
                                                    revision,
                                                ) && view.drawing_current(cx)
                                                    && view
                                                        .drawing
                                                        .as_ref()
                                                        .is_some_and(DrawingTool::is_selecting)
                                                {
                                                    view.choose_canvas_shape(
                                                        Some(shape.clone()),
                                                        window,
                                                        cx,
                                                    );
                                                }
                                            });
                                        }),
                                );
                            }
                            for layer in canvas_layers(&target) {
                                for group in group_destinations(&layer) {
                                    let Destination::Group(layer, group, label) = group else {
                                        continue;
                                    };
                                    let owner = owner.clone();
                                    let target = target.clone();
                                    let key = key.clone();
                                    let current = selected.contains(&group.identity());
                                    menu = menu.item(
                                        PopupMenuItem::new(label).checked(current).on_click(
                                            move |_, window, cx| {
                                                let _ = owner.update(cx, |view, cx| {
                                                    if view.is_original_selection(
                                                        &key,
                                                        &Some(target.clone()),
                                                        revision,
                                                    ) && view.drawing_current(cx)
                                                        && view
                                                            .drawing
                                                            .as_ref()
                                                            .is_some_and(DrawingTool::is_selecting)
                                                        && let Some(filter) = target.filter()
                                                    {
                                                        let available = filter
                                                            .canvas_shape_editors()
                                                            .into_iter()
                                                            .map(|shape| (shape.identity(), shape))
                                                            .collect();
                                                        let selected =
                                                            selection::Selection::from_items(
                                                                vec![(
                                                                    layer.clone(),
                                                                    group.clone(),
                                                                )],
                                                                &available,
                                                            );
                                                        view.choose_canvas_selection(
                                                            selected, window, cx,
                                                        );
                                                    }
                                                });
                                            },
                                        ),
                                    );
                                }
                            }
                            menu.scrollable(true)
                        }),
                )
                .when(self.inspector.canvas_shape.is_some(), |panel| {
                    panel.child(Form::new().columns(2).children(fields))
                })
                .when(
                    tool.single_selection()
                        .is_some_and(|shape| shape.path().is_some()),
                    |panel| panel.child(self.render_point_tools(tool, busy, cx)),
                )
                .when(!tool.selection.is_empty(), |panel| {
                    panel
                        .child(
                            div()
                                .h_flex()
                                .flex_wrap()
                                .gap_2()
                                .child(self.render_canvas_transform_menu(tool, busy, cx))
                                .children(
                                    CurveCommand::IMMEDIATE
                                        .into_iter()
                                        .filter(|command| command.available(&tool.selection.shapes))
                                        .map(|command| {
                                            let (id, label) = command.button();
                                            Button::new(id)
                                                .small()
                                                .ghost()
                                                .label(label)
                                                .disabled(busy || self.inspector.is_changed(cx))
                                                .on_click(cx.listener(
                                                    move |view, _, window, cx| {
                                                        view.edit_canvas_curves(
                                                            command, window, cx,
                                                        );
                                                    },
                                                ))
                                        }),
                                )
                                .child(
                                    Button::new("texture-canvas-delete-shape")
                                        .small()
                                        .ghost()
                                        .label(if tool.selection.has_group() {
                                            "Delete selected items"
                                        } else if tool.selection.items.len() == 1 {
                                            "Delete shape"
                                        } else {
                                            "Delete selected shapes"
                                        })
                                        .disabled(busy || self.inspector.is_changed(cx))
                                        .on_click(cx.listener(|view, _, window, cx| {
                                            view.delete_canvas_shape(window, cx)
                                        })),
                                ),
                        )
                        .when(
                            CurveCommand::Optimize.available(&tool.selection.shapes),
                            |panel| panel.child(self.render_curve_optimize(tool, busy, cx)),
                        )
                        .child(self.render_canvas_handle_controls(tool, busy, cx))
                })
                .into_any_element(),
        )
    }

    fn render_curve_optimize(
        &self,
        tool: &DrawingTool,
        busy: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let command = CurveCommand::Optimize;
        let (id, label) = command.button();
        let disabled = busy || self.inspector.is_changed(cx);
        let strength = tool.optimize_strength.read(cx).value().start();
        div()
            .h_flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(div().text_sm().child("Strength"))
            .child(
                div()
                    .id("texture-canvas-optimize-strength")
                    .test_support()
                    .role(gpui_kit::Role::Group)
                    .aria_label("Curve optimization strength")
                    .w(rems(8.))
                    .child(Slider::new(&tool.optimize_strength).disabled(disabled)),
            )
            .child(div().text_sm().child(format!("{strength:.0}")))
            .child(
                Button::new(id)
                    .small()
                    .ghost()
                    .label(label)
                    .disabled(disabled)
                    .on_click(cx.listener(move |view, _, window, cx| {
                        view.edit_canvas_curves(command, window, cx)
                    })),
            )
    }

    fn edit_canvas_curves(
        &mut self,
        command: CurveCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.drawing_current(cx) {
            return;
        }
        let Some((selection, strength)) = self
            .drawing
            .as_ref()
            .filter(|tool| tool.is_selecting() && command.available(&tool.selection.shapes))
            .map(|tool| {
                (
                    tool.selection.shapes.clone(),
                    tool.optimize_strength.read(cx).value().start(),
                )
            })
        else {
            return;
        };
        let Some(active) = selection.last() else {
            return;
        };
        let mut edited = Vec::new();
        let mut no_change = false;
        let applied = self.apply_canvas_shape_change(
            false,
            |filter| {
                edited = match command {
                    CurveCommand::Merge => {
                        vec![filter.edit_merge_canvas_curves(&selection, active)?]
                    }
                    CurveCommand::Separate => active.edit_separate_curve()?,
                    CurveCommand::Optimize => {
                        filter.edit_optimize_canvas_curves(&selection, strength)?
                    }
                };
                no_change = edited.is_empty();
                Ok(!no_change)
            },
            window,
            cx,
        );
        if applied {
            self.choose_canvas_shapes(edited, window, cx);
            self.drawing_focus.focus(window, cx);
        } else if no_change {
            self.status = Some("No selected curves could be simplified at this strength.".into());
            cx.notify();
        }
    }

    pub(super) fn render_drawing_surface(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let tool = self.drawing.as_ref()?;
        let can_draw = !self.preview_pending && self.drawing_current(cx);
        let transform_preview = tool.drag.as_ref().and_then(|drag| {
            if let DragMode::Handle(handle) = &drag.mode {
                handle.preview(drag).ok()
            } else {
                drag.move_preview()
            }
        });
        let handle_frame = if tool.handles.is_none() {
            None
        } else {
            transform_preview
                .as_ref()
                .map(mycela_engine::CanvasHandlePreview::frame)
                .or_else(|| tool.handles.as_ref()?.frame())
        };
        let outlines = match tool.primitive {
            Some(primitive) => tool
                .drag
                .as_ref()
                .and_then(|drag| primitive.outline(drag.bounds()?).ok())
                .map(|mut outline| {
                    if let Some(first) = outline.first().copied() {
                        outline.push(first);
                    }
                    vec![outline]
                }),
            None if tool.curve.is_some() => None,
            None if let Some(preview) = transform_preview => preview.outlines().ok(),
            None => Some(
                tool.selection
                    .shapes
                    .iter()
                    .filter_map(|shape| shape.guide_outline(None).ok())
                    .flatten()
                    .collect(),
            ),
        };
        let selecting = tool.is_selecting();
        let curve = tool.curve.clone();
        let grid_preferences = grid::GridPreferences::current(cx);
        let snap_grid = grid_preferences.snap_grid();
        let marquee = tool
            .drag
            .as_ref()
            .filter(|drag| matches!(drag.mode, DragMode::Marquee) && drag.position != drag.start)
            .map(ShapeDrag::box_bounds);
        let space = Rc::new(Cell::new(None));
        let layout = space.clone();
        let focus = self.drawing_focus.clone();
        let mover = cx.weak_entity();
        let releaser = cx.weak_entity();
        let theme = cx.theme();
        let (accent, background, grid) = (theme.primary, theme.background, theme.border);
        let surface = canvas(
            move |bounds, window, _| {
                let fitted = TileSpace::fit(bounds, snap_grid);
                layout.set(fitted);
                (fitted, window.rem_size() / 16.)
            },
            move |_, (space, stroke), window, _| {
                if let Some(space) = space {
                    window.with_content_mask(Some(ContentMask { bounds: space.0 }), |window| {
                        paint_guide(
                            space,
                            grid_preferences,
                            outlines.as_deref().unwrap_or_default(),
                            marquee,
                            stroke,
                            (accent, background, grid),
                            window,
                        );
                        if let Some(curve) = &curve {
                            curve.paint(space, stroke, (accent, background), window);
                        } else if let Some(frame) = handle_frame {
                            handles::paint(frame, space, stroke, (accent, background), window);
                        }
                    });
                }
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if phase == DispatchPhase::Capture {
                        let _ = mover.update(cx, |view, cx| {
                            if let Some(space) = space
                                && view.move_canvas_curve(space, event, window, cx)
                            {
                                return;
                            }
                            if view
                                .drawing
                                .as_ref()
                                .is_some_and(|tool| tool.drag.is_some())
                            {
                                view.move_shape_drag(event, window, cx);
                            }
                        });
                    }
                });
                window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                    if phase == DispatchPhase::Capture && event.button == MouseButton::Left {
                        let _ = releaser
                            .update(cx, |view, cx| view.finish_shape_drag(event, window, cx));
                    }
                });
            },
        )
        .size_full();

        Some(
            div()
                .id("texture-canvas-drawing-surface")
                .test_support()
                .role(gpui_kit::Role::Group)
                .aria_label(if selecting {
                    "Canvas tile shape selection"
                } else {
                    "Canvas tile drawing surface"
                })
                .absolute()
                .size_full()
                .track_focus(&focus)
                .tab_stop(true)
                .key_context("TextureCanvasDrawing")
                .when(can_draw && !selecting, |surface| surface.cursor_crosshair())
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |view, event, window, cx| {
                        if let Some(space) = space.get() {
                            view.start_shape_drag(space, event, window, cx);
                        }
                    }),
                )
                .on_modifiers_changed(cx.listener(
                    |view, event: &gpui_kit::ModifiersChangedEvent, window, cx| {
                        if view.drawing_focus.is_focused(window)
                            && let Some(drag) =
                                view.drawing.as_mut().and_then(|tool| tool.drag.as_mut())
                        {
                            drag.modifiers = event.modifiers;
                            cx.notify();
                        }
                    },
                ))
                .on_action(cx.listener(|view, _: &CancelCanvasDrawing, window, cx| {
                    if view.cancel_canvas_curve()
                        || view.drawing.as_mut().is_some_and(DrawingTool::cancel_drag)
                    {
                        cx.notify();
                    } else {
                        view.leave_drawing(window, cx);
                    }
                    cx.stop_propagation();
                }))
                .on_action(cx.listener(|view, _: &FinishCanvasPath, window, cx| {
                    if view
                        .drawing
                        .as_ref()
                        .and_then(|tool| tool.curve.as_ref())
                        .is_some_and(path::CurveTool::is_pen)
                    {
                        view.finish_canvas_curve(window, cx);
                        cx.stop_propagation();
                    }
                }))
                .on_action(cx.listener(|view, _: &DeleteCanvasShape, window, cx| {
                    if view.delete_canvas_point(window, cx) {
                        cx.stop_propagation();
                    } else if view.drawing.as_ref().is_some_and(DrawingTool::is_selecting) {
                        view.delete_canvas_shape(window, cx);
                        cx.stop_propagation();
                    }
                }))
                .child(surface)
                .into_any_element(),
        )
    }
}

fn paint_guide(
    space: TileSpace,
    grid_preferences: grid::GridPreferences,
    outlines: &[Vec<[f64; 2]>],
    marquee: Option<Bounds<Pixels>>,
    stroke: Pixels,
    (accent, background, grid): (Hsla, Hsla, Hsla),
    window: &mut Window,
) {
    let bounds = space.0;
    paint_border(bounds, stroke, accent, window);
    if let Some(marquee) = marquee {
        paint_selection_box(marquee, stroke, accent, window);
    }
    grid::paint(space, grid_preferences, stroke, grid, window);
    if outlines.is_empty() {
        return;
    }
    for (width, color) in [(3. * stroke, background), (stroke, accent)] {
        let mut path = PathBuilder::stroke(width);
        for x in [-TILE, 0., TILE] {
            for y in [-TILE, 0., TILE] {
                let project = |[u, v]: [f64; 2]| space.window_point([u as f32 + x, v as f32 + y]);
                for vertices in outlines.iter().filter(|points| points.len() > 1) {
                    path.move_to(project(vertices[0]));
                    for &vertex in &vertices[1..] {
                        path.line_to(project(vertex));
                    }
                }
            }
        }
        if let Ok(path) = path.build() {
            window.paint_path(path, color);
        }
    }
}

fn paint_selection_box(marquee: Bounds<Pixels>, stroke: Pixels, accent: Hsla, window: &mut Window) {
    if marquee.size.width >= stroke && marquee.size.height >= stroke {
        window.paint_quad(fill(
            marquee,
            Hsla {
                a: accent.a * 0.12,
                ..accent
            },
        ));
        paint_border(marquee, stroke, accent, window);
    }
}

fn paint_border(bounds: Bounds<Pixels>, stroke: Pixels, color: Hsla, window: &mut Window) {
    for edge in [
        Bounds::new(bounds.origin, size(bounds.size.width, stroke)),
        Bounds::new(
            point(bounds.left(), bounds.bottom() - stroke),
            size(bounds.size.width, stroke),
        ),
        Bounds::new(bounds.origin, size(stroke, bounds.size.height)),
        Bounds::new(
            point(bounds.right() - stroke, bounds.top()),
            size(stroke, bounds.size.height),
        ),
    ] {
        window.paint_quad(fill(edge, color));
    }
}
