//! Actual aoa/as protected supplied-map bodies and workers. All operands come
//! from genuine retained parameters, caller maps and current map storage.
use super::numeric::{add, div, mul, sub};
use super::{PointwiseError, PointwiseFilter, PointwiseKind, SharedImageMap};
use crate::procedural_schedule::{for_each_chunk, for_each_dense_band, for_each_row_chunk};
use crate::{
    FloatBuffer, ImageError, ImageGeometry, ProceduralExecution,
    framework_cast::integer,
    image_map::{read_pixel, write_pixel},
};
fn at(value: i32) -> Result<usize, ImageError> {
    usize::try_from(value).map_err(|_| ImageError::Index)
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
    amplitude: f32,
    source: &'a [f32],
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
    fn apply<E>(
        &self,
        destination: &FloatBuffer,
        execution: ProceduralExecution,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), PointwiseError<E>> {
        let width = self.width as usize;
        destination.with_mut_slice(|pixels| {
            let columns = if self.height > 0 && (1..=pixels.len()).contains(&width) {
                self.aligned_columns()?
            } else {
                None
            };
            let columns = columns.as_deref();
            let split_work = matches!(
                execution,
                ProceduralExecution::WorkerPool {
                    processor_count: 2..
                }
            ) && self.height > 1
                && self.width > 0
                && self.width.checked_mul(self.height).map(|n| n as usize) == Some(pixels.len())
                && pixels.len() >= 512 * 512;
            if split_work {
                // Check cancellation on the calling thread before dispatching each wave.
                // Workers only share the immutable samples and own disjoint rows.
                let rows_per_wave = (1024 * 1024 / width).max(1);
                for (wave, band) in pixels.chunks_mut(rows_per_wave * width).enumerate() {
                    super::check_render_cancelled(cancelled)?;
                    let first_row = wave * rows_per_wave;
                    for_each_dense_band(band, width, execution, |start, band| {
                        for (offset, row) in band.chunks_exact_mut(width).enumerate() {
                            self.render_row((first_row + start + offset) as i32, row, columns)?;
                        }
                        Ok(())
                    })?;
                }
                return Ok(());
            }
            for_each_row_chunk::<E>(self.height, execution, |start, count| {
                let end = start.wrapping_add(count);
                for row in start..end {
                    if row % 32 == 0 {
                        super::check_render_cancelled(cancelled)?;
                    }
                    if self.width > 0 {
                        let offset = at(row.wrapping_mul(self.width))?;
                        let pixels = pixels
                            .get_mut(offset..offset + width)
                            .ok_or(ImageError::Index)?;
                        self.render_row(row, pixels, columns)?;
                    }
                }
                Ok(())
            })
        })
    }

    fn coordinate(&self, position: f32, axis: usize) -> i32 {
        integer(f64::from(wrap(
            mul(position, self.scale),
            self.source_size[axis],
        )))
    }

    fn aligned_columns(&self) -> Result<Option<Vec<i32>>, ImageError> {
        let axis = match self.rotation {
            0 | 4 => 0,
            2 | 6 => 1,
            _ => return Ok(None),
        };
        let mut columns = Vec::new();
        columns
            .try_reserve_exact(self.width as usize)
            .map_err(|_| ImageError::Allocation)?;
        for column in 0..self.width {
            let column = if self.rotation >= 4 {
                column.wrapping_neg()
            } else {
                column
            };
            columns.push(self.coordinate(column as f32, axis));
        }
        Ok(Some(columns))
    }

    fn render_row(
        &self,
        row: i32,
        pixels: &mut [f32],
        columns: Option<&[i32]>,
    ) -> Result<(), ImageError> {
        if let Some(columns) = columns {
            let axis = if self.rotation % 4 == 0 { 1 } else { 0 };
            let row = if matches!(self.rotation, 2 | 4) {
                row.wrapping_neg()
            } else {
                row
            };
            let fixed = self.coordinate(row as f32, axis);
            let (base, stride) = if axis == 1 {
                (fixed.wrapping_mul(self.source_width), 1)
            } else {
                (fixed, self.source_width)
            };
            for (pixel, &column) in pixels.iter_mut().zip(columns) {
                self.accumulate(pixel, base.wrapping_add(column.wrapping_mul(stride)))?;
            }
        } else {
            for (column, pixel) in pixels.iter_mut().enumerate() {
                let column = column as i32;
                let (x, y) = match self.rotation {
                    1 => (column.wrapping_sub(row), column.wrapping_add(row)),
                    3 => (
                        column.wrapping_neg().wrapping_sub(row),
                        column.wrapping_sub(row),
                    ),
                    5 => (
                        column.wrapping_neg().wrapping_add(row),
                        column.wrapping_neg().wrapping_sub(row),
                    ),
                    _ => (
                        column.wrapping_add(row),
                        column.wrapping_neg().wrapping_add(row),
                    ),
                };
                let sx = self.coordinate(mul(x as f32, 0.5), 0);
                let sy = self.coordinate(mul(y as f32, 0.5), 1);
                self.accumulate(pixel, sx.wrapping_add(sy.wrapping_mul(self.source_width)))?;
            }
        }
        Ok(())
    }

    fn accumulate(&self, target: &mut f32, index: i32) -> Result<(), ImageError> {
        let contribution = mul(read_pixel(self.source, index)?, self.amplitude);
        *target = if self.blend {
            add(*target, contribution)
        } else if *target > contribution {
            *target
        } else {
            contribution
        };
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
    let original = original.shared_values();
    let samples = destination.shared_values();
    for_each_chunk::<E>(original.len() as i32, execution, |start, count| {
        let mut ix = start;
        let end = start.wrapping_add(count);
        while ix < end {
            if ix % 8192 == 0 {
                super::check_render_cancelled(cancelled)?;
            }
            let a = read_pixel(&original, ix)?;
            if a < lo {
                lo = a;
            }
            if a > hi {
                hi = a;
            }
            let b = read_pixel(&samples, ix)?;
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
    drop(samples);
    let scale = mul(div(1., sub(out_hi, out_lo)), sub(hi, lo));
    destination.with_mut_slice(|pixels| {
        for_each_chunk::<E>(pixels.len() as i32, execution, |start, count| {
            let mut ix = start;
            let end = start.wrapping_add(count);
            while ix < end {
                if ix % 8192 == 0 {
                    super::check_render_cancelled(cancelled)?;
                }
                let value = add(mul(sub(read_pixel(pixels, ix)?, out_lo), scale), lo);
                write_pixel(pixels, ix, value)?;
                ix = ix.wrapping_add(1);
            }
            Ok(())
        })
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
                    scale: mul(scale, ratio),
                    amplitude,
                    source: &source_pixels,
                };
                job.apply(&destination, execution, cancelled)?;
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
