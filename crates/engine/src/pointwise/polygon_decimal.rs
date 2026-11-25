//! 96-bit decimal intermediates for source Polygon scanline geometry.
//! BigInt is used only for exact intermediate arithmetic, then rounded to the
//! CLR decimal coefficient/scale domain. The shared Framework Double conversion
//! profile is reused; operation rounding still needs original-runtime verification.
use crate::{DecimalBits, ImageError, PointwiseError};
use num_bigint::{BigInt, Sign};
use std::cmp::Ordering;
#[derive(Clone, Debug)]
pub(super) struct D {
    coefficient: BigInt,
    scale: u32,
}
fn ten(n: u32) -> BigInt {
    BigInt::from(10u8).pow(n)
}
fn max() -> BigInt {
    (BigInt::from(1u8) << 96usize) - BigInt::from(1u8)
}
fn abs(v: &BigInt) -> BigInt {
    if v.sign() == Sign::Minus {
        -v
    } else {
        v.clone()
    }
}
fn rounded(n: &BigInt, denominator: &BigInt) -> BigInt {
    let magnitude = abs(n);
    let mut q = &magnitude / denominator;
    let r = magnitude % denominator;
    let twice = &r * 2u8;
    if twice > *denominator || (twice == *denominator && (&q % 2u8) != BigInt::from(0u8)) {
        q += 1u8;
    }
    if n.sign() == Sign::Minus { -q } else { q }
}
impl D {
    pub(super) fn integer(n: i32) -> Self {
        Self {
            coefficient: BigInt::from(n),
            scale: 0,
        }
    }
    fn fit<E>(coefficient: BigInt, scale: u32) -> Result<Self, PointwiseError<E>> {
        let mut drop = scale.saturating_sub(28);
        loop {
            if drop > scale {
                return Err(PointwiseError::Image(ImageError::Overflow));
            }
            let next = if drop == 0 {
                coefficient.clone()
            } else {
                rounded(&coefficient, &ten(drop))
            };
            if abs(&next) <= max() {
                return Ok(Self {
                    coefficient: next,
                    scale: scale - drop,
                });
            }
            drop += 1;
        }
    }
    pub(super) fn from_f64<E>(value: f64) -> Result<Self, PointwiseError<E>> {
        let decimal =
            super::property::wire::framework_decimal_from_f64::<()>(value).map_err(|error| {
                match error {
                    super::property::wire::LivePropertyWireError::DecimalOverflow => {
                        PointwiseError::Image(ImageError::Overflow)
                    }
                    _ => PointwiseError::UnsupportedCapability("Framework decimal conversion"),
                }
            })?;
        let bits = decimal.bits();
        let magnitude = BigInt::from(bits[0])
            + (BigInt::from(bits[1]) << 32usize)
            + (BigInt::from(bits[2]) << 64usize);
        let coefficient = if bits[3] & 0x8000_0000 != 0 {
            -magnitude
        } else {
            magnitude
        };
        Ok(Self {
            coefficient,
            scale: (bits[3] >> 16) & 0xff,
        })
    }

    pub(super) fn add<E>(&self, other: &Self) -> Result<Self, PointwiseError<E>> {
        let scale = self.scale.max(other.scale);
        Self::fit(
            &self.coefficient * ten(scale - self.scale)
                + &other.coefficient * ten(scale - other.scale),
            scale,
        )
    }
    pub(super) fn sub<E>(&self, other: &Self) -> Result<Self, PointwiseError<E>> {
        let scale = self.scale.max(other.scale);
        Self::fit(
            &self.coefficient * ten(scale - self.scale)
                - &other.coefficient * ten(scale - other.scale),
            scale,
        )
    }
    pub(super) fn mul<E>(&self, other: &Self) -> Result<Self, PointwiseError<E>> {
        Self::fit(
            &self.coefficient * &other.coefficient,
            self.scale + other.scale,
        )
    }
    pub(super) fn div<E>(&self, other: &Self) -> Result<Self, PointwiseError<E>> {
        if other.coefficient == BigInt::from(0u8) {
            return Err(PointwiseError::DivideByZero);
        }
        let negative = self.coefficient.sign() != other.coefficient.sign();
        let numerator = abs(&self.coefficient) * ten(other.scale);
        let denominator = abs(&other.coefficient) * ten(self.scale);
        let mut scale = self.scale.saturating_sub(other.scale);
        loop {
            let scaled = &numerator * ten(scale);
            let q = &scaled / &denominator;
            let rem = &scaled % &denominator;
            if q > max() {
                if scale == 0 {
                    return Err(PointwiseError::Image(ImageError::Overflow));
                }
                loop {
                    scale -= 1;
                    let result = rounded(&(&numerator * ten(scale)), &denominator);
                    if result <= max() {
                        return Ok(Self {
                            coefficient: if negative { -result } else { result },
                            scale,
                        });
                    }
                    if scale == 0 {
                        return Err(PointwiseError::Image(ImageError::Overflow));
                    }
                }
            }
            if rem == BigInt::from(0u8) {
                return Ok(Self {
                    coefficient: if negative { -q } else { q },
                    scale,
                });
            }
            if scale == 28 || (&q * 10u8 + (&rem * 10u8 / &denominator)) > max() {
                let mut result = rounded(&scaled, &denominator);
                if result > max() {
                    if scale == 0 {
                        return Err(PointwiseError::Image(ImageError::Overflow));
                    }
                    scale -= 1;
                    result = rounded(&(&numerator * ten(scale)), &denominator);
                }
                return Ok(Self {
                    coefficient: if negative { -result } else { result },
                    scale,
                });
            }
            scale += 1;
        }
    }
    pub(super) fn rem_one<E>(&self) -> Result<Self, PointwiseError<E>> {
        Self::fit(&self.coefficient % ten(self.scale), self.scale)
    }
    pub(super) fn cmp(&self, other: &Self) -> Ordering {
        let scale = self.scale.max(other.scale);
        (&self.coefficient * ten(scale - self.scale))
            .cmp(&(&other.coefficient * ten(scale - other.scale)))
    }
    pub(super) fn to_f64(&self) -> f64 {
        let (_, mut bytes) = self.coefficient.to_bytes_le();
        bytes.resize(12, 0);
        let limb = |i| u32::from_le_bytes(bytes[i..i + 4].try_into().expect("decimal limb"));
        DecimalBits::new([
            limb(0),
            limb(4),
            limb(8),
            (self.scale << 16)
                | if self.coefficient.sign() == Sign::Minus {
                    0x8000_0000
                } else {
                    0
                },
        ])
        .expect("fitted decimal")
        .to_framework_f64()
    }
}
