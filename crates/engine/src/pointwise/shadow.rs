//! ar7 Shadow protected image render and local scale commit.
use super::{PointwiseError, PointwiseFilter, PointwiseRenderOptions, SharedImageMap};
use crate::{ImageGeometry, ProceduralExecution};

pub(super) fn retained<E: 'static>(
    filter: &PointwiseFilter<E>,
    inputs: &[Option<SharedImageMap>],
    geometry: ImageGeometry,
    time: f64,
    execution: ProceduralExecution,
    options: PointwiseRenderOptions,
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
    let input = inputs
        .first()
        .ok_or(PointwiseError::Index)?
        .as_ref()
        .ok_or(PointwiseError::NullReference)?;
    let source = input.channel(0, false)?;
    let height_factor = filter.factor();
    let mut height = (filter.parameter(0)?.effective_value_at(time)? / height_factor) as f32;
    let skip = filter.parameter(3)?;
    let index = skip.effective_index_at(time)?;
    let options_root = skip.options()?;
    let choices = options_root.entries();
    let selected = choices
        .get(usize::try_from(index).map_err(|_| PointwiseError::Index)?)
        .ok_or(PointwiseError::Index)?;
    let skip_color = selected
        .as_ref()
        .is_some_and(|s| s.as_slice() == [89u16, 101, 115]);
    let light = filter.parameter(1)?.custom_object()?;
    if light.as_ref().is_some_and(|o| !o.is_light_position()) {
        return Err(PointwiseError::InvalidCast);
    }
    let shadow_factor = filter.factor();
    let strength = (100.0
        - super::point_light::max_clr(
            0.0,
            super::point_light::min_clr(
                100.0,
                filter.parameter(2)?.effective_value_at(time)? / shadow_factor,
            ),
        )) as f32;
    if options.is_color_only() && skip_color {
        height = 0.0;
    }
    let light = light.as_ref().ok_or(PointwiseError::NullReference)?;
    let result = super::point_light::shadow(
        &source,
        geometry,
        height,
        light.light_position()?,
        strength,
        execution,
        cancelled,
    )?;
    result.copy_auxiliary(input)?;
    Ok(Some(result))
}

pub(super) fn commit_scale<E: 'static>(
    filter: &PointwiseFilter<E>,
    factor: f64,
) -> Result<(), PointwiseError<E>> {
    commit_height_scale(filter, factor)?;
    let strength = filter.parameter(2)?;
    if super::parameter_ops::is_animated(|| strength.retained_animation())? {
        super::point_light::scale_point_light_curve(&strength, factor)?;
    } else {
        let value = strength.effective_value_at(0.0)? / factor;
        strength.set_number(super::point_light::max_clr(
            0.0,
            super::point_light::min_clr(100.0, value),
        ))?;
    }
    Ok(())
}

pub(super) fn commit_height_scale<E: 'static>(
    filter: &PointwiseFilter<E>,
    factor: f64,
) -> Result<(), PointwiseError<E>> {
    let height = filter.parameter(0)?;
    if super::parameter_ops::is_animated(|| height.retained_animation())? {
        super::point_light::scale_point_light_curve(&height, factor)?;
    } else {
        height.set_number(height.effective_value_at(0.0)? / factor)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "shadow_test.rs"]
mod tests;
