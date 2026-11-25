//! Actual ImageMap b6/b3 resize used by Shape Blur's CDF, not a generic texture
//! library. n/e/i retain caller buffers, minimum1 chunks and literal f32 sums.
use super::numeric::{add, div, mul, sub};
use super::{PointwiseError, SharedImageMap};
use crate::{FloatBuffer, ImageError, ImageGeometry, ProceduralExecution, SharedNamedChannel};
fn ix(i: i32) -> Result<usize, ImageError> {
    usize::try_from(i).map_err(|_| ImageError::Index)
}
fn read(a: &FloatBuffer, i: i32) -> Result<f32, ImageError> {
    a.read(ix(i)?)
}
fn write(a: &FloatBuffer, i: i32, value: f32) -> Result<(), ImageError> {
    a.write(ix(i)?, value)
}
fn cast(x: f32) -> i32 {
    crate::framework_cast::integer(f64::from(x))
}
pub(super) fn half<E>(
    source: &FloatBuffer,
    destination: &FloatBuffer,
    width: i32,
    height: i32,
    sw: i32,
    execution: ProceduralExecution,
) -> Result<(), PointwiseError<E>> {
    crate::procedural_schedule::for_each_row_chunk(height, execution, |start, count| {
        let mut row = start;
        while row < start.wrapping_add(count) {
            let mut top = row.wrapping_mul(2).wrapping_mul(sw);
            let mut bottom = row.wrapping_mul(2).wrapping_add(1).wrapping_mul(sw);
            let mut at = row.wrapping_mul(width);
            let end = at.wrapping_add(width);
            while at < end {
                let a = read(source, top)?;
                let b = read(source, top.wrapping_add(1))?;
                let c = read(source, bottom)?;
                let d = read(source, bottom.wrapping_add(1))?;
                write(destination, at, mul(add(add(add(a, b), c), d), 0.25))?;
                top = top.wrapping_add(2);
                bottom = bottom.wrapping_add(2);
                at = at.wrapping_add(1);
            }
            row = row.wrapping_add(1);
        }
        Ok(())
    })
}
pub(super) fn horizontal<E>(
    source: &FloatBuffer,
    destination: &FloatBuffer,
    width: i32,
    height: i32,
    sw: i32,
    execution: ProceduralExecution,
) -> Result<(), PointwiseError<E>> {
    // Constructor operations occur before z5, including a zero work count.
    let ratio = div(sw as f32, width as f32);
    let span = cast(add(ratio, 0.5)) as f32;
    let reciprocal = div(1., span);
    crate::procedural_schedule::for_each_row_chunk(width, execution, |start, count| {
        let mut column = start;
        while column < start.wrapping_add(count) {
            let left = mul(column as f32, ratio);
            let right = add(left, span);
            let a = cast(left);
            let b = cast(right);
            let lw = sub(a.wrapping_add(1) as f32, left);
            let rw = sub(right, b as f32);
            if f64::from(lw) == 1. && f64::from(rw) == 0. {
                for row in 0..height {
                    let offset = row.wrapping_mul(sw);
                    let mut sum = read(source, a.wrapping_add(offset))?;
                    let mut i = a.wrapping_add(1);
                    while i < b {
                        sum = add(sum, read(source, i.wrapping_add(offset))?);
                        i = i.wrapping_add(1);
                    }
                    write(
                        destination,
                        column.wrapping_add(row.wrapping_mul(width)),
                        mul(sum, reciprocal),
                    )?;
                }
            } else {
                let inside = b < sw;
                for row in 0..height {
                    let offset = row.wrapping_mul(sw);
                    let mut sum = mul(read(source, a.wrapping_add(offset))?, lw);
                    sum = add(
                        sum,
                        mul(
                            read(
                                source,
                                if inside {
                                    b.wrapping_add(offset)
                                } else {
                                    offset
                                },
                            )?,
                            rw,
                        ),
                    );
                    let mut i = a.wrapping_add(1);
                    while i < b {
                        sum = add(sum, read(source, i.wrapping_add(offset))?);
                        i = i.wrapping_add(1);
                    }
                    write(
                        destination,
                        column.wrapping_add(row.wrapping_mul(width)),
                        mul(sum, reciprocal),
                    )?;
                }
            }
            column = column.wrapping_add(1);
        }
        Ok(())
    })
}
#[allow(clippy::too_many_arguments)] // Actual i ctor roles include the boundary flag.
fn vertical<E>(
    source: &FloatBuffer,
    destination: &FloatBuffer,
    width: i32,
    height: i32,
    sh: i32,
    clamp_y: bool,
    execution: ProceduralExecution,
) -> Result<(), PointwiseError<E>> {
    let ratio = div(sh as f32, height as f32);
    let span = cast(add(ratio, 0.5)) as f32;
    let reciprocal = div(1., span);
    crate::procedural_schedule::for_each_row_chunk(height, execution, |start, count| {
        let mut row = start;
        while row < start.wrapping_add(count) {
            let output = row.wrapping_mul(width);
            let top = mul(row as f32, ratio);
            let bottom = add(top, span);
            let a = cast(top);
            let b = cast(bottom);
            let next = a.wrapping_add(1);
            let tw = sub(next as f32, top);
            let bw = sub(bottom, b as f32);
            let ao = a.wrapping_mul(width);
            let bo = b.wrapping_mul(width);
            if f64::from(tw) == 1. && f64::from(bw) == 0. {
                for column in 0..width {
                    let mut sum = read(source, column.wrapping_add(ao))?;
                    let mut i = next;
                    while i < b {
                        sum = add(
                            sum,
                            read(source, column.wrapping_add(i.wrapping_mul(width)))?,
                        );
                        i = i.wrapping_add(1);
                    }
                    write(
                        destination,
                        column.wrapping_add(output),
                        mul(sum, reciprocal),
                    )?;
                }
            } else {
                let inside = b < sh;
                for column in 0..width {
                    let mut sum = mul(read(source, column.wrapping_add(ao))?, tw);
                    let endpoint = if inside {
                        column.wrapping_add(bo)
                    } else if clamp_y {
                        column.wrapping_add(bo).wrapping_sub(width)
                    } else {
                        column
                    };
                    sum = add(sum, mul(read(source, endpoint)?, bw));
                    let mut i = next;
                    while i < b {
                        sum = add(
                            sum,
                            read(source, column.wrapping_add(i.wrapping_mul(width)))?,
                        );
                        i = i.wrapping_add(1);
                    }
                    write(
                        destination,
                        column.wrapping_add(output),
                        mul(sum, reciprocal),
                    )?;
                }
            }
            row = row.wrapping_add(1);
        }
        Ok(())
    })
}
/// Source b6 chooses Cubic with false boundary and half-pixel true. The private
/// common body also retains i's literal true-boundary branch for static laws.
pub(crate) fn resize<E>(
    source: &SharedImageMap,
    width: i32,
    height: i32,
    execution: ProceduralExecution,
) -> Result<SharedImageMap, PointwiseError<E>> {
    resize_profile(source, width, height, 3, false, execution)
}
pub(crate) fn resize_clamped<E>(
    source: &SharedImageMap,
    width: i32,
    height: i32,
    execution: ProceduralExecution,
) -> Result<SharedImageMap, PointwiseError<E>> {
    resize_profile(source, width, height, 3, true, execution)
}
pub(super) fn resize_with_mode<E>(
    source: &SharedImageMap,
    width: i32,
    height: i32,
    mode: i32,
    execution: ProceduralExecution,
) -> Result<SharedImageMap, PointwiseError<E>> {
    resize_profile(source, width, height, mode, false, execution)
}
fn resize_profile<E>(
    source: &SharedImageMap,
    width: i32,
    height: i32,
    mode: i32,
    clamp_y: bool,
    execution: ProceduralExecution,
) -> Result<SharedImageMap, PointwiseError<E>> {
    let sw = source.geometry().width();
    let sh = source.geometry().height();
    if sw == width && sh == height {
        return Ok(source.clone());
    }
    let output = SharedImageMap::new(ImageGeometry::new(width, height));
    let mut channel = 0;
    if width.wrapping_mul(2) == sw && height.wrapping_mul(2) == sh {
        while channel < (source.channel_count()? as i32) {
            if let Some(previous) = source.earlier_alias(channel)? {
                let values = output.channel(previous as i32, true)?;
                output.set_buffer(channel, Some(&values))?;
            } else {
                let values = source.channel(channel, true)?;
                if values.len() == 1 {
                    output.set_constant(channel, values.read(0)?)?;
                } else {
                    let destination = output.materialize(channel)?;
                    half::<E>(&values, &destination, width, height, sw, execution)?;
                }
            }
            channel += 1;
        }
    } else {
        let mut temporary: Option<FloatBuffer> = None;
        while channel < (source.channel_count()? as i32) {
            if let Some(previous) = source.earlier_alias(channel)? {
                let values = output.channel(previous as i32, true)?;
                output.set_buffer(channel, Some(&values))?;
            } else {
                let values = source.channel(channel, true)?;
                let x = if sw <= width || values.len() == 1 {
                    values
                } else {
                    if temporary.is_none() {
                        temporary = Some(crate::procedural_sampling::allocate(
                            width.wrapping_mul(sh),
                        )?);
                    }
                    let target = temporary.as_ref().ok_or(ImageError::NullReference)?.clone();
                    horizontal::<E>(&values, &target, width, sh, sw, execution)?;
                    target
                };
                let mut y = if sh <= height || x.len() == 1 {
                    x
                } else {
                    let target = crate::procedural_sampling::allocate(
                        (if sw <= width { sw } else { width }).wrapping_mul(height),
                    )?;
                    vertical::<E>(
                        &x,
                        &target,
                        if sw <= width { sw } else { width },
                        height,
                        sh,
                        clamp_y,
                        execution,
                    )?;
                    target
                };
                if y.len() > 1 {
                    let rw = if sw <= width { sw } else { width };
                    let rh = if sh <= height { sh } else { height };
                    if rw != width || rh != height {
                        let target =
                            crate::procedural_sampling::allocate(width.wrapping_mul(height))?;
                        crate::procedural_sampling::resample_resize(
                            mode,
                            Some(&y),
                            rw,
                            rh,
                            Some(&target),
                            width,
                            height,
                            execution,
                            clamp_y,
                        )?;
                        y = target;
                    }
                }
                if !y.is_empty() {
                    output.set_buffer(channel, Some(&y))?;
                }
                if temporary.as_ref().is_some_and(|t| t.shares_storage(&y)) {
                    temporary = None;
                }
            }
            output.seal_channel(channel)?;
            channel += 1;
        }
    }
    let mut named = 0;
    while named < source.named_channels().len() {
        let child = source
            .named_channel(named as i32)?
            .ok_or(ImageError::NullReference)?;
        let intermediate = source.geometry_copy();
        intermediate.set_store_reference(0, child.data().as_ref())?;
        let resized = resize_profile::<E>(&intermediate, width, height, mode, clamp_y, execution)?;
        let data = resized.channel(0, true)?;
        output.append_named(SharedNamedChannel::new(
            Some(child.name_utf16().to_vec()),
            Some(crate::SharedFloatStore::new(data)?),
        ))?;
        output.seal_named()?;
        named += 1;
    }
    Ok(output)
}
