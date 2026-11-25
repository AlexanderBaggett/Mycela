// Observed Native Framework64 conv.i4 profile. Source methods apply their own
// subsequent Abs/allocation/index operations; this conversion does not normalize
// values or preselect those errors.
pub(crate) fn integer(value: f64) -> i32 {
    if (-2147483648.0..2147483648.0).contains(&value) {
        value as i32
    } else {
        i32::MIN
    }
}
