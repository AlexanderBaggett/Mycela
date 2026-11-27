//! Source-specific preferred-root r7 kernels used by amm. Neither the preferred
//! array nor a returned input is copied. New amm worker IEEE priority is pending
//! original capture; expressions keep the actual CIL operand order.
use super::numeric::{add, div, mul, scalar_add, scalar_mul, sub};
use crate::{
    FloatBuffer, ImageError, ProceduralExecution, procedural_schedule::for_each_dense_band,
};
use std::sync::Arc;
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
enum Operand {
    Scalar(f32),
    Raster(Arc<Vec<f32>>),
    Destination,
}

impl Operand {
    fn new(source: &FloatBuffer, destination: &FloatBuffer) -> Result<Self, ImageError> {
        if source.len() == 1 {
            Ok(Self::Scalar(source.read(0)?))
        } else if source.shares_storage(destination) {
            Ok(Self::Destination)
        } else {
            Ok(Self::Raster(source.shared_values()))
        }
    }

    fn at(&self, index: usize, old: f32) -> f32 {
        match self {
            Self::Scalar(value) => *value,
            Self::Raster(values) => values[index],
            Self::Destination => old,
        }
    }
}

pub(super) fn compatible(length: usize, sizes: &[usize]) -> bool {
    length > 1 && sizes.iter().all(|&n| n == 1 || n == length)
}

fn blend_dense(
    out: &FloatBuffer,
    a: &Operand,
    b: &Operand,
    execution: ProceduralExecution,
    combine: impl Fn(f32, f32) -> f32 + Sync,
) -> Result<(), ImageError> {
    out.with_mut_slice(|pixels| {
        for_each_dense_band(pixels, 1, execution, |start, band| {
            match (a, b) {
                (Operand::Raster(a), Operand::Raster(b)) => {
                    for ((out, &x), &y) in band.iter_mut().zip(&a[start..]).zip(&b[start..]) {
                        *out = combine(x, y);
                    }
                }
                _ => {
                    for (offset, value) in band.iter_mut().enumerate() {
                        let index = start + offset;
                        *value = combine(a.at(index, *value), b.at(index, *value));
                    }
                }
            }
            Ok(())
        })
    })
}

pub(super) fn map_dense(
    out: &FloatBuffer,
    a: &FloatBuffer,
    b: &FloatBuffer,
    execution: ProceduralExecution,
    combine: impl Fn(f32, f32) -> f32 + Sync,
) -> Result<(), ImageError> {
    blend_dense(
        out,
        &Operand::new(a, out)?,
        &Operand::new(b, out)?,
        execution,
        combine,
    )
}

pub(super) fn pair(
    op: Pair,
    first: Option<&FloatBuffer>,
    second: Option<&FloatBuffer>,
    preferred: Option<&FloatBuffer>,
    execution: ProceduralExecution,
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
    if compatible(na.max(nb), &[na, nb]) {
        let a = Operand::new(a, &out)?;
        let b = Operand::new(b, &out)?;
        // Select the expression before traversing the raster. Each closure
        // retains the distinct scalar/dense operand order and NaN priority.
        match op {
            Add if na == 1 => blend_dense(&out, &a, &b, execution, |x, y| scalar_add(y, x)),
            Add if nb == 1 => blend_dense(&out, &a, &b, execution, scalar_add),
            Add => blend_dense(&out, &a, &b, execution, add),
            Multiply if na == 1 => blend_dense(&out, &a, &b, execution, |x, y| scalar_mul(y, x)),
            Multiply if nb == 1 => blend_dense(&out, &a, &b, execution, scalar_mul),
            Multiply => blend_dense(&out, &a, &b, execution, mul),
            Inverse if na == 1 => {
                blend_dense(&out, &a, &b, execution, |x, y| scalar_mul(y, sub(1., x)))
            }
            Inverse => blend_dense(&out, &a, &b, execution, |x, y| mul(sub(1., x), y)),
            ComplementAdd if na == 1 => {
                blend_dense(&out, &a, &b, execution, |x, y| scalar_add(y, sub(1., x)))
            }
            ComplementAdd => blend_dense(&out, &a, &b, execution, |x, y| add(sub(1., x), y)),
            Divide if let Operand::Scalar(y) = b => {
                if y > 0.0 || y < -0.0 {
                    let reciprocal = div(1., y);
                    blend_dense(&out, &a, &b, execution, |x, _| mul(x, reciprocal))
                } else {
                    blend_dense(&out, &a, &b, execution, |x, _| signed_limit(x))
                }
            }
            Divide => blend_dense(&out, &a, &b, execution, divide_dense),
        }?;
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

fn divide_dense(x: f32, y: f32) -> f32 {
    if y > 0.0 || y < -0.0 {
        div(x, y)
    } else {
        signed_limit(x)
    }
}
fn dense_pair(op: Pair, x: f32, y: f32) -> f32 {
    match op {
        Pair::Add => add(x, y),
        Pair::Multiply => mul(x, y),
        Pair::Inverse => mul(sub(1.0, x), y),
        Pair::ComplementAdd => add(sub(1.0, x), y),
        Pair::Divide => divide_dense(x, y),
    }
}
/// amm.h, distinct from r7's algebraically equivalent screen expression.
pub(super) fn screen(
    first: Option<&FloatBuffer>,
    second: Option<&FloatBuffer>,
    execution: ProceduralExecution,
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
    let screen = |x, y| add(x, mul(y, sub(1.0, x)));
    if compatible(out.len(), &[a.len(), b.len()]) {
        map_dense(&out, a, b, execution, screen)?;
        return Ok(Some(out));
    }
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
        out.write(i, screen(x, y))?;
    }
    Ok(Some(out))
}
