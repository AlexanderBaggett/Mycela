//! Canvas pointer samples fitted to the original cubic path records.
use super::authoring_path::reverse_points;
use super::*;

type Point = [f32; 2];
type Cubic = [Point; 4];

impl CanvasPath {
    pub fn from_freehand(samples: &[Point], smoothness: f32, closed: bool) -> Option<Self> {
        let mut points = fit_points(samples, smoothness)?;
        // The original keeps a small closing gap in a fresh freehand loop.
        if closed {
            if points.len() > 3 {
                points.pop();
            } else if points.len() == 3 {
                let [a, b] = [&points[1], &points[2]];
                let controls = [
                    a.position(),
                    a.handle(CanvasPathHandle::Outgoing),
                    b.handle(CanvasPathHandle::Incoming),
                    b.position(),
                ]
                .map(|point| point.map(|value| quantize_point(f64::from(value))));
                let [left, right] = split_cubic(controls, 0.5);
                let store = |point: [f64; 2]| point.map(|value| quantize_point(value) as f32);
                points[2].move_to(store(left[3]));
                points[2].move_handle(CanvasPathHandle::Incoming, store(left[2]), true);
                points[2].move_handle(CanvasPathHandle::Outgoing, store(right[1]), true);
            }
        }
        let path = Self {
            width: 3.,
            contours: vec![Contour { closed, points }],
        };
        path.valid().then_some(path)
    }

    /// Fits a stroke that starts or ends on an existing open contour.
    pub fn attach_freehand(
        &mut self,
        samples: &[Point],
        smoothness: f32,
        start: Option<CanvasPathEndpoint>,
        end: Option<CanvasPathEndpoint>,
    ) -> bool {
        let Some(mut fitted) = fit_points(samples, smoothness) else {
            return false;
        };
        let (Some(from), target) = (start.or(end), end.filter(|_| start.is_some())) else {
            return false;
        };
        if target == Some(from) {
            return false;
        }
        let Some(anchor) = self.endpoint(from).cloned() else {
            return false;
        };
        if start.is_none() {
            reverse_points(&mut fitted);
        }
        fitted[0] = match from {
            CanvasPathEndpoint::Start(_) => anchor.reversed(),
            CanvasPathEndpoint::End(_) => anchor,
        };
        let arrival = if let Some(target) = target {
            let Some(point) = self.endpoint(target).cloned() else {
                return false;
            };
            let mut point = match target {
                CanvasPathEndpoint::Start(_) => point,
                CanvasPathEndpoint::End(_) => point.reversed(),
            };
            let Some(mut last) = fitted.pop() else {
                return false;
            };
            last.move_to(point.position());
            point.move_handle(
                CanvasPathHandle::Incoming,
                last.handle(CanvasPathHandle::Incoming),
                true,
            );
            Some((target, point))
        } else {
            None
        };
        self.extend_open(from, fitted, arrival)
    }
}

fn fit_points(samples: &[Point], smoothness: f32) -> Option<Vec<CanvasPathPoint>> {
    if !(0. ..=100.).contains(&smoothness) {
        return None;
    }
    let smoothness = f64::from(smoothness);
    let threshold = smoothness * smoothness * smoothness / 50000. + 0.03;
    fit_to_error(samples, threshold)
}

pub(super) fn fit_to_error(samples: &[Point], threshold: f64) -> Option<Vec<CanvasPathPoint>> {
    if samples.len() < 2
        || !samples.iter().flatten().all(|value| value.is_finite())
        || samples.iter().all(|point| *point == samples[0])
    {
        return None;
    }
    let end = samples.len() - 1;
    let start_tangent = unit(sub(samples[1], samples[0]));
    let end_tangent = unit(sub(samples[end - 1], samples[end]));
    let mut pending = vec![(0, end, start_tangent, end_tangent)];
    let mut cubics = Vec::new();
    let mut parameters = Vec::new();

    // Keep the earlier half first and avoid recursion on long pointer traces.
    while let Some((first, last, left, right)) = pending.pop() {
        let points = &samples[first..=last];
        if points.len() == 2 {
            cubics.push(default_cubic(points[0], points[1], left, right));
            continue;
        }
        chord_parameters(points, &mut parameters);
        let mut cubic = fit_cubic(points, &parameters, left, right);
        let (mut error, mut split) = maximum_error(points, &parameters, cubic);
        if error < threshold {
            cubics.push(cubic);
            continue;
        }
        if error < threshold * threshold {
            for _ in 0..4 {
                for (parameter, point) in parameters.iter_mut().zip(points) {
                    *parameter = refine_parameter(cubic, *point, *parameter);
                }
                cubic = fit_cubic(points, &parameters, left, right);
                if !cubic.iter().flatten().all(|value| value.is_finite()) {
                    break;
                }
                (error, split) = maximum_error(points, &parameters, cubic);
                if error < threshold {
                    break;
                }
            }
            if error < threshold {
                cubics.push(cubic);
                continue;
            }
        }
        let split = first + split;
        let tangent = unit(
            add(
                sub(samples[split - 1], samples[split]),
                sub(samples[split], samples[split + 1]),
            )
            .map(|axis| axis / 2.),
        );
        pending.push((split, last, tangent.map(|axis| -axis), right));
        pending.push((first, split, left, tangent));
    }

    let first = cubics.first()?;
    let mut points = vec![CanvasPathPoint::new(first[0], first[0], first[1], 2)];
    for pair in cubics.windows(2) {
        let [left, right] = [pair[0], pair[1]];
        points.push(fitted_point(left[3], left[2], right[1]));
    }
    let last = cubics.last()?;
    if first[0] != last[3] {
        points.push(fitted_point(last[3], last[2], last[3]));
    }
    (points.len() >= 2 && points.iter().all(CanvasPathPoint::valid)).then_some(points)
}

