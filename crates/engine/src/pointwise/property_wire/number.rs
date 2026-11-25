//! Framework Double->Decimal conversion profile used by Limits.IsValid.
//! This extends the existing F057 fixed-limit converter to compare arbitrary
//! retained Decimal bounds exactly; it never replaces the stored input Double.
use super::*;
use std::cmp::Ordering;
pub(crate) fn converted<E>(value: f64) -> Result<DecimalBits, LivePropertyWireError<E>> {
    const POWERS: [f64; 29] = [
        1., 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10, 1e11, 1e12, 1e13, 1e14, 1e15, 1e16,
        1e17, 1e18, 1e19, 1e20, 1e21, 1e22, 1e23, 1e24, 1e25, 1e26, 1e27, 1e28,
    ];
    let exponent = ((value.to_bits() >> 52) & 2047) as i32 - 1022;
    if exponent < -94 {
        return Ok(DecimalBits::new([0; 4])?);
    }
    if exponent > 96 {
        return Err(LivePropertyWireError::DecimalOverflow);
    }
    let mut scale = 14 - ((exponent * 19728) >> 16);
    let mut scaled = value.abs();
    if scale >= 0 {
        scale = scale.min(28);
        scaled *= POWERS[scale as usize];
    } else if scale != -1 || scaled >= 1e15 {
        scaled /= POWERS[(-scale) as usize];
    } else {
        scale = 0;
    }
    if scaled < 1e14 && scale < 28 {
        scaled *= 10.;
        scale += 1;
    }
    let mut coefficient = scaled as u128;
    let remainder = scaled - coefficient as f64;
    if remainder > 0.5 || (remainder == 0.5 && coefficient & 1 != 0) {
        coefficient += 1;
    }
    if coefficient == 0 {
        return Ok(DecimalBits::new([0; 4])?);
    }
    if scale < 0 {
        coefficient *= 10_u128.pow((-scale) as u32);
        scale = 0;
    }
    if coefficient > (1_u128 << 96) - 1 {
        return Err(LivePropertyWireError::DecimalOverflow);
    }
    Ok(DecimalBits::new([
        coefficient as u32,
        (coefficient >> 32) as u32,
        (coefficient >> 64) as u32,
        ((scale as u32) << 16)
            | if value.is_sign_negative() {
                0x8000_0000
            } else {
                0
            },
    ])?)
}
fn magnitude(d: DecimalBits) -> (u128, usize, bool) {
    let b = d.bits();
    (
        u128::from(b[0]) | (u128::from(b[1]) << 32) | (u128::from(b[2]) << 64),
        ((b[3] >> 16) & 255) as usize,
        b[3] >> 31 != 0,
    )
}
fn compare(left: DecimalBits, right: DecimalBits) -> Ordering {
    let (a, sa, na) = magnitude(left);
    let (b, sb, nb) = magnitude(right);
    if a == 0 && b == 0 {
        return Ordering::Equal;
    }
    let na = na && a != 0;
    let nb = nb && b != 0;
    if na != nb {
        return if na {
            Ordering::Less
        } else {
            Ordering::Greater
        };
    }
    if a == 0 {
        return if nb {
            Ordering::Greater
        } else {
            Ordering::Less
        };
    }
    if b == 0 {
        return if na {
            Ordering::Less
        } else {
            Ordering::Greater
        };
    }
    // At most 29 coefficient digits plus 28 scale digits. Decimal digit
    // alignment avoids overflowing even u128 for opposite scale extremes.
    let scale = sa.max(sb);
    let a = format!("{a}{}", "0".repeat(scale - sa));
    let b = format!("{b}{}", "0".repeat(scale - sb));
    let result = a.len().cmp(&b.len()).then_with(|| a.cmp(&b));
    if na { result.reverse() } else { result }
}
pub(crate) fn validate_wire_number<E>(
    value: f64,
    limits: &SharedLimits,
) -> Result<(), LivePropertyWireError<E>> {
    let decimal = converted(value)?;
    // Source evaluates Minimum before Maximum and skips the latter comparison
    // when the first fails. Each read is from the held actual Limits root.
    if compare(decimal, limits.snapshot().minimum) == Ordering::Less {
        return Err(LivePropertyWireError::LimitViolation);
    }
    if compare(decimal, limits.snapshot().maximum) == Ordering::Greater {
        return Err(LivePropertyWireError::LimitViolation);
    }
    Ok(())
}

#[cfg(test)]
mod laws {
    use super::*;
    fn d(coefficient: u32, scale: u32, negative: bool) -> DecimalBits {
        DecimalBits::new([
            coefficient,
            0,
            0,
            scale << 16 | if negative { 0x8000_0000 } else { 0 },
        ])
        .unwrap()
    }
    #[test]
    fn decimal_comparison_orders_zero_fractional_and_scale_extremes() {
        for zero in [d(0, 0, false), d(0, 28, true)] {
            assert_eq!(compare(zero, d(1, 1, false)), Ordering::Less);
            assert_eq!(compare(d(1, 1, false), zero), Ordering::Greater);
            assert_eq!(compare(zero, d(1, 1, true)), Ordering::Greater);
            assert_eq!(compare(d(1, 1, true), zero), Ordering::Less);
        }
        assert_eq!(compare(d(1, 28, false), d(1, 0, false)), Ordering::Less);
        assert_eq!(compare(d(10, 1, true), d(1, 0, true)), Ordering::Equal);
    }
    #[test]
    fn held_decimal_bounds_are_not_lossy_double_comparisons() {
        let l = SharedLimits::default();
        let low = d(1, 1, false);
        l.edit(crate::LimitsEdit::Minimum(low));
        assert!(matches!(
            validate_wire_number::<()>(0., &l),
            Err(LivePropertyWireError::LimitViolation)
        ));
        assert!(validate_wire_number::<()>(0.1, &l).is_ok());
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(matches!(
                validate_wire_number::<()>(bad, &l),
                Err(LivePropertyWireError::DecimalOverflow)
            ));
        }
    }
}
