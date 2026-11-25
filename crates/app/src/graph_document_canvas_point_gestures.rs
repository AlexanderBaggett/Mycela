//! Canvas point selection and drag snapshots.
use super::*;

#[derive(Clone)]
pub(super) enum PointGesture {
    Drag(PointDrag),
    Marquee {
        start: Point<Pixels>,
        position: Point<Pixels>,
        space: TileSpace,
    },
}

#[derive(Clone, Copy)]
struct Hit {
    vertex: Vertex,
    handle: Option<CanvasPathHandle>,
    value: [f32; 2],
}

#[derive(Clone, Copy)]
struct TangentStart {
    vertex: Vertex,
    angle: f64,
    length: f64,
}

#[derive(Clone)]
pub(super) struct PointDrag {
    space: TileSpace,
    hit: Hit,
    start: Point<Pixels>,
    click_deselects: bool,
    vertices: Vec<Vertex>,
    tangents: Vec<TangentStart>,
    collapsed: Vec<(Vertex, f64)>,
    proposed: CanvasPath,
}

impl Points {
    pub(super) fn press(&mut self, space: TileSpace, event: &MouseDownEvent) {
        let hit = self.hit(space, event.position, event.modifiers.shift);
        if let Some(hit) = hit {
            self.segment = None;
            let was_selected = self.selected.contains(&hit.vertex);
            if event.modifiers.shift {
                self.selected.insert(hit.vertex);
                self.active = self.active.or(Some(hit.vertex));
            } else if was_selected {
                self.active = Some(hit.vertex);
            } else {
                self.set_single(Some(hit.vertex));
            }
            let mut vertices: Vec<_> = self.selected_vertices().collect();
            vertices.sort_unstable_by_key(|vertex| (vertex.contour, vertex.point));
            let tangents = if let Some(handle) = hit.handle {
                vertices
                    .iter()
                    .filter_map(|&vertex| {
                        let [x, y] = self.vertex(vertex)?.handle_offset(handle).map(f64::from);
                        Some(TangentStart {
                            vertex,
                            angle: self.direction(vertex, handle),
                            length: x.hypot(y).max(0.01),
                        })
                    })
                    .collect()
            } else {
                Vec::new()
            };
            self.drag = Some(PointGesture::Drag(PointDrag {
                space,
                hit,
                start: event.position,
                click_deselects: event.modifiers.shift && was_selected && hit.handle.is_none(),
                vertices,
                tangents,
                collapsed: Vec::new(),
                proposed: (*self.loaded).clone(),
            }));
            return;
        }
        let sample = space.tile_point(event.position);
        self.segment = self
            .wraps()
            .filter_map(|shift| self.loaded.pick_segment(unshift(sample, shift), 1.))
            .min_by(|a, b| a.distance_squared().total_cmp(&b.distance_squared()));
        if self.segment.is_some() {
            self.set_single(None);
            self.drag = None;
        } else {
            self.drag = Some(PointGesture::Marquee {
                start: event.position,
                position: event.position,
                space,
            });
        }
    }

    fn hit(&self, space: TileSpace, cursor: Point<Pixels>, additive: bool) -> Option<Hit> {
        let mut best: Option<Hit> = None;
        let mut distance = 8. * 8.;
        let mut consider = |hit: Hit, shift| {
            let candidate = distance_squared(space.window_point(shifted(hit.value, shift)), cursor);
            // Shift-click can pick the unselected endpoint of a split whose
            // two new endpoints are still coincident.
            let unselected_tie = additive
                && candidate == distance
                && hit.handle.is_none()
                && !self.selected.contains(&hit.vertex)
                && best.is_some_and(|previous| {
                    previous.handle.is_none() && self.selected.contains(&previous.vertex)
                });
            if candidate < distance || unselected_tie {
                distance = candidate;
                best = Some(hit);
            }
        };
        for shift in self.wraps() {
            for (contour, (closed, points)) in self.loaded.contours().enumerate() {
                for (point, item) in points.iter().enumerate() {
                    let vertex = Vertex { contour, point };
                    consider(
                        Hit {
                            vertex,
                            handle: None,
                            value: item.position(),
                        },
                        shift,
                    );
                    if self.selected.contains(&vertex) {
                        for handle in point_handles(closed, point, points.len()) {
                            consider(
                                Hit {
                                    vertex,
                                    handle: Some(handle),
                                    value: item.handle(handle),
                                },
                                shift,
                            );
                        }
                    }
                }
            }
        }
        best
    }

    pub(super) fn move_gesture(&mut self, event: &MouseMoveEvent) -> bool {
        match &mut self.drag {
            Some(PointGesture::Drag(drag)) => {
                drag.update(&self.loaded, event.position, event.modifiers)
            }
            Some(PointGesture::Marquee { position, .. }) => *position = event.position,
            None => return false,
        }
        true
    }

    pub(super) fn release(&mut self, event: &MouseUpEvent, commit: bool) -> CurveAction {
        let Some(gesture) = self.drag.take().filter(|_| commit) else {
            return CurveAction::None;
        };
        match gesture {
            PointGesture::Drag(mut drag) => {
                if near(event.position, drag.start, px(2.)) {
                    if drag.click_deselects {
                        self.selected.remove(&drag.hit.vertex);
                        if self.active == Some(drag.hit.vertex) {
                            self.active = self.first_selected();
                        }
                    } else if !event.modifiers.shift && drag.hit.handle.is_none() {
                        self.set_single(Some(drag.hit.vertex));
                    }
                    return CurveAction::None;
                }
                drag.update(&self.loaded, event.position, event.modifiers);
                if let Some(handle) = drag.hit.handle {
                    for tangent in &drag.tangents {
                        self.directions.remove(&(tangent.vertex, handle));
                    }
                    for (vertex, angle) in drag.collapsed {
                        self.remember_direction(vertex, handle, [0.; 2], angle);
                    }
                }
                self.change(drag.proposed)
            }
            PointGesture::Marquee { start, space, .. } => {
                self.select_in_box(space, start, event.position, event.modifiers.shift);
                CurveAction::None
            }
        }
    }

