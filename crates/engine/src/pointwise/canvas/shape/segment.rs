//! Picking and subdividing a retained Canvas Bézier segment.
use super::*;

type Point = [f64; 2];
type Cubic = [Point; 4];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CanvasPathSegment {
    contour: usize,
    start: usize,
    parameter: f64,
    position: [f32; 2],
    distance_squared: f64,
}

impl CanvasPathSegment {
    pub fn contour(self) -> usize {
        self.contour
    }

    pub fn parameter(self) -> f64 {
        self.parameter
    }

    pub fn position(self) -> [f32; 2] {
        self.position
    }

    pub fn distance_squared(self) -> f64 {
        self.distance_squared
    }
}

impl CanvasPath {
    /// Finds an interior segment point in authored Canvas coordinates.
    pub fn pick_segment(&self, sample: [f32; 2], tolerance: f64) -> Option<CanvasPathSegment> {
        if !sample.into_iter().all(f32::is_finite) || !tolerance.is_finite() || tolerance <= 0. {
            return None;
        }
        let sample = sample.map(f64::from);
        let mut best = None;
        let mut distance = tolerance * tolerance;
        for (contour, outline) in self.contours.iter().enumerate() {
            if outline.points.len() < 2 {
                continue;
            }
            for (start, (controls, straight)) in
                outline.segment_controls([0.; 2], false).enumerate()
            {
                if !bounds_near(sample, controls, tolerance) {
                    continue;
                }
                let (parameter, point) = if straight {
                    let [a, _, _, b] = controls;
                    let delta = subtract(b, a);
                    let length_squared = dot(delta, delta);
                    if length_squared == 0. {
                        continue;
                    }
                    let t = (dot(subtract(sample, a), delta) / length_squared).clamp(0., 1.);
                    (t, std::array::from_fn(|axis| a[axis] + t * delta[axis]))
                } else {
                    closest_parameter(controls, sample)
                };
                let offset = subtract(point, sample);
                let squared = dot(offset, offset);
                if parameter > 0. && parameter < 1. && squared <= distance {
                    distance = squared;
                    best = Some(CanvasPathSegment {
                        contour,
                        start,
                        parameter,
                        position: point.map(|axis| axis as f32),
                        distance_squared: squared,
                    });
                }
            }
        }
        best
    }

    /// Adds a vertex and replaces its adjoining segment by two subdivisions.
    pub fn insert_on_segment(&mut self, segment: CanvasPathSegment) -> Option<usize> {
        self.subdivide_segment(segment)
    }

    /// Replaces a segment by two subdivisions with coincident open endpoints.
    pub fn split_on_segment(&mut self, segment: CanvasPathSegment) -> bool {
        let Some(index) = self.subdivide_segment(segment) else {
            return false;
        };
        let point = &self.contours[segment.contour].points[index];
        let position = point.position();
        let mode = if point.mode == 0 { 0 } else { 2 };
        let ending = CanvasPathPoint::new(
            position,
            point.handle(CanvasPathHandle::Incoming),
            position,
            mode,
        );
        let beginning = CanvasPathPoint::new(
            position,
            position,
            point.handle(CanvasPathHandle::Outgoing),
            mode,
        );
        self.break_contour(segment.contour, index, ending, beginning);
        true
    }

