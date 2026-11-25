//! ac2's map layer body over retained images and settings at the requested time.
use super::effect_maps::choice_is_at;
use super::{
    MapArrayOperation as Blend, PointwiseError, PointwiseFilter, PointwiseKind,
    PointwiseRenderOptions, SharedImageMap, combine_map_arrays, interpolate_map_arrays,
};
use crate::{FloatBuffer, ImageError, SharedFloatStore, SharedSelectionOptions};

impl<E: 'static> PointwiseFilter<E> {
    /// Actual ac2.g refresh: one new name array, sequential content-sensitive
    /// replacement of both choices, then raw index reads. Equal arrays retain
    /// their old roots. Source notifications are controls-only, so render
    /// caches stay warm; generic Changed delivery is not modeled by this API.
    pub fn refresh_layer_map_options(
        &self,
        input: Option<&SharedImageMap>,
    ) -> Result<[i32; 2], PointwiseError<E>> {
        if self.kind() != PointwiseKind::LayerMaps {
            return Err(PointwiseError::UnsupportedCapability(
                "Layer Maps option refresh on another node kind",
            ));
        }
        let input = input.ok_or(ImageError::NullReference)?;
        let count = input.named_channels().len();
        let mut names = Vec::new();
        names
            .try_reserve_exact(count.max(1))
            .map_err(|_| ImageError::Allocation)?;
        if count == 0 {
            names.push(Some("None Found".encode_utf16().collect()));
        } else {
            for i in 0..count {
                let channel = input
                    .named_channel(i as i32)?
                    .ok_or(ImageError::NullReference)?;
                names.push(Some(channel.name_utf16().to_vec()));
            }
        }
        let options = SharedSelectionOptions::new(names);
        self.parameter(0)?.set_options(Some(options.clone()))?;
        self.parameter(1)?.set_options(Some(options))?;
        Ok([self.parameter(0)?.index(), self.parameter(1)?.index()])
    }
}
fn data(image: &SharedImageMap, index: i32) -> Result<FloatBuffer, ImageError> {
    if index >= image.named_channels().len() as i32 {
        return Ok(FloatBuffer::new(vec![0.0]));
    }
    image
        .named_channel(index)?
        .ok_or(ImageError::NullReference)?
        .data()
        .ok_or(ImageError::NullReference)?
        .data(true)
}
fn remove(image: &SharedImageMap, index: i32) -> Result<(), ImageError> {
    if index < image.named_channels().len() as i32 {
        image.remove_named_channel(index)?;
    }
    Ok(())
}
fn remove_both(image: &SharedImageMap, first: i32, second: i32) -> Result<(), ImageError> {
    if first >= image.named_channels().len() as i32 {
        remove(image, second)
    } else if second >= image.named_channels().len() as i32 {
        image.remove_named_channel(first)
    } else {
        image.remove_two_named_channels(first, second)
    }
}
pub(super) fn render<E: 'static>(
    filter: &PointwiseFilter<E>,
    inputs: &[Option<SharedImageMap>],
    width: i32,
    _height: i32,
    options: PointwiseRenderOptions,
    time: Option<f64>,
) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
    let input = inputs.first().cloned().ok_or(PointwiseError::Index)?;
    // ai5.a (width), not an arbitrary viewport flag, controls this refresh.
    // It occurs before wo.b, including when channel rendering is disabled.
    if width == 168 {
        filter.refresh_layer_map_options(input.as_ref())?;
    }
    if !options.is_channel_rendering() {
        return Ok(input);
    }
    let input = input.ok_or(ImageError::NullReference)?;
    let out = input.shallow_copy()?;
    let index = |slot| {
        let property = filter.parameter(slot)?;
        match time {
            Some(time) => property.effective_index_at(time),
            None => Ok(property.index()),
        }
    };
    let first = data(&out, index(0)?)?;
    let second = data(&out, index(1)?)?;
    let opacity = filter.parameter(3)?;
    let opacity = match time {
        Some(time) => opacity.effective_value_at(time)?,
        None => opacity.number(),
    } as f32;
    // The source resolves the live mode string at each sequential comparison.
    let mode = |text| choice_is_at(filter, 2, time, text);
    let combined = if mode("Normal")? {
        Some(first.clone())
    } else if mode("Add")? {
        combine_map_arrays(Blend::Add, Some(&first), Some(&second))?
    } else if mode("Subtract")? {
        combine_map_arrays(Blend::Subtract, Some(&second), Some(&first))?
    } else if mode("Multiply")? {
        combine_map_arrays(Blend::Multiply, Some(&first), Some(&second))?
    } else if mode("Inverse Multiply")? {
        combine_map_arrays(Blend::InverseMultiply, Some(&first), Some(&second))?
    } else if mode("Difference")? {
        combine_map_arrays(Blend::Difference, Some(&first), Some(&second))?
    } else if mode("Negate")? {
        combine_map_arrays(Blend::Subtract, Some(&first), Some(&second))?
    } else if mode("Greatest")? {
        combine_map_arrays(Blend::Greatest, Some(&first), Some(&second))?
    } else if mode("Least")? {
        combine_map_arrays(Blend::Least, Some(&first), Some(&second))?
    } else {
        None
    };
    let reusable = combined
        .as_ref()
        .filter(|v| !v.shares_storage(&first) && !v.shares_storage(&second));
    let mut result = interpolate_map_arrays(combined.as_ref(), Some(&second), opacity, reusable)?;
    let clear = |text| choice_is_at(filter, 5, time, text);
    if clear("First")? {
        remove(&out, index(0)?)?;
    } else if clear("Second")? {
        remove(&out, index(1)?)?;
    } else if clear("Both")? {
        remove_both(&out, index(0)?, index(1)?)?;
    } else if clear("All")? {
        out.clear_named_channels();
    }
    if choice_is_at(filter, 6, time, "Yes")? {
        result = super::fix_map_contrast(Some(&result))?;
    }
    let name = filter.parameter(4)?.text()?;
    out.add_named_store(name, Some(SharedFloatStore::new(result)?))?;
    Ok(Some(out))
}
