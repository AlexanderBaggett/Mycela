//! y2's literal odd padding and r7 affine composition over real retained maps.
use super::numeric::{add, mul, sub};
use super::{PointwiseError, SharedImageMap};
use crate::{FloatBuffer, ImageError, ImageGeometry, ProceduralExecution, SharedFloatStore};
fn read(values: &FloatBuffer, index: i32) -> Result<f32, ImageError> {
    values.read(usize::try_from(index).map_err(|_| ImageError::Index)?)
}
fn write(values: &FloatBuffer, index: i32, value: f32) -> Result<(), ImageError> {
    values.write(
        usize::try_from(index).map_err(|_| ImageError::Index)?,
        value,
    )
}
#[allow(clippy::too_many_arguments)]
pub(super) fn pad<E>(
    source: &SharedImageMap,
    width: i32,
    height: i32,
    rw: i32,
    rh: i32,
    execution: ProceduralExecution,
) -> Result<SharedImageMap, PointwiseError<E>> {
    let output = SharedImageMap::new(ImageGeometry::new(width, height));
    let x = rh.wrapping_sub(rw).wrapping_div(2).max(0);
    let y = rw.wrapping_sub(rh).wrapping_div(2).max(0);
    let mut channel = 0;
    while channel < source.channel_count()? as i32 {
        let values = source.channel(channel, true)?;
        if values.len() == 1 {
            output.set_constant(channel, values.read(0)?)?;
        } else {
            let destination = output.materialize(channel)?;
            crate::procedural_schedule::for_each_row_chunk::<E>(
                source.geometry().height(),
                execution,
                |start, count| {
                    for row in start..start.wrapping_add(count) {
                        let to = row.wrapping_add(y).wrapping_mul(width).wrapping_add(x);
                        let from = row.wrapping_mul(source.geometry().width());
                        for col in 0..source.geometry().width() {
                            let value = read(&values, from.wrapping_add(col))?;
                            write(&destination, to.wrapping_add(col), value)?;
                        }
                    }
                    Ok(())
                },
            )?;
        }
        channel += 1;
    }
    let alpha = output.materialize(3)?;
    crate::procedural_schedule::for_each_row_chunk::<E>(height, execution, |start, count| {
        for row in start..start.wrapping_add(count) {
            let at = row.wrapping_mul(width);
            if row < y || row >= rh.wrapping_add(y) {
                for col in 0..width {
                    write(&alpha, at.wrapping_add(col), 1.0)?;
                }
            } else {
                for col in 0..x {
                    write(&alpha, at.wrapping_add(col), 1.0)?;
                }
                for col in rw.wrapping_add(x)..width {
                    write(&alpha, at.wrapping_add(col), 1.0)?;
                }
            }
        }
        Ok(())
    })?;
    Ok(output)
}
fn rem<E>(value: i32, extent: i32) -> Result<i32, PointwiseError<E>> {
    value.checked_rem(extent).ok_or(if extent == 0 {
        PointwiseError::DivideByZero
    } else {
        ImageError::Overflow.into()
    })
}
fn sample<E>(
    mut x: f32,
    mut y: f32,
    source: &FloatBuffer,
    width: i32,
    height: i32,
) -> Result<f32, PointwiseError<E>> {
    if x < 0.0 {
        x = add(x % (width as f32), width as f32);
    } else if x >= width as f32 {
        x %= width as f32;
    }
    let mut left = crate::framework_cast::integer(f64::from(x));
    let mut right = left.wrapping_add(1);
    let fx = sub(x, left as f32);
    if right >= width {
        left = rem(left, width)?;
        right = rem(right, width)?;
    }
    if y < 0.0 {
        y = add(y % (height as f32), height as f32);
    } else if y >= height as f32 {
        y %= height as f32;
    }
    let mut top = crate::framework_cast::integer(f64::from(y));
    let mut bottom = top.wrapping_add(1);
    let fy = sub(y, top as f32);
    if bottom >= height {
        top = rem(top, height)?;
        bottom = rem(bottom, height)?;
    }
    let tl = read(source, left.wrapping_add(top.wrapping_mul(width)))?;
    let tr = read(source, right.wrapping_add(top.wrapping_mul(width)))?;
    let upper = add(tl, mul(fx, sub(tr, tl)));
    let bl = read(source, left.wrapping_add(bottom.wrapping_mul(width)))?;
    let br = read(source, right.wrapping_add(bottom.wrapping_mul(width)))?;
    let lower = add(bl, mul(fx, sub(br, bl)));
    Ok(add(upper, mul(fy, sub(lower, upper))))
}
pub(super) fn affine<E>(
    source: &SharedImageMap,
    size: [f64; 2],
    mut location: [f64; 2],
    effects: bool,
    execution: ProceduralExecution,
) -> Result<SharedImageMap, PointwiseError<E>> {
    let width = source.geometry().width();
    let height = source.geometry().height();
    if size[0] < 0.0 {
        location[0] = 0.0 - location[0];
    }
    if size[1] < 0.0 {
        location[1] = 0.0 - location[1];
    }
    for value in &mut location {
        *value %= 1.0;
        if *value < 0.0 {
            *value += 1.0;
        }
    }
    if size == [1.0, 1.0] && location == [0.0, 0.0] {
        return Ok(source.clone());
    }
    let abs = [size[0].abs(), size[1].abs()];
    let source = if abs[0] > 1.0 || abs[1] > 1.0 {
        super::blur::affine_preblur(
            source,
            abs[0] / 2.0 / f64::from(width) / 1.3,
            abs[1] / 2.0 / f64::from(height) / 1.3,
            execution,
        )?
    } else {
        source.clone()
    };
    let output = SharedImageMap::new(ImageGeometry::new(
        source.geometry().width(),
        source.geometry().height(),
    ));
    let mut offsets = [
        location[0] * f64::from(width),
        (location[1] + size[1] - 1.0) * f64::from(height),
    ];
    for axis in 0..2 {
        if abs[axis] == 1.0 {
            offsets[axis] = crate::round_decimal_places(offsets[axis], 0)
                .map_err(|_| ImageError::ArgumentOutOfRange)?;
        }
    }
    if size[0] < 0.0 {
        offsets[0] -= 1.0;
    }
    if size[1] < 0.0 {
        offsets[1] += 1.0;
    }
    let scales = [size[0] as f32, size[1] as f32];
    let shift = [offsets[0] as f32, offsets[1] as f32];
    let channels = if effects {
        source.all_channel_count()
    } else {
        source.channel_count()? as i32
    };
    for channel in 0..channels {
        let values = source.channel_with_effects(channel, true)?;
        if values.len() == 1 {
            output.copy_indexed_constant(channel, values.read(0)?, &source)?;
        } else {
            let destination = output.allocate_indexed(channel, &source)?;
            crate::procedural_schedule::for_each_chunk::<E>(
                width.wrapping_mul(height),
                execution,
                |start, count| {
                    let mut row = start.checked_div(width).ok_or(if width == 0 {
                        PointwiseError::DivideByZero
                    } else {
                        ImageError::Overflow.into()
                    })?;
                    let mut col = rem(start, width)?;
                    let mut sy = sub(mul(row as f32, scales[1]), shift[1]) % (height as f32);
                    if sy < 0.0 {
                        sy = add(sy, height as f32);
                    }
                    for at in start..start.wrapping_add(count) {
                        if col >= width {
                            col = 0;
                            row = row.wrapping_add(1);
                            sy = sub(mul(row as f32, scales[1]), shift[1]) % (height as f32);
                            if sy < 0.0 {
                                sy = add(sy, height as f32);
                            }
                        }
                        let mut sx = add(mul(col as f32, scales[0]), shift[0]) % (width as f32);
                        if sx < 0.0 {
                            sx = add(sx, width as f32);
                        }
                        let value = sample::<E>(sx, sy, &values, width, height)?;
                        write(&destination, at, value)?;
                        col = col.wrapping_add(1);
                    }
                    Ok(())
                },
            )?;
        }
        output.seal_channel(channel)?;
    }
    if source.region_store().is_some() {
        let values = source
            .region_store()
            .ok_or(ImageError::NullReference)?
            .data(true)?;
        let destination = crate::procedural_sampling::allocate(values.len() as i32)?;
        crate::procedural_schedule::for_each_chunk::<E>(
            width.wrapping_mul(height),
            execution,
            |start, count| {
                let mut row = start.checked_div(width).ok_or(if width == 0 {
                    PointwiseError::DivideByZero
                } else {
                    ImageError::Overflow.into()
                })?;
                let mut col = rem(start, width)?;
                for at in start..start.wrapping_add(count) {
                    if col >= width {
                        col = 0;
                        row = row.wrapping_add(1);
                    }
                    let mut sy = rem(
                        crate::framework_cast::integer(f64::from(row) * size[1] - offsets[1]),
                        height,
                    )?;
                    if sy < 0 {
                        sy = sy.wrapping_add(height);
                    }
                    let mut sx = rem(
                        crate::framework_cast::integer(f64::from(col) * size[0] + offsets[0]),
                        width,
                    )?;
                    if sx < 0 {
                        sx = sx.wrapping_add(width);
                    }
                    let value = read(&values, sx.wrapping_add(sy.wrapping_mul(width)))?;
                    write(&destination, at, value)?;
                    col = col.wrapping_add(1);
                }
                Ok(())
            },
        )?;
        output.set_region_store(Some(SharedFloatStore::new(destination)?));
    }
    Ok(output)
}
