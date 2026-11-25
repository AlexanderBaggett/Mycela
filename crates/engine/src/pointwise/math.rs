use super::numeric::{add, div, mul, scalar_add, scalar_mul, sub};
use crate::{FloatBuffer, ImageError};
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArithmeticOperation {
    AddScalar,
    AddArrays,
    MultiplyScalar,
    MultiplyArrays,
}
/// Retains original nullable arrays; aliases are read and written sequentially.
#[derive(Clone, Debug)]
pub struct ArithmeticWorker {
    operation: ArithmeticOperation,
    destination: Option<FloatBuffer>,
    source: Option<FloatBuffer>,
    other: Option<FloatBuffer>,
    scalar: f32,
}
impl ArithmeticWorker {
    pub fn new(
        operation: ArithmeticOperation,
        destination: Option<FloatBuffer>,
        source: Option<FloatBuffer>,
        other: Option<FloatBuffer>,
        scalar: f32,
    ) -> Self {
        Self {
            operation,
            destination,
            source,
            other,
            scalar,
        }
    }
    pub fn len(&self) -> Result<i32, ImageError> {
        i32::try_from(self.source.as_ref().ok_or(ImageError::NullReference)?.len())
            .map_err(|_| ImageError::Overflow)
    }
    pub fn is_empty(&self) -> Result<bool, ImageError> {
        self.len().map(|v| v == 0)
    }
    pub fn run(&self) -> Result<(), ImageError> {
        self.run_range(0, self.len()?)
    }
    pub fn run_range(&self, start: i32, count: i32) -> Result<(), ImageError> {
        let end = start.wrapping_add(count);
        let mut ix = start;
        while ix < end {
            let source = self.source.as_ref().ok_or(ImageError::NullReference)?;
            let at = usize::try_from(ix).map_err(|_| ImageError::Index)?;
            let left = source.read(at)?;
            let right = match self.operation {
                ArithmeticOperation::AddArrays | ArithmeticOperation::MultiplyArrays => self
                    .other
                    .as_ref()
                    .ok_or(ImageError::NullReference)?
                    .read(at)?,
                _ => self.scalar,
            };
            let value = match self.operation {
                ArithmeticOperation::AddScalar => scalar_add(left, right),
                ArithmeticOperation::AddArrays => add(left, right),
                ArithmeticOperation::MultiplyScalar => scalar_mul(left, right),
                ArithmeticOperation::MultiplyArrays => mul(left, right),
            };
            self.destination
                .as_ref()
                .ok_or(ImageError::NullReference)?
                .write(at, value)?;
            ix = ix.wrapping_add(1);
        }
        Ok(())
    }
    pub fn operation(&self) -> ArithmeticOperation {
        self.operation
    }
    pub fn destination(&self) -> Option<FloatBuffer> {
        self.destination.clone()
    }
    pub fn source(&self) -> Option<FloatBuffer> {
        self.source.clone()
    }
    pub fn other(&self) -> Option<FloatBuffer> {
        self.other.clone()
    }
    pub fn scalar(&self) -> f32 {
        self.scalar
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HsbOperation {
    RgbToHsb,
    HsbToRgb,
}
fn put(destination: Option<&FloatBuffer>, ix: usize, value: f32) -> Result<(), ImageError> {
    destination
        .ok_or(ImageError::NullReference)?
        .write(ix, value)
}
/// Original r7.b scalar helper, with ordered partial destination writes.
pub fn hsb_to_rgb(
    destination: Option<&FloatBuffer>,
    h: f32,
    s: f32,
    b: f32,
) -> Result<(), ImageError> {
    let rgb = hsb_values(h, s, b);
    for (i, value) in rgb.into_iter().enumerate() {
        put(destination, i, value)?;
    }
    Ok(())
}
pub fn rgb_to_hsb(
    destination: Option<&FloatBuffer>,
    r: f32,
    g: f32,
    b: f32,
) -> Result<(), ImageError> {
    let values = rgb_values(r, g, b, false);
    put(destination, 2, values[2])?;
    put(destination, 1, values[1])?;
    put(destination, 0, values[0])
}
pub(super) fn hsb_values(h: f32, s: f32, b: f32) -> [f32; 3] {
    let (r, g, blue) = if h < 0.5 {
        if h < 1.0f32 / 6.0 {
            (1.0, mul(h, 6.0), 0.0)
        } else if h < 1.0f32 / 3.0 {
            (sub(2.0, mul(h, 6.0)), 1.0, 0.0)
        } else {
            (0.0, 1.0, sub(mul(h, 6.0), 2.0))
        }
    } else if h < 2.0f32 / 3.0 {
        (0.0, sub(4.0, mul(h, 6.0)), 1.0)
    } else if h < 5.0f32 / 6.0 {
        (sub(mul(h, 6.0), 4.0), 0.0, 1.0)
    } else {
        (1.0, 0.0, sub(6.0, mul(h, 6.0)))
    };
    [
        mul(add(sub(1.0, s), mul(s, r)), b),
        mul(add(sub(1.0, s), mul(s, g)), b),
        mul(add(sub(1.0, s), mul(s, blue)), b),
    ]
}
pub(super) fn rgb_values(r: f32, g: f32, b: f32, reciprocal: bool) -> [f32; 3] {
    let (low, high) = if r < g { (r, g) } else { (g, r) };
    let (min, mid, mut max) = if b < low {
        (b, low, high)
    } else if b < high {
        (low, b, high)
    } else {
        (low, high, b)
    };
    let brightness = max;
    let saturation = if min == max {
        if max == 0.0 {
            max = 1e-5;
        }
        1e-5
    } else {
        if max == 0.0 {
            max = 1e-5;
        }
        let s = sub(1.0, div(min, max));
        if s == 0.0 { 1e-5 } else { s }
    };
    let ratio = add(1.0, div(sub(mid, max), mul(saturation, max)));
    let divide = |v: f32| {
        if reciprocal {
            mul(v, 1.0f32 / 6.0)
        } else {
            div(v, 6.0)
        }
    };
    let hue = if r >= g && r > b {
        if g > b {
            divide(ratio)
        } else {
            sub(1.0, divide(ratio))
        }
    } else if g > r && g > b {
        if r > b {
            divide(sub(2.0, ratio))
        } else {
            divide(add(2.0, ratio))
        }
    } else if g > r {
        divide(sub(4.0, ratio))
    } else {
        divide(add(4.0, ratio))
    };
    [hue, saturation, brightness]
}
/// Actual out-array publication order is observable even when a later read fails.
pub fn hsb_arrays(
    operation: HsbOperation,
    outputs: &mut [Option<FloatBuffer>; 3],
    inputs: [Option<FloatBuffer>; 3],
) -> Result<(), ImageError> {
    let mut lengths = [0; 3];
    for (i, v) in inputs.iter().enumerate() {
        lengths[i] = v.as_ref().ok_or(ImageError::NullReference)?.len();
    }
    let length = lengths[0].max(lengths[1].max(lengths[2]));
    for out in outputs.iter_mut() {
        let mut values = Vec::new();
        values
            .try_reserve_exact(length)
            .map_err(|_| ImageError::Allocation)?;
        values.resize(length, 0.0);
        *out = Some(FloatBuffer::new(values));
    }
    let mut values = [0.0; 3];
    for (i, v) in inputs.iter().enumerate() {
        values[i] = v.as_ref().ok_or(ImageError::NullReference)?.read(0)?;
    }
    for ix in 0..length {
        for ch in 0..3 {
            if lengths[ch] > 1 {
                values[ch] = inputs[ch]
                    .as_ref()
                    .ok_or(ImageError::NullReference)?
                    .read(ix)?;
            }
        }
        let out = match operation {
            HsbOperation::RgbToHsb => rgb_values(values[0], values[1], values[2], true),
            HsbOperation::HsbToRgb => hsb_values(values[0], values[1], values[2]),
        };
        // r7.k writes B, then S, then H; r7.h writes R,G,B. These
        // destination arrays are fresh/nonaliased, so the final cells agree.
        for (ch, target) in outputs.iter().enumerate() {
            target
                .as_ref()
                .ok_or(ImageError::NullReference)?
                .write(ix, out[ch])?;
        }
    }
    if operation == HsbOperation::HsbToRgb {
        let mut constant = [true; 3];
        let mut first = [0.0; 3];
        for ch in 0..3 {
            first[ch] = outputs[ch]
                .as_ref()
                .ok_or(ImageError::NullReference)?
                .read(0)?;
        }
        for ix in 1..length {
            for ch in 0..3 {
                if constant[ch]
                    && outputs[ch]
                        .as_ref()
                        .ok_or(ImageError::NullReference)?
                        .read(ix)?
                        != first[ch]
                {
                    constant[ch] = false;
                }
            }
            if constant.iter().all(|v| !*v) {
                break;
            }
        }
        for ch in 0..3 {
            if constant[ch] {
                outputs[ch] = Some(FloatBuffer::new(vec![first[ch]]));
            }
        }
    }
    Ok(())
}
