//! Wizard Color (aa) supplied-image body and its retained color properties.
//! Shared workers used by the supplied-image and connected graph renderers.
use super::{PointwiseError, PointwiseFilter, SharedImageMap};
use crate::procedural_schedule::for_each_chunk;
use crate::{ImageGeometry, ProceduralExecution};

// Keep the source's ordered comparisons and signed-zero behavior.
#[allow(clippy::manual_clamp)]
pub(super) fn retained<E: 'static>(
    filter: &PointwiseFilter<E>,
    inputs: &[Option<SharedImageMap>],
    geometry: ImageGeometry,
    time: f64,
    execution: ProceduralExecution,
) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
    let input = inputs
        .first()
        .ok_or(PointwiseError::Index)?
        .as_ref()
        .ok_or(PointwiseError::NullReference)?;
    let first = filter
        .parameter(0)?
        .effective_color_at(time)?
        .components()
        .map(|v| v as f32);
    let last = filter
        .parameter(1)?
        .effective_color_at(time)?
        .components()
        .map(|v| v as f32);
    let source = input.channel(0, false)?;
    let output = SharedImageMap::new(geometry);
    for channel in 0..3usize {
        let previous =
            (0..channel).find(|&i| first[i] == first[channel] && last[i] == last[channel]);
        if let Some(previous) = previous {
            let buffer = output.channel(previous as i32, false)?;
            output.set_buffer(channel as i32, Some(&buffer))?;
        } else if first[channel] == last[channel] {
            output.set_constant(channel as i32, first[channel])?;
        } else {
            let destination = output.destination_count(channel as i32, source.len())?;
            let length = i32::try_from(source.len()).map_err(|_| crate::ImageError::Overflow)?;
            let start_value = first[channel];
            let delta = last[channel] - start_value;
            for_each_chunk::<E>(length, execution, |start, count| {
                let mut index = start;
                let end = start.wrapping_add(count);
                while index < end {
                    let at = usize::try_from(index).map_err(|_| crate::ImageError::Index)?;
                    let mut value = source.read(at)?;
                    if value < 0. {
                        value = 0.;
                    } else if value > 1. {
                        value = 1.;
                    }
                    destination.write(at, start_value + value * delta)?;
                    index = index.wrapping_add(1);
                }
                Ok(())
            })?;
        }
        output.seal_channel(channel as i32)?;
    }
    output.copy_channel_from(3, input, 3)?;
    output.set_region_store(input.region_store());
    output.merge_effects(&[Some(input.clone())])?;
    Ok(Some(output))
}

// uk always invokes its worker, even when all outputs are constant; sn
// independently allocates, seals and aliases channels, then copies alpha.
#[allow(clippy::manual_clamp)]
pub(super) fn palettes<E: 'static>(
    filter: &PointwiseFilter<E>,
    inputs: &[Option<SharedImageMap>],
    geometry: ImageGeometry,
    time: f64,
    execution: ProceduralExecution,
) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
    use super::PointwiseKind;
    let input = inputs
        .first()
        .ok_or(PointwiseError::Index)?
        .as_ref()
        .ok_or(PointwiseError::NullReference)?;
    if filter.kind() == PointwiseKind::WizardFiveColor {
        let output = SharedImageMap::new(geometry);
        let mut colors = [[0.0f32; 3]; 5];
        for (index, color) in colors.iter_mut().enumerate() {
            *color = filter
                .parameter(index as i32)?
                .effective_color_at(time)?
                .components()
                .map(|v| v as f32);
        }
        let source = input.channel(0, false)?;
        let mut destinations = [None, None, None];
        for channel in 0..3 {
            let value = colors[0][channel];
            if colors[1..].iter().all(|color| color[channel] == value) {
                output.set_constant(channel as i32, value)?;
            } else {
                destinations[channel] =
                    Some(output.destination_count(channel as i32, source.len())?);
            }
        }
        let length = i32::try_from(source.len()).map_err(|_| crate::ImageError::Overflow)?;
        for_each_chunk::<E>(length, execution, |start, count| {
            let mut index = start;
            let end = start.wrapping_add(count);
            while index < end {
                let at = usize::try_from(index).map_err(|_| crate::ImageError::Index)?;
                let mut value = source.read(at)?;
                if value < 0. {
                    value = 0.;
                } else if value > 1. {
                    value = 1.;
                }
                let scaled = value * 4.;
                let mut segment = crate::framework_cast::integer(f64::from(scaled));
                if segment == 4 {
                    segment -= 1;
                }
                let segment = usize::try_from(segment)
                    .ok()
                    .filter(|&v| v < 4)
                    .ok_or(PointwiseError::Index)?;
                let weight = scaled - segment as f32;
                for channel in 0..3 {
                    if let Some(destination) = &destinations[channel] {
                        destination.write(
                            at,
                            colors[segment][channel]
                                + weight
                                    * (colors[segment + 1][channel] - colors[segment][channel]),
                        )?;
                    }
                }
                index = index.wrapping_add(1);
            }
            Ok(())
        })?;
        output.set_region_store(input.region_store());
        output.merge_effects(&[Some(input.clone())])?;
        return Ok(Some(output));
    }
    let mut colors = [[0.0f32; 3]; 3];
    for (index, color) in colors.iter_mut().enumerate() {
        *color = filter
            .parameter(index as i32)?
            .effective_color_at(time)?
            .components()
            .map(|v| v as f32);
    }
    let source = input.channel(0, false)?;
    let output = SharedImageMap::new(geometry);
    let midpoint = filter.parameter(3)?.effective_value_at(time)? as f32;
    let left_inverse = if midpoint == 0. { 0. } else { 1. / midpoint };
    let right_inverse = if midpoint == 1. {
        0.
    } else {
        1. / (1. - midpoint)
    };
    let length = i32::try_from(source.len()).map_err(|_| crate::ImageError::Overflow)?;
    for channel in 0..3 {
        let previous = (0..channel).find(|&i| colors.iter().all(|c| c[i] == c[channel]));
        if let Some(previous) = previous {
            let buffer = output.channel(previous as i32, false)?;
            output.set_buffer(channel as i32, Some(&buffer))?;
        } else if colors[0][channel] == colors[1][channel]
            && colors[0][channel] == colors[2][channel]
        {
            output.set_constant(channel as i32, colors[0][channel])?;
        } else {
            let destination = output.destination_count(channel as i32, source.len())?;
            let left = colors[0][channel];
            let right = colors[1][channel];
            let middle = colors[2][channel];
            let lower = (middle - left) * left_inverse;
            let upper = (right - middle) * right_inverse;
            for_each_chunk::<E>(length, execution, |start, count| {
                let mut index = start;
                let end = start.wrapping_add(count);
                while index < end {
                    let at = usize::try_from(index).map_err(|_| crate::ImageError::Index)?;
                    let value = source.read(at)?;
                    let result = if value < midpoint {
                        if value < 0. {
                            left
                        } else {
                            left + value * lower
                        }
                    } else if value > 1. {
                        middle + (1. - midpoint) * upper
                    } else {
                        middle + (value - midpoint) * upper
                    };
                    destination.write(at, result)?;
                    index = index.wrapping_add(1);
                }
                Ok(())
            })?;
        }
        output.seal_channel(channel as i32)?;
    }
    output.copy_channel_from(3, input, 3)?;
    output.set_region_store(input.region_store());
    output.merge_effects(&[Some(input.clone())])?;
    Ok(Some(output))
}

#[cfg(test)]
#[path = "wizard_color_test.rs"]
mod tests;
