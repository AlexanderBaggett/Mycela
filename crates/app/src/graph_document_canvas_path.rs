//! Transient pen and control-point gestures for retained Canvas curves.
use super::*;
use mycela_engine::{
    CanvasPathEndpoint, CanvasPathHandle, CanvasPathPoint, CanvasPathSegment, CanvasPointKind,
    CanvasShapeField,
};
use std::sync::Arc;

#[path = "graph_document_canvas_freehand.rs"]
mod freehand;
#[path = "graph_document_canvas_point_controls.rs"]
mod point_controls;
#[path = "graph_document_canvas_point_gestures.rs"]
mod point_gestures;

use freehand::Freehand;
use point_gestures::PointGesture;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CurveDrawingMode {
    Tangents,
    Freehand,
}

const HANDLES: [CanvasPathHandle; 2] = [CanvasPathHandle::Incoming, CanvasPathHandle::Outgoing];
const ENDPOINT_RADIUS: f32 = 9.;
const ANGLE_STEP: f32 = std::f32::consts::PI / 12.;

#[derive(Clone)]
pub(super) enum CurveTool {
    Pen(Pen),
    Freehand(Freehand),
    Points(Points),
}

#[derive(Clone, Default)]
pub(super) struct Pen {
    points: Vec<CanvasPathPoint>,
    pending: Option<PenPress>,
    hover: Option<CanvasPathPoint>,
    source: Option<PenSource>,
    extension: Option<EndpointHit>,
}

#[derive(Clone)]
struct PenSource {
    editor: CanvasShape,
    path: Arc<CanvasPath>,
    shift: [f32; 2],
}

#[derive(Clone, Copy)]
struct EndpointHit {
    endpoint: CanvasPathEndpoint,
    shift: [f32; 2],
}

#[derive(Clone, Copy)]
enum PenTarget {
    Add,
    Start(EndpointHit),
    End(EndpointHit),
    Close,
}

#[derive(Clone)]
struct PenPress {
    space: TileSpace,
    start: Point<Pixels>,
    last: Point<Pixels>,
    tangent_started: bool,
    point: CanvasPathPoint,
    target: PenTarget,
    shift: [f32; 2],
    handle: CanvasPathHandle,
}

#[derive(Clone)]
pub(super) struct Points {
    editor: CanvasShape,
    loaded: Arc<CanvasPath>,
    center_shift: [f32; 2],
    active: Option<Vertex>,
    selected: HashSet<Vertex>,
    segment: Option<CanvasPathSegment>,
    directions: point_controls::TangentDirections,
    drag: Option<PointGesture>,
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
struct Vertex {
    contour: usize,
    point: usize,
}

pub(super) enum CurveAction {
    None,
    Insert(CanvasPath),
    Update(CanvasShape, Arc<CanvasPath>, CanvasPath),
    Remove(CanvasShape, Arc<CanvasPath>),
}

#[derive(Clone, Copy)]
enum PointCommand {
    Insert,
    Split,
    Join,
}

impl CurveTool {
    pub(super) fn is_pen(&self) -> bool {
        matches!(self, Self::Pen(_))
    }

    pub(super) fn drawing_mode(&self) -> Option<CurveDrawingMode> {
        match self {
            Self::Pen(_) => Some(CurveDrawingMode::Tangents),
            Self::Freehand(_) => Some(CurveDrawingMode::Freehand),
            Self::Points(_) => None,
        }
    }

    fn source(&self) -> Option<&PenSource> {
        match self {
            Self::Pen(pen) => pen.source.as_ref(),
            Self::Freehand(freehand) => freehand.source.as_ref(),
            Self::Points(_) => None,
        }
    }

    pub(super) fn can_finish(&self) -> bool {
        matches!(self, Self::Pen(pen) if pen.can_finish())
    }

    pub(super) fn press(&mut self, space: TileSpace, event: &MouseDownEvent, cx: &gpui_kit::App) {
        match self {
            Self::Pen(pen) => pen.press(space, event),
            Self::Freehand(freehand) => freehand.press(space, event, cx),
            Self::Points(points) => points.press(space, event),
        }
    }

