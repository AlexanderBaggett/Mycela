//! ha's retained Equalize body. The existing ki adaptive blur owns blur state,
//! storage, planner, worker scheduling and required-memory effects.
use super::blur::{AdaptiveSettings, adaptive};
use super::numeric::{add, div, sub};
use super::{
    PointwiseError, PointwiseFilter, PointwiseRenderEffects, SharedImageMap, check_render_cancelled,
};
use crate::procedural_schedule::for_each_chunk;
use crate::{FloatBuffer, ImageError, ImageGeometry, ProceduralExecution};

// r7.c(Single[]): ordered binary32 accumulation of the actual stored array,
// including its scalar representation; not a logical-pixel-area weighted mean.
pub(super) fn mean<E>(
    values: &FloatBuffer,
    cancelled: &dyn Fn() -> bool,
) -> Result<f32, PointwiseError<E>> {
    let mut total = 0.0;
    for start in (0..values.len()).step_by(1024) {
        check_render_cancelled(cancelled)?;
        let end = (start + 1024).min(values.len());
        for index in start..end {
            total = add(total, values.read(index)?);
        }
    }
    Ok(div(total, values.len() as f32))
}
fn image<E>(
    input: &SharedImageMap,
    radius: f64,
    factor: f64,
    seamless: bool,
    execution: ProceduralExecution,
    effects: &mut PointwiseRenderEffects<'_, E>,
) -> Result<SharedImageMap, PointwiseError<E>> {
    // ha creates ai5 from the source dimensions, irrespective of caller context.
    // Its ordinary two-argument context has zero origin.
    let geometry = input.geometry();
    let blurred = adaptive(
        input,
        ImageGeometry::new(geometry.width(), geometry.height()),
        AdaptiveSettings {
            horizontal: true,
            vertical: true,
            radius,
            factor,
            periodic: seamless,
            all_channels: false,
            force_expansion: false,
        },
        execution,
        effects,
    )?;
    if blurred.same_reference(input) {
        let output = SharedImageMap::new(ImageGeometry::new(geometry.width(), geometry.height()));
        let mut channel = 0;
        while channel < input.channel_count()? as i32 {
            let value = mean(&input.channel(channel, false)?, &|| false)?;
            output.set_constant(channel, value)?;
            channel = channel.wrapping_add(1);
        }
        Ok(output)
    } else {
        let mut channel = 0;
        while channel < input.channel_count()? as i32 {
            // Source obtains original then destination before calculating mean.
            let original = input.channel(channel, false)?;
            let destination = blurred.channel(channel, false)?;
            let center = mean(&original, &|| false)?;
            for_each_chunk::<E>(destination.len() as i32, execution, |start, count| {
                let end = start.wrapping_add(count);
                let mut index = start;
                while index < end {
                    let at = usize::try_from(index).map_err(|_| ImageError::Index)?;
                    // Preserve subtraction then addition and source read order.
                    let difference = sub(center, destination.read(at)?);
                    let value = add(difference, original.read(at)?);
                    destination.write(at, value)?;
                    index = index.wrapping_add(1);
                }
                Ok(())
            })?;
            // ha has no extra seal, scalar collapse or cross-channel alias skip.
            channel = channel.wrapping_add(1);
        }
        Ok(blurred)
    }
}
pub(super) fn retained<E: 'static>(
    filter: &PointwiseFilter<E>,
    inputs: &[Option<SharedImageMap>],
    _geometry: ImageGeometry,
    time: f64,
    execution: ProceduralExecution,
    effects: &mut PointwiseRenderEffects<'_, E>,
) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
    let input = inputs.first().ok_or(PointwiseError::Index)?;
    let radius = filter.parameter(0)?;
    if radius.effective_value_at(time)? == 1.0 {
        return Ok(input.clone());
    }
    // The protected body reads effective Radius again before Factor/null input.
    let value = radius.effective_value_at(time)?;
    let factor = filter.factor();
    let source = input.as_ref().ok_or(PointwiseError::NullReference)?;
    let output = image(source, value, factor, true, execution, effects)?;
    if !output.same_reference(source) {
        output.copy_auxiliary(source)?;
    }
    Ok(Some(output))
}
impl<E> PointwiseFilter<E> {
    /// Actual static ha.g. Unlike protected rendering, this has no Radius==1
    /// shortcut and does not copy RegionsMap or named effect-map tails.
    pub fn equalize_image(
        input: Option<&SharedImageMap>,
        radius: f64,
        factor: f64,
        seamless: bool,
        execution: ProceduralExecution,
        effects: &mut PointwiseRenderEffects<'_, E>,
    ) -> Result<SharedImageMap, PointwiseError<E>> {
        image(
            input.ok_or(PointwiseError::NullReference)?,
            radius,
            factor,
            seamless,
            execution,
            effects,
        )
    }
}
#[cfg(test)]
#[path = "equalize_test.rs"]
mod tests;
