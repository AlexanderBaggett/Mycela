//! Positive, normal-range power for the legacy coordinate hash. The original
//! CLR's AVX path uses fused multiply-add with split logarithm and exponential
//! tables. Keeping this arithmetic portable keeps the hash's fractional result
//! independent of the platform libm for this range.
#[path = "fractional_power_tables.rs"]
mod table;

const LN2_HI: f64 = f64::from_bits(0x3fe62e42e0000000);
const LN2_LO: f64 = f64::from_bits(0x3e6efa39ef35793c);
const EXP_STEPS_PER_NAT: f64 = f64::from_bits(0x40571547652b82fe);
const EXP_STEP_HI: f64 = f64::from_bits(0x3f862e42f0000000);
const NEG_EXP_STEP_LO: f64 = f64::from_bits(0xbdfdf473de6af278);

const fn reciprocals() -> [[f64; 2]; 257] {
    let mut table = [[0.0; 2]; 257];
    let mut i = 0;
    while i < table.len() {
        let divisor = 256 + i;
        let scaled = 131072 / divisor;
        let remainder = 131072 % divisor;
        table[i] = [
            scaled as f64 / 256.0,
            remainder as f64 / (256 * divisor) as f64,
        ];
        i += 1;
    }
    table
}
const RECIPROCAL: [[f64; 2]; 257] = reciprocals();

fn upper_significand(value: f64) -> f64 {
    f64::from_bits(value.to_bits() & !0x07ffffff)
}

/// The ordinary positive-base path is sufficient for hash inputs near texture
/// coordinates. Exceptional inputs and exponents reaching underflow or overflow
/// use the host primitive.
pub(super) fn pow(base: f64, power: f64) -> f64 {
    if !base.is_normal() || base < 0.0 || !power.is_finite() {
        return base.powf(power);
    }

    // Normalize to [0.5, 1) and round the mantissa to the nearest 1/256.
    let raw = base.to_bits();
    let exponent = ((raw >> 52) as i32 - 1023) as f64;
    let mantissa = raw & 0x000f_ffff_ffff_ffff;
    let center = (mantissa + 0x0000_0800_0000_0000) & !0x0000_0fff_ffff_ffff;
    let index = (center >> 44) as usize;
    let normalize = 0x3fe0_0000_0000_0000;
    let delta = f64::from_bits(center | normalize) - f64::from_bits(mantissa | normalize);
    let [inv_hi, inv_lo] = RECIPROCAL[index];
    let d_hi = delta * inv_hi;
    let d_lo = delta * inv_lo;
    let d = d_hi + d_lo;
    let d_error = (d_hi - d) + d_lo;
    // Logarithm reduction around the nearest center. Track the rounding error
    // of the difference and the sums before splitting the logarithm.
    let polynomial = f64::from_bits(0x3fc2492492492494)
        .mul_add(d, 1. / 6.)
        .mul_add(d, 1. / 5.)
        .mul_add(d, 1. / 4.)
        .mul_add(d, 1. / 3.)
        .mul_add(d, 1. / 2.);
    let correction = polynomial.mul_add(d * d, d_error);
    let [center_hi, center_lo] = table::LOG_CENTER[index].map(f64::from_bits);
    let small = exponent.mul_add(LN2_LO, -correction) + center_lo;
    let low = small - d;
    let log_large = exponent.mul_add(LN2_HI, center_hi);
    let log = log_large + low;
    let log_hi = upper_significand(log);
    let log_lo = (((log_large - log) + low) + (small - (d + low))) + (log - log_hi);

    let power_hi = upper_significand(power);
    let power_lo = power - power_hi;
    let product = log_hi * power_hi;
    let correction = (power_lo * log_lo + power_lo * log_hi) + log_lo * power_hi;
    let product_hi = product + correction;
    let product_lo = (product - product_hi) + correction;

    // Reduce exp(product) to a table factor 2^(index/64), a small
    // polynomial remainder, and an exact power-of-two scale.
    let scaled = product_hi * EXP_STEPS_PER_NAT;
    if !(-65_536.0..=65_536.0).contains(&scaled) {
        return base.powf(power);
    }
    let steps = scaled.round_ties_even() as i32;
    let index = (steps & 63) as usize;
    let exponent = steps >> 6;
    if !(-1021..=1023).contains(&exponent) {
        return base.powf(power);
    }
    let steps = f64::from(steps);
    let r = ((-steps).mul_add(EXP_STEP_HI, product_hi) + steps * NEG_EXP_STEP_LO) + product_lo;
    let correction = (1. / 720.0_f64)
        .mul_add(r, 1. / 120.)
        .mul_add(r, 1. / 24.)
        .mul_add(r, 1. / 6.)
        .mul_add(r, 1. / 2.)
        .mul_add(r, 1.)
        * r;
    let [exp_hi, exp_lo] = table::EXP_FRACTION[index].map(f64::from_bits);
    let approximate = (correction * exp_hi + (correction * exp_lo + exp_lo)) + exp_hi;
    approximate * f64::from_bits(((exponent + 1023) as u64) << 52)
}