    fn moved(&mut self, space: TileSpace, event: &MouseMoveEvent, focused: bool) -> bool {
        let dragging = focused && event.pressed_button == Some(MouseButton::Left);
        match self {
            Self::Pen(pen) => {
                if let Some(pending) = &mut pen.pending
                    && dragging
                {
                    pending.update(event.position, event.modifiers);
                    return true;
                }
                let hover = space
                    .0
                    .contains(&event.position)
                    .then(|| pen.hover_point(space, event.position, event.modifiers));
                let changed = pen.hover != hover;
                pen.hover = hover;
                changed
            }
            Self::Freehand(freehand) => dragging && freehand.moved(event.position),
            Self::Points(points) => dragging && points.move_gesture(event),
        }
    }

    pub(super) fn release(&mut self, event: &MouseUpEvent, commit: bool) -> CurveAction {
        match self {
            Self::Pen(pen) => pen.release(event, commit),
            Self::Freehand(freehand) => freehand.release(event, commit),
            Self::Points(points) => points.release(event, commit),
        }
    }

    pub(super) fn paint(
        &self,
        space: TileSpace,
        stroke: Pixels,
        colors: (Hsla, Hsla),
        window: &mut Window,
    ) {
        let (accent, background) = colors;
        match self {
            Self::Pen(pen) => pen.paint(space, stroke, colors, window),
            Self::Freehand(freehand) => freehand.paint(space, stroke, colors, window),
            Self::Points(points) => {
                let path = points.proposed_path();
                if let Some(bounds) = points.marquee_bounds() {
                    paint_selection_box(bounds, stroke, accent, window);
                }
                for shift in points.wraps() {
                    let project =
                        |[x, y]: [f32; 2]| space.window_point([x + shift[0], y + shift[1]]);
                    for (contour, (closed, vertices)) in path.contours().enumerate() {
                        paint_contour(vertices, closed, project, stroke, colors, window);
                        for (point, vertex) in vertices.iter().enumerate() {
                            let position = project(vertex.position());
                            if points.selected.contains(&Vertex { contour, point }) {
                                paint_tangents(
                                    vertex,
                                    point_handles(closed, point, vertices.len()),
                                    project,
                                    stroke,
                                    colors,
                                    window,
                                );
                                paint_marker(position, 4. * stroke, accent, window);
                            } else {
                                paint_marker(position, 3. * stroke, background, window);
                                paint_marker(position, 2. * stroke, accent, window);
                            }
                        }
                    }
                    if let Some(segment) = points.segment {
                        let position = project(segment.position());
                        paint_marker(position, 5. * stroke, background, window);
                        paint_marker(position, 3. * stroke, accent, window);
                    }
                }
            }
        }
    }
}

impl Pen {
    fn can_finish(&self) -> bool {
        self.pending.is_none() && (self.points.len() >= 2 || self.extension.is_some())
    }

    fn draft_shift(&self) -> [f32; 2] {
        self.extension.map_or([0.; 2], |hit| hit.shift)
    }

    fn target(&self, space: TileSpace, position: Point<Pixels>) -> PenTarget {
        if let Some(source) = self.source.as_ref()
            && (self.points.is_empty() || self.extension.is_some())
            && let Some(hit) =
                source.hit(space, position, self.extension.map(|start| start.endpoint))
        {
            return if self.extension.is_some() {
                PenTarget::End(hit)
            } else {
                PenTarget::Start(hit)
            };
        }
        if self.extension.is_none()
            && self.points.len() >= 2
            && near(
                space.window_point(self.points[0].position()),
                position,
                px(9.),
            )
        {
            return PenTarget::Close;
        }
        PenTarget::Add
    }

    fn target_anchor(&self, target: PenTarget) -> Option<CanvasPathPoint> {
        if matches!(target, PenTarget::Close) {
            return self.points.first().cloned();
        }
        let source = self.source.as_ref()?;
        let (endpoint, reverse) = match target {
            PenTarget::Start(hit) => (
                hit.endpoint,
                matches!(hit.endpoint, CanvasPathEndpoint::Start(_)),
            ),
            PenTarget::End(hit) => (
                hit.endpoint,
                matches!(hit.endpoint, CanvasPathEndpoint::End(_)),
            ),
            _ => return None,
        };
        let point = source.path.endpoint(endpoint)?.clone();
        Some(if reverse { point.reversed() } else { point })
    }

