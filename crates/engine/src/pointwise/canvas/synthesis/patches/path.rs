use super::Knot;
use crate::ImageError;

pub(super) type Point = [f64; 2];

#[derive(Clone, Copy)]
pub(super) struct Bounds {
    pub(super) min: Point,
    pub(super) size: Point,
}

impl Bounds {
    pub(super) fn center(self) -> Point {
        std::array::from_fn(|axis| self.min[axis] + self.size[axis] * 0.5)
    }

    pub(super) fn resized(self, from: [usize; 2], to: [usize; 2]) -> Self {
        let center = self.center();
        let size =
            std::array::from_fn(|axis| self.size[axis] * (to[axis] as f64 / from[axis] as f64));
        Self {
            min: std::array::from_fn(|axis| center[axis] - size[axis] * 0.5),
            size,
        }
    }
}

#[derive(Clone, Copy)]
struct Anchor {
    point: Point,
    before: Point,
    after: Point,
    mode: i32,
}

impl Anchor {
    fn from_knot(knot: &Knot) -> Self {
        let point = knot.point.map(f64::from);
        Self {
            point,
            before: std::array::from_fn(|axis| point[axis] + f64::from(knot.incoming[axis])),
            after: std::array::from_fn(|axis| point[axis] + f64::from(knot.outgoing[axis])),
            mode: knot.mode,
        }
    }

    fn is_corner(self) -> bool {
        self.mode == 0
    }

    fn corner_handle(self, other: Self, before: bool) -> Point {
        if other.is_corner() {
            return std::array::from_fn(|axis| {
                self.point[axis] - 0.25 * (self.point[axis] - other.point[axis])
            });
        }
        if self.point == other.point {
            return self.point;
        }
        if before {
            Cubic::new([other.point, other.after, self.point, self.point]).at(0.75)
        } else {
            Cubic::new([self.point, self.point, other.before, other.point]).at(0.25)
        }
    }

    fn align_smooth_handles(&mut self) {
        let [dx, dy] = std::array::from_fn(|axis| self.before[axis] - self.point[axis]);
        let [x, y] = std::array::from_fn(|axis| self.after[axis] - self.point[axis]);
        let length = (x * x + y * y).sqrt();
        let incoming = if dx == 0. && dy == 0. {
            0.
        } else {
            dy.atan2(dx) * 180. / std::f64::consts::PI
        };
        let mut opposite = -incoming - 180.;
        if opposite < 0. {
            opposite += 360.;
        }
        let angle = -opposite * std::f64::consts::PI / 180.;
        self.after = [
            self.point[0] + length * angle.cos(),
            self.point[1] + length * angle.sin(),
        ];
    }
}

impl Knot {
    /// The original links the closed contour after decoding and replaces Corner tangents.
    pub(in crate::pointwise::canvas::synthesis) fn canonical_handles(
        knots: &[Self],
        index: usize,
    ) -> [[f32; 2]; 2] {
        let knot = &knots[index];
        if knot.mode != 0 {
            return [knot.incoming, knot.outgoing];
        }
        let anchor = Anchor::from_knot(knot);
        let previous = Anchor::from_knot(&knots[(index + knots.len() - 1) % knots.len()]);
        let next = Anchor::from_knot(&knots[(index + 1) % knots.len()]);
        let incoming = anchor.corner_handle(previous, true);
        let outgoing = anchor.corner_handle(next, false);
        [incoming, outgoing]
            .map(|handle| std::array::from_fn(|axis| (handle[axis] - anchor.point[axis]) as f32))
    }
}

pub(super) struct PlacementOutline([Anchor; 4]);

