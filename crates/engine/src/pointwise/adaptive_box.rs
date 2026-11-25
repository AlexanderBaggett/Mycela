//! Variable Blur running-window workers. Scratch is reset per render chunk.
use super::adaptive::{AxisSettings, clamp, floats, read, write};
use super::numeric::{add, div, mul, sub};
use super::{PointwiseError, max32, min32};
use crate::{FloatBuffer, ImageError, ProceduralExecution};
fn ints(n: i32) -> Result<Vec<i32>, ImageError> {
    let n = usize::try_from(n).map_err(|_| ImageError::Overflow)?;
    let mut v = Vec::new();
    v.try_reserve_exact(n).map_err(|_| ImageError::Allocation)?;
    v.resize(n, 0);
    Ok(v)
}
struct Axis<'a> {
    source: &'a FloatBuffer,
    dest: &'a FloatBuffer,
    mask: &'a FloatBuffer,
    settings: AxisSettings,
    vertical: bool,
}
impl Axis<'_> {
    fn neighbor(&self, cell: i32, coordinate: i32, offset: i32, lower: bool) -> Option<i32> {
        let width = self.settings.geometry.width();
        let height = self.settings.geometry.height();
        let (step, extent) = if self.vertical {
            (width, height)
        } else {
            (1, width)
        };
        let position = coordinate.wrapping_add(offset);
        let offset = if (lower && position < 0) || (!lower && position >= extent) {
            if !self.settings.seamless {
                return None;
            }
            position.rem_euclid(extent) - coordinate
        } else {
            offset
        };
        Some(cell.wrapping_add(offset.wrapping_mul(step)))
    }
    fn contribution(
        &self,
        cell: i32,
        coordinate: i32,
        offset: i32,
        lower: bool,
    ) -> Result<Option<f32>, ImageError> {
        self.neighbor(cell, coordinate, offset, lower)
            .map(|at| read(self.source, at))
            .transpose()
    }
    #[allow(clippy::too_many_arguments)] // Literal left/right interval operands; bounds checks differ by source site.
    fn interval(
        &self,
        sums: &FloatBuffer,
        ix: i32,
        cell: i32,
        coordinate: i32,
        first: i32,
        last: i32,
        subtract: bool,
        lower: bool,
    ) -> Result<(), ImageError> {
        let mut offset = first;
        while offset < last {
            if let Some(value) = self.contribution(cell, coordinate, offset, lower)? {
                let old = read(sums, ix)?;
                write(
                    sums,
                    ix,
                    if subtract {
                        sub(old, value)
                    } else {
                        add(old, value)
                    },
                )?;
            }
            offset = offset.wrapping_add(1);
        }
        Ok(())
    }
    fn execute<E>(&self, execution: ProceduralExecution) -> Result<(), PointwiseError<E>> {
        let s = self.settings;
        let width = s.geometry.width();
        let height = s.geometry.height();
        let area = width.wrapping_mul(height);
        let dense = s.present && self.mask.len() > 1;
        crate::procedural_schedule::for_each_chunk(area, execution, |start, count| {
            if width == 0 {
                return Err(PointwiseError::DivideByZero);
            }
            let mut row = start / width;
            let mut col = start % width;
            let end = start.wrapping_add(count);
            let mut previous = 0i32;
            let sums = floats(count)?;
            let denominators = floats(count)?;
            // Actual vertical ctor/bx allocates this AFTER the two Single arrays.
            let mut previous_columns = if self.vertical {
                ints(width)?
            } else {
                Vec::new()
            };
            for cell_counter in start..end {
                if col >= width {
                    col = 0;
                    row = row.wrapping_add(1);
                }
                let cell = col.wrapping_add(row.wrapping_mul(width));
                let mut fractional = 0.;
                let mut radius = if s.present {
                    mul(clamp(read(self.mask, 0)?), s.radius)
                } else {
                    s.radius
                };
                if dense {
                    radius = mul(clamp(read(self.mask, cell)?), s.radius);
                }
                if self.vertical {
                    // The previous sum comes from the same column one row earlier.
                    previous = previous_columns[col as usize];
                } else if col == 0 {
                    previous = 0;
                }
                radius = mul(radius, (width / 3) as f32);
                let span = crate::framework_cast::integer(f64::from(radius));
                let fraction = sub(radius, span as f32);
                let coordinate = if self.vertical { row } else { col };
                let extent = if self.vertical { height } else { width };
                let denominator = if s.seamless {
                    add(mul(radius, 2.), 1.)
                } else {
                    add(
                        sub(
                            min32(
                                add(coordinate as f32, radius),
                                extent.wrapping_sub(1) as f32,
                            ),
                            max32(sub(coordinate as f32, radius), 0.),
                        ),
                        1.,
                    )
                };
                let ix = cell_counter.wrapping_sub(start);
                let difference = span.wrapping_sub(previous);
                let rebuild = difference.checked_abs().ok_or(ImageError::Overflow)? > span;
                if fraction > 0. {
                    let edge = span.wrapping_add(1);
                    if let Some(v) =
                        self.contribution(cell, coordinate, edge.wrapping_neg(), true)?
                    {
                        fractional = add(fractional, mul(v, fraction));
                    }
                    if let Some(v) = self.contribution(cell, coordinate, edge, false)? {
                        fractional = add(fractional, mul(v, fraction));
                    }
                }
                if previous == 0 || rebuild {
                    self.interval(
                        &sums,
                        ix,
                        cell,
                        coordinate,
                        span.wrapping_neg(),
                        0,
                        false,
                        true,
                    )?;
                    self.interval(
                        &sums,
                        ix,
                        cell,
                        coordinate,
                        1,
                        span.wrapping_add(1),
                        false,
                        false,
                    )?;
                } else {
                    let step = if self.vertical { width } else { 1 };
                    write(&sums, ix, read(&sums, ix.wrapping_sub(step))?)?;
                    write(
                        &sums,
                        ix,
                        add(
                            read(&sums, ix)?,
                            read(self.source, cell.wrapping_sub(step))?,
                        ),
                    )?;
                    write(&sums, ix, sub(read(&sums, ix)?, read(self.source, cell)?))?;
                    let new_left = span.wrapping_neg();
                    let old_left = previous.wrapping_neg().wrapping_sub(1);
                    if new_left < old_left {
                        self.interval(
                            &sums, ix, cell, coordinate, new_left, old_left, false, true,
                        )?;
                    } else {
                        self.interval(&sums, ix, cell, coordinate, old_left, new_left, true, true)?;
                    }
                    let new_right = span.wrapping_add(1);
                    let old_right = previous;
                    if new_right < old_right {
                        self.interval(
                            &sums, ix, cell, coordinate, new_right, old_right, true, false,
                        )?;
                    } else {
                        self.interval(
                            &sums, ix, cell, coordinate, old_right, new_right, false, false,
                        )?;
                    }
                }
                write(self.dest, cell, add(read(self.source, cell)?, fractional))?;
                write(&denominators, ix, denominator)?;
                if self.vertical {
                    previous_columns[col as usize] = span;
                } else {
                    previous = span;
                }
                col = col.wrapping_add(1);
            }
            for cell in start..end {
                let ix = cell.wrapping_sub(start);
                write(
                    self.dest,
                    cell,
                    add(read(self.dest, cell)?, read(&sums, ix)?),
                )?;
                write(
                    self.dest,
                    cell,
                    div(read(self.dest, cell)?, read(&denominators, ix)?),
                )?;
            }
            Ok(())
        })
    }
}
pub(super) fn axis<E>(
    source: &FloatBuffer,
    dest: &FloatBuffer,
    mask: &FloatBuffer,
    settings: AxisSettings,
    vertical: bool,
    execution: ProceduralExecution,
) -> Result<(), PointwiseError<E>> {
    Axis {
        source,
        dest,
        mask,
        settings,
        vertical,
    }
    .execute(execution)
}
