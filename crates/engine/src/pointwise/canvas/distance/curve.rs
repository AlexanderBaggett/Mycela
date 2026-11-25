//! Distance-field evaluation for retained Canvas curves.
use super::{Bounds, Edge, PERIOD, Point, combine, dot, filled, subtract};
use crate::pointwise::canvas::shape::{
    CubicCoefficients, DistanceSegment, Outline, quantize_point, split_cubic,
};
use crate::{ImageError, PointwiseError};

#[derive(Clone, Copy)]
pub(super) struct Segment {
    controls: [Point; 4],
    cubic: CubicCoefficients,
    pub(super) bounds: Bounds,
    corners: [bool; 2],
    rounded: [bool; 2],
    pub(super) straight: bool,
    convex: bool,
    depth: u8,
    radius: f64,
}

impl Segment {
    pub(super) fn new(
        controls: [Point; 4],
        corners: [bool; 2],
        rounded: [bool; 2],
        depth: u8,
        radius: f64,
    ) -> Self {
        let [a, b, c, d] = controls;
        let chord = Edge { start: a, end: d };
        let tolerance = if rounded == [true; 2] { 0.001 } else { 0.0001 };
        let straight = corners == [true; 2]
            || (chord.distance_squared(b) < tolerance && chord.distance_squared(c) < tolerance);
        let convex = outside_tangent(a, b, d) && outside_tangent(d, c, a);
        let bounds = Bounds::edge(chord).union(Bounds::edge(Edge { start: b, end: c }));
        Self {
            controls,
            cubic: CubicCoefficients::new(controls),
            bounds,
            corners,
            rounded,
            straight,
            convex,
            depth,
            radius,
        }
    }

    pub(super) fn chord(self) -> Edge {
        Edge {
            start: self.controls[0],
            end: self.controls[3],
        }
    }

    pub(super) fn tile_copy(self, [x, y]: Point) -> Self {
        let controls = self
            .controls
            .map(|[u, v]| [u + x, v + y].map(quantize_point));
        Self::new(controls, self.corners, self.rounded, 3, self.radius)
    }

    pub(super) fn position(self, t: f64) -> Point {
        self.cubic.position(t)
    }

    pub(super) fn with_line_controls(self) -> Self {
        let chord = self.chord();
        Self::new(
            [chord.start, chord.at(0.25), chord.at(0.75), chord.end],
            [true; 2],
            self.rounded,
            self.depth,
            self.radius,
        )
    }

    pub(super) fn split(self) -> [Self; 2] {
        self.split_at(0.5, None)
    }

    pub(super) fn split_at(self, parameter: f64, point: Option<Point>) -> [Self; 2] {
        let [mut left, mut right] = split_cubic(self.controls, parameter)
            .map(|points| points.map(|point| point.map(quantize_point)));
        if let Some(point) = point {
            left[3] = point;
            right[0] = point;
            if self.corners == [true; 2] {
                for controls in [&mut left, &mut right] {
                    let edge = Edge {
                        start: controls[0],
                        end: controls[3],
                    };
                    controls[1] = edge.at(0.25).map(quantize_point);
                    controls[2] = edge.at(0.75).map(quantize_point);
                }
            }
        }
        [
            Self::new(
                left,
                [self.corners[0], false],
                [self.rounded[0], false],
                self.depth + 1,
                self.radius,
            ),
            Self::new(
                right,
                [false, self.corners[1]],
                [false, self.rounded[1]],
                self.depth + 1,
                self.radius,
            ),
        ]
    }

    fn use_endpoints(self, sample: Point) -> bool {
        let [a, b, c, d] = self.controls;
        self.convex
            && !((outside_tangent(a, b, sample) || outside_tangent(a, d, sample))
                && (outside_tangent(d, c, sample) || outside_tangent(d, a, sample)))
    }

    pub(super) fn nearest_parameter(self, sample: Point) -> f64 {
        if self.use_endpoints(sample) {
            let [a, _, _, b] = self.controls;
            return if squared_distance(sample, a) < squared_distance(sample, b) {
                0.
            } else {
                1.
            };
        }
        let [low, high] = self.cubic.nearest_bracket(sample);
        (low + high) * 0.5
    }

