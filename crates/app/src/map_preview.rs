//! Presentation of an applied image, separate from the retained filter graph.
//! The projection reads source handles without clearing maps or materializing
//! upstream channels. It makes no new claim about Genetica's bitmap layouts.
use super::{ForegroundError, LiveInvert};
use crate::LIVE_PREVIEW_SIZE;
use crate::filter_pipeline::{
    FilterStage, RenderGeometry, RenderInvert, RenderStage, render_chain,
};
use gpui_kit::{RenderImage, SharedString};
use mycela_engine::{
    FloatBuffer, ImageChannelObservation, ImageMap, PreviewBitmap, PreviewBitmapFormat,
    PreviewBitmapOptions, SharedImageMap,
};
use std::sync::Arc;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum PreviewSelection {
    #[default]
    BaseColor,
    // Equal names can belong to different stores. The occurrence is scoped to
    // that exact UTF-16 name, never the position in the full map list.
    Named {
        name: Vec<u16>,
        occurrence: usize,
    },
}
impl PreviewSelection {
    pub(crate) fn name(&self) -> Option<&[u16]> {
        match self {
            Self::BaseColor => None,
            Self::Named { name, .. } => Some(name),
        }
    }
    pub(crate) fn label(&self) -> SharedString {
        match self {
            Self::BaseColor => "Base color".into(),
            Self::Named { name, occurrence } => {
                let name = String::from_utf16_lossy(name);
                let name = if name.is_empty() {
                    "Unnamed map"
                } else {
                    &name
                };
                if *occurrence == 0 {
                    format!("Map: {name}").into()
                } else {
                    format!("Map: {name} ({})", occurrence + 1).into()
                }
            }
        }
    }
}
fn failure(message: impl std::fmt::Display) -> ForegroundError {
    ForegroundError::Operation(message.to_string())
}
pub(crate) fn choices(image: &SharedImageMap) -> Vec<PreviewSelection> {
    let mut choices = vec![PreviewSelection::BaseColor];
    for channel in image.named_channels().into_iter().flatten() {
        let name = channel.name_utf16().to_vec();
        let occurrence = choices
            .iter()
            .filter(|choice| choice.name() == Some(name.as_slice()))
            .count();
        choices.push(PreviewSelection::Named { name, occurrence });
    }
    choices
}
fn retained_channel(
    observation: &ImageChannelObservation,
    channel: usize,
) -> Result<FloatBuffer, ForegroundError> {
    let flags = observation
        .flags()
        .ok_or_else(|| failure("The applied image is disposed."))?;
    if flags.get(channel).copied().unwrap_or(false) {
        observation
            .planes()
            .and_then(|planes| planes.get(channel))
            .and_then(Option::as_ref)
            .ok_or_else(|| failure("The applied image channel has no storage."))?
            .data(true)
            .map_err(failure)
    } else {
        Ok(observation
            .constants()
            .and_then(|constants| constants.get(channel))
            .and_then(Option::as_ref)
            .cloned()
            .unwrap_or_else(|| FloatBuffer::new(vec![0.0])))
    }
}
pub(crate) fn project_image(
    image: &SharedImageMap,
    selection: &PreviewSelection,
) -> Result<ImageMap, ForegroundError> {
    if image.is_disposed() {
        return Err(failure("The applied image is disposed."));
    }
    let observation = image.channel_observation();
    let mut projection = ImageMap::new(image.geometry());
    match selection {
        PreviewSelection::BaseColor => {
            for channel in 0..3 {
                projection
                    .set_buffer(channel, &retained_channel(&observation, channel as usize)?)
                    .map_err(failure)?;
            }
        }
        PreviewSelection::Named { name, occurrence } => {
            let selected = image
                .named_channels()
                .into_iter()
                .flatten()
                .filter(|channel| channel.name_utf16() == name.as_slice())
                .nth(*occurrence)
                .ok_or_else(|| {
                    failure("The selected map is no longer available. Choose another map.")
                })?;
            let values = selected
                .data()
                .ok_or_else(|| failure("The selected map has no storage."))?
                .data(true)
                .map_err(failure)?;
            for channel in 0..3 {
                projection.set_buffer(channel, &values).map_err(failure)?;
            }
        }
    }
    // Every view retains the actual source transparency, including map views.
    projection
        .set_buffer(3, &retained_channel(&observation, 3)?)
        .map_err(failure)?;
    Ok(projection)
}

