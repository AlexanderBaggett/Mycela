//! Cube cross face selection, padding and projection for `ady` / `abf`.
use super::{PointwiseError, SharedImageMap};
use crate::{FloatBuffer, ImageError, ImageGeometry, ProceduralExecution};
use std::f64::consts::PI;

type Line = (i32, i32, bool, i32, i32, bool, i32, bool, bool);
fn area<E>(w: i32, h: i32) -> Result<usize, PointwiseError<E>> {
    if w <= 0 || h <= 0 {
        return Err(PointwiseError::UnsupportedCapability(
            "Convert Environment nonpositive cross geometry",
        ));
    }
    usize::try_from(w.checked_mul(h).ok_or(ImageError::Overflow)?)
        .map_err(|_| ImageError::Overflow.into())
}
fn zeros<E>(n: usize) -> Result<Vec<f32>, PointwiseError<E>> {
    let mut v = Vec::new();
    v.try_reserve_exact(n).map_err(|_| ImageError::Allocation)?;
    v.resize(n, 0.0);
    Ok(v)
}
fn offset<E>(x: i32, y: i32, w: i32, h: i32) -> Result<usize, PointwiseError<E>> {
    if x < 0 || x >= w || y < 0 || y >= h {
        return Err(ImageError::Index.into());
    }
    usize::try_from(y * w + x).map_err(|_| ImageError::Index.into())
}

/// `ady` crops the square input to the source cross rectangle. Constant
/// channels follow its literal nf branch, which writes zero rather than copying.
pub(super) fn crop<E>(
    source: &SharedImageMap,
    format: i32,
    cancelled: &dyn Fn() -> bool,
) -> Result<SharedImageMap, PointwiseError<E>> {
    let sw = source.geometry().width();
    let sh = source.geometry().height();
    let (w, h) = if format == 3 {
        (sw, (sw / 4) * 3)
    } else {
        ((sh / 4) * 3, sh)
    };
    let n = area::<E>(w, h)?;
    let result = SharedImageMap::new(ImageGeometry::new(w, h));
    for ch in 0..4 {
        if source.uses_plane(ch)? {
            let input = source.channel(ch, true)?;
            let mut out = zeros::<E>(n)?;
            for y in 0..h {
                super::check_render_cancelled(cancelled)?;
                for x in 0..w {
                    let src = offset::<E>(x, y, sw, sh)?;
                    let dst = offset::<E>(x, y, w, h)?;
                    out[dst] = input.read(src)?;
                }
            }
            result.set_buffer(ch, Some(&FloatBuffer::new(out)))?;
            if format == 4 {
                result.seal_channel(ch)?;
            }
        } else {
            result.set_constant(ch, 0.0)?;
            if format == 4 {
                result.seal_channel(ch)?;
            }
        }
    }
    if format == 4 {
        result.seal_permanently()?;
    }
    Ok(result)
}

// Faces are selected in original order with inclusive/exclusive edge tests.
pub(super) fn direction_coordinate(
    dx: f64,
    dy: f64,
    dz: f64,
    w: i32,
    h: i32,
    format: i32,
) -> (f32, f32) {
    let n = f64::from(if format == 3 { h / 3 } else { w / 3 });
    let incl = |x: f64| (-1.0..=1.0).contains(&x);
    let excl = |x: f64| x > -1.0 && x < 1.0;
    let xy = |a: f64, b: f64| {
        (
            (n + n * (1.0 + a) * 0.5) as f32,
            (n + n * (1.0 + b) * 0.5) as f32,
        )
    };
    let k = 1.0 / dz;
    if k >= 0.0 {
        let a = k * dx;
        let b = k * dy;
        if incl(a) && excl(b) {
            return xy(a, b);
        }
    }
    let k = -1.0 / dz;
    if k >= 0.0 {
        let a = -k * dx;
        let b = k * dy;
        if incl(a) && excl(b) {
            if format == 3 {
                return (
                    (3.0 * n + n * (1.0 + a) * 0.5) as f32,
                    (n + n * (1.0 + b) * 0.5) as f32,
                );
            }
            return (
                (n + n * (1.0 - a) * 0.5) as f32,
                (3.0 * n + n * (1.0 - b) * 0.5) as f32,
            );
        }
    }
    let k = -1.0 / dx;
    if k >= 0.0 {
        let b = k * dy;
        let a = k * dz;
        if excl(b) && incl(a) {
            return (
                (n * (1.0 + a) * 0.5) as f32,
                (n + n * (1.0 + b) * 0.5) as f32,
            );
        }
    }
    let k = 1.0 / dx;
    if k >= 0.0 {
        let b = k * dy;
        let a = -k * dz;
        if excl(b) && incl(a) {
            return (
                (2.0 * n + n * (1.0 + a) * 0.5) as f32,
                (n + n * (1.0 + b) * 0.5) as f32,
            );
        }
    }
    let k = -1.0 / dy;
    if k >= 0.0 {
        let a = k * dx;
        let b = k * dz;
        if excl(b) && excl(a) {
            return (
                (n + n * (1.0 + a) * 0.5) as f32,
                (n * (1.0 + b) * 0.5) as f32,
            );
        }
    }
    let k = 1.0 / dy;
    if k >= 0.0 {
        let a = k * dx;
        let b = -k * dz;
        if excl(b) && excl(a) {
            return (
                (n + n * (1.0 + a) * 0.5) as f32,
                (2.0 * n + n * (1.0 + b) * 0.5) as f32,
            );
        }
    }
    (0.0, 0.0)
}