    fn hover_point(
        &self,
        space: TileSpace,
        position: Point<Pixels>,
        modifiers: Modifiers,
    ) -> CanvasPathPoint {
        let target = self.target(space, position);
        if let Some(point) = self.target_anchor(target) {
            return point;
        }
        CanvasPathPoint::corner(self.vertex_position(
            unshift(space.tile_point(position), self.draft_shift()),
            modifiers,
            space.1,
        ))
    }

    fn press(&mut self, space: TileSpace, event: &MouseDownEvent) {
        let target = self.target(space, event.position);
        let shift = match target {
            PenTarget::Start(hit) | PenTarget::End(hit) => hit.shift,
            _ => self.draft_shift(),
        };
        let point = self.target_anchor(target).unwrap_or_else(|| {
            CanvasPathPoint::corner(self.vertex_position(
                unshift(space.tile_point(event.position), shift),
                event.modifiers,
                space.1,
            ))
        });
        let draw_incoming = match target {
            PenTarget::Start(hit) => matches!(hit.endpoint, CanvasPathEndpoint::Start(_)),
            PenTarget::End(hit) => matches!(hit.endpoint, CanvasPathEndpoint::End(_)),
            PenTarget::Add => self
                .extension
                .is_some_and(|hit| matches!(hit.endpoint, CanvasPathEndpoint::Start(_))),
            PenTarget::Close => false,
        };
        self.pending = Some(PenPress {
            space,
            start: event.position,
            last: event.position,
            tangent_started: false,
            point,
            target,
            shift,
            handle: if draw_incoming {
                CanvasPathHandle::Incoming
            } else {
                CanvasPathHandle::Outgoing
            },
        });
    }

    fn release(&mut self, event: &MouseUpEvent, commit: bool) -> CurveAction {
        let Some(mut pending) = self.pending.take().filter(|_| commit) else {
            return CurveAction::None;
        };
        self.hover = None;
        pending.update(event.position, event.modifiers);
        match pending.target {
            PenTarget::Start(hit) => {
                self.extension = Some(hit);
                self.points.push(pending.point);
                CurveAction::None
            }
            PenTarget::End(hit) => self.finish_extension(Some((hit.endpoint, pending.point))),
            PenTarget::Add => {
                self.points.push(pending.point);
                CurveAction::None
            }
            PenTarget::Close => {
                self.points[0] = pending.point;
                self.finish_new(true)
            }
        }
    }

    pub(super) fn refresh(&mut self) {
        let Some(source) = &self.source else {
            return;
        };
        let updated = PenSource::new(source.editor.clone());
        if updated
            .as_ref()
            .is_none_or(|updated| updated.path != source.path)
        {
            self.cancel();
        }
        self.source = updated;
    }

    fn cancel(&mut self) {
        self.points.clear();
        self.pending = None;
        self.hover = None;
        self.extension = None;
    }

    fn paint(&self, space: TileSpace, stroke: Pixels, colors: (Hsla, Hsla), window: &mut Window) {
        let (accent, background) = colors;
        if let Some(source) = &self.source
            && (self.extension.is_some() || self.points.is_empty())
        {
            source.paint_endpoints(space, stroke, colors, window);
        }
        let mut vertices = self.points.clone();
        let close_requested = self
            .pending
            .as_ref()
            .is_some_and(|press| matches!(press.target, PenTarget::Close));
        let close_hovered = self.extension.is_none()
            && self.pending.is_none()
            && self.points.len() >= 2
            && self.hover.as_ref() == self.points.first();
        let closing = close_requested || close_hovered;
        if let Some(press) = &self.pending {
            if closing {
                vertices[0] = press.point.clone();
            } else {
                vertices.push(press.point.clone());
            }
        } else if let Some(hover) = &self.hover
            && !closing
        {
            vertices.push(hover.clone());
        }

        let shift = self.draft_shift();
        let project = |position| space.window_point(shifted(position, shift));
        if let Some(hit) = self.extension
            && let Some(source) = &self.source
        {
            for copy in wrapped_shifts(unshift([0.; 2], shift)) {
                paint_contour(
                    &vertices,
                    false,
                    |p| space.window_point(shifted(p, copy)),
                    stroke,
                    colors,
                    window,
                );
            }
            if let Some(anchor) = source.path.endpoint(hit.endpoint) {
                paint_marker(project(anchor.position()), 4. * stroke, accent, window);
            }
        } else {
            paint_contour(&vertices, closing, project, stroke, colors, window);
            if let Some(first) = self.points.first() {
                paint_marker(project(first.position()), 4. * stroke, accent, window);
            }
        }
        if let Some(press) = &self.pending {
            paint_tangents(
                &press.point,
                HANDLES,
                |p| space.window_point(shifted(p, press.shift)),
                stroke,
                (accent, background),
                window,
            );
        }
    }

