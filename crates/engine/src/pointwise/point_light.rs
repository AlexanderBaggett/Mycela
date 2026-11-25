//! Retained Point Light, ambient occlusion, and the shared shadow worker.
use super::blur::{AdaptiveSettings, adaptive};
use super::{PointwiseError, PointwiseRenderEffects, SharedImageMap, check_render_cancelled};
use crate::procedural_schedule::{for_each_chunk, for_each_row_chunk};
use crate::{FloatBuffer, ImageError, ImageGeometry, ProceduralExecution};

/// Already-evaluated arguments of asp.a; n7 evaluates its own properties.
#[derive(Clone, Copy, Debug)]
pub struct PointLightArguments {
    pub x: f64,
    pub y: f64,
    pub height_factor: f64,
    pub shadow_strength: f64,
    pub global_illumination: f64,
    pub ambient_occlusion: f64,
    pub occlusion_radius: f64,
    pub factor: f64,
}

fn at(i: i32) -> Result<usize, ImageError> {
    usize::try_from(i).map_err(|_| ImageError::Index)
}
pub(super) fn max_clr(a: f64, b: f64) -> f64 {
    if a.is_nan() {
        a
    } else if b.is_nan() {
        b
    } else if a > b {
        a
    } else {
        b
    }
}
pub(super) fn min_clr(a: f64, b: f64) -> f64 {
    if a.is_nan() {
        a
    } else if b.is_nan() {
        b
    } else if a < b {
        a
    } else {
        b
    }
}
// The installed IL uses `neg` here (and for the scalar result), despite
// decompiled C# spelling this as subtraction from zero.
pub(super) fn light_direction(x: f64, y: f64) -> (f64, f64, f64) {
    let mut nx = -x;
    let mut ny = y;
    let mut nz = -1.0;
    let length = (nx * nx + ny * ny + nz * nz).sqrt();
    nx /= length;
    ny /= length;
    nz /= length;
    (nx, ny, nz)
}

// ar7.g accumulates the minimum shade from all eligible ray samples.
// The shared scheduler visits chunks in order, keeping the reduction stable.
pub(super) fn shadow<E>(
    height: &FloatBuffer,
    geometry: ImageGeometry,
    z: f32,
    [x, y]: [f64; 2],
    mut strength: f32,
    execution: ProceduralExecution,
    cancelled: &dyn Fn() -> bool,
) -> Result<SharedImageMap, PointwiseError<E>> {
    check_render_cancelled(cancelled)?;
    let map = SharedImageMap::new(ImageGeometry::new(geometry.width(), geometry.height()));
    map.set_constant(0, 1.0)?;
    if height.len() == 1 || z == 0.0 {
        return Ok(map);
    }
    let width = geometry.width();
    let h = geometry.height();
    let dimension = width.max(h);
    let distance = max_clr(x * x + y * y, 1e-5).sqrt();
    let dx = (-x / distance) as f32;
    let dy = (y / distance) as f32;
    let step_height = (1.0 / distance / f64::from(dimension)) as f32;
    if strength == 100.0 {
        strength = 99.99;
    }
    let decay = if strength >= 50.0 {
        1.0 / ((100.0 - strength) / 50.0)
    } else {
        strength / 50.0
    };
    if width <= 0
        || h <= 0
        || !dx.is_finite()
        || !dy.is_finite()
        || !step_height.is_finite()
        || !decay.is_finite()
    {
        return Err(PointwiseError::UnsupportedCapability(
            "ar7 shadow dimensions or nonfinite stepping",
        ));
    }
    let total = width.checked_mul(h).ok_or(ImageError::Overflow)?;
    let dst = map.materialize(0)?;
    for_each_chunk::<E>(total, execution, |start, count| {
        check_render_cancelled(cancelled)?;
        let mut n = start;
        let end = start.wrapping_add(count);
        while n < end {
            if n % 64 == 0 {
                check_render_cancelled(cancelled)?;
            }
            let sx = n % width;
            let sy = n / width;
            let base = height.read(at(n)?)? * z;
            let mut px = sx as f32;
            let mut py = sy as f32;
            let mut misses = 0i32;
            let mut step = 4.0f32;
            let mut shade = 0.0f32;
            let mut previous = 0.0f32;
            let mut steps = 0u32;
            while shade < 1.0 {
                steps = steps.wrapping_add(1);
                if steps.is_multiple_of(64) {
                    check_render_cancelled(cancelled)?;
                }
                px += dx * step;
                py += dy * step;
                if step > 1.0 {
                    step *= 0.5;
                    if step < 1.0 {
                        step = 1.0;
                    }
                }
                if !px.is_finite()
                    || !py.is_finite()
                    || (px as f64).abs() >= 2147483648.0
                    || (py as f64).abs() >= 2147483648.0
                {
                    return Err(PointwiseError::UnsupportedCapability(
                        "ar7 shadow coordinate conversion outside characterized range",
                    ));
                }
                let mut rx = crate::framework_cast::integer((px as f64).round_ties_even());
                let mut ry = crate::framework_cast::integer((py as f64).round_ties_even());
                if rx != sx || ry != sy {
                    let vx = sx.wrapping_sub(rx);
                    let vy = sy.wrapping_sub(ry);
                    let square = vx.wrapping_mul(vx).wrapping_add(vy.wrapping_mul(vy));
                    let rise = step_height * (square as f64).sqrt() as f32;
                    shade = rise * decay;
                    if shade > 1.0 {
                        shade = 1.0;
                    }
                    let current = shade;
                    shade = (shade + previous) * 0.5;
                    previous = current;
                    // These remainders have the same final value as the source
                    // wrap loops for positive dimensions, without enormous loops.
                    rx = rx.rem_euclid(width);
                    ry = ry.rem_euclid(h);
                    let sample_height = base - rise;
                    let index = rx + ry * width;
                    let ix = at(index)?;
                    if sample_height < height.read(ix)? * z + 0.001 {
                        let old = misses;
                        misses = misses.wrapping_add(1);
                        if old >= 10 {
                            break;
                        }
                        continue;
                    }
                    if shade < dst.read(ix)? {
                        dst.write(ix, shade)?;
                    }
                } else {
                    let old = misses;
                    misses = misses.wrapping_add(1);
                    if old >= 10 {
                        break;
                    }
                }
            }
            n = n.wrapping_add(1);
        }
        Ok(())
    })?;
    Ok(map)
}