/// `abf.a` copies the cross into a one-pixel border and stitches adjacent
/// face edges. Operations are ordered because some corner lines read the
/// already-padded image, just as the original does.
pub(super) fn padded<E>(
    source: &SharedImageMap,
    format: i32,
    cancelled: &dyn Fn() -> bool,
) -> Result<SharedImageMap, PointwiseError<E>> {
    let w = source.geometry().width();
    let h = source.geometry().height();
    let pw = w.checked_add(2).ok_or(ImageError::Overflow)?;
    let ph = h.checked_add(2).ok_or(ImageError::Overflow)?;
    let out_n = area::<E>(pw, ph)?;
    let count = if source.has_transparency()? { 4 } else { 3 };
    let mut src = Vec::new();
    let mut dst = Vec::new();
    for ch in 0..count {
        let input = source.materialize(ch)?;
        let mut out = zeros::<E>(out_n)?;
        for y in 0..h {
            super::check_render_cancelled(cancelled)?;
            for x in 0..w {
                out[offset::<E>(x + 1, y + 1, pw, ph)?] = input.read(offset::<E>(x, y, w, h)?)?;
            }
        }
        src.push(input);
        dst.push(out);
        source.seal_channel(ch)?;
    }
    let n = if format == 3 { h / 3 } else { w / 3 };
    let mut lines: Vec<Line> = Vec::new();
    if format == 3 && w * 3 == h * 4 {
        lines.extend_from_slice(&[
            (0, n, true, n, 1, false, n, false, false),
            (n, 0, false, 1, n, true, n, false, false),
            (0, n, false, 4 * n + 1, n + 1, false, n, false, false),
            (4 * n - 1, n, false, 0, n + 1, false, n, false, false),
            (0, 2 * n - 1, true, n, 2 * n + 1, false, n, true, false),
            (n, 2 * n, false, 1, 2 * n + 1, true, n, true, false),
            (n, 0, true, 3 * n + 1, n, true, n, true, false),
            (3 * n, n, true, 1 + n, 0, true, n, true, false),
            (2 * n - 1, 0, false, 2 * n + 1, n, true, n, true, false),
            (2 * n, n, true, 2 * n + 1, 1, false, n, true, false),
            (
                2 * n - 1,
                2 * n,
                false,
                2 * n + 1,
                2 * n + 1,
                true,
                n,
                false,
                false,
            ),
            (
                2 * n,
                2 * n - 1,
                true,
                2 * n + 1,
                2 * n + 1,
                false,
                n,
                false,
                false,
            ),
            (
                n,
                3 * n - 1,
                true,
                3 * n + 1,
                2 * n + 1,
                true,
                n,
                true,
                false,
            ),
            (
                3 * n,
                2 * n - 1,
                true,
                n + 1,
                3 * n + 1,
                true,
                n,
                true,
                false,
            ),
            (4 * n, n + 1, false, 4 * n + 1, n, false, 1, false, true),
            (
                4 * n,
                2 * n,
                false,
                4 * n + 1,
                2 * n + 1,
                false,
                1,
                false,
                true,
            ),
            (4 * n, n + 1, false, 0, n, false, 1, false, true),
            (4 * n, 2 * n, false, 0, 2 * n + 1, false, 1, false, true),
            (4 * n, n + 1, false, n, 0, false, 1, false, true),
            (4 * n, 2 * n, false, n, 3 * n + 1, false, 1, false, true),
            (3 * n + 1, n + 1, false, 2 * n + 1, 0, false, 1, false, true),
            (
                3 * n + 1,
                2 * n,
                false,
                2 * n + 1,
                3 * n + 1,
                false,
                1,
                false,
                true,
            ),
        ]);
    } else if w * 4 == h * 3 {
        lines.extend_from_slice(&[
            (0, n, true, n, 1, false, n, false, false),
            (n, 0, false, 1, n, true, n, false, false),
            (n, 0, true, n + 1, 4 * n + 1, true, n, false, false),
            (n, 4 * n - 1, true, n + 1, 0, true, n, false, false),
            (0, 2 * n - 1, true, n, 2 * n + 1, false, n, true, false),
            (n, 2 * n, false, 1, 2 * n + 1, true, n, true, false),
            (
                3 * n - 1,
                n,
                false,
                2 * n + 1,
                3 * n + 1,
                false,
                n,
                true,
                false,
            ),
            (
                2 * n - 1,
                3 * n,
                false,
                3 * n + 1,
                n + 1,
                false,
                n,
                true,
                false,
            ),
            (2 * n - 1, 0, false, 2 * n + 1, n, true, n, true, false),
            (2 * n, n, true, 2 * n + 1, 1, false, n, true, false),
            (
                2 * n - 1,
                2 * n,
                false,
                2 * n + 1,
                2 * n + 1,
                true,
                n,
                false,
                false,
            ),
            (
                2 * n,
                2 * n - 1,
                true,
                2 * n + 1,
                2 * n + 1,
                false,
                n,
                false,
                false,
            ),
            (0, n, false, n, 3 * n + 1, false, n, true, false),
            (n, 3 * n, false, 0, n + 1, false, n, true, false),
            (n, 1, true, n, 4 * n + 1, true, n + 2, false, true),
            (n, 4 * n, true, n, 0, true, n + 2, false, true),
            (
                2 * n + 1,
                3 * n,
                false,
                3 * n + 1,
                n,
                false,
                n + 2,
                true,
                true,
            ),
            (n + 1, 3 * n, false, 0, n, false, n + 2, true, true),
        ]);
    }
    for (sx, sy, sh, tx, ty, th, len, reverse, from_dst) in lines {
        for ch in 0..count as usize {
            let mut temp = Vec::new();
            temp.try_reserve_exact(len as usize)
                .map_err(|_| ImageError::Allocation)?;
            for i in 0..len {
                let x = sx + if sh { i } else { 0 };
                let y = sy + if sh { 0 } else { i };
                temp.push(if from_dst {
                    dst[ch][offset::<E>(x, y, pw, ph)?]
                } else {
                    src[ch].read(offset::<E>(x, y, w, h)?)?
                });
            }
            for i in 0..len {
                let x = tx + if th { i } else { 0 };
                let y = ty + if th { 0 } else { i };
                dst[ch][offset::<E>(x, y, pw, ph)?] =
                    temp[(if reverse { len - 1 - i } else { i }) as usize];
            }
        }
    }
    let result = SharedImageMap::new(ImageGeometry::new(pw, ph));
    for (ch, values) in dst.into_iter().enumerate() {
        result.set_buffer(ch as i32, Some(&FloatBuffer::new(values)))?;
    }
    Ok(result)
}

