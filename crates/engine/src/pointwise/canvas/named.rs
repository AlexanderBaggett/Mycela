//! Style blends the paint's named maps as red planes in saved channel order.
use super::{
    blend::{BlendGrid, StyleBlend},
    raster::filled,
    style::Blend,
};
use crate::pointwise::{PointwiseRenderEffects, SharedImageMap, blur};
use crate::{
    FloatBuffer, ImageError, PointwiseError, ProceduralExecution, SharedFloatStore,
    SharedNamedChannel,
};

pub(super) fn blurred<E>(
    source: &SharedImageMap,
    settings: blur::AdaptiveSettings,
    retain_source_maps: bool,
    execution: ProceduralExecution,
    effects: &mut PointwiseRenderEffects<'_, E>,
) -> Result<SharedImageMap, PointwiseError<E>> {
    let geometry = source.geometry();
    let output = if retain_source_maps {
        source.shallow_copy()?
    } else {
        SharedImageMap::new(geometry)
    };
    let mut plane = SharedImageMap::new(geometry);
    for channel in source.named_channels() {
        crate::pointwise::check_render_cancelled(effects.cancel_check())?;
        let channel = channel.ok_or(ImageError::NullReference)?;
        let store = channel.data().ok_or(ImageError::NullReference)?;
        let values = store.data(true)?;
        if values.len() == 1 {
            plane.set_constant(0, values.read(0)?)?;
        } else {
            plane.set_store_reference(0, Some(&store))?;
        }
        plane = blur::adaptive(&plane, geometry, settings, execution, effects)?;
        output.add_named_store(
            Some(channel.name_utf16().to_vec()),
            Some(SharedFloatStore::new(plane.materialize(0)?)?),
        )?;
    }
    Ok(output)
}

pub(super) fn composite<E>(
    grid: &BlendGrid,
    target: &SharedImageMap,
    paint: &SharedImageMap,
    transparency: &[f32],
    settings: &Blend,
    effects: &mut PointwiseRenderEffects<'_, E>,
) -> Result<(), PointwiseError<E>> {
    let source = paint.named_channels();
    if source.is_empty() {
        return Ok(());
    }
    let mut channels = target.named_channels();
    let present = settings.for_named_map(true);
    let new = settings.for_named_map(false);
    let present_blend = StyleBlend::new(&present)?;
    let new_blend = StyleBlend::new(&new)?;

    // Genetica replaces only the red plane between named blends. The other
    // destination planes retain the previous named blend's scratch values.
    let mut destination = filled(transparency.len(), [0.; 4])?;
    let mut brush = filled(transparency.len(), [0.; 4])?;
    for (index, source) in source.into_iter().enumerate() {
        crate::pointwise::check_render_cancelled(effects.cancel_check())?;
        let source = source.ok_or(ImageError::NullReference)?;
        let (name, settings, blend, mut result) = if let Some(current) = channels.get(index) {
            let current = current.as_ref().ok_or(ImageError::NullReference)?;
            (
                current.name_utf16(),
                &present,
                &present_blend,
                read(current, transparency.len())?,
            )
        } else {
            (
                source.name_utf16(),
                &new,
                &new_blend,
                filled(transparency.len(), 0.)?,
            )
        };
        for (index, pixel) in destination.iter_mut().enumerate() {
            pixel[0] = result[index % result.len()];
        }

        let values = read(&source, transparency.len())?;
        for (index, (pixel, &alpha)) in brush.iter_mut().zip(transparency).enumerate() {
            *pixel = [values[index % values.len()], 0., 0., alpha];
        }
        grid.composite(blend, settings, &mut destination, &mut brush, effects)?;

        // The source ImageMap copies a one-sample array into a temporary
        // constant. Blending then touches the temporary red plane only.
        if index < channels.len() && result.len() == 1 {
            continue;
        }
        for (value, pixel) in result.iter_mut().zip(&destination) {
            *value = pixel[0];
        }
        let data = SharedFloatStore::new(FloatBuffer::new(result))?;
        let output = Some(SharedNamedChannel::new(Some(name.to_vec()), Some(data)));
        if index == channels.len() {
            channels.push(output);
        } else {
            channels[index] = output;
        }
    }
    target.clear_named_channels();
    for channel in channels {
        target.add_named_channel(channel.ok_or(ImageError::NullReference)?)?;
    }
    Ok(())
}

fn read(channel: &SharedNamedChannel, length: usize) -> Result<Vec<f32>, ImageError> {
    let values = channel
        .data()
        .ok_or(ImageError::NullReference)?
        .data(true)?;
    if values.len() != 1 && values.len() < length {
        return Err(ImageError::Argument);
    }
    Ok(values.values())
}
