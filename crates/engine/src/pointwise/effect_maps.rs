//! ak3/bt/w3/ad2 protected supplied-map bodies over the real retained maps.
//! Scene/group/clone scheduling and animated/adjusted properties remain
//! separate capabilities; Original image graphs use these same bodies.
use super::numeric::{div, sub};
use super::{
    ArithmeticOperation as Op, ArithmeticWorker, PointwiseError, PointwiseFilter, PointwiseKind,
    PointwiseRenderOptions, SharedImageMap,
};
use crate::{FloatBuffer, ImageError, SharedFloatStore, SharedNamedChannel};

fn allocate(length: usize) -> Result<FloatBuffer, ImageError> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(length)
        .map_err(|_| ImageError::Allocation)?;
    values.resize(length, 0.0);
    Ok(FloatBuffer::new(values))
}

/// Actual bt.g contrast operation, including the retained-input early return.
/// Addition and multiplication are distinct whole-array worker passes, using
/// the existing original-worker scalar operand policies.
pub fn fix_map_contrast(input: Option<&FloatBuffer>) -> Result<FloatBuffer, ImageError> {
    let input = input.ok_or(ImageError::NullReference)?;
    let mut low = 1.0f32;
    let mut high = 0.0f32;
    for i in 0..input.len() {
        if input.read(i)? < low {
            low = input.read(i)?;
        }
        if input.read(i)? > high {
            high = input.read(i)?;
        }
    }
    if low < 0.0 {
        low = 0.0;
    }
    if high > 1.0 {
        high = 1.0;
    }
    if high != low && sub(high, low) < 0.998f32 {
        let out = allocate(input.len())?;
        let factor = div(1.0, sub(high, low));
        ArithmeticWorker::new(
            Op::AddScalar,
            Some(out.clone()),
            Some(input.clone()),
            None,
            sub(0.0, low),
        )
        .run()?;
        ArithmeticWorker::new(
            Op::MultiplyScalar,
            Some(out.clone()),
            Some(out.clone()),
            None,
            factor,
        )
        .run()?;
        Ok(out)
    } else {
        Ok(input.clone())
    }
}

fn literal(value: &Option<Vec<u16>>, expected: &str) -> bool {
    value
        .as_ref()
        .is_some_and(|v| v.iter().copied().eq(expected.encode_utf16()))
}
fn required(input: Option<SharedImageMap>) -> Result<SharedImageMap, ImageError> {
    input.ok_or(ImageError::NullReference)
}
fn named(image: &SharedImageMap, index: i32) -> Result<SharedNamedChannel, ImageError> {
    image.named_channel(index)?.ok_or(ImageError::NullReference)
}
fn stored(values: FloatBuffer) -> Result<SharedFloatStore, ImageError> {
    SharedFloatStore::new(values)
}
fn trim(value: Option<Vec<u16>>) -> Result<Vec<u16>, ImageError> {
    let value = value.ok_or(ImageError::NullReference)?;
    let mut start = 0;
    let mut end = value.len();
    while start < end && crate::image_map::name_whitespace(value[start]) {
        start += 1;
    }
    while end > start && crate::image_map::name_whitespace(value[end - 1]) {
        end -= 1;
    }
    Ok(value[start..end].to_vec())
}

pub(super) fn choice_is_at<E: 'static>(
    filter: &PointwiseFilter<E>,
    index: i32,
    time: Option<f64>,
    expected: &str,
) -> Result<bool, PointwiseError<E>> {
    match time {
        Some(time) => super::adaptive::text(filter, index, time, expected),
        None => Ok(literal(&filter.parameter(index)?.text()?, expected)),
    }
}

