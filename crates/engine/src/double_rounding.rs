use std::{error::Error, fmt};

// F118: exact loaded roundPower10Double bits from the pinned runtime. Reuse the
// same stored power for multiplication and division; do not recompute powers.
const POWERS10_BITS: [u64; 16] = [
    0x3ff0000000000000,
    0x4024000000000000,
    0x4059000000000000,
    0x408f400000000000,
    0x40c3880000000000,
    0x40f86a0000000000,
    0x412e848000000000,
    0x416312d000000000,
    0x4197d78400000000,
    0x41cdcd6500000000,
    0x4202a05f20000000,
    0x42374876e8000000,
    0x426d1a94a2000000,
    0x42a2309ce5400000,
    0x42d6bcc41e900000,
    0x430c6bf526340000,
];
const ROUND_LIMIT: f64 = f64::from_bits(0x4341c37937e08000);

/// Pinned default Math.Round(double, int) behavior with nearest-even rounding.
/// Invalid precision fails before inspecting floating magnitude. Large values,
/// infinities and unordered NaNs bypass rounding with their original bits.
/// Native Round/Abs bodies are unavailable; intrinsic compatibility is scoped
/// to the characterized finite corpus and initial floating-point environment.
pub fn round_decimal_places(value: f64, digits: i32) -> Result<f64, DecimalRoundingError> {
    if !(0..=15).contains(&digits) {
        return Err(DecimalRoundingError::Precision);
    }
    Ok(round_known_precision(value, digits as usize))
}

// Only the validated public entry point and the fixed index-three helper call
// this core, so the table index is in bounds without another fallible path.
fn round_known_precision(value: f64, digits: usize) -> f64 {
    if value.abs() < ROUND_LIMIT {
        let power = f64::from_bits(POWERS10_BITS[digits]);
        let product = value * power;
        let rounded = product.round_ties_even();
        rounded / power
    } else {
        // Return the input, including signaling NaN payloads;
        // do not return the absolute value or normalize a nonfinite bypass.
        value
    }
}

pub(crate) fn round_three_decimal_places(value: f64) -> f64 {
    round_known_precision(value, 3)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecimalRoundingError {
    Precision,
}

impl DecimalRoundingError {
    pub const fn source_class(self) -> &'static str {
        match self {
            Self::Precision => "ArgumentOutOfRangeException",
        }
    }
}

impl fmt::Display for DecimalRoundingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.source_class())
    }
}

impl Error for DecimalRoundingError {}
