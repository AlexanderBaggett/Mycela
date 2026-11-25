use super::*;
fn finite_slope(value: f32) -> f32 {
    if value == f32::NEG_INFINITY {
        f32::MIN
    } else if value == f32::INFINITY {
        f32::MAX
    } else {
        value
    }
}

fn line_at(left: Point, right: Point, x: f32) -> Point {
    let slope = finite_slope((right.y - left.y) / (right.x - left.x));
    slope_at(left, slope, x)
}

fn slope_at(origin: Point, slope: f32, x: f32) -> Point {
    Point {
        x,
        y: origin.y + slope * (x - origin.x),
    }
}

fn distance(left: Point, right: Point) -> f32 {
    // The sum/products occur as f32, before the source's Math.Sqrt widening.
    let square = (left.x - right.x) * (left.x - right.x) + (left.y - right.y) * (left.y - right.y);
    libm::sqrt(f64::from(square)) as f32
}

fn tangents(points: [Point; 4], outgoing: i32, incoming: i32) -> [Point; 2] {
    let [before, left, right, after] = points;
    let reach = distance(left, right) / 5.0_f32;
    let mut extrapolated = line_at(before, left, left.x + 0.2_f32 * (left.x - before.x));
    if distance(left, extrapolated) > reach {
        let previous = distance(before, left);
        let step = reach * (left.x - before.x) / previous;
        extrapolated = line_at(before, left, left.x + step);
    }
    let mut slope = finite_slope((right.y - before.y) / (right.x - before.x));
    let mut cross = finite_slope((right.y - extrapolated.y) / (right.x - extrapolated.x));
    let first_x = if slope == cross {
        left.x + 0.3_f32 * (right.x - left.x)
    } else {
        (left.y - extrapolated.y + cross * extrapolated.x - slope * left.x) / (cross - slope)
    };
    let mut first = slope_at(left, slope, first_x);
    // Source clamps X after computing Y; it does not recompute Y here.
    if first.x > 0.5_f32 * (left.x + right.x) {
        first.x = 0.5_f32 * (left.x + right.x);
    }
    extrapolated = line_at(right, after, right.x - 0.2_f32 * (after.x - right.x));
    if distance(right, extrapolated) > reach {
        let following = distance(right, after);
        let step = reach * (after.x - right.x) / following;
        extrapolated = line_at(right, after, right.x - step);
    }
    slope = finite_slope((after.y - left.y) / (after.x - left.x));
    cross = finite_slope((left.y - extrapolated.y) / (left.x - extrapolated.x));
    let second_x = if slope == cross {
        right.x - 0.3_f32 * (right.x - left.x)
    } else {
        (right.y - extrapolated.y + cross * extrapolated.x - slope * right.x) / (cross - slope)
    };
    let mut second = slope_at(right, slope, second_x);
    if second.x < 0.5_f32 * (left.x + right.x) {
        second.x = 0.5_f32 * (left.x + right.x);
    }
    if outgoing == 0 {
        first = left;
    }
    if incoming == 0 {
        second = right;
    }
    [first, second]
}

fn basis(index: usize, order: usize, knots: &[i32; 8], time: f64) -> f64 {
    if order == 1 {
        return if f64::from(knots[index]) <= time && time < f64::from(knots[index + 1]) {
            1.0
        } else {
            0.0
        };
    }
    let left_empty = knots[index + order - 1] == knots[index];
    let right_empty = knots[index + order] == knots[index + 1];
    if left_empty && right_empty {
        return 0.0;
    }
    if left_empty {
        return (f64::from(knots[index + order]) - time)
            / f64::from(knots[index + order] - knots[index + 1])
            * basis(index + 1, order - 1, knots, time);
    }
    if right_empty {
        return (time - f64::from(knots[index]))
            / f64::from(knots[index + order - 1] - knots[index])
            * basis(index, order - 1, knots, time);
    }
    (time - f64::from(knots[index])) / f64::from(knots[index + order - 1] - knots[index])
        * basis(index, order - 1, knots, time)
        + (f64::from(knots[index + order]) - time)
            / f64::from(knots[index + order] - knots[index + 1])
            * basis(index + 1, order - 1, knots, time)
}

