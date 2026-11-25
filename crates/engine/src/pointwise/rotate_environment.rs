//! Source `g.k8` and `kh.a(ImageMap, awj, null)` environment rotation.
use super::numeric::{add, mul, sub};
use super::{PointwiseError, PointwiseFilter, SharedImageMap};
use crate::procedural_schedule::for_each_chunk;
use crate::{FloatBuffer, ImageError, ImageGeometry, ProceduralExecution};
use std::f64::consts::PI;

fn index(i: i32) -> Result<usize, ImageError> {
    usize::try_from(i).map_err(|_| ImageError::Index)
}
fn read(a: &FloatBuffer, i: i32) -> Result<f32, ImageError> {
    a.read(index(i)?)
}
fn write(a: &FloatBuffer, i: i32, value: f32) -> Result<(), ImageError> {
    a.write(index(i)?, value)
}
fn cast(a: f64) -> i32 {
    crate::framework_cast::integer(a)
}

// r7.c's polar sampling: longitude repeats and latitude reflects at the poles.
// Source performs all four array reads even for zero interpolation weights.
pub(super) fn sample<E>(
    mut x: f32,
    mut y: f32,
    data: &FloatBuffer,
    w: i32,
    h: i32,
) -> Result<f32, PointwiseError<E>> {
    if x < 0.0 {
        x = add(x, w as f32);
        if x >= w as f32 {
            x %= w as f32;
        }
    } else if x >= w as f32 {
        x %= w as f32;
    }
    if y < 0.0 {
        y = -y;
    } else if y >= h as f32 {
        y %= h.wrapping_mul(2) as f32;
        if y >= h as f32 {
            y = sub(sub(h.wrapping_mul(2) as f32, y), 1.0);
        }
    }
    let x0 = cast(f64::from(x));
    let mut x1 = x0.wrapping_add(1);
    if x1 >= w {
        x1 = x1.wrapping_sub(w);
    }
    let y0 = cast(f64::from(y));
    let mut y1 = y0.wrapping_add(1);
    if y1 >= h {
        y1 = h.wrapping_mul(2).wrapping_sub(y1).wrapping_sub(1);
    }
    let fx = sub(x, x0 as f32);
    let fy = sub(y, y0 as f32);
    let tl = read(data, x0.wrapping_add(y0.wrapping_mul(w)))?;
    let tr = read(data, x1.wrapping_add(y0.wrapping_mul(w)))?;
    let bl = read(data, x0.wrapping_add(y1.wrapping_mul(w)))?;
    let br = read(data, x1.wrapping_add(y1.wrapping_mul(w)))?;
    let top = add(tl, mul(fx, sub(tr, tl)));
    let bottom = add(bl, mul(fx, sub(br, bl)));
    Ok(add(top, mul(fy, sub(bottom, top))))
}

fn source_angle<E>(value: f64) -> Result<f64, PointwiseError<E>> {
    // Fresh +0 is retained when the setter compares equal to either signed zero.
    if value == 0.0 {
        return Ok(0.0);
    }
    // A fresh awj DoubleProperty ignores nonfinite and out-of-range values.
    if !(-360.0..=360.0).contains(&value) {
        return Ok(0.0);
    }
    crate::round_decimal_places(value, 1)
        .map_err(|_| PointwiseError::Image(ImageError::ArgumentOutOfRange))
}
fn empty_buffer<E>(len: usize) -> Result<FloatBuffer, PointwiseError<E>> {
    let mut data = Vec::new();
    data.try_reserve_exact(len)
        .map_err(|_| ImageError::Allocation)?;
    data.resize(len, 0.0);
    Ok(FloatBuffer::new(data))
}