    fn vertex_position(
        &self,
        position: [f32; 2],
        modifiers: Modifiers,
        grid: CanvasSnapGrid,
    ) -> [f32; 2] {
        if modifiers.alt {
            grid.snap(position)
        } else if modifiers.shift
            && let Some(previous) = self.points.last()
        {
            snap_angle(previous.position(), position)
        } else {
            position
        }
    }

    fn finish(&mut self) -> CurveAction {
        if self.extension.is_some() {
            self.finish_extension(None)
        } else {
            self.finish_new(false)
        }
    }

    fn finish_new(&mut self, closed: bool) -> CurveAction {
        if self.points.len() < 2 {
            return CurveAction::None;
        }
        self.hover = None;
        CanvasPath::from_points(std::mem::take(&mut self.points), closed)
            .map_or(CurveAction::None, CurveAction::Insert)
    }

    fn finish_extension(
        &mut self,
        arrival: Option<(CanvasPathEndpoint, CanvasPathPoint)>,
    ) -> CurveAction {
        let Some(hit) = self.extension.take() else {
            return CurveAction::None;
        };
        self.hover = None;
        let points = std::mem::take(&mut self.points);
        let Some(source) = self.source.as_ref() else {
            return CurveAction::None;
        };
        let mut proposed = (*source.path).clone();
        if proposed.extend_open(hit.endpoint, points, arrival) && proposed != *source.path {
            CurveAction::Update(source.editor.clone(), source.path.clone(), proposed)
        } else {
            CurveAction::None
        }
    }
}

impl PenSource {
    fn new(editor: CanvasShape) -> Option<Self> {
        let path = Arc::new(editor.path()?);
        Some(Self {
            shift: center_shift(&editor)?,
            editor,
            path,
        })
    }

    fn endpoints(&self) -> impl Iterator<Item = (CanvasPathEndpoint, &CanvasPathPoint)> {
        self.path.contours().enumerate().flat_map(|(index, _)| {
            [
                CanvasPathEndpoint::Start(index),
                CanvasPathEndpoint::End(index),
            ]
            .into_iter()
            .filter_map(|end| Some((end, self.path.endpoint(end)?)))
        })
    }

    fn hit(
        &self,
        space: TileSpace,
        position: Point<Pixels>,
        exclude: Option<CanvasPathEndpoint>,
    ) -> Option<EndpointHit> {
        let mut best = None;
        let mut distance = ENDPOINT_RADIUS.powi(2);
        for (endpoint, vertex) in self
            .endpoints()
            .filter(|(endpoint, _)| Some(*endpoint) != exclude)
        {
            for shift in wrapped_shifts(self.shift) {
                let projected = space.window_point(shifted(vertex.position(), shift));
                let candidate = distance_squared(projected, position);
                if candidate < distance {
                    best = Some(EndpointHit { endpoint, shift });
                    distance = candidate;
                }
            }
        }
        best
    }

    fn paint_endpoints(
        &self,
        space: TileSpace,
        stroke: Pixels,
        (accent, background): (Hsla, Hsla),
        window: &mut Window,
    ) {
        for (_, vertex) in self.endpoints() {
            for shift in wrapped_shifts(self.shift) {
                let vertex = space.window_point(shifted(vertex.position(), shift));
                if space.0.contains(&vertex) {
                    paint_marker(vertex, 5. * stroke, background, window);
                    paint_marker(vertex, 3. * stroke, accent, window);
                }
            }
        }
    }
}

impl PenPress {
    fn update(&mut self, position: Point<Pixels>, modifiers: Modifiers) {
        if !self.tangent_started && near(position, self.start, px(3.)) {
            return;
        }
        self.tangent_started = true;
        if position == self.last {
            return;
        }
        let mut end = unshift(self.space.tile_point(position), self.shift);
        let origin = self.point.position();
        if modifiers.control {
            let previous = unshift(self.space.tile_point(self.last), self.shift);
            self.point.move_to(std::array::from_fn(|axis| {
                origin[axis] + end[axis] - previous[axis]
            }));
        } else if modifiers.alt {
            self.point.move_handle(self.handle, end, true);
        } else {
            if modifiers.shift {
                end = snap_angle(origin, end);
            }
            let opposite = std::array::from_fn(|axis| 2. * origin[axis] - end[axis]);
            let (incoming, outgoing) = match self.handle {
                CanvasPathHandle::Incoming => (end, opposite),
                CanvasPathHandle::Outgoing => (opposite, end),
            };
            self.point
                .move_handle(CanvasPathHandle::Incoming, incoming, true);
            self.point
                .move_handle(CanvasPathHandle::Outgoing, outgoing, false);
        }
        self.last = position;
    }
}

impl Points {
    fn new(editor: CanvasShape) -> Option<Self> {
        let path = editor.path()?;
        Some(Self {
            center_shift: center_shift(&editor)?,
            editor,
            loaded: Arc::new(path),
            active: None,
            selected: HashSet::new(),
            segment: None,
            directions: HashMap::new(),
            drag: None,
        })
    }

