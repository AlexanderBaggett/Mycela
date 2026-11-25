//! Tone Curve's real retained-curve sampling and protected supplied-image body.
//! The fixed positive rl.a(0,1,.0002) call is shared by the worker and final
//! channel guarantees.
use super::numeric::{mul, scalar_add, sub};
use super::{
    PointwiseChannelGuarantees, PointwiseError, PointwiseFilter, PointwiseKind, SharedImageMap,
};
use crate::{
    AnalyticCurveKind, CurveLayerHandle, FloatBuffer, ImageError, ImageGeometry, SharedCurve,
};

pub(super) fn samples<E>(curve: &SharedCurve<E>) -> Result<FloatBuffer, PointwiseError<E>> {
    // rl.a(float,float,float): all arithmetic before Math.Round is binary32.
    // Tone Curve supplies these actual fixed arguments, not a sampled golden.
    let start = 0.0_f32;
    let end = 1.0_f32;
    let step = 0.0002_f32;
    let count = crate::framework_cast::integer(libm::round(f64::from((end - start) / step)))
        .wrapping_add(1);
    let count = usize::try_from(count).map_err(|_| ImageError::Overflow)?;
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_| ImageError::Allocation)?;
    values.resize(count, 0.0);
    let table = FloatBuffer::new(values);
    let mut digits = 0;
    let mut scaled_step = step;
    while scaled_step <= 1.0 {
        scaled_step *= 10.0;
        digits += 1;
    }
    // The fixed input derives precision4. The exact power is an algorithm
    // operand, not a source-expected cache. Every value comes from live sample().
    let mut power = 1.0_f64;
    for _ in 0..digits {
        power *= 10.0;
    }
    for ix in 0..count {
        let position = start + (ix as f32) * step;
        let time = (libm::round(f64::from(position) * power) / power) as f32;
        table.write(ix, curve.sample(time)?)?;
    }
    Ok(table)
}

fn apply(
    source: &FloatBuffer,
    destination: &FloatBuffer,
    table: &FloatBuffer,
    last: i32,
) -> Result<(), ImageError> {
    // i0.a.bx reads source, then table, then writes destination. Do not use a
    // prevalidated zip: short destinations retain the actual committed prefix.
    for ix in 0..source.len() {
        let value = source.read(ix)?;
        if value >= 1.0 {
            destination.write(ix, table.read(last as usize)?)?;
        } else if value <= 0.0 {
            destination.write(ix, table.read(0)?)?;
        } else {
            let position = mul(value, last as f32);
            let index = crate::framework_cast::integer(f64::from(position));
            // Native Framework conv.i4 of NaN is negative. The source leaves
            // this destination cell unchanged; it does not interpolate a NaN.
            if index >= 0 {
                let lower = table.read(index as usize)?;
                let upper = table.read(index.wrapping_add(1) as usize)?;
                let fraction = sub(position, index as f32);
                // In the captured Framework64 profile, i0+a.bx's final ADD
                // retains the right interpolation NaN when both terms are
                // NaN (scalar and dense LUT cases). Preserve finite a+b order;
                // this is a policy for this source site, not all CLR arithmetic.
                destination.write(ix, scalar_add(lower, mul(fraction, sub(upper, lower))))?;
            }
        }
    }
    Ok(())
}

/// Actual i0.g supplied table/image body. A fresh output is allocated before
/// the table's null/length checks. Ignored channels retain the source arrays;
/// affected channels use source scalar/dense allocation and sequential errors.
/// Source node/property/APV scheduling is not represented by this helper.
pub fn render_tone_curve(
    input: Option<&SharedImageMap>,
    width: i32,
    height: i32,
    table: Option<&FloatBuffer>,
    ignored: [bool; 4],
) -> Result<SharedImageMap, ImageError> {
    let out = SharedImageMap::new(ImageGeometry::new(width, height));
    let table = table.ok_or(ImageError::NullReference)?;
    if table.len() < 2 {
        return Err(ImageError::Argument);
    }
    let last = table.len().wrapping_sub(1) as i32;
    for (channel, ignore) in ignored.into_iter().enumerate() {
        let input = input.ok_or(ImageError::NullReference)?;
        if ignore {
            let array = input.channel(channel as i32, false)?;
            out.set_buffer(channel as i32, Some(&array))?;
            out.seal_channel(channel as i32)?;
        } else {
            let array = input.channel(channel as i32, false)?;
            let destination = out.destination(channel as i32, &array)?;
            apply(&array, &destination, table, last)?;
            out.seal_channel(channel as i32)?;
        }
    }
    out.set_region_store(input.ok_or(ImageError::NullReference)?.region_store());
    out.merge_effects(&[input.cloned()])?;
    Ok(out)
}