impl PlacementOutline {
    pub(super) fn new(bounds: Bounds, photo_size: [usize; 2]) -> Result<Option<Self>, ImageError> {
        let [x, y] = std::array::from_fn(|axis| bounds.min[axis] / photo_size[axis] as f64);
        let [w, h] = std::array::from_fn(|axis| bounds.size[axis] / photo_size[axis] as f64);
        let mut anchors = [[x, y], [x + w, y], [x + w, y + h], [x, y + h]].map(|point| Anchor {
            point,
            before: point,
            after: point,
            mode: 0,
        });
        set_corners(&mut anchors);
        // Genetica fits the sampled bounds of a normalized rectangle back into
        // the source bounds before rotating it. Cubic coordinate rounding means
        // the fitted anchors can extend slightly past the source bounds.
        let sampled = Path::new(Vec::from(anchors))?.bounds;
        if sampled.size.contains(&0.) {
            return Ok(None);
        }
        let scale: Point = std::array::from_fn(|axis| bounds.size[axis] / sampled.size[axis]);
        let fit = |point: Point| {
            std::array::from_fn(|axis| {
                bounds.min[axis] + (point[axis] - sampled.min[axis]) * scale[axis]
            })
        };
        Ok(Some(Self(Self::map(anchors, fit))))
    }

    pub(super) fn transform(&self, transform: impl Fn(Point) -> Point) -> Result<Path, ImageError> {
        Path::new(Vec::from(Self::map(self.0, transform)))
    }

    fn map(anchors: [Anchor; 4], transform: impl Fn(Point) -> Point) -> [Anchor; 4] {
        anchors.map(|anchor| Anchor {
            point: transform(anchor.point),
            before: transform(anchor.before),
            after: transform(anchor.after),
            ..anchor
        })
    }
}

pub(super) struct Path {
    anchors: Vec<Anchor>,
    points: Vec<Point>,
    pub(super) bounds: Bounds,
}

impl Path {
    pub(super) fn outline(&self) -> &[Point] {
        &self.points
    }

    pub(super) fn handles(knots: &[Knot], index: usize) -> Option<[[f32; 2]; 2]> {
        let knot = knots.get(index)?;
        if knot.mode != 0 {
            return Some([knot.incoming, knot.outgoing]);
        }
        let anchors = anchors_from_knots(knots).ok()?;
        let anchor = anchors.get(index)?;
        Some(
            [anchor.before, anchor.after].map(|handle| {
                std::array::from_fn(|axis| (handle[axis] - anchor.point[axis]) as f32)
            }),
        )
    }

    pub(super) fn from_knots(knots: &[Knot]) -> Result<Option<Self>, ImageError> {
        if knots.len() < 3 {
            return Ok(None);
        }
        let path = Self::new(anchors_from_knots(knots)?)?;
        Ok((path.bounds.size[0] > 0. && path.bounds.size[1] > 0.).then_some(path))
    }

    pub(super) fn split(knots: &mut Vec<Knot>, first: usize, t: f64) -> Option<usize> {
        if knots.len() < 2 || first >= knots.len() || !(t > 0. && t < 1.) {
            return None;
        }
        let anchors = anchors_from_knots(knots).ok()?;
        let second = (first + 1) % knots.len();
        let [a, b] = [anchors[first], anchors[second]];
        let [p0, p1, p2, p3] =
            [a.point, a.after, b.before, b.point].map(|point| point.map(quantize));
        let interpolate = |a: Point, b: Point| std::array::from_fn(|i| a[i] + t * (b[i] - a[i]));
        let offset = |point: Point, handle: Point| {
            std::array::from_fn(|i| (quantize(handle[i]) - point[i]) as f32)
        };
        let before = interpolate(p0, p1);
        let between = interpolate(p1, p2);
        let after = interpolate(p2, p3);
        let incoming = interpolate(before, between);
        let outgoing = interpolate(between, after);
        let point = interpolate(incoming, outgoing).map(quantize);
        let mode = if a.is_corner() && b.is_corner() {
            0
        } else {
            for (index, anchor) in [(first, a), (second, b)] {
                knots[index].mode = 2;
                if anchor.is_corner() {
                    knots[index].incoming = offset(anchor.point, anchor.before);
                    knots[index].outgoing = offset(anchor.point, anchor.after);
                }
            }
            knots[first].outgoing = offset(a.point, before);
            knots[second].incoming = offset(b.point, after);
            1
        };
        knots.insert(
            first + 1,
            Knot {
                point: point.map(|coord| coord as f32),
                incoming: offset(point, incoming),
                outgoing: offset(point, outgoing),
                mode,
                flags: [true, false, false],
            },
        );
        Some(first + 1)
    }

