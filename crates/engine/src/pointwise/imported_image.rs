//! Real mn custom state behind the existing aoi object/event authority.
use super::{Payload, PointwiseCustomObject, PointwiseMatrixHandler, imported_region::Region};
use crate::{
    BinaryCursor, BinaryWriter, ByteBuffer, EncodedByteStore, ImportedImageError,
    ImportedImageSaveError, ImportedImageWrite, PointwiseError, SharedImageMap,
};
use std::{
    fmt,
    sync::{Arc, Mutex, MutexGuard, Weak},
};

fn guard<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|error| error.into_inner())
}
#[derive(Default)]
struct Fields {
    width: i32,
    height: i32,
    dimensions_valid: bool,
    container: Option<Arc<Container>>,
}
pub(super) struct ImportedState {
    fields: Mutex<Fields>,
}
impl ImportedState {
    fn new() -> Self {
        Self {
            fields: Mutex::new(Fields::default()),
        }
    }
}
struct ContainerFields {
    store: Option<EncodedByteStore>,
    cached_image: Option<SharedImageMap>,
    jpeg: Option<Arc<CapturedJpeg>>,
    // Actual receiver IDs, not weak-only receivers. The outward sponsor retains
    // their complete existing aoi bodies without a container→body Arc cycle.
    dimensions: Vec<u64>,
}
struct Container {
    fields: Mutex<ContainerFields>,
}

/// One successful decode for an immutable encoded generation in a capture.
/// Consumers receive separate image, store and writable buffer authorities.
pub(in crate::pointwise) struct CapturedJpeg {
    encoded: Weak<Vec<u8>>,
    raster: Mutex<Option<SharedImageMap>>,
}
impl CapturedJpeg {
    pub(in crate::pointwise) fn new(encoded: &Arc<Vec<u8>>) -> Self {
        Self {
            encoded: Arc::downgrade(encoded),
            raster: Mutex::new(None),
        }
    }

    fn matches(&self, encoded: &Arc<Vec<u8>>) -> bool {
        self.encoded.as_ptr() == Arc::as_ptr(encoded)
    }

    fn decode(&self, encoded: &[u8]) -> Result<SharedImageMap, crate::ImageDecodeError> {
        let mut raster = guard(&self.raster);
        if raster.is_none() {
            let bitmap = crate::image_decode::decode_jpeg(encoded)?;
            let decoded = SharedImageMap::from_bitmap(Some(&bitmap), false);
            bitmap.dispose();
            *raster = Some(decoded?);
        }
        let image = raster.as_ref().expect("successful JPEG decode");
        Ok(image.share_pixel_backing())
    }
}
impl Container {
    fn store(&self) -> Option<EncodedByteStore> {
        guard(&self.fields).store.clone()
    }
    fn remove_receiver(&self, id: u64) {
        let mut fields = guard(&self.fields);
        if let Some(ix) = fields.dimensions.iter().rposition(|actual| *actual == id) {
            fields.dimensions.remove(ix);
        }
    }
}
/// Shared actual mn+a container. Its outward handle sponsors every actual
/// dimensions receiver; cache observation does not invoke a decoder.
pub struct ImportedImageContainer<E = ()> {
    core: Arc<Container>,
    region: Arc<Region<E>>,
}
impl<E> Clone for ImportedImageContainer<E> {
    fn clone(&self) -> Self {
        Self {
            core: self.core.clone(),
            region: self.region.clone(),
        }
    }
}
impl<E> fmt::Debug for ImportedImageContainer<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ImportedImageContainer({:p})", Arc::as_ptr(&self.core))
    }
}
impl<E> ImportedImageContainer<E> {
    pub fn same_reference(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.core, &other.core)
    }
    pub fn store(&self) -> Option<EncodedByteStore> {
        self.core.store()
    }
    pub fn cached_image(&self) -> Option<SharedImageMap> {
        guard(&self.core.fields).cached_image.clone()
    }
    pub fn dimension_listeners(&self) -> Vec<PointwiseCustomObject<E>> {
        let ids = guard(&self.core.fields).dimensions.clone();
        ids.into_iter()
            .map(|id| {
                let body = Region::body(&self.region, id);
                PointwiseCustomObject {
                    body,
                    anchor: None,
                    imported_region: Some(self.region.clone()),
                }
                .external()
            })
            .collect()
    }
}
/// Host work not yet supplied by the standalone application. No completed
/// logger/dialog is inferred, and no source store clearing is fabricated.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImportedImageReporting {
    ErrorLogger,
    InsufficientMemoryDialog,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImportedImageDecodeError {
    Storage(ImportedImageError),
    Decode(crate::ImageDecodeError),
    HostReporting {
        cause: crate::ImageDecodeError,
        boundary: ImportedImageReporting,
    },
}
impl ImportedImageDecodeError {
    pub const fn source_class(self) -> Option<&'static str> {
        match self {
            Self::Storage(error) => Some(error.source_class()),
            Self::Decode(error) => error.source_class(),
            Self::HostReporting { .. } => None,
        }
    }
}
impl fmt::Display for ImportedImageDecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for ImportedImageDecodeError {}