pub(super) fn render<E: 'static>(
    filter: &PointwiseFilter<E>,
    inputs: &[Option<SharedImageMap>],
    width: i32,
    height: i32,
    time: Option<f64>,
) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
    let input = inputs.first().cloned().ok_or(PointwiseError::Index)?;
    let property = filter.parameter(0)?;
    if property.curve()?.layers().is_empty() {
        return Ok(input);
    }
    // Sampling errors precede even the first choice's string resolution.
    let table = samples(&property.curve()?)?;
    let mut ignored = [false; 4];
    for (channel, value) in ignored.iter_mut().enumerate() {
        let property = filter.parameter(channel as i32 + 1)?;
        let choice = if let Some(time) = time {
            let index = usize::try_from(property.effective_index_at(time)?)
                .map_err(|_| PointwiseError::Index)?;
            property
                .options()?
                .entries()
                .get(index)
                .cloned()
                .ok_or(PointwiseError::Index)?
        } else {
            property.text()?
        };
        *value = choice.as_deref() == Some(&[b'N' as u16, b'o' as u16]);
    }
    Ok(Some(render_tone_curve(
        input.as_ref(),
        width,
        height,
        Some(&table),
        ignored,
    )?))
}

fn legacy_fields<E: 'static>(filter: &PointwiseFilter<E>) -> Result<[f32; 3], PointwiseError<E>> {
    filter
        .parameter(0)?
        .custom_object()?
        .ok_or(PointwiseError::NullReference)?
        .legacy_tone_curve()
}

pub(super) fn render_legacy<E: 'static>(
    filter: &PointwiseFilter<E>,
    inputs: &[Option<SharedImageMap>],
    width: i32,
    height: i32,
    time: Option<f64>,
) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
    let input = inputs.first().ok_or(PointwiseError::Index)?.as_ref();
    let [shape, start, end] = legacy_fields(filter)?;
    let table = wizard_table::<E>(shape, start, end)?;
    let mut ignored = [false; 4];
    for (channel, ignore) in ignored.iter_mut().enumerate() {
        *ignore = super::effect_maps::choice_is_at(filter, channel as i32 + 1, time, "No")?;
    }
    Ok(Some(render_tone_curve(
        input,
        width,
        height,
        Some(&table),
        ignored,
    )?))
}

pub(super) fn legacy_guarantees<E: 'static>(
    filter: &PointwiseFilter<E>,
    input: PointwiseChannelGuarantees,
) -> Result<PointwiseChannelGuarantees, PointwiseError<E>> {
    let mut affected = [false; 4];
    let mut all_disabled = true;
    for (channel, value) in affected.iter_mut().enumerate() {
        let property = filter.parameter(channel as i32 + 1)?;
        let animated = property.axis_is_animated(0)? || property.has_animated_binding()?;
        *value = animated || property.index() == 0;
        all_disabled &= !animated && property.index() == 1;
    }
    let [shape, start, end] = legacy_fields(filter)?;
    let ranges = std::array::from_fn(|channel| input.effective_range(channel));
    let mut constants = input.constants();
    if all_disabled
        || (ranges.into_iter().all(|value| value) && [shape, start, end] == [0.0, 0.0, 1.0])
    {
        return Ok(PointwiseChannelGuarantees::new(
            constants,
            ranges,
            input.has_scene(),
            Some(0),
        ));
    }
    for (channel, value) in constants.iter_mut().enumerate() {
        if affected[channel] {
            *value = if start == end {
                Some(start)
            } else if start == 0.0 && *value == Some(0.0) {
                Some(0.0)
            } else {
                None
            };
        }
    }
    Ok(PointwiseChannelGuarantees::new(
        constants,
        affected,
        input.has_scene(),
        None,
    ))
}

pub(super) fn install_arc<E: 'static>(
    target: &PointwiseFilter<E>,
    shape: f64,
    start_percent: f32,
    end_percent: f32,
) -> Result<(), PointwiseError<E>> {
    let curve = target.parameter(0)?.curve()?;
    let layer = CurveLayerHandle::new(AnalyticCurveKind::Arc);
    let bend = (1.0 - 2.0_f64.powf(-shape.abs())).copysign(shape);
    let shape = crate::round_decimal_places(bend * 0.5 + 0.5, 3)
        .map_err(|_| PointwiseError::Arithmetic)?
        * 100.0;
    layer
        .shape_control()
        .ok_or(PointwiseError::InvalidOperation)?
        .set_scalar(shape as f32)?;
    for (id, value) in [(11, start_percent), (12, end_percent)] {
        layer
            .control(id)?
            .ok_or(PointwiseError::InvalidOperation)?
            .set_scalar(value)?;
    }
    curve.clear();
    curve.append(&[Some(layer)]);
    Ok(())
}

