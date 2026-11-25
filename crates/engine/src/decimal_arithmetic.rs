use crate::DecimalBits;
use std::{error::Error, fmt};

// F116: first 29 binary64 values loaded from the pinned runtime's linked
// s_doublePowers10 blob. Keep table bits, not recomputed powi results.
const POWERS10_BITS: [u64; 29] = [
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
    0x4341c37937e08000,
    0x4376345785d8a000,
    0x43abc16d674ec800,
    0x43e158e460913d00,
    0x4415af1d78b58c40,
    0x444b1ae4d6e2ef50,
    0x4480f0cf064dd592,
    0x44b52d02c7e14af6,
    0x44ea784379d99db4,
    0x45208b2a2c280291,
    0x4554adf4b7320335,
    0x4589d971e4fe8402,
    0x45c027e72f1f1281,
];

impl DecimalBits {
    /// Pinned Mono Decimal.ToDouble operation order, retaining signed zero.
    /// Separate casts, multiply, add and divide are observable binary64 steps;
    /// a whole-coefficient cast, fused operation or normalized scale differs.
    pub fn to_f64(self) -> f64 {
        let [low, middle, high, flags] = self.bits();
        let low64 = u64::from(low) | (u64::from(middle) << 32);
        let high_part = f64::from(high) * f64::from_bits(0x43f0000000000000);
        let combined = low64 as f64 + high_part;
        // DecimalBits construction proves this index is in 0..=28.
        let scale = ((flags >> 16) & 0xff) as usize;
        let value = combined / f64::from_bits(POWERS10_BITS[scale]);
        if flags & 0x8000_0000 != 0 {
            // Primary CIL uses neg, not subtraction from positive zero.
            -value
        } else {
            value
        }
    }

    /// Decimal-to-Double arithmetic of the captured Framework64/OLEAUT32 image.
    ///
    /// The source divides the low and high contributions separately, then adds
    /// them. Keep these binary64 steps distinct: changing their order, using a
    /// power table or casting the whole coefficient changes rounding. The
    /// addition of positive high-part zero also determines the sign of zero.
    /// This profile is independently verified under nearest-even rounding;
    /// it does not replace the separately verified Mono conversion above.
    pub fn to_framework_f64(self) -> f64 {
        let [low, middle, high, flags] = self.bits();
        let scale = (flags >> 16) & 0xff;
        let mut denominator = 1.0;
        for _ in 0..scale {
            denominator *= 10.0;
        }
        if flags & 0x8000_0000 != 0 {
            denominator = -denominator;
        }

        let mut upper = 0.0;
        if high != 0 {
            upper = f64::from(high) / denominator;
            upper *= 4_294_967_296.0;
            upper *= 4_294_967_296.0;
        }
        let low64 = u64::from(low) | (u64::from(middle) << 32);
        let lower = if low64 & (1_u64 << 63) != 0 {
            // The native unsigned conversion preserves the sticky low bit
            // before its signed-range conversion and subsequent doubling.
            let half = ((low64 >> 1) | (low64 & 1)) as f64;
            half + half
        } else {
            low64 as f64
        };
        let lower = lower / denominator;
        lower + upper
    }

    /// Captured Framework64 Decimal-to-Single, including its final narrowing.
    /// Both native exports use the same ordered binary64 arithmetic; only the
    /// Single export narrows that result before returning it.
    pub fn to_framework_f32(self) -> f32 {
        self.to_framework_f64() as f32
    }

    /// Exact literal 10m FIRST, with this value as the RIGHT operand, for the
    /// pinned Framework64/OLEAUT32 implementation. This narrow operation keeps
    /// the right sign/scale, including signed/scaled and wrap-created zero.
    /// It must not implement the opposite operand order or general Multiply.
    /// The caller selects the source compatibility profile explicitly.
    pub(crate) fn framework_ten_times(self) -> Self {
        const MAX96: u128 = (1_u128 << 96) - 1;
        let [low, middle, high, _] = self.bits();
        let coefficient = u128::from(low) | (u128::from(middle) << 32) | (u128::from(high) << 64);
        // The full product fits in 100 bits. Discard precisely the carry above
        // bit95, as the pinned ten-FIRST native path does; never reduce scale.
        let product = (10 * coefficient) & MAX96;
        self.with_coefficient([
            product as u32,
            (product >> 32) as u32,
            (product >> 64) as u32,
        ])
    }

    /// Mono-qualified multiplication by positive, scale-zero 10m, preserving
    /// storage representation and overflow. Native profiles are separate. This
    /// is not general decimal multiplication.
    pub fn multiply_by_ten(self) -> Result<Self, DecimalArithmeticError> {
        const MAX96: u128 = (1_u128 << 96) - 1;
        let [low, middle, high, _] = self.bits();
        let coefficient = u128::from(low) | (u128::from(middle) << 32) | (u128::from(high) << 64);
        // A 96-bit coefficient times ten fits within 100 bits, hence u128.
        let product = coefficient * 10;
        if product <= MAX96 {
            // Any three u32 limbs form a valid coefficient. The internal helper
            // preserves this already-valid sign/scale, including signed zero.
            Ok(self.with_coefficient([
                product as u32,
                (product >> 32) as u32,
                (product >> 64) as u32,
            ]))
        } else {
            // Wide product / 10 is exact, with zero remainder, and equals the
            // original valid coefficient. Only one scale reduction is needed;
            // reducing scale zero is precisely the source overflow branch.
            self.reduce_scale_once()
                .ok_or(DecimalArithmeticError::Overflow)
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecimalArithmeticError {
    Overflow,
}

impl DecimalArithmeticError {
    pub const fn source_class(self) -> &'static str {
        match self {
            Self::Overflow => "OverflowException",
        }
    }
}

impl fmt::Display for DecimalArithmeticError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.source_class())
    }
}

impl Error for DecimalArithmeticError {}
