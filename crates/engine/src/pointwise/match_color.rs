//! h's two actual RGB stages. Context/input/array geometry remains distinct.
use super::adaptive::{floats, index};
use super::equalize::mean;
use super::numeric::{add, mul, sub};
use super::{
    ArithmeticOperation, ArithmeticWorker, PointwiseError, PointwiseFilter, SharedImageMap,
    check_render_cancelled,
};
use crate::{ImageError, ImageGeometry, ProceduralExecution, procedural_schedule::for_each_chunk};

pub(super) fn retained<E: 'static>(
    filter: &PointwiseFilter<E>,
    inputs: &[Option<SharedImageMap>],
    geometry: ImageGeometry,
    time: f64,
    execution: ProceduralExecution,
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
    let mut original = inputs.first().ok_or(PointwiseError::Index)?.clone();
    let reference = inputs.get(1).ok_or(PointwiseError::Index)?.clone();
    check_render_cancelled(cancelled)?;
    let strength = (filter.parameter(0)?.effective_value_at(time)? / 100.) as f32;
    let conformity = (filter.parameter(1)?.effective_value_at(time)? / 100.) as f32;
    let remaining = sub(1., conformity);
    if strength == 0. && conformity == 0. {
        return Ok(original);
    }
    if conformity > 0. {
        let source = original.as_ref().ok_or(PointwiseError::NullReference)?;
        let arrays = [
            source.channel(0, false)?,
            source.channel(1, false)?,
            source.channel(2, false)?,
        ];
        let dense = arrays.each_ref().map(|a| a.len() > 1);
        let count = arrays.iter().map(|a| a.len()).max().unwrap_or(0);
        let count = i32::try_from(count).map_err(|_| ImageError::Overflow)?;
        let grey = floats(count)?;
        for_each_chunk::<E>(count, execution, |start, count| {
            // These three scalar probes precede the loop in every actual bx.
            let mut red = arrays[0].read(0)?;
            let mut green = arrays[1].read(0)?;
            let mut blue = arrays[2].read(0)?;
            let mut cell = start;
            while cell < start.wrapping_add(count) {
                if cell % 1024 == 0 {
                    check_render_cancelled(cancelled)?;
                }
                let at = index(cell)?;
                if dense[0] {
                    red = arrays[0].read(at)?;
                }
                if dense[1] {
                    green = arrays[1].read(at)?;
                }
                if dense[2] {
                    blue = arrays[2].read(at)?;
                }
                grey.write(
                    at,
                    mul(
                        conformity,
                        add(add(mul(0.299, red), mul(0.587, green)), mul(0.114, blue)),
                    ),
                )?;
                cell = cell.wrapping_add(1);
            }
            Ok(())
        })?;
        drop(arrays);
        let intermediate = source.geometry_copy();
        for channel in 0..3 {
            let values = source.channel(channel, false)?;
            let destination = intermediate.destination_count(channel, count as usize)?;
            let _ = values.read(0)?;
            let dense = values.len() > 1;
            for_each_chunk::<E>(count, execution, |start, count| {
                if dense {
                    let mut cell = start;
                    while cell < start.wrapping_add(count) {
                        if cell % 1024 == 0 {
                            check_render_cancelled(cancelled)?;
                        }
                        let at = index(cell)?;
                        destination
                            .write(at, add(mul(remaining, values.read(at)?), grey.read(at)?))?;
                        cell = cell.wrapping_add(1);
                    }
                } else {
                    let constant = mul(remaining, values.read(0)?);
                    let mut cell = start;
                    while cell < start.wrapping_add(count) {
                        if cell % 1024 == 0 {
                            check_render_cancelled(cancelled)?;
                        }
                        let at = index(cell)?;
                        destination.write(at, add(constant, grey.read(at)?))?;
                        cell = cell.wrapping_add(1);
                    }
                }
                Ok(())
            })?;
            intermediate.seal_channel(channel)?;
        }
        drop(grey);
        intermediate.copy_channel_from(3, source, 3)?;
        intermediate.copy_auxiliary(source)?;
        original = Some(intermediate);
        original
            .as_ref()
            .ok_or(PointwiseError::NullReference)?
            .seal_permanently()?;
    }
    let output = SharedImageMap::new(geometry);
    for channel in 0..3 {
        let values = original
            .as_ref()
            .ok_or(PointwiseError::NullReference)?
            .channel(channel, false)?;
        let from = mul(mean(&values, cancelled)?, strength);
        let target = reference
            .as_ref()
            .ok_or(PointwiseError::NullReference)?
            .channel(channel, false)?;
        let to = mul(mean(&target, cancelled)?, strength);
        let destination = output.destination_count(channel, values.len())?;
        // h IL551 NEG, then ADD; preserve signed-zero/NaN sign, no 0-x rewrite.
        let shift = add(-from, to);
        let worker = ArithmeticWorker::new(
            ArithmeticOperation::AddScalar,
            Some(destination),
            Some(values),
            None,
            shift,
        );
        for_each_chunk::<E>(worker.len()?, execution, |start, count| {
            let end = start.wrapping_add(count);
            for offset in (start..end).step_by(1024) {
                check_render_cancelled(cancelled)?;
                worker.run_range(offset, (end - offset).min(1024))?;
            }
            Ok(())
        })?;
        output.seal_channel(channel)?;
    }
    check_render_cancelled(cancelled)?;
    let original = original.as_ref().ok_or(PointwiseError::NullReference)?;
    output.copy_channel_from(3, original, 3)?;
    output.copy_auxiliary(original)?;
    Ok(Some(output))
}
#[cfg(test)]
#[path = "match_art_test.rs"]
mod tests;
