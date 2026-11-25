//! zj's ten analytic workers, covering all thirteen literal modes. These are
//! source-body translations, not new source goldens or generic trig/JIT laws.
//! Large images run the original row and pixel partitions on scoped workers.
//! A caller may retire a job between small blocks of pixel writes.
use super::{PointwiseError, check_render_cancelled};
use crate::{FloatBuffer, ImageError, ImageGeometry, ProceduralExecution};
use std::{f64::consts::PI, ops::Range};

#[path = "gradient_workers.rs"]
mod workers;
use workers::{BLOCK_PIXELS, PixelBlock, PixelSink, WorkerError, WorkerResult};

/// Phase setup shared only by branches with the same source operations. Conical
/// has the different negative-X correction and retains a separate pixel formula.
struct Phase {
    width: f64,
    height: f64,
    x_period: f64,
    size: [f64; 2],
    x: f64,
    y: i32,
}
impl Phase {
    fn new(
        width: i32,
        height: i32,
        size: [f64; 2],
        mut location: [f64; 2],
        conical: bool,
    ) -> WorkerResult<Self> {
        if size[0] < 0.0 {
            location[0] += size[0] % 1.0;
        }
        if size[1] > 0.0 {
            location[1] += size[1] % 1.0;
        }
        Self::from_worker_location(width, height, size, location, conical)
    }
    fn from_worker_location(
        width: i32,
        height: i32,
        size: [f64; 2],
        location: [f64; 2],
        conical: bool,
    ) -> WorkerResult<Self> {
        let mut x = location[0] * f64::from(width) % f64::from(width);
        if x < 0.0 {
            x += f64::from(width);
        }
        if size[0] < 0.0 {
            x = 0.0 - x - if conical { 0.0 } else { 1.0 };
        }
        let raw_y = crate::framework_cast::integer(location[1] * f64::from(height));
        // Source uses integer REM, not floating wrapping. Ordinary render
        // allocation fails before zero-area inputs reach this point.
        let mut y = raw_y.checked_rem(height).ok_or(if height == 0 {
            WorkerError::DivideByZero
        } else {
            WorkerError::Image(ImageError::Overflow)
        })?;
        if size[1] < 0.0 {
            y = y.wrapping_neg().wrapping_add(1);
        }
        Ok(Self {
            width: f64::from(width),
            height: f64::from(height),
            x_period: f64::from(width),
            size,
            x,
            y,
        })
    }
    fn x(&self, col: i32) -> f64 {
        let mut x = (f64::from(col) * self.size[0] + self.x) % self.x_period;
        if self.size[0] < 0.0 {
            x += self.width;
        }
        x
    }
    fn y(&self, row: i32, seam: bool) -> f64 {
        let mut y =
            (f64::from(row.wrapping_neg()) * self.size[1] + f64::from(self.y)) % self.height;
        if seam {
            y -= 0.001;
        }
        if y < 0.0 {
            y += self.height;
        }
        y
    }
}
fn write(out: &mut PixelSink<'_>, width: i32, row: i32, col: i32, value: f32) -> WorkerResult {
    let ix = row.wrapping_mul(width).wrapping_add(col);
    out.write(usize::try_from(ix).map_err(|_| WorkerError::Index)?, value)?;
    Ok(())
}
// Pixel workers iterate wrapped AREA, not nested height/width loops. Two
// negative dimensions can have a positive area and still execute these workers.
fn pixels(width: i32, range: Range<i32>) -> impl Iterator<Item = (usize, i32, i32)> {
    let end = range.end;
    let mut ix = range.start;
    let (mut row, mut col) = (ix / width, ix % width);
    std::iter::from_fn(move || {
        if ix >= end {
            return None;
        }
        if col >= width {
            col = 0;
            row = row.wrapping_add(1);
        }
        let result = (ix as usize, row, col);
        ix = ix.wrapping_add(1);
        col = col.wrapping_add(1);
        Some(result)
    })
}
struct WorkRange<'a> {
    range: Range<i32>,
    cancelled: &'a dyn Fn() -> bool,
    written: usize,
}