    pub(super) fn refresh(&mut self) -> bool {
        self.drag = None;
        let Some(path) = self.editor.path() else {
            return false;
        };
        let Some(shift) = center_shift(&self.editor) else {
            return false;
        };
        if *self.loaded != path {
            let count = |(closed, points): (bool, &[CanvasPathPoint])| (closed, points.len());
            if !self
                .loaded
                .contours()
                .map(count)
                .eq(path.contours().map(count))
            {
                self.directions.clear();
                let active = self.active.filter(|vertex| {
                    path.contours()
                        .nth(vertex.contour)
                        .is_some_and(|(_, points)| vertex.point < points.len())
                });
                self.set_single(active);
            }
            self.loaded = Arc::new(path);
            self.segment = None;
        }
        self.center_shift = shift;
        true
    }

    fn wraps(&self) -> impl Iterator<Item = [f32; 2]> + use<> {
        wrapped_shifts(self.center_shift)
    }

    fn set_single(&mut self, vertex: Option<Vertex>) {
        self.active = vertex;
        self.selected.clear();
        self.selected.extend(vertex);
    }

    fn selected_vertices(&self) -> impl Iterator<Item = Vertex> + '_ {
        self.selected.iter().copied()
    }

    fn change(&self, proposed: CanvasPath) -> CurveAction {
        if proposed == *self.loaded {
            CurveAction::None
        } else {
            CurveAction::Update(self.editor.clone(), self.loaded.clone(), proposed)
        }
    }

    fn delete(&mut self) -> CurveAction {
        let mut proposed = (*self.loaded).clone();
        let vertices: Vec<_> = self
            .selected_vertices()
            .map(|vertex| (vertex.contour, vertex.point))
            .collect();
        if !proposed.delete_points(&vertices) {
            return CurveAction::None;
        }
        self.set_single(None);
        if proposed.contours().next().is_none() {
            CurveAction::Remove(self.editor.clone(), self.loaded.clone())
        } else {
            self.change(proposed)
        }
    }

    fn selected_endpoints(&self) -> Option<[CanvasPathEndpoint; 2]> {
        if self.selected.len() != 2 {
            return None;
        }
        let a = self.active?;
        let b = self.selected_vertices().find(|&vertex| vertex != a)?;
        Some([a.endpoint(&self.loaded)?, b.endpoint(&self.loaded)?])
    }

    fn can_apply(&self, command: PointCommand) -> bool {
        self.drag.is_none()
            && match command {
                PointCommand::Insert => self.segment.is_some(),
                PointCommand::Split => {
                    self.segment.is_some()
                        || (self.selected.len() == 1
                            && self.active.is_some_and(|vertex| {
                                self.loaded.can_split_at_point(vertex.contour, vertex.point)
                            }))
                }
                PointCommand::Join => self
                    .selected_endpoints()
                    .is_some_and(|[a, b]| self.loaded.can_join_endpoints(a, b)),
            }
    }