/// Static asp.a: does not copy RegionsMap or named channels. Geometry is the
/// original ai5 dimensions, and input is the actual retained ImageMap.
pub fn point_light_image<E>(
    input: Option<&SharedImageMap>,
    geometry: ImageGeometry,
    p: PointLightArguments,
    execution: ProceduralExecution,
    effects: &mut PointwiseRenderEffects<'_, E>,
) -> Result<SharedImageMap, PointwiseError<E>> {
    let input = input.ok_or(PointwiseError::NullReference)?;
    let source = input.channel(0, false)?;
    let w = geometry.width();
    let h = geometry.height();
    let output = SharedImageMap::new(ImageGeometry::new(w, h));
    // ImageMap.b(0, source) -> nr(0, source.Length) rejects an empty
    // array before considering the destination geometry. Zero height bypasses
    // nr entirely and reads the scalar destination instead.
    let dst = if p.height_factor == 0.0 {
        output.channel(0, false)?
    } else if source.is_empty() {
        return Err(ImageError::Argument.into());
    } else if source.len() == 1 {
        output.channel(0, false)?
    } else {
        output.materialize(0)?
    };
    let (nx, ny, nz) = light_direction(p.x, p.y);
    let scale_x = f64::from(w) * p.height_factor;
    let scale_y = f64::from(h) * p.height_factor;
    if dst.len() == 1 {
        dst.write(0, (-nz) as f32)?;
    } else {
        for_each_row_chunk::<E>(h, execution, |start, count| {
            let mut row = start;
            let end = start.wrapping_add(count);
            while row < end {
                let base = row.wrapping_mul(w);
                let next = if row != h.wrapping_sub(1) {
                    row.wrapping_add(1).wrapping_mul(w)
                } else {
                    0
                };
                let mut right = f64::from(source.read(at(base)?)?);
                let mut col = w.wrapping_sub(1);
                while col >= 0 {
                    let value = f64::from(source.read(at(col.wrapping_add(base))?)?);
                    let below = f64::from(source.read(at(col.wrapping_add(next))?)?);
                    let gx = (right - value) * scale_x;
                    let gy = (below - value) * scale_y;
                    let normal = (gx * gx + gy * gy + 1.0).sqrt();
                    dst.write(
                        at(col.wrapping_add(base))?,
                        ((nx * gx + ny * gy - nz) / normal) as f32,
                    )?;
                    right = value;
                    col = col.wrapping_sub(1);
                }
                row = row.wrapping_add(1);
            }
            Ok(())
        })?;
    }
    let gi = (p.global_illumination / 100.0) as f32;
    let mut shadow_buffer = None;
    if p.height_factor != 0.0 {
        let strength = (100.0 - p.shadow_strength) as f32;
        if strength < 100.0 && gi < 1.0 {
            let shadow_map = shadow(
                &source,
                geometry,
                p.height_factor as f32,
                [p.x, p.y],
                strength,
                execution,
                effects.cancel_check(),
            )?;
            let values = shadow_map.channel(0, false)?;
            if values.len() != 1 {
                shadow_buffer = Some(values);
            }
        }
    }
    let direct = 1.0f32 - gi;
    for_each_chunk::<E>(dst.len() as i32, execution, |start, count| {
        let mut i = start;
        let end = start.wrapping_add(count);
        while i < end {
            let ix = at(i)?;
            let value = dst.read(ix)?;
            let bright = if value > 0.0 { value } else { 0.0 };
            let soft = value * 0.5 + 0.5;
            let s = match &shadow_buffer {
                Some(a) => a.read(ix)?,
                None => 1.0,
            };
            dst.write(ix, gi * soft + direct * (bright * s))?;
            i = i.wrapping_add(1);
        }
        Ok(())
    })?;
    let radius = p.occlusion_radius / 100.0;
    let occlusion = p.ambient_occlusion / 100.0;
    if p.height_factor != 0.0 && radius != 0.0 && occlusion != 0.0 {
        let blurred = adaptive(
            input,
            geometry,
            AdaptiveSettings {
                horizontal: true,
                vertical: true,
                radius,
                factor: p.factor,
                periodic: true,
                all_channels: false,
                force_expansion: false,
            },
            execution,
            effects,
        )?;
        let array = blurred.channel(0, false)?;
        if !blurred.same_reference(input) && array.len() != 1 {
            let mut gain = (occlusion * 14.0) as f32;
            let illumination = (1.0 + nz * 0.6) as f32;
            let height_scale = min_clr(1.0, p.height_factor.abs() * 5.0) as f32;
            gain *= illumination * height_scale;
            let sign = if p.height_factor >= 0.0 { 1.0 } else { -1.0 };
            for_each_chunk::<E>(array.len() as i32, execution, |start, count| {
                let mut i = start;
                let end = start.wrapping_add(count);
                while i < end {
                    let ix = at(i)?;
                    let delta = (array.read(ix)? - source.read(ix)?) * sign;
                    let value = if delta > 0.0 {
                        let depth = 1.0f32 - (1.0f64 - f64::from(delta * delta)).sqrt() as f32;
                        let light = 1.0f32 - depth * gain;
                        if light > 0.0 { light } else { 0.0 }
                    } else {
                        1.0
                    };
                    array.write(ix, value)?;
                    dst.write(ix, dst.read(ix)? * array.read(ix)?)?;
                    i = i.wrapping_add(1);
                }
                Ok(())
            })?;
        }
    }
    Ok(output)
}

