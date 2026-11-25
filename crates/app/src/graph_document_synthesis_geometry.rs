//! Normalized Synthesis geometry in the graph inspector.
use super::*;
use SynthesisGeometry::{Crop, Grid, GridPoint, Patch, Placement};
use gpui_kit::assets::IconName;
use gpui_kit::{
    Bounds, ContentMask, CursorStyle, FillOptions, FillRule, Hsla, Modifiers, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, PathBuilder, PathStyle, Pixels, Point,
    ScrollDelta, ScrollWheelEvent, canvas, fill, point, size,
};
use mycela_engine::{SynthesisCropHandle, SynthesisPatchShape};
use std::{cell::Cell, rc::Rc};

#[path = "graph_document_synthesis_geometry_full_size.rs"]
mod full_size;
#[path = "graph_document_synthesis_manual.rs"]
pub(super) mod manual;
#[path = "graph_document_synthesis_geometry_patch.rs"]
mod patch;
use mycela_engine::{SynthesisPatchTransformBounds, SynthesisPatchTransformField};
pub(super) use patch::TransformNumbers;
use patch::{PatchPart, TransformBox};

#[derive(Clone, Copy)]
pub(super) enum GeometryCommand {
    Select(Option<SynthesisGeometry>),
    SelectPatch(usize),
    Move(Handle, [f32; 2]),
    MoveGrid([f32; 2]),
    TransformPatch(usize, mycela_engine::SynthesisPatchTransform, ImageGeometry),
    SetPatchTransformField(usize, SynthesisPatchTransformField, f64, ImageGeometry),
    TogglePatchTool,
    Crop(SynthesisCropHandle, [f32; 2], Modifiers),
    AddPatch,
    CreatePatch(SynthesisPatchShape),
    RemovePatch(usize),
    InsertKnot(usize, usize),
    SplitSegment(usize, usize, f64),
    RemoveKnot(usize, usize),
    KnotMode(usize, usize, usize),
    Flip(usize, usize),
    AddPlacement(usize),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Handle {
    Point(SynthesisGeometry),
    Crop(SynthesisCropHandle),
    PatchTransform {
        selection: usize,
        part: PatchPart,
    },
    Tangent {
        selection: usize,
        knot: usize,
        incoming: bool,
    },
}

impl Handle {
    fn selection(self) -> SynthesisGeometry {
        match self {
            Self::Point(point) => point,
            Self::Crop(_) => Crop,
            Self::PatchTransform { selection, .. } => Patch { selection, knot: 0 },
            Self::Tangent {
                selection, knot, ..
            } => Patch { selection, knot },
        }
    }

    fn apply(self, edit: &mut SynthesisEdit, layer: usize, position: [f32; 2]) -> bool {
        match self {
            Self::Point(geometry) => edit.move_geometry_point(layer, geometry, position),
            Self::Crop(_) | Self::PatchTransform { .. } => false,
            Self::Tangent {
                selection,
                knot,
                incoming,
            } => edit.move_patch_handle(layer, selection, knot, incoming, position),
        }
    }
}

fn crop_cursor(handle: SynthesisCropHandle, dragging: bool) -> CursorStyle {
    use SynthesisCropHandle::*;
    match handle {
        Move if dragging => CursorStyle::ClosedHand,
        Move => CursorStyle::OpenHand,
        North | South => CursorStyle::ResizeUpDown,
        West | East => CursorStyle::ResizeLeftRight,
        NorthWest | SouthEast => CursorStyle::ResizeUpLeftDownRight,
        NorthEast | SouthWest => CursorStyle::ResizeUpRightDownLeft,
    }
}

#[derive(Clone, Copy)]
pub(super) struct Drag {
    handle: Handle,
    space: Bounds<Pixels>,
    fitted: Bounds<Pixels>,
    start: Point<Pixels>,
    original: [f32; 2],
    position: [f32; 2],
    modifiers: Modifiers,
}

impl Drag {
    fn patch_transform(
        self,
        position: [f32; 2],
        modifiers: Modifiers,
    ) -> Option<(usize, mycela_engine::SynthesisPatchTransform, ImageGeometry)> {
        let Handle::PatchTransform { selection, part } = self.handle else {
            return None;
        };
        let fitted = fitted_geometry(self.fitted);
        Some((selection, part.transform(self, position, modifiers), fitted))
    }

    fn update(self, cursor: Point<Pixels>) -> [f32; 2] {
        let delta = cursor - self.start;
        let delta = [
            delta.x / self.space.size.width,
            delta.y / self.space.size.height,
        ];
        if matches!(self.handle, Handle::Crop(_)) {
            return delta;
        }
        let position = [self.original[0] + delta[0], self.original[1] + delta[1]];
        match self.handle {
            Handle::Tangent { .. } | Handle::Crop(_) | Handle::PatchTransform { .. } => position,
            Handle::Point(_) => position.map(|coord| coord.clamp(0., 1.)),
        }
    }

    fn moved(self, cursor: Point<Pixels>) -> bool {
        let delta = cursor - self.start;
        delta.x.abs().max(delta.y.abs()) >= gpui_kit::px(3.)
    }
}

#[derive(Clone, Copy)]
struct GuideFrame {
    viewport: Bounds<Pixels>,
    fitted: Bounds<Pixels>,
    image: Bounds<Pixels>,
}

fn fitted_geometry(fitted: Bounds<Pixels>) -> ImageGeometry {
    ImageGeometry::new(
        fitted.size.width.as_f32().round().max(1.) as i32,
        fitted.size.height.as_f32().round().max(1.) as i32,
    )
}

#[derive(Clone, Copy)]
struct SourcePan {
    start: Point<Pixels>,
    center: [f32; 2],
    frame: GuideFrame,
}

#[derive(Clone, Copy, PartialEq)]
struct SourceView {
    zoom: f32,
    center: [f32; 2],
}

impl Default for SourceView {
    fn default() -> Self {
        Self {
            zoom: 1.,
            center: [0.5; 2],
        }
    }
}

impl SourceView {
    const MIN_ZOOM: f32 = 0.125;
    const MAX_ZOOM: f32 = 32.;

    fn image_bounds(self, fitted: Bounds<Pixels>) -> Bounds<Pixels> {
        let scaled = size(
            fitted.size.width * self.zoom,
            fitted.size.height * self.zoom,
        );
        let center = fitted.center();
        Bounds::new(
            center
                - point(
                    scaled.width * self.center[0],
                    scaled.height * self.center[1],
                ),
            scaled,
        )
    }

    fn zoom(&mut self, factor: f32, anchor: [f32; 2]) -> bool {
        let next = (self.zoom * factor).clamp(Self::MIN_ZOOM, Self::MAX_ZOOM);
        if next == self.zoom {
            return false;
        }
        let movement = 1. - self.zoom / next;
        for (center, anchor) in self.center.iter_mut().zip(anchor) {
            let offset = (anchor / self.zoom).clamp(-*center, 1. - *center);
            *center += movement * offset;
        }
        self.zoom = next;
        true
    }

    fn pan(&mut self, pan: SourcePan, position: Point<Pixels>) -> bool {
        let delta = position - pan.start;
        let viewport = pan.frame.viewport.size;
        let image = pan.frame.image.size;
        let center = [
            (-delta.x / image.width, viewport.width / image.width),
            (-delta.y / image.height, viewport.height / image.height),
        ]
        .map(|(movement, span)| (movement, span / 2. - span.min(1.) / 4.));
        let center = std::array::from_fn(|axis| {
            let (movement, overhang) = center[axis];
            (pan.center[axis] + movement).clamp(-overhang, 1. + overhang)
        });
        if center == self.center {
            return false;
        }
        self.center = center;
        true
    }
}

#[derive(Clone, Copy)]
enum SourceViewEvent {
    Zoom(f32, [f32; 2]),
    Fit,
    BeginPan(Point<Pixels>, GuideFrame),
    Pan(Point<Pixels>),
    EndPan,
}

#[derive(Default)]
pub(super) struct SourceNavigation {
    fitted: Rc<Cell<Option<ImageGeometry>>>,
    view: SourceView,
    pan: Option<SourcePan>,
}

fn patch_values(
    edit: &SynthesisEdit,
    layer: usize,
    geometry: Option<SynthesisGeometry>,
    drag: Option<Drag>,
) -> Option<SynthesisPatchTransformBounds> {
    if let Some((selection, transform, fitted)) =
        drag.and_then(|drag| drag.patch_transform(drag.position, drag.modifiers))
    {
        return edit
            .preview_patch_transform(layer, selection, transform, Some(fitted))?
            .patch_transform_bounds(layer, selection);
    }
    edit.patch_transform_bounds(layer, PatchList::selected(geometry)?)
}

impl Panel {
    pub(super) fn sync_patch_numbers(&self, window: &mut Window, cx: &mut App) {
        if let (Some(numbers), Some(layer)) = (&self.patch_numbers, self.layer)
            && let Some(bounds) = patch_values(&self.edit, layer, self.geometry, self.drag)
        {
            numbers.sync(bounds, window, cx);
        }
    }
}

impl SourceNavigation {
    pub(super) fn copy_view_from(&mut self, other: &Self) {
        self.view = other.view;
        self.fitted = other.fitted.clone();
    }

    fn update(&mut self, event: SourceViewEvent, editing: bool) -> bool {
        if editing {
            return false;
        }
        match event {
            SourceViewEvent::Zoom(factor, offset) if self.pan.is_none() => {
                self.view.zoom(factor, offset)
            }
            SourceViewEvent::Zoom(..) => false,
            SourceViewEvent::Fit if self.pan.is_none() && self.view != SourceView::default() => {
                self.view = SourceView::default();
                true
            }
            SourceViewEvent::Fit => false,
            SourceViewEvent::BeginPan(start, mut frame) => {
                frame.image = self.view.image_bounds(frame.fitted);
                self.pan = Some(SourcePan {
                    start,
                    center: self.view.center,
                    frame,
                });
                true
            }
            SourceViewEvent::Pan(position) => {
                self.pan.is_some_and(|pan| self.view.pan(pan, position))
            }
            SourceViewEvent::EndPan => self.pan.take().is_some(),
        }
    }