    fn subdivide_segment(&mut self, segment: CanvasPathSegment) -> Option<usize> {
        let contour = self.contours.get_mut(segment.contour)?;
        let count = contour.points.len();
        let next = segment.start.checked_add(1)?;
        if count < 2
            || segment.start >= count
            || (!contour.closed && next == count)
            || segment.parameter <= 0.
            || segment.parameter >= 1.
        {
            return None;
        }
        let start = &contour.points[segment.start];
        let end = &contour.points[next % count];
        let straight = start.mode == 0 && end.mode == 0;
        let mut controls = segment_controls(start, end);
        if straight {
            let [a, _, _, b] = controls;
            controls[1] = std::array::from_fn(|axis| a[axis] + (b[axis] - a[axis]) / 3.);
            controls[2] = std::array::from_fn(|axis| b[axis] - (b[axis] - a[axis]) / 3.);
        }
        let [left, right] = split_cubic(controls, segment.parameter);
        let store = |point: Point| point.map(|axis| quantize_point(axis) as f32);
        let position = store(left[3]);
        let vertex = CanvasPathPoint::new(
            position,
            store(left[2]),
            store(right[1]),
            if straight { 0 } else { 1 },
        );
        if !vertex.valid() || position == start.position() || position == end.position() {
            return None;
        }

        if !straight {
            contour.points[segment.start].move_handle(
                CanvasPathHandle::Outgoing,
                store(left[1]),
                true,
            );
            contour.points[next % count].move_handle(
                CanvasPathHandle::Incoming,
                store(right[2]),
                true,
            );
        }
        contour.points.insert(next, vertex);
        Some(next)
    }
}

fn bounds_near(sample: Point, controls: Cubic, tolerance: f64) -> bool {
    sample.into_iter().enumerate().all(|(axis, value)| {
        let mut low = f64::INFINITY;
        let mut high = f64::NEG_INFINITY;
        for point in controls {
            if !point[axis].is_finite() {
                return false;
            }
            low = low.min(point[axis]);
            high = high.max(point[axis]);
        }
        (low - tolerance..=high + tolerance).contains(&value)
    })
}

fn closest_parameter(controls: Cubic, sample: Point) -> (f64, Point) {
    let curve = CubicCoefficients::new(controls);
    let [low, high] = curve.nearest_bracket(sample);
    let t = (low + high) * 0.5;
    (t, curve.position(t))
}

#[derive(Clone, Copy)]
pub(in crate::pointwise::canvas) struct CubicCoefficients([[f64; 4]; 2]);

impl CubicCoefficients {
    pub(in crate::pointwise::canvas) fn new([a, b, c, d]: Cubic) -> Self {
        Self([0, 1].map(|axis| {
            let first = 3. * (b[axis] - a[axis]);
            let second = 3. * (c[axis] - b[axis]) - first;
            let third = d[axis] - a[axis] - first - second;
            [a[axis], first, second, third]
        }))
    }

    pub(in crate::pointwise::canvas) fn position(self, t: f64) -> Point {
        let squared = t * t;
        let cubed = squared * t;
        self.0
            .map(|[x, w, v, u]| u * cubed + v * squared + w * t + x)
    }

    pub(in crate::pointwise::canvas) fn nearest_bracket(self, sample: Point) -> [f64; 2] {
        // Genetica starts at 128 steps, refines with 32 and requires two
        // consecutive converged passes.
        let mut low = 0.;
        let mut high = 1.;
        let mut close_last_pass = false;
        for pass in 0..16 {
            let steps = if pass == 0 { 128 } else { 32 };
            let step = (high - low) / f64::from(steps);
            let mut best = f64::MAX;
            let mut next_best = f64::MAX;
            let mut closest = low;
            let mut t = low;
            for _ in 0..=steps {
                let delta = subtract(sample, self.position(t));
                let distance = dot(delta, delta);
                if distance < best {
                    next_best = best;
                    best = distance;
                    closest = t;
                } else {
                    next_best = next_best.min(distance);
                }
                t += step;
            }
            if best == 0. || step <= f64::EPSILON {
                return [closest.clamp(0., 1.); 2];
            }
            low = (closest - step).max(0.);
            high = (closest + step).min(1.);
            let close = (next_best - best) / best < 0.1 || next_best < f64::from(1.0e-8_f32);
            if close && close_last_pass {
                break;
            }
            close_last_pass = close;
        }
        [low, high]
    }
}

fn subtract(a: Point, b: Point) -> Point {
    [a[0] - b[0], a[1] - b[1]]
}

fn dot(a: Point, b: Point) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}
