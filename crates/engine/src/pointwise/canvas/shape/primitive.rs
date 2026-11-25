//! Geometric outlines in the source shapes' normalized bounding box.
use super::{CanvasPrimitive, DistanceSegment, OutlineSampling, PointwiseError, ShapeBase, cubic};
use std::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI, TAU};

type Point = [f32; 2];
const CORNERS: [Point; 4] = [[-0.5, -0.5], [0.5, -0.5], [0.5, 0.5], [-0.5, 0.5]];
const QUARTER_ARC_HANDLE: f32 = 0.552285;

pub(super) fn outline<E>(
    kind: CanvasPrimitive,
    base: &ShapeBase,
    sampling: OutlineSampling,
) -> Result<Vec<[f64; 2]>, PointwiseError<E>> {
    Ok(build_outline(kind, base, sampling)?.vertices)
}

pub(super) struct DistanceOutline {
    pub(super) vertices: Vec<[f64; 2]>,
    pub(super) segments: Vec<DistanceSegment>,
}

pub(super) fn distance_outline<E>(
    kind: CanvasPrimitive,
    base: &ShapeBase,
) -> Result<DistanceOutline, PointwiseError<E>> {
    let outline = build_outline(kind, base, OutlineSampling::Distance)?;
    Ok(DistanceOutline {
        vertices: outline.vertices,
        segments: outline.segments,
    })
}

fn build_outline<E>(
    kind: CanvasPrimitive,
    base: &ShapeBase,
    sampling: OutlineSampling,
) -> Result<Outline<'_>, PointwiseError<E>> {
    use CanvasPrimitive::*;

    if base.transform.iter().any(|value| !value.is_finite()) {
        return Err(PointwiseError::Argument);
    }
    let mut outline = Outline {
        base,
        matrix: if matches!(sampling, OutlineSampling::DocumentBounds) {
            PrimitiveMatrix::authored(base)
        } else {
            PrimitiveMatrix::new(base)
        },
        vertices: Vec::new(),
        segments: Vec::new(),
        endpoints: None,
        sampling,
    };
    // Read the retained wire keys rather than the translated labels. The
    // original dictionary keeps the last value when a key is repeated.
    let control = |name: &str| {
        base.controls
            .iter()
            .rev()
            .find(|(key, _)| key.iter().copied().eq(name.encode_utf16()))
            .map(|(_, value)| *value)
            .filter(|value| value.is_finite())
            .ok_or(PointwiseError::Argument)
    };
    let count = |name| {
        let value = control(name)? as i32;
        if !(3..=4096).contains(&value) {
            return Err(PointwiseError::Argument);
        }
        Ok(value)
    };

    match kind {
        Rectangle => outline.polygon(CORNERS),
        IsoscelesTriangle => outline.polygon([[0., -0.5], [0.5, 0.5], [-0.5, 0.5]]),
        RightTriangle => outline.polygon([[-0.5, -0.5], [0.5, 0.5], [-0.5, 0.5]]),
        ClippedRectangle => {
            rectangle_corners(
                &mut outline,
                control("Radius")?,
                Corner::Clipped(control("Clip")? * 0.01),
            );
        }
        RoundedRectangle => {
            rectangle_corners(
                &mut outline,
                control("Radius")?,
                Corner::Rounded(control("Direction")? * 0.01),
            );
        }
        Ellipse => circle(&mut outline, 0.5),
        Pie => pie(&mut outline, control("Start Angle")?, control("End Angle")?)?,
        Parallelogram => {
            let slant = control("Slant")? * 0.01;
            outline.polygon([
                CORNERS[3],
                [slant - 0.5, -0.5],
                CORNERS[1],
                [0.5 - slant, 0.5],
            ]);
        }
        Trapezoid => {
            let inset = control("Slant")? * 0.01 * 0.5;
            outline.polygon([
                CORNERS[3],
                [inset - 0.5, -0.5],
                [0.5 - inset, -0.5],
                CORNERS[2],
            ]);
        }
        Polygon => polygon(&mut outline, count("Sides")?, control("Rounding")? * 0.01),
        Star => star(
            &mut outline,
            count("Points")?,
            control("Inner Radius")?,
            control("Twist")?,
        ),
        CurvedStar => curved_star(
            &mut outline,
            count("Points")?,
            control("Direction")?,
            control("Strength")?,
            control("Twist")?,
        ),
        Gear => gear(
            &mut outline,
            count("Teeth")?,
            control("Inner Radius")?,
            control("Middle Radius")?,
            control("Bevel")?,
        ),
        Flower => flower(
            &mut outline,
            count("Petals")?,
            control("Inner Radius")?,
            control("Inner Tension")?,
            control("Outer Tension")?,
        ),
        Arrow | DoubleArrow | NotchedArrow => {
            let thickness_key = if kind == Arrow {
                "Thickness"
            } else {
                "Tickness"
            };
            arrow(
                &mut outline,
                kind,
                control("Head Width")? * 0.01,
                control(thickness_key)? * 0.01 * 0.5,
            );
        }
        Chevron => {
            let slant = control("Slant")? * 0.01;
            outline.polygon([
                [0.5, 0.],
                [0.5 - slant, -0.5],
                CORNERS[0],
                [slant - 0.5, 0.],
                CORNERS[3],
                [0.5 - slant, 0.5],
            ]);
        }
    }

    outline.vertices.dedup();
    if outline.vertices.len() > 1 && outline.vertices.first() == outline.vertices.last() {
        outline.vertices.pop();
    }
    if let Some([first, last]) = outline.endpoints {
        outline.line(last, first);
    }
    Ok(outline)
}