pub(super) fn render<E: 'static>(
    filter: &PointwiseFilter<E>,
    inputs: &[Option<SharedImageMap>],
    width: i32,
    height: i32,
    options: PointwiseRenderOptions,
    time: Option<f64>,
) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
    // The resolver runs before wo.b. A disabled body returns even a null input
    // without reading choices, string values, geometry or disposed channels.
    let input = inputs.first().cloned().ok_or(PointwiseError::Index)?;
    if !options.is_channel_rendering() {
        return Ok(input);
    }
    let input = required(input)?;
    let out = input.shallow_copy()?;
    match filter.kind() {
        PointwiseKind::ClearMaps => {
            let keep = literal(&filter.parameter(0)?.text()?, "Keep Selection");
            let selection = filter.parameter(1)?;
            let index = if literal(&selection.text()?, "Last") {
                out.named_channels().len() as i32 - 1
            } else {
                selection.index().wrapping_sub(1)
            };
            let mut retained = None;
            let mut name = None;
            if keep && index >= 0 {
                if (index as usize) < out.named_channels().len() {
                    let channel = named(&out, index)?;
                    retained = channel.data();
                    name = Some(channel.name_utf16().to_vec());
                } else {
                    retained = Some(stored(input.channel(0, false)?)?);
                }
            }
            if keep {
                out.clear_named_channels();
            } else if index >= 0 && (index as usize) < out.named_channels().len() {
                out.remove_named_channel(index)?;
            }
            if let Some(data) = retained {
                let replacement = trim(filter.parameter(2)?.text()?)?;
                out.add_named_store(
                    if replacement.is_empty() {
                        name
                    } else {
                        Some(replacement)
                    },
                    Some(data),
                )?;
            }
        }
        PointwiseKind::StoreMap => {
            if choice_is_at(filter, 1, time, "Yes")? {
                out.clear_named_channels();
            }
            let mut array = input.channel(0, false)?;
            if choice_is_at(filter, 2, time, "Yes")? {
                array = fix_map_contrast(Some(&array))?;
            }
            // NamedChannel(string,float[]) reads the raw name before wrapping
            // the actual array in a fresh store. The array remains aliased.
            let name = filter.parameter(0)?.text()?;
            out.add_named_store(name, Some(stored(array)?))?;
        }
        PointwiseKind::MergeMaps => {
            let count = out.named_channels().len();
            let mut array = if count == 0 {
                input.channel(0, false)?
            } else if count == 1 {
                named(&out, 0)?
                    .data()
                    .ok_or(ImageError::NullReference)?
                    .data(true)?
            } else {
                let area = width.wrapping_mul(height);
                let length = usize::try_from(area).map_err(|_| ImageError::Overflow)?;
                let array = allocate(length)?;
                for i in 0..count {
                    let other = named(&out, i as i32)?
                        .data()
                        .ok_or(ImageError::NullReference)?
                        .data(true)?;
                    if other.len() == 1 {
                        ArithmeticWorker::new(
                            Op::AddScalar,
                            Some(array.clone()),
                            Some(array.clone()),
                            None,
                            other.read(0)?,
                        )
                        .run()?;
                    } else {
                        ArithmeticWorker::new(
                            Op::AddArrays,
                            Some(array.clone()),
                            Some(array.clone()),
                            Some(other),
                            0.0,
                        )
                        .run()?;
                    }
                }
                ArithmeticWorker::new(
                    Op::MultiplyScalar,
                    Some(array.clone()),
                    Some(array.clone()),
                    None,
                    div(1.0, count as f32),
                )
                .run()?;
                array
            };
            if choice_is_at(filter, 1, time, "Yes")? || out.named_channels().len() == 1 {
                out.clear_named_channels();
            }
            if choice_is_at(filter, 2, time, "Yes")? {
                array = fix_map_contrast(Some(&array))?;
            }
            let name = filter.parameter(0)?.text()?;
            out.add_named_store(name, Some(stored(array)?))?;
        }
        PointwiseKind::FixMapContrast => {
            out.clear_named_channels();
            for i in 0..input.named_channels().len() {
                let array = named(&input, i as i32)?
                    .data()
                    .ok_or(ImageError::NullReference)?
                    .data(true)?;
                let array = fix_map_contrast(Some(&array))?;
                // The source obtains the name after the contrast pass.
                let name = named(&input, i as i32)?.name_utf16().to_vec();
                out.add_named_store(Some(name), Some(stored(array)?))?;
                out.seal_channel((i as i32).wrapping_add(4))?;
            }
        }
        _ => unreachable!("effect-map dispatch accepts only the four actual kinds"),
    }
    Ok(Some(out))
}