    fn apply(&mut self, command: PointCommand) -> CurveAction {
        if !self.can_apply(command) {
            return CurveAction::None;
        }
        let mut proposed = (*self.loaded).clone();
        let mut inserted = None;
        let changed = match command {
            PointCommand::Insert => {
                inserted = self.segment.and_then(|segment| {
                    let point = proposed.insert_on_segment(segment)?;
                    Some(Vertex {
                        contour: segment.contour(),
                        point,
                    })
                });
                inserted.is_some()
            }
            PointCommand::Split => {
                if let Some(segment) = self.segment {
                    proposed.split_on_segment(segment)
                } else {
                    self.active
                        .is_some_and(|vertex| proposed.split_at_point(vertex.contour, vertex.point))
                }
            }
            PointCommand::Join => self
                .selected_endpoints()
                .is_some_and(|[a, b]| proposed.join_endpoints(a, b)),
        };
        if changed {
            self.set_single(inserted);
            self.segment = None;
            self.change(proposed)
        } else {
            CurveAction::None
        }
    }
}

impl Vertex {
    fn endpoint(self, path: &CanvasPath) -> Option<CanvasPathEndpoint> {
        let (closed, points) = path.contours().nth(self.contour)?;
        if closed || points.len() < 2 {
            None
        } else if self.point == 0 {
            Some(CanvasPathEndpoint::Start(self.contour))
        } else if self.point + 1 == points.len() {
            Some(CanvasPathEndpoint::End(self.contour))
        } else {
            None
        }
    }
}

impl PointCommand {
    const ALL: [Self; 3] = [Self::Insert, Self::Split, Self::Join];

    fn button(self, segment: bool) -> (&'static str, &'static str) {
        match self {
            Self::Insert => ("texture-canvas-insert-point", "Add point"),
            Self::Split => (
                "texture-canvas-split-point",
                if segment {
                    "Split at segment"
                } else {
                    "Split at point"
                },
            ),
            Self::Join => ("texture-canvas-join-endpoints", "Join endpoints"),
        }
    }
}

fn shifted(position: [f32; 2], shift: [f32; 2]) -> [f32; 2] {
    std::array::from_fn(|axis| position[axis] + shift[axis])
}

fn unshift(position: [f32; 2], shift: [f32; 2]) -> [f32; 2] {
    std::array::from_fn(|axis| position[axis] - shift[axis])
}

fn wrapped_shifts(center_shift: [f32; 2]) -> impl Iterator<Item = [f32; 2]> {
    [-TILE, 0., TILE]
        .into_iter()
        .flat_map(move |x| [-TILE, 0., TILE].map(|y| unshift([x, y], center_shift)))
}

fn center_shift(editor: &CanvasShape) -> Option<[f32; 2]> {
    let values = editor.settings().ok()?;
    Some(
        [
            values.number(CanvasShapeField::CenterX)?,
            values.number(CanvasShapeField::CenterY)?,
        ]
        .map(|value| (value / TILE).floor() * TILE),
    )
}

fn point_handles(
    closed: bool,
    index: usize,
    count: usize,
) -> impl Iterator<Item = CanvasPathHandle> {
    HANDLES.into_iter().filter(move |handle| {
        closed
            || match handle {
                CanvasPathHandle::Incoming => index > 0,
                CanvasPathHandle::Outgoing => index + 1 < count,
            }
    })
}

fn snap_angle(origin: [f32; 2], end: [f32; 2]) -> [f32; 2] {
    let [x, y] = [end[0] - origin[0], end[1] - origin[1]];
    let angle = (y.atan2(x) / ANGLE_STEP).round() * ANGLE_STEP;
    let radius = x.hypot(y);
    [
        origin[0] + radius * angle.cos(),
        origin[1] + radius * angle.sin(),
    ]
}

fn distance_squared(a: Point<Pixels>, b: Point<Pixels>) -> f32 {
    let x = (a.x - b.x) / px(1.);
    let y = (a.y - b.y) / px(1.);
    x * x + y * y
}

fn near(a: Point<Pixels>, b: Point<Pixels>, radius: Pixels) -> bool {
    distance_squared(a, b) < (radius / px(1.)).powi(2)
}