struct Outline<'a> {
    base: &'a ShapeBase,
    matrix: PrimitiveMatrix,
    vertices: Vec<[f64; 2]>,
    segments: Vec<DistanceSegment>,
    endpoints: Option<[Point; 2]>,
    sampling: OutlineSampling,
}

impl Outline<'_> {
    fn vertex(&mut self, point: Point) {
        self.vertices
            .push(self.matrix.transform(point).map(super::round_endpoint));
        self.begin_segment(point);
    }

    fn begin_segment(&mut self, point: Point) {
        if !matches!(self.sampling, OutlineSampling::Distance) {
            return;
        }
        if let Some([start, previous]) = self.endpoints {
            self.line(previous, point);
            self.endpoints = Some([start, point]);
        } else {
            self.endpoints = Some([point; 2]);
        }
    }

    fn line(&mut self, start: Point, end: Point) {
        let from = self.matrix.transform(start).map(super::round_endpoint);
        let to = self.matrix.transform(end).map(super::round_endpoint);
        if from != to {
            let handle = |t: f64| {
                std::array::from_fn(|axis| {
                    let first = f64::from(start[axis]);
                    (first + t * (f64::from(end[axis]) - first)) as f32
                })
            };
            self.segments.push(DistanceSegment {
                controls: [
                    from,
                    self.matrix.transform_prepared(handle(0.25)),
                    self.matrix.transform_prepared(handle(0.75)),
                    to,
                ],
                corners: [true; 2],
                rounded: [true; 2],
            });
        }
    }

    fn polygon(&mut self, points: impl IntoIterator<Item = Point>) {
        for point in points {
            self.vertex(point);
        }
    }

    fn cubic(&mut self, points: [Point; 4]) {
        let mut controls = points.map(|point| self.matrix.transform(point));
        cubic(&mut self.vertices, controls, self.sampling);
        self.begin_segment(points[0]);
        if let Some([start, _]) = self.endpoints {
            controls[0] = controls[0].map(super::round_endpoint);
            controls[3] = controls[3].map(super::round_endpoint);
            self.segments.push(DistanceSegment {
                controls,
                corners: [false; 2],
                rounded: [true; 2],
            });
            self.endpoints = Some([start, points[3]]);
        }
    }
}

pub(super) struct PrimitiveMatrix {
    center: Point,
    x: Point,
    y: Point,
}

impl PrimitiveMatrix {
    fn new(base: &ShapeBase) -> Self {
        let mut matrix = Self::authored(base);
        matrix.center = matrix.center.map(|value| value.rem_euclid(100.));
        matrix
    }

