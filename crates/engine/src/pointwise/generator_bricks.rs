//! uq's retained Density/Factor/scale authority. Source uq.cs179–254 and
//! qualified 06001af6..1af9; local ok.py/s6 order is in generator.rs.
use super::*;
use crate::procedural_bricks::{BricksInputSource, render_bricks_with_inputs};
use crate::{ImageGeometry, ProceduralExecution};
use GradientGeneratorParameterKind::{
    BlockDensity, BorderWidth, Location, Orientation, Shifts, Size, SlideVariation, WidthVariation,
};

impl<E: 'static> GradientGenerator<E> {
    /// uq.g(scale): actual inactive raw ±1 Size axes, not normalized extents.
    /// Ancestor/APV providers are absent from this unbound retained authority.
    pub(super) fn bricks_scale_predicate(&self, factor: f64) -> Result<bool, PointwiseError<E>> {
        if factor.partial_cmp(&1.0) != Some(std::cmp::Ordering::Less) {
            return Ok(false);
        }
        let size = self.parameter(Size);
        if size.is_animated()? {
            return Ok(false);
        }
        let x = size.pair()?[0];
        if x.abs() != 1.0 {
            return Ok(false);
        }
        Ok(size.pair()?[1].abs() == 1.0)
    }
    /// uq.g(out4,factor,time). Distinct local outputs retain the source's
    /// Density X, Density Y, Border, Shifts read and arithmetic order.
    pub(super) fn bricks_resolved_values(
        &self,
        factor: f64,
        time: f64,
    ) -> Result<[f64; 4], PointwiseError<E>> {
        let density = self.parameter(BlockDensity);
        let mut values = [
            density.raw_value_at(0, time)?,
            density.raw_value_at(1, time)?,
            self.parameter(BorderWidth).effective_value_at(time)?,
            self.parameter(Shifts).effective_value_at(time)?,
        ];
        if self.bricks_scale_predicate(factor)? && (values[0] > 1.0 || values[1] > 1.0) {
            values[0] = max64(
                round_decimal_places(values[0] * factor, 0)
                    .map_err(|_| ImageError::ArgumentOutOfRange)?,
                1.0,
            );
            values[1] = max64(
                round_decimal_places(values[1] * factor, 0)
                    .map_err(|_| ImageError::ArgumentOutOfRange)?,
                1.0,
            );
            values[2] /= factor;
            values[3] = round_decimal_places(values[3] * factor, 0)
                .map_err(|_| ImageError::ArgumentOutOfRange)?;
        }
        Ok(values)
    }
    /// Actual local uq.gb, after ok.py has committed both Size extents. Flags
    /// are read from the real lazy Density owner at EACH source getter site;
    /// callbacks may replace that owner or fail after an earlier commit.
    pub(in crate::pointwise) fn commit_bricks_scale(
        &self,
        factor: f64,
    ) -> Result<(), PointwiseError<E>> {
        if !self.bricks_scale_predicate(factor)? {
            return Ok(());
        }
        let density = self.parameter(BlockDensity);
        if !(density.extent(0)? > 1.0 || density.extent(1)? > 1.0) {
            return Ok(());
        }
        let values = self.bricks_resolved_values(factor, 0.0)?;
        for (axis, value) in values[..2].iter().copied().enumerate() {
            let flag = density
                .animation()?
                .flags()
                .ok_or(AnimationError::NullReference)?
                .value(axis as i32)
                .map_err(|_| AnimationError::<E>::Index)?;
            if flag {
                scale_curve(&density, axis, factor as f32)?;
            } else {
                density.set_axis(axis, value)?;
            }
        }
        let border = self.parameter(BorderWidth);
        if border.is_animated()? {
            scale_curve(&border, 0, (1.0 / factor) as f32)?;
        } else {
            border.set_number(values[2])?;
        }
        let shifts = self.parameter(Shifts);
        if shifts.is_animated()? {
            scale_curve(&shifts, 0, factor as f32)?;
        } else {
            shifts.set_number(values[3])?;
        }
        Ok(())
    }
    pub(super) fn render_bricks_retained(
        &self,
        width: i32,
        height: i32,
        time: f64,
        execution: ProceduralExecution,
    ) -> Result<SharedImageMap, PointwiseError<E>> {
        render_bricks_with_inputs(
            ImageGeometry::new(width, height),
            execution,
            &mut Retained { owner: self, time },
        )
    }
}
fn scale_curve<E: 'static>(
    parameter: &GradientGeneratorParameter<E>,
    axis: usize,
    factor: f32,
) -> Result<(), PointwiseError<E>> {
    let curve = parameter
        .animation()?
        .curves()
        .ok_or(AnimationError::NullReference)?
        .value(axis as i32)?
        .ok_or(AnimationError::NullReference)?;
    let minimum = parameter.limits()?.snapshot().minimum.to_framework_f32();
    let maximum = parameter.limits()?.snapshot().maximum.to_framework_f32();
    let increment = parameter.limits()?.snapshot().increment.to_framework_f32();
    curve
        .scale_bounds(factor, minimum, maximum, increment)
        .map_err(AnimationError::<E>::Bounds)?;
    Ok(())
}
struct Retained<'a, E> {
    owner: &'a GradientGenerator<E>,
    time: f64,
}
impl<E: 'static> BricksInputSource<E> for Retained<'_, E> {
    fn resolved_values(&mut self) -> Result<[f64; 4], PointwiseError<E>> {
        self.owner
            .bricks_resolved_values(self.owner.factor(), self.time)
    }
    fn is_horizontal(&mut self) -> Result<bool, PointwiseError<E>> {
        let text = self
            .owner
            .parameter(Orientation)
            .effective_text_at(self.time)?;
        Ok(text
            .as_deref()
            .is_some_and(|v| v.iter().copied().eq("Horizontal".encode_utf16())))
    }
    fn size_axis(&mut self, axis: usize) -> Result<f64, PointwiseError<E>> {
        self.owner.parameter(Size).value_at(axis, self.time)
    }
    fn location_axis(&mut self, axis: usize) -> Result<f64, PointwiseError<E>> {
        self.owner.parameter(Location).raw_value_at(axis, self.time)
    }
    fn width_variation(&mut self) -> Result<f64, PointwiseError<E>> {
        self.owner
            .parameter(WidthVariation)
            .effective_value_at(self.time)
    }
    fn slide_variation(&mut self) -> Result<f64, PointwiseError<E>> {
        self.owner
            .parameter(SlideVariation)
            .effective_value_at(self.time)
    }
}

#[cfg(test)]
#[path = "generator_bricks_test.rs"]
mod tests;