pub(super) fn migrate_legacy<E: 'static>(
    source: &PointwiseFilter<E>,
) -> Result<PointwiseFilter<E>, PointwiseError<E>> {
    let target = PointwiseFilter::new(PointwiseKind::ToneCurve)?;
    let [shape, start, end] = legacy_fields(source)?;
    // i0(dh) multiplies the stored Single endpoints before the control setter.
    install_arc(&target, f64::from(shape), start * 100.0, end * 100.0)?;
    for index in 1..=4 {
        target
            .parameter(index)?
            .set_index(source.parameter(index)?.index())?;
    }
    Ok(target)
}

fn matches_samples<E>(
    curve: &SharedCurve<E>,
    expected: impl Fn(i32) -> f64,
) -> Result<bool, PointwiseError<E>> {
    for index in 0..=1000 {
        let value = f64::from(curve.sample(index as f32 / 1000.0)?);
        let equal = (expected(index) - value).abs() < 0.00001;
        if !equal {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(super) fn guarantees<E: 'static>(
    filter: &PointwiseFilter<E>,
    input: PointwiseChannelGuarantees,
) -> Result<PointwiseChannelGuarantees, PointwiseError<E>> {
    let curve = filter.parameter(0)?.curve()?;
    let ranges = std::array::from_fn(|channel| input.effective_range(channel));
    let forward =
        || PointwiseChannelGuarantees::new(input.constants(), ranges, input.has_scene(), Some(0));
    if curve.layers().is_empty() {
        return Ok(forward());
    }
    let mut affected = [false; 4];
    let mut all_disabled = true;
    for (channel, value) in affected.iter_mut().enumerate() {
        let property = filter.parameter(channel as i32 + 1)?;
        let animated = property.axis_is_animated(0)? || property.has_animated_binding()?;
        *value = animated || property.index() == 0;
        all_disabled &= !animated && property.index() == 1;
    }
    if all_disabled
        || (ranges.into_iter().all(|in_range| in_range)
            && matches_samples(&curve, |index| f64::from(index) / 1000.0)?)
    {
        return Ok(forward());
    }

    let mut constants = input.constants();
    for (channel, value) in constants.iter_mut().enumerate() {
        if !affected[channel] {
            continue;
        }
        *value = match *value {
            Some(value) if value >= 1.0 => Some(curve.sample(1.0)?),
            Some(value) if value <= 0.0 => Some(curve.sample(0.0)?),
            Some(value) => {
                // i0.a6 uses length * value - 1; the dense worker uses
                // value * (length - 1). Keep the source's distinct offset.
                let table = samples(&curve)?;
                let position = sub(mul(value, table.len() as f32), 1.0);
                let index = crate::framework_cast::integer(f64::from(position));
                let index = usize::try_from(index).map_err(|_| ImageError::Index)?;
                let low = table.read(index)?;
                let high = table.read(index + 1)?;
                Some(scalar_add(
                    low,
                    mul(sub(position, index as f32), sub(high, low)),
                ))
            }
            None => {
                let first = curve.sample(0.0)?;
                matches_samples(&curve, |_| f64::from(first))?.then_some(first)
            }
        };
    }
    Ok(PointwiseChannelGuarantees::new(
        constants,
        std::array::from_fn(|channel| affected[channel] || ranges[channel]),
        input.has_scene(),
        None,
    ))
}

/// Source r7.b/a fixed table for the hidden Wizard Tone Curve node.
fn wizard_table<E>(curve: f32, start: f32, end: f32) -> Result<FloatBuffer, PointwiseError<E>> {
    if curve.is_infinite() {
        return Err(PointwiseError::UnsupportedCapability(
            "Wizard Tone Curve infinite iteration",
        ));
    }
    let positive = f64::from(curve) >= 0.;
    let magnitude = f64::from(curve).abs();
    let mut input = vec![0.0f32; 10000];
    for (index, value) in input.iter_mut().enumerate().skip(1) {
        *value = index as f32 / 9999.;
    }
    let mut output = vec![0.0f32; 10000];
    let mut iteration = 0i32;
    if curve == 0. {
        output = input;
    } else {
        while f64::from(iteration) < magnitude {
            let remaining = magnitude - f64::from(iteration);
            if iteration > 0 {
                input.clone_from(&output);
            }
            let fractional = remaining > 0. && remaining < 1.;
            let previous = if fractional {
                Some(output.clone())
            } else {
                None
            };
            for index in 1..input.len() {
                let value = input[index];
                output[index] = if positive {
                    let delta = f64::from(value) - 1.;
                    libm::sqrt(1. - delta * delta) as f32
                } else {
                    1. - libm::sqrt(1. - f64::from(value * value)) as f32
                };
            }
            if let Some(mut previous) = previous {
                let weight = remaining as f32;
                let inverse = 1. - weight;
                for index in 0..output.len() {
                    previous[index] = previous[index] * inverse + output[index] * weight;
                }
                if magnitude < 1. {
                    for index in 0..output.len() {
                        previous[index] += input[index] * inverse;
                    }
                }
                output = previous;
                break;
            }
            iteration = iteration.checked_add(1).ok_or(PointwiseError::Arithmetic)?;
        }
    }
    if start != 0. || end != 1. {
        for value in &mut output {
            *value = *value * (end - start) + start;
        }
    }
    Ok(FloatBuffer::new(output))
}
pub(super) fn render_wizard<E: 'static>(
    filter: &PointwiseFilter<E>,
    inputs: &[Option<SharedImageMap>],
    width: i32,
    height: i32,
    time: f64,
) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
    let input = inputs.first().cloned().ok_or(PointwiseError::Index)?;
    let table = wizard_table::<E>(
        filter.parameter(0)?.effective_value_at(time)? as f32,
        filter.parameter(1)?.effective_value_at(time)? as f32,
        filter.parameter(2)?.effective_value_at(time)? as f32,
    )?;
    let mut ignored = [false; 4];
    for (channel, ignore) in ignored.iter_mut().enumerate() {
        let parameter = filter.parameter(channel as i32 + 3)?;
        let index = parameter.effective_index_at(time)?;
        let options = parameter.options()?.entries();
        let selected = options
            .get(usize::try_from(index).map_err(|_| PointwiseError::Index)?)
            .ok_or(PointwiseError::Index)?;
        *ignore = selected.as_deref() == Some(&[b'N' as u16, b'o' as u16]);
    }
    Ok(Some(render_tone_curve(
        input.as_ref(),
        width,
        height,
        Some(&table),
        ignored,
    )?))
}

