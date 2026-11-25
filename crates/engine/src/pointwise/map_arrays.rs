//! r7's retained scalar/dense map operations. Branches intentionally preserve
//! zero/one shortcuts, loop lengths, partial writes and output-root choices.
use super::numeric::{add, mul, sub};
use super::{ArithmeticOperation as Op, ArithmeticWorker};
use crate::{FloatBuffer, ImageError};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MapArrayOperation {
    Add,
    Subtract,
    Multiply,
    InverseMultiply,
    Difference,
    Greatest,
    Least,
}
fn required(value: Option<&FloatBuffer>) -> Result<&FloatBuffer, ImageError> {
    value.ok_or(ImageError::NullReference)
}
fn allocate(length: usize, preferred: Option<&FloatBuffer>) -> Result<FloatBuffer, ImageError> {
    if let Some(value) = preferred.filter(|v| v.len() == length) {
        return Ok(value.clone());
    }
    let mut values = Vec::new();
    values
        .try_reserve_exact(length)
        .map_err(|_| ImageError::Allocation)?;
    values.resize(length, 0.0);
    Ok(FloatBuffer::new(values))
}
fn zero() -> FloatBuffer {
    FloatBuffer::new(vec![0.0])
}
fn collapse(value: FloatBuffer) -> Result<FloatBuffer, ImageError> {
    // r7.a(ref float[]) reads element zero even for an empty array. A one-cell
    // or uniform result becomes a fresh root, preserving the first sample bits.
    let first = value.read(0)?;
    for i in 1..value.len() {
        if value.read(i)? != first {
            return Ok(value);
        }
    }
    Ok(FloatBuffer::new(vec![first]))
}
fn worker(
    op: Op,
    out: &FloatBuffer,
    a: &FloatBuffer,
    b: Option<&FloatBuffer>,
    scalar: f32,
) -> Result<(), ImageError> {
    ArithmeticWorker::new(op, Some(out.clone()), Some(a.clone()), b.cloned(), scalar).run()
}

