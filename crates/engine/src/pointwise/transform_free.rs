//! m: MatrixD, anisotropic prefilter and separate Double interpolation.
use super::numeric::{mul, sub};
use super::transform::*;
use super::{PointwiseError, PointwiseFilter, PointwiseRenderOptions, SharedImageMap};
use crate::{ImageGeometry, MatrixWireRecord, ProceduralExecution};
fn point(e: [f64; 6], x: f32, y: f32, cx: f32, cy: f32, w: i32, h: i32) -> (f64, f64) {
    (
        f64::from(sub(x, cx)) * e[0]
            + f64::from(sub(y, cy)) * e[2]
            + e[4] * f64::from(w)
            + f64::from(cx),
        f64::from(sub(x, cx)) * e[1]
            + f64::from(sub(y, cy)) * e[3]
            + e[5] * f64::from(h)
            + f64::from(cy),
    )
}
#[allow(clippy::manual_range_contains)] // Preserve the exact source bound comparison order.
fn inside(x: f64, y: f64, w: i32, h: i32) -> bool {
    x >= -1E-6 && x < f64::from(w) + 1E-6 && y >= -1E-6 && y < f64::from(h) + 1E-6
}
pub(super) fn retained<E: 'static>(
    f: &PointwiseFilter<E>,
    inputs: &[Option<SharedImageMap>],
    geometry: ImageGeometry,
    time: f64,
    execution: ProceduralExecution,
    options: PointwiseRenderOptions,
) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
    let original = get(inputs, 0)?;
    let w = geometry.width();
    let h = geometry.height();
    let sx = f.parameter(0)?.value_at(0, time)?;
    let sy = f.parameter(0)?.value_at(1, time)?;
    let mut tx = f.parameter(1)?.raw_value_at(0, time)?;
    // Actual CIL NEG. Do not rewrite as subtraction from positive zero.
    let mut ty = -f.parameter(1)?.raw_value_at(1, time)?;
    let kx = f.parameter(2)?.raw_value_at(0, time)?;
    let ky = f.parameter(2)?.raw_value_at(1, time)?;
    let angle = f.parameter(3)?.effective_value_at(time)?;
    let beginning = super::adaptive::text(f, 4, time, "Beginning")?;
    let seamless = f.parameter(5)?.effective_index_at(time)?;
    let tiles = seamless == 0;
    let repeating = seamless == 2;
    if tiles {
        for t in [&mut tx, &mut ty] {
            *t %= 1.;
            if *t < 0. {
                *t += 1.;
            }
            if *t > 0.5 {
                *t -= 1.;
            }
        }
    }
    if sx == 1.
        && sy == 1.
        && f64::from(cast(tx)) == tx
        && f64::from(cast(ty)) == ty
        && kx == 0.
        && ky == 0.
        && angle == 0.
    {
        return Ok(original);
    }
    let mut matrix = MatrixWireRecord {
        elements: [sx, ky, kx, sy, 0., 0.],
    };
    matrix.rotate_degrees(angle, !beginning);
    matrix.translate(tx, ty, true);
    let mut lengths = [0.; 2];
    if matrix.is_invertible() {
        let mut inverse = matrix.copy_value();
        inverse.invert();
        let mut corners = [[-0.5, -0.5], [0.5, -0.5], [-0.5, 0.5]];
        inverse
            .transform_points_f64(Some(&mut corners))
            .map_err(|_| PointwiseError::NullReference)?;
        let [a, b, c] = corners;
        lengths = [
            libm::sqrt(libm::pow(b[0] - a[0], 2.) + libm::pow(b[1] - a[1], 2.)),
            libm::sqrt(libm::pow(c[0] - a[0], 2.) + libm::pow(c[1] - a[1], 2.)),
        ];
    }
    let mut blur = [0.; 2];
    for axis in 0..2 {
        if lengths[axis] != 0. {
            blur[axis] = (1. / lengths[axis]).abs();
        }
    }
    let mut source = required(original.as_ref())?.clone();
    if blur[0] > 1. || blur[1] > 1. {
        source = super::blur::affine_preblur(
            &source,
            blur[0] / 2. / f64::from(w) / 1.3,
            blur[1] / 2. / f64::from(h) / 1.3,
            execution,
        )?;
    }
    // Actual ImageMap(ImageMap) copies geometry only, preserving its origin.
    let output = source.geometry_copy();
    let cx = mul(w as f32, 0.5);
    let cy = mul(w as f32, 0.5);
    for ch in 0..count(&source, options.is_channel_rendering(), true)? {
        let values = source.channel_with_effects(ch, true)?;
        if values.len() <= 1 && ch != 3 {
            output.copy_indexed_constant(ch, values.read(0)?, &source)?;
        } else {
            let dest = output.allocate_indexed(ch, &source)?;
            let e = matrix.elements;
            crate::procedural_schedule::for_each_row_chunk::<E>(h, execution, |start, n| {
                for row in start..start.wrapping_add(n) {
                    for col in 0..w {
                        let (mut x, mut y) = point(e, col as f32, row as f32, cx, cy, w, h);
                        let mut valid = inside(x, y, w, h);
                        if tiles && !valid {
                            'neighbors: for ix in -1i32..=1 {
                                let nx = col.wrapping_add(ix.wrapping_mul(w)) as f32;
                                for iy in -1i32..=1 {
                                    if ix != 0 || iy != 0 {
                                        let ny = row.wrapping_add(iy.wrapping_mul(h)) as f32;
                                        (x, y) = point(e, nx, ny, cx, cy, w, h);
                                        valid = inside(x, y, w, h);
                                        if valid {
                                            break 'neighbors;
                                        }
                                    }
                                }
                            }
                        }
                        let at = col.wrapping_add(row.wrapping_mul(w));
                        if !repeating && !valid {
                            if ch == 3 {
                                write(&dest, at, 1.)?;
                            }
                        } else {
                            write(
                                &dest,
                                at,
                                if values.len() > 1 {
                                    sample_double(x, y, &values, w, h)?
                                } else {
                                    values.read(0)?
                                },
                            )?;
                        }
                    }
                }
                Ok(())
            })?;
        }
        source.seal_channel(ch)?;
        output.seal_channel(ch)?;
    }
    Ok(Some(output))
}