impl WorkRange<'_> {
    #[inline]
    fn before_pixel(&mut self) -> WorkerResult {
        // Poll inside each source range, including a single wide row or
        // a flat work unit executed entirely on the calling thread.
        if self.written.is_multiple_of(BLOCK_PIXELS) && (self.cancelled)() {
            return Err(WorkerError::Cancelled);
        }
        self.written += 1;
        Ok(())
    }
}

fn ramp_coefficients(
    vertical: bool,
    width: i32,
    height: i32,
    size: [f64; 2],
    location: [f64; 2],
) -> [f32; 2] {
    let axis = usize::from(vertical);
    let dimension = if vertical { height } else { width };
    let mut step = (size[axis] / f64::from(dimension)) as f32;
    if step < 0.0 {
        step += 1.0;
    }
    let mut offset = if vertical {
        0.0 - location[1] as f32
    } else {
        location[0] as f32
    };
    if size[axis] < 0.0 {
        // Both source branches divide this correction by WIDTH.
        offset = 0.0 - offset - 1.0 / width as f32;
    }
    [step, offset]
}

// Horizontal and Vertical deliberately use f32 throughout their worker body.
fn ramp(
    vertical: bool,
    out: &mut PixelSink<'_>,
    width: i32,
    height: i32,
    [step, offset]: [f32; 2],
    mut work: WorkRange<'_>,
) -> WorkerResult {
    for unit in work.range.clone() {
        let mut value = (unit as f32 * step + offset) % 1.0;
        if value < 0.0 {
            value += 1.0;
        }
        if vertical {
            for col in 0..width {
                work.before_pixel()?;
                write(out, width, unit, col, value)?;
            }
        } else {
            for row in 0..height {
                work.before_pixel()?;
                write(out, width, row, unit, value)?;
            }
        }
    }
    Ok(())
}
fn conical(
    out: &mut PixelSink<'_>,
    p: &Phase,
    [half_x, half_y]: [f64; 2],
    factor: f64,
    mut work: WorkRange<'_>,
) -> WorkerResult {
    for (ix, row, col) in pixels(p.width as i32, work.range.clone()) {
        work.before_pixel()?;
        let y = p.y(row, true) - half_y;
        let mut x = (f64::from(col.wrapping_neg()) * p.size[0] - p.x) % p.x_period + half_x;
        if p.size[0] < 0.0 {
            x -= p.width;
        }
        out.write(ix, ((libm::atan2(y, x) + PI) * factor) as f32)?;
    }
    Ok(())
}
fn radial(
    out: &mut PixelSink<'_>,
    p: &Phase,
    [half_x, half_y]: [f64; 2],
    factor: f64,
    mut work: WorkRange<'_>,
) -> WorkerResult {
    for (ix, row, col) in pixels(p.width as i32, work.range.clone()) {
        work.before_pixel()?;
        let y = p.y(row, false) - half_y;
        let y2 = y * y;
        // Radial subtracts the center BEFORE its negative-Size correction.
        let mut x = (f64::from(col) * p.size[0] + p.x) % p.x_period - half_x;
        if p.size[0] < 0.0 {
            x += p.width;
        }
        out.write(ix, (1.0 - libm::sqrt(x * x + y2) * factor) as f32)?;
    }
    Ok(())
}
fn pyramid(
    dimpled: bool,
    out: &mut PixelSink<'_>,
    p: &Phase,
    [half_x, half_y]: [f64; 2],
    [recip_x, recip_y]: [f64; 2],
    diagonal_boundary: f64,
    mut work: WorkRange<'_>,
) -> WorkerResult {
    for (ix, row, col) in pixels(p.width as i32, work.range.clone()) {
        work.before_pixel()?;
        let mut y = p.y(row, true);
        let mut sign_y = 0.0;
        if dimpled {
            y *= 2.0;
            if y >= p.height {
                y -= p.height;
                sign_y = -0.5;
            } else {
                sign_y = 0.5;
            }
        }
        let mut x = p.x(col);
        let mut sign_x = 0.0;
        if dimpled {
            x *= 2.0;
            if x >= p.width {
                x -= p.width;
                sign_x = 0.0 - sign_y;
            } else {
                sign_x = sign_y;
            }
        }
        // Keep the source comparison tree, including unordered branches.
        let value = if x < y {
            if !y.lt(&(diagonal_boundary - x)) {
                1.0 - (y - half_y) * recip_y
            } else {
                x * recip_x
            }
        } else if !y.lt(&(diagonal_boundary - x)) {
            1.0 - (x - half_x) * recip_x
        } else {
            y * recip_y
        };
        out.write(
            ix,
            if dimpled {
                (value * sign_x + 0.5) as f32
            } else {
                value as f32
            },
        )?;
    }
    Ok(())
}
fn sine_product(
    dimples: bool,
    out: &mut PixelSink<'_>,
    p: &Phase,
    [fx, fy]: [f64; 2],
    mut work: WorkRange<'_>,
) -> WorkerResult {
    for (ix, row, col) in pixels(p.width as i32, work.range.clone()) {
        work.before_pixel()?;
        let sin_y = libm::sin(p.y(row, true) * fy);
        let value = libm::sin(p.x(col) * fx) * sin_y;
        out.write(
            ix,
            if dimples {
                (value * 0.5 + 0.5) as f32
            } else {
                value as f32
            },
        )?;
    }
    Ok(())
}
fn star(
    dimpled: bool,
    out: &mut PixelSink<'_>,
    p: &Phase,
    mut work: WorkRange<'_>,
) -> WorkerResult {
    let width = p.width as i32;
    let fx = PI / p.width;
    let fy = PI / p.height;
    let quarter = PI / 2.0;
    for row in work.range.clone() {
        let mut y = p.y(row, true);
        let mut sign_y = 0.0;
        if dimpled {
            y *= 2.0;
            if y >= p.height {
                y -= p.height;
                sign_y = -0.5;
            } else {
                sign_y = 0.5;
            }
        }
        y *= fy;
        let sin_y = libm::sin(y);
        let mut shifted_y = y + quarter;
        if shifted_y >= PI {
            shifted_y -= PI;
        }
        let shifted_sin_y = libm::sin(shifted_y);
        for col in 0..width {
            work.before_pixel()?;
            let mut x = p.x(col);
            let mut sign_x = 0.0;
            if dimpled {
                x *= 2.0;
                if x >= p.width {
                    x -= p.width;
                    sign_x = 0.0 - sign_y;
                } else {
                    sign_x = sign_y;
                }
            }
            x *= fx;
            let first = libm::sin(x) * sin_y;
            let mut shifted_x = x + quarter;
            if shifted_x >= PI {
                shifted_x -= PI;
            }
            let second = libm::sin(shifted_x) * shifted_sin_y;
            let sum = first + second;
            let value = if sum != 0.0 { first / sum } else { 0.0 };
            write(
                out,
                width,
                row,
                col,
                if dimpled {
                    (value * sign_x + 0.5) as f32
                } else {
                    value as f32
                },
            )?;
        }
    }
    Ok(())
}
fn pillow(
    dimpled: bool,
    out: &mut PixelSink<'_>,
    p: &Phase,
    mut work: WorkRange<'_>,
) -> WorkerResult {
    let width = p.width as i32;
    let fx = PI / p.width;
    let fy = PI / p.height;
    for row in work.range.clone() {
        let mut y = p.y(row, true);
        let mut sign_y = 0.0;
        if dimpled {
            y *= 2.0;
            if y >= p.height {
                y -= p.height;
                sign_y = -0.5;
            } else {
                sign_y = 0.5;
            }
        }
        let sin_y = libm::sin(y * fy);
        for col in 0..width {
            work.before_pixel()?;
            let mut x = p.x(col);
            let mut sign_x = 0.0;
            if dimpled {
                x *= 2.0;
                if x >= p.width {
                    x -= p.width;
                    sign_x = 0.0 - sign_y;
                } else {
                    sign_x = sign_y;
                }
            }
            x *= fx;
            let numerator = libm::sin(x) * sin_y;
            let cosine = libm::cos(0.0 - x);
            let square = cosine * cosine;
            let sum = numerator + square;
            let value = if sum != 0.0 { numerator / sum } else { 0.0 };
            write(
                out,
                width,
                row,
                col,
                if dimpled {
                    (value * sign_x + 0.5) as f32
                } else {
                    value as f32
                },
            )?;
        }
    }
    Ok(())
}
fn saddle(
    out: &mut PixelSink<'_>,
    p: &Phase,
    [fx, fy]: [f64; 2],
    cosine_shift: f64,
    mut work: WorkRange<'_>,
) -> WorkerResult {
    for (ix, row, col) in pixels(p.width as i32, work.range.clone()) {
        work.before_pixel()?;
        let sin_y = libm::sin(p.y(row, true) * fy);
        let x = p.x(col) * fx;
        let numerator = libm::sin(x) * sin_y;
        let cosine = libm::cos(cosine_shift - x);
        let square = cosine * cosine;
        let sum = numerator + square;
        let value = if numerator == 0.0 || sum == 0.0 {
            1.0
        } else {
            numerator / sum
        };
        out.write(ix, value as f32)?;
    }
    Ok(())
}

