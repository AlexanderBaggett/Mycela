//! Source-specific preferred-root r7 kernels used by amm. Neither the preferred
//! array nor a returned input is copied. New amm worker IEEE priority is pending
//! original capture; expressions keep the actual CIL operand order.
use super::numeric::{add, div, mul, scalar_add, scalar_mul, sub};
use crate::{FloatBuffer, ImageError};
pub(super) type Array = Option<FloatBuffer>;
#[derive(Clone, Copy)]
pub(super) enum Pair {
    Add,
    Multiply,
    Inverse,
    ComplementAdd,
    Divide,
}
pub(super) fn required(v: Option<&FloatBuffer>) -> Result<&FloatBuffer, ImageError> {
    v.ok_or(ImageError::NullReference)
}
pub(super) fn allocate(
    n: usize,
    preferred: Option<&FloatBuffer>,
) -> Result<FloatBuffer, ImageError> {
    if let Some(p) = preferred.filter(|p| p.len() == n) {
        return Ok(p.clone());
    }
    let mut v = Vec::new();
    v.try_reserve_exact(n).map_err(|_| ImageError::Allocation)?;
    v.resize(n, 0.0);
    Ok(FloatBuffer::new(v))
}
pub(super) fn reusable(
    value: Option<&FloatBuffer>,
    a: Option<&FloatBuffer>,
    b: Option<&FloatBuffer>,
) -> Array {
    value
        .filter(|v| {
            !a.is_some_and(|a| v.shares_storage(a)) && !b.is_some_and(|b| v.shares_storage(b))
        })
        .cloned()
}
fn signed_limit(x: f32) -> f32 {
    if x > 0.0 {
        100.0
    } else if x < 0.0 {
        -100.0
    } else {
        0.0
    }
}
pub(super) fn pair(
    op: Pair,
    first: Option<&FloatBuffer>,
    second: Option<&FloatBuffer>,
    preferred: Option<&FloatBuffer>,
) -> Result<Array, ImageError> {
    use Pair::*;
    let a = required(first)?;
    let na = a.len();
    match op {
        Add if na == 1 && a.read(0)? == 0.0 => return Ok(second.cloned()),
        Multiply if na == 1 => {
            if a.read(0)? == 0.0 {
                return Ok(Some(FloatBuffer::new(vec![0.0])));
            }
            if a.read(0)? == 1.0 {
                return Ok(second.cloned());
            }
        }
        Inverse if na == 1 => {
            if a.read(0)? == 0.0 {
                return Ok(second.cloned());
            }
            if a.read(0)? == 1.0 {
                return Ok(Some(FloatBuffer::new(vec![0.0])));
            }
        }
        ComplementAdd if na == 1 && a.read(0)? == 1.0 => return Ok(second.cloned()),
        Divide if na == 1 && a.read(0)? == 0.0 => return Ok(Some(a.clone())),
        _ => {}
    }
    let b = required(second)?;
    let nb = b.len();
    match op {
        Add if nb == 1 && b.read(0)? == 0.0 => return Ok(Some(a.clone())),
        Multiply if na != 1 && nb == 1 => {
            if b.read(0)? == 0.0 {
                return Ok(Some(FloatBuffer::new(vec![0.0])));
            }
            if b.read(0)? == 1.0 {
                return Ok(Some(a.clone()));
            }
        }
        Inverse if na != 1 && nb == 1 && b.read(0)? == 0.0 => {
            return Ok(Some(FloatBuffer::new(vec![0.0])));
        }
        Divide if nb == 1 && b.read(0)? == 1.0 => return Ok(Some(a.clone())),
        _ => {}
    }
    let out = allocate(na.max(nb), preferred)?;
    if na > 0
        && nb > 0
        && na.max(nb) > 1
        && (na == 1 || nb == 1 || na == nb)
        && !out.shares_storage(a)
        && !out.shares_storage(b)
    {
        let a = a.shared_values();
        let b = b.shared_values();
        out.with_mut_slice(|pixels| match (na, nb) {
            (1, _) => {
                let x = a[0];
                for (out, &y) in pixels.iter_mut().zip(b.iter()) {
                    *out = scalar_first(op, x, y);
                }
            }
            (_, 1) => {
                let y = b[0];
                let reciprocal = div(1.0, y);
                for (out, &x) in pixels.iter_mut().zip(a.iter()) {
                    *out = scalar_second(op, x, y, reciprocal);
                }
            }
            _ => {
                for ((out, &x), &y) in pixels.iter_mut().zip(a.iter()).zip(b.iter()) {
                    *out = dense_pair(op, x, y);
                }
            }
        });
        return Ok(Some(out));
    }
    if na == 1 && nb == 1 {
        let x = a.read(0)?;
        let y = b.read(0)?;
        let value = match op {
            Add => add(x, y),
            Multiply => mul(x, y),
            Inverse => mul(sub(1.0, x), y),
            ComplementAdd => add(sub(1.0, x), y),
            Divide => {
                if y != 0.0 {
                    div(x, y)
                } else if x > 0.0 {
                    100.0
                } else if x < 0.0 {
                    -100.0
                } else {
                    return Ok(Some(out));
                }
            }
        };
        out.write(0, value)?;
    } else if na == 1 && nb > 1 {
        let x = a.read(0)?;
        let n = if matches!(op, Divide) { out.len() } else { nb };
        for i in 0..n {
            let y = b.read(i)?;
            out.write(i, scalar_first(op, x, y))?;
        }
    } else if na > 1 && nb == 1 {
        let y = b.read(0)?;
        let reciprocal = div(1.0, y);
        for i in 0..na {
            let x = a.read(i)?;
            out.write(i, scalar_second(op, x, y, reciprocal))?;
        }
    } else if na > 1 && nb > 1 {
        for i in 0..na {
            // bw reads denominator before numerator; other workers read first.
            let v = if matches!(op, Divide) {
                let y = b.read(i)?;
                dense_pair(op, a.read(i)?, y)
            } else {
                let x = a.read(i)?;
                dense_pair(op, x, b.read(i)?)
            };
            out.write(i, v)?;
        }
    }
    Ok(Some(out))
}
fn scalar_first(op: Pair, x: f32, y: f32) -> f32 {
    match op {
        Pair::Add => scalar_add(y, x),
        Pair::Multiply => scalar_mul(y, x),
        Pair::Inverse => scalar_mul(y, sub(1.0, x)),
        Pair::ComplementAdd => scalar_add(y, sub(1.0, x)),
        Pair::Divide => dense_pair(op, x, y),
    }
}