    fn controls<T: 'static>(
        &self,
        inline: bool,
        disabled: bool,
        patch_tool: Option<Button>,
        navigate: impl Fn(&mut T, SourceViewEvent, &mut Context<T>) + 'static,
        cx: &Context<T>,
    ) -> impl IntoElement {
        let [out, value, zoom_in, fit] = if inline {
            [
                "synthesis-inline-source-zoom-out",
                "synthesis-inline-source-zoom-value",
                "synthesis-inline-source-zoom-in",
                "synthesis-inline-source-fit",
            ]
        } else {
            [
                "synthesis-source-zoom-out",
                "synthesis-source-zoom-value",
                "synthesis-source-zoom-in",
                "synthesis-source-fit",
            ]
        };
        let disabled = disabled || self.pan.is_some();
        let navigate = Rc::new(navigate);
        let control = |id, label, tooltip, icon: Option<IconName>, event, unavailable| {
            let navigate = navigate.clone();
            let button = Button::new(id)
                .small()
                .tooltip(tooltip)
                .tab_index(0)
                .disabled(disabled || unavailable)
                .on_click(cx.listener(move |owner, _, _, cx| navigate(owner, event, cx)));
            if let Some(icon) = icon.filter(|_| inline) {
                button.icon(icon).accessibility_label(label)
            } else {
                button.label(label)
            }
        };
        let level = format!("{:.0}%", self.view.zoom * 100.);
        div()
            .h_flex()
            .flex_wrap()
            .gap_2()
            .child(control(
                out,
                "Zoom out",
                "Zoom out around the center. Right-drag the source to pan.",
                Some(IconName::ZoomOut),
                SourceViewEvent::Zoom(0.5, [0.; 2]),
                self.view.zoom == SourceView::MIN_ZOOM,
            ))
            .child(
                div()
                    .id(value)
                    .test_support()
                    .role(gpui_kit::Role::Status)
                    .aria_label(level.clone())
                    .text_sm()
                    .child(level),
            )
            .child(control(
                zoom_in,
                "Zoom in",
                "Zoom in around the center. Scroll to zoom at the pointer.",
                Some(IconName::ZoomIn),
                SourceViewEvent::Zoom(2., [0.; 2]),
                self.view.zoom == SourceView::MAX_ZOOM,
            ))
            .child(control(
                fit,
                "Fit",
                "Fit the source. Scroll to zoom or right-drag to pan.",
                None,
                SourceViewEvent::Fit,
                self.view == SourceView::default(),
            ))
            .children(patch_tool)
    }
}

#[derive(Clone, Copy)]
enum GridEvent {
    Begin {
        index: Option<usize>,
        space: Bounds<Pixels>,
        start: Point<Pixels>,
        extend: bool,
    },
    Track(Point<Pixels>),
    Release(Point<Pixels>),
}

#[derive(Clone, Copy)]
struct GridGesture {
    start: Point<Pixels>,
    cursor: Point<Pixels>,
    space: Bounds<Pixels>,
    marquee: bool,
}

impl GridGesture {
    fn moved(self) -> bool {
        let delta = self.cursor - self.start;
        delta.x.abs().max(delta.y.abs()) >= gpui_kit::px(3.)
    }

    fn delta(self) -> [f32; 2] {
        let delta = self.cursor - self.start;
        [
            delta.x / self.space.size.width,
            delta.y / self.space.size.height,
        ]
    }

    fn bounds(self) -> Bounds<Pixels> {
        let origin = point(
            self.start.x.min(self.cursor.x),
            self.start.y.min(self.cursor.y),
        );
        let delta = self.cursor - self.start;
        Bounds::new(origin, size(delta.x.abs(), delta.y.abs()))
    }
}

#[derive(Default)]
pub(super) struct GridSelection {
    indices: Vec<usize>,
    gesture: Option<GridGesture>,
}

impl GridSelection {
    pub(super) fn new(geometry: Option<SynthesisGeometry>) -> Self {
        let mut grid = Self::default();
        grid.select(geometry);
        grid
    }

    fn select(&mut self, geometry: Option<SynthesisGeometry>) {
        self.indices.clear();
        self.gesture = None;
        if let Some(GridPoint(index)) = geometry {
            self.indices.push(index);
        }
    }

    pub(super) fn copy_points_from(&mut self, other: &Self) {
        self.indices.clone_from(&other.indices);
    }

    fn geometry(&self) -> SynthesisGeometry {
        match self.indices.as_slice() {
            [only] => GridPoint(*only),
            _ => Grid,
        }
    }

    fn update(&mut self, event: GridEvent, points: &[[f32; 2]]) -> GuideUpdate {
        match event {
            GridEvent::Begin {
                index,
                space,
                start,
                extend,
            } => {
                if let Some(index) = index {
                    match self.indices.binary_search(&index) {
                        Ok(existing) if extend => {
                            self.indices.remove(existing);
                        }
                        Ok(_) => {}
                        Err(insert) if extend => self.indices.insert(insert, index),
                        Err(_) => {
                            self.indices.clear();
                            self.indices.push(index);
                        }
                    }
                } else if !extend {
                    self.indices.clear();
                }
                self.gesture = Some(GridGesture {
                    start,
                    cursor: start,
                    space,
                    marquee: index.is_none(),
                });
                GuideUpdate::Repaint
            }
            GridEvent::Track(cursor) => {
                if let Some(gesture) = self.gesture.as_mut()
                    && gesture.cursor != cursor
                {
                    gesture.cursor = cursor;
                    return GuideUpdate::Repaint;
                }
                GuideUpdate::None
            }
            GridEvent::Release(cursor) => {
                let Some(mut gesture) = self.gesture.take() else {
                    return GuideUpdate::None;
                };
                gesture.cursor = cursor;
                let delta = if gesture.marquee {
                    if gesture.moved() {
                        let bounds = gesture.bounds();
                        for (index, &point) in points.iter().enumerate() {
                            if visible_position(Handle::Point(GridPoint(index)), point)
                                && bounds.contains(&project(gesture.space, point))
                                && let Err(insert) = self.indices.binary_search(&index)
                            {
                                self.indices.insert(insert, index);
                            }
                        }
                    }
                    [0.; 2]
                } else if gesture.moved() {
                    gesture.delta()
                } else {
                    [0.; 2]
                };
                GuideUpdate::Edit(GeometryCommand::MoveGrid(delta))
            }
        }
    }

    fn moving(&self) -> Option<[f32; 2]> {
        self.gesture
            .filter(|gesture| !gesture.marquee && gesture.moved())
            .map(GridGesture::delta)
    }

    fn marquee(&self) -> Option<Bounds<Pixels>> {
        self.gesture
            .filter(|gesture| gesture.marquee && gesture.moved())
            .map(GridGesture::bounds)
    }
}

#[derive(Clone, Copy)]
enum GuideEvent {
    Navigate(SourceViewEvent),
    Grid(GridEvent),
    Edit(GeometryCommand),
    Begin(Drag),
    CancelTransform,
    Track(Point<Pixels>, Modifiers, Option<GuideHover>),
    Release(Point<Pixels>, Modifiers, bool),
}

enum GuideUpdate {
    None,
    Repaint,
    Edit(GeometryCommand),
}

fn guide_update(
    drag: &mut Option<Drag>,
    hover: &mut Option<GuideHover>,
    grid: &mut GridSelection,
    points: &[[f32; 2]],
    event: GuideEvent,
) -> GuideUpdate {
    match event {
        GuideEvent::Navigate(_) => GuideUpdate::None,
        GuideEvent::Grid(event) => grid.update(event, points),
        GuideEvent::Edit(command) => GuideUpdate::Edit(command),
        GuideEvent::Begin(next) => {
            *hover = None;
            *drag = Some(next);
            GuideUpdate::Repaint
        }
        GuideEvent::CancelTransform => {
            *drag = None;
            *hover = None;
            GuideUpdate::Repaint
        }
        GuideEvent::Track(cursor, modifiers, hit) => {
            if let Some(drag) = drag {
                let next = if drag.moved(cursor) {
                    drag.update(cursor)
                } else {
                    drag.original
                };
                if drag.position != next
                    || (matches!(drag.handle, Handle::Crop(_) | Handle::PatchTransform { .. })
                        && drag.modifiers != modifiers)
                {
                    drag.position = next;
                    drag.modifiers = modifiers;
                    return GuideUpdate::Repaint;
                }
            } else if *hover != hit {
                *hover = hit;
                return GuideUpdate::Repaint;
            }
            GuideUpdate::None
        }
        GuideEvent::Release(cursor, modifiers, outside) => {
            let Some(drag) = drag.take() else {
                return GuideUpdate::None;
            };
            let command = match drag.handle {
                Handle::Crop(handle) if drag.moved(cursor) => {
                    GeometryCommand::Crop(handle, drag.update(cursor), modifiers)
                }
                Handle::PatchTransform { selection, part } => {
                    if !drag.moved(cursor) {
                        if part == PatchPart::Body {
                            return GuideUpdate::Edit(GeometryCommand::SelectPatch(selection));
                        }
                        return GuideUpdate::Repaint;
                    }
                    let Some((selection, transform, fitted)) =
                        drag.patch_transform(drag.update(cursor), modifiers)
                    else {
                        return GuideUpdate::Repaint;
                    };
                    GeometryCommand::TransformPatch(selection, transform, fitted)
                }
                _ if outside => return GuideUpdate::Repaint,
                _ if drag.moved(cursor) => GeometryCommand::Move(drag.handle, drag.update(cursor)),
                Handle::Crop(_) => return GuideUpdate::Repaint,
                _ => GeometryCommand::Select(Some(drag.handle.selection())),
            };
            GuideUpdate::Edit(command)
        }
    }
}