// abf.d maps each cross face pixel to longitude/latitude coordinates.
fn face_coordinate(format: i32, x: i32, y: i32, w: i32, sw: i32, sh: i32) -> Option<(f32, f32)> {
    let n = f64::from(if format == 4 { w / 3 } else { w / 4 });
    let px = f64::from(x) + 0.5;
    let py = f64::from(y) + 0.5;
    let col = (px / n).floor().min(if format == 3 { 3.0 } else { 2.0 }) as i32;
    let row = (py / n)
        .floor()
        .clamp(0.0, if format == 3 { 2.0 } else { 3.0 }) as i32;
    let face = |u: f64, v: f64| (1.0 / (1.0 + u * u + v * v)).sqrt();
    let (z, dx, dy) = match (col, row) {
        (1, 0) => {
            let u = (px - n) / n * 2.0 - 1.0;
            let v = py / n * 2.0 - 1.0;
            let dy = -face(u, v);
            (-v * dy, -u * dy, dy)
        }
        (1, 2) => {
            let u = (px - n) / n * 2.0 - 1.0;
            let v = (py - 2.0 * n) / n * 2.0 - 1.0;
            let dy = face(u, v);
            (-v * dy, u * dy, dy)
        }
        (1, 1) => {
            let u = (px - n) / n * 2.0 - 1.0;
            let v = (py - n) / n * 2.0 - 1.0;
            let z = face(u, v);
            (z, u * z, v * z)
        }
        (1, 3) | (3, 1) => {
            let (u, v) = if format == 3 {
                ((px - 3.0 * n) / n * 2.0 - 1.0, (py - n) / n * 2.0 - 1.0)
            } else {
                ((px - n) / n * 2.0 - 1.0, (py - 3.0 * n) / n * 2.0 - 1.0)
            };
            let z = -face(u, v);
            if format == 4 {
                (z, -u * z, v * z)
            } else {
                (z, u * z, -v * z)
            }
        }
        (0, 1) => {
            let u = px / n * 2.0 - 1.0;
            let v = (py - n) / n * 2.0 - 1.0;
            let dx = -face(u, v);
            (-u * dx, dx, -v * dx)
        }
        (2, 1) => {
            let u = (px - 2.0 * n) / n * 2.0 - 1.0;
            let v = (py - n) / n * 2.0 - 1.0;
            let dx = face(u, v);
            (-u * dx, dx, v * dx)
        }
        _ => return None,
    };
    let longitude = dx.atan2(z);
    let sn = longitude.sin();
    let latitude = if sn.abs() > 1e-6 {
        dy.atan2(dx / sn)
    } else {
        dy.atan2(z / longitude.cos())
    };
    Some((
        ((longitude / PI + 1.0) / (2.0 / f64::from(sw))) as f32,
        ((latitude / (PI / 2.0) + 1.0) / (2.0 / f64::from(sh))) as f32,
    ))
}
pub(crate) fn from_latlong<E>(
    source: &SharedImageMap,
    format: i32,
    w: i32,
    cancelled: &dyn Fn() -> bool,
) -> Result<SharedImageMap, PointwiseError<E>> {
    let h = if format == 3 {
        (w / 4) * 3
    } else {
        (w / 3) * 4
    };
    let n = area::<E>(w, h)?;
    let sw = source.geometry().width();
    let sh = source.geometry().height();
    if sw <= 0 || sh <= 0 || (if format == 3 { w / 4 } else { w / 3 }) == 0 {
        return Err(PointwiseError::UnsupportedCapability(
            "Convert Environment nonpositive cross geometry",
        ));
    }
    let count = source.channel_count()?;
    let planes = (0..count)
        .map(|ch| source.channel(ch as i32, true))
        .collect::<Result<Vec<_>, _>>()?;
    let mut outputs = Vec::new();
    for _ in 0..count {
        outputs.push(zeros::<E>(n)?);
    }
    for y in 0..h {
        super::check_render_cancelled(cancelled)?;
        for x in 0..w {
            if let Some((u, v)) = face_coordinate(format, x, y, w, sw, sh) {
                let at = offset::<E>(x, y, w, h)?;
                for (dest, plane) in outputs.iter_mut().zip(&planes) {
                    dest[at] = super::convert_environment::sample::<E>(plane, u, v, sw, sh)?;
                }
            }
        }
    }
    let result = SharedImageMap::new(ImageGeometry::new(w, h));
    for (ch, values) in outputs.into_iter().enumerate() {
        result.set_buffer(ch as i32, Some(&FloatBuffer::new(values)))?;
        result.seal_channel(ch as i32)?;
    }
    Ok(result)
}
/// Reproduce ady's square output buffer and its vertical-cross final resize.
pub(super) fn square<E>(
    source: &SharedImageMap,
    format: i32,
    width: i32,
    execution: ProceduralExecution,
    cancelled: &dyn Fn() -> bool,
) -> Result<SharedImageMap, PointwiseError<E>> {
    let sw = source.geometry().width();
    let sh = source.geometry().height();
    let size = if format == 3 { width } else { sh };
    let n = area::<E>(size, size)?;
    let result = SharedImageMap::new(ImageGeometry::new(size, size));
    for ch in 0..4 {
        if source.uses_plane(ch)? {
            let src = source.channel(ch, true)?;
            let mut out = zeros::<E>(n)?;
            for y in 0..sh {
                super::check_render_cancelled(cancelled)?;
                for x in 0..sw {
                    out[offset::<E>(x, y, size, size)?] = src.read(offset::<E>(x, y, sw, sh)?)?;
                }
            }
            result.set_buffer(ch, Some(&FloatBuffer::new(out)))?;
            result.seal_channel(ch)?;
        } else {
            result.set_constant(ch, 0.0)?;
        }
    }
    if format == 4 {
        super::image_resize::resize(&result, width, width, execution)
    } else {
        Ok(result)
    }
}