    fn authored(base: &ShapeBase) -> Self {
        let [cx, cy, width, height, skew_x, skew_y, angle] = base.transform;
        let (sin, cos) = (-f64::from(angle)).to_radians().sin_cos();
        let (sin, cos) = (sin as f32, cos as f32);
        let rotate = |x, y| [x * cos - y * sin, x * sin + y * cos];
        Self {
            center: [cx, cy],
            x: rotate(width, -0.5 * skew_y),
            y: rotate(-0.5 * skew_x, height),
        }
    }

    pub(super) fn authored_corners(base: &ShapeBase) -> [[f32; 2]; 4] {
        let matrix = Self::authored(base);
        [[-0.5, -0.5], [-0.5, 0.5], [0.5, -0.5], [0.5, 0.5]].map(|[x, y]| {
            std::array::from_fn(|i| x * matrix.x[i] + y * matrix.y[i] + matrix.center[i])
        })
    }

    fn transform(&self, point: Point) -> [f64; 2] {
        use super::quantize_point;
        self.transform_prepared(point.map(|value| quantize_point(f64::from(value)) as f32))
    }

    fn transform_prepared(&self, [x, y]: Point) -> [f64; 2] {
        use super::quantize_point;
        // Genetica quantizes the authored segment, uses a GDI+ single-precision
        // matrix, then quantizes again when it updates the segment points.
        std::array::from_fn(|i| {
            quantize_point(f64::from(x * self.x[i] + y * self.y[i] + self.center[i]))
        })
    }
}

fn polar(radius: f32, angle: f64) -> Point {
    [radius * angle.cos() as f32, radius * angle.sin() as f32]
}

fn offset(start: Point, radius: f32, angle: f64) -> Point {
    let [x, y] = polar(radius, angle);
    [start[0] + x, start[1] + y]
}

fn interpolate(start: Point, end: Point, amount: f32) -> Point {
    std::array::from_fn(|axis| start[axis] + amount * (end[axis] - start[axis]))
}

fn arc_handle(angle: f64) -> f32 {
    if angle <= 0. {
        return 0.;
    }
    ((8. * (0.5 * angle).sin() - 4. * angle.sin()) / (3. * (1. - angle.cos()))) as f32
}

fn circle(outline: &mut Outline<'_>, radius: f32) {
    let handle = QUARTER_ARC_HANDLE * radius;
    let mut points = [
        [-radius, 0.],
        [-radius, -handle],
        [-handle, -radius],
        [0., -radius],
    ];
    for _ in 0..4 {
        outline.cubic(points);
        points = points.map(|[x, y]| [-y, x]);
    }
}

enum Corner {
    Clipped(f32),
    Rounded(f32),
}

fn rectangle_corners(outline: &mut Outline<'_>, radius: f32, corner: Corner) {
    if radius == 0. {
        outline.polygon(CORNERS);
        return;
    }
    let [width, height] = [
        outline.base.transform[2].abs(),
        outline.base.transform[3].abs(),
    ];
    let radius = 0.005 * radius;
    let size = if width > height {
        [radius * height / width, radius]
    } else {
        [radius, radius * width / height]
    };
    let scale = if width < height {
        [1., width / height]
    } else {
        [height / width, 1.]
    };
    for (i, current) in CORNERS.into_iter().enumerate() {
        let prev = CORNERS[(i + 3) % 4];
        let next = CORNERS[(i + 1) % 4];
        let start =
            std::array::from_fn(|axis| current[axis] + size[axis] * (prev[axis] - current[axis]));
        let end =
            std::array::from_fn(|axis| current[axis] + size[axis] * (next[axis] - current[axis]));
        match corner {
            Corner::Clipped(clip) => {
                let across = std::array::from_fn(|axis| start[axis] + end[axis] - current[axis]);
                outline.polygon([start, interpolate(current, across, clip)]);
            }
            Corner::Rounded(direction) => {
                let angle = -FRAC_PI_4 + i as f64 * FRAC_PI_2;
                let bend = f64::from(direction) * 0.25 * PI;
                let length = QUARTER_ARC_HANDLE * size[0].max(size[1]);
                let handle = |point: Point, theta| {
                    let end = offset(point, length, theta);
                    std::array::from_fn(|axis| {
                        point[axis] + scale[axis] * (end[axis] - point[axis])
                    })
                };
                outline.cubic([
                    start,
                    handle(start, angle - bend),
                    handle(end, angle + PI + bend),
                    end,
                ]);
            }
        }
        outline.vertex(end);
    }
}