pub(crate) fn project_bitmap(
    image: &SharedImageMap,
    selection: &PreviewSelection,
) -> Result<PreviewBitmap, ForegroundError> {
    let mut projection = project_image(image, selection)?;
    mycela_engine::preview_bitmap(
        Some(&mut projection),
        PreviewBitmapOptions::default().with_dither(false),
        None,
    )
    .map_err(failure)
}

// Transport uses the projected bitmap dimensions. Final export only needs the
// CPU bitmap, which keeps its own requested geometry.
pub(crate) fn project(
    image: &SharedImageMap,
    selection: &PreviewSelection,
) -> Result<(PreviewBitmap, Arc<RenderImage>), ForegroundError> {
    let bitmap = project_bitmap(image, selection)?;
    let raster = raster(&bitmap)?;
    Ok((bitmap, raster))
}
pub(crate) fn raster(bitmap: &PreviewBitmap) -> Result<Arc<RenderImage>, ForegroundError> {
    let (width, height) = (bitmap.width(), bitmap.height());
    let bytes = (width as usize)
        .checked_mul(height as usize)
        .and_then(|area| area.checked_mul(4))
        .ok_or_else(|| failure("The applied preview dimensions are too large."))?;
    let mut bgra = Vec::new();
    bgra.try_reserve_exact(bytes).map_err(failure)?;
    match bitmap.format() {
        PreviewBitmapFormat::Argb32 => {
            for row in bitmap.rows().map_err(failure)? {
                bgra.extend_from_slice(&row);
            }
        }
        PreviewBitmapFormat::Rgb24 => {
            for row in bitmap.rows().map_err(failure)? {
                for pixel in row.chunks_exact(3) {
                    bgra.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
                }
            }
        }
        PreviewBitmapFormat::PArgb32 => {
            for y in 0..height {
                for x in 0..width {
                    bgra.extend_from_slice(
                        &bitmap.argb_pixel(x, y).map_err(failure)?.to_le_bytes(),
                    );
                }
            }
        }
    }
    // Raster dimensions and BGRA are transport/data boundaries, not UI colors.
    let frame = image::Frame::new(
        image::RgbaImage::from_raw(width as u32, height as u32, bgra)
            .ok_or_else(|| failure("The applied preview has unexpected dimensions."))?,
    );
    Ok(Arc::new(RenderImage::new(vec![frame])))
}
pub(crate) struct PreparedPreview {
    pub(crate) image: SharedImageMap,
    pub(crate) selection: PreviewSelection,
    pub(crate) bitmap: PreviewBitmap,
    pub(crate) raster: Arc<RenderImage>,
}
pub(crate) fn render(
    mut image: SharedImageMap,
    invert: Option<&LiveInvert>,
    filters: &[FilterStage],
    requested: &PreviewSelection,
    time: f64,
) -> Result<PreparedPreview, ForegroundError> {
    let live_invert = invert.map(LiveInvert::model);
    image = render_chain(
        image,
        live_invert.as_ref().map(RenderInvert::Live),
        filters.iter().map(|stage| RenderStage::Live(stage, time)),
        RenderGeometry::new(LIVE_PREVIEW_SIZE, LIVE_PREVIEW_SIZE).map_err(failure)?,
        &|| false,
    )
    .map_err(failure)?;
    let selection = if choices(&image).contains(requested) {
        requested.clone()
    } else {
        PreviewSelection::BaseColor
    };
    let (bitmap, raster) = project(&image, &selection)?;
    Ok(PreparedPreview {
        image,
        selection,
        bitmap,
        raster,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mycela_engine::{ImageGeometry, SharedFloatStore};

    #[test]
    fn cpu_projection_keeps_non_square_geometry_named_occurrence_and_source_alpha() {
        let image = SharedImageMap::new(ImageGeometry::new(5, 2));
        image.set_constant(3, 0.25).unwrap();
        let name: Vec<u16> = "Height".encode_utf16().collect();
        let a = SharedFloatStore::new(FloatBuffer::new(vec![0.2])).unwrap();
        let b = SharedFloatStore::new(FloatBuffer::new(vec![0.8; 10])).unwrap();
        a.seal(true);
        b.seal(true);
        image
            .add_named_store(Some(name.clone()), Some(a.clone()))
            .unwrap();
        image
            .add_named_store(Some(name.clone()), Some(b.clone()))
            .unwrap();
        let wrapper = image.named_channel(1).unwrap().unwrap();
        for (occurrence, pixel) in [(0, 0xbf333333), (1, 0xbfcccccc)] {
            let bitmap = project_bitmap(
                &image,
                &PreviewSelection::Named {
                    name: name.clone(),
                    occurrence,
                },
            )
            .unwrap();
            assert_eq!((bitmap.width(), bitmap.height()), (5, 2));
            assert_eq!(bitmap.argb_pixel(4, 1).unwrap(), pixel);
        }
        assert!(wrapper.same_wrapper(&image.named_channel(1).unwrap().unwrap()));
        assert!(wrapper.data().unwrap().same_store(&b));
        assert!(a.is_sealed() && b.is_sealed());
        assert_eq!(image.channel(3, false).unwrap().values(), vec![0.25]);
        assert!(
            project_bitmap(
                &image,
                &PreviewSelection::Named {
                    name,
                    occurrence: 2
                }
            )
            .is_err()
        );
    }

    #[test]
    fn selecting_maps_preserves_actual_wrappers_stores_seals_and_transparency() {
        let image = SharedImageMap::new(ImageGeometry::new(96, 96));
        for (channel, value) in [0.2, 0.4, 0.8, 0.25].into_iter().enumerate() {
            image.set_constant(channel as i32, value).unwrap();
        }
        let name: Vec<_> = "Height".encode_utf16().collect();
        let values = FloatBuffer::new(vec![0.6; 96 * 96]);
        let store = SharedFloatStore::new(values.clone()).unwrap();
        store.seal(true);
        image
            .add_named_store(Some(name.clone()), Some(store.clone()))
            .unwrap();
        let wrapper = image.named_channel(0).unwrap().unwrap();
        let before = image.channel_observation();
        let (base, _) = project(&image, &PreviewSelection::BaseColor).unwrap();
        let (map, _) = project(
            &image,
            &PreviewSelection::Named {
                name,
                occurrence: 0,
            },
        )
        .unwrap();
        assert_eq!(base.argb_pixel(0, 0).unwrap(), 0xbf3366cc);
        assert_eq!(map.argb_pixel(0, 0).unwrap(), 0xbf999999);
        assert!(
            image
                .named_channel(0)
                .unwrap()
                .unwrap()
                .same_wrapper(&wrapper)
        );
        assert!(wrapper.data().unwrap().same_store(&store));
        assert!(store.is_sealed() && store.is_temporarily_sealed());
        assert!(store.buffer_observation().shares_storage(&values));
        let after = image.channel_observation();
        assert_eq!(before.flags(), after.flags());
        assert_eq!(
            before.constants().unwrap().len(),
            after.constants().unwrap().len()
        );
        for (left, right) in before
            .constants()
            .unwrap()
            .iter()
            .zip(after.constants().unwrap())
        {
            assert!(match (left, right) {
                (Some(a), Some(b)) => a.shares_storage(b),
                (None, None) => true,
                _ => false,
            });
        }
    }

    #[test]
    fn equal_names_select_distinct_actual_stores_and_unavailable_maps_fail() {
        let image = SharedImageMap::new(ImageGeometry::new(96, 96));
        let name: Vec<_> = "Map".encode_utf16().collect();
        for value in [0.2, 0.8] {
            image
                .add_named_store(
                    Some(name.clone()),
                    Some(SharedFloatStore::new(FloatBuffer::new(vec![value])).unwrap()),
                )
                .unwrap();
        }
        let list = choices(&image);
        assert_eq!(list.len(), 3);
        assert_ne!(list[1], list[2]);
        assert_eq!(
            project(&image, &list[1])
                .unwrap()
                .0
                .argb_pixel(0, 0)
                .unwrap(),
            0xff333333
        );
        assert_eq!(
            project(&image, &list[2])
                .unwrap()
                .0
                .argb_pixel(0, 0)
                .unwrap(),
            0xffcccccc
        );
        image.remove_named_channel(1).unwrap();
        assert!(project(&image, &list[2]).is_err());
        image
            .named_channel(0)
            .unwrap()
            .unwrap()
            .data()
            .unwrap()
            .dispose();
        assert!(project(&image, &list[1]).is_err());
        assert_eq!(image.named_channels().len(), 1);
        image.dispose();
        assert!(project(&image, &PreviewSelection::BaseColor).is_err());
    }
}