    fn distance_squared(self, sample: Point) -> f64 {
        let chord = self.chord();
        if self.straight {
            return chord.distance_squared(sample);
        }
        if self.use_endpoints(sample) {
            return squared_distance(sample, chord.start).min(squared_distance(sample, chord.end));
        }
        let [low, high] = self.cubic.nearest_bracket(sample);
        let middle = self.cubic.position((low + high) * 0.5);
        [low, high].into_iter().fold(f64::MAX, |best, t| {
            best.min(
                Edge {
                    start: self.cubic.position(t),
                    end: middle,
                }
                .distance_squared(sample),
            )
        })
    }
}

fn squared_distance(a: Point, b: Point) -> f64 {
    let delta = subtract(a, b);
    dot(delta, delta)
}

fn outside_tangent([x, y]: Point, [u, v]: Point, [p, q]: Point) -> bool {
    x * x + y * y + u * p - x * (u + p) + v * q - y * (v + q) > 0.
}

fn disjoint_copies(segments: &[DistanceSegment]) -> bool {
    // If a closed path overlaps one of its translations, its exposed
    // boundary must be resolved before measuring the distance.
    (0..2).all(|axis| {
        let (min, max) = segments
            .iter()
            .flat_map(|segment| segment.controls.map(|point| point[axis]))
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(min, max), value| {
                (min.min(value), max.max(value))
            });
        max - min < PERIOD
    })
}

fn periodic_segments<E>(
    outlines: &[Outline],
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<Vec<Segment>>, PointwiseError<E>> {
    let mut fills = outlines.iter().filter(|outline| outline.stroke.is_none());
    if let Some(fill) = fills.next() {
        let Some(curve) = &fill.curve else {
            return Ok(None);
        };
        if !matches!(fill.combine, 2 | 6) || !disjoint_copies(curve) {
            return Ok(None);
        }
        // The retained mixed path covers Normal Combine with one closed
        // contour. Other mixed operations retain the line-based fallback.
        if outlines.len() > 1
            && (fills.next().is_some() || outlines.iter().any(|outline| outline.combine != 6))
        {
            return Ok(None);
        }
    }

    let mut strokes = Vec::new();
    let mut boundary = Vec::new();
    for outline in outlines {
        crate::pointwise::check_render_cancelled(cancelled)?;
        let Some(segments) = &outline.curve else {
            return Ok(None);
        };
        if !(0..=7).contains(&outline.combine)
            || segments.is_empty()
            || segments.iter().any(|segment| {
                segment
                    .controls
                    .iter()
                    .flatten()
                    .any(|value| !value.is_finite())
            })
        {
            return Ok(None);
        }
        let (periodic, radius) = match &outline.stroke {
            Some(stroke) => (&mut strokes, stroke.radius),
            None => (&mut boundary, 0.),
        };
        for y in [-PERIOD, 0., PERIOD] {
            for x in [-PERIOD, 0., PERIOD] {
                for &DistanceSegment {
                    controls,
                    corners,
                    rounded,
                } in segments
                {
                    let controls = controls.map(|[u, v]| [u + x, v + y].map(quantize_point));
                    // The source's per-tile copy resets subdivision depth, including
                    // pieces marked as straight before they reach the tile worker.
                    periodic.push(Segment::new(controls, corners, rounded, 3, radius));
                }
            }
        }
    }
    if !boundary.is_empty() && !strokes.is_empty() {
        // Genetica measures the stroke only along centerline pieces outside
        // the closed fill, in either stacking order.
        let Some(clipped) = combine::clip_strokes(&strokes, &boundary, cancelled)? else {
            return Ok(None);
        };
        strokes = clipped;
    }
    strokes.extend(boundary);
    Ok((!strokes.is_empty()).then_some(strokes))
}

fn signed_distance(segments: &[Segment], sample: Point, covered_by_fill: bool) -> f64 {
    let mut boundary = f64::MAX;
    let mut nearest_center = f64::MAX;
    let mut stroke_edge = f64::MAX;
    let mut covered = covered_by_fill;
    for segment in segments {
        let lower_bound = segment.bounds.distance_squared(sample);
        if segment.radius == 0. {
            if lower_bound < boundary {
                boundary = boundary.min(segment.distance_squared(sample));
            }
        } else if lower_bound < nearest_center * nearest_center
            || (!covered && lower_bound <= segment.radius * segment.radius)
        {
            let center = segment.distance_squared(sample).sqrt();
            covered |= center <= segment.radius;
            // A wider stroke can cover this point while a closer centerline
            // supplies the feather distance.
            if center < nearest_center {
                nearest_center = center;
                stroke_edge = (center - segment.radius).abs();
            }
        }
    }
    let distance = boundary.sqrt().min(stroke_edge);
    if covered { distance } else { -distance }
}

#[derive(Clone, Copy)]
struct Tile {
    origin: [usize; 2],
    size: [usize; 2],
}

impl Tile {
    fn quarters(self) -> [Self; 4] {
        let [x, y] = self.origin;
        let [width, height] = self.size;
        let [left, top] = [width / 2, height / 2];
        let [right, bottom] = [width - left, height - top];
        [
            Self {
                origin: [x, y],
                size: [left, top],
            },
            Self {
                origin: [x + left, y],
                size: [right, top],
            },
            Self {
                origin: [x, y + top],
                size: [left, bottom],
            },
            Self {
                origin: [x + left, y + top],
                size: [right, bottom],
            },
        ]
    }

    fn subdivision_depth(self) -> u8 {
        let [width, height] = self.size;
        if width <= 16 || height <= 16 {
            return 3;
        }
        let shortest = width.min(height);
        6 - u8::from(shortest <= 128) - u8::from(shortest <= 44)
    }
}

struct Field<'a> {
    samples: &'a mut [f32],
    coverage: &'a [bool],
    width: usize,
    grid: f64,
    scale: f64,
}