/// Actual retained mn object, using PointwiseCustomObject's existing aoi event
/// stream. Clone retains identity; source_clone creates a distinct shared copy.
pub struct PointwiseImportedImage<E = ()> {
    object: PointwiseCustomObject<E>,
}
impl<E> Clone for PointwiseImportedImage<E> {
    fn clone(&self) -> Self {
        Self {
            object: self.object.clone(),
        }
    }
}
impl<E> fmt::Debug for PointwiseImportedImage<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PointwiseImportedImage({:?})", self.object)
    }
}
impl<E> Default for PointwiseImportedImage<E> {
    fn default() -> Self {
        Self::new()
    }
}
impl<E> PointwiseImportedImage<E> {
    pub fn new() -> Self {
        Self::from_object(PointwiseCustomObject::from_payload(Payload::Imported(
            ImportedState::new(),
        )))
    }
    pub(super) fn from_object(object: PointwiseCustomObject<E>) -> Self {
        Self { object }
    }
    fn fields(&self) -> &Mutex<Fields> {
        match &self.object.body.payload {
            Payload::Imported(state) => &state.fields,
            _ => unreachable!("typed ImportedImage handle"),
        }
    }
    fn region(&self) -> &Arc<Region<E>> {
        self.object
            .imported_region
            .as_ref()
            .expect("published imported receiver sponsor")
    }
    fn container_core(&self) -> Option<Arc<Container>> {
        guard(self.fields()).container.clone()
    }
    pub fn custom_object(&self) -> PointwiseCustomObject<E> {
        self.object.clone()
    }
    pub fn same_reference(&self, other: &Self) -> bool {
        self.object.same_reference(&other.object)
    }
    pub fn cached_dimensions(&self) -> [i32; 2] {
        let fields = guard(self.fields());
        [fields.width, fields.height]
    }
    pub fn has_valid_dimensions(&self) -> bool {
        guard(self.fields()).dimensions_valid
    }
    pub fn container(&self) -> Option<ImportedImageContainer<E>> {
        self.container_core().map(|core| ImportedImageContainer {
            core,
            region: self.region().clone(),
        })
    }
    pub fn bytes(&self) -> Result<Option<ByteBuffer>, ImportedImageError> {
        // mn2138 rereads the retained container/store on the nonnull branch.
        if self.container_core().is_none() {
            return Ok(None);
        }
        if self
            .container_core()
            .ok_or(ImportedImageError::NullReference)?
            .store()
            .is_none()
        {
            return Ok(None);
        }
        let store = self
            .container_core()
            .ok_or(ImportedImageError::NullReference)?
            .store()
            .ok_or(ImportedImageError::NullReference)?;
        Ok(Some(store.bytes()?))
    }
    pub fn set_bytes(&self, bytes: Option<ByteBuffer>) -> Result<(), ImportedImageError> {
        let Some(bytes) = bytes else {
            let old = guard(self.fields()).container.take();
            drop(old);
            // The source null arm does not unsubscribe the previous container.
            return Ok(());
        };
        if let Some(old) = self.container_core() {
            old.remove_receiver(self.object.body.id);
        }
        // Empty input and constructor failures occur AFTER removal but
        // BEFORE the current container field changes.
        let store = EncodedByteStore::new(bytes)?;
        let container = Arc::new(Container {
            fields: Mutex::new(ContainerFields {
                store: Some(store),
                cached_image: None,
                jpeg: None,
                dimensions: Vec::new(),
            }),
        });
        let old = guard(self.fields()).container.replace(container.clone());
        drop(old);
        guard(&container.fields)
            .dimensions
            .push(self.object.body.id);
        Ok(())
    }
    pub(in crate::pointwise) fn copy_fields_from(&self, source: &Self) {
        // Cast/null checks are performed by the existing public aoi wrapper.
        // Copy does not unsubscribe the old container or subscribe to the new.
        let width = source.cached_dimensions()[0];
        guard(self.fields()).width = width;
        let height = source.cached_dimensions()[1];
        guard(self.fields()).height = height;
        guard(self.fields()).dimensions_valid = true;
        Region::merge(self.region(), source.region());
        let container = source.container_core();
        let old = std::mem::replace(&mut guard(self.fields()).container, container);
        drop(old);
    }
    pub fn source_clone(&self) -> Self {
        let copy = Self::new();
        copy.copy_fields_from(self);
        copy
    }
    /// Actual mn.n lazy getter. Warm cache reads preserve the retained map root.
    pub fn image(&self) -> Result<Option<SharedImageMap>, ImportedImageDecodeError> {
        if self.container_core().is_none() {
            return Ok(None);
        }
        let container = self
            .container_core()
            .ok_or(ImportedImageDecodeError::Storage(
                ImportedImageError::NullReference,
            ))?;
        self.container_image(&container)
    }
    pub fn width(&self) -> Result<i32, ImportedImageDecodeError> {
        if self.has_valid_dimensions() {
            return Ok(self.cached_dimensions()[0]);
        }
        if self.image()?.is_none() {
            return Ok(0);
        }
        Ok(self
            .image()?
            .ok_or(ImportedImageDecodeError::Decode(
                crate::ImageDecodeError::NullReference,
            ))?
            .geometry()
            .width())
    }
    pub fn height(&self) -> Result<i32, ImportedImageDecodeError> {
        if self.has_valid_dimensions() {
            return Ok(self.cached_dimensions()[1]);
        }
        if self.image()?.is_none() {
            return Ok(0);
        }
        Ok(self
            .image()?
            .ok_or(ImportedImageDecodeError::Decode(
                crate::ImageDecodeError::NullReference,
            ))?
            .geometry()
            .height())
    }
    fn container_image(
        &self,
        container: &Arc<Container>,
    ) -> Result<Option<SharedImageMap>, ImportedImageDecodeError> {
        let cold = guard(&container.fields).cached_image.is_none();
        if cold && container.store().is_some() {
            // Store access and stream construction precede the source inner try.
            let store = container.store().ok_or(ImportedImageDecodeError::Storage(
                ImportedImageError::NullReference,
            ))?;
            let bytes = store.bytes().map_err(ImportedImageDecodeError::Storage)?;
            // Synchronous profile: each cold invocation reads current caller
            // bytes. The temporary snapshot is never retained as owner state.
            let input = bytes.generation();
            let mut reader = BinaryCursor::new(&input)
                .map_err(|e| ImportedImageDecodeError::Storage(e.into()))?;
            if let Err(cause) = self.decode_container(container, &input, &mut reader) {
                if cause.source_class().is_none() {
                    return Err(ImportedImageDecodeError::Decode(cause));
                }
                let boundary = if cause.source_class() == Some("OutOfMemoryException") {
                    ImportedImageReporting::InsufficientMemoryDialog
                } else {
                    ImportedImageReporting::ErrorLogger
                };
                // Source can keep a published unsealed cache on failure. The
                // real logger/modal boundary is unported: leave its later
                // store=null write unperformed and expose the reached effect.
                return Err(ImportedImageDecodeError::HostReporting { cause, boundary });
            }
        }
        // Separate final read: do not return a decoded local from the cold arm.
        Ok(guard(&container.fields).cached_image.clone())
    }
    fn decode_container(
        &self,
        container: &Arc<Container>,
        input: &Arc<Vec<u8>>,
        reader: &mut BinaryCursor<'_>,
    ) -> Result<(), crate::ImageDecodeError> {
        use crate::ImageDecodeError;
        let mut probe = [0u8; 4];
        let length = reader.length()?;
        if length == 0 {
            return Err(ImageDecodeError::UnsupportedCapability(
                "zero-length disposed/corrupt encoded store",
            ));
        }
        reader.read_into(&mut probe, 0, (4 % length) as usize)?;
        reader.seek(0)?;
        let native = if &probe == b"#?RA" {
            Self::publish_cache(
                container,
                SharedImageMap::from_image(crate::decode_hdr(reader)?),
            );
            false
        } else if probe[0].eq_ignore_ascii_case(&b'p') && probe[1].eq_ignore_ascii_case(&b'f') {
            Self::publish_cache(
                container,
                SharedImageMap::from_image(crate::decode_pfm(reader)?),
            );
            false
        } else if crate::probe_tga(reader)? {
            reader.seek(0)?;
            Self::publish_cache(
                container,
                SharedImageMap::from_image(crate::decode_tga(reader)?),
            );
            false
        } else {
            true
        };
        if native {
            let jpeg = guard(&container.fields).jpeg.clone();
            if let Some(jpeg) = jpeg.filter(|jpeg| jpeg.matches(input)) {
                let map = jpeg.decode(input)?;
                Self::publish_cache(container, map);
                self.emit_dimensions(container)?;
            } else {
                let bitmap = crate::image_decode::decode_bitmap(input)?;
                let result = (|| {
                    let map = SharedImageMap::from_bitmap(Some(&bitmap), false)?;
                    Self::publish_cache(container, map);
                    self.emit_dimensions(container)
                })();
                bitmap.dispose(); // real owned root cleanup, including failure arm
                result?;
            }
        }
        // Actual2157 rereads current m_c AFTER effects and Bitmap finally.
        let current = guard(&container.fields)
            .cached_image
            .clone()
            .ok_or(ImageDecodeError::NullReference)?;
        current.seal_permanently()?;
        Ok(())
    }
    fn publish_cache(container: &Arc<Container>, map: SharedImageMap) {
        let old = guard(&container.fields).cached_image.replace(map);
        drop(old);
    }
    fn emit_dimensions(&self, container: &Arc<Container>) -> Result<(), crate::ImageDecodeError> {
        if guard(&container.fields).dimensions.is_empty() {
            return Ok(());
        }
        // Source rechecks/rereads its delegate and current cache; sponsor bodies
        // are the same actual receivers used by setter/copy, not notifications.
        let receivers = guard(&container.fields).dimensions.clone();
        let current = guard(&container.fields)
            .cached_image
            .clone()
            .ok_or(crate::ImageDecodeError::NullReference)?;
        let width = current.geometry().width();
        let current = guard(&container.fields)
            .cached_image
            .clone()
            .ok_or(crate::ImageDecodeError::NullReference)?;
        let height = current.geometry().height();
        for id in receivers {
            let body = Region::body(self.region(), id);
            match &body.payload {
                Payload::Imported(state) => {
                    guard(&state.fields).width = width;
                    guard(&state.fields).height = height;
                    guard(&state.fields).dimensions_valid = true;
                }
                _ => unreachable!("actual imported dimension receiver"),
            }
        }
        Ok(())
    }
    pub fn clear_image_cache(&self) {
        if self.container_core().is_some()
            && let Some(container) = self.container_core()
        {
            let old = guard(&container.fields).cached_image.take();
            drop(old);
        }
    }
    pub fn save_with<W>(
        &self,
        mut write: impl FnMut(ImportedImageWrite<'_>) -> Result<(), W>,
    ) -> Result<(), ImportedImageSaveError<W>> {
        let first = self.bytes().map_err(ImportedImageSaveError::Image)?;
        let array = if first.is_none() {
            ByteBuffer::new(Vec::new())
        } else {
            self.bytes().map_err(ImportedImageSaveError::Image)?.ok_or(
                ImportedImageSaveError::Image(ImportedImageError::NullReference),
            )?
        };
        let length = i32::try_from(array.len()).map_err(|_| {
            ImportedImageSaveError::Image(ImportedImageError::UnsupportedCapability(
                "CLR Byte[] length exceeds Int32",
            ))
        })?;
        write(ImportedImageWrite::Length(length)).map_err(ImportedImageSaveError::Writer)?;
        write(ImportedImageWrite::Bytes(&array)).map_err(ImportedImageSaveError::Writer)
    }
    pub fn save(&self, writer: &mut BinaryWriter) -> Result<(), ImportedImageError> {
        self.save_with(|command| match command {
            ImportedImageWrite::Length(length) => writer.write_i32(length),
            ImportedImageWrite::Bytes(bytes) => writer.write_bytes(&bytes.generation()),
        })
        .map_err(|error| match error {
            ImportedImageSaveError::Image(error) => error,
            ImportedImageSaveError::Writer(error) => ImportedImageError::Binary(error),
        })
    }
    pub fn save_nullable(
        &self,
        writer: Option<&mut BinaryWriter>,
    ) -> Result<(), ImportedImageError> {
        match writer {
            Some(writer) => self.save(writer),
            None => self.save(&mut BinaryWriter::null_receiver()),
        }
    }
    pub fn load(
        &self,
        reader: &mut BinaryCursor<'_>,
        _encoding: f64,
    ) -> Result<(), ImportedImageError> {
        let length = reader.read_i32()?;
        if length > 0 {
            self.set_bytes(Some(ByteBuffer::new(reader.read_bytes(length)?)))?;
        }
        Ok(())
    }
    pub fn load_nullable(
        &self,
        reader: Option<&mut BinaryCursor<'_>>,
        encoding: f64,
    ) -> Result<(), ImportedImageError> {
        match reader {
            Some(reader) => self.load(reader, encoding),
            None => self.load(&mut BinaryCursor::null_receiver(), encoding),
        }
    }
    pub fn add_handler(&self, handler: PointwiseMatrixHandler<E>) {
        self.object.add_handler(handler);
    }
    pub fn remove_handler(&self, handler: &PointwiseMatrixHandler<E>) {
        self.object.remove_handler(handler);
    }
    pub fn host_handlers(&self) -> Vec<PointwiseMatrixHandler<E>> {
        self.object.host_handlers()
    }
    pub fn copy_and_notify(&self, source: Option<&Self>) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        self.object
            .copy_and_notify(source.map(|source| &source.object))
    }
    pub fn notify(&self) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        self.object.notify()
    }
}