// Caller must use the actual source conv.i4/newarr/+2 error order when deriving
// count. This helper deliberately does not silently bound count or substitute
// a default cache on allocation failure.
fn segment<E>(control: [Point; 4], count: i32) -> Result<Vec<Point>, CurveError<E>> {
    let length = usize::try_from(count).map_err(|_| CurveError::Overflow)?;
    let mut points = Vec::new();
    points
        .try_reserve_exact(length)
        .map_err(|_| CurveError::Allocation)?;
    let knots = [0, 0, 0, 0, 1, 1, 1, 1];
    let step = 1.0_f64 / f64::from(count.wrapping_sub(1));
    let mut time = 0.0_f64;
    for _ in 0..count - 1 {
        let mut point = Point::default();
        for (index, control_point) in control.iter().enumerate() {
            let weight = basis(index, 4, &knots, time) as f32;
            point.x += control_point.x * weight;
            point.y += control_point.y * weight;
        }
        points.push(point);
        time += step;
    }
    if count <= 0 {
        return Err(CurveError::Index);
    }
    points.push(control[3]);
    Ok(points)
}

fn sample_count<E>(left: Point, right: Point) -> Result<i32, CurveError<E>> {
    let count = libm::ceil(f64::from((right.x - left.x) / 0.001_f32));
    if !(-2147483648.0..2147483648.0).contains(&count) {
        return Err(CurveError::UnsupportedCapability);
    }
    Ok((count as i32).wrapping_add(2))
}
fn append_segment<E>(
    output: &mut Vec<Point>,
    points: [Point; 4],
    outgoing: i32,
    incoming: i32,
) -> Result<(), CurveError<E>> {
    let control = tangents(points, outgoing, incoming);
    let count = sample_count(points[1], points[2])?;
    let segment = segment([points[1], control[0], control[1], points[2]], count)?;
    output
        .try_reserve(segment.len())
        .map_err(|_| CurveError::Allocation)?;
    output.extend(segment);
    Ok(())
}
pub(super) fn cache<E>(
    points: &[Point],
    entries: &[CustomPointHandle],
    looped: bool,
) -> Result<Vec<Point>, CurveError<E>> {
    let mut output = Vec::new();
    if points.len() > 1 {
        if looped {
            for i in 0..points.len().saturating_sub(3) {
                append_segment(
                    &mut output,
                    [points[i], points[i + 1], points[i + 2], points[i + 3]],
                    entries[i + 1].0.outgoing,
                    entries[i + 2].0.incoming,
                )?;
            }
        } else {
            let left = points[0];
            let right = points[1];
            let before = Point {
                x: left.x - 1.0,
                y: left.y,
            };
            let after = if points.len() > 2 {
                points[2]
            } else {
                Point {
                    x: right.x + 1.0,
                    y: right.y,
                }
            };
            append_segment(
                &mut output,
                [before, left, right, after],
                entries[0].0.outgoing,
                entries[1].0.incoming,
            )?;
            let first = output.first().ok_or(CurveError::Index)?;
            if first.x != 0.0 {
                output.insert(0, Point { x: 0.0, y: left.y });
            }
            if points.len() > 3 {
                for i in 1..points.len() - 2 {
                    append_segment(
                        &mut output,
                        [points[i - 1], points[i], points[i + 1], points[i + 2]],
                        entries[i].0.outgoing,
                        entries[i + 1].0.incoming,
                    )?;
                }
            }
            if points.len() > 2 {
                let n = points.len();
                let last = points[n - 1];
                append_segment(
                    &mut output,
                    [
                        points[n - 3],
                        points[n - 2],
                        last,
                        Point {
                            x: last.x + 1.0,
                            y: last.y,
                        },
                    ],
                    entries[n - 2].0.outgoing,
                    entries[n - 1].0.incoming,
                )?;
            }
            output.push(Point {
                x: 1.0,
                y: points[points.len() - 1].y,
            });
        }
    } else {
        let y = if points.len() == 1 { points[0].y } else { 0.5 };
        output.push(Point { x: 0.0, y });
        output.push(Point { x: 1.0, y });
    }
    Ok(output)
}