impl<E> PointwiseFilter<E> {
    /// Source i0.g(): sequentially choose Red Yes and Green/Blue/Alpha No.
    /// This is the actual stored-choice operation; no option strings are cached.
    pub fn set_tone_curve_red_only(&self) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        if self.kind() != PointwiseKind::ToneCurve {
            return Err(PointwiseError::UnsupportedCapability(
                "Tone Curve channel operation on another node kind",
            ));
        }
        for (property, index) in [(1, 0), (2, 1), (3, 1), (4, 1)] {
            self.parameter(property)?.set_index(index)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod worker_laws {
    use super::*;
    // Static source-body laws, not original-runtime output goldens.
    #[test]
    fn endpoints_nan_skip_and_short_destination_keep_real_writes() {
        let source = FloatBuffer::new(vec![f32::NEG_INFINITY, 0.25, f32::NAN, f32::INFINITY]);
        let table = FloatBuffer::new(vec![0.0, 0.5, 1.0]);
        let out = FloatBuffer::new(vec![9.0; 4]);
        apply(&source, &out, &table, 2).unwrap();
        assert_eq!(out.values(), vec![0.0, 0.25, 9.0, 1.0]);
        let short = FloatBuffer::new(vec![8.0]);
        assert_eq!(apply(&source, &short, &table, 2), Err(ImageError::Index));
        assert_eq!(short.values(), vec![0.0]);
    }
    #[test]
    fn sampling_uses_the_live_curve_and_rounds_fixed_grid_before_sampling() {
        let curve = SharedCurve::<()>::new(2.0, 3.0, false);
        let table = samples(&curve).unwrap();
        assert_eq!(table.len(), 5001);
        assert_eq!(table.read(0).unwrap(), 2.0);
        assert_eq!(table.read(5000).unwrap(), 2.0);
        curve.set_minimum(4.0);
        assert_eq!(samples(&curve).unwrap().read(123).unwrap(), 4.0);
    }
}

#[cfg(test)]
#[path = "wizard_shapes_test.rs"]
mod wizard_shapes_tests;
