//! Point and tangent edits that update the adjacent automatic Corner handles.
use super::*;
use std::collections::HashSet;

impl CanvasPath {
    /// Deletes selected vertices together. An empty result means the owning
    /// Curve shape should be removed by the document command.
    pub fn delete_points(&mut self, vertices: &[(usize, usize)]) -> bool {
        if vertices.is_empty()
            || vertices.iter().any(|&(contour, point)| {
                self.contours
                    .get(contour)
                    .is_none_or(|path| point >= path.points.len())
            })
        {
            return false;
        }
        let selected: HashSet<_> = vertices.iter().copied().collect();
        for (index, contour) in self.contours.iter_mut().enumerate() {
            if (0..contour.points.len()).any(|point| selected.contains(&(index, point))) {
                contour.delete_points(|point| selected.contains(&(index, point)));
            }
        }
        self.contours.retain(|contour| contour.points.len() >= 2);
        true
    }

    pub fn can_set_point_kind(&self, contour: usize, point: usize, kind: CanvasPointKind) -> bool {
        self.contours.get(contour).is_some_and(|contour| {
            point < contour.points.len()
                && (kind != CanvasPointKind::Smooth
                    || contour.closed
                    || (point > 0 && point + 1 < contour.points.len()))
        })
    }

    pub fn set_point_kind(
        &mut self,
        contour: usize,
        point: usize,
        kind: CanvasPointKind,
        incoming_angle_if_zero: f64,
    ) -> bool {
        if !incoming_angle_if_zero.is_finite() || !self.can_set_point_kind(contour, point, kind) {
            return false;
        }
        let contour = &mut self.contours[contour];
        let vertex = &mut contour.points[point];
        match kind {
            CanvasPointKind::Corner => {
                vertex.mode = 0;
                contour.refresh_corner(point);
            }
            CanvasPointKind::Smooth => {
                vertex.mode = 1;
                vertex.align_opposite(CanvasPathHandle::Incoming, incoming_angle_if_zero);
            }
            CanvasPointKind::Independent => {
                vertex.mode = 2;
                return true;
            }
        }
        contour.refresh_neighbours(point, true, true);
        true
    }

    pub fn set_point_position(&mut self, contour: usize, point: usize, position: [f32; 2]) -> bool {
        let Some(contour) = self.contours.get_mut(contour) else {
            return false;
        };
        let Some(vertex) = contour.points.get_mut(point) else {
            return false;
        };
        if !position.into_iter().all(f32::is_finite) {
            return false;
        }
        if position == vertex.position() {
            return true;
        }
        vertex.move_to(position);
        contour.refresh_corner(point);
        contour.refresh_neighbours(point, true, true);
        true
    }

    /// Sets a connected tangent using vertex-relative coordinates. A collapsed
    /// smooth tangent can use its remembered angle to align the other side.
    pub fn set_point_tangent(
        &mut self,
        contour: usize,
        point: usize,
        handle: CanvasPathHandle,
        offset: [f32; 2],
        angle_if_zero: f64,
        detach: bool,
    ) -> bool {
        let Some(contour) = self.contours.get_mut(contour) else {
            return false;
        };
        contour.set_tangent(point, handle, offset, angle_if_zero, detach)
    }
}

impl Contour {
    fn set_tangent(
        &mut self,
        point: usize,
        handle: CanvasPathHandle,
        offset: [f32; 2],
        angle_if_zero: f64,
        detach: bool,
    ) -> bool {
        if !offset.into_iter().all(f32::is_finite)
            || !angle_if_zero.is_finite()
            || self.neighbour(point, handle).is_none()
        {
            return false;
        }
        let vertex = &mut self.points[point];
        let smooth = vertex.kind() == CanvasPointKind::Smooth && !detach;
        vertex.set_offset(handle, offset);
        if smooth {
            vertex.align_opposite(handle, angle_if_zero);
        } else {
            vertex.mode = 2;
        }
        self.refresh_neighbours(
            point,
            smooth || handle == CanvasPathHandle::Incoming,
            smooth || handle == CanvasPathHandle::Outgoing,
        );
        true
    }

    pub(super) fn prepare_curve_refit(&mut self) {
        for point in 0..self.points.len() {
            self.refresh_corner(point);
        }
    }

    pub(super) fn restore_collapsed_tangents(&mut self) {
        for point in 0..self.points.len() {
            for (handle, opposite) in [
                (CanvasPathHandle::Incoming, CanvasPathHandle::Outgoing),
                (CanvasPathHandle::Outgoing, CanvasPathHandle::Incoming),
            ] {
                let Some(neighbour) = self.neighbour(point, handle) else {
                    continue;
                };
                let position = self.points[point].position();
                if self.points[point]
                    .handle(handle)
                    .into_iter()
                    .zip(position)
                    .any(|(value, center)| (f64::from(value) - f64::from(center)).abs() > 0.0001)
                {
                    continue;
                }
                let near = self.points[neighbour].handle(opposite);
                let [x, y] = std::array::from_fn(|axis| -(near[axis] - position[axis]) / 4.);
                let [_, old_y] = self.points[point].handle_offset(handle);
                // The source sets each axis separately, so neighbouring
                // Corner handles are refreshed on both writes.
                self.set_tangent(point, handle, [x, old_y], 0., false);
                self.set_tangent(point, handle, [x, y], 0., false);
            }
        }
    }
}

