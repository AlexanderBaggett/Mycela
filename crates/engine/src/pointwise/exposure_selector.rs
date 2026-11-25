//! pe/ek supplied-map bodies and nz workers; all arrays remain real map roots.
use super::numeric::{add, div, mul, sub};
use super::{
    PointwiseError, PointwiseFilter, PointwiseKind, SharedImageMap, check_render_cancelled,
};
use crate::procedural_schedule::for_each_chunk;
use crate::{ImageGeometry, ProceduralExecution};
pub(super) fn retained<E: 'static>(
    filter: &PointwiseFilter<E>,
    inputs: &[Option<SharedImageMap>],
    geometry: ImageGeometry,
    time: f64,
    execution: ProceduralExecution,
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
    let input = inputs.first().ok_or(PointwiseError::Index)?.clone();
    check_render_cancelled(cancelled)?;
    if filter.kind() == PointwiseKind::Exposure {
        let multiplier = 2.0_f64.powf(filter.parameter(0)?.effective_value_at(time)?) as f32;
        let reciprocal = (1.0 / filter.parameter(1)?.effective_value_at(time)?) as f32;
        if f64::from(reciprocal) == 1.0 && multiplier == 1. {
            return Ok(input);
        }
        let source = input.ok_or(PointwiseError::NullReference)?;
        let size = source.geometry();
        let output = SharedImageMap::new(ImageGeometry::new(size.width(), size.height()));
        for channel in 0..3 {
            let values = source.channel(channel, false)?;
            let target = if values.len() == 1 {
                output.channel(channel, false)?
            } else {
                output.materialize(channel)?
            };
            for_each_chunk::<E>(values.len() as i32, execution, |start, count| {
                let mut ix = start;
                let end = start.wrapping_add(count);
                while ix < end {
                    if ix % 1024 == 0 {
                        check_render_cancelled(cancelled)?;
                    }
                    let at = usize::try_from(ix).map_err(|_| crate::ImageError::Index)?;
                    let value = mul(values.read(at)?, multiplier);
                    let transformed = if value < 0. {
                        (-f64::from(-value).powf(f64::from(reciprocal))) as f32
                    } else {
                        f64::from(value).powf(f64::from(reciprocal)) as f32
                    };
                    target.write(at, transformed)?;
                    ix = ix.wrapping_add(1);
                }
                Ok(())
            })?;
            output.seal_channel(channel)?;
        }
        check_render_cancelled(cancelled)?;
        output.copy_channel_from(3, &source, 3)?;
        output.set_region_store(source.region_store());
        output.merge_effects(&[Some(source)])?;
        return Ok(Some(output));
    }
    let output = SharedImageMap::new(geometry);
    let tolerance = filter.parameter(2)?.effective_value_at(time)? as f32;
    if tolerance == 0. {
        return Ok(Some(output));
    }
    let source = input.ok_or(PointwiseError::NullReference)?;
    let arrays = [
        source.channel(0, false)?,
        source.channel(1, false)?,
        source.channel(2, false)?,
        source.channel(3, false)?,
    ];
    let dense = arrays.each_ref().map(|a| a.len() > 1);
    let target = if dense.into_iter().any(|v| v) {
        output.materialize(0)?
    } else {
        output.channel(0, false)?
    };
    let color = filter
        .parameter(0)?
        .effective_color_at(time)?
        .components()
        .map(|v| v as f32);
    let transparency = filter.parameter(1)?.effective_value_at(time)? as f32;
    for array in &arrays {
        let _ = array.read(0)?;
    }
    let chosen = [color[0], color[1], color[2], transparency];
    for_each_chunk::<E>(target.len() as i32, execution, |start, count| {
        let mut delta = [
            sub(arrays[0].read(0)?, chosen[0]),
            sub(arrays[1].read(0)?, chosen[1]),
            sub(arrays[2].read(0)?, chosen[2]),
            sub(arrays[3].read(0)?, chosen[3]),
        ];
        let mut ix = start;
        let end = start.wrapping_add(count);
        while ix < end {
            if ix % 1024 == 0 {
                check_render_cancelled(cancelled)?;
            }
            let at = usize::try_from(ix).map_err(|_| crate::ImageError::Index)?;
            for channel in 0..4 {
                if dense[channel] {
                    delta[channel] = sub(arrays[channel].read(at)?, chosen[channel]);
                }
            }
            let sum = add(
                add(
                    add(mul(delta[0], delta[0]), mul(delta[1], delta[1])),
                    mul(delta[2], delta[2]),
                ),
                mul(delta[3], delta[3]),
            );
            let value = sub(1., div(f64::from(sum).sqrt() as f32, tolerance));
            if value > 0. {
                target.write(at, value)?;
            }
            ix = ix.wrapping_add(1);
        }
        Ok(())
    })?;
    check_render_cancelled(cancelled)?;
    output.set_region_store(source.region_store());
    output.merge_effects(&[Some(source)])?;
    Ok(Some(output))
}

#[cfg(test)]
#[path = "exposure_selector_test.rs"]
mod tests;
