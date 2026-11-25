//! The retained Bricks 'n Boards compatibility node (awu).
use super::{PointwiseError, PointwiseFilter, PointwiseParameter, SharedImageMap, parameter_ops};
use crate::procedural_bricks::{BricksInputSource, LegacyBrickFill, render_legacy_bricks};
use crate::{AnimationError, ImageGeometry, ProceduralExecution};

pub(super) const FILL_MODES: [&str; 4] =
    ["Plain", "Color", "Horizontal Gradient", "Vertical Gradient"];

pub(super) fn preserves_size<E: 'static>(
    size: &PointwiseParameter<E>,
    scale: f64,
) -> Result<bool, PointwiseError<E>> {
    Ok(scale < 1.
        && size.binding_index() == -1
        && !parameter_ops::is_animated(|| size.retained_animation())?
        && size.pair()?.map(f64::abs) == [1.; 2])
}

pub(super) fn commit_size<E: 'static>(
    filter: &PointwiseFilter<E>,
    scale: f64,
) -> Result<(), PointwiseError<E>> {
    let size = filter.parameter(0)?;
    if !preserves_size(&size, scale)? {
        let extents = [size.extent(0)? * scale, size.extent(1)? * scale];
        let [x, y] = super::polygon::round_scaled_size(extents, scale)?;
        size.set_extent(0, x)?;
        size.set_extent(1, y)?;
    }
    Ok(())
}

fn resolved_values<E: 'static>(
    filter: &PointwiseFilter<E>,
    scale: f64,
    time: f64,
) -> Result<[f64; 4], PointwiseError<E>> {
    let density = filter.parameter(2)?;
    let mut x = density.raw_value_at(0, time)?;
    let mut y = density.raw_value_at(1, time)?;
    let mut border = filter.parameter(3)?.effective_value_at(time)?;
    let mut shifts = filter.parameter(4)?.effective_value_at(time)?;
    if preserves_size(&filter.parameter(0)?, scale)? && (x > 1. || y > 1.) {
        x = (x * scale).round_ties_even().max(1.);
        y = (y * scale).round_ties_even().max(1.);
        border /= scale;
        shifts = (shifts * scale).round_ties_even();
    }
    Ok([x, y, border, shifts])
}

pub(super) fn commit_scale<E: 'static>(
    filter: &PointwiseFilter<E>,
    scale: f64,
) -> Result<(), PointwiseError<E>> {
    if !preserves_size(&filter.parameter(0)?, scale)? {
        return Ok(());
    }
    let density = filter.parameter(2)?;
    if !(density.extent(0)? > 1. || density.extent(1)? > 1.) {
        return Ok(());
    }
    let [x, y, border, shifts] = resolved_values(filter, scale, 0.)?;
    for (axis, value) in [x, y].into_iter().enumerate() {
        if density.axis_is_animated(axis)? {
            density.multiply_axis(axis, scale)?;
        } else {
            density.set_axis(axis, value)?;
        }
    }
    for (index, value, multiplier) in [(3, border, 1. / scale), (4, shifts, scale)] {
        let property = filter.parameter(index)?;
        if parameter_ops::is_animated(|| property.retained_animation())? {
            let limits = property.limits()?.snapshot();
            property
                .animation_curve(0)?
                .scale_bounds(
                    multiplier as f32,
                    limits.minimum.to_framework_f32(),
                    limits.maximum.to_framework_f32(),
                    limits.increment.to_framework_f32(),
                )
                .map_err(AnimationError::Bounds)?;
        } else {
            property.set_number(value)?;
        }
    }
    Ok(())
}

struct BricksProperties<'a, E> {
    filter: &'a PointwiseFilter<E>,
    time: f64,
}

impl<E: 'static> BricksProperties<'_, E> {
    fn selected_text(&self, index: i32) -> Result<Vec<u16>, PointwiseError<E>> {
        let property = self.filter.parameter(index)?;
        let choice = property.effective_index_at(self.time)?;
        property
            .options()?
            .entries()
            .get(choice as usize)
            .cloned()
            .flatten()
            .ok_or(PointwiseError::Index)
    }
}

impl<E: 'static> BricksInputSource<E> for BricksProperties<'_, E> {
    fn resolved_values(&mut self) -> Result<[f64; 4], PointwiseError<E>> {
        resolved_values(self.filter, self.filter.factor(), self.time)
    }

    fn is_horizontal(&mut self) -> Result<bool, PointwiseError<E>> {
        Ok(self
            .selected_text(8)?
            .into_iter()
            .eq("Horizontal".encode_utf16()))
    }

    fn size_axis(&mut self, axis: usize) -> Result<f64, PointwiseError<E>> {
        self.filter.parameter(0)?.value_at(axis, self.time)
    }

    fn location_axis(&mut self, axis: usize) -> Result<f64, PointwiseError<E>> {
        self.filter.parameter(1)?.raw_value_at(axis, self.time)
    }

    fn width_variation(&mut self) -> Result<f64, PointwiseError<E>> {
        self.filter.parameter(5)?.effective_value_at(self.time)
    }

    fn slide_variation(&mut self) -> Result<f64, PointwiseError<E>> {
        self.filter.parameter(6)?.effective_value_at(self.time)
    }
}

pub(super) fn render<E: 'static>(
    filter: &PointwiseFilter<E>,
    images: &[Option<SharedImageMap>],
    geometry: ImageGeometry,
    time: f64,
    execution: ProceduralExecution,
    cancelled: &dyn Fn() -> bool,
) -> Result<SharedImageMap, PointwiseError<E>> {
    super::check_render_cancelled(cancelled)?;
    let color = images.first().ok_or(PointwiseError::Index)?;
    let mut properties = BricksProperties { filter, time };
    let mode = properties.selected_text(7)?;
    let fill = if mode.iter().copied().eq("Color".encode_utf16()) {
        LegacyBrickFill::Color
    } else if mode
        .iter()
        .copied()
        .eq("Horizontal Gradient".encode_utf16())
    {
        LegacyBrickFill::HorizontalGradient
    } else if mode.iter().copied().eq("Vertical Gradient".encode_utf16()) {
        LegacyBrickFill::VerticalGradient
    } else {
        LegacyBrickFill::Plain
    };
    let empty = SharedImageMap::new(geometry);
    render_legacy_bricks(
        geometry,
        execution,
        &mut properties,
        fill,
        color.as_ref().unwrap_or(&empty),
        cancelled,
    )
}
