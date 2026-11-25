//! adj.k8 and its exact three separable custom-quickselect pairs (F207–F210).
//! Real slot topology is read at the late source site, independently of bm maps.
use super::numeric::{add, mul, sub};
use super::{PointwiseError, PointwiseFilter, PointwiseRenderEffects, SharedImageMap};
use crate::{FloatBuffer, ImageError, ImageGeometry, ProceduralExecution};

fn ix(value: i32) -> Result<usize, ImageError> {
    usize::try_from(value).map_err(|_| ImageError::Index)
}
fn read(a: &FloatBuffer, at: i32) -> Result<f32, ImageError> {
    a.read(ix(at)?)
}
fn write(a: &FloatBuffer, at: i32, value: f32) -> Result<(), ImageError> {
    a.write(ix(at)?, value)
}
fn sample(a: &[f32], at: i32) -> Result<f32, ImageError> {
    a.get(ix(at)?).copied().ok_or(ImageError::Index)
}
fn store(a: &mut [f32], at: i32, value: f32) -> Result<(), ImageError> {
    *a.get_mut(ix(at)?).ok_or(ImageError::Index)? = value;
    Ok(())
}
fn swap(a: &mut [f32], first: i32, second: i32) -> Result<(), ImageError> {
    let saved = sample(a, first)?;
    let other = sample(a, second)?; // second read precedes either store
    store(a, first, other)?;
    store(a, second, saved)
}
fn partition(a: &mut [f32], low: i32, high: i32, pivot: i32) -> Result<i32, ImageError> {
    let value = sample(a, pivot)?;
    swap(a, pivot, high)?;
    let mut destination = low;
    let mut at = low;
    while at < high {
        // Source bge.un skips equal/greater AND unordered values. No comparer.
        if sample(a, at)? < value {
            swap(a, destination, at)?;
            destination = destination.wrapping_add(1);
        }
        at = at.wrapping_add(1);
    }
    swap(a, high, destination)?;
    Ok(destination)
}
fn select(a: &mut [f32], low: i32, high: i32, selected: i32) -> Result<f32, ImageError> {
    let mut pivot = low.wrapping_add(high) / 2;
    if low < high {
        pivot = partition(a, low, high, pivot)?;
        if pivot > selected {
            select(a, low, pivot.wrapping_sub(1), selected)?;
        } else if pivot < selected {
            select(a, pivot.wrapping_add(1), high, selected)?;
        }
    }
    sample(a, selected)
}
fn median(a: &mut [f32]) -> Result<f32, ImageError> {
    let length = i32::try_from(a.len()).map_err(|_| ImageError::Overflow)?;
    select(a, 0, length.wrapping_sub(1), length / 2)
}
#[allow(clippy::manual_clamp)] // Literal source comparisons preserve unordered/signed-zero behavior.
fn clamp(value: f32) -> f32 {
    if value < 0. {
        0.
    } else if value > 1. {
        1.
    } else {
        value
    }
}
fn scratch(area: i32) -> Result<FloatBuffer, ImageError> {
    let size = usize::try_from(area).map_err(|_| ImageError::Overflow)?;
    let mut values = Vec::new();
    values
        .try_reserve_exact(size)
        .map_err(|_| ImageError::Allocation)?;
    values.resize(size, 0.);
    Ok(FloatBuffer::new(values))
}
fn push(values: &mut Vec<f32>, value: f32) -> Result<(), ImageError> {
    // Framework List<Single> starts empty, grows4 then doubles. Allocation/OOM
    // timing remains qualified separately; every sample is read before Add.
    if values.len() == values.capacity() {
        let next = if values.capacity() == 0 {
            4
        } else {
            values
                .capacity()
                .checked_mul(2)
                .ok_or(ImageError::Allocation)?
        };
        values
            .try_reserve_exact(next - values.len())
            .map_err(|_| ImageError::Allocation)?;
    }
    values.push(value);
    Ok(())
}
struct Pass<'a> {
    source: &'a FloatBuffer,
    destination: &'a FloatBuffer,
    mask: &'a FloatBuffer,
    geometry: ImageGeometry,
    radius: f32,
    mask_present: bool,
    seamless: bool,
    vertical: bool,
}
impl Pass<'_> {
    fn execute<E>(&self, execution: ProceduralExecution) -> Result<(), PointwiseError<E>> {
        let width = self.geometry.width();
        let height = self.geometry.height();
        let dense_mask = self.mask_present && self.mask.len() > 1;
        let area = width.wrapping_mul(height);
        let scale = (width / 3) as f32; // BOTH workers use integer width/3
        crate::procedural_schedule::for_each_chunk(area, execution, |start, count| {
            if width == 0 {
                return Err(PointwiseError::DivideByZero);
            }
            if start == i32::MIN && width == -1 {
                return Err(PointwiseError::Arithmetic);
            }
            let mut row = start / width;
            let mut column = start % width;
            let end = start.wrapping_add(count);
            let mut row_base = row.wrapping_mul(width);
            let mut at = start;
            while at < end {
                if column >= width {
                    column = 0;
                    row = row.wrapping_add(1);
                    row_base = row.wrapping_mul(width);
                }
                let cell = column.wrapping_add(row_base);
                let mut radius = if self.mask_present {
                    mul(clamp(read(self.mask, 0)?), self.radius)
                } else {
                    self.radius
                };
                if dense_mask {
                    radius = mul(clamp(read(self.mask, cell)?), self.radius);
                }
                radius = add(mul(radius, scale), 0.5);
                let mut size = crate::framework_cast::integer(f64::from(add(radius, 0.5)));
                let remainder = sub(
                    sub(radius, 0.5),
                    crate::framework_cast::integer(f64::from(sub(radius, 0.5))) as f32,
                );
                if remainder == 0. {
                    size = size.wrapping_sub(1);
                }
                let mut values = Vec::new(); // fresh real list PER pixel
                let mut offset = size.wrapping_neg();
                while offset <= size {
                    let coordinate = if self.vertical {
                        row.wrapping_add(offset)
                    } else {
                        column.wrapping_add(offset)
                    };
                    let extent = if self.vertical { height } else { width };
                    let stride = if self.vertical { width } else { 1 };
                    let wrap = if self.vertical { area } else { width };
                    let mut source = offset.wrapping_mul(stride).wrapping_add(cell);
                    if self.seamless {
                        if coordinate < 0 {
                            source = source.wrapping_add(wrap);
                        }
                        if coordinate >= extent {
                            source = source.wrapping_sub(wrap);
                        }
                    } else if coordinate < 0 || coordinate >= extent {
                        offset = offset.wrapping_add(1);
                        continue;
                    }
                    let value = read(self.source, source)?;
                    push(&mut values, value)?;
                    offset = offset.wrapping_add(1);
                }
                let mut samples = Vec::new(); // actual independent ToArray
                samples
                    .try_reserve_exact(values.len())
                    .map_err(|_| ImageError::Allocation)?;
                samples.extend_from_slice(&values);
                let value = median(&mut samples)?;
                write(self.destination, cell, value)?; // destination checked LAST
                at = at.wrapping_add(1);
                column = column.wrapping_add(1);
            }
            Ok(())
        })
    }
}
pub(super) fn retained<E: 'static>(
    filter: &PointwiseFilter<E>,
    inputs: &[Option<SharedImageMap>],
    geometry: ImageGeometry,
    time: f64,
    execution: ProceduralExecution,
    effects: &mut PointwiseRenderEffects<'_, E>,
) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
    // g(out0,out1) resolves both slots before allocation or either dereference.
    let original = inputs.first().cloned().ok_or(PointwiseError::Index)?;
    let mask = inputs.get(1).cloned().ok_or(PointwiseError::Index)?;
    let output = SharedImageMap::new(geometry);
    let mask = mask.as_ref().ok_or(PointwiseError::NullReference)?;
    let mask_values = mask.channel(0, false)?;
    let normalized = filter.parameter(0)?.effective_value_at(time)? / filter.factor();
    let radius = (if normalized > 1. { 1. } else { normalized }) as f32;
    let mask_present = filter.node().slot(1)?.node().is_some();
    let choice = filter.parameter(1)?;
    let selected = ix(choice.effective_index_at(time)?)?;
    let entries = choice.options()?.entries();
    let seamless = entries
        .get(selected)
        .ok_or(PointwiseError::Index)?
        .as_ref()
        .is_some_and(|v| v.iter().copied().eq("Yes".encode_utf16()));
    let mut channel = 0;
    while channel < 4 {
        // successful real zl cctor's immutable ordinary bound
        let original = original.as_ref().ok_or(PointwiseError::NullReference)?;
        let source = original.channel(channel, false)?;
        if source.len() == 1 {
            output.set_constant(channel, source.read(0)?)?;
        } else {
            let destination = output.materialize(channel)?;
            let temporary = scratch(original.geometry().area())?;
            for pass in 0..3 {
                Pass {
                    source: if pass == 0 { &source } else { &destination },
                    destination: &temporary,
                    mask: &mask_values,
                    geometry,
                    radius,
                    mask_present,
                    seamless,
                    vertical: false,
                }
                .execute::<E>(execution)?;
                Pass {
                    source: &temporary,
                    destination: &destination,
                    mask: &mask_values,
                    geometry,
                    radius,
                    mask_present,
                    seamless,
                    vertical: true,
                }
                .execute::<E>(execution)?;
            }
        }
        original.seal_channel(channel)?;
        output.seal_channel(channel)?;
        effects.preflight(geometry.width(), 2)?;
        channel += 1;
    }
    mask.seal_channel(0)?;
    output.copy_auxiliary(original.as_ref().ok_or(PointwiseError::NullReference)?)?;
    Ok(Some(output))
}
#[cfg(test)]
#[path = "median_test.rs"]
mod tests;