    pub(super) fn fit(mut self, size: [usize; 2]) -> Result<Self, ImageError> {
        let target = size.map(|length| length as f64);
        self = self.remap(target, false)?;
        if self.bounds.min != [0.; 2] || self.bounds.size != target {
            self = self.remap(target, true)?;
        }
        Ok(self)
    }

    fn remap(self, target: Point, reconnect: bool) -> Result<Self, ImageError> {
        let scale: Point = std::array::from_fn(|axis| target[axis] / self.bounds.size[axis]);
        let transform =
            |point: Point| std::array::from_fn(|i| (point[i] - self.bounds.min[i]) * scale[i]);
        let mut anchors = self.anchors;
        for anchor in &mut anchors {
            anchor.point = transform(anchor.point);
            anchor.before = transform(anchor.before);
            anchor.after = transform(anchor.after);
            if anchor.mode == 1 {
                // Fitting a smooth knot restores the opposite handle angle
                // from the transformed incoming handle and keeps the new length.
                anchor.align_smooth_handles();
            }
        }
        if reconnect {
            set_corners(&mut anchors);
        }
        Self::new(anchors)
    }

    fn new(anchors: Vec<Anchor>) -> Result<Self, ImageError> {
        let count = anchors.len().checked_mul(100).ok_or(ImageError::Overflow)?;
        let mut points = Vec::new();
        points
            .try_reserve_exact(count)
            .map_err(|_| ImageError::Allocation)?;
        let mut min = [f64::MAX; 2];
        let mut max = [f64::MIN; 2];
        for (index, anchor) in anchors.iter().enumerate() {
            let next = anchors[(index + 1) % anchors.len()];
            let segment = Cubic::new([anchor.point, anchor.after, next.before, next.point]);
            for sample in 1..=100 {
                let point = segment.at(f64::from(0.01_f32) * f64::from(sample));
                for axis in 0..2 {
                    min[axis] = min[axis].min(point[axis]);
                    max[axis] = max[axis].max(point[axis]);
                }
                points.push(point);
            }
        }
        let bounds = Bounds {
            min,
            size: std::array::from_fn(|i| max[i] - min[i]),
        };
        Ok(Self {
            anchors,
            points,
            bounds,
        })
    }

    pub(super) fn crossings(&self, y: f64, output: &mut Vec<f64>) {
        output.clear();
        let add = |output: &mut Vec<f64>, x| match output
            .binary_search_by(|value| value.partial_cmp(&x).unwrap_or(std::cmp::Ordering::Equal))
        {
            Ok(index) => {
                output.remove(index);
            }
            Err(index) => output.insert(index, x),
        };
        let count = self.points.len();
        for index in 0..count {
            let [x0, y0] = self.points[index];
            let [x1, y1] = self.points[(index + 1) % count];
            if (y0 - y).abs() < 0.0005 {
                if (y0 - y1).abs() < 0.0005 {
                    add(output, x0);
                    add(output, x1);
                } else {
                    let previous = self.points[(index + count - 1) % count][1];
                    if (y0 - previous) * (y0 - y1) < -0.0005 {
                        add(output, x0);
                    }
                }
            } else if (y1 - y).abs() >= 0.0005 {
                let relative = (y - y0) / (y1 - y0);
                if relative > 0. && relative < 1. {
                    add(output, x0 + (x1 - x0) * relative);
                }
            }
        }
    }
}

fn anchors_from_knots(knots: &[Knot]) -> Result<Vec<Anchor>, ImageError> {
    let mut anchors = Vec::new();
    anchors
        .try_reserve_exact(knots.len())
        .map_err(|_| ImageError::Allocation)?;
    for knot in knots {
        let anchor = Anchor::from_knot(knot);
        if anchor
            .point
            .into_iter()
            .chain(anchor.before)
            .chain(anchor.after)
            .any(|value| !value.is_finite())
        {
            return Err(ImageError::Argument);
        }
        anchors.push(anchor);
    }
    set_corners(&mut anchors);
    Ok(anchors)
}