impl CanvasPathPoint {
    fn set_offset(&mut self, handle: CanvasPathHandle, offset: [f32; 2]) {
        let index = handle.offset();
        self.coordinates[index..index + 2].copy_from_slice(&offset);
    }

    fn align_opposite(&mut self, handle: CanvasPathHandle, angle_if_zero: f64) {
        let [x, y] = self.handle_offset(handle).map(f64::from);
        let direction = if x == 0. && y == 0. {
            -angle_if_zero.to_radians()
        } else {
            y.atan2(x)
        };
        let opposite = match handle {
            CanvasPathHandle::Incoming => CanvasPathHandle::Outgoing,
            CanvasPathHandle::Outgoing => CanvasPathHandle::Incoming,
        };
        let [x, y] = self.handle_offset(opposite).map(f64::from);
        let length = (x * x + y * y).sqrt();
        let (sin, cos) = (direction + std::f64::consts::PI).sin_cos();
        self.set_offset(opposite, [(cos * length) as f32, (sin * length) as f32]);
    }
}

impl Contour {
    fn delete_points(&mut self, is_selected: impl Fn(usize) -> bool) {
        let mut links: Vec<_> = (0..self.points.len())
            .map(|index| self.neighbours(index))
            .collect();
        for index in (0..links.len()).filter(|&index| is_selected(index)) {
            let [previous, next] = links[index];
            if let Some(previous) = previous {
                links[previous][1] = None;
            }
            if let Some(next) = next {
                links[next][0] = None;
            }
            if let (Some(previous), Some(next)) = (previous, next)
                && previous != next
                && links[previous][0] != Some(next)
            {
                links[previous][1] = Some(next);
                links[next][0] = Some(previous);
                // Recompute at every join. A subsequent cut can leave one of
                // these newly computed handles stored at a loose endpoint.
                self.refresh_corner_with_neighbours(previous, links[previous]);
                self.refresh_corner_with_neighbours(next, links[next]);
            }
        }
        let mut index = 0;
        let mut kept = 0;
        let mut start = None;
        self.points.retain(|_| {
            let retain = !is_selected(index);
            if retain {
                if links[index][0].is_none() {
                    start = Some(kept);
                }
                kept += 1;
            }
            index += 1;
            retain
        });
        if let Some(start) = start {
            self.points.rotate_left(start);
            self.closed = false;
        }
    }

    fn neighbour(&self, index: usize, direction: CanvasPathHandle) -> Option<usize> {
        let count = self.points.len();
        if count < 2 || index >= count {
            return None;
        }
        match direction {
            CanvasPathHandle::Incoming if index > 0 || self.closed => {
                Some((index + count - 1) % count)
            }
            CanvasPathHandle::Outgoing if index + 1 < count || self.closed => {
                Some((index + 1) % count)
            }
            _ => None,
        }
    }

    fn refresh_neighbours(&mut self, point: usize, previous: bool, next: bool) {
        if previous && let Some(index) = self.neighbour(point, CanvasPathHandle::Incoming) {
            self.refresh_corner(index);
        }
        if next && let Some(index) = self.neighbour(point, CanvasPathHandle::Outgoing) {
            self.refresh_corner(index);
        }
    }

    fn refresh_corner(&mut self, index: usize) {
        self.refresh_corner_with_neighbours(index, self.neighbours(index));
    }

    fn neighbours(&self, index: usize) -> [Option<usize>; 2] {
        [CanvasPathHandle::Incoming, CanvasPathHandle::Outgoing]
            .map(|handle| self.neighbour(index, handle))
    }

