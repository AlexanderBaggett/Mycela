//! Periodic signed distance to the exposed boundaries of the combined shapes.
use super::{raster::filled, shape::Outline};
use crate::{ImageError, PointwiseError};

mod combine;
mod curve;

type Point = [f64; 2];
const PERIOD: f64 = 100.;

#[derive(Clone, Copy)]
struct Edge {
    start: Point,
    end: Point,
}

#[derive(Clone, Copy)]
struct Bounds {
    min: Point,
    max: Point,
}

impl Bounds {
    fn edge(edge: Edge) -> Self {
        Self {
            min: std::array::from_fn(|axis| edge.start[axis].min(edge.end[axis])),
            max: std::array::from_fn(|axis| edge.start[axis].max(edge.end[axis])),
        }
    }

    fn union(self, other: Self) -> Self {
        Self {
            min: std::array::from_fn(|axis| self.min[axis].min(other.min[axis])),
            max: std::array::from_fn(|axis| self.max[axis].max(other.max[axis])),
        }
    }

    fn overlaps(self, other: Self) -> bool {
        (0..2).all(|axis| self.max[axis] >= other.min[axis] && other.max[axis] >= self.min[axis])
    }

    fn translated(self, [x, y]: Point) -> Self {
        let shift = |[u, v]: Point| [u + x, v + y];
        Self {
            min: shift(self.min),
            max: shift(self.max),
        }
    }

    fn distance_squared(self, point: Point) -> f64 {
        let dx = (self.min[0] - point[0]).max(point[0] - self.max[0]).max(0.);
        let dy = (self.min[1] - point[1]).max(point[1] - self.max[1]).max(0.);
        dx * dx + dy * dy
    }
}

impl Edge {
    fn at(self, t: f64) -> Point {
        std::array::from_fn(|axis| self.start[axis] + t * (self.end[axis] - self.start[axis]))
    }

    fn direction(self) -> Point {
        subtract(self.end, self.start)
    }

    fn distance_squared(self, point: Point) -> f64 {
        let direction = self.direction();
        let relative = subtract(point, self.start);
        let projection = dot(relative, direction);
        let t = if projection <= 0. {
            0.
        } else {
            (projection / dot(direction, direction)).min(1.)
        };
        let delta = subtract(point, self.at(t));
        dot(delta, delta)
    }

    fn split_against(self, other: Self, cuts: &mut Vec<f64>) {
        let vector = self.direction();
        let other_vector = other.direction();
        let start = subtract(other.start, self.start);
        let denominator = cross(vector, other_vector);
        if denominator != 0. {
            let along_self = cross(start, other_vector) / denominator;
            let along_other = cross(start, vector) / denominator;
            if along_self > 0. && along_self < 1. && (0. ..=1.).contains(&along_other) {
                cuts.push(along_self);
            }
        } else if cross(start, vector) == 0. {
            let length = dot(vector, vector);
            for point in [other.start, other.end] {
                let along = dot(subtract(point, self.start), vector) / length;
                if along > 0. && along < 1. {
                    cuts.push(along);
                }
            }
        }
    }

    fn probes(self, from: f64, to: f64) -> [Point; 2] {
        let [dx, dy] = self.direction();
        let length = dx.hypot(dy);
        let epsilon = 1.0e-7_f64.min((to - from) * length * 0.01);
        let [x, y] = Self {
            start: self.at(from),
            end: self.at(to),
        }
        .at(0.5);
        let nx = -dy / length * epsilon;
        let ny = dx / length * epsilon;
        [[x + nx, y + ny], [x - nx, y - ny]]
    }
}

fn dot(a: Point, b: Point) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

fn cross(a: Point, b: Point) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}

fn subtract(a: Point, b: Point) -> Point {
    [a[0] - b[0], a[1] - b[1]]
}

struct Copy {
    edges: Vec<Edge>,
    bounds: Bounds,
}

impl Copy {
    fn contains(&self, [x, y]: Point) -> bool {
        if x < self.bounds.min[0]
            || x > self.bounds.max[0]
            || y < self.bounds.min[1]
            || y > self.bounds.max[1]
        {
            return false;
        }
        let mut inside = false;
        for edge in &self.edges {
            let [x0, y0] = edge.start;
            let [x1, y1] = edge.end;
            if ((y0 < y && y <= y1) || (y1 < y && y <= y0))
                && x0 + (y - y0) * (x1 - x0) / (y1 - y0) < x
            {
                inside = !inside;
            }
        }
        inside
    }
}