impl Field<'_> {
    fn position(&self, i: usize) -> f64 {
        0.5 * self.grid + i as f64 * self.grid
    }

    fn tile<E>(
        &mut self,
        tile: Tile,
        segments: &[Segment],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), PointwiseError<E>> {
        crate::pointwise::check_render_cancelled(cancelled)?;
        let [width, height] = tile.size;
        if width <= 4 || height <= 4 {
            let [x, y] = tile.origin;
            for row in y..y + height {
                let py = self.position(row);
                for col in x..x + width {
                    let point = [self.position(col), py];
                    let index = row * self.width + col;
                    let signed = signed_distance(segments, point, self.coverage[index]);
                    self.samples[index] = (0.5 + signed * self.scale) as f32;
                }
            }
            return Ok(());
        }

        let depth = tile.subdivision_depth();
        let mut split = Vec::new();
        if segments.iter().any(|segment| segment.depth < depth) {
            let [dx, dy] = tile.size.map(|n| n as f64 * self.grid * 0.5);
            for &segment in segments {
                let Bounds {
                    min: [x0, y0],
                    max: [x1, y1],
                } = segment.bounds;
                if segment.depth < depth && (x1 - x0 > dx || y1 - y0 > dy) {
                    split.extend(segment.split());
                } else {
                    split.push(segment);
                }
            }
        }
        let segments = if split.is_empty() { segments } else { &split };
        let mut distances = Vec::with_capacity(segments.len());
        let mut candidates = Vec::with_capacity(segments.len());
        for child in tile.quarters() {
            let span = child.size.map(|n| n as f64 * self.grid);
            let center = [0, 1].map(|axis| self.position(child.origin[axis]) + span[axis] * 0.5);
            distances.clear();
            distances.extend(
                segments
                    .iter()
                    .map(|segment| segment.distance_squared(center).sqrt()),
            );
            let cutoff =
                distances.iter().copied().fold(f64::MAX, f64::min) + dot(span, span).sqrt();
            candidates.clear();
            candidates.extend(segments.iter().zip(&distances).filter_map(
                |(&segment, &distance)| (distance - segment.radius < cutoff).then_some(segment),
            ));
            self.tile(child, &candidates, cancelled)?;
        }
        Ok(())
    }
}

