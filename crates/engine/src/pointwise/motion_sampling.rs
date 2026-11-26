//! Exact r7.b2eb8 / r7.a2ebc four-read Single interpolation. Coordinate work
//! precedes raw TL/TR/BL/BR loads; zero weights never suppress source reads.
use super::PointwiseError;
use super::numeric::{add, mul, sub};
use crate::{FloatBuffer, ImageError, image_map::read_pixel};
fn remainder<E>(a: i32, b: i32) -> Result<i32, PointwiseError<E>> {
    a.checked_rem(b).ok_or(if b == 0 {
        PointwiseError::DivideByZero
    } else {
        PointwiseError::Arithmetic
    })
}
fn wrapped<E>(mut axis: f32, extent: i32) -> Result<(i32, i32, f32), PointwiseError<E>> {
    // REM is a literal Single site. Nonfinite/native helper payload behavior
    // remains scoped to future observed profiles rather than a global rewrite.
    if axis < 0. {
        axis = add(axis % (extent as f32), extent as f32);
    } else if axis >= extent as f32 {
        axis %= extent as f32;
    }
    let mut first = crate::framework_cast::integer(f64::from(axis));
    let mut next = first.wrapping_add(1);
    let fraction = sub(axis, first as f32);
    if next >= extent {
        first = remainder(first, extent)?;
        next = remainder(next, extent)?;
    }
    Ok((first, next, fraction))
}
fn clamped(axis: f32, extent: i32) -> (i32, i32, f32) {
    if axis < 0. {
        (0, 0, 0.)
    } else if axis >= extent.wrapping_sub(1) as f32 {
        let last = extent.wrapping_sub(1);
        (last, last, 0.)
    } else {
        let first = crate::framework_cast::integer(f64::from(axis));
        (first, first.wrapping_add(1), sub(axis, first as f32))
    }
}
pub(super) fn sample<E>(
    x: f32,
    y: f32,
    source: &FloatBuffer,
    width: i32,
    height: i32,
    seamless: bool,
) -> Result<f32, PointwiseError<E>> {
    sample_from(x, y, width, height, seamless, |index| {
        source.read(usize::try_from(index).map_err(|_| ImageError::Index)?)
    })
}

pub(super) fn sample_pixels<E>(
    x: f32,
    y: f32,
    source: &[f32],
    width: i32,
    height: i32,
    seamless: bool,
) -> Result<f32, PointwiseError<E>> {
    sample_from(x, y, width, height, seamless, |index| {
        read_pixel(source, index)
    })
}

fn sample_from<E>(
    x: f32,
    y: f32,
    width: i32,
    height: i32,
    seamless: bool,
    read: impl Fn(i32) -> Result<f32, ImageError>,
) -> Result<f32, PointwiseError<E>> {
    let (x0, x1, fx) = if seamless {
        wrapped(x, width)?
    } else {
        clamped(x, width)
    };
    let (y0, y1, fy) = if seamless {
        wrapped(y, height)?
    } else {
        clamped(y, height)
    };
    let y0 = y0.wrapping_mul(width);
    let y1 = y1.wrapping_mul(width);
    let tl = read(x0.wrapping_add(y0))?;
    let tr = read(x1.wrapping_add(y0))?;
    let top = add(tl, mul(fx, sub(tr, tl)));
    let bl = read(x0.wrapping_add(y1))?;
    let br = read(x1.wrapping_add(y1))?;
    let bottom = add(bl, mul(fx, sub(br, bl)));
    Ok(add(top, mul(fy, sub(bottom, top))))
}
