//! Authoring helpers for the same contour and point records used by GTX.
use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CanvasPointKind {
    Corner,
    Smooth,
    Independent,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CanvasPathHandle {
    Incoming,
    Outgoing,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CanvasPathEndpoint {
    Start(usize),
    End(usize),
}

impl CanvasPathEndpoint {
    fn contour(self) -> usize {
        match self {
            Self::Start(index) | Self::End(index) => index,
        }
    }
}

enum Connection {
    Bridge(CanvasPathEndpoint, CanvasPathPoint),
    Weld(CanvasPathEndpoint),
}

impl Connection {
    fn endpoint(&self) -> CanvasPathEndpoint {
        match self {
            Self::Bridge(endpoint, _) | Self::Weld(endpoint) => *endpoint,
        }
    }
}

impl CanvasPathHandle {
    pub(super) fn offset(self) -> usize {
        match self {
            Self::Incoming => 2,
            Self::Outgoing => 4,
        }
    }
}

impl CanvasPathPoint {
    pub fn corner(position: [f32; 2]) -> Self {
        Self::new(position, position, position, 0)
    }

    pub fn smooth(position: [f32; 2], outgoing: [f32; 2]) -> Self {
        let incoming = std::array::from_fn(|axis| 2. * position[axis] - outgoing[axis]);
        Self::new(position, incoming, outgoing, 1)
    }

    pub fn position(&self) -> [f32; 2] {
        [self.coordinates[0], self.coordinates[1]]
    }

    pub fn kind(&self) -> CanvasPointKind {
        match self.mode {
            0 => CanvasPointKind::Corner,
            1 => CanvasPointKind::Smooth,
            _ => CanvasPointKind::Independent,
        }
    }

    pub fn handle(&self, handle: CanvasPathHandle) -> [f32; 2] {
        let offset = self.handle_offset(handle);
        std::array::from_fn(|axis| self.coordinates[axis] + offset[axis])
    }

    pub fn handle_offset(&self, handle: CanvasPathHandle) -> [f32; 2] {
        let at = handle.offset();
        [self.coordinates[at], self.coordinates[at + 1]]
    }

    pub fn move_to(&mut self, position: [f32; 2]) {
        self.coordinates[..2].copy_from_slice(&position);
    }

    pub fn reversed(mut self) -> Self {
        self.swap_handles();
        self
    }

    fn swap_handles(&mut self) {
        self.coordinates.swap(2, 4);
        self.coordinates.swap(3, 5);
    }

    pub(super) fn valid(&self) -> bool {
        self.coordinates.iter().all(|value| value.is_finite())
    }

    /// An independent handle leaves the opposite tangent alone. A smooth handle
    /// follows the moved handle's direction and keeps its own previous length.
    pub fn move_handle(&mut self, handle: CanvasPathHandle, position: [f32; 2], independent: bool) {
        let at = handle.offset();
        let [x, y] = self.position();
        let delta = [position[0] - x, position[1] - y];
        self.coordinates[at..at + 2].copy_from_slice(&delta);
        if independent {
            self.mode = 2;
        } else {
            let opposite = 6 - at;
            let length = self.coordinates[opposite].hypot(self.coordinates[opposite + 1]);
            let distance = delta[0].hypot(delta[1]);
            if distance != 0. {
                for (axis, axis_delta) in delta.into_iter().enumerate() {
                    self.coordinates[opposite + axis] = -axis_delta / distance * length;
                }
            }
            self.mode = 1;
        }
    }
}

impl CanvasPath {
    pub fn from_points(mut points: Vec<CanvasPathPoint>, closed: bool) -> Option<Self> {
        if points.len() < 2 {
            return None;
        }
        // A drawn endpoint keeps its tangents when the drawing operation completes.
        if points[0].mode == 1 || closed {
            points[0].mode = 2;
        }
        if !closed
            && let Some(last) = points.last_mut()
            && last.mode == 1
        {
            last.mode = 2;
        }
        let path = Self {
            width: 3.,
            contours: vec![Contour { closed, points }],
        };
        path.valid().then_some(path)
    }

    pub fn stroke_width(&self) -> f32 {
        self.width
    }

    pub fn contours(&self) -> impl Iterator<Item = (bool, &[CanvasPathPoint])> {
        self.contours
            .iter()
            .map(|contour| (contour.closed, contour.points.as_slice()))
    }

    pub fn point_mut(&mut self, contour: usize, point: usize) -> Option<&mut CanvasPathPoint> {
        self.contours.get_mut(contour)?.points.get_mut(point)
    }

    pub fn remove_point(&mut self, contour: usize, point: usize) -> bool {
        let Some(contour) = self.contours.get_mut(contour) else {
            return false;
        };
        if contour.points.len() <= 2 || point >= contour.points.len() {
            return false;
        }
        contour.points.remove(point);
        true
    }

    pub fn endpoint(&self, endpoint: CanvasPathEndpoint) -> Option<&CanvasPathPoint> {
        let contour = self.contours.get(endpoint.contour())?;
        if contour.closed || contour.points.len() < 2 {
            return None;
        }
        match endpoint {
            CanvasPathEndpoint::Start(_) => contour.points.first(),
            CanvasPathEndpoint::End(_) => contour.points.last(),
        }
    }

    /// Adds vertices to an open contour and optionally connects its new end to an
    /// existing endpoint in this shape. Tangents are supplied in drawing order.
    pub fn extend_open(
        &mut self,
        from: CanvasPathEndpoint,
        mut points: Vec<CanvasPathPoint>,
        target: Option<(CanvasPathEndpoint, CanvasPathPoint)>,
    ) -> bool {
        if points.is_empty()
            || !points.iter().all(CanvasPathPoint::valid)
            || target.as_ref().is_some_and(|(end, point)| {
                *end == from || self.endpoint(*end).is_none() || !point.valid()
            })
            || self.endpoint(from).is_none()
        {
            return false;
        }
        if target.is_none()
            && let Some(last) = points.last_mut()
            && last.mode == 1
        {
            last.mode = 2;
        }
        self.splice_open(
            from,
            points,
            target.map(|(end, point)| Connection::Bridge(end, point)),
        );
        true
    }

    pub fn can_join_endpoints(&self, from: CanvasPathEndpoint, to: CanvasPathEndpoint) -> bool {
        from != to
            && self.endpoint(from).is_some()
            && self.endpoint(to).is_some()
            && (from.contour() != to.contour() || self.contours[from.contour()].points.len() >= 3)
    }

    /// Welds two endpoints in the same shape at their midpoint.
    pub fn join_endpoints(&mut self, from: CanvasPathEndpoint, to: CanvasPathEndpoint) -> bool {
        if !self.can_join_endpoints(from, to) {
            return false;
        }
        let source = &self.contours[from.contour()].points;
        let target = &self.contours[to.contour()].points;
        let (a, previous) = match from {
            CanvasPathEndpoint::Start(_) => (source[0].clone().reversed(), &source[1]),
            CanvasPathEndpoint::End(_) => {
                (source[source.len() - 1].clone(), &source[source.len() - 2])
            }
        };
        let (b, next) = match to {
            CanvasPathEndpoint::Start(_) => (target[0].clone(), &target[1]),
            CanvasPathEndpoint::End(_) => (
                target[target.len() - 1].clone().reversed(),
                &target[target.len() - 2],
            ),
        };
        let [x, y] = a.position();
        let [u, v] = b.position();
        let midpoint = [0.5 * x + 0.5 * u, 0.5 * y + 0.5 * v];
        let weld = if previous.mode == 0 && next.mode == 0 {
            CanvasPathPoint::corner(midpoint)
        } else {
            CanvasPathPoint::new(
                midpoint,
                a.handle(CanvasPathHandle::Incoming),
                b.handle(CanvasPathHandle::Outgoing),
                2,
            )
        };
        self.splice_open(from, vec![weld], Some(Connection::Weld(to)));
        true
    }

    fn splice_open(
        &mut self,
        from: CanvasPathEndpoint,
        extension: Vec<CanvasPathPoint>,
        connection: Option<Connection>,
    ) {
        let from_index = from.contour();
        let mut points = std::mem::take(&mut self.contours[from_index].points);
        let reverse_source = matches!(from, CanvasPathEndpoint::Start(_));
        let reverse_result = match &connection {
            Some(Connection::Weld(target)) => matches!(target, CanvasPathEndpoint::End(_)),
            _ => reverse_source,
        };
        if reverse_source {
            reverse_points(&mut points);
        }
        points.pop();
        points.extend(extension);
        let mut closed = false;
        let mut removed = None;
        if let Some(connection) = connection {
            let end = connection.endpoint();
            let index = end.contour();
            if index == from_index {
                closed = true;
                match connection {
                    Connection::Bridge(_, arrival) => points[0] = arrival,
                    Connection::Weld(_) => {
                        points.remove(0);
                    }
                }
            } else {
                let mut following = std::mem::take(&mut self.contours[index].points);
                if matches!(end, CanvasPathEndpoint::End(_)) {
                    reverse_points(&mut following);
                }
                match connection {
                    Connection::Bridge(_, arrival) => following[0] = arrival,
                    Connection::Weld(_) => {
                        following.remove(0);
                    }
                }
                points.extend(following);
                removed = Some(index);
            }
        }
        if reverse_result {
            reverse_points(&mut points);
        }
        self.contours[from_index] = Contour { closed, points };
        if let Some(index) = removed {
            self.contours.remove(index);
        }
    }

    pub fn can_split_at_point(&self, contour: usize, point: usize) -> bool {
        self.contours.get(contour).is_some_and(|contour| {
            let count = contour.points.len();
            count >= 2 && point < count && (contour.closed || (point > 0 && point + 1 < count))
        })
    }

    /// Breaks an interior vertex into two coincident endpoints.
    pub fn split_at_point(&mut self, contour: usize, point: usize) -> bool {
        if !self.can_split_at_point(contour, point) {
            return false;
        }
        let selected = &mut self.contours[contour];
        let count = selected.points.len();
        let previous = (point + count - 1) % count;
        let next = (point + 1) % count;
        let mut duplicate = selected.points[point].clone();
        if duplicate.mode != 0 {
            selected.points[previous].mode = 2;
            selected.points[next].mode = 2;
        }
        duplicate.mode = if selected.points[previous].mode == 0 && selected.points[next].mode == 0 {
            0
        } else {
            2
        };
        self.break_contour(contour, point, duplicate.clone(), duplicate);
        true
    }

    pub(super) fn break_contour(
        &mut self,
        contour: usize,
        point: usize,
        ending: CanvasPathPoint,
        beginning: CanvasPathPoint,
    ) {
        let selected = &mut self.contours[contour];
        if selected.closed {
            selected.points.rotate_left(point);
            selected.points[0] = beginning;
            selected.points.push(ending);
            selected.closed = false;
        } else {
            let mut following = selected.points.split_off(point);
            following[0] = beginning;
            selected.points.push(ending);
            self.contours.insert(
                contour + 1,
                Contour {
                    closed: false,
                    points: following,
                },
            );
        }
    }

    pub(in crate::pointwise::canvas) fn valid(&self) -> bool {
        self.width.is_finite()
            && self.width >= 0.
            && !self.contours.is_empty()
            && self.contours.iter().all(|contour| {
                contour.points.len() >= 2 && contour.points.iter().all(CanvasPathPoint::valid)
            })
    }
}

pub(super) fn reverse_points(points: &mut [CanvasPathPoint]) {
    points.reverse();
    for point in points {
        point.swap_handles();
    }
}

impl CanvasShape {
    pub(in crate::pointwise::canvas) fn from_canvas_path<E>(
        path: CanvasPath,
    ) -> Result<Self, PointwiseError<E>> {
        Self::from_canvas_path_with_mode(path, 6)
    }

    pub(in crate::pointwise::canvas) fn from_canvas_path_with_mode<E>(
        path: CanvasPath,
        combine: i32,
    ) -> Result<Self, PointwiseError<E>> {
        let mut shape = Shape {
            identity: Arc::new(()),
            header: ItemHeader::new("Curve"),
            kind: 2,
            base: ShapeBase {
                segments: Vec::new(),
                combine,
                transform: [0.; 7],
                controls: Vec::new(),
            },
            details: ShapeDetails::Path(path),
        };
        shape.refresh_path_bounds()?;
        Ok(Self::Shape(Box::new(shape)))
    }
}

impl Shape {
    pub(in crate::pointwise::canvas) fn combine_mode(&self) -> i32 {
        self.base.combine
    }

    pub(in crate::pointwise::canvas) fn path(&self) -> Option<&CanvasPath> {
        match &self.details {
            ShapeDetails::Path(path) => Some(path),
            _ => None,
        }
    }

    pub(in crate::pointwise::canvas) fn open_stroke_width(&self) -> Option<f32> {
        self.path()
            .filter(|path| path.is_open())
            .map(CanvasPath::stroke_width)
    }

    pub(in crate::pointwise::canvas) fn set_path_width(&mut self, width: f32) {
        if let ShapeDetails::Path(path) = &mut self.details {
            path.width = width;
        }
    }

    pub(in crate::pointwise::canvas) fn edit_path_points<E>(
        &mut self,
        path: CanvasPath,
    ) -> Result<(), PointwiseError<E>> {
        self.details = ShapeDetails::Path(path);
        self.refresh_path_bounds()
    }

    fn refresh_path_bounds<E>(&mut self) -> Result<(), PointwiseError<E>> {
        if !self.path().is_some_and(CanvasPath::valid) {
            return Err(PointwiseError::Argument);
        }
        let [low, high] = self.document_bounds()?;
        let [w, h] = [high[0] - low[0], high[1] - low[1]];
        self.base.transform = [
            low[0] + 0.5 * w,
            low[1] + 0.5 * h,
            w.max(0.01),
            h.max(0.01),
            0.,
            0.,
            0.,
        ];
        Ok(())
    }
}