pub(super) fn retained<E: 'static>(
    f: &PointwiseFilter<E>,
    inputs: &[Option<SharedImageMap>],
    geometry: ImageGeometry,
    time: f64,
    execution: ProceduralExecution,
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
    super::check_render_cancelled(cancelled)?;
    let source = inputs.first().ok_or(PointwiseError::Index)?.as_ref();
    let heading = f.parameter(0)?.effective_value_at(time)?;
    let pitch = f.parameter(1)?.effective_value_at(time)?;
    let roll = f.parameter(2)?.effective_value_at(time)?;
    if heading % 360.0 == 0.0 && pitch % 360.0 == 0.0 && roll % 360.0 == 0.0 {
        return Ok(source.cloned());
    }
    // `awj.b` assigns three fresh DoubleProperties after the identity check.
    let degrees = [
        source_angle::<E>(heading)?,
        source_angle::<E>(pitch)?,
        source_angle::<E>(roll)?,
    ];
    let source = source.ok_or(PointwiseError::NullReference)?;
    let output = rotate_map(source, geometry, degrees, execution, cancelled)?;
    output.merge_effects(&[Some(source.clone())])?;
    Ok(Some(output))
}

/// The rotation stage shared by Rotate Environment and an Environment value.
pub(crate) fn rotate_map<E>(
    source: &SharedImageMap,
    geometry: ImageGeometry,
    degrees: [f64; 3],
    execution: ProceduralExecution,
    cancelled: &dyn Fn() -> bool,
) -> Result<SharedImageMap, PointwiseError<E>> {
    let output = SharedImageMap::new(geometry);
    let row_width = geometry.width().max(1) as usize;
    let [heading, pitch, roll] = degrees.map(|angle| angle / 180.0 * PI);
    for ch in 0..4 {
        if source.uses_plane(ch)? {
            let input = source.materialize(ch)?;
            let dest = output.materialize(ch)?;
            let length = dest.len();
            for start in (0..length).step_by(row_width) {
                super::check_render_cancelled(cancelled)?;
                for i in start..(start + row_width).min(length) {
                    dest.write(i, input.read(i)?)?;
                }
            }
            output.seal_channel(ch)?;
        } else {
            output.copy_channel_from(ch, source, ch)?;
        }
    }
    if heading != 0.0 || pitch != 0.0 || roll != 0.0 {
        let width = geometry.width();
        let height = geometry.height();
        // Positive ordinary dimensions are the portable worker domain. The
        // exact CLR cast/exception profile for invalid geometry is unproved.
        if width <= 0 || height <= 0 {
            return Err(PointwiseError::UnsupportedCapability(
                "environment rotation nonpositive geometry",
            ));
        }
        let small = cast(f64::from(width.wrapping_mul(height)).sqrt()) < 4000;
        let count = output.channel_count()? as i32;
        let dy = 2.0 / f64::from(height);
        let dx = 2.0 / f64::from(width);
        let lat_scale = PI / 2.0;
        let inverse_lat = 1.0 / lat_scale;
        let inv_pi = 1.0 / PI;
        let inverse_x = 1.0 / dx;
        let inverse_y = 1.0 / dy;
        let (ch, sh) = (heading.cos(), heading.sin());
        let (cp, sp) = (pitch.cos(), pitch.sin());
        let (cr, sr) = (roll.cos(), roll.sin());
        let m0 = cr * ch + sr * sp * sh;
        let m1 = sr * cp;
        let m2 = -cr * sh + sr * sp * ch;
        let m3 = -sr * ch + cr * sp * sh;
        let m4 = cr * cp;
        let m5 = sr * sh + cr * sp * ch;
        let m6 = cp * sh;
        let m7 = -sp;
        let m8 = cp * ch;
        let shift_only = roll == 0.0 && pitch == 0.0 && heading != 0.0;
        let shift = cast(f64::from(width) / PI / 2.0 * heading);
        let mut longitudes = Vec::new();
        if !shift_only {
            longitudes
                .try_reserve_exact(width as usize)
                .map_err(|_| ImageError::Allocation)?;
            longitudes.extend((0..width).map(|x| {
                let longitude = (f64::from(x) * dx - 1.0) * PI;
                (longitude.sin(), longitude.cos())
            }));
        }
        let area = width.wrapping_mul(height);
        let mut channels: Vec<(i32, FloatBuffer, FloatBuffer)> = Vec::new();
        for ch in 0..count {
            if !small && !output.uses_plane(ch)? {
                continue;
            }
            let input = output.materialize(ch)?;
            let dst = empty_buffer(input.len())?;
            channels.push((ch, input, dst));
            if !small {
                rotate::<E>(
                    &channels[channels.len() - 1..],
                    width,
                    height,
                    area,
                    execution,
                    shift_only,
                    shift,
                    &longitudes,
                    dy,
                    lat_scale,
                    inverse_lat,
                    inv_pi,
                    inverse_x,
                    inverse_y,
                    [m0, m1, m2, m3, m4, m5, m6, m7, m8],
                    cancelled,
                )?;
                let (_, _, dst) = channels.pop().expect("just pushed");
                output.set_buffer(ch, Some(&dst))?;
                output.seal_channel(ch)?;
            }
        }
        if small {
            rotate::<E>(
                &channels,
                width,
                height,
                area,
                execution,
                shift_only,
                shift,
                &longitudes,
                dy,
                lat_scale,
                inverse_lat,
                inv_pi,
                inverse_x,
                inverse_y,
                [m0, m1, m2, m3, m4, m5, m6, m7, m8],
                cancelled,
            )?;
            for (ch, _, dst) in channels {
                output.set_buffer(ch, Some(&dst))?;
            }
        }
    }
    Ok(output)
}