impl GraphDocumentView {
    fn compact_patch_picker(&self, window: &Window) -> bool {
        let inspector_width = self.inspector_scroll.bounds().size.width;
        let rem = window.rem_size();
        window.viewport_size().width < rem * 82.
            || (inspector_width > Pixels::ZERO && inspector_width < rem * 17.)
    }

    fn navigate_synthesis_source(
        &mut self,
        target: &Target,
        event: SourceViewEvent,
        cx: &mut Context<Self>,
    ) {
        if !target.current(self) || self.is_busy() {
            return;
        }
        let Some(panel) = self.synthesis.as_mut() else {
            return;
        };
        let editing = panel.drag.is_some() || panel.grid_selection.gesture.is_some();
        if panel.source_navigation.update(event, editing) {
            panel.curve_hover = None;
            cx.notify();
        }
    }

    pub(super) fn synthesis_geometry_command(
        &mut self,
        target: &Target,
        command: GeometryCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(layer) = target.layer else { return };
        if matches!(command, GeometryCommand::TogglePatchTool) {
            let Some(panel) = self.synthesis.as_mut().filter(|panel| panel.drag.is_none()) else {
                return;
            };
            let Some(bounds) = patch_values(&panel.edit, layer, panel.geometry, None) else {
                return;
            };
            panel.patch_transform_mode = !panel.patch_transform_mode;
            panel.patch_numbers = panel.patch_transform_mode.then(|| {
                TransformNumbers::new(
                    bounds,
                    |view: &mut GraphDocumentView, field, value, window, cx| {
                        let Some(panel) = view.synthesis.as_ref() else {
                            return;
                        };
                        let Some(selection) = PatchList::selected(panel.geometry) else {
                            return;
                        };
                        let Some(fitted) = panel.source_navigation.fitted.get() else {
                            return;
                        };
                        let target = Target::new(view, panel);
                        view.synthesis_command(
                            &target,
                            Command::Geometry(GeometryCommand::SetPatchTransformField(
                                selection, field, value, fitted,
                            )),
                            window,
                            cx,
                        );
                    },
                    window,
                    cx,
                )
            });
            panel.curve_hover = None;
            cx.notify();
            return;
        }
        let Some(panel) = self.synthesis.as_ref() else {
            return;
        };
        let source_was_open = matches!(panel.geometry, Some(Patch { .. } | Grid | GridPoint(_)));
        let creating_patch = matches!(command, GeometryCommand::CreatePatch(_));
        let patch_count = panel.edit.patch_counts(layer).map(|(patches, _)| patches);
        let list_changes = matches!(
            command,
            GeometryCommand::AddPatch
                | GeometryCommand::CreatePatch(_)
                | GeometryCommand::RemovePatch(_)
        );
        let preserve_position = if source_was_open
            && self.compact_synthesis_inspector(window)
            && matches!(
                command,
                GeometryCommand::CreatePatch(_)
                    | GeometryCommand::Move(..)
                    | GeometryCommand::MoveGrid(_)
                    | GeometryCommand::TransformPatch(..)
                    | GeometryCommand::SetPatchTransformField(..)
            ) {
            self.synthesis_guide_top
                .get()
                .map(|top| (top, self.inspector_scroll.offset().y))
        } else {
            None
        };
        let mut edit = panel.edit.clone();
        let Some((label, next)) = edit_geometry(
            &mut edit,
            layer,
            panel.geometry,
            command,
            &panel.grid_selection,
            panel
                .guide_source
                .as_ref()
                .and_then(source::Preview::geometry),
        ) else {
            if creating_patch {
                self.error = Some("Couldn’t fit the patch inside this source.".into());
                cx.notify();
            }
            return;
        };
        if let Some(label) = label {
            self.commit_synthesis(target, edit, (Some(layer), next), label, window, cx);
        } else {
            let next = Panel::new(panel.node.clone(), edit, (Some(layer), next), window, cx)
                .keep_material_editor(panel);
            self.install_synthesis_panel(Some(next), window, cx);
            self.refresh_synthesis_guide_source(cx);
            self.observe_drafts(cx);
            cx.notify();
        }
        if list_changes
            && let Some(panel) = self.synthesis.as_ref()
            && panel.layer == Some(layer)
            && panel
                .edit
                .patch_counts(layer)
                .is_some_and(|(count, _)| Some(count) != patch_count)
        {
            if self.compact_patch_picker(window) || PatchList::selected(panel.geometry).is_none() {
                self.focus.focus(window, cx);
            } else {
                panel.patch_list.focus(window, cx);
            }
        }
        if (!source_was_open || preserve_position.is_some())
            && matches!(next, Some(Patch { .. } | Grid | GridPoint(_)))
        {
            let revision = self.revision;
            let selection_serial = self.selection_serial;
            cx.on_next_frame(window, move |view, window, cx| {
                let still_current = move |view: &GraphDocumentView| {
                    view.revision == revision
                        && view.selection_serial == selection_serial
                        && view
                            .synthesis
                            .as_ref()
                            .is_some_and(|panel| panel.geometry == next)
                };
                if !still_current(view) || view.inspector_scroll.max_offset().y <= Pixels::ZERO {
                    return;
                }
                if let Some((previous_top, previous_offset)) = preserve_position {
                    cx.on_next_frame(window, move |view, window, _| {
                        let mut offset = view.inspector_scroll.offset();
                        let Some(top) = view.synthesis_guide_top.get() else {
                            return;
                        };
                        if still_current(view) && offset.y == previous_offset && top != previous_top
                        {
                            offset.y = (offset.y + previous_top - top)
                                .clamp(-view.inspector_scroll.max_offset().y, Pixels::ZERO);
                            view.inspector_scroll.set_offset(offset);
                            window.refresh();
                        }
                    });
                } else {
                    view.synthesis_source_anchor.scroll_to(window, cx);
                    window.on_next_frame(|window, _| window.refresh());
                }
            });
        }
    }
}

fn edit_geometry(
    edit: &mut SynthesisEdit,
    layer: usize,
    current: Option<SynthesisGeometry>,
    command: GeometryCommand,
    grid: &GridSelection,
    source_geometry: Option<ImageGeometry>,
) -> Option<(Option<&'static str>, Option<SynthesisGeometry>)> {
    let (label, next) = match command {
        GeometryCommand::Select(next) => return Some((None, next)),
        GeometryCommand::SelectPatch(selection) => {
            let next =
                current.filter(|current| PatchList::selected(Some(*current)) == Some(selection));
            return Some((None, next.or(Some(Patch { selection, knot: 0 }))));
        }
        GeometryCommand::TogglePatchTool => return None,
        GeometryCommand::Move(geometry, point) => {
            if !geometry.apply(edit, layer, point) {
                return Some((None, Some(geometry.selection())));
            }
            let label = match geometry {
                Handle::Point(_) => "Move Synthesis point",
                Handle::Tangent { .. } => "Move patch handle",
                Handle::Crop(_) | Handle::PatchTransform { .. } => return None,
            };
            (label, Some(geometry.selection()))
        }
        GeometryCommand::MoveGrid(delta) => {
            let changed = delta != [0.; 2]
                && edit.move_repeat_grid_points(layer, &grid.indices, delta, source_geometry);
            return Some((
                changed.then_some("Move Synthesis grid points"),
                Some(grid.geometry()),
            ));
        }
        GeometryCommand::TransformPatch(selection, transform, fitted) => {
            let next = current
                .filter(|current| PatchList::selected(Some(*current)) == Some(selection))
                .unwrap_or(Patch { selection, knot: 0 });
            let changed = edit.transform_patch(layer, selection, transform, Some(fitted));
            return Some((changed.then_some("Transform Synthesis patch"), Some(next)));
        }
        GeometryCommand::SetPatchTransformField(selection, field, value, fitted) => {
            let changed =
                edit.set_patch_transform_field(layer, selection, field, value, Some(fitted));
            return Some((changed.then_some("Transform Synthesis patch"), current));
        }
        GeometryCommand::Crop(handle, delta, modifiers) => {
            let rectangle =
                edit.crop_for_drag(layer, handle, delta, modifiers.shift, modifiers.control)?;
            let changed = edit.set_crop_rectangle(layer, rectangle);
            return Some((changed.then_some("Edit Blend crop"), Some(Crop)));
        }
        GeometryCommand::AddPatch => {
            let selection = edit.add_patch(layer)?;
            ("Add Synthesis patch", Some(Patch { selection, knot: 0 }))
        }
        GeometryCommand::CreatePatch(shape) => {
            let selected = match current {
                Some(Patch { selection, .. }) => Some(selection),
                _ => edit
                    .patch_counts(layer)
                    .filter(|(count, _)| *count != 0)
                    .map(|_| 0),
            };
            let selection = edit.add_patch_shape(layer, shape, selected, source_geometry)?;
            ("Create Synthesis patch", Some(Patch { selection, knot: 0 }))
        }
        GeometryCommand::RemovePatch(selection) => {
            if !edit.remove_patch(layer, selection) {
                return None;
            }
            let next = edit.patch_counts(layer).and_then(|(count, _)| {
                Some(Patch {
                    selection: selection.min(count.checked_sub(1)?),
                    knot: 0,
                })
            });
            ("Remove Synthesis patch", next)
        }
        GeometryCommand::InsertKnot(selection, knot) => {
            let knot = edit.insert_patch_knot(layer, selection, knot)?;
            ("Insert patch point", Some(Patch { selection, knot }))
        }
        GeometryCommand::SplitSegment(selection, segment, t) => {
            let knot = edit.split_patch_segment(layer, selection, segment, t)?;
            ("Insert patch point", Some(Patch { selection, knot }))
        }
        GeometryCommand::RemoveKnot(selection, knot) => {
            if !edit.remove_patch_knot(layer, selection, knot) {
                return None;
            }
            (
                "Remove patch point",
                Some(Patch {
                    selection,
                    knot: knot.saturating_sub(1),
                }),
            )
        }
        GeometryCommand::Flip(selection, axis) => {
            if !edit.flip_patch(layer, selection, axis) {
                return None;
            }
            ("Flip Synthesis patch", current)
        }
        GeometryCommand::KnotMode(selection, knot, mode) => {
            if !edit.set_patch_knot_mode(layer, selection, knot, mode) {
                return None;
            }
            ("Change patch point mode", current)
        }
        GeometryCommand::AddPlacement(selection) => {
            let index = edit.add_placement(layer, selection)?;
            ("Add manual patch placement", Some(Placement(index)))
        }
    };
    Some((Some(label), next))
}

