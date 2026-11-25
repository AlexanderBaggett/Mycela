//! Pixelate aoy 1b09/1b0d and four distinct workers. None is raw Single copy;
//! arithmetic paths retain f32 operations, unlike the Noise resamplers.
use super::super::{
    min64,
    numeric::{add, div, mul, sub},
};
use super::{PointwiseError, SharedImageMap, at, named_tail, read, write};
use crate::procedural_schedule::{for_each_chunk, for_each_row_chunk};
use crate::{
    FloatBuffer, ImageError, ImageGeometry, ProceduralExecution, framework_cast::integer,
    round_decimal_places,
};

/// Already evaluated paired Pixels, actual Factor, and smoothing index.
/// These are caller operands, not a replacement retained property authority.
#[derive(Clone, Copy, Debug)]
pub struct PixelateOptions {
    pixels: [f64; 2],
    factor: f64,
    smoothing: i32,
    execution: ProceduralExecution,
}
impl PixelateOptions {
    pub const fn new(
        pixels: [f64; 2],
        factor: f64,
        smoothing: i32,
        execution: ProceduralExecution,
    ) -> Self {
        Self {
            pixels,
            factor,
            smoothing,
            execution,
        }
    }
    pub const fn pixels(&self) -> [f64; 2] {
        self.pixels
    }
    pub const fn factor(&self) -> f64 {
        self.factor
    }
    pub const fn smoothing(&self) -> i32 {
        self.smoothing
    }
    pub const fn execution(&self) -> ProceduralExecution {
        self.execution
    }
}
pub fn render_pixelate<E>(
    input: Option<&SharedImageMap>,
    options: &PixelateOptions,
    geometry: ImageGeometry,
) -> Result<SharedImageMap, PointwiseError<E>> {
    render_with_inputs(
        input,
        geometry,
        options.execution,
        &mut || {
            Ok([
                options.pixels[0] * options.factor,
                options.pixels[1] * options.factor,
            ])
        },
        &mut || Ok(options.smoothing),
    )
}
/// The future real retained adapter can preserve late repeated choice reads;
/// this private seam is not a production fixed-success provider or fixture API.
pub(super) fn render_with_inputs<E>(
    input: Option<&SharedImageMap>,
    geometry: ImageGeometry,
    execution: ProceduralExecution,
    pixels: &mut impl FnMut() -> Result<[f64; 2], PointwiseError<E>>,
    smoothing: &mut impl FnMut() -> Result<i32, PointwiseError<E>>,
) -> Result<SharedImageMap, PointwiseError<E>> {
    let input = input.ok_or(PointwiseError::NullReference)?;
    if input.is_empty()? {
        return Ok(input.clone());
    }
    let out = SharedImageMap::new(geometry);
    let values = pixels()?;
    let nx = integer(min64(
        round_decimal_places(values[0], 0).map_err(|_| ImageError::ArgumentOutOfRange)?,
        f64::from(geometry.width()),
    ));
    let ny = integer(min64(
        round_decimal_places(values[1], 0).map_err(|_| ImageError::ArgumentOutOfRange)?,
        f64::from(geometry.height()),
    ));
    let sx = div(geometry.width() as f32, nx as f32);
    let sy = div(geometry.height() as f32, ny as f32);
    // aoy loops static zl.g=4, not input.ng(). Extra input channels are not read.
    for channel in 0..4 {
        let source = input.channel(channel, false)?;
        if source.len() == 1 {
            out.set_constant(channel, source.read(0)?)?;
        } else {
            let destination = out.materialize(channel)?;
            if smoothing()? == 0 {
                let offset = [
                    integer(f64::from(div(sx, 2.0))),
                    integer(f64::from(div(sy, 2.0))),
                ];
                for_each_chunk(geometry.area(), execution, |start, count| {
                    nearest(
                        Some(&source),
                        Some(&destination),
                        geometry,
                        [sx, sy],
                        offset,
                        start,
                        count,
                    )
                })?;
            } else if smoothing()? == 1 {
                for_each_row_chunk(geometry.height(), execution, |start, count| {
                    linear(
                        Some(&source),
                        Some(&destination),
                        geometry,
                        [nx, ny],
                        [sx, sy],
                        false,
                        start,
                        count,
                    )
                })?;
            } else if smoothing()? == 2 {
                for_each_row_chunk(geometry.height(), execution, |start, count| {
                    linear(
                        Some(&source),
                        Some(&destination),
                        geometry,
                        [nx, ny],
                        [sx, sy],
                        true,
                        start,
                        count,
                    )
                })?;
            } else if smoothing()? == 3 {
                for_each_chunk(geometry.area(), execution, |start, count| {
                    cubic(
                        Some(&source),
                        Some(&destination),
                        geometry,
                        [nx, ny],
                        [sx, sy],
                        start,
                        count,
                    )
                })?;
            }
        }
        out.seal_channel(channel)?;
    }
    named_tail(&out, input)?;
    Ok(out)
}
fn quotient<E>(a: i32, b: i32) -> Result<i32, PointwiseError<E>> {
    a.checked_div(b).ok_or(if b == 0 {
        PointwiseError::DivideByZero
    } else {
        ImageError::Overflow.into()
    })
}
fn remainder<E>(a: i32, b: i32) -> Result<i32, PointwiseError<E>> {
    a.checked_rem(b).ok_or(if b == 0 {
        PointwiseError::DivideByZero
    } else {
        ImageError::Overflow.into()
    })
}
fn trunc32(value: f32) -> i32 {
    integer(f64::from(value))
}
fn coarse(position: i32, step: f32) -> i32 {
    trunc32(mul(trunc32(div(position as f32, step)) as f32, step))
}
#[allow(clippy::too_many_arguments)] // Actual worker fields plus scheduler interval.
fn nearest<E>(
    source: Option<&FloatBuffer>,
    destination: Option<&FloatBuffer>,
    geometry: ImageGeometry,
    steps: [f32; 2],
    offset: [i32; 2],
    start: i32,
    count: i32,
) -> Result<(), PointwiseError<E>> {
    let width = geometry.width();
    let [sx, sy] = steps;
    let mut y = quotient(start, width)?;
    let mut x = remainder(start, width)?;
    let end = start.wrapping_add(count);
    let mut row = coarse(y, sy).wrapping_add(offset[1]).wrapping_mul(width);
    let mut index = start;
    while index < end {
        if x >= width {
            x = 0;
            y = y.wrapping_add(1);
            row = coarse(y, sy).wrapping_add(offset[1]).wrapping_mul(width);
        }
        let value = read(
            source,
            coarse(x, sx).wrapping_add(offset[0]).wrapping_add(row),
        )?;
        write(destination, index, value)?; // No widening, arithmetic, or NaN quieting.
        index = index.wrapping_add(1);
        x = x.wrapping_add(1);
    }
    Ok(())
}
fn ease(fraction: f32) -> f32 {
    div(
        sub(
            1.0,
            libm::cos(f64::from(fraction) * std::f64::consts::PI) as f32,
        ),
        2.0,
    )
}
#[allow(clippy::too_many_arguments)]
fn linear<E>(
    source: Option<&FloatBuffer>,
    destination: Option<&FloatBuffer>,
    geometry: ImageGeometry,
    counts: [i32; 2],
    steps: [f32; 2],
    cosine: bool,
    start: i32,
    count: i32,
) -> Result<(), PointwiseError<E>> {
    let width = geometry.width();
    let [nx, ny] = counts;
    let [sx, sy] = steps;
    let mut y = start;
    while y < start.wrapping_add(count) {
        let position = div(y as f32, sy);
        let coarse_y = trunc32(position);
        let fraction = sub(position, coarse_y as f32);
        let fy = if cosine { ease(fraction) } else { fraction };
        let row = trunc32(mul(coarse_y as f32, sy)).wrapping_mul(width);
        let mut next_y = coarse_y.wrapping_add(1);
        if next_y >= ny {
            next_y = next_y.wrapping_sub(ny);
        }
        let next_row = trunc32(mul(next_y as f32, sy)).wrapping_mul(width);
        let mut x = 0i32;
        while x < width {
            let position = div(x as f32, sx);
            let coarse_x = trunc32(position);
            let fraction = sub(position, coarse_x as f32);
            let fx = if cosine { ease(fraction) } else { fraction };
            let column = trunc32(mul(coarse_x as f32, sx));
            let mut next_x = coarse_x.wrapping_add(1);
            if next_x >= nx {
                next_x = next_x.wrapping_sub(nx);
            }
            let next_column = trunc32(mul(next_x as f32, sx));
            let tl = mul(
                mul(read(source, column.wrapping_add(row))?, sub(1.0, fx)),
                sub(1.0, fy),
            );
            let bl = mul(
                mul(read(source, column.wrapping_add(next_row))?, sub(1.0, fx)),
                fy,
            );
            let mut value = add(tl, bl);
            let tr = mul(
                mul(read(source, next_column.wrapping_add(row))?, fx),
                sub(1.0, fy),
            );
            value = add(value, tr);
            let br = mul(
                mul(read(source, next_column.wrapping_add(next_row))?, fx),
                fy,
            );
            value = add(value, br);
            write(destination, at(x, y, width), value)?;
            x = x.wrapping_add(1);
        }
        y = y.wrapping_add(1);
    }
    Ok(())
}
// Exact one-argument ImageMap.b00ad, not the unrelated four-out polynomial.
fn basis(x: f64) -> f64 {
    let mut n = x + 2.0;
    let mut result = n * n * n * (1.0 / 6.0);
    if x > -1.0 {
        n = x + 1.0;
        result -= n * n * n * (2.0 / 3.0);
        if x > 0.0 {
            result += x * x * x;
            if x > 1.0 {
                n = x - 1.0;
                return result - n * n * n * (2.0 / 3.0);
            }
        }
    }
    result
}
fn tap<E>(index: i32, count: i32) -> Result<i32, PointwiseError<E>> {
    if index < 0 {
        Ok(index.wrapping_add(count))
    } else if index >= count {
        remainder(index, count)
    } else {
        Ok(index)
    }
}
#[allow(clippy::too_many_arguments)]
fn cubic<E>(
    source: Option<&FloatBuffer>,
    destination: Option<&FloatBuffer>,
    geometry: ImageGeometry,
    counts: [i32; 2],
    steps: [f32; 2],
    start: i32,
    count: i32,
) -> Result<(), PointwiseError<E>> {
    let width = geometry.width();
    let [nx, ny] = counts;
    let [sx, sy] = steps;
    let mut y = quotient(start, width)?;
    let mut x = remainder(start, width)?;
    let end = start.wrapping_add(count);
    let mut position = div(y as f32, sy);
    let mut coarse_y = trunc32(position);
    let mut fy = sub(position, coarse_y as f32);
    let mut index = start;
    while index < end {
        if x >= width {
            x = 0;
            y = y.wrapping_add(1);
            position = div(y as f32, sy);
            coarse_y = trunc32(position);
            fy = sub(position, coarse_y as f32);
        }
        let mut value = 0.0f32;
        let position = div(x as f32, sx);
        let coarse_x = trunc32(position);
        let fx = sub(position, coarse_x as f32);
        for dy in -1i32..=2 {
            let sample_y = trunc32(mul(tap(coarse_y.wrapping_add(dy), ny)? as f32, sy));
            // Y basis BEFORE inner samples, fraction-minus-tap.
            let wy = basis(f64::from(sub(fy, dy as f32))) as f32;
            let row = sample_y.wrapping_mul(width);
            for dx in -1i32..=2 {
                let sample_x = trunc32(mul(tap(coarse_x.wrapping_add(dx), nx)? as f32, sx));
                let sample = read(source, sample_x.wrapping_add(row))?;
                // X basis AFTER this source read, tap-minus-fraction.
                let wx = basis(f64::from(sub(dx as f32, fx))) as f32;
                value = add(value, mul(mul(sample, wx), wy));
            }
        }
        write(destination, index, value)?;
        index = index.wrapping_add(1);
        x = x.wrapping_add(1);
    }
    Ok(())
}

#[cfg(test)]
#[path = "spatial_pixelate_test.rs"]
mod tests;
