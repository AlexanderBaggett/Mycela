//! Split and classify retained closed boundaries before tiling the distance field.
use super::{Bounds, Edge, PERIOD, Point, cross, curve::Segment, dot, subtract};
use crate::PointwiseError;
use crate::pointwise::canvas::shape::{Outline, quantize_point};

const POINT_EPSILON: f64 = 1.0e-5;
const PARALLEL_EPSILON: f64 = 1.0e-6;

pub(super) fn prepare<E>(
    outlines: &[Outline],
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<Vec<Segment>>, PointwiseError<E>> {
    let mut inputs = Vec::new();
    for outline in outlines {
        let Some(segments) = &outline.curve else {
            return Ok(None);
        };
        if outline.stroke.is_some()
            || !matches!(outline.combine, 0..=2 | 6)
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
        inputs.push((
            outline.combine,
            segments
                .iter()
                .map(|segment| {
                    Segment::new(segment.controls, segment.corners, segment.rounded, 3, 0.)
                })
                .collect::<Vec<_>>(),
        ));
    }
    let bounds = inputs
        .iter()
        .flat_map(|(_, segments)| segments.iter().map(|segment| segment.bounds))
        .reduce(Bounds::union);
    if bounds.is_none_or(|bounds| (0..2).any(|axis| bounds.max[axis] - bounds.min[axis] >= PERIOD))
    {
        if inputs.iter().all(|(operation, _)| *operation == 6) {
            return periodic_union(inputs, cancelled);
        }
        return Ok(None);
    }
    merge(inputs, cancelled)
}

fn merge<E>(
    inputs: impl IntoIterator<Item = (i32, Vec<Segment>)>,
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<Vec<Segment>>, PointwiseError<E>> {
    let mut boundary = Vec::new();
    for (operation, incoming) in inputs {
        crate::pointwise::check_render_cancelled(cancelled)?;
        let keeps_both = operation == 2;
        let Some(mut retained) = clip(
            &boundary,
            &incoming,
            |inside| keeps_both || inside == (operation == 1),
            cancelled,
        )?
        else {
            return Ok(None);
        };
        let added = match operation {
            0 => clip(&incoming, &boundary, |inside| inside, cancelled)?,
            1 | 2 | 6 => clip(
                &incoming,
                &boundary,
                |inside| keeps_both || inside == (operation == 1),
                cancelled,
            )?,
            _ => unreachable!(),
        };
        let Some(added) = added else {
            return Ok(None);
        };
        retained.extend(added);
        boundary = retained;
    }
    Ok((!boundary.is_empty()).then_some(boundary))
}

struct UnionOutline {
    segments: Vec<Segment>,
    bounds: Bounds,
    offset: Point,
}

// A union can be combined in separate neighborhoods when every overlapping
// pair agrees on its translation and each neighborhood fits within one period.
fn periodic_union<E>(
    inputs: Vec<(i32, Vec<Segment>)>,
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<Vec<Segment>>, PointwiseError<E>> {
    let outlines: Vec<_> = inputs
        .into_iter()
        .filter_map(|(_, segments)| {
            let bounds = segments
                .iter()
                .map(|segment| segment.bounds)
                .reduce(Bounds::union)?;
            let offset = std::array::from_fn(|axis| -(bounds.min[axis] / PERIOD).floor() * PERIOD);
            Some(UnionOutline {
                segments,
                bounds: bounds.translated(offset),
                offset,
            })
        })
        .collect();
    let mut placements = vec![None; outlines.len()];
    let mut boundary = Vec::new();
    for seed in 0..outlines.len() {
        crate::pointwise::check_render_cancelled(cancelled)?;
        if placements[seed].is_some() {
            continue;
        }
        placements[seed] = Some([0.; 2]);
        let mut component = vec![seed];
        let mut bounds = outlines[seed].bounds;
        let mut next = 0;
        while let Some(&index) = component.get(next) {
            crate::pointwise::check_render_cancelled(cancelled)?;
            next += 1;
            let origin = placements[index].unwrap();
            for (other, outline) in outlines.iter().enumerate() {
                for dy in [-PERIOD, 0., PERIOD] {
                    for dx in [-PERIOD, 0., PERIOD] {
                        let shift = [dx, dy];
                        if !outlines[index]
                            .bounds
                            .overlaps(outline.bounds.translated(shift))
                        {
                            continue;
                        }
                        let offset = [origin[0] + dx, origin[1] + dy];
                        if let Some(existing) = placements[other] {
                            if existing != offset {
                                // A component winding across the tile requires clipping
                                // against its own translated boundary.
                                return Ok(None);
                            }
                            continue;
                        }
                        placements[other] = Some(offset);
                        component.push(other);
                        bounds = bounds.union(outline.bounds.translated(offset));
                    }
                }
            }
        }
        if (0..2).any(|axis| bounds.max[axis] - bounds.min[axis] >= PERIOD) {
            return Ok(None);
        }
        component.sort_unstable();
        let contours = component.into_iter().map(|index| {
            let outline = &outlines[index];
            let placement = placements[index].unwrap();
            let offset = std::array::from_fn(|axis| outline.offset[axis] + placement[axis]);
            (
                6,
                outline
                    .segments
                    .iter()
                    .map(|segment| {
                        if offset == [0.; 2] {
                            *segment
                        } else {
                            segment.tile_copy(offset)
                        }
                    })
                    .collect(),
            )
        });
        let Some(merged) = merge(contours, cancelled)? else {
            return Ok(None);
        };
        boundary.extend(merged);
    }
    Ok((!boundary.is_empty()).then_some(boundary))
}

struct Chords {
    bounds: Bounds,
    edges: Vec<(Bounds, Edge)>,
    straight: bool,
}

fn flatten(segment: Segment, depth: u8, output: &mut Vec<(Bounds, Edge)>) {
    if segment.straight || depth == 16 {
        let edge = segment.chord();
        if edge.start != edge.end {
            output.push((Bounds::edge(edge), edge));
        }
        return;
    }
    for half in segment.split() {
        flatten(half, depth + 1, output);
    }
}

pub(super) fn clip_strokes<E>(
    strokes: &[Segment],
    fill: &[Segment],
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<Vec<Segment>>, PointwiseError<E>> {
    clip(strokes, fill, |inside| !inside, cancelled)
}

fn clip<E>(
    measured: &[Segment],
    other: &[Segment],
    keep: impl Fn(bool) -> bool,
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<Vec<Segment>>, PointwiseError<E>> {
    let other: Vec<_> = other
        .iter()
        .map(|&segment| {
            let mut edges = Vec::new();
            flatten(segment, 0, &mut edges);
            Chords {
                bounds: segment.bounds,
                edges,
                straight: segment.straight,
            }
        })
        .collect();

    let mut exposed = Vec::new();
    let mut pieces = Vec::new();
    let mut next = Vec::new();
    for &segment in measured {
        crate::pointwise::check_render_cancelled(cancelled)?;
        pieces.clear();
        pieces.push(segment);
        for other in &other {
            next.clear();
            for &piece in &pieces {
                // The straight/straight path refreshes the quarter-chord
                // controls before testing coincidence with the other edge.
                let piece = if piece.straight && other.straight {
                    piece.with_line_controls()
                } else {
                    piece
                };
                if split_against(piece, other, &mut next).is_none() {
                    return Ok(None);
                }
            }
            std::mem::swap(&mut pieces, &mut next);
        }
        for &piece in &pieces {
            let p = piece.position(0.5);
            if keep(contains(&other, p)) {
                exposed.push(piece);
            }
        }
    }
    Ok(Some(exposed))
}

fn contains(shape: &[Chords], [x, y]: Point) -> bool {
    shape
        .iter()
        .flat_map(|part| &part.edges)
        .fold(false, |inside, (_, edge)| {
            let [x0, y0] = edge.start;
            let [x1, y1] = edge.end;
            let crosses = ((y0 < y && y <= y1) || (y1 < y && y <= y0))
                && x0 + (y - y0) * (x1 - x0) / (y1 - y0) < x;
            inside ^ crosses
        })
}

fn split_against(mut segment: Segment, other: &Chords, pieces: &mut Vec<Segment>) -> Option<()> {
    if segment.bounds.overlaps(other.bounds) {
        let mut flat = Vec::new();
        flatten(segment, 0, &mut flat);
        let mut intersections = Vec::new();
        for (bounds, chord) in flat {
            let mut on_chord = Vec::new();
            for &(candidate, edge) in &other.edges {
                if bounds.overlaps(candidate)
                    && let Some(t) = intersect(chord, edge)?
                {
                    on_chord.push(t);
                }
            }
            on_chord.sort_unstable_by(f64::total_cmp);
            intersections.extend(on_chord.into_iter().map(|t| chord.at(t)));
        }
        for point in intersections {
            let chord = segment.chord();
            if near(point, chord.start) || near(point, chord.end) {
                continue;
            }
            let parameter = segment.nearest_parameter(point);
            let point = point.map(quantize_point);
            let [left, right] = segment.split_at(parameter, Some(point));
            if left.chord().start != left.chord().end && right.chord().start != right.chord().end {
                pieces.push(left);
                segment = right;
            }
        }
    }
    pieces.push(segment);
    Some(())
}

fn near(a: Point, b: Point) -> bool {
    subtract(a, b)
        .into_iter()
        .all(|difference| difference.abs() <= POINT_EPSILON)
}

fn intersect(a: Edge, b: Edge) -> Option<Option<f64>> {
    let u = a.direction();
    let v = b.direction();
    let offset = subtract(b.start, a.start);
    let denominator = cross(u, v);
    let from_a = cross(offset, v);
    let from_b = cross(offset, u);
    if denominator.abs() > PARALLEL_EPSILON {
        let t = from_a / denominator;
        let s = from_b / denominator;
        return Some(((0. ..=1.).contains(&t) && (0. ..=1.).contains(&s)).then_some(t));
    }
    if from_a.abs() < PARALLEL_EPSILON && from_b.abs() < PARALLEL_EPSILON {
        let length = dot(u, u);
        let p = dot(offset, u) / length;
        let q = dot(subtract(b.end, a.start), u) / length;
        if (p.max(q).min(1.) - p.min(q).max(0.)) * length.sqrt() > POINT_EPSILON {
            // Coincident intervals need the source combiner's edge joining rules.
            return None;
        }
    }
    Some(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pointwise::canvas::shape::DistanceSegment;

    #[test]
    fn union_clips_a_neighbor_across_the_tile_seam() {
        let rectangle = |min: Point, max: Point| {
            let points = [min, [max[0], min[1]], max, [min[0], max[1]]];
            let curve = points
                .iter()
                .zip(points.iter().cycle().skip(1))
                .map(|(&start, &end)| {
                    let edge = Edge { start, end };
                    DistanceSegment {
                        controls: [start, edge.at(0.25), edge.at(0.75), end],
                        corners: [true; 2],
                        rounded: [false; 2],
                    }
                })
                .collect();
            Outline {
                contours: vec![points.to_vec()],
                combine: 6,
                stroke: None,
                curve: Some(curve),
            }
        };
        let outlines = [
            rectangle([95., 10.], [105., 20.]),
            rectangle([2., 12.], [8., 18.]),
        ];
        let boundary = prepare::<()>(&outlines, &|| false).unwrap().unwrap();
        let vertical = |x: f64| {
            boundary.iter().any(|part| {
                let chord = part.chord();
                chord.start[0] == x && chord.end[0] == x
            })
        };
        assert!(vertical(108.));
        assert!(!vertical(102.));
    }
}
