//! amh/tz retained supplied-map bodies (F219/F220). No output-derived operands.
use super::numeric::{add, div, mul, sub};
use super::{
    PointwiseError, PointwiseFilter, PointwiseKind, PointwiseRenderEffects, PointwiseRenderOptions,
    SharedImageMap,
};
use crate::{FloatBuffer, ImageError, ImageGeometry, ProceduralExecution};
pub(super) fn index(i: i32) -> Result<usize, ImageError> {
    usize::try_from(i).map_err(|_| ImageError::Index)
}
pub(super) fn read(a: &FloatBuffer, i: i32) -> Result<f32, ImageError> {
    a.read(index(i)?)
}
pub(super) fn write(a: &FloatBuffer, i: i32, v: f32) -> Result<(), ImageError> {
    a.write(index(i)?, v)
}
pub(super) fn floats(n: i32) -> Result<FloatBuffer, ImageError> {
    let n = usize::try_from(n).map_err(|_| ImageError::Overflow)?;
    let mut values = Vec::new();
    values
        .try_reserve_exact(n)
        .map_err(|_| ImageError::Allocation)?;
    values.resize(n, 0.);
    Ok(FloatBuffer::new(values))
}
#[allow(clippy::manual_clamp)] // Literal ordered zl.g, unordered values pass through.
pub(super) fn clamp(v: f32) -> f32 {
    if v < 0. {
        0.
    } else if v > 1. {
        1.
    } else {
        v
    }
}
pub(super) fn text<E: 'static>(
    filter: &PointwiseFilter<E>,
    parameter: i32,
    time: f64,
    expected: &str,
) -> Result<bool, PointwiseError<E>> {
    let p = filter.parameter(parameter)?;
    let i = index(p.effective_index_at(time)?)?;
    let values = p.options()?.entries();
    Ok(values
        .get(i)
        .ok_or(PointwiseError::Index)?
        .as_ref()
        .is_some_and(|v| v.iter().copied().eq(expected.encode_utf16())))
}
fn brightness<E>(
    image: &SharedImageMap,
    execution: ProceduralExecution,
) -> Result<FloatBuffer, PointwiseError<E>> {
    let r = image.channel(0, false)?;
    let g = image.channel(1, false)?;
    let b = image.channel(2, false)?;
    let dense = [r.len() > 1, g.len() > 1, b.len() > 1];
    if dense.iter().any(|v| *v) {
        let out = floats(image.geometry().area())?;
        // amh+b count is red.Length, even if another plane is the dense one.
        let count = i32::try_from(r.len()).map_err(|_| ImageError::Overflow)?;
        crate::procedural_schedule::for_each_chunk(count, execution, |start, count| {
            for cell in start..start.wrapping_add(count) {
                let red = read(&r, if dense[0] { cell } else { 0 })?;
                let green = read(&g, if dense[1] { cell } else { 0 })?;
                let blue = read(&b, if dense[2] { cell } else { 0 })?;
                write(&out, cell, mul(add(add(red, green), blue), div(1., 3.)))?;
            }
            Ok(())
        })?;
        Ok(out)
    } else {
        // Enumerable.Average<float>: Double accumulator, selector reads in order.
        let mut sum = 0.;
        for a in [&r, &g, &b] {
            sum = crate::source_float::ordered_add(sum, f64::from(a.read(0)?));
        }
        Ok(FloatBuffer::new(vec![(sum / 3.) as f32]))
    }
}
#[derive(Clone, Copy)]
pub(super) struct AxisSettings {
    pub(super) geometry: ImageGeometry,
    pub(super) radius: f32,
    pub(super) present: bool,
    pub(super) seamless: bool,
}
struct SmartAxis<'a> {
    source: &'a FloatBuffer,
    destination: &'a FloatBuffer,
    mask: &'a FloatBuffer,
    guidance: Option<&'a FloatBuffer>,
    settings: AxisSettings,
    reciprocal: f32,
    vertical: bool,
}
impl SmartAxis<'_> {
    fn execute<E>(&self, execution: ProceduralExecution) -> Result<(), PointwiseError<E>> {
        let s = self.settings;
        let width = s.geometry.width();
        let height = s.geometry.height();
        let area = width.wrapping_mul(height);
        let mask_dense = s.present && self.mask.len() > 1;
        let guide_dense = self.guidance.is_some_and(|v| v.len() > 1);
        let step = if self.vertical { width } else { 1 };
        let extent = if self.vertical { height } else { width };
        let wrap = if self.vertical { area } else { width };
        crate::procedural_schedule::for_each_chunk(area, execution, |start, count| {
            if width == 0 {
                return Err(PointwiseError::DivideByZero);
            }
            let mut row = start / width;
            let mut col = start % width;
            for _ in start..start.wrapping_add(count) {
                if col >= width {
                    col = 0;
                    row = row.wrapping_add(1);
                }
                let cell = col.wrapping_add(row.wrapping_mul(width));
                let mut radius = if s.present {
                    mul(clamp(read(self.mask, 0)?), s.radius)
                } else {
                    s.radius
                };
                if mask_dense {
                    radius = mul(clamp(read(self.mask, cell)?), s.radius);
                }
                radius = add(mul(radius, (width / 3) as f32), 0.5);
                let mut span = crate::framework_cast::integer(f64::from(add(radius, 0.5)));
                let smaller = sub(radius, 0.5);
                let fraction = sub(
                    smaller,
                    crate::framework_cast::integer(f64::from(smaller)) as f32,
                );
                if fraction == 0. {
                    span = span.wrapping_sub(1);
                }
                let center = match self.guidance {
                    None => read(self.source, cell)?,
                    Some(g) => read(g, if guide_dense { cell } else { 0 })?,
                };
                let coordinate = if self.vertical { row } else { col };
                let mut sum = 0.;
                let mut weights = 0.;
                let mut offset = span.wrapping_neg();
                while offset <= span {
                    let mut at = cell.wrapping_add(offset.wrapping_mul(step));
                    let position = coordinate.wrapping_add(offset);
                    let edge = (offset == span.wrapping_neg() || offset == span) && fraction > 0.;
                    if s.seamless {
                        if position < 0 {
                            at = at.wrapping_add(wrap);
                        } else if position >= extent {
                            at = at.wrapping_sub(wrap);
                        }
                    } else if position < 0 || position >= extent {
                        offset = offset.wrapping_add(1);
                        continue;
                    }
                    let guide = match self.guidance {
                        None => read(self.source, at)?,
                        Some(g) => read(g, if guide_dense { at } else { 0 })?,
                    };
                    let mut difference = sub(center, guide);
                    // Actual IL neg, rather than decompiler's 0f-value spelling.
                    if difference < 0. {
                        difference = -difference;
                    }
                    let mut weight = sub(1., mul(difference, self.reciprocal));
                    if weight > 0. {
                        if edge {
                            weight = mul(weight, fraction);
                        }
                        sum = add(sum, mul(read(self.source, at)?, weight));
                        weights = add(weights, weight);
                    }
                    offset = offset.wrapping_add(1);
                }
                write(self.destination, cell, div(sum, weights))?;
                col = col.wrapping_add(1);
            }
            Ok(())
        })
    }
}
#[allow(clippy::too_many_arguments)] // Actual supplied-render entry operands and borrowed effects.
pub(super) fn retained<E: 'static>(
    filter: &PointwiseFilter<E>,
    inputs: &[Option<SharedImageMap>],
    geometry: ImageGeometry,
    time: f64,
    execution: ProceduralExecution,
    options: PointwiseRenderOptions,
    effects: &mut PointwiseRenderEffects<'_, E>,
) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
    if filter.kind() == PointwiseKind::Smear {
        return super::smear::retained(filter, inputs, geometry, time, execution, options);
    }
    let original = inputs.first().cloned().ok_or(PointwiseError::Index)?;
    let mask = inputs.get(1).cloned().ok_or(PointwiseError::Index)?;
    let output = SharedImageMap::new(geometry);
    let mask = mask.as_ref().ok_or(PointwiseError::NullReference)?;
    let mask_values = mask.channel(0, false)?;
    let ratio = filter.parameter(0)?.effective_value_at(time)? / filter.factor();
    let radius = (if ratio > 1. { 1. } else { ratio }) as f32;
    let smart = filter.kind() == PointwiseKind::SmartBlur;
    let reciprocal = if smart {
        div(1., filter.parameter(1)?.effective_value_at(time)? as f32)
    } else {
        0.
    };
    let present = filter.node().slot(1)?.node().is_some();
    let seamless = text(filter, if smart { 3 } else { 1 }, time, "Yes")?;
    if smart && reciprocal.is_infinite() {
        return Ok(Some(
            original
                .as_ref()
                .ok_or(PointwiseError::NullReference)?
                .deep_copy()?,
        ));
    }
    let use_brightness = smart && !text(filter, 2, time, "Multi-Pass")?;
    let guidance = if use_brightness {
        Some(brightness(
            original.as_ref().ok_or(PointwiseError::NullReference)?,
            execution,
        )?)
    } else {
        None
    };
    let settings = AxisSettings {
        geometry,
        radius,
        present,
        seamless,
    };
    channels(
        original.as_ref(),
        &output,
        &mask_values,
        settings,
        execution,
        effects,
        smart.then_some(GuidedChannels {
            reciprocal,
            guidance: guidance.as_ref(),
            seal_input: true,
        }),
    )?;
    mask.seal_channel(0)?;
    output.copy_auxiliary(original.as_ref().ok_or(PointwiseError::NullReference)?)?;
    Ok(Some(output))
}

