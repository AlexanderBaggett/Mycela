//! Trigonometry for code paths measured against the .NET Framework x64 runtime.

/// Return sine and cosine rounded to binary64. The Framework's x87 result
/// matters when a small displacement samples immediately next to a zero pixel.
pub fn sin_cos(angle: f64) -> (f64, f64) {
    if !angle.is_finite() {
        let result = if angle.is_nan() {
            f64::from_bits(angle.to_bits() | 0x0008_0000_0000_0000)
        } else {
            f64::from_bits(0xfff8_0000_0000_0000)
        };
        return (result, result);
    }
    if angle.abs() >= (1_u64 << 63) as f64 {
        // The original x87 instructions leave an out-of-range operand intact.
        return (angle, angle);
    }
    finite_sin_cos(angle)
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[allow(unsafe_code)]
fn finite_sin_cos(angle: f64) -> (f64, f64) {
    let mut sin = 0.0_f64;
    let mut cos = 0.0_f64;
    // SAFETY: The input and two distinct output pointers refer to live local
    // f64 values. The caller checks the FSINCOS input range, so the two stores
    // pop both results and restore the x87 stack depth.
    unsafe {
        core::arch::asm!(
            "fld qword ptr [{angle}]",
            "fsincos",
            "fstp qword ptr [{cos}]",
            "fstp qword ptr [{sin}]",
            angle = in(reg) &raw const angle,
            cos = in(reg) &raw mut cos,
            sin = in(reg) &raw mut sin,
            out("st(0)") _,
            out("st(1)") _,
            options(nostack, preserves_flags),
        );
    }
    (sin, cos)
}

#[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
fn finite_sin_cos(angle: f64) -> (f64, f64) {
    libm::sincos(angle)
}

#[cfg(all(test, any(target_arch = "x86", target_arch = "x86_64")))]
mod tests {
    use super::sin_cos;

    #[test]
    fn preserves_framework_turns_and_out_of_range_operands() {
        for (angle, sin, cos) in [
            (
                0x8000_0000_0000_0000,
                0x8000_0000_0000_0000,
                0x3ff0_0000_0000_0000,
            ),
            (
                0x4009_21fb_5444_2d18,
                0x3ca1_a600_0000_0000,
                0xbff0_0000_0000_0000,
            ),
            (
                0x4012_d97c_7f33_21d2,
                0xbff0_0000_0000_0000,
                0xbcaa_7900_0000_0000,
            ),
            (
                0x43e1_58e4_6091_3d00,
                0x43e1_58e4_6091_3d00,
                0x43e1_58e4_6091_3d00,
            ),
            (
                0x7ff0_0000_0000_0000,
                0xfff8_0000_0000_0000,
                0xfff8_0000_0000_0000,
            ),
        ] {
            let calculated = sin_cos(f64::from_bits(angle));
            assert_eq!((calculated.0.to_bits(), calculated.1.to_bits()), (sin, cos));
        }
    }
}
