//! ajj orchestration over the genuine shared tz/amh channel/axis workers.
use super::adaptive::{AxisSettings, GuidedChannels, channels, text};
use super::numeric::div;
use super::{PointwiseError, PointwiseFilter, PointwiseRenderEffects, SharedImageMap, max64};
use crate::{ImageGeometry, ProceduralExecution};
#[allow(clippy::too_many_arguments)]
pub(super) fn retained<E: 'static>(
    filter: &PointwiseFilter<E>,
    inputs: &[Option<SharedImageMap>],
    geometry: ImageGeometry,
    time: f64,
    execution: ProceduralExecution,
    effects: &mut PointwiseRenderEffects<'_, E>,
) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
    let original = inputs.first().ok_or(PointwiseError::Index)?.clone();
    let radius_map = inputs.get(1).ok_or(PointwiseError::Index)?.clone();
    let driver = inputs.get(2).ok_or(PointwiseError::Index)?.clone();
    let output = SharedImageMap::new(geometry);
    let radius_map = radius_map.as_ref().ok_or(PointwiseError::NullReference)?;
    let mask = radius_map.channel(0, false)?;
    let ratio = filter.parameter(0)?.effective_value_at(time)? / filter.factor();
    let radius = (if ratio > 1. { 1. } else { ratio }) as f32;
    // Original Math.Max(Double,Double) before conv.r4; Rust f64::max would
    // discard NaNs and does not own this source operand-selection rule.
    let threshold = max64(1E-5, filter.parameter(1)?.effective_value_at(time)?) as f32;
    let reciprocal = div(1., threshold);
    let present = filter.node().slot(1)?.node().is_some();
    let seamless = text(filter, 2, time, "Yes")?;
    let driver_present = filter.node().slot(2)?.node().is_some();
    let driver = driver
        .as_ref()
        .ok_or(PointwiseError::NullReference)?
        .channel(0, false)?;
    let settings = AxisSettings {
        geometry,
        radius,
        present,
        seamless,
    };
    let guided = (driver.len() != 1).then_some(GuidedChannels {
        reciprocal,
        guidance: driver_present.then_some(&driver),
        seal_input: false,
    });
    channels(
        original.as_ref(),
        &output,
        &mask,
        settings,
        execution,
        effects,
        guided,
    )?;
    output.copy_auxiliary(original.as_ref().ok_or(PointwiseError::NullReference)?)?;
    Ok(Some(output))
}
