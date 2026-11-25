//! Owner-neutral operations of the actual ordinary os authority. Owners keep
//! distinct state/notification/Size policies; these functions own no model.
use super::{PointwiseError, max64, min64};
use crate::{AnimationError, GradientAnimation, ImageError, SharedLimits, round_decimal_places};
pub(super) fn normalize<E>(limits: &SharedLimits, value: f64) -> Result<f64, PointwiseError<E>> {
    let l = limits.snapshot();
    let value = round_decimal_places(value, l.decimal_places)
        .map_err(|_| ImageError::ArgumentOutOfRange)?;
    Ok(min64(
        l.maximum.to_framework_f64(),
        max64(l.minimum.to_framework_f64(), value),
    ))
}

// Qualified os2467 operation order. Its broad nonfinite golden is Mono-only;
// the native generator floor covers finite zero-modifier values, not every JIT.
pub(in crate::pointwise) fn seed_modifier(value: f64, modifier: f64, negative: bool) -> f64 {
    use crate::source_float::ordered_add;
    if value >= 0.0 && modifier >= 0.0 {
        ordered_add(value, modifier) % 1_000_000.0
    } else {
        let value = if value >= 0.0 {
            value
        } else {
            -ordered_add(value, 1.0)
        };
        let modifier = if modifier >= 0.0 {
            modifier
        } else {
            -ordered_add(modifier, 1.0)
        };
        let result = ordered_add(value, modifier) % 1_000_000.0;
        if negative {
            -ordered_add(result, 1.0)
        } else {
            result
        }
    }
}
pub(super) fn commit_pair(
    current: &mut [f64; 2],
    previous: &mut [f64; 2],
    values: [f64; 2],
) -> bool {
    let changed = (0..2).any(|i| current[i] != values[i] || previous[i] != current[i]);
    if changed {
        *current = values;
        *previous = values;
    }
    changed
}
pub(super) fn commit_axis(
    current: &mut [f64; 2],
    previous: &mut [f64; 2],
    axis: usize,
    value: f64,
) -> bool {
    let changed = current[axis] != value || previous[axis] != current[axis];
    if changed {
        current[axis] = value;
        previous[axis] = value;
    }
    changed
}
pub(super) fn is_animated<E: 'static>(
    get: impl Fn() -> Result<Option<GradientAnimation<E>>, PointwiseError<E>>,
) -> Result<bool, PointwiseError<E>> {
    match get()? {
        Some(a) => Ok(a.is_active()?),
        None => Ok(false),
    }
}
pub(super) fn axis_active<E: 'static>(
    get: impl Fn() -> Result<Option<GradientAnimation<E>>, PointwiseError<E>>,
    axis: usize,
) -> Result<bool, PointwiseError<E>> {
    if !is_animated(&get)? {
        return Ok(false);
    }
    let count = get()?.ok_or(AnimationError::NullReference)?.count();
    if i64::try_from(axis).unwrap_or(i64::MAX) >= i64::from(count) {
        return Ok(false);
    }
    Ok(get()?
        .ok_or(AnimationError::NullReference)?
        .flags()
        .ok_or(AnimationError::NullReference)?
        .value(axis as i32)
        .map_err(|_| AnimationError::<E>::Index)?)
}
pub(super) fn curve<E: 'static>(
    get: impl Fn() -> Result<Option<GradientAnimation<E>>, PointwiseError<E>>,
    axis: usize,
) -> Result<crate::SharedCurve<E>, PointwiseError<E>> {
    Ok(get()?
        .ok_or(AnimationError::NullReference)?
        .curves()
        .ok_or(AnimationError::NullReference)?
        .value(axis as i32)?
        .ok_or(AnimationError::NullReference)?)
}
pub(super) fn animated_choice_index<E: 'static>(
    curve: crate::SharedCurve<E>,
    option_count: usize,
    time: f64,
) -> Result<i32, PointwiseError<E>> {
    let value = f64::from(curve.sample(time as f32).map_err(AnimationError::Curve)?);
    choice_index_from_numeric(value, option_count)
}

pub(crate) fn choice_index_from_numeric<E>(
    value: f64,
    option_count: usize,
) -> Result<i32, PointwiseError<E>> {
    let rounded = round_decimal_places(value, 0).map_err(|_| ImageError::ArgumentOutOfRange)?;
    let index = crate::framework_cast::integer(rounded).wrapping_sub(1);
    Ok(if index < 0 {
        0
    } else if index as i64 >= option_count as i64 {
        (option_count as i32).wrapping_sub(1)
    } else {
        index
    })
}
pub(super) fn pair_value<E: 'static>(
    get: impl Fn() -> Result<Option<GradientAnimation<E>>, PointwiseError<E>>,
    limits: impl Fn() -> Result<SharedLimits, PointwiseError<E>>,
    raw: impl Fn() -> Result<[f64; 2], PointwiseError<E>>,
    axis: usize,
    time: f64,
) -> Result<f64, PointwiseError<E>> {
    if axis >= 2 {
        return Err(PointwiseError::Index);
    }
    if axis_active(&get, axis)? {
        let value = f64::from(
            curve(get, axis)?
                .sample(time as f32)
                .map_err(AnimationError::Curve)?,
        );
        if limits()?.snapshot().decimal_places == 0 {
            Ok(round_decimal_places(value, 0).map_err(|_| ImageError::ArgumentOutOfRange)?)
        } else {
            Ok(value)
        }
    } else {
        Ok(raw()?[axis])
    }
}
pub(super) fn scalar_value<E: 'static>(
    get: impl Fn() -> Result<Option<GradientAnimation<E>>, PointwiseError<E>>,
    limits: impl Fn() -> Result<SharedLimits, PointwiseError<E>>,
    raw: impl Fn() -> Result<f64, PointwiseError<E>>,
    time: f64,
) -> Result<f64, PointwiseError<E>> {
    if is_animated(&get)? {
        let value = f64::from(
            curve(get, 0)?
                .sample(time as f32)
                .map_err(AnimationError::Curve)?,
        );
        Ok(if limits()?.snapshot().decimal_places == 0 {
            value.floor()
        } else {
            value
        })
    } else {
        raw()
    }
}