fn paint_contour(
    points: &[CanvasPathPoint],
    closed: bool,
    project: impl Fn([f32; 2]) -> Point<Pixels>,
    stroke: Pixels,
    (accent, background): (Hsla, Hsla),
    window: &mut Window,
) {
    if points.len() < 2 {
        return;
    }
    for (width, color) in [(3. * stroke, background), (stroke, accent)] {
        let mut path = PathBuilder::stroke(width);
        path.move_to(project(points[0].position()));
        let next = points.iter().cycle().skip(1);
        for (start, end) in points
            .iter()
            .zip(next)
            .take(points.len() - usize::from(!closed))
        {
            if start.kind() == CanvasPointKind::Corner && end.kind() == CanvasPointKind::Corner {
                path.line_to(project(end.position()));
            } else {
                path.cubic_bezier_to(
                    project(end.position()),
                    project(start.handle(CanvasPathHandle::Outgoing)),
                    project(end.handle(CanvasPathHandle::Incoming)),
                );
            }
        }
        if let Ok(path) = path.build() {
            window.paint_path(path, color);
        }
    }
}

fn paint_marker(position: Point<Pixels>, half: Pixels, color: Hsla, window: &mut Window) {
    window.paint_quad(fill(
        Bounds::new(
            point(position.x - half, position.y - half),
            size(2. * half, 2. * half),
        ),
        color,
    ));
}

fn paint_tangents(
    vertex: &CanvasPathPoint,
    handles: impl IntoIterator<Item = CanvasPathHandle>,
    project: impl Fn([f32; 2]) -> Point<Pixels>,
    stroke: Pixels,
    (accent, background): (Hsla, Hsla),
    window: &mut Window,
) {
    let origin = project(vertex.position());
    for handle in handles {
        let end = project(vertex.handle(handle));
        if end == origin {
            continue;
        }
        let mut path = PathBuilder::stroke(stroke);
        path.move_to(origin);
        path.line_to(end);
        if let Ok(path) = path.build() {
            window.paint_path(path, accent);
        }
        paint_marker(end, 3. * stroke, accent, window);
        paint_marker(end, 2. * stroke, background, window);
    }
}

impl GraphDocumentView {
    pub(super) fn choose_canvas_curve(
        &mut self,
        target: &Node,
        mode: CurveDrawingMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_busy()
            || self.inspector.is_changed(cx)
            || !self
                .selected_node()
                .is_some_and(|node| node.same_reference(target))
        {
            return;
        }
        if self
            .drawing
            .as_ref()
            .and_then(|tool| tool.curve.as_ref())
            .and_then(CurveTool::drawing_mode)
            == Some(mode)
        {
            self.drawing_focus.focus(window, cx);
            return;
        }
        let before = self
            .drawing
            .as_ref()
            .and_then(|tool| tool.selection.active_shape());
        let source = self
            .drawing
            .as_ref()
            .and_then(DrawingTool::single_selection)
            .and_then(PenSource::new)
            .or_else(|| self.drawing.as_ref()?.curve.as_ref()?.source().cloned());
        self.choose_canvas_tool(target, None, window, cx);
        self.choose_canvas_shape(None, window, cx);
        if let Some(tool) = &mut self.drawing {
            if let Some(shape) = before {
                tool.destination = Destination::Before(shape);
            }
            tool.handles = None;
            tool.curve = Some(match mode {
                CurveDrawingMode::Tangents => CurveTool::Pen(Pen {
                    source,
                    ..Pen::default()
                }),
                CurveDrawingMode::Freehand => CurveTool::Freehand(Freehand::new(source, cx)),
            });
        }
        cx.notify();
    }

    pub(super) fn finish_canvas_curve(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.drawing_current(cx)
            && let Some(CurveTool::Pen(pen)) =
                self.drawing.as_mut().and_then(|tool| tool.curve.as_mut())
            && pen.pending.is_none()
        {
            let action = pen.finish();
            self.commit_curve_action(action, window, cx);
            self.drawing_focus.focus(window, cx);
            cx.notify();
        }
    }

    pub(super) fn move_canvas_curve(
        &mut self,
        space: TileSpace,
        event: &MouseMoveEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let current = !self.preview_pending && self.drawing_current(cx);
        let Some(curve) = self.drawing.as_mut().and_then(|tool| tool.curve.as_mut()) else {
            return false;
        };
        if current && curve.moved(space, event, self.drawing_focus.is_focused(window)) {
            cx.notify();
        }
        true
    }

    pub(super) fn cancel_canvas_curve(&mut self) -> bool {
        let Some(tool) = &mut self.drawing else {
            return false;
        };
        match tool.curve.as_mut() {
            Some(CurveTool::Pen(pen)) if !pen.points.is_empty() || pen.pending.is_some() => {
                pen.cancel();
                true
            }
            Some(CurveTool::Freehand(freehand)) => freehand.cancel(),
            Some(CurveTool::Points(points)) if points.drag.is_some() => {
                points.drag = None;
                true
            }
            Some(CurveTool::Points(_)) => {
                tool.curve = None;
                true
            }
            _ => false,
        }
    }