fn fitted_point(at: Point, incoming: Point, outgoing: Point) -> CanvasPathPoint {
    let before = sub(incoming, at);
    let after = sub(at, outgoing);
    let lengths = [dot(before, before).sqrt(), dot(after, after).sqrt()];
    let smooth = (1.
        - dot(
            before.map(|x| x / lengths[0]),
            after.map(|x| x / lengths[1]),
        ))
    .abs()
        < 0.0001;
    CanvasPathPoint::new(at, incoming, outgoing, if smooth { 1 } else { 2 })
}

fn chord_parameters(points: &[Point], parameters: &mut Vec<f64>) {
    parameters.clear();
    parameters.push(0.);
    let mut total = 0.;
    for pair in points.windows(2) {
        total += distance(pair[0], pair[1]);
        parameters.push(total);
    }
    if total != 0. {
        for parameter in &mut parameters[1..] {
            *parameter /= total;
        }
    }
}

fn fit_cubic(points: &[Point], parameters: &[f64], left: Point, right: Point) -> Cubic {
    let first = points[0];
    let last = points[points.len() - 1];
    let [mut c00, mut c01, mut c11, mut x0, mut x1] = [0.; 5];
    for (&point, &t) in points.iter().zip(parameters) {
        let s = 1. - t;
        let [b0, b1, b2, b3] = [s * s * s, 3. * t * (s * s), 3. * t * t * s, t * t * t];
        let a = scale(left, b1);
        let b = scale(right, b2);
        c00 += dot(a, a);
        c01 += dot(a, b);
        c11 += dot(b, b);
        let residual = sub(
            point,
            add(
                scale(first, b0),
                add(scale(first, b1), add(scale(last, b2), scale(last, b3))),
            ),
        );
        x0 += dot(a, residual);
        x1 += dot(b, residual);
    }
    let determinant = f64::from(c00 * c11 - c01 * c01);
    let (alpha, beta) = if determinant == 0. {
        (0., 0.)
    } else {
        (
            f64::from(x0 * c11 - x1 * c01) / determinant,
            f64::from(c00 * x1 - c01 * x0) / determinant,
        )
    };
    let epsilon = 0.000001 * distance(first, last);
    if alpha < epsilon || beta < epsilon {
        default_cubic(first, last, left, right)
    } else {
        [
            first,
            add(first, with_length(left, alpha)),
            add(last, with_length(right, beta)),
            last,
        ]
    }
}

fn default_cubic(first: Point, last: Point, left: Point, right: Point) -> Cubic {
    let length = distance(first, last) / 3.;
    [
        first,
        add(first, with_length(left, length)),
        add(last, with_length(right, length)),
        last,
    ]
}

fn maximum_error(points: &[Point], parameters: &[f64], cubic: Cubic) -> (f64, usize) {
    let mut result = (0., points.len() / 2);
    for index in 1..points.len() - 1 {
        let difference = sub(evaluate(cubic, parameters[index]), points[index]);
        let error = f64::from(dot(difference, difference));
        if error >= result.0 {
            result = (error, index);
        }
    }
    result
}

fn refine_parameter([a, b, c, d]: Cubic, point: Point, t: f64) -> f64 {
    let cubic = [a, b, c, d];
    let derivative = [sub(b, a), sub(c, b), sub(d, c)].map(|p| p.map(|axis| axis * 3.));
    let second = [
        sub(derivative[1], derivative[0]),
        sub(derivative[2], derivative[1]),
    ]
    .map(|p| p.map(|axis| axis * 2.));
    let delta = sub(evaluate(cubic, t), point);
    let tangent = evaluate(derivative, t);
    let normal = evaluate(second, t);
    let denominator = f64::from(dot(tangent, tangent) + dot(delta, normal));
    if denominator == 0. {
        t
    } else {
        t - f64::from(dot(delta, tangent)) / denominator
    }
}

fn evaluate<const N: usize>(mut points: [Point; N], t: f64) -> Point {
    for remaining in (1..N).rev() {
        for i in 0..remaining {
            points[i] = mix(points[i], points[i + 1], t);
        }
    }
    points[0]
}

fn mix(a: Point, b: Point, t: f64) -> Point {
    std::array::from_fn(|i| ((1. - t) * f64::from(a[i]) + t * f64::from(b[i])) as f32)
}

fn add([x, y]: Point, [u, v]: Point) -> Point {
    [x + u, y + v]
}
fn sub([x, y]: Point, [u, v]: Point) -> Point {
    [x - u, y - v]
}
fn dot([x, y]: Point, [u, v]: Point) -> f32 {
    x * u + y * v
}

fn scale(point: Point, factor: f64) -> Point {
    point.map(|axis| (f64::from(axis) * factor) as f32)
}

fn distance(a: Point, b: Point) -> f64 {
    let [x, y] = sub(a, b).map(f64::from);
    (x * x + y * y).sqrt()
}

fn unit(point: Point) -> Point {
    let squared = f64::from(dot(point, point));
    if squared == 0. {
        point
    } else {
        scale(point, 1. / squared.sqrt())
    }
}

fn with_length(point: Point, length: f64) -> Point {
    let magnitude = f64::from(dot(point, point)).sqrt();
    if magnitude == 0. {
        point
    } else {
        point.map(|axis| axis * (length / magnitude) as f32)
    }
}