pub(super) fn source_full_size_button(
    panel: &Panel,
    target: &Target,
    disabled: bool,
    cx: &Context<GraphDocumentView>,
) -> Option<Button> {
    let (_, kind) = panel.edit.layer(panel.layer?)?;
    matches!(
        kind,
        SynthesisLayerKind::Patches | SynthesisLayerKind::Repeating | SynthesisLayerKind::Blend
    )
    .then(|| full_size::button(target, disabled || !panel.has_layer_source(), cx))
}

fn patch_creation_controls(
    inline: bool,
    disabled: bool,
    has_raster: bool,
    picker: Option<AnyElement>,
    dispatch: impl Fn(GeometryCommand, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    use SynthesisPatchShape::{Circle, Rectangle, Triangle};
    let [rectangle, circle, triangle, small] = if inline {
        [
            "synthesis-create-rectangle",
            "synthesis-create-circle",
            "synthesis-create-triangle",
            "synthesis-add-patch",
        ]
    } else {
        [
            "synthesis-full-create-rectangle",
            "synthesis-full-create-circle",
            "synthesis-full-create-triangle",
            "synthesis-full-add-patch",
        ]
    };
    let dispatch = Rc::new(dispatch);
    let button = |id, label, tooltip, command, unavailable| {
        let dispatch = dispatch.clone();
        Button::new(id)
            .small()
            .label(label)
            .tooltip(tooltip)
            .tab_index(0)
            .disabled(disabled || unavailable)
            .on_click(move |_, window, cx| dispatch(command, window, cx))
    };
    div()
        .h_flex()
        .flex_wrap()
        .gap_2()
        .child(button(
            rectangle,
            "Rectangle",
            "Create a rectangle patch",
            GeometryCommand::CreatePatch(Rectangle),
            !has_raster,
        ))
        .child(button(
            circle,
            "Circle",
            "Create a circle patch",
            GeometryCommand::CreatePatch(Circle),
            !has_raster,
        ))
        .child(button(
            triangle,
            "Triangle",
            "Create a triangle patch",
            GeometryCommand::CreatePatch(Triangle),
            !has_raster,
        ))
        .child(
            button(
                small,
                "Small rectangle",
                "Create a small centered patch",
                GeometryCommand::AddPatch,
                false,
            )
            .ghost(),
        )
        .children(picker)
}

pub(super) fn render(
    view: &GraphDocumentView,
    panel: &Panel,
    target: &Target,
    disabled: bool,
    busy: bool,
    window: &Window,
    cx: &mut Context<GraphDocumentView>,
) -> Option<AnyElement> {
    let layer = panel.layer?;
    let geometry = panel.geometry;
    let compact_picker = view.compact_patch_picker(window);
    let list_view = || PatchListView {
        inline: true,
        edit: &panel.edit,
        layer,
        selection: geometry,
        disabled,
    };
    let select_patch = {
        let target = target.clone();
        let owner = cx.weak_entity();
        move |selection, window: &mut Window, cx: &mut App| {
            let _ = owner.update(cx, |view, cx| {
                view.synthesis_command(
                    &target,
                    Command::Geometry(GeometryCommand::SelectPatch(selection)),
                    window,
                    cx,
                );
            });
        }
    };
    let select = |next| GeometryCommand::Select(Some(next));
    let action = |id, label, command, inactive| {
        let target = target.clone();
        let owner = cx.weak_entity();
        Button::new(id)
            .small()
            .label(label)
            .disabled(disabled || inactive)
            .on_click(move |_, window, cx| {
                let _ = owner.update(cx, |view, cx| {
                    view.synthesis_command(&target, Command::Geometry(command), window, cx)
                });
            })
    };
    // The toolbar's navigation buttons give keyboard access to every handle.
    let navigate =
        |kind, index: usize, count, previous, next, make: &dyn Fn(usize) -> SynthesisGeometry| {
            div()
                .h_flex()
                .gap_2()
                .flex_wrap()
                .child(
                    div()
                        .text_sm()
                        .child(format!("{kind} {} of {count}", index + 1)),
                )
                .child(action(
                    previous,
                    "Previous",
                    select(make(index.saturating_sub(1))),
                    index == 0,
                ))
                .child(action(
                    next,
                    "Next",
                    select(make(index.saturating_add(1))),
                    index.saturating_add(1) >= count,
                ))
        };
    let actions = || div().h_flex().gap_2().flex_wrap();
    let mut section = div().v_flex().gap_2();
    let mut grid_navigation = None;
    let mut patch_controls = None;

    if let Some((_, points)) = panel.edit.repeat_grid(layer) {
        let open = geometry.is_some();
        section = section.child(
            actions()
                .id("synthesis-repeat-source-tools")
                .anchor_scroll(Some(view.synthesis_source_anchor.clone()))
                .child(action(
                    "synthesis-grid-toggle",
                    if open { "Hide grid" } else { "Edit grid" },
                    GeometryCommand::Select((!open).then_some(Grid)),
                    false,
                ))
                .when(open, |row| {
                    row.child(action(
                        "synthesis-grid-size",
                        "Grid size",
                        select(Grid),
                        geometry == Some(Grid),
                    ))
                    .child(action(
                        "synthesis-grid-point",
                        "Edit point",
                        select(GridPoint(0)),
                        points.is_empty() || matches!(geometry, Some(GridPoint(_))),
                    ))
                }),
        );
        if open {
            let selected = match geometry {
                Some(GridPoint(point)) => Some(point),
                _ => None,
            };
            if let Some(index) = selected {
                grid_navigation = Some(navigate(
                    "Point",
                    index,
                    points.len(),
                    "synthesis-prev-grid-point",
                    "synthesis-next-grid-point",
                    &GridPoint,
                ));
            }
        }
    } else if let Some((patches, _)) = panel.edit.patch_counts(layer) {
        section = section.child(
            actions()
                .child(action(
                    "synthesis-edit-patches",
                    "Edit patches",
                    select(Patch {
                        selection: 0,
                        knot: 0,
                    }),
                    patches == 0 || matches!(geometry, Some(Patch { .. })),
                ))
                .child(manual::open_button(target, disabled, cx)),
        );
        section = section.child(
            div()
                .id("synthesis-patch-source-tools")
                .anchor_scroll(Some(view.synthesis_source_anchor.clone()))
                .child(patch_creation_controls(
                    true,
                    disabled || !panel.has_layer_source(),
                    panel
                        .guide_source
                        .as_ref()
                        .and_then(source::Preview::geometry)
                        .is_some(),
                    (compact_picker && matches!(geometry, Some(Patch { .. }))).then(|| {
                        PatchList::compact_picker(list_view(), select_patch.clone())
                            .into_any_element()
                    }),
                    {
                        let target = target.clone();
                        let owner = cx.weak_entity();
                        move |command, window, cx| {
                            let _ = owner.update(cx, |view, cx| {
                                view.synthesis_command(
                                    &target,
                                    Command::Geometry(command),
                                    window,
                                    cx,
                                );
                            });
                        }
                    },
                )),
        );
        match geometry {
            Some(Patch { selection, knot }) => {
                let points = panel
                    .edit
                    .patch_knots(layer, selection)?
                    .collect::<Vec<_>>();
                let [horizontal, vertical] = panel.edit.patch_flips(layer, selection)?;
                let knot_mode = panel.edit.patch_knot_mode(layer, selection, knot);
                patch_controls = Some(
                    div()
                        .v_flex()
                        .gap_2()
                        .child(navigate(
                            "Patch",
                            selection,
                            patches,
                            "synthesis-prev-patch",
                            "synthesis-next-patch",
                            &|selection| Patch { selection, knot: 0 },
                        ))
                        .child(
                            actions()
                                .child(action(
                                    "synthesis-add-placement",
                                    "Place patch",
                                    GeometryCommand::AddPlacement(selection),
                                    points.len() < 3,
                                ))
                                .child(
                                    action(
                                        "synthesis-flip-x",
                                        "Flip X",
                                        GeometryCommand::Flip(selection, 0),
                                        false,
                                    )
                                    .selected(horizontal),
                                )
                                .child(
                                    action(
                                        "synthesis-flip-y",
                                        "Flip Y",
                                        GeometryCommand::Flip(selection, 1),
                                        false,
                                    )
                                    .selected(vertical),
                                )
                                .child(
                                    action(
                                        "synthesis-remove-patch",
                                        "Remove patch",
                                        GeometryCommand::RemovePatch(selection),
                                        false,
                                    )
                                    .ghost(),
                                ),
                        )
                        .when(
                            !panel.patch_transform_mode && !points.is_empty(),
                            |section| {
                                section
                                    .child(navigate(
                                        "Point",
                                        knot,
                                        points.len(),
                                        "synthesis-prev-patch-point",
                                        "synthesis-next-patch-point",
                                        &|knot| Patch { selection, knot },
                                    ))
                                    .child(
                                        actions()
                                            .child(knot_mode_menu(
                                                knot_mode,
                                                selection,
                                                knot,
                                                target,
                                                &cx.weak_entity(),
                                                disabled,
                                            ))
                                            .child(action(
                                                "synthesis-insert-knot",
                                                "Insert point",
                                                GeometryCommand::InsertKnot(selection, knot),
                                                points.len() < 2,
                                            ))
                                            .child(
                                                action(
                                                    "synthesis-remove-knot",
                                                    "Remove point",
                                                    GeometryCommand::RemoveKnot(selection, knot),
                                                    points.len() <= 3,
                                                )
                                                .ghost(),
                                            ),
                                    )
                            },
                        ),
                );
            }
            _ => {}
        }
    } else if geometry != Some(Crop) {
        return None;
    }
    let raster = panel
        .guide_source
        .as_ref()
        .and_then(|source| match &source.state {
            source::State::Ready(raster) => Some(raster.clone()),
            _ => None,
        });
    let crop = geometry == Some(Crop);
    let crop_unavailable = crop && raster.is_none();
    let navigation = panel.source_guide_layer().is_some();
    let has_raster = raster.is_some();
    let mut fields = geometry.map(|_| {
        view.render_synthesis_fields(
            panel,
            target,
            (
                panel
                    .geometry_fields
                    .iter()
                    .enumerate()
                    .filter(|(_, (field, _))| {
                        !panel.patch_transform_mode
                            || !matches!(geometry, Some(Patch { .. }))
                            || field.number_field().is_some()
                    }),
                "synthesis-geometry-control",
            ),
            (busy || crop_unavailable, disabled || crop_unavailable),
            Form::new(),
            cx,
        )
    });
    let patch_list = (!compact_picker && matches!(geometry, Some(Patch { .. })))
        .then(|| panel.patch_list.render(list_view(), select_patch, cx));
    if let Some(guide) = Guide::from_edit(
        &panel.edit,
        layer,
        geometry,
        panel.drag,
        &panel.grid_selection,
        raster.as_ref().map(|raster| raster.geometry),
        panel.patch_transform_mode,
    )
    .or_else(|| {
        geometry
            .is_none()
            .then_some(Guide {
                handles: Vec::new(),
                lines: Lines::None,
                label: "Patches source image",
            })
            .filter(|_| {
                panel
                    .edit
                    .patch_counts(layer)
                    .is_some_and(|(count, _)| count == 0)
            })
    }) {
        if navigation {
            section = section.child(panel.source_navigation.controls(
                true,
                disabled
                    || !has_raster
                    || panel.drag.is_some()
                    || panel.grid_selection.gesture.is_some(),
                matches!(geometry, Some(Patch { .. })).then(|| {
                    action(
                        "synthesis-patch-tool",
                        if panel.patch_transform_mode {
                            "Edit points"
                        } else {
                            "Transform patch"
                        },
                        GeometryCommand::TogglePatchTool,
                        !has_raster || panel.drag.is_some(),
                    )
                }),
                {
                    let target = target.clone();
                    move |view: &mut GraphDocumentView, event, cx| {
                        view.navigate_synthesis_source(&target, event, cx);
                    }
                },
                cx,
            ));
        }
        let guide = guide.render(
            GuideView {
                selected: geometry,
                drag: panel.drag,
                hover: panel.curve_hover,
                raster,
                full_height: None,
                source_view: (navigation && has_raster).then_some(panel.source_navigation.view),
                panning: panel.source_navigation.pan.is_some(),
                disabled: disabled || crop_unavailable,
                focus: panel.source_focus.clone(),
                fitted_size: panel.source_navigation.fitted.clone(),
            },
            {
                let target = target.clone();
                move |view: &mut GraphDocumentView, event, window, cx| {
                    if let GuideEvent::Navigate(event) = event {
                        view.navigate_synthesis_source(&target, event, cx);
                        return;
                    }
                    if !target.current(view) || view.is_busy() {
                        return;
                    }
                    let Some(panel) = view.synthesis.as_mut().filter(|panel| !panel.pending(cx))
                    else {
                        return;
                    };
                    if panel.source_navigation.pan.is_some() {
                        return;
                    }
                    let points = panel
                        .edit
                        .repeat_grid(layer)
                        .map_or(&[][..], |(_, points)| points);
                    let transforming = panel
                        .drag
                        .is_some_and(|drag| matches!(drag.handle, Handle::PatchTransform { .. }));
                    match guide_update(
                        &mut panel.drag,
                        &mut panel.curve_hover,
                        &mut panel.grid_selection,
                        points,
                        event,
                    ) {
                        GuideUpdate::None => {}
                        GuideUpdate::Repaint => {
                            let still_transforming = panel.drag.is_some_and(|drag| {
                                matches!(drag.handle, Handle::PatchTransform { .. })
                            });
                            if transforming || still_transforming {
                                panel.sync_patch_numbers(window, cx);
                            }
                            if transforming && !still_transforming {
                                view.focus.focus(window, cx);
                            }
                            cx.notify();
                        }
                        GuideUpdate::Edit(command) => {
                            if transforming {
                                view.focus.focus(window, cx);
                            }
                            view.synthesis_command(&target, Command::Geometry(command), window, cx);
                            cx.notify();
                        }
                    }
                }
            },
            cx,
        );
        section = if crop {
            section.child(
                div()
                    .h_flex()
                    .flex_wrap()
                    .gap_2()
                    .child(div().flex_1().min_w(gpui_kit::px(150.)).child(guide))
                    .child(
                        div()
                            .flex_1()
                            .min_w(gpui_kit::px(164.))
                            .children(fields.take()),
                    ),
            )
        } else {
            let top = view.synthesis_guide_top.clone();
            let workspace = if let Some(list) = patch_list {
                div()
                    .h_flex()
                    .h_40()
                    .gap_2()
                    .items_stretch()
                    .child(div().flex_1().min_w_0().child(guide))
                    .child(div().w(rems(7.5)).flex_none().child(list))
                    .into_any_element()
            } else {
                guide.into_any_element()
            };
            section.child(
                div()
                    .on_children_prepainted(move |children, _, _| {
                        top.set(children.first().map(|bounds| bounds.top()));
                    })
                    .child(workspace),
            )
        };
        if matches!(geometry, Some(Patch { .. })) {
            section = section.child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(if panel.patch_transform_mode {
                        "Drag inside to move. Shift locks proportions. Ctrl resizes from the center."
                    } else {
                        "Double-click the outline to insert a point."
                    }),
            );
        }
        if panel.source_guide_layer().is_some() {
            let status = match panel.guide_source.as_ref().map(|source| &source.state) {
                Some(source::State::Loading(_)) => Some("Loading source image…".into()),
                Some(source::State::Failed(error)) => {
                    Some(format!("Couldn’t show source image. {error}"))
                }
                Some(source::State::Ready(_)) => None,
                None => Some("Connect an input or choose a source image.".into()),
            };
            if let Some(status) = status {
                section = section.child(
                    div()
                        .id("synthesis-guide-source-status")
                        .test_support()
                        .role(gpui_kit::Role::Status)
                        .aria_label(status.clone())
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(status),
                );
            }
        }
    }
    Some(
        section
            .children(grid_navigation)
            .children(patch_controls)
            .children(
                panel
                    .patch_numbers
                    .as_ref()
                    .filter(|_| matches!(geometry, Some(Patch { .. })))
                    .map(|numbers| {
                        numbers.render(true, disabled || !has_raster || panel.drag.is_some())
                    }),
            )
            .when(
                panel.edit.patch_counts(layer).is_some() && geometry.is_some(),
                |section| {
                    section.child(
                        action(
                            "synthesis-hide-geometry",
                            "Hide geometry",
                            GeometryCommand::Select(None),
                            false,
                        )
                        .ghost(),
                    )
                },
            )
            .children(fields)
            .into_any_element(),
    )
}