    fn refresh_corner_with_neighbours(&mut self, index: usize, neighbours: [Option<usize>; 2]) {
        if self.points[index].kind() != CanvasPointKind::Corner {
            return;
        }
        let at = self.points[index].position();
        for (handle, other) in [CanvasPathHandle::Incoming, CanvasPathHandle::Outgoing]
            .into_iter()
            .zip(neighbours)
        {
            let Some(other) = other else {
                continue;
            };
            let neighbour = &self.points[other];
            let there = neighbour.position();
            let target = if neighbour.kind() == CanvasPointKind::Corner || there == at {
                std::array::from_fn(|axis| at[axis] - 0.25 * (at[axis] - there[axis]))
            } else {
                let far_handle = match handle {
                    CanvasPathHandle::Incoming => CanvasPathHandle::Outgoing,
                    CanvasPathHandle::Outgoing => CanvasPathHandle::Incoming,
                };
                // From the Corner outwards, the nearest handle is at the
                // corner. The far handle comes from the adjacent vertex.
                let points = [at, at, neighbour.handle(far_handle), there]
                    .map(|point| point.map(|value| quantize_point(f64::from(value))));
                split_cubic(points, 0.25)[0][3].map(|axis| axis as f32)
            };
            let offset = std::array::from_fn(|axis| target[axis] - at[axis]);
            self.points[index].set_offset(handle, offset);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn points(record: &str) -> Vec<CanvasPathPoint> {
        record
            .split(';')
            .map(|point| {
                let (values, mode) = point.rsplit_once(',').unwrap();
                let coordinates = values
                    .split(',')
                    .map(|value| value.parse::<f32>().unwrap())
                    .collect::<Vec<_>>()
                    .try_into()
                    .unwrap();
                CanvasPathPoint {
                    coordinates,
                    mode: mode.parse().unwrap(),
                    flags: [false; 3],
                }
            })
            .collect()
    }

    #[test]
    fn canvas_batch_point_deletion_matches_genetica() {
        fn contours(record: &str) -> Vec<Contour> {
            if record == "removed" {
                return Vec::new();
            }
            record
                .split('|')
                .map(|contour| {
                    let (kind, coordinates) = contour.split_once(':').unwrap();
                    let points = coordinates
                        .split(';')
                        .map(|point| format!("{point},0"))
                        .collect::<Vec<_>>()
                        .join(";");
                    Contour {
                        closed: kind == "c",
                        points: self::points(&points),
                    }
                })
                .collect()
        }

        let capture =
            include_str!("../../../../../../../port/evidence/canvas-point-batch-original.tsv");
        for row in capture.lines().skip(2) {
            let [case, input, indices, output]: [&str; 4] =
                row.split('\t').collect::<Vec<_>>().try_into().unwrap();
            let mut path = CanvasPath {
                width: 3.,
                contours: contours(input),
            };
            let vertices: Vec<_> = path
                .contours()
                .enumerate()
                .flat_map(|(contour, (_, points))| {
                    (0..points.len()).map(move |point| (contour, point))
                })
                .collect();
            let selected: Vec<_> = indices
                .split(',')
                .map(|index| vertices[index.parse::<usize>().unwrap()])
                .collect();
            assert!(path.delete_points(&selected));
            assert_eq!(path.contours, contours(output), "{case}");
        }
    }

    #[test]
    fn canvas_numeric_point_controls_match_genetica() {
        let capture =
            include_str!("../../../../../../../port/evidence/canvas-point-controls-original.tsv");
        for row in capture.lines().skip(2) {
            let [case, input, edits, output]: [&str; 4] =
                row.split('\t').collect::<Vec<_>>().try_into().unwrap();
            let mut path = CanvasPath::from_points(points(input), false).unwrap();
            let mut hints = [0.; 2];
            for step in edits.split(';') {
                let (field, value) = step.split_once(',').unwrap();
                let value = value.parse::<f32>().unwrap();
                let point = &path.contours().next().unwrap().1[1];
                match field {
                    "mode" => {
                        let kind = [
                            CanvasPointKind::Corner,
                            CanvasPointKind::Smooth,
                            CanvasPointKind::Independent,
                        ][value as usize];
                        assert!(path.set_point_kind(0, 1, kind, hints[0]));
                    }
                    "x" | "y" => {
                        let mut position = point.position();
                        position[usize::from(field == "y")] = value;
                        assert!(path.set_point_position(0, 1, position));
                    }
                    _ => {
                        let (side, field) = field.split_once('-').unwrap();
                        let side = usize::from(side == "out");
                        let mut offset = point.handle_offset(
                            [CanvasPathHandle::Incoming, CanvasPathHandle::Outgoing][side],
                        );
                        let [x, y] = offset.map(f64::from);
                        let length = x.hypot(y);
                        let angle = if field == "angle" {
                            f64::from(value)
                        } else if length == 0. {
                            hints[side]
                        } else {
                            -y.atan2(x).to_degrees()
                        };
                        if field == "length" || field == "angle" {
                            let length = if field == "length" {
                                f64::from(value)
                            } else {
                                length
                            };
                            let (sin, cos) = (-angle.to_radians()).sin_cos();
                            offset = [(length * cos) as f32, (length * sin) as f32];
                        } else {
                            offset[usize::from(field == "y")] = value;
                        }
                        hints[side] = angle;
                        assert!(path.set_point_tangent(
                            0,
                            1,
                            [CanvasPathHandle::Incoming, CanvasPathHandle::Outgoing][side],
                            offset,
                            angle,
                            false
                        ));
                    }
                }
            }
            let actual = path.contours().next().unwrap().1;
            let expected = points(output);
            assert_eq!(actual.len(), expected.len(), "{case}");
            for (actual, expected) in actual.iter().zip(expected) {
                assert_eq!(actual.kind(), expected.kind(), "{case}");
                for (x, y) in actual.coordinates.into_iter().zip(expected.coordinates) {
                    assert!((x - y).abs() <= 0.00015, "{case}: {x} != {y}");
                }
            }
        }
        let path = CanvasPath::from_points(
            vec![
                CanvasPathPoint::corner([20., 30.]),
                CanvasPathPoint::corner([30., 40.]),
            ],
            false,
        )
        .unwrap();
        assert!(!path.can_set_point_kind(0, 0, CanvasPointKind::Smooth));
        assert!(path.can_set_point_kind(0, 0, CanvasPointKind::Independent));
    }
}