pub(super) fn render<E>(
    outlines: &[Outline],
    coverage: &[bool],
    width: usize,
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<Vec<f32>>, PointwiseError<E>> {
    let segments = if outlines.len() > 1 && outlines.iter().all(|outline| outline.stroke.is_none())
    {
        let Some(boundary) = combine::prepare(outlines, cancelled)? else {
            return Ok(None);
        };
        let mut periodic = Vec::with_capacity(boundary.len() * 9);
        for y in [-PERIOD, 0., PERIOD] {
            for x in [-PERIOD, 0., PERIOD] {
                periodic.extend(boundary.iter().map(|segment| segment.tile_copy([x, y])));
            }
        }
        periodic
    } else {
        let Some(segments) = periodic_segments(outlines, cancelled)? else {
            return Ok(None);
        };
        segments
    };
    let grid = f64::from(100. / width as f32);
    let mut samples = filled(width.checked_mul(width).ok_or(ImageError::Overflow)?, 0.)?;
    let mut field = Field {
        samples: &mut samples,
        coverage,
        width,
        grid,
        scale: 2. / (width as f64 * grid),
    };
    // Genetica chooses its initial tile size from the machine processor count.
    // Use the same partition for preview and export regardless of scheduling.
    let processors = std::thread::available_parallelism().map_or(1, usize::from);
    let mut tiles = if processors == 1 {
        1
    } else {
        (processors as f64 * 2.5).sqrt().ceil() as usize
    };
    if width / tiles < 4 {
        tiles = 1;
    }
    let stride = width / tiles;
    for row in 0..tiles {
        for col in 0..tiles {
            let origin = [col * stride, row * stride];
            let size = [col, row].map(|index| {
                if index + 1 == tiles {
                    stride + width % tiles
                } else {
                    stride
                }
            });
            field.tile(Tile { origin, size }, &segments, cancelled)?;
        }
    }
    Ok(Some(samples))
}

#[cfg(test)]
mod tests {
    use crate::{
        CanvasPath, CanvasPathPoint, ImageGeometry, PointwiseFilter, PointwiseKind,
        PointwiseRenderEffects, PointwiseRenderOptions, ProceduralExecution, RenderCaptureSession,
    };

    #[test]
    fn normal_combine_clips_open_stroke_at_closed_fill_in_both_orders() {
        let path = |points: &[[f32; 2]], closed| {
            CanvasPath::from_points(
                points
                    .iter()
                    .copied()
                    .map(CanvasPathPoint::corner)
                    .collect(),
                closed,
            )
            .unwrap()
        };
        let stroke = path(&[[20., 40.5], [80., 40.5]], false);
        let fill = path(&[[25., 40.], [75., 40.], [75., 70.], [25., 70.]], true);
        let geometry = ImageGeometry::new(128, 128);
        let execution = ProceduralExecution::CallingThread;
        for order in [[stroke.clone(), fill.clone()], [fill, stroke]] {
            let filter = PointwiseFilter::<()>::new(PointwiseKind::Canvas).unwrap();
            let layer = filter.edit_add_canvas_layer().unwrap();
            for path in order {
                layer.edit_insert_path(path).unwrap();
            }
            let capture = RenderCaptureSession::new()
                .capture_node(&filter.node(), 0.)
                .unwrap();
            let mut capacity = |_| Ok(true);
            let mut preflight = |_, _| Ok(());
            let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut preflight);
            let export = capture
                .render_export_with_effects(
                    geometry,
                    execution,
                    PointwiseRenderOptions::new(false, false),
                    &mut effects,
                )
                .unwrap();
            let preview = capture
                .render_preview_with_effects(geometry, execution, &mut effects)
                .unwrap();
            for outcome in [export, preview] {
                assert!(outcome.caught_errors().is_empty());
                let image = outcome.image().unwrap();
                let transparency = image.materialize(3).unwrap().values();
                // Native Genetica exports 2,560 opaque samples in both orders.
                // The centerline lies inside the fill but its width crosses the edge.
                assert_eq!(transparency[50 * 128 + 64], 1.);
                assert_eq!(transparency[51 * 128 + 64], 0.);
                assert_eq!(transparency[50 * 128 + 28], 0.);
                assert_eq!(
                    transparency.iter().filter(|&&value| value == 0.).count(),
                    2_560,
                );
            }
        }
    }
}