    pub(super) fn delete_canvas_point(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let current = self.drawing_current(cx);
        let Some(CurveTool::Points(points)) =
            self.drawing.as_mut().and_then(|tool| tool.curve.as_mut())
        else {
            return false;
        };
        if current && points.drag.is_none() {
            let action = points.delete();
            self.commit_curve_action(action, window, cx);
        }
        true
    }

    pub(super) fn commit_curve_action(
        &mut self,
        action: CurveAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            CurveAction::None => {}
            CurveAction::Insert(path) => {
                if let Some(tool) = &self.drawing {
                    self.finish_shape_placement(
                        Placement::Curve(path),
                        tool.destination.clone(),
                        window,
                        cx,
                    );
                }
            }
            CurveAction::Remove(editor, expected) => {
                self.apply_canvas_shape_edit(
                    false,
                    |_| editor.edit_remove_curve(&expected),
                    window,
                    cx,
                );
            }
            CurveAction::Update(editor, expected, proposed) => {
                self.apply_canvas_shape_change(
                    false,
                    |_| editor.edit_path(&expected, proposed),
                    window,
                    cx,
                );
            }
        }
    }

    fn edit_canvas_point_topology(
        &mut self,
        command: PointCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.drawing_current(cx)
            && let Some(CurveTool::Points(points)) =
                self.drawing.as_mut().and_then(|tool| tool.curve.as_mut())
        {
            let action = points.apply(command);
            self.commit_curve_action(action, window, cx);
            self.drawing_focus.focus(window, cx);
        }
    }

    pub(super) fn render_point_tools(
        &self,
        tool: &DrawingTool,
        busy: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let editing = matches!(&tool.curve, Some(CurveTool::Points(_)));
        let count = match &tool.curve {
            Some(CurveTool::Points(points)) if points.drag.is_none() => points.selected.len(),
            _ => 0,
        };
        let disabled = busy || self.inspector.is_changed(cx);
        div()
            .h_flex()
            .flex_wrap()
            .gap_2()
            .child(
                Button::new("texture-canvas-edit-points")
                    .small()
                    .selected(editing)
                    .label(if editing {
                        "Done editing points"
                    } else {
                        "Edit points"
                    })
                    .disabled(disabled)
                    .on_click(cx.listener(|view, _, window, cx| {
                        if !view.drawing_current(cx) {
                            return;
                        }
                        if let Some(tool) = &mut view.drawing {
                            if matches!(tool.curve, Some(CurveTool::Points(_))) {
                                tool.curve = None;
                            } else if let Some(points) =
                                tool.single_selection().and_then(Points::new)
                            {
                                tool.cancel_drag();
                                tool.curve = Some(CurveTool::Points(points));
                            }
                            view.drawing_focus.focus(window, cx);
                            cx.notify();
                        }
                    })),
            )
            .when(editing, |row| {
                row.children(PointCommand::ALL.into_iter().map(|command| {
                    let segment = matches!(&tool.curve, Some(CurveTool::Points(points)) if points.segment.is_some());
                    let (id, label) = command.button(segment);
                    let enabled = matches!(&tool.curve, Some(CurveTool::Points(points)) if points.can_apply(command));
                    Button::new(id).small().ghost().label(label).disabled(disabled || !enabled)
                        .on_click(cx.listener(move |view, _, window, cx| view.edit_canvas_point_topology(command, window, cx)))
                })).child(
                    Button::new("texture-canvas-delete-point")
                        .small()
                        .ghost()
                        .label(if count == 1 { "Delete point" } else { "Delete points" })
                        .disabled(disabled || count == 0)
                        .on_click(cx.listener(|view, _, window, cx| {
                            view.delete_canvas_point(window, cx);
                            view.drawing_focus.focus(window, cx);
                        })),
                )
            })
            .when_some(tool.curve.as_ref().and_then(|curve| match curve {
                CurveTool::Points(points) if points.active.is_some() => Some(points),
                _ => None,
            }), |row, points| row.child(self.render_canvas_point_properties(points, disabled, cx)))
    }
}