#[allow(clippy::too_many_arguments, clippy::manual_range_contains)]
fn rotate<E>(
    channels: &[(i32, FloatBuffer, FloatBuffer)],
    width: i32,
    height: i32,
    area: i32,
    execution: ProceduralExecution,
    shift_only: bool,
    shift: i32,
    longitudes: &[(f64, f64)],
    dy: f64,
    lat_scale: f64,
    inverse_lat: f64,
    inv_pi: f64,
    inverse_x: f64,
    inverse_y: f64,
    m: [f64; 9],
    cancelled: &dyn Fn() -> bool,
) -> Result<(), PointwiseError<E>> {
    for_each_chunk::<E>(area, execution, |start, count| {
        super::check_render_cancelled(cancelled)?;
        let mut x = start % width;
        let mut y = start / width;
        let mut row = y.wrapping_mul(width);
        let mut lat = (f64::from(y) * dy - 1.0) * lat_scale;
        let mut cos_lat = lat.cos();
        let mut sin_lat = lat.sin();
        let end = start.wrapping_add(count);
        let mut i = start;
        while i < end {
            if x == width {
                super::check_render_cancelled(cancelled)?;
                x = 0;
                y = y.wrapping_add(1);
                row = y.wrapping_mul(width);
                lat = (f64::from(y) * dy - 1.0) * lat_scale;
                cos_lat = lat.cos();
                sin_lat = lat.sin();
            }
            if shift_only {
                let mut sx = x.wrapping_add(shift);
                if sx < 0 {
                    sx = width.wrapping_add(sx);
                } else if sx >= width {
                    sx %= width;
                }
                let src = sx.wrapping_add(row);
                for (_, input, output) in channels {
                    write(output, i, read(input, src)?)?;
                }
            } else {
                let (sin_lon, cos_lon) = longitudes[x as usize];
                let a = cos_lat * sin_lon;
                let b = cos_lat * cos_lon;
                let vx = m[0] * a + m[3] * sin_lat + m[6] * b;
                let vy = m[1] * a + m[4] * sin_lat + m[7] * b;
                let vz = m[2] * a + m[5] * sin_lat + m[8] * b;
                let phi = vx.atan2(vz);
                let sin_phi = phi.sin();
                let theta = if sin_phi > 1e-6 || sin_phi < -1e-6 {
                    vy.atan2(vx / sin_phi)
                } else {
                    vy.atan2(vz / phi.cos())
                };
                let sx = ((phi * inv_pi + 1.0) * inverse_x) as f32;
                let sy = ((theta * inverse_lat + 1.0) * inverse_y) as f32;
                for (_, input, output) in channels {
                    write(output, i, sample::<E>(sx, sy, input, width, height)?)?;
                }
            }
            i = i.wrapping_add(1);
            x = x.wrapping_add(1);
        }
        Ok(())
    })
}

#[cfg(test)]
#[path = "rotate_environment_test.rs"]
mod tests;