#[cfg(test)]
#[path = "point_light_test.rs"]
mod tests;

/// Protected n7.k8. Slot/property capture is outside this routine; this
/// evaluates current retained values in source order at the actual time.
pub(super) fn retained<E: 'static>(
    filter: &super::PointwiseFilter<E>,
    inputs: &[Option<SharedImageMap>],
    geometry: ImageGeometry,
    time: f64,
    execution: ProceduralExecution,
    options: super::PointwiseRenderOptions,
    effects: &mut PointwiseRenderEffects<'_, E>,
) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
    let source = inputs.first().ok_or(PointwiseError::Index)?.as_ref();
    let skip = filter.parameter(6)?.effective_index_at(time)? == 0;
    let object = filter.parameter(0)?.custom_object()?;
    if object
        .as_ref()
        .is_some_and(|value| !value.is_light_position())
    {
        return Err(PointwiseError::InvalidCast);
    }
    let height_factor = filter.factor();
    let mut height = filter.parameter(1)?.effective_value_at(time)? / height_factor;
    if skip && options.is_color_only() {
        height = 0.0;
    }
    let x = object
        .as_ref()
        .ok_or(PointwiseError::NullReference)?
        .light_position_x()?;
    let y = object
        .as_ref()
        .ok_or(PointwiseError::NullReference)?
        .light_position_y()?;
    let shadow_factor = filter.factor();
    let shadow = max_clr(
        0.0,
        min_clr(
            100.0,
            filter.parameter(2)?.effective_value_at(time)? / shadow_factor,
        ),
    );
    let gi = filter.parameter(3)?.effective_value_at(time)?;
    let ao_factor = filter.factor();
    let first_height = filter.parameter(1)?.effective_value_at(time)? / 1.0;
    let first_scale = min_clr(1.0, first_height.abs() * 5.0);
    let scaled_height = filter.parameter(1)?.effective_value_at(time)? / ao_factor;
    let second_scale = min_clr(1.0, scaled_height.abs() * 5.0);
    let ao_raw = if second_scale == 0.0 {
        0.0
    } else {
        filter.parameter(4)?.effective_value_at(time)? * first_scale / second_scale
    };
    let ao_property = filter.parameter(4)?;
    let ao = if ao_raw <= ao_property.limits()?.snapshot().minimum.to_framework_f64() {
        ao_property.limits()?.snapshot().minimum.to_framework_f64()
    } else if ao_raw >= ao_property.limits()?.snapshot().maximum.to_framework_f64() {
        ao_property.limits()?.snapshot().maximum.to_framework_f64()
    } else {
        ao_raw
    };
    let radius_factor = filter.factor();
    let radius_property = filter.parameter(5)?;
    let radius_raw = radius_property.effective_value_at(time)? / radius_factor;
    let radius = if radius_raw
        <= radius_property
            .limits()?
            .snapshot()
            .minimum
            .to_framework_f64()
    {
        radius_property
            .limits()?
            .snapshot()
            .minimum
            .to_framework_f64()
    } else if radius_raw
        >= radius_property
            .limits()?
            .snapshot()
            .maximum
            .to_framework_f64()
    {
        radius_property
            .limits()?
            .snapshot()
            .maximum
            .to_framework_f64()
    } else {
        radius_raw
    };
    let factor = filter.factor();
    let output = point_light_image(
        source,
        geometry,
        PointLightArguments {
            x,
            y,
            height_factor: height,
            shadow_strength: shadow,
            global_illumination: gi,
            ambient_occlusion: ao,
            occlusion_radius: radius,
            factor,
        },
        execution,
        effects,
    )?;
    output.copy_auxiliary(source.ok_or(PointwiseError::NullReference)?)?;
    Ok(Some(output))
}