/// Original r7 two-array operations. A shortcut may return a retained null or
/// input array; it is not replaced by a generic broadcast or copied snapshot.
pub fn combine_map_arrays(
    operation: MapArrayOperation,
    first: Option<&FloatBuffer>,
    second: Option<&FloatBuffer>,
) -> Result<Option<FloatBuffer>, ImageError> {
    use MapArrayOperation as M;
    // The subtraction/difference bodies inspect the SECOND operand before the
    // first. Add/multiply/inverse inspect the first before any second access.
    let (a, b) = match operation {
        M::Subtract | M::Difference => {
            let b = required(second)?;
            if b.len() == 1 && b.read(0)? == 0.0 {
                return Ok(first.cloned());
            }
            (required(first)?, b)
        }
        M::Add => {
            let a = required(first)?;
            if a.len() == 1 && a.read(0)? == 0.0 {
                return Ok(second.cloned());
            }
            let b = required(second)?;
            if b.len() == 1 && b.read(0)? == 0.0 {
                return Ok(Some(a.clone()));
            }
            (a, b)
        }
        M::Multiply => {
            let a = required(first)?;
            if a.len() == 1 {
                if a.read(0)? == 0.0 {
                    return Ok(Some(zero()));
                }
                if a.read(0)? == 1.0 {
                    return Ok(second.cloned());
                }
            } else {
                let b = required(second)?;
                if b.len() == 1 {
                    if b.read(0)? == 0.0 {
                        return Ok(Some(zero()));
                    }
                    if b.read(0)? == 1.0 {
                        return Ok(Some(a.clone()));
                    }
                }
            }
            (a, required(second)?)
        }
        M::InverseMultiply => {
            let a = required(first)?;
            if a.len() == 1 {
                if a.read(0)? == 0.0 {
                    return Ok(second.cloned());
                }
                if a.read(0)? == 1.0 {
                    return Ok(Some(zero()));
                }
            } else {
                let b = required(second)?;
                if b.len() == 1 && b.read(0)? == 0.0 {
                    return Ok(Some(zero()));
                }
            }
            (a, required(second)?)
        }
        M::Greatest | M::Least => (required(first)?, required(second)?),
    };
    let out = allocate(a.len().max(b.len()), None)?;
    match (a.len(), b.len()) {
        (1, 1) => {
            let x = a.read(0)?;
            let y = b.read(0)?;
            out.write(
                0,
                match operation {
                    M::Add => add(x, y),
                    M::Subtract => sub(x, y),
                    M::Multiply => mul(x, y),
                    M::InverseMultiply => mul(sub(1.0, x), y),
                    M::Difference => {
                        if x > y {
                            sub(x, y)
                        } else {
                            sub(y, x)
                        }
                    }
                    M::Greatest => {
                        if x > y {
                            x
                        } else {
                            y
                        }
                    }
                    M::Least => {
                        if x < y {
                            x
                        } else {
                            y
                        }
                    }
                },
            )?;
        }
        (1, n) if n > 1 => {
            let x = a.read(0)?;
            match operation {
                M::Add => worker(Op::AddScalar, &out, b, None, x)?,
                M::Multiply => worker(Op::MultiplyScalar, &out, b, None, x)?,
                M::InverseMultiply => worker(Op::MultiplyScalar, &out, b, None, sub(1.0, x))?,
                _ => {
                    for i in 0..b.len() {
                        let y = b.read(i)?;
                        out.write(
                            i,
                            match operation {
                                M::Subtract => sub(x, y),
                                M::Difference => sub(y, x).abs(),
                                M::Greatest => {
                                    if x > y {
                                        x
                                    } else {
                                        y
                                    }
                                }
                                M::Least => {
                                    if x < y {
                                        x
                                    } else {
                                        y
                                    }
                                }
                                _ => unreachable!("worker branch handled above"),
                            },
                        )?;
                    }
                }
            }
        }
        (n, 1) if n > 1 => {
            let y = b.read(0)?;
            match operation {
                M::Add => worker(Op::AddScalar, &out, a, None, y)?,
                M::Subtract => worker(Op::AddScalar, &out, a, None, sub(0.0, y))?,
                M::Multiply => worker(Op::MultiplyScalar, &out, a, None, y)?,
                _ => {
                    for i in 0..a.len() {
                        let x = a.read(i)?;
                        out.write(
                            i,
                            match operation {
                                M::InverseMultiply => mul(sub(1.0, x), y),
                                M::Difference => sub(x, y).abs(),
                                M::Greatest => {
                                    if y > x {
                                        y
                                    } else {
                                        x
                                    }
                                }
                                M::Least => {
                                    if y < x {
                                        y
                                    } else {
                                        x
                                    }
                                }
                                _ => unreachable!("worker branch handled above"),
                            },
                        )?;
                    }
                }
            }
        }
        (n, m) if n > 1 && m > 1 => match operation {
            M::Add => worker(Op::AddArrays, &out, a, Some(b), 0.0)?,
            M::Multiply => worker(Op::MultiplyArrays, &out, a, Some(b), 0.0)?,
            _ => {
                for i in 0..a.len() {
                    let x = a.read(i)?;
                    let y = b.read(i)?;
                    out.write(
                        i,
                        match operation {
                            M::Subtract => sub(x, y),
                            M::InverseMultiply => mul(sub(1.0, x), y),
                            M::Difference => sub(x, y).abs(),
                            M::Greatest => {
                                if x > y {
                                    x
                                } else {
                                    y
                                }
                            }
                            M::Least => {
                                if x < y {
                                    x
                                } else {
                                    y
                                }
                            }
                            _ => unreachable!("worker branch handled above"),
                        },
                    )?;
                }
            }
        },
        _ => (), // Empty-array branches execute none of the source workers.
    }
    if matches!(operation, M::Greatest | M::Least) {
        Ok(Some(collapse(out)?))
    } else {
        Ok(Some(out))
    }
}

/// Source r7 opacity helper, including a caller-supplied reusable destination.
/// The destination can alias an operand and keeps partial writes on failure.
pub fn interpolate_map_arrays(
    first: Option<&FloatBuffer>,
    second: Option<&FloatBuffer>,
    opacity: f32,
    preferred: Option<&FloatBuffer>,
) -> Result<FloatBuffer, ImageError> {
    let a = required(first)?;
    let b = required(second)?;
    let out = allocate(a.len().max(b.len()), preferred)?;
    let inverse = sub(1.0, opacity);
    match (a.len(), b.len()) {
        (1, 1) => out.write(0, add(mul(a.read(0)?, opacity), mul(b.read(0)?, inverse)))?,
        (1, n) if n > 1 => {
            let constant = a.read(0)?;
            for i in 0..b.len() {
                out.write(i, add(mul(constant, opacity), mul(b.read(i)?, inverse)))?;
            }
        }
        (n, 1) if n > 1 => {
            let constant = b.read(0)?;
            // r7+n evaluates the scalar SECOND term first in this branch.
            for i in 0..a.len() {
                out.write(i, add(mul(constant, inverse), mul(a.read(i)?, opacity)))?;
            }
        }
        (n, m) if n > 1 && m > 1 => {
            for i in 0..a.len() {
                out.write(i, add(mul(a.read(i)?, opacity), mul(b.read(i)?, inverse)))?;
            }
        }
        _ => (),
    }
    collapse(out)
}