    fn select_in_box(
        &mut self,
        space: TileSpace,
        start: Point<Pixels>,
        end: Point<Pixels>,
        additive: bool,
    ) {
        if !additive {
            self.set_single(None);
        }
        if near(start, end, px(4.)) {
            return;
        }
        let bounds = drag_rectangle(start, end);
        for shift in self.wraps() {
            for (contour, (_, points)) in self.loaded.contours().enumerate() {
                for (point, item) in points.iter().enumerate() {
                    let at = space.window_point(shifted(item.position(), shift));
                    if bounds.contains(&at) {
                        self.selected.insert(Vertex { contour, point });
                    }
                }
            }
        }
        self.active = self.active.or_else(|| self.first_selected());
    }

    fn first_selected(&self) -> Option<Vertex> {
        self.selected_vertices()
            .min_by_key(|vertex| (vertex.contour, vertex.point))
    }

    pub(super) fn proposed_path(&self) -> &CanvasPath {
        match &self.drag {
            Some(PointGesture::Drag(drag)) => &drag.proposed,
            _ => &self.loaded,
        }
    }

    pub(super) fn marquee_bounds(&self) -> Option<Bounds<Pixels>> {
        match self.drag {
            Some(PointGesture::Marquee {
                start, position, ..
            }) => Some(drag_rectangle(start, position)),
            _ => None,
        }
    }
}

impl PointDrag {
    fn update(&mut self, loaded: &CanvasPath, position: Point<Pixels>, modifiers: Modifiers) {
        self.proposed.clone_from(loaded);
        self.collapsed.clear();
        if near(position, self.start, px(2.)) {
            return;
        }
        let pointer_delta = unshift(
            self.space.tile_point(position),
            self.space.tile_point(self.start),
        );
        let mut target = shifted(self.hit.value, pointer_delta);
        let alt_only = modifiers
            == Modifiers {
                alt: true,
                ..Default::default()
            };
        let shift_only = modifiers
            == Modifiers {
                shift: true,
                ..Default::default()
            };
        if alt_only {
            target = self.space.1.snap(target);
        }
        if let Some(handle) = self.hit.handle {
            self.move_tangents(loaded, handle, target, modifiers.control, shift_only);
        } else {
            let mut delta = unshift(target, self.hit.value);
            if shift_only {
                let [x, y] = delta.map(f64::from);
                let angle = snap_angle(y.atan2(x).to_degrees(), 0., 90.).to_radians();
                let (sin, cos) = angle.sin_cos();
                let length = x.hypot(y);
                delta = [(cos * length) as f32, (sin * length) as f32];
            }
            self.move_vertices(loaded, delta);
        }
    }

    fn move_vertices(&mut self, loaded: &CanvasPath, delta: [f32; 2]) {
        let active = self.hit.vertex;
        for vertex in std::iter::once(active).chain(
            self.vertices
                .iter()
                .copied()
                .filter(|&vertex| vertex != active),
        ) {
            if let Some(point) = path_vertex(loaded, vertex) {
                self.proposed.set_point_position(
                    vertex.contour,
                    vertex.point,
                    shifted(point.position(), delta),
                );
            }
        }
    }

    fn move_tangents(
        &mut self,
        loaded: &CanvasPath,
        handle: CanvasPathHandle,
        target: [f32; 2],
        lock_angle: bool,
        snap: bool,
    ) {
        let active = self.hit.vertex;
        let Some(point) = path_vertex(loaded, active) else {
            return;
        };
        let Some(anchor) = self.tangents.iter().find(|start| start.vertex == active) else {
            return;
        };
        let [x, y] = unshift(target, point.position()).map(f64::from);
        let length = x.hypot(y);
        let mut angle = -y.atan2(x).to_degrees();
        if lock_angle {
            angle = anchor.angle;
        } else if snap {
            angle = -snap_angle(-angle, -anchor.angle, 15.);
        }
        let rotation = angle - anchor.angle;
        let scale = length / anchor.length;
        for &TangentStart { vertex, .. } in &self.tangents {
            if vertex != active
                && path_vertex(loaded, vertex)
                    .is_some_and(|point| point.kind() == CanvasPointKind::Corner)
            {
                self.proposed.set_point_kind(
                    vertex.contour,
                    vertex.point,
                    CanvasPointKind::Independent,
                    0.,
                );
            }
        }
        for &TangentStart {
            vertex,
            angle,
            length,
        } in &self.tangents
        {
            let direction = angle + rotation;
            let length = length * scale;
            let (sin, cos) = (-direction.to_radians()).sin_cos();
            let offset = [(length * cos) as f32, (length * sin) as f32];
            if self.proposed.set_point_tangent(
                vertex.contour,
                vertex.point,
                handle,
                offset,
                direction,
                false,
            ) && offset == [0.; 2]
            {
                self.collapsed.push((vertex, direction));
            }
        }
    }
}

fn path_vertex(path: &CanvasPath, vertex: Vertex) -> Option<&CanvasPathPoint> {
    path.contours().nth(vertex.contour)?.1.get(vertex.point)
}

fn snap_angle(angle: f64, initial: f64, increment: f64) -> f64 {
    let angle = angle.rem_euclid(360.);
    let mut best = initial.rem_euclid(increment);
    let mut candidate = best + increment;
    while candidate <= 360. {
        if (candidate - angle).abs() < (best - angle).abs() {
            best = candidate;
        }
        candidate += increment;
    }
    best
}