struct Guide {
    handles: Vec<(Handle, [f32; 2])>,
    lines: Lines,
    label: &'static str,
}

struct GuideView {
    selected: Option<SynthesisGeometry>,
    drag: Option<Drag>,
    hover: Option<GuideHover>,
    raster: Option<source::Raster>,
    full_height: Option<Pixels>,
    source_view: Option<SourceView>,
    panning: bool,
    disabled: bool,
    focus: FocusHandle,
    fitted_size: Rc<Cell<Option<ImageGeometry>>>,
}

enum Lines {
    Grid {
        dimensions: [usize; 2],
        boundary: Option<([f32; 4], bool)>,
        selected: Vec<usize>,
        marquee: Option<Bounds<Pixels>>,
    },
    Closed {
        selection: usize,
        outline: Vec<[f32; 2]>,
        other_outlines: Vec<(usize, Vec<[f32; 2]>)>,
        transform: Option<TransformBox>,
    },
    Crop([f32; 4]),
    None,
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum GuideHover {
    Curve(CurveHit),
    Crop(SynthesisCropHandle),
    Patch(PatchPart),
}

#[derive(Clone, Copy, PartialEq)]
pub(super) struct CurveHit {
    segment: usize,
    t: f32,
    position: [f32; 2],
}

impl Lines {
    fn patch_hit(&self, frame: GuideFrame, cursor: Point<Pixels>) -> Option<(Handle, [f32; 2])> {
        let Self::Closed {
            selection,
            outline,
            other_outlines,
            transform,
        } = self
        else {
            return None;
        };
        if let Some(handle) = transform
            .as_ref()
            .and_then(|bounds| bounds.hit(frame, cursor))
        {
            return Some(handle);
        }
        let cursor = [
            (cursor.x - frame.image.left()) / frame.image.size.width,
            (cursor.y - frame.image.top()) / frame.image.size.height,
        ];
        let selection = if patch::contains(outline, cursor) {
            *selection
        } else {
            other_outlines
                .iter()
                .find(|(_, outline)| patch::contains(outline, cursor))?
                .0
        };
        Some((
            Handle::PatchTransform {
                selection,
                part: PatchPart::Body,
            },
            [0.; 2],
        ))
    }