/// One closed outline in authored coordinates, prepared once for separation.
pub(super) struct FilledContour(Copy);

impl FilledContour {
    pub(super) fn new(points: &[[f64; 2]]) -> Option<Self> {
        let edges: Vec<_> = closed_edges(points).collect();
        let bounds = edges
            .iter()
            .copied()
            .map(Bounds::edge)
            .reduce(Bounds::union)?;
        Some(Self(Copy { edges, bounds }))
    }

    /// Tests filled area and shared edges against the current even-odd fill.
    pub(super) fn overlaps(&self, group: &[&Self]) -> bool {
        let candidate = &self.0;
        let group: Vec<_> = group
            .iter()
            .map(|contour| &contour.0)
            .filter(|other| candidate.bounds.overlaps(other.bounds))
            .collect();
        if group.is_empty() {
            return false;
        }
        let mut edges: Vec<_> = std::iter::once(candidate)
            .chain(group.iter().copied())
            .flat_map(|outline| outline.edges.iter().copied())
            .map(|edge| (Bounds::edge(edge), edge))
            .collect();
        edges.sort_unstable_by(|(a, _), (b, _)| a.min[0].total_cmp(&b.min[0]));
        let mut cuts = Vec::new();
        let mut crossings = Vec::new();
        for &(bounds, edge) in edges
            .iter()
            .filter(|(bounds, _)| bounds.overlaps(candidate.bounds))
        {
            cuts.clear();
            cuts.extend([0., 1.]);
            for &(other_bounds, other) in &edges {
                if other_bounds.min[0] > bounds.max[0] {
                    break;
                }
                if bounds.overlaps(other_bounds) {
                    edge.split_against(other, &mut cuts);
                }
            }
            cuts.sort_unstable_by(f64::total_cmp);
            for pair in cuts.windows(2) {
                let [a, b] = [pair[0], pair[1]];
                if a >= b {
                    continue;
                }
                let ray_axis = usize::from(edge.start[0] != edge.end[0]);
                let [left, right] = edge.probes(a, b).map(|point| {
                    let group_contains = compound_contains(&group, point, ray_axis, &mut crossings);
                    (candidate.contains(point), group_contains)
                });
                if left == (true, true)
                    || right == (true, true)
                    || (left.0 != right.0 && left.1 != right.1)
                {
                    return true;
                }
            }
        }
        false
    }
}

fn compound_contains(
    group: &[&Copy],
    point: Point,
    ray_axis: usize,
    crossings: &mut Vec<f64>,
) -> bool {
    if let [only] = group {
        return only.contains(point);
    }
    let transverse = 1 - ray_axis;
    crossings.clear();
    for edge in group.iter().flat_map(|part| &part.edges) {
        let [start, end] = [edge.start[transverse], edge.end[transverse]];
        if (start < point[transverse] && point[transverse] <= end)
            || (end < point[transverse] && point[transverse] <= start)
        {
            let t = (point[transverse] - start) / (end - start);
            crossings.push(edge.at(t)[ray_axis]);
        }
    }
    // Paired edges count as one crossing. Use the smaller count when shared
    // borders produce different counts on the two sides of the ray.
    crossings.sort_unstable_by(f64::total_cmp);
    crossings.dedup_by(|a, b| (*a - *b).abs() < 1.0e-7);
    let before = crossings.partition_point(|&crossing| crossing < point[ray_axis]);
    before.min(crossings.len() - before) % 2 == 1
}

fn closed_edges(points: &[Point]) -> impl Iterator<Item = Edge> + '_ {
    points
        .iter()
        .zip(points.iter().cycle().skip(1))
        .filter_map(|(&start, &end)| (start != end).then_some(Edge { start, end }))
}

struct Shape {
    copies: Vec<Copy>,
    combine: i32,
}

impl Shape {
    fn new<E>(outline: &Outline) -> Result<Self, PointwiseError<E>> {
        if !matches!(outline.combine, 0..=2 | 6) {
            return Err(PointwiseError::UnsupportedCapability(
                "Canvas distance-field combination",
            ));
        }
        let edges: Vec<_> = outline
            .contours
            .iter()
            .flat_map(|contour| closed_edges(contour))
            .collect();
        let Some(bounds) = edges
            .iter()
            .copied()
            .map(Bounds::edge)
            .reduce(Bounds::union)
        else {
            return Ok(Self {
                copies: Vec::new(),
                combine: outline.combine,
            });
        };
        let mut copies = Vec::new();
        for x in [-PERIOD, 0., PERIOD] {
            for y in [-PERIOD, 0., PERIOD] {
                let shift = |[u, v]: Point| [u + x, v + y];
                copies.push(Copy {
                    edges: edges
                        .iter()
                        .map(|edge| Edge {
                            start: shift(edge.start),
                            end: shift(edge.end),
                        })
                        .collect(),
                    bounds: Bounds {
                        min: shift(bounds.min),
                        max: shift(bounds.max),
                    },
                });
            }
        }
        Ok(Self {
            copies,
            combine: outline.combine,
        })
    }
}