fn scalar_second(op: Pair, x: f32, y: f32, reciprocal: f32) -> f32 {
    match op {
        Pair::Add => scalar_add(x, y),
        Pair::Multiply => scalar_mul(x, y),
        Pair::Inverse => mul(sub(1.0, x), y),
        Pair::ComplementAdd => add(sub(1.0, x), y),
        Pair::Divide => {
            if y > 0.0 || y < -0.0 {
                mul(x, reciprocal)
            } else {
                signed_limit(x)
            }
        }
    }
}

fn dense_pair(op: Pair, x: f32, y: f32) -> f32 {
    match op {
        Pair::Add => add(x, y),
        Pair::Multiply => mul(x, y),
        Pair::Inverse => mul(sub(1.0, x), y),
        Pair::ComplementAdd => add(sub(1.0, x), y),
        Pair::Divide => {
            if y > 0.0 || y < -0.0 {
                div(x, y)
            } else {
                signed_limit(x)
            }
        }
    }
}
/// amm.h, distinct from r7's algebraically equivalent screen expression.
pub(super) fn screen(
    first: Option<&FloatBuffer>,
    second: Option<&FloatBuffer>,
) -> Result<Array, ImageError> {
    let a = required(first)?;
    if a.len() == 1 && a.read(0)? == 0.0 {
        return Ok(second.cloned());
    }
    let b = required(second)?;
    if b.len() == 1 && b.read(0)? == 0.0 {
        return Ok(Some(a.clone()));
    }
    let out = allocate(a.len().max(b.len()), None)?;
    let n = match (a.len(), b.len()) {
        (1, 1) => 1,
        (1, n) if n > 1 => n,
        (n, 1) if n > 1 => n,
        (n, m) if n > 1 && m > 1 => n,
        _ => 0,
    };
    for i in 0..n {
        let x = a.read(if a.len() == 1 { 0 } else { i })?;
        let y = b.read(if b.len() == 1 { 0 } else { i })?;
        out.write(i, add(x, mul(y, sub(1.0, x))))?;
    }
    Ok(Some(out))
}