    fn hit(
        &self,
        knots: &[(Handle, [f32; 2])],
        space: Bounds<Pixels>,
        cursor: Point<Pixels>,
    ) -> Option<CurveHit> {
        let Self::Closed { outline, .. } = self else {
            return None;
        };
        let count = knots
            .iter()
            .take_while(|(handle, _)| matches!(handle, Handle::Point(_)))
            .count();
        let samples = outline.len().checked_div(count)?;
        if samples == 0 || !space.contains(&cursor) {
            return None;
        }
        let cursor = [
            (cursor.x - space.left()) / space.size.width,
            (cursor.y - space.top()) / space.size.height,
        ];
        let mut closest = None;
        let mut limit = (gpui_kit::px(6.) / space.size.width).powi(2);
        for (index, &end) in outline.iter().enumerate() {
            let sample = index % samples;
            let segment = index / samples;
            let start = if sample == 0 {
                knots[segment].1
            } else {
                outline[index - 1]
            };
            let [dx, dy] = [end[0] - start[0], end[1] - start[1]];
            let length = dx * dx + dy * dy;
            if length == 0. {
                continue;
            }
            let fraction = (((cursor[0] - start[0]) * dx + (cursor[1] - start[1]) * dy) / length)
                .clamp(0., 1.);
            let t = (sample as f32 + fraction) / samples as f32;
            let position = [start[0] + fraction * dx, start[1] + fraction * dy];
            let distance = (position[0] - cursor[0]).powi(2) + (position[1] - cursor[1]).powi(2);
            if distance < limit && t > 0. && t < 1. {
                limit = distance;
                closest = Some(CurveHit {
                    segment,
                    t,
                    position,
                });
            }
        }
        closest
    }
}

impl Guide {
    fn from_edit(
        edit: &SynthesisEdit,
        layer: usize,
        selected: Option<SynthesisGeometry>,
        drag: Option<Drag>,
        grid: &GridSelection,
        source_geometry: Option<ImageGeometry>,
        transform_mode: bool,
    ) -> Option<Self> {
        Some(match selected? {
            Crop => {
                use SynthesisCropHandle::*;
                let mut rect = edit.crop_rectangle(layer)?;
                if let Some(Drag {
                    handle: Handle::Crop(handle),
                    position,
                    modifiers,
                    ..
                }) = drag
                    && let Some(next) = edit.crop_for_drag(
                        layer,
                        handle,
                        position,
                        modifiers.shift,
                        modifiers.control,
                    )
                {
                    rect = next;
                }
                let [x, y, width, height] = rect;
                let [left, top, right, bottom] = [
                    x.min(x + width),
                    y.min(y + height),
                    x.max(x + width),
                    y.max(y + height),
                ];
                let mid_x = (left + right) / 2.;
                let mid_y = (top + bottom) / 2.;
                let handles = [
                    (NorthWest, [left, top]),
                    (North, [mid_x, top]),
                    (NorthEast, [right, top]),
                    (West, [left, mid_y]),
                    (East, [right, mid_y]),
                    (SouthWest, [left, bottom]),
                    (South, [mid_x, bottom]),
                    (SouthEast, [right, bottom]),
                ]
                .map(|(handle, position)| (Handle::Crop(handle), position));
                Self {
                    handles: handles.into(),
                    lines: Lines::Crop([left, top, right, bottom]),
                    label: "Blend crop over source image",
                }
            }
            Grid | GridPoint(_) => {
                let (dimensions, points) = edit.repeat_grid(layer)?;
                let projected = grid.moving().and_then(|delta| {
                    edit.repeat_grid_for_drag(layer, &grid.indices, delta, source_geometry)
                });
                let visible = projected.as_deref().unwrap_or(points);
                Self {
                    handles: visible
                        .iter()
                        .copied()
                        .enumerate()
                        .map(|(index, point)| (Handle::Point(GridPoint(index)), point))
                        .collect(),
                    lines: Lines::Grid {
                        dimensions,
                        boundary: edit.repeat_guide_bounds(layer, source_geometry, visible),
                        selected: grid.indices.clone(),
                        marquee: grid.marquee(),
                    },
                    label: "Repeating pattern grid",
                }
            }
            Patch { selection, knot } => {
                let transform =
                    drag.and_then(|drag| drag.patch_transform(drag.position, drag.modifiers));
                let selection = transform.map_or(selection, |(selection, ..)| selection);
                let mut preview = transform
                    .and_then(|(selection, command, fitted)| {
                        edit.preview_patch_transform(layer, selection, command, Some(fitted))
                    })
                    .unwrap_or_else(|| edit.clone());
                if let Some(drag) = drag
                    && transform.is_none()
                {
                    drag.handle.apply(&mut preview, layer, drag.position);
                }
                let mut handles: Vec<_> = if transform_mode || transform.is_some() {
                    Vec::new()
                } else {
                    edit.patch_knots(layer, selection)?
                        .enumerate()
                        .map(|(knot, point)| (Handle::Point(Patch { selection, knot }), point))
                        .collect()
                };
                if !handles.is_empty()
                    && let Some([incoming, outgoing]) =
                        preview.patch_handles(layer, selection, knot)
                {
                    handles.extend([(true, incoming), (false, outgoing)].map(
                        |(incoming, point)| {
                            (
                                Handle::Tangent {
                                    selection,
                                    knot,
                                    incoming,
                                },
                                point,
                            )
                        },
                    ));
                }
                let outline = preview.patch_outline(layer, selection).unwrap_or_default();
                let count = edit.patch_counts(layer).map_or(0, |(count, _)| count);
                let other_outlines = (0..count)
                    .filter(|&index| index != selection)
                    .filter_map(|index| {
                        edit.patch_outline(layer, index)
                            .map(|outline| (index, outline))
                    })
                    .collect();
                let transform = transform_mode
                    .then(|| {
                        if let Some((
                            _,
                            mycela_engine::SynthesisPatchTransform::Rotate { pointer, .. },
                            _,
                        )) = transform
                        {
                            let bounds = edit.patch_transform_bounds(layer, selection)?;
                            let [x, y] = [0, 1].map(|axis| pointer[axis] - bounds.pivot[axis]);
                            let angle = if [x, y] == [0.; 2] {
                                0.
                            } else {
                                std::f32::consts::FRAC_PI_2 + y.atan2(x)
                            };
                            Some(TransformBox {
                                selection,
                                bounds,
                                angle,
                            })
                        } else {
                            preview
                                .patch_transform_bounds(layer, selection)
                                .map(|bounds| TransformBox {
                                    selection,
                                    bounds,
                                    angle: 0.,
                                })
                        }
                    })
                    .flatten();
                Self {
                    handles,
                    lines: Lines::Closed {
                        selection,
                        outline,
                        other_outlines,
                        transform,
                    },
                    label: if transform_mode {
                        "Patch transform handles"
                    } else {
                        "Patch selection outline. Double-click its edge to insert a point."
                    },
                }
            }
            Placement(_) => return None,
        })
    }

