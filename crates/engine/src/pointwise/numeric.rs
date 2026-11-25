//! Operation-specific binary32 behavior of the characterized Framework profile.
//! These choices belong to the named source arithmetic sites, not all CLR/JIT
//! arithmetic. Stored values and copies never pass through this module.
const QUIET: u32 = 0x0040_0000;
const INVALID: f32 = f32::from_bits(0xffc0_0000);
fn nan(value: f32) -> bool {
    value.to_bits() & 0x7fff_ffff > 0x7f80_0000
}
fn quiet(value: f32) -> f32 {
    f32::from_bits(value.to_bits() | QUIET)
}
fn left(a: f32, b: f32) -> Option<f32> {
    if nan(a) {
        Some(quiet(a))
    } else if nan(b) {
        Some(quiet(b))
    } else {
        None
    }
}
fn invalid(value: f32) -> f32 {
    if nan(value) { INVALID } else { value }
}
// pm/afi and the characterized r7 expression order retain a quieted LEFT NaN.
// Handling operands before ordinary Rust arithmetic prevents optimizer changes
// to commutative operand selection from changing observable sign/payload bits.
pub(super) fn add(a: f32, b: f32) -> f32 {
    left(a, b).unwrap_or_else(|| invalid(a + b))
}
pub(super) fn mul(a: f32, b: f32) -> f32 {
    left(a, b).unwrap_or_else(|| invalid(a * b))
}
pub(super) fn sub(a: f32, b: f32) -> f32 {
    left(a, b).unwrap_or_else(|| invalid(a - b))
}
pub(super) fn div(a: f32, b: f32) -> f32 {
    left(a, b).unwrap_or_else(|| invalid(a / b))
}
// TexTree r7.a uses strict bounds when treating a channel as an identity.
pub(super) fn approximately_one(value: f32) -> bool {
    0.999_f32 < value && value < 1.001_f32
}
// Actual ns/yn scalar methods select the RIGHT operand when both are NaN.
// Do not commute the finite arithmetic: signed zero and errors retain source order.
pub(super) fn scalar_add(a: f32, b: f32) -> f32 {
    left(b, a).unwrap_or_else(|| invalid(a + b))
}
pub(super) fn scalar_mul(a: f32, b: f32) -> f32 {
    left(b, a).unwrap_or_else(|| invalid(a * b))
}
/// CE's sole CIL REM site has divisor +1. The pinned CLR float remainder
/// candidate returns the negative invalid literal for a nonfinite dividend;
/// two original CE traces connect that branch to the observed render result.
/// Arbitrary divisors and actual generated JIT helper binding remain unproved.
pub(super) fn hue_remainder(value: f32) -> f32 {
    if !value.is_finite() {
        INVALID
    } else {
        value % 1.0
    }
}