fn set_corners(anchors: &mut [Anchor]) {
    for index in 0..anchors.len() {
        let anchor = anchors[index];
        if !anchor.is_corner() {
            continue;
        }
        let previous = anchors[(index + anchors.len() - 1) % anchors.len()];
        let next = anchors[(index + 1) % anchors.len()];
        anchors[index].before = anchor.corner_handle(previous, true);
        anchors[index].after = anchor.corner_handle(next, false);
    }
}

fn quantize(value: f64) -> f64 {
    ((value.abs() * 100000. + 0.5) as i32 as f64 / 100000.).copysign(value)
}

struct Cubic([[f64; 4]; 2]);
impl Cubic {
    fn new(points: [Point; 4]) -> Self {
        Self(std::array::from_fn(|axis| {
            let [p0, p1, p2, p3] = points.map(|point| quantize(point[axis]));
            let linear = 3. * (p1 - p0);
            let quadratic = 3. * (p2 - p1) - linear;
            [p3 - p0 - linear - quadratic, quadratic, linear, p0]
        }))
    }

    fn at(&self, t: f64) -> Point {
        let squared = t * t;
        let cubed = squared * t;
        self.0
            .map(|[a, b, c, d]| a * cubed + b * squared + c * t + d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fitting_a_patch_realigns_saved_smooth_handles() {
        let knot = |point, incoming, outgoing, mode| Knot {
            point,
            incoming,
            outgoing,
            mode,
            flags: [false; 3],
        };
        let knots = [
            knot([0., 0.], [0.1, 0.2], [-0.2, -0.39], 1),
            knot([1., 0.], [0.; 2], [0.; 2], 0),
            knot([0.5, 1.], [0.; 2], [0.; 2], 0),
        ];
        let path = Path::from_knots(&knots).unwrap().unwrap();
        let fitted = path.fit([189, 284]).unwrap();
        let anchor = fitted.anchors[0];
        let [dx, dy] = std::array::from_fn(|axis| anchor.before[axis] - anchor.point[axis]);
        let [x, y] = std::array::from_fn(|axis| anchor.after[axis] - anchor.point[axis]);
        assert!((dx * y - dy * x).abs() < 1.0e-9);
        assert!(dx * x + dy * y < 0.);
    }

    #[test]
    fn inserting_a_linked_knot_splits_the_existing_cubic() {
        let knot = |point, incoming, outgoing| Knot {
            point,
            incoming,
            outgoing,
            mode: 2,
            flags: [false; 3],
        };
        let mut knots = vec![
            knot([0., 0.], [0., 0.25], [0.25, 0.5]),
            knot([1., 0.], [-0.25, 0.5], [0., 0.25]),
            knot([0.5, 1.], [0.25, -0.25], [-0.25, -0.25]),
        ];
        let segment = |anchors: &[Anchor], a: usize, b: usize| {
            Cubic::new([
                anchors[a].point,
                anchors[a].after,
                anchors[b].before,
                anchors[b].point,
            ])
        };
        let original = knots.clone();
        assert_eq!(Path::split(&mut knots, 0, f64::NAN), None);
        for (start, t) in [(0, 0.5), (0, 0.27), (2, 0.68)] {
            let mut knots = original.clone();
            let old = segment(&anchors_from_knots(&knots).unwrap(), start, (start + 1) % 3);
            assert_eq!(Path::split(&mut knots, start, t), Some(start + 1));
            assert_eq!(knots[start + 1].mode, 1);
            let anchors = anchors_from_knots(&knots).unwrap();
            let first = segment(&anchors, start, start + 1);
            let second = segment(&anchors, start + 1, (start + 2) % 4);
            for sample in [0., 0.25, 0.5, 0.75, 1.] {
                for (a, b) in [
                    (first.at(sample), old.at(sample * t)),
                    (second.at(sample), old.at(t + sample * (1. - t))),
                ] {
                    for axis in 0..2 {
                        assert!((a[axis] - b[axis]).abs() <= 0.00001);
                    }
                }
            }
        }
    }
}
