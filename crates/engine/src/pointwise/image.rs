use super::guard;
use crate::{
    FloatBuffer, ImageError, ImageGeometry, ImageMap, SharedFloatStore, SharedNamedChannel,
};
use std::{
    fmt,
    sync::{Arc, Mutex},
};

/// Retained identity over one ImageMap authority. No image value is copied when
/// handles are cloned, and each operation releases its map lock before returning.
#[derive(Clone)]
pub struct SharedImageMap(Arc<Mutex<ImageMap>>);
impl fmt::Debug for SharedImageMap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SharedImageMap({:p})", Arc::as_ptr(&self.0))
    }
}
impl SharedImageMap {
    /// Retain the genuinely computed bitmap conversion in the existing map root.
    pub fn from_bitmap(
        bitmap: Option<&crate::Bitmap>,
        red_only: bool,
    ) -> Result<Self, crate::BitmapError> {
        ImageMap::from_bitmap(bitmap, red_only).map(Self::from_image)
    }
    pub fn has_transparency(&self) -> Result<bool, ImageError> {
        guard(&self.0).has_transparency()
    }
    pub fn new(geometry: ImageGeometry) -> Self {
        Self(Arc::new(Mutex::new(ImageMap::new(geometry))))
    }
    pub fn from_image(image: ImageMap) -> Self {
        Self(Arc::new(Mutex::new(image)))
    }
    pub fn same_reference(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    pub fn seal_permanently(&self) -> Result<(), ImageError> {
        guard(&self.0).seal_permanently()
    }
    pub fn is_permanently_sealed(&self) -> bool {
        guard(&self.0).is_permanently_sealed()
    }
    pub(super) fn finish_graph_render(&self) -> Result<(), ImageError> {
        guard(&self.0).finish_graph_render()
    }
    /// Presentation over this actual map. The private guard never crosses a
    /// caller callback; existing preview target identity and buffer rules apply.
    pub fn preview_bitmap(
        &self,
        options: crate::PreviewBitmapOptions,
        target: Option<&crate::PreviewBitmap>,
    ) -> Result<crate::PreviewBitmap, crate::PreviewBitmapError> {
        crate::preview_bitmap(Some(&mut guard(&self.0)), options, target)
    }
    /// Concrete callback-free Invert operation. Selection resolution belongs to
    /// SharedInvertNode and completes before this private map borrow begins.
    pub(crate) fn render_invert_resolved(
        input: Option<&Self>,
        width: i32,
        height: i32,
        ignored: [bool; 4],
    ) -> Result<Self, ImageError> {
        let mut source = input.map(|image| guard(&image.0));
        crate::invert::render_resolved_channels(source.as_deref_mut(), width, height, ignored)
            .map(Self::from_image)
    }
    pub(super) fn geometry_copy(&self) -> Self {
        Self::from_image(guard(&self.0).geometry_copy())
    }
    pub(super) fn uses_no_maps(&self) -> Result<bool, ImageError> {
        guard(&self.0).uses_no_maps()
    }
    pub(super) fn seal_stores_temporarily(&self) -> Result<(), ImageError> {
        guard(&self.0).seal_stores_temporarily()
    }
    pub(super) fn earlier_alias(&self, channel: i32) -> Result<Option<usize>, ImageError> {
        guard(&self.0).earlier_channel_alias(channel)
    }
    pub(super) fn copy_channel_within(
        &self,
        destination: i32,
        source: i32,
    ) -> Result<(), ImageError> {
        guard(&self.0).copy_channel_within(destination, source)
    }
    pub(super) fn copy_channel_from(
        &self,
        destination: i32,
        source: &Self,
        channel: i32,
    ) -> Result<(), ImageError> {
        if self.same_reference(source) {
            return self.copy_channel_within(destination, channel);
        }
        guard(&self.0).copy_channel_from(destination, &mut guard(&source.0), channel)
    }
    /// Exact ImageMap.nz deep copy, including independent named/region stores.
    pub fn deep_copy(&self) -> Result<Self, ImageError> {
        guard(&self.0).deep_copy().map(Self::from_image)
    }
    pub(super) fn share_pixel_backing(&self) -> Self {
        Self::from_image(crate::image_map::ImageCapture::share_image(
            self.capture_fields(),
        ))
    }
    pub fn geometry(&self) -> ImageGeometry {
        guard(&self.0).geometry()
    }
    pub fn is_disposed(&self) -> bool {
        guard(&self.0).is_disposed()
    }
    pub fn dispose(&self) {
        guard(&self.0).dispose()
    }
    pub fn channel(&self, channel: i32, readonly: bool) -> Result<FloatBuffer, ImageError> {
        guard(&self.0).channel(channel, readonly)
    }
    /// The source nf gate, including disposed-image validation.
    pub fn uses_plane(&self, channel: i32) -> Result<bool, ImageError> {
        guard(&self.0).uses_plane(channel)
    }
    pub(super) fn channel_with_effects(
        &self,
        channel: i32,
        readonly: bool,
    ) -> Result<FloatBuffer, ImageError> {
        guard(&self.0).channel_with_effects(channel, readonly)
    }
    pub(super) fn range(
        &self,
        channel: i32,
        start: i32,
        end: i32,
    ) -> Result<FloatBuffer, ImageError> {
        guard(&self.0).range(channel, start, end)
    }
    /// Actual public ImageMap named-add composition, retaining this exact
    /// wrapper and the existing deduplication, order and error behavior.
    pub fn add_named_channel(&self, channel: crate::SharedNamedChannel) -> Result<(), ImageError> {
        self.append_named(channel)
    }
    pub(super) fn append_named(
        &self,
        channel: crate::SharedNamedChannel,
    ) -> Result<(), ImageError> {
        guard(&self.0).add_named_channel(channel)
    }
    pub(super) fn seal_named(&self) -> Result<(), ImageError> {
        guard(&self.0).seal_named_channels()
    }
    pub(super) fn compress_uniform(&self) -> Result<(), ImageError> {
        guard(&self.0).compress_uniform_channels()
    }
    pub fn materialize(&self, channel: i32) -> Result<FloatBuffer, ImageError> {
        guard(&self.0).materialize(channel)
    }
    pub fn set_constant(&self, channel: i32, value: f32) -> Result<(), ImageError> {
        guard(&self.0).set_constant(channel, value)
    }
    pub fn set_buffer(&self, channel: i32, buffer: Option<&FloatBuffer>) -> Result<(), ImageError> {
        guard(&self.0).set_buffer(channel, buffer.ok_or(ImageError::NullReference)?)
    }
    /// Actual nv retained-store assignment for internal image resize. The
    /// nullable store is resolved before the destination authority is borrowed.
    pub(super) fn set_store_reference(
        &self,
        channel: i32,
        store: Option<&SharedFloatStore>,
    ) -> Result<(), ImageError> {
        let store = store.ok_or(ImageError::NullReference)?;
        guard(&self.0).set_store(channel, store)
    }
    pub fn all_channel_count(&self) -> i32 {
        guard(&self.0).all_channel_count()
    }
    /// Source b(index,source): named array allocation precedes source Name read.
    pub(super) fn allocate_indexed(
        &self,
        index: i32,
        source: &Self,
    ) -> Result<FloatBuffer, ImageError> {
        if index < 4 {
            return self.materialize(index);
        }
        let length = usize::try_from(self.geometry().area()).map_err(|_| ImageError::Overflow)?;
        let mut values = Vec::new();
        values
            .try_reserve_exact(length)
            .map_err(|_| ImageError::Allocation)?;
        values.resize(length, 0.0);
        let array = FloatBuffer::new(values);
        let name = source
            .named_channel(index - 4)?
            .ok_or(ImageError::NullReference)?
            .name_utf16()
            .to_vec();
        let store = SharedFloatStore::new(array.clone())?;
        self.add_named_store(Some(name), Some(store))?;
        Ok(array)
    }
    /// Source b(index,Single,source): fresh scalar array/store/wrapper for named.
    pub(super) fn copy_indexed_constant(
        &self,
        index: i32,
        value: f32,
        source: &Self,
    ) -> Result<(), ImageError> {
        if index < 4 {
            return self.set_constant(index, value);
        }
        let array = FloatBuffer::new(vec![value]);
        let name = source
            .named_channel(index - 4)?
            .ok_or(ImageError::NullReference)?
            .name_utf16()
            .to_vec();
        self.add_named_store(Some(name), Some(SharedFloatStore::new(array)?))
    }
    pub fn channel_count(&self) -> Result<usize, ImageError> {
        guard(&self.0).channel_count()
    }
    pub fn is_empty(&self) -> Result<bool, ImageError> {
        guard(&self.0).is_empty()
    }
    pub fn region_store(&self) -> Option<SharedFloatStore> {
        guard(&self.0).region_store()
    }
    pub fn set_region_store(&self, value: Option<SharedFloatStore>) {
        guard(&self.0).set_region_store(value)
    }
    /// Actual ny shallow copy: fresh ImageMap/list, retained planes/stores and
    /// named wrappers. The sole map authority performs source copy ordering.
    pub fn shallow_copy(&self) -> Result<Self, ImageError> {
        guard(&self.0).shallow_copy().map(Self::from_image)
    }
    pub fn clear_named_channels(&self) {
        guard(&self.0).clear_named_channels();
    }
    pub fn remove_named_channel(&self, index: i32) -> Result<(), ImageError> {
        guard(&self.0).remove_named_channel(index)
    }
    pub fn remove_two_named_channels(&self, first: i32, second: i32) -> Result<(), ImageError> {
        guard(&self.0).remove_two_named_channels(first, second)
    }
    pub fn named_channel(&self, index: i32) -> Result<Option<SharedNamedChannel>, ImageError> {
        guard(&self.0).named_channel(index)
    }
    pub fn seal_channel(&self, index: i32) -> Result<(), ImageError> {
        guard(&self.0).seal_channel(index)
    }
    pub fn named_channels(&self) -> Vec<Option<SharedNamedChannel>> {
        guard(&self.0).named_channels()
    }
    pub fn add_named_store(
        &self,
        name: Option<Vec<u16>>,
        data: Option<SharedFloatStore>,
    ) -> Result<(), ImageError> {
        guard(&self.0).add_named_store(name, data)
    }
    /// Nonmutating retained channel observations, for source trace comparison.
    /// Unlike channel(), this does not allocate default constants/materialize.
    pub(in crate::pointwise) fn capture_fields(&self) -> crate::image_map::RawImage {
        guard(&self.0).capture_fields()
    }
    pub fn channel_observation(&self) -> crate::ImageChannelObservation {
        guard(&self.0).channel_observation()
    }
    // Actual nr guard then scalar/area-sized selection; requested length is
    // not a new allocation size. In particular >Area can fault in the worker.
    pub(super) fn destination_count(
        &self,
        channel: i32,
        length: usize,
    ) -> Result<FloatBuffer, ImageError> {
        if length == 0 {
            return Err(ImageError::Argument);
        }
        if length == 1 {
            self.channel(channel, false)
        } else {
            self.materialize(channel)
        }
    }
    pub(super) fn destination(
        &self,
        channel: i32,
        source: &FloatBuffer,
    ) -> Result<FloatBuffer, ImageError> {
        if source.len() == 1 {
            self.channel(channel, false)
        } else {
            self.materialize(channel)
        }
    }
    pub(super) fn merge_effects(
        &self,
        inputs: &[Option<SharedImageMap>],
    ) -> Result<(), ImageError> {
        // Destination roles are resolved at their actual position after Clear;
        // later self roles duplicate the prefix appended by earlier sources.
        let mut sources = std::array::from_fn(|_| crate::image_map::RetainedNamedSource::Missing);
        for (i, input) in inputs.iter().take(3).enumerate() {
            sources[i] = match input {
                None => crate::image_map::RetainedNamedSource::Missing,
                Some(source) if self.same_reference(source) => {
                    crate::image_map::RetainedNamedSource::Destination
                }
                Some(source) => {
                    crate::image_map::RetainedNamedSource::Entries(source.named_channels())
                }
            };
        }
        guard(&self.0).merge_retained_named_sources(sources)
    }
    pub(super) fn copy_auxiliary(&self, input: &Self) -> Result<(), ImageError> {
        self.set_region_store(input.region_store());
        self.merge_effects(&[Some(input.clone())])
    }
}

#[cfg(test)]
mod preview_tests {
    use super::SharedImageMap;
    use crate::{ImageGeometry, PreviewBitmapOptions};
    #[test]
    fn shared_preview_reads_actual_map_and_preserves_matching_target_storage() {
        let image = SharedImageMap::new(ImageGeometry::new(2, 2));
        for (channel, value) in [0.2, 0.4, 0.8, 0.25].into_iter().enumerate() {
            image.set_constant(channel as i32, value).unwrap();
        }
        let options = PreviewBitmapOptions::default().with_dither(false);
        let first = image.preview_bitmap(options, None).unwrap();
        let retained = first.clone();
        let before = retained.argb_pixel(0, 0).unwrap();
        image.set_constant(2, 0.1).unwrap();
        let second = image.preview_bitmap(options, Some(&first)).unwrap();
        assert_ne!(second.argb_pixel(0, 0).unwrap(), before);
        assert_eq!(
            retained.argb_pixel(0, 0).unwrap(),
            second.argb_pixel(0, 0).unwrap()
        );
        assert_eq!(image.channel(2, true).unwrap().read(0).unwrap(), 0.1);
    }
}
