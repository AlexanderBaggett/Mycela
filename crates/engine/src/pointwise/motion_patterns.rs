//! The actual six Motion sampling patterns (a29b2–29b7), not a shape library.
//! Math calls use the host native f64 profile, pending the fixed72 replay;
//! neither portable transcendental bits nor resource-equivalent GC is claimed.
#![allow(clippy::approx_constant)] // Exact widened-Single knot literals are source data.
use super::numeric::{add, mul, sub};
use crate::ImageError;
use std::f64::consts::PI;
const TURN: f64 = 2.0 * PI;
#[derive(Clone, Copy, Default, Debug)]
pub(super) struct Point {
    pub(super) x: f32,
    pub(super) y: f32,
}
fn array<T: Default + Clone>(count: i32) -> Result<Vec<T>, ImageError> {
    let n = usize::try_from(count).map_err(|_| ImageError::Overflow)?;
    let mut out = Vec::new();
    out.try_reserve_exact(n)
        .map_err(|_| ImageError::Allocation)?;
    out.resize(n, T::default());
    Ok(out)
}
pub(super) fn blank(count: usize) -> Result<Vec<Point>, ImageError> {
    array(i32::try_from(count).map_err(|_| ImageError::Overflow)?)
}
fn push<T>(list: &mut Vec<T>, value: T) -> Result<(), ImageError> {
    if list.len() == list.capacity() {
        let next = if list.capacity() == 0 {
            4
        } else {
            list.capacity()
                .checked_mul(2)
                .ok_or(ImageError::Allocation)?
        };
        list.try_reserve_exact(next - list.len())
            .map_err(|_| ImageError::Allocation)?;
    }
    list.push(value);
    Ok(())
}
fn to_array(list: &[Point]) -> Result<Vec<Point>, ImageError> {
    let mut out = array(i32::try_from(list.len()).map_err(|_| ImageError::Overflow)?)?;
    out.copy_from_slice(list);
    Ok(out)
}
#[allow(clippy::manual_clamp)] // Preserve the exact ordered source correction/clamp sequence.
fn radians(mut degrees: f64) -> f64 {
    if degrees < 0. {
        degrees += 360.;
    }
    degrees *= 0.017453292519943295;
    if degrees < 0. {
        0.
    } else if degrees > TURN {
        TURN
    } else {
        degrees
    }
}
fn line(samples: i32, angle: f64, width: f32) -> Result<Vec<Point>, ImageError> {
    let mut list = Vec::new();
    let rotation = radians(angle);
    let a = 0.0 + rotation;
    let b = PI + rotation;
    let w = f64::from(width);
    let (x0, y0, x1, y1) = (w * a.cos(), w * a.sin(), w * b.cos(), w * b.sin());
    for i in 0..samples {
        let t = f64::from(i) / f64::from(samples.wrapping_sub(1));
        push(
            &mut list,
            Point {
                x: (x0 + t * (x1 - x0)) as f32,
                y: (y0 + t * (y1 - y0)) as f32,
            },
        )?;
    }
    to_array(&list)
}
fn cross(samples: i32, angle: f64, width: f32) -> Result<Vec<Point>, ImageError> {
    let mut list = Vec::new();
    let r = radians(angle);
    let (a, b, c, d) = (PI / 2.0 + r, 0.0 + r, PI + r, 4.71238898038469 + r);
    let w = f64::from(width);
    // Source computes horizontal endpoints before vertical ones, then emits
    // the vertical diameter first. Counts0/1 retain their IEEE denominators.
    let (hx0, hy0, hx1, hy1) = (w * b.cos(), w * b.sin(), w * c.cos(), w * c.sin());
    let (vx0, vy0, vx1, vy1) = (w * a.cos(), w * a.sin(), w * d.cos(), w * d.sin());
    let first = if samples > 3 { samples >> 1 } else { 0 };
    let second = if samples > 3 {
        samples.wrapping_sub(first)
    } else {
        samples
    };
    for i in 0..first {
        let t = f64::from(i) / f64::from(first.wrapping_sub(1));
        push(
            &mut list,
            Point {
                x: (vx0 + t * (vx1 - vx0)) as f32,
                y: (vy0 + t * (vy1 - vy0)) as f32,
            },
        )?;
    }
    for i in 0..second {
        let t = f64::from(i) / f64::from(second.wrapping_sub(1));
        push(
            &mut list,
            Point {
                x: (hx0 + t * (hx1 - hx0)) as f32,
                y: (hy0 + t * (hy1 - hy0)) as f32,
            },
        )?;
    }
    to_array(&list)
}
fn triple(samples: i32, angle: f64, width: f32) -> Result<Vec<Point>, ImageError> {
    let mut radial = array::<Point>(3)?;
    let mut list = Vec::new();
    let phase = PI / 2.0;
    let step = TURN / 3.0;
    let mut angles = array::<f64>(3)?;
    let r = radians(angle);
    for i in 0..3 {
        angles[i] = f64::from((phase + step * i as f64) as f32) + r;
        radial[i] = Point {
            x: mul(width, -(angles[i].cos() as f32)),
            y: mul(width, -(angles[i].sin() as f32)),
        };
    }
    let mut counts = array::<i32>(3)?;
    if samples == 1 {
        push(&mut list, Point::default())?;
    } else {
        match samples % 3 {
            0 => counts.fill(samples / 3),
            1 => {
                for (i, c) in counts.iter_mut().enumerate() {
                    *c = samples.wrapping_sub(1) / 3 + if i == 0 { 1 } else { 0 };
                }
            }
            2 => {
                for (i, c) in counts.iter_mut().enumerate() {
                    *c = samples.wrapping_sub(1) / 3 + if i == 0 { 0 } else { 1 };
                }
            }
            _ => {}
        }
        for i in 0..3 {
            for j in 0..counts[i] {
                let denominator = if i == 0 && samples % 3 == 1 {
                    f64::from(counts[i]) - 1.0
                } else if i != 0 && samples % 3 == 1 {
                    f64::from(counts[i])
                } else {
                    f64::from(counts[i]) - 1.0 + 0.5
                };
                let t = f64::from(j) / denominator;
                // This is genuine Single SUB, unlike the radial unary NEG above.
                push(
                    &mut list,
                    Point {
                        x: (f64::from(radial[i].x) + t * f64::from(sub(0., radial[i].x))) as f32,
                        y: (f64::from(radial[i].y) + t * f64::from(sub(0., radial[i].y))) as f32,
                    },
                )?;
            }
        }
    }
    to_array(&list)
}
type PolygonVertices = (Vec<Point>, Vec<f32>);