fn pie<E>(outline: &mut Outline<'_>, start: f32, end: f32) -> Result<(), PointwiseError<E>> {
    let begin = 360. - end;
    let mut finish = 360. - start;
    if finish <= begin {
        finish += 360.;
    }
    let sweep = finish - begin;
    if !(0. ..=360.).contains(&sweep) {
        return Err(PointwiseError::Argument);
    }
    let begin = begin.rem_euclid(360.);
    let mut from = f64::from(begin) * PI / 180.;
    let to = f64::from(begin + sweep) * PI / 180.;
    outline.vertex([0., 0.]);
    let mut boundary = ((from + FRAC_PI_2) / FRAC_PI_2).floor() * FRAC_PI_2;
    while from < to {
        let next = boundary.min(to);
        let start = polar(0.5, from);
        let end = polar(0.5, next);
        let handle = 0.5 * arc_handle(next - from);
        outline.cubic([
            start,
            offset(start, handle, from + FRAC_PI_2),
            offset(end, handle, next - FRAC_PI_2),
            end,
        ]);
        from = next;
        boundary += FRAC_PI_2;
    }
    outline.vertex(polar(0.5, to));
    Ok(())
}

fn polygon(outline: &mut Outline<'_>, sides: i32, rounding: f32) {
    let step = TAU / f64::from(sides);
    let mut theta = FRAC_PI_2 - PI / f64::from(sides);
    let points: Vec<_> = (0..sides)
        .map(|_| {
            let point = polar(0.5, theta);
            theta += step;
            point
        })
        .collect();
    if rounding == 0. {
        outline.polygon(points);
        return;
    }
    if rounding == 1. {
        circle(outline, 0.5 * (PI / f64::from(sides)).cos() as f32);
        return;
    }

    let corner = |point: Point, adjacent: Point| {
        std::array::from_fn(|axis| point[axis] + 0.5 * (adjacent[axis] - point[axis]) * rounding)
    };
    let inner = |point: Point| point.map(|axis| axis - axis * rounding);
    let from = corner(points[0], points[points.len() - 1]);
    let inner_first = inner(points[0]);
    let [dx, dy] = difference(from, inner_first);
    let radius = f64::from(dx * dx + dy * dy).sqrt();
    let handle =
        radius as f32 * arc_handle(bearing(points[0], inner_first) - bearing(from, inner_first));

    for (i, &outer) in points.iter().enumerate() {
        let center = inner(outer);
        let start = corner(outer, points[(i + points.len() - 1) % points.len()]);
        let end = corner(outer, points[(i + 1) % points.len()]);
        let middle_angle = bearing(outer, center);
        let middle = [
            center[0] + (radius * middle_angle.cos()) as f32,
            center[1] + (radius * middle_angle.sin()) as f32,
        ];
        outline.cubic([
            start,
            offset(start, handle, bearing(start, center) + FRAC_PI_2),
            offset(middle, handle, middle_angle - FRAC_PI_2),
            middle,
        ]);
        outline.cubic([
            middle,
            offset(middle, handle, middle_angle + FRAC_PI_2),
            offset(end, handle, bearing(end, center) - FRAC_PI_2),
            end,
        ]);
        outline.vertex(end);
    }
}

fn difference(point: Point, origin: Point) -> Point {
    [point[0] - origin[0], point[1] - origin[1]]
}

fn bearing(point: Point, origin: Point) -> f64 {
    let [x, y] = difference(point, origin);
    f64::from(y).atan2(f64::from(x))
}

fn star(outline: &mut Outline<'_>, points: i32, inner: f32, twist: f32) {
    let step = TAU / f64::from(points);
    let mut outer_angle = -FRAC_PI_2;
    let mut inner_angle = outer_angle - 0.5 * step + f64::from(twist * 0.01) * step;
    let inner = inner * 0.01 * 0.5;
    for _ in 0..points {
        outline.polygon([polar(inner, inner_angle), polar(0.5, outer_angle)]);
        outer_angle += step;
        inner_angle += step;
    }
}

