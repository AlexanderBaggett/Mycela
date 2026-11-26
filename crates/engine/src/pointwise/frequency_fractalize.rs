//! Actual aoa/as protected supplied-map bodies and workers. All operands come
//! from genuine retained parameters, caller maps and current map storage.
use super::numeric::{add, div, mul, sub};
use super::{PointwiseError, PointwiseFilter, PointwiseKind, SharedImageMap};
use crate::procedural_schedule::{for_each_chunk, for_each_row_chunk};
use crate::{
    FloatBuffer, ImageError, ImageGeometry, ProceduralExecution,
    framework_cast::integer,
    image_map::{read_pixel, write_pixel},
};
fn at(value: i32) -> Result<usize, ImageError> {
    usize::try_from(value).map_err(|_| ImageError::Index)
}
fn read(values: &FloatBuffer, ix: i32) -> Result<f32, ImageError> {
    values.read(at(ix)?)
}
fn write(values: &FloatBuffer, ix: i32, value: f32) -> Result<(), ImageError> {
    values.write(at(ix)?, value)
}
fn remainder(value: f32, divisor: f32) -> f32 {
    value % divisor
}
// The source two ordered comparisons accept NaN in this identity scan.
// RangeInclusive::contains would reject it and change the returned map identity.
#[allow(clippy::manual_range_contains)]
fn frequency<E: 'static>(
    filter: &PointwiseFilter<E>,
    input: Option<&SharedImageMap>,
    geometry: ImageGeometry,
    time: f64,
    execution: ProceduralExecution,
) -> Result<SharedImageMap, PointwiseError<E>> {
    // aoa reads both numbers and selected text before dereferencing its input.
    let rate = filter.parameter(0)?.effective_value_at(time)? as f32;
    let mut phase = filter.parameter(1)?.effective_value_at(time)? as f32;
    let parameter = filter.parameter(2)?;
    let ix = parameter.effective_index_at(time)?;
    let modifier = parameter
        .options()?
        .entries()
        .get(at(ix)?)
        .cloned()
        .ok_or(PointwiseError::Index)?;
    let is = |value: &str| {
        modifier
            .as_ref()
            .is_some_and(|v| v.iter().copied().eq(value.encode_utf16()))
    };
    let mode = if is("None") {
        0
    } else if is("Saw Tooth") {
        1
    } else if is("Sinusoid") {
        2
    } else {
        -1
    };
    let source = input.ok_or(PointwiseError::NullReference)?;
    if rate == 0.5 && phase == 0. && mode == 1 {
        let mut valid = true;
        let mut channel = 0;
        while channel < source.channel_count()? as i32 {
            let values = source.channel(channel, false)?;
            let values = values.shared_values();
            for &value in values.iter() {
                if value < 0. || value > 1. {
                    valid = false;
                    break;
                }
            }
            if !valid {
                break;
            }
            channel += 1;
        }
        if valid {
            return Ok(source.clone());
        }
    }
    let output = SharedImageMap::new(geometry);
    if f64::from(phase) < 0. {
        phase = add(phase, 1.);
    }
    let mut channel = 0;
    while channel < source.channel_count()? as i32 {
        let values = source.channel(channel, false)?;
        if values.len() != 1 || values.read(0)? != 0. {
            let destination = if values.len() == 1 || rate == 0. {
                output.channel(channel, false)?
            } else {
                output.materialize(channel)?
            };
            if (0..=2).contains(&mode) {
                let shifted = add(phase, 0.5);
                let values = values.shared_values();
                destination.with_mut_slice(|pixels| {
                    for_each_chunk::<E>(pixels.len() as i32, execution, |start, count| {
                        let mut ix = start;
                        let end = start.wrapping_add(count);
                        while ix < end {
                            let value = read_pixel(&values, ix)?;
                            let transformed = if mode == 2 {
                                add(
                                    div(
                                        ((f64::from(add(mul(value, rate), shifted))
                                            * (std::f64::consts::PI * 2.))
                                            .cos()) as f32,
                                        2.,
                                    ),
                                    0.5,
                                )
                            } else {
                                let mut v = remainder(
                                    add(mul(value, rate), phase),
                                    if mode == 0 { 1.00001_f32 } else { 1. },
                                );
                                if v < 0. {
                                    v = add(v, 1.);
                                }
                                if mode == 1 {
                                    if v > 0.5 {
                                        v = sub(1., v);
                                    }
                                    v = mul(v, 2.);
                                }
                                v
                            };
                            write_pixel(pixels, ix, transformed)?;
                            ix = ix.wrapping_add(1);
                        }
                        Ok(())
                    })
                })?;
            }
            output.seal_channel(channel)?;
        }
        channel += 1;
    }
    // aoa n2 copies named maps, not RegionsMap.
    output.merge_effects(&[Some(source.clone())])?;
    Ok(output)
}
struct Octave<'a> {
    blend: bool,
    width: i32,
    height: i32,
    source_width: i32,
    rotation: i32,
    source_size: [f32; 2],
    scale: f32,
    ratio: f32,
    amplitude: f32,
    source: &'a [f32],
    destination: &'a FloatBuffer,
}
fn wrap(value: f32, extent: f32) -> f32 {
    if value >= extent || value < 0. {
        let v = remainder(value, extent);
        if v < 0. { add(v, extent) } else { v }
    } else {
        value
    }
}
impl Octave<'_> {
    fn run<E>(
        &self,
        start: i32,
        count: i32,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), PointwiseError<E>> {
        let mut row = start;
        let end = start.wrapping_add(count);
        while row < end {
            if row % 32 == 0 {
                super::check_render_cancelled(cancelled)?;
            }
            self.destination.with_mut_slice(|pixels| {
                let target_row = row.wrapping_mul(self.width);
                let (mut x, fixed_x) = if self.rotation == 2 {
                    (row.wrapping_neg() as f32, true)
                } else if self.rotation == 6 {
                    (row as f32, true)
                } else {
                    (0., false)
                };
                let (mut y, fixed_y) = if self.rotation == 0 {
                    (row as f32, true)
                } else if self.rotation == 4 {
                    (row.wrapping_neg() as f32, true)
                } else {
                    (0., false)
                };
                if fixed_x {
                    x = wrap(mul(x, mul(self.scale, self.ratio)), self.source_size[0]);
                }
                if fixed_y {
                    y = wrap(mul(y, mul(self.scale, self.ratio)), self.source_size[1]);
                }
                let mut column = 0;
                while column < self.width {
                    match self.rotation {
                        0 => x = column as f32,
                        1 => {
                            x = mul(column.wrapping_sub(row) as f32, 0.5);
                            y = mul(column.wrapping_add(row) as f32, 0.5);
                        }
                        2 => y = column as f32,
                        3 => {
                            x = mul(column.wrapping_neg().wrapping_sub(row) as f32, 0.5);
                            y = mul(column.wrapping_sub(row) as f32, 0.5);
                        }
                        4 => x = column.wrapping_neg() as f32,
                        5 => {
                            x = mul(column.wrapping_neg().wrapping_add(row) as f32, 0.5);
                            y = mul(column.wrapping_neg().wrapping_sub(row) as f32, 0.5);
                        }
                        6 => y = column.wrapping_neg() as f32,
                        _ => {
                            x = mul(column.wrapping_add(row) as f32, 0.5);
                            y = mul(column.wrapping_neg().wrapping_add(row) as f32, 0.5);
                        }
                    }
                    if !fixed_x {
                        x = wrap(mul(x, mul(self.scale, self.ratio)), self.source_size[0]);
                    }
                    if !fixed_y {
                        y = wrap(mul(y, mul(self.scale, self.ratio)), self.source_size[1]);
                    }
                    let sx = integer(f64::from(x));
                    let sy = integer(f64::from(y));
                    let value = read_pixel(
                        self.source,
                        sx.wrapping_add(sy.wrapping_mul(self.source_width)),
                    )?;
                    let target = column.wrapping_add(target_row);
                    let current = read_pixel(pixels, target)?;
                    let contribution = mul(value, self.amplitude);
                    write_pixel(
                        pixels,
                        target,
                        if self.blend {
                            add(current, contribution)
                        } else if current > contribution {
                            current
                        } else {
                            contribution
                        },
                    )?;
                    column = column.wrapping_add(1);
                }
                Ok::<(), PointwiseError<E>>(())
            })?;
            row = row.wrapping_add(1);
        }
        Ok(())
    }
}
fn transform(rotation: i32, option: i32, iteration: i32) -> i32 {
    if option < 8 {
        rotation.wrapping_add(option) % 8
    } else {
        match option {
            8 => rotation.wrapping_add(iteration) % 8,
            9 => rotation.wrapping_add(iteration / 2) % 8,
            10 => rotation.wrapping_add(iteration / 3) % 8,
            11 => iteration / 2 % 8,
            12 => iteration / 3 % 8,
            _ => rotation,
        }
    }
}
fn normalize<E>(
    original: &FloatBuffer,
    destination: &FloatBuffer,
    execution: ProceduralExecution,
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<f32>, PointwiseError<E>> {
    // The source min/max worker counts original.Length, including real
    // scheduler validation/chunks before its subsequent remapping worker.
    // The existing portable scheduler retains sequential chunk dispatch;
    // native monitor races/tie ordering remain its explicit frontier.
    let mut lo = f32::MAX;
    let mut hi = f32::MIN;
    let mut out_lo = f32::MAX;
    let mut out_hi = f32::MIN;
    for_each_chunk::<E>(original.len() as i32, execution, |start, count| {
        let mut ix = start;
        let end = start.wrapping_add(count);
        while ix < end {
            if ix % 8192 == 0 {
                super::check_render_cancelled(cancelled)?;
            }
            let a = read(original, ix)?;
            if a < lo {
                lo = a;
            }
            if a > hi {
                hi = a;
            }
            let b = read(destination, ix)?;
            if b < out_lo {
                out_lo = b;
            }
            if b > out_hi {
                out_hi = b;
            }
            ix = ix.wrapping_add(1);
        }
        Ok(())
    })?;
    if out_hi == out_lo || hi == lo {
        return Ok(Some(hi));
    }
    let scale = mul(div(1., sub(out_hi, out_lo)), sub(hi, lo));
    for_each_chunk::<E>(destination.len() as i32, execution, |start, count| {
        let mut ix = start;
        let end = start.wrapping_add(count);
        while ix < end {
            if ix % 8192 == 0 {
                super::check_render_cancelled(cancelled)?;
            }
            let value = add(mul(sub(read(destination, ix)?, out_lo), scale), lo);
            write(destination, ix, value)?;
            ix = ix.wrapping_add(1);
        }
        Ok(())
    })?;
    Ok(None)
}
fn fractalize<E: 'static>(
    filter: &PointwiseFilter<E>,
    input: Option<&SharedImageMap>,
    geometry: ImageGeometry,
    time: f64,
    execution: ProceduralExecution,
    cancelled: &dyn Fn() -> bool,
) -> Result<SharedImageMap, PointwiseError<E>> {
    super::check_render_cancelled(cancelled)?;
    let output = SharedImageMap::new(geometry);
    let blend = filter.parameter(2)?.effective_index_at(time)? == 0;
    let mut option = filter.parameter(3)?.effective_index_at(time)?;
    if option >= 5 {
        option = option.wrapping_add(3);
    }
    let iterations = integer(filter.parameter(0)?.effective_value_at(time)?);
    let falloff = filter.parameter(1)?.effective_value_at(time)?;
    let source = input.ok_or(PointwiseError::NullReference)?;
    let mut channel = 0;
    while channel < source.channel_count()? as i32 {
        super::check_render_cancelled(cancelled)?;
        let values = source.channel(channel, false)?;
        if values.len() == 1 {
            output.set_constant(channel, values.read(0)?)?;
        } else {
            let destination = output.materialize(channel)?;
            let power = if falloff >= 0. {
                1. + falloff
            } else {
                1. / (1. - falloff)
            };
            let mut rotation = 0;
            let mut previous = 0;
            let mut scale = 1_f32;
            let mut scale_integer = 1_i32;
            let mut octave_map = source.geometry_copy();
            octave_map.set_buffer(0, Some(&values))?;
            let mut octave_values = octave_map.materialize(0)?;
            let mut ratio = 1_f32;
            let mut iteration: i32 = 0;
            while iteration <= iterations {
                super::check_render_cancelled(cancelled)?;
                if iteration > 0 {
                    rotation = transform(rotation, option, iteration);
                    if rotation % 2 != 0 || previous % 2 != 1 {
                        scale = mul(scale, 2.);
                    }
                    previous = rotation;
                    scale_integer = scale_integer.wrapping_mul(2);
                    if iteration % 3 == 0 {
                        let original = source.geometry();
                        let mut width = integer(
                            (f64::from(original.width()) / (f64::from(scale) / 2.))
                                .round_ties_even(),
                        );
                        let mut height = integer(
                            (f64::from(original.height()) / (f64::from(scale) / 2.))
                                .round_ties_even(),
                        );
                        if blend {
                            if width.wrapping_mul(height) < 1 {
                                break;
                            }
                        } else {
                            width = width.max(12);
                            height = height.max(12);
                        }
                        if octave_map.geometry().area() > width.wrapping_mul(height) {
                            let intermediate = source.geometry_copy();
                            intermediate.set_buffer(0, Some(&values))?;
                            octave_map = super::image_resize::resize(
                                &intermediate,
                                width,
                                height,
                                execution,
                            )?;
                            octave_values = octave_map.channel(0, false)?;
                        }
                        ratio = div(
                            octave_map.geometry().width() as f32,
                            source.geometry().width() as f32,
                        );
                    }
                }
                // The discarded ordinary ai5 constructor only publishes its
                // supplied dimensions; no fabricated host/context is retained.
                let dimensions = octave_map.geometry();
                let amplitude = (1. / f64::from(scale_integer).powf(power)) as f32;
                let source_pixels = octave_values.shared_values();
                let job = Octave {
                    blend,
                    width: geometry.width(),
                    height: geometry.height(),
                    source_width: dimensions.width(),
                    rotation,
                    source_size: [dimensions.width() as f32, dimensions.height() as f32],
                    scale,
                    ratio,
                    amplitude,
                    source: &source_pixels,
                    destination: &destination,
                };
                for_each_row_chunk::<E>(job.height, execution, |start, count| {
                    job.run(start, count, cancelled)
                })?;
                iteration = iteration.wrapping_add(1);
            }
            if blend
                && let Some(value) = normalize::<E>(&values, &destination, execution, cancelled)?
            {
                output.set_constant(channel, value)?;
            }
        }
        output.seal_channel(channel)?;
        channel += 1;
    }
    output.merge_effects(&[Some(source.clone())])?;
    Ok(output)
}
pub(super) fn retained<E: 'static>(
    filter: &PointwiseFilter<E>,
    inputs: &[Option<SharedImageMap>],
    geometry: ImageGeometry,
    time: f64,
    execution: ProceduralExecution,
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
    let input = inputs.first().ok_or(PointwiseError::Index)?.as_ref();
    Ok(Some(if filter.kind() == PointwiseKind::Frequency {
        frequency(filter, input, geometry, time, execution)?
    } else {
        fractalize(filter, input, geometry, time, execution, cancelled)?
    }))
}
#[cfg(test)]
#[path = "frequency_fractalize_test.rs"]
mod tests;