fn vertices(shape: i32, width: f32) -> Result<Option<PolygonVertices>, ImageError> {
    let root2 = 2.0f64.sqrt() as f32; // reached even for unknown shape
    let (count, phase, step) = match shape {
        0 => (3, PI / 2., 2.094395160675049),
        1 => (4, PI / 4., 1.5707963705062866),
        3 => (2, 0., 3.1415927410125732),
        _ => return Ok(None),
    };
    let mut points = array::<Point>(count)?;
    let mut knots = array::<f32>(count)?;
    for i in 0..count as usize {
        knots[i] = (phase + step * i as f64) as f32;
        let x = mul(width, f64::from(knots[i]).cos() as f32);
        let y = mul(width, f64::from(knots[i]).sin() as f32);
        points[i] = if shape == 1 {
            Point {
                x: mul(x, root2),
                y: mul(y, root2),
            }
        } else {
            Point { x, y }
        };
    }
    Ok(Some((points, knots)))
}
fn polygon(mut phase: f64, shape: i32, width: f32) -> Result<Point, ImageError> {
    let Some((points, knots)) = vertices(shape, width)? else {
        return Ok(Point::default());
    };
    let (mut x, mut y) = (-100., -100.);
    if phase < 0. {
        phase = if phase.abs() > TURN {
            phase + TURN * 2.0
        } else {
            phase + TURN
        };
    } else if phase > TURN {
        phase = if phase > TURN * 2.0 {
            phase - TURN * 2.0
        } else {
            phase - TURN
        };
    }
    let span = sub(knots[1], knots[0]);
    let last = knots.len() - 1;
    let gap = sub(TURN as f32, knots[last]);
    for i in 0..points.len() - 1 {
        if f64::from(knots[i]) <= phase && phase <= f64::from(knots[i + 1]) {
            let t = (phase - f64::from(knots[i])) / f64::from(span);
            x = f64::from(points[i].x) + t * f64::from(sub(points[i + 1].x, points[i].x));
            y = f64::from(points[i].y) + t * f64::from(sub(points[i + 1].y, points[i].y));
            break;
        }
        if (f64::from(knots[last]) <= phase && phase <= TURN)
            || (0. <= phase && phase <= f64::from(knots[0]))
        {
            let t = if 0. <= phase && phase <= f64::from(knots[0]) {
                (phase - 0.0 + f64::from(gap)) / f64::from(span)
            } else {
                (phase - f64::from(knots[last])) / f64::from(span)
            };
            x = f64::from(points[last].x) + t * f64::from(sub(points[0].x, points[last].x));
            y = f64::from(points[last].y) + t * f64::from(sub(points[0].y, points[last].y));
            break;
        }
    }
    Ok(Point {
        x: x as f32,
        y: y as f32,
    })
}
pub(super) fn pattern(
    samples: i32,
    angle: f64,
    shape: i32,
    width: f32,
) -> Result<Vec<Point>, ImageError> {
    // Even helper-dispatch branches allocate the initial PointF[samples] first.
    let mut out = array::<Point>(samples)?;
    let step = TURN / f64::from(samples);
    let rotation = (angle - 90.0) * PI / 180.0;
    match shape {
        2 => {
            for (i, p) in out.iter_mut().enumerate() {
                let phase = 360.0 - (rotation + step * i as f64);
                *p = Point {
                    x: (f64::from(width) * -(-phase).sin()) as f32,
                    y: (f64::from(width) * -(-phase).cos()) as f32,
                };
            }
        }
        3 => out = line(samples, angle, width)?,
        4 => out = triple(samples, angle, width)?,
        5 => out = cross(samples, angle, width)?,
        _ => {
            let offset = if shape == 0 { -PI / 2.0 } else { 0.0 };
            let sine = (rotation + offset).sin() as f32;
            let cosine = (rotation + offset).cos() as f32;
            for (i, p) in out.iter_mut().enumerate() {
                let point = polygon(step * i as f64, shape, width)?;
                *p = Point {
                    x: sub(mul(point.x, cosine), mul(point.y, sine)),
                    y: add(mul(point.x, sine), mul(point.y, cosine)),
                };
            }
        }
    }
    Ok(out)
}
