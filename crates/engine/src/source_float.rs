// Pinned source arithmetic selects the first NaN operand and quiets its stored
// payload. Keep that choice explicit so optimized commutative operations cannot
// select the other operand's sign/payload.
pub(crate) fn first_nan(a: f64, b: f64) -> Option<f64> {
    let value = if a.is_nan() {
        a
    } else if b.is_nan() {
        b
    } else {
        return None;
    };
    Some(f64::from_bits(value.to_bits() | 0x0008_0000_0000_0000))
}

pub(crate) fn ordered_add(a: f64, b: f64) -> f64 {
    match first_nan(a, b) {
        Some(value) => value,
        None => a + b,
    }
}