// The encoded bytes and genuine cold decode are the only cache producers.
// No public field setter or captured-state hydration path is exposed.

// These hooks transfer only actual target raw state into private frame owners.
// They are not public setters or expected-output hydration APIs.
impl<E> PointwiseImportedImage<E> {
    pub(in crate::pointwise) fn capture_fields(
        &self,
    ) -> ([i32; 2], bool, Option<ImportedImageContainer<E>>) {
        let fields = guard(self.fields());
        let container = fields
            .container
            .as_ref()
            .map(|core| ImportedImageContainer {
                core: core.clone(),
                region: self.region().clone(),
            });
        (
            [fields.width, fields.height],
            fields.dimensions_valid,
            container,
        )
    }
    pub(in crate::pointwise) fn empty_captured_container(&self) -> ImportedImageContainer<E> {
        ImportedImageContainer {
            core: Arc::new(Container {
                fields: Mutex::new(ContainerFields {
                    store: None,
                    cached_image: None,
                    jpeg: None,
                    dimensions: Vec::new(),
                }),
            }),
            region: self.region().clone(),
        }
    }
    pub(in crate::pointwise) fn install_captured_fields(
        &self,
        dimensions: [i32; 2],
        valid: bool,
        container: Option<&ImportedImageContainer<E>>,
    ) {
        if let Some(container) = container {
            Region::merge(self.region(), &container.region);
        }
        let mut fields = guard(self.fields());
        fields.width = dimensions[0];
        fields.height = dimensions[1];
        fields.dimensions_valid = valid;
        fields.container = container.map(|v| v.core.clone());
    }
}
impl<E> ImportedImageContainer<E> {
    pub(in crate::pointwise) fn install_captured_fields(
        &self,
        store: Option<EncodedByteStore>,
        cached_image: Option<SharedImageMap>,
        jpeg: Option<Arc<CapturedJpeg>>,
        receivers: &[PointwiseCustomObject<E>],
    ) {
        let mut dimensions = Vec::with_capacity(receivers.len());
        for receiver in receivers {
            let region = receiver
                .imported_region
                .as_ref()
                .expect("captured actual mn receiver");
            Region::merge(&self.region, region);
            dimensions.push(receiver.body.id);
        }
        *guard(&self.core.fields) = ContainerFields {
            store,
            cached_image,
            jpeg,
            dimensions,
        };
    }
}