fn contains(shapes: &[Shape], point: Point) -> bool {
    let point = point.map(|axis| axis.rem_euclid(PERIOD));
    let mut inside = false;
    for shape in shapes {
        let covers = shape.copies.iter().any(|copy| copy.contains(point));
        inside = match shape.combine {
            0 => inside && !covers,
            1 => inside && covers,
            2 => inside != covers,
            _ => inside || covers,
        };
    }
    inside
}

fn boundaries<E>(
    shapes: &[Shape],
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<Edge>, PointwiseError<E>> {
    let mut edges: Vec<_> = shapes
        .iter()
        .flat_map(|shape| &shape.copies)
        .flat_map(|copy| copy.edges.iter().copied())
        .map(|edge| (Bounds::edge(edge), edge))
        .collect();
    edges.sort_unstable_by(|(a, _), (b, _)| a.min[0].total_cmp(&b.min[0]));
    let mut cuts = Vec::new();
    let mut boundary = Vec::new();
    for &(bounds, edge) in &edges {
        crate::pointwise::check_render_cancelled(cancelled)?;
        cuts.clear();
        cuts.extend([0., 1.]);
        for &(other_bounds, other) in &edges {
            if other_bounds.min[0] > bounds.max[0] {
                break;
            }
            if bounds.overlaps(other_bounds) {
                edge.split_against(other, &mut cuts);
            }
        }
        cuts.sort_unstable_by(f64::total_cmp);
        let [dx, dy] = edge.direction();
        let normal_length = dx.hypot(dy);
        for pair in cuts.windows(2) {
            let [a, b] = [pair[0], pair[1]];
            if a == b {
                continue;
            }
            let segment = Edge {
                start: edge.at(a),
                end: edge.at(b),
            };
            let [x, y] = segment.at(0.5);
            // Probe either side within this interval, even where intersections
            // split it into a very short edge.
            let epsilon = 1.0e-7_f64.min((b - a) * normal_length * 0.01);
            let nx = -dy / normal_length * epsilon;
            let ny = dx / normal_length * epsilon;
            if contains(shapes, [x + nx, y + ny]) != contains(shapes, [x - nx, y - ny]) {
                boundary.push(segment);
            }
        }
    }
    Ok(boundary)
}

enum Edges<'a> {
    Leaf(&'a [Edge]),
    Branch(Box<[DistanceTree<'a>; 2]>),
}

struct DistanceTree<'a> {
    bounds: Bounds,
    edges: Edges<'a>,
}

impl<'a> DistanceTree<'a> {
    fn new(edges: &'a mut [Edge]) -> Option<Self> {
        let bounds = edges
            .iter()
            .copied()
            .map(Bounds::edge)
            .reduce(Bounds::union)?;
        let edges = if edges.len() <= 8 {
            Edges::Leaf(edges)
        } else {
            let axis = usize::from(bounds.max[1] - bounds.min[1] > bounds.max[0] - bounds.min[0]);
            let half = edges.len() / 2;
            edges.select_nth_unstable_by(half, |a, b| {
                (a.start[axis] + a.end[axis]).total_cmp(&(b.start[axis] + b.end[axis]))
            });
            let (a, b) = edges.split_at_mut(half);
            Edges::Branch(Box::new([Self::new(a)?, Self::new(b)?]))
        };
        Some(Self { bounds, edges })
    }

    fn nearest(&self, point: Point, best: f64) -> f64 {
        if self.bounds.distance_squared(point) >= best {
            return best;
        }
        match &self.edges {
            Edges::Leaf(edges) => edges
                .iter()
                .fold(best, |best, edge| best.min(edge.distance_squared(point))),
            Edges::Branch(branch) => {
                let first = usize::from(
                    branch[1].bounds.distance_squared(point)
                        < branch[0].bounds.distance_squared(point),
                );
                let best = branch[first].nearest(point, best);
                branch[1 - first].nearest(point, best)
            }
        }
    }
}

pub(super) fn render<E>(
    outlines: &[Outline],
    coverage: &[bool],
    width: usize,
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<f32>, PointwiseError<E>> {
    if coverage.len() >= width.saturating_mul(width)
        && let Some(field) = curve::render(outlines, coverage, width, cancelled)?
    {
        return Ok(field);
    }
    let shapes = outlines
        .iter()
        .filter(|outline| outline.stroke.is_none())
        .map(Shape::new)
        .collect::<Result<Vec<_>, _>>()?;
    let mut edges = boundaries(&shapes, cancelled)?;
    let tree = DistanceTree::new(&mut edges);
    let mut centerlines = stroke_edges(outlines)?;
    let strokes: Vec<_> = centerlines
        .iter_mut()
        .filter_map(|stroke| DistanceTree::new(&mut stroke.edges).map(|tree| (stroke.radius, tree)))
        .collect();
    let mut field = filled(width.checked_mul(width).ok_or(ImageError::Overflow)?, 0.)?;
    let grid = f64::from(100. / width as f32);
    let scale = 2. / (width as f64 * grid);
    for (row, line) in field.chunks_exact_mut(width).enumerate() {
        crate::pointwise::check_render_cancelled(cancelled)?;
        let y = (row as f64 + 0.5) * grid;
        for (col, value) in line.iter_mut().enumerate() {
            let point = [(col as f64 + 0.5) * grid, y];
            let index = row * width + col;
            let inside = coverage
                .get(index)
                .copied()
                .unwrap_or_else(|| contains(&shapes, point));
            let distance = tree.as_ref().map_or(f64::INFINITY, |tree| {
                tree.nearest(point, f64::INFINITY).sqrt()
            });
            let distance = signed_distance(&strokes, point, inside, distance);
            *value = (distance * scale + 0.5) as f32;
        }
    }
    Ok(field)
}

struct StrokeEdges {
    radius: f64,
    edges: Vec<Edge>,
}

fn stroke_edges<E>(outlines: &[Outline]) -> Result<Vec<StrokeEdges>, PointwiseError<E>> {
    let mut groups: Vec<StrokeEdges> = Vec::new();
    for (index, outline) in outlines.iter().enumerate() {
        let Some(stroke) = &outline.stroke else {
            continue;
        };
        if index != 0 && outline.combine != 6 {
            return Err(PointwiseError::UnsupportedCapability(
                "Canvas stroke combination",
            ));
        }
        let group = groups
            .iter()
            .position(|group| group.radius == stroke.radius)
            .unwrap_or_else(|| {
                groups.push(StrokeEdges {
                    radius: stroke.radius,
                    edges: Vec::new(),
                });
                groups.len() - 1
            });
        let edges = &mut groups[group].edges;
        for line in &stroke.lines {
            for endpoints in line.windows(2) {
                let [start, end] = [endpoints[0], endpoints[1]];
                let tile = std::array::from_fn::<_, 2, _>(|axis| {
                    ((start[axis] + end[axis]) / (2. * PERIOD)).floor() * PERIOD
                });
                for x in [-PERIOD, 0., PERIOD] {
                    for y in [-PERIOD, 0., PERIOD] {
                        let shift = |[u, v]: Point| [u - tile[0] + x, v - tile[1] + y];
                        edges.push(Edge {
                            start: shift(start),
                            end: shift(end),
                        });
                    }
                }
            }
        }
    }
    Ok(groups)
}

fn signed_distance(
    strokes: &[(f64, DistanceTree<'_>)],
    point: Point,
    inside_fill: bool,
    mut boundary: f64,
) -> f64 {
    let mut inside = inside_fill;
    let mut nearest_center = f64::INFINITY;
    let mut stroke_edge = f64::INFINITY;
    // Coverage considers every radius. Outside a fill, the feather distance uses
    // the edge of the stroke whose centerline is nearest to the sample.
    for (radius, tree) in strokes {
        let center = tree.nearest(point, f64::INFINITY).sqrt();
        if *radius == 0. {
            boundary = boundary.min(center);
        } else if !inside_fill && center < nearest_center {
            nearest_center = center;
            stroke_edge = (center - radius).abs();
        }
        inside |= center <= *radius;
    }
    let distance = boundary.min(stroke_edge);
    let distance = if distance.is_finite() {
        distance
    } else {
        PERIOD
    };
    if inside { distance } else { -distance }
}