    fn render<T: 'static>(
        self,
        state: GuideView,
        interaction: impl Fn(&mut T, GuideEvent, &mut Window, &mut Context<T>) + 'static,
        cx: &Context<T>,
    ) -> impl IntoElement {
        let GuideView {
            selected,
            drag,
            hover,
            raster,
            full_height,
            source_view,
            panning,
            disabled,
            focus,
            fitted_size,
        } = state;
        let measured = Rc::new(Cell::new(None));
        let layout = measured.clone();
        let Guide {
            handles,
            lines,
            label,
        } = self;
        let handles = Rc::new(handles);
        let lines = Rc::new(lines);
        let point_hit = {
            let handles = handles.clone();
            move |space, cursor| {
                handles
                    .iter()
                    .copied()
                    .filter_map(|(handle, position)| {
                        if !visible_position(handle, position) {
                            return None;
                        }
                        let delta: Point<Pixels> = project(space, position) - cursor;
                        let distance = delta.x.abs().max(delta.y.abs());
                        (distance <= gpui_kit::px(9.)).then_some((handle, position, distance))
                    })
                    .min_by_key(|&(handle, _, distance)| {
                        (
                            distance,
                            !matches!(handle, Handle::Point(_)),
                            Some(handle.selection()) != selected,
                        )
                    })
                    .map(|(handle, position, _)| (handle, position))
            }
        };
        let curve_hit = {
            let lines = lines.clone();
            let handles = handles.clone();
            let point_hit = point_hit.clone();
            move |space, cursor| {
                if point_hit(space, cursor).is_some() {
                    return None;
                }
                lines.hit(&handles, space, cursor)
            }
        };
        let drag_space = measured.clone();
        let hover_space = measured.clone();
        let move_curve_hit = curve_hit.clone();
        let move_point_hit = point_hit.clone();
        let press_lines = lines.clone();
        let hover_lines = lines.clone();
        let patch = match &*lines {
            Lines::Closed { selection, .. } => Some(*selection),
            _ => None,
        };
        let is_grid = matches!(&*lines, Lines::Grid { .. });
        let crop = match &*lines {
            Lines::Crop(bounds) => Some(*bounds),
            _ => None,
        };
        let interior = move |space: Bounds<Pixels>, cursor| {
            let [left, top, right, bottom] = crop?;
            let origin = project(space, [left, top]);
            let extent = project(space, [right, bottom]) - origin;
            Bounds::new(origin, size(extent.x, extent.y))
                .contains(&cursor)
                .then_some((Handle::Crop(SynthesisCropHandle::Move), [0.; 2]))
        };
        let crop_handle = drag
            .and_then(|drag| match drag.handle {
                Handle::Crop(handle) => Some(handle),
                _ => None,
            })
            .or(match hover {
                Some(GuideHover::Crop(handle)) => Some(handle),
                _ => None,
            });
        let patch_part = drag
            .and_then(|drag| match drag.handle {
                Handle::PatchTransform { part, .. } => Some(part),
                _ => None,
            })
            .or(match hover {
                Some(GuideHover::Patch(part)) => Some(part),
                _ => None,
            });
        let theme = cx.theme();
        let colors = (
            theme.border,
            theme.primary,
            theme.foreground,
            theme.background,
        );
        let aspect = drag.map_or_else(
            || raster.as_ref().map_or(1., |raster| raster.aspect),
            |drag| drag.space.size.width / drag.space.size.height,
        );
        let surface = canvas(
            move |bounds, window, _| {
                let stroke = window.rem_size() / 16.;
                let margin = 16. * stroke;
                let width = bounds.size.width - margin;
                let height = bounds.size.height - margin;
                let fitted = size(width.min(height * aspect), height.min(width / aspect));
                let offset = point(
                    (bounds.size.width - fitted.width) / 2.,
                    (bounds.size.height - fitted.height) / 2.,
                );
                let fitted = Bounds::new(bounds.origin + offset, fitted);
                fitted_size.set(
                    (width > Pixels::ZERO && height > Pixels::ZERO)
                        .then(|| fitted_geometry(fitted)),
                );
                layout.set(
                    (width > Pixels::ZERO && height > Pixels::ZERO).then(|| GuideFrame {
                        viewport: bounds,
                        fitted,
                        image: source_view.map_or(fitted, |view| view.image_bounds(fitted)),
                    }),
                );
                (layout.get(), stroke)
            },
            move |bounds, (frame, stroke), window, _| {
                if let Some(frame) = frame {
                    let space = frame.image;
                    window.with_content_mask(Some(ContentMask { bounds }), |window| {
                        if let Some(raster) = raster.as_ref() {
                            paint_source(space, &lines, raster, colors.3, window);
                        }
                        paint(
                            space,
                            stroke,
                            (&handles, &lines),
                            (selected, drag, hover),
                            colors,
                            window,
                        );
                        if let Lines::Closed {
                            transform: Some(transform),
                            ..
                        } = &*lines
                        {
                            transform.paint(
                                frame,
                                stroke,
                                patch_part,
                                (colors.1, colors.3),
                                window,
                            );
                        }
                    });
                }
            },
        )
        .size_full();
        let interaction = Rc::new(interaction);
        let press = interaction.clone();
        let track = interaction.clone();
        let release = interaction.clone();
        let cancel = interaction.clone();
        let press_focus = focus.clone();
        div()
            .id(if full_height.is_some() {
                "synthesis-full-size-surface"
            } else {
                "synthesis-geometry-surface"
            })
            .test_support()
            .role(gpui_kit::Role::Group)
            .aria_label(label)
            .when(patch.is_some(), |surface| surface.track_focus(&focus))
            .when(
                drag.is_some_and(|drag| matches!(drag.handle, Handle::PatchTransform { .. })),
                |view| {
                    view.on_key_down(cx.listener(
                        move |view, event: &gpui_kit::KeyDownEvent, window, cx| {
                            if event.keystroke.key == "escape" {
                                cx.stop_propagation();
                                cancel(view, GuideEvent::CancelTransform, window, cx);
                            }
                        },
                    ))
                },
            )
            .h_40()
            .when(crop.is_some(), |surface| surface.h(gpui_kit::rems(9.)))
            .when_some(full_height, |surface, height| surface.h(height))
            .w_full()
            .border_1()
            .border_color(theme.border)
            .bg(theme.muted)
            .when_some(crop_handle.filter(|_| !disabled), |surface, handle| {
                surface.cursor(crop_cursor(handle, drag.is_some()))
            })
            .when_some(patch_part.filter(|_| !disabled), |surface, part| {
                surface.cursor(part.cursor(drag.is_some()))
            })
            .when(panning, |surface| surface.cursor(CursorStyle::ClosedHand))
            .child(surface)
            .when(!disabled && source_view.is_some(), |view| {
                let wheel_bounds = measured.clone();
                let zoom = interaction.clone();
                let begin_pan = interaction.clone();
                let end_pan = interaction.clone();
                let end_pan_outside = interaction.clone();
                view.on_scroll_wheel(cx.listener(
                    move |view, event: &ScrollWheelEvent, window, cx| {
                        let Some(GuideFrame { fitted, .. }) = wheel_bounds.get() else {
                            return;
                        };
                        let step = match event.delta {
                            ScrollDelta::Lines(lines) => lines.y.clamp(-1., 1.),
                            ScrollDelta::Pixels(delta) => (delta.y.as_f32() / 60.).clamp(-1., 1.),
                        };
                        if step == 0. {
                            return;
                        }
                        let offset = event.position - fitted.center();
                        cx.stop_propagation();
                        zoom(
                            view,
                            GuideEvent::Navigate(SourceViewEvent::Zoom(
                                2_f32.powf(step),
                                [offset.x / fitted.size.width, offset.y / fitted.size.height],
                            )),
                            window,
                            cx,
                        );
                    },
                ))
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                        if let Some(frame) = measured.get() {
                            begin_pan(
                                view,
                                GuideEvent::Navigate(SourceViewEvent::BeginPan(
                                    event.position,
                                    frame,
                                )),
                                window,
                                cx,
                            );
                        }
                    }),
                )
                .on_mouse_up(
                    MouseButton::Right,
                    cx.listener(move |view, _: &MouseUpEvent, window, cx| {
                        end_pan(
                            view,
                            GuideEvent::Navigate(SourceViewEvent::EndPan),
                            window,
                            cx,
                        );
                    }),
                )
                .on_mouse_up_out(
                    MouseButton::Right,
                    cx.listener(move |view, _: &MouseUpEvent, window, cx| {
                        end_pan_outside(
                            view,
                            GuideEvent::Navigate(SourceViewEvent::EndPan),
                            window,
                            cx,
                        );
                    }),
                )
            })
            .when(!disabled, |view| {
                view.on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                        let Some(frame) = drag_space.get() else {
                            return;
                        };
                        let space = frame.image;
                        if is_grid {
                            let index = match point_hit(space, event.position) {
                                Some((Handle::Point(GridPoint(index)), _)) => Some(index),
                                _ => None,
                            };
                            press(
                                view,
                                GuideEvent::Grid(GridEvent::Begin {
                                    index,
                                    space,
                                    start: event.position,
                                    extend: event.modifiers.shift,
                                }),
                                window,
                                cx,
                            );
                        } else if event.click_count == 2
                            && let Some(selection) = patch
                            && let Some(hit) = curve_hit(space, event.position)
                        {
                            press(
                                view,
                                GuideEvent::Edit(GeometryCommand::SplitSegment(
                                    selection,
                                    hit.segment,
                                    f64::from(hit.t),
                                )),
                                window,
                                cx,
                            );
                        } else if let Some((handle, position)) = point_hit(space, event.position)
                            .or_else(|| interior(space, event.position))
                            .or_else(|| {
                                curve_hit(space, event.position)
                                    .is_none()
                                    .then(|| press_lines.patch_hit(frame, event.position))
                                    .flatten()
                            })
                        {
                            let position = if matches!(handle, Handle::Crop(_)) {
                                [0.; 2]
                            } else {
                                position
                            };
                            if matches!(handle, Handle::PatchTransform { .. }) {
                                press_focus.focus(window, cx);
                            }
                            press(
                                view,
                                GuideEvent::Begin(Drag {
                                    handle,
                                    space,
                                    fitted: frame.fitted,
                                    start: event.position,
                                    original: position,
                                    position,
                                    modifiers: event.modifiers,
                                }),
                                window,
                                cx,
                            );
                        }
                    }),
                )
                .on_mouse_move(
                    cx.listener(move |view, event: &MouseMoveEvent, window, cx| {
                        if source_view.is_some()
                            && (panning || event.pressed_button == Some(MouseButton::Right))
                        {
                            let navigation = if event.pressed_button == Some(MouseButton::Right) {
                                SourceViewEvent::Pan(event.position)
                            } else {
                                SourceViewEvent::EndPan
                            };
                            track(view, GuideEvent::Navigate(navigation), window, cx);
                            return;
                        }
                        if is_grid {
                            track(
                                view,
                                GuideEvent::Grid(GridEvent::Track(event.position)),
                                window,
                                cx,
                            );
                            return;
                        }
                        let hit = hover_space.get().and_then(|frame| {
                            let space = frame.image;
                            if crop.is_some() {
                                let (handle, _) = move_point_hit(space, event.position)
                                    .or_else(|| interior(space, event.position))?;
                                match handle {
                                    Handle::Crop(handle) => Some(GuideHover::Crop(handle)),
                                    _ => None,
                                }
                            } else if patch.is_some() {
                                move_curve_hit(space, event.position)
                                    .map(GuideHover::Curve)
                                    .or_else(|| {
                                        if move_point_hit(space, event.position).is_some() {
                                            return None;
                                        }
                                        let (Handle::PatchTransform { part, .. }, _) =
                                            hover_lines.patch_hit(frame, event.position)?
                                        else {
                                            return None;
                                        };
                                        Some(GuideHover::Patch(part))
                                    })
                            } else {
                                None
                            }
                        });
                        track(
                            view,
                            GuideEvent::Track(event.position, event.modifiers, hit),
                            window,
                            cx,
                        );
                    }),
                )
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(move |view, event: &MouseUpEvent, window, cx| {
                        release(
                            view,
                            if is_grid {
                                GuideEvent::Grid(GridEvent::Release(event.position))
                            } else {
                                GuideEvent::Release(event.position, event.modifiers, false)
                            },
                            window,
                            cx,
                        );
                    }),
                )
                .on_mouse_up_out(
                    MouseButton::Left,
                    cx.listener(move |view, event: &MouseUpEvent, window, cx| {
                        interaction(
                            view,
                            if is_grid {
                                GuideEvent::Grid(GridEvent::Release(event.position))
                            } else {
                                GuideEvent::Release(event.position, event.modifiers, true)
                            },
                            window,
                            cx,
                        );
                    }),
                )
            })
    }
}