pub(super) fn render<E>(
    mode: usize,
    out: &FloatBuffer,
    geometry: ImageGeometry,
    size: [f64; 2],
    location: [f64; 2],
    execution: ProceduralExecution,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), PointwiseError<E>> {
    use crate::procedural_schedule::{chunks, row_chunks};

    let (width, height) = (geometry.width(), geometry.height());
    let ranges = match mode {
        0 => row_chunks(width, execution),
        1 | 6 | 7 | 11 | 12 => row_chunks(height, execution),
        2..=5 | 8..=10 => chunks(width.wrapping_mul(height), execution),
        _ => Err(PointwiseError::Argument),
    }?;
    let run = |(start, count): (i32, i32), stopped: &dyn Fn() -> bool, buffered: bool| {
        let work = WorkRange {
            range: start..start.wrapping_add(count),
            cancelled: stopped,
            written: 0,
        };
        let mut block = PixelBlock::new(if mode == 0 { width as usize } else { 1 });
        let mut sink = PixelSink::new(out, buffered.then_some(&mut block));
        if mode <= 1 {
            let vertical = mode == 1;
            let coefficients = ramp_coefficients(vertical, width, height, size, location);
            ramp(vertical, &mut sink, width, height, coefficients, work)?;
        } else {
            let p = Phase::new(width, height, size, location, mode == 2)?;
            let center = [p.width / 2.0, p.height / 2.0];
            let frequency = [PI / p.width, PI / p.height];
            match mode {
                2 => conical(&mut sink, &p, center, 1.0 / (2.0 * PI), work),
                3 => {
                    let diagonal =
                        libm::sqrt(libm::pow(center[0], 2.0) + libm::pow(center[1], 2.0));
                    radial(&mut sink, &p, center, 1.0 / diagonal, work)
                }
                4 | 10 => pyramid(
                    mode == 10,
                    &mut sink,
                    &p,
                    center,
                    [1.0 / center[0], 1.0 / center[1]],
                    p.height,
                    work,
                ),
                5 => sine_product(false, &mut sink, &p, frequency, work),
                6 | 11 => star(mode == 11, &mut sink, &p, work),
                7 | 12 => pillow(mode == 12, &mut sink, &p, work),
                8 => saddle(&mut sink, &p, frequency, PI / 2.0, work),
                // Preserve the original reciprocal expression for Dimples.
                9 => sine_product(
                    true,
                    &mut sink,
                    &p,
                    [1.0 / (p.width / PI / 2.0), 1.0 / (p.height / PI / 2.0)],
                    work,
                ),
                _ => unreachable!("validated Gradient mode"),
            }?;
        }
        sink.finish()
    };

    let count = workers::worker_count(execution, width, height, out.len()).min(ranges.len());
    if count > 1 {
        workers::run_parallel(&ranges, count, cancelled, &|range, stopped| {
            run(range, stopped, true)
        })?;
    } else {
        for range in ranges {
            run(range, cancelled, false)?;
        }
    }
    check_render_cancelled(cancelled)
}

#[cfg(test)]
#[path = "gradient_worker_test.rs"]
mod source_worker_fixture;