fn curved_star(outline: &mut Outline<'_>, points: i32, direction: f32, strength: f32, twist: f32) {
    let step = TAU / f64::from(points);
    let direction = FRAC_PI_2 * (1. + f64::from(direction) * 0.01);
    let delta = (direction - (0.5 * step + FRAC_PI_2)).abs();
    let maximum = strength * 0.01 * 0.5;
    let handle = if delta < FRAC_PI_2 {
        (maximum * ((0.5 * step).sin() / delta.cos()) as f32).min(maximum)
    } else {
        maximum
    } * 1.2;
    let twist = f64::from(twist * 0.01 * 0.5) * PI;
    let mut start = -FRAC_PI_2;
    let mut end = start + step;
    let mut start_handle = start + direction - twist;
    let mut end_handle = end - direction - twist;
    for _ in 0..points {
        let a = polar(0.5, start);
        let b = polar(0.5, end);
        outline.cubic([
            a,
            offset(a, handle, start_handle),
            offset(b, handle, end_handle),
            b,
        ]);
        start += step;
        end += step;
        start_handle += step;
        end_handle += step;
    }
}

fn gear(outline: &mut Outline<'_>, teeth: i32, inner: f32, middle: f32, bevel: f32) {
    let inner = inner * 0.01 * 0.5;
    let middle = middle * 0.01;
    let bevel = bevel * 0.01 * 0.5;
    let step = TAU / f64::from(teeth);
    let mut angle = FRAC_PI_2;
    for _ in 0..teeth {
        let left = angle - 0.25 * step;
        let right = angle + 0.25 * step;
        let outer_left = polar(0.5, left);
        let outer_right = polar(0.5, right);
        let inner_left = polar(inner, left);
        let inner_right = polar(inner, right);
        outline.polygon([
            inner_left,
            interpolate(inner_left, outer_left, middle),
            interpolate(outer_left, outer_right, bevel),
            interpolate(outer_right, outer_left, bevel),
            interpolate(inner_right, outer_right, middle),
            inner_right,
        ]);
        angle += step;
    }
}

fn flower(
    outline: &mut Outline<'_>,
    petals: i32,
    inner: f32,
    inner_tension: f32,
    outer_tension: f32,
) {
    let step = TAU / f64::from(petals);
    let inner = inner * 0.01 * 0.5;
    let inner_handle = inner_tension * 0.01 * (0.5 * step).tan() as f32;
    let outer_handle = 2. * outer_tension * 0.01 * 0.5 * (0.5 * step).sin() as f32;
    let mut angle = -FRAC_PI_2;
    for _ in 0..petals {
        let a = angle - 0.5 * step;
        let b = angle + 0.5 * step;
        let start = polar(inner, a);
        let end = polar(inner, b);
        let tip = polar(0.5, angle);
        outline.cubic([
            start,
            offset(start, inner_handle, a + FRAC_PI_2),
            offset(tip, outer_handle, angle - FRAC_PI_2),
            tip,
        ]);
        outline.cubic([
            tip,
            offset(tip, outer_handle, angle + FRAC_PI_2),
            offset(end, inner_handle, b - FRAC_PI_2),
            end,
        ]);
        angle += step;
    }
}

fn arrow(outline: &mut Outline<'_>, kind: CanvasPrimitive, width: f32, half_thickness: f32) {
    let head_width = if kind == CanvasPrimitive::DoubleArrow {
        width * 0.5
    } else {
        width
    };
    let head = 0.5 - head_width;
    let upper = -0.5 + (0.5 - half_thickness);
    let lower = 0.5 - (0.5 - half_thickness);
    outline.polygon([[0.5, 0.], [head, -0.5], [head, upper]]);

    if kind == CanvasPrimitive::DoubleArrow {
        let tail = head_width - 0.5;
        outline.polygon([
            [tail, upper],
            [tail, -0.5],
            [-0.5, 0.],
            [tail, 0.5],
            [tail, lower],
        ]);
    } else {
        outline.vertex([-0.5, upper]);
        if kind == CanvasPrimitive::NotchedArrow {
            outline.vertex([-0.5 + half_thickness * width, 0.]);
        }
        outline.vertex([-0.5, lower]);
    }
    outline.polygon([[head, lower], [head, 0.5]]);
}