#[derive(Clone, Copy)]
pub(super) struct GuidedChannels<'a> {
    pub(super) reciprocal: f32,
    pub(super) guidance: Option<&'a FloatBuffer>,
    pub(super) seal_input: bool,
}
// Exact tz.g / amh channel-loop composition. Art uses the same workers but
// its own parameter/slot reads, threshold floor and source-sealing policy.
#[allow(clippy::too_many_arguments)]
pub(super) fn channels<E>(
    original: Option<&SharedImageMap>,
    output: &SharedImageMap,
    mask_values: &FloatBuffer,
    settings: AxisSettings,
    execution: ProceduralExecution,
    effects: &mut PointwiseRenderEffects<'_, E>,
    guided: Option<GuidedChannels<'_>>,
) -> Result<(), PointwiseError<E>> {
    let smart = guided.is_some();
    let guidance = guided.and_then(|g| g.guidance);
    let reciprocal = guided.map_or(0., |g| g.reciprocal);
    let geometry = settings.geometry;
    for channel in 0..4 {
        let original = original.ok_or(PointwiseError::NullReference)?;
        let source = original.channel(channel, false)?;
        if source.len() == 1 {
            output.set_constant(channel, source.read(0)?)?;
        } else {
            let dest = output.materialize(channel)?;
            let scratch = floats(original.geometry().area())?;
            for pass in 0..3 {
                let input = if pass == 0 { &source } else { &dest };
                if smart {
                    SmartAxis {
                        source: input,
                        destination: &scratch,
                        mask: mask_values,
                        guidance,
                        settings,
                        reciprocal,
                        vertical: false,
                    }
                    .execute::<E>(execution)?;
                    SmartAxis {
                        source: &scratch,
                        destination: &dest,
                        mask: mask_values,
                        guidance,
                        settings,
                        reciprocal,
                        vertical: true,
                    }
                    .execute::<E>(execution)?;
                } else {
                    super::adaptive_box::axis::<E>(
                        input,
                        &scratch,
                        mask_values,
                        settings,
                        false,
                        execution,
                    )?;
                    super::adaptive_box::axis::<E>(
                        &scratch,
                        &dest,
                        mask_values,
                        settings,
                        true,
                        execution,
                    )?;
                }
            }
        }
        if guided.is_some_and(|g| g.seal_input) {
            original.seal_channel(channel)?;
        }
        output.seal_channel(channel)?;
        effects.preflight(geometry.width(), 2)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "adaptive_test.rs"]
mod tests;