/// Source n7.gb local scale commit. Actual property setters and curve bounds
/// remain the owners of normalization, notifications, and failure prefixes.
impl<E: 'static> super::PointwiseFilter<E> {
    pub(super) fn commit_point_light_scale(&self, factor: f64) -> Result<(), PointwiseError<E>> {
        let height = self.parameter(1)?;
        let mut old_scale = 1.0;
        let mut new_scale = 1.0;
        if super::parameter_ops::is_animated(|| height.retained_animation())? {
            scale_point_light_curve(&height, factor)?;
        } else {
            old_scale = min_clr(1.0, height.number().abs() * 5.0);
            height.set_number(height.effective_value_at(0.0)? / factor)?;
            new_scale = min_clr(1.0, height.number().abs() * 5.0);
        }
        if old_scale != new_scale && new_scale != 0.0 {
            let ao = self.parameter(4)?;
            if !super::parameter_ops::is_animated(|| ao.retained_animation())? {
                ao.set_number(ao.number() * old_scale / new_scale)?;
            }
        }
        let radius = self.parameter(5)?;
        if super::parameter_ops::is_animated(|| radius.retained_animation())? {
            scale_point_light_curve(&radius, factor)?;
        } else {
            radius.set_number(min_clr(radius.number() / factor, 100.0))?;
        }
        let shadow = self.parameter(2)?;
        if super::parameter_ops::is_animated(|| shadow.retained_animation())? {
            scale_point_light_curve(&shadow, factor)?;
        } else {
            shadow.set_number(max_clr(
                0.0,
                min_clr(100.0, shadow.effective_value_at(0.0)? / factor),
            ))?;
        }
        Ok(())
    }
}

pub(super) fn scale_point_light_curve<E: 'static>(
    property: &super::PointwiseParameter<E>,
    factor: f64,
) -> Result<(), PointwiseError<E>> {
    let curve = property.animation_curve(0)?;
    let minimum = property.limits()?.snapshot().minimum.to_framework_f32();
    let maximum = property.limits()?.snapshot().maximum.to_framework_f32();
    let increment = property.limits()?.snapshot().increment.to_framework_f32();
    curve
        .scale_bounds((1.0 / factor) as f32, minimum, maximum, increment)
        .map_err(crate::AnimationError::Bounds)?;
    Ok(())
}