fn knot_mode_menu(
    mode: Option<usize>,
    selection: usize,
    knot: usize,
    target: &Target,
    owner: &WeakEntity<GraphDocumentView>,
    disabled: bool,
) -> impl IntoElement {
    let modes = ["Corner", "Linked handles", "Independent handles"];
    let target = target.clone();
    let owner = owner.clone();
    let button = Button::new("synthesis-knot-mode").small().label(
        mode.and_then(|index| modes.get(index))
            .copied()
            .unwrap_or("Point mode"),
    );
    synthesis_menu(button, disabled, move |mut menu, _, _| {
        for (index, name) in modes.into_iter().enumerate() {
            menu = menu.item(menu_item(
                name,
                mode == Some(index),
                Command::Geometry(GeometryCommand::KnotMode(selection, knot, index)),
                &target,
                &owner,
            ));
        }
        menu
    })
}

fn project(bounds: Bounds<Pixels>, [x, y]: [f32; 2]) -> Point<Pixels> {
    point(
        bounds.left() + x * bounds.size.width,
        bounds.top() + y * bounds.size.height,
    )
}

fn visible_position(handle: Handle, position: [f32; 2]) -> bool {
    position.iter().all(|value| {
        value.is_finite()
            && (matches!(handle, Handle::Tangent { .. }) || (0. ..=1.).contains(value))
    })
}

fn rectangle([left, top, right, bottom]: [f32; 4]) -> [[f32; 2]; 4] {
    [[left, top], [right, top], [right, bottom], [left, bottom]]
}

fn paint_source(
    space: Bounds<Pixels>,
    lines: &Lines,
    raster: &source::Raster,
    background: Hsla,
    window: &mut Window,
) {
    window.with_content_mask(Some(ContentMask { bounds: space }), |window| {
        if !raster.images().any(|image| {
            window
                .paint_image(space, space, Default::default(), image, 0, false)
                .is_ok()
        }) {
            return;
        }
        let grid;
        let outline: &[[f32; 2]] = match lines {
            Lines::Closed { outline, .. } => outline,
            Lines::Grid {
                boundary: Some((bounds, _)),
                ..
            } => {
                grid = rectangle(*bounds);
                &grid
            }
            Lines::Crop(bounds) => {
                grid = rectangle(*bounds);
                &grid
            }
            _ => return,
        };
        {
            let mut wash = PathBuilder::fill().with_style(PathStyle::Fill(
                FillOptions::default().with_fill_rule(FillRule::EvenOdd),
            ));
            let frame = [[0., 0.], [1., 0.], [1., 1.], [0., 1.]].map(|p| project(space, p));
            wash.add_polygon(&frame, true);
            let points: Vec<_> = outline
                .iter()
                .copied()
                .filter(|point| point.iter().all(|axis| axis.is_finite()))
                .map(|position| project(space, position))
                .collect();
            if points.len() >= 3 {
                wash.add_polygon(&points, true);
            }
            if let Ok(path) = wash.build() {
                window.paint_path(path, background.opacity(0.5));
            }
        }
    });
}

fn paint(
    space: Bounds<Pixels>,
    stroke: Pixels,
    (handles, lines): (&[(Handle, [f32; 2])], &Lines),
    (selected, drag, hover): (Option<SynthesisGeometry>, Option<Drag>, Option<GuideHover>),
    (border, accent, foreground, background): (Hsla, Hsla, Hsla, Hsla),
    window: &mut Window,
) {
    let point_at = |(handle, original)| {
        drag.filter(|drag| drag.handle == handle && !matches!(handle, Handle::Crop(_)))
            .map_or(original, |drag| drag.position)
    };
    let project = |position| project(space, position);
    let mut path = PathBuilder::stroke(stroke);
    let mut outline = |points: &[[f32; 2]]| {
        let mut projected = points
            .iter()
            .filter(|point| point.iter().all(|axis| axis.is_finite()))
            .copied()
            .map(project);
        if let Some(first) = projected.next() {
            path.move_to(first);
            for point in projected {
                path.line_to(point);
            }
            path.close();
        }
    };
    outline(&[[0., 0.], [1., 0.], [1., 1.], [0., 1.]]);
    if let Ok(path) = path.build() {
        window.paint_path(path, border);
    }

    if let Lines::Closed { other_outlines, .. } = lines {
        let mut path = PathBuilder::stroke(stroke);
        for (_, outline) in other_outlines {
            let points: Vec<_> = outline
                .iter()
                .copied()
                .filter(|point| point.iter().all(|axis| axis.is_finite()))
                .map(project)
                .collect();
            if points.len() >= 3 {
                path.add_polygon(&points, true);
            }
        }
        if let Ok(path) = path.build() {
            window.paint_path(path, foreground.opacity(0.52));
        }
    }
    let mut path = PathBuilder::stroke(stroke);
    match lines {
        Lines::Grid {
            dimensions: [columns, rows],
            ..
        } if *rows > 0 => {
            let mut segment = |first, second| {
                let a = point_at(handles[first]);
                let b = point_at(handles[second]);
                if a.iter().chain(&b).all(|axis| axis.is_finite()) {
                    path.move_to(project(a));
                    path.line_to(project(b));
                }
            };
            for x in 0..*columns {
                for y in 0..*rows {
                    let index = x * rows + y;
                    if y + 1 < *rows {
                        segment(index, index + 1);
                    }
                    if x + 1 < *columns {
                        segment(index, index + rows);
                    }
                }
            }
        }
        Lines::Closed { outline, .. } => {
            let mut projected = outline
                .iter()
                .filter(|point| point.iter().all(|axis| axis.is_finite()))
                .copied()
                .map(project);
            if let Some(first) = projected.next() {
                path.move_to(first);
                for point in projected {
                    path.line_to(point);
                }
                path.close();
            }
        }
        Lines::Crop(bounds) => path.add_polygon(&rectangle(*bounds).map(project), true),
        _ => {}
    }
    if let Ok(path) = path.build() {
        window.paint_path(path, foreground);
    }
    if let Lines::Grid {
        boundary: Some((bounds, true)),
        ..
    } = lines
    {
        let mut boundary = PathBuilder::stroke(1.5 * stroke);
        boundary.add_polygon(&rectangle(*bounds).map(project), true);
        if let Ok(path) = boundary.build() {
            window.paint_path(path, accent);
        }
    }

    let grid_selection = if let Lines::Grid {
        selected, marquee, ..
    } = lines
    {
        if let Some(bounds) = marquee {
            window.paint_quad(fill(*bounds, accent.opacity(0.16)));
            let mut outline = PathBuilder::stroke(stroke);
            outline.add_polygon(
                &[
                    bounds.origin,
                    bounds.top_right(),
                    bounds.bottom_right(),
                    bounds.bottom_left(),
                ],
                true,
            );
            if let Ok(outline) = outline.build() {
                window.paint_path(outline, accent);
            }
        }
        selected.as_slice()
    } else {
        &[]
    };
    let marker = |center, radius| {
        Bounds::new(
            center - point(radius, radius),
            size(2. * radius, 2. * radius),
        )
    };
    let focus = drag
        .map(|drag| drag.handle)
        .or(match hover {
            Some(GuideHover::Crop(handle)) => Some(Handle::Crop(handle)),
            _ => None,
        })
        .or_else(|| selected.map(Handle::Point));
    let anchor = selected.and_then(|selected| {
        handles
            .iter()
            .copied()
            .find(|(handle, _)| *handle == Handle::Point(selected))
    });
    for &(handle, coord) in handles
        .iter()
        .filter(|(handle, _)| Some(*handle) != focus)
        .chain(handles.iter().filter(|(handle, _)| Some(*handle) == focus))
    {
        let active = match handle {
            Handle::Point(GridPoint(index)) => grid_selection.binary_search(&index).is_ok(),
            _ => Some(handle) == focus,
        };
        let position = point_at((handle, coord));
        if !visible_position(handle, position) {
            continue;
        }
        let center = project(position);
        if let Handle::Tangent { .. } = handle {
            if let Some(anchor) = anchor {
                let mut connector = PathBuilder::stroke(stroke);
                connector.move_to(project(point_at(anchor)));
                connector.line_to(center);
                if let Ok(connector) = connector.build() {
                    window.paint_path(connector, accent);
                }
            }
            window.paint_quad(fill(marker(center, 3. * stroke), accent).corner_radii(3. * stroke));
            if !active {
                window.paint_quad(
                    fill(marker(center, 2. * stroke), background).corner_radii(2. * stroke),
                );
            }
            continue;
        }
        let radius = if active { 4. * stroke } else { 3. * stroke };
        window.paint_quad(fill(
            marker(center, radius),
            if active { accent } else { foreground },
        ));
        if !active {
            window.paint_quad(fill(marker(center, radius - stroke), background));
        }
    }
    if let Some(GuideHover::Curve(hover)) = hover {
        let at = project(hover.position);
        window.paint_quad(fill(marker(at, 4. * stroke), background));
        window.paint_quad(fill(marker(at, 2. * stroke), accent));
    }
}
