use std::{
    error::Error,
    fmt,
    sync::{Arc, Mutex},
};

/// A float array authority. Cloning a handle preserves its writable aliases.
#[derive(Clone)]
pub struct FloatBuffer(Arc<Mutex<FloatStorage>>);

#[derive(Clone)]
enum FloatStorage {
    Owned(Vec<f32>),
    Shared(Arc<Vec<f32>>),
}
impl FloatStorage {
    fn values(&self) -> &[f32] {
        match self {
            Self::Owned(values) => values,
            Self::Shared(values) => values,
        }
    }

    fn share_values(&mut self) -> Arc<Vec<f32>> {
        if let Self::Owned(values) = self {
            *self = Self::Shared(Arc::new(std::mem::take(values)));
        }
        match self {
            Self::Shared(values) => values.clone(),
            Self::Owned(_) => unreachable!("shared float values"),
        }
    }

    fn values_mut(&mut self) -> &mut [f32] {
        match self {
            Self::Owned(values) => values,
            Self::Shared(values) => Arc::make_mut(values).as_mut_slice(),
        }
    }
}
impl fmt::Debug for FloatBuffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Debug writers can call back into this buffer; do not hold its lock.
        write!(f, "FloatBuffer({:p})", Arc::as_ptr(&self.0))
    }
}

impl FloatBuffer {
    pub fn new(values: Vec<f32>) -> Self {
        Self(Arc::new(Mutex::new(FloatStorage::Owned(values))))
    }
    pub fn values(&self) -> Vec<f32> {
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .values()
            .to_vec()
    }
    pub fn len(&self) -> usize {
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .values()
            .len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub(crate) fn uniform_value(&self) -> Option<f32> {
        let values = self.0.lock().unwrap_or_else(|error| error.into_inner());
        let (&first, rest) = values.values().split_first()?;
        rest.iter().all(|&value| value == first).then_some(first)
    }
    pub fn read(&self, index: usize) -> Result<f32, ImageError> {
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .values()
            .get(index)
            .copied()
            .ok_or(ImageError::Index)
    }
    pub fn write(&self, index: usize, value: f32) -> Result<(), ImageError> {
        let mut values = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if index >= values.values().len() {
            return Err(ImageError::Index);
        }
        values.values_mut()[index] = value;
        Ok(())
    }
    /// A read-only view of one plane. A subsequent writer splits the
    /// backing storage while writable handles still share the same owner.
    pub(crate) fn shared_values(&self) -> Arc<Vec<f32>> {
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .share_values()
    }

    pub(crate) fn with_mut_slice<T>(&self, update: impl FnOnce(&mut [f32]) -> T) -> T {
        let mut values = self.0.lock().unwrap_or_else(|error| error.into_inner());
        update(values.values_mut())
    }
    /// Commit a short worker block under one lock. Strided blocks are used
    /// by column workers whose pixels are interleaved in row-major storage.
    pub(crate) fn write_block(
        &self,
        start: usize,
        stride: usize,
        block: &[f32],
    ) -> Result<(), ImageError> {
        if block.is_empty() {
            return Ok(());
        }
        let mut values = self.0.lock().unwrap_or_else(|error| error.into_inner());
        let end = (block.len() - 1)
            .checked_mul(stride)
            .and_then(|offset| start.checked_add(offset))
            .filter(|last| *last < values.values().len())
            .ok_or(ImageError::Index)?;
        let values = values.values_mut();
        if stride == 1 {
            values[start..=end].copy_from_slice(block);
        } else {
            for (offset, value) in block.iter().enumerate() {
                values[start + offset * stride] = *value;
            }
        }
        Ok(())
    }
    pub fn shares_storage(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    fn detached_copy(&self, share_backing: bool) -> Self {
        let mut storage = self.0.lock().unwrap_or_else(|error| error.into_inner());
        let backing = if share_backing {
            FloatStorage::Shared(storage.share_values())
        } else {
            storage.clone()
        };
        Self(Arc::new(Mutex::new(backing)))
    }
    fn copy_range(&self, start: usize, length: usize) -> Result<Self, ImageError> {
        let mut copy = Vec::new();
        copy.try_reserve_exact(length)
            .map_err(|_| ImageError::Allocation)?;
        let values = self.0.lock().unwrap_or_else(|error| error.into_inner());
        let end = start.checked_add(length).ok_or(ImageError::Argument)?;
        let slice = values
            .values()
            .get(start..end)
            .ok_or(ImageError::Argument)?;
        copy.extend_from_slice(slice);
        Ok(Self::new(copy))
    }
    #[cfg(test)]
    fn identity(&self) -> usize {
        Arc::as_ptr(&self.0) as usize
    }
    #[cfg(test)]
    pub(crate) fn shares_read_only_backing(&self, other: &Self) -> bool {
        let backing =
            |buffer: &Self| match &*buffer.0.lock().unwrap_or_else(|error| error.into_inner()) {
                FloatStorage::Shared(values) => Some(values.clone()),
                FloatStorage::Owned(_) => None,
            };
        match (backing(self), backing(other)) {
            (Some(left), Some(right)) => Arc::ptr_eq(&left, &right),
            _ => false,
        }
    }
}

pub(crate) fn read_pixel(values: &[f32], index: i32) -> Result<f32, ImageError> {
    values
        .get(usize::try_from(index).map_err(|_| ImageError::Index)?)
        .copied()
        .ok_or(ImageError::Index)
}

pub(crate) fn write_pixel(values: &mut [f32], index: i32, value: f32) -> Result<(), ImageError> {
    let pixel = values
        .get_mut(usize::try_from(index).map_err(|_| ImageError::Index)?)
        .ok_or(ImageError::Index)?;
    *pixel = value;
    Ok(())
}

/// A retained in-memory store, distinct from the float array it owns.
/// Disposal invalidates public retrieval through every shared store handle;
/// already returned arrays and the fixed length remain valid.
/// Disk spill, sealing and replacement of backing arrays remain unported.
#[derive(Clone, Debug)]
pub struct SharedFloatStore(Arc<FloatStoreCore>);
#[derive(Debug)]
struct FloatStoreCore {
    length: usize,
    state: Mutex<FloatStoreState>,
}
#[derive(Debug)]
struct FloatStoreState {
    buffer: FloatBuffer,
    disposed: bool,
    sealed: bool,
    temporary_seal: bool,
}
impl SharedFloatStore {
    pub fn new(buffer: FloatBuffer) -> Result<Self, ImageError> {
        Self::from_optional_buffer(Some(buffer))
    }
    /// A null array fails while reading its length, before the source base
    /// constructor's separate zero-length rejection.
    pub fn from_optional_buffer(buffer: Option<FloatBuffer>) -> Result<Self, ImageError> {
        let buffer = buffer.ok_or(ImageError::NullReference)?;
        let length = buffer.len();
        if length == 0 {
            return Err(ImageError::Argument);
        }
        Ok(Self(Arc::new(FloatStoreCore {
            length,
            state: Mutex::new(FloatStoreState {
                buffer,
                disposed: false,
                sealed: false,
                temporary_seal: false,
            }),
        })))
    }
    /// Idempotent store disposal retains its array and fixed length. Disposing
    /// a containing ImageMap only releases references and never calls this.
    pub fn dispose(&self) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        state.disposed = true;
        state.sealed = false;
    }
    pub fn seal(&self, temporary: bool) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        if !state.sealed || state.temporary_seal {
            state.sealed = true;
            state.temporary_seal = temporary;
        }
    }
    pub fn is_temporarily_sealed(&self) -> bool {
        self.0
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .temporary_seal
    }
    pub fn is_sealed(&self) -> bool {
        self.0
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .sealed
    }
    pub fn data(&self, readonly: bool) -> Result<FloatBuffer, ImageError> {
        self.data_range(0, self.0.length as i32, readonly)
    }
    /// The disposed check precedes range arithmetic, validation and allocation.
    /// A complete range returns the retained caller array; a subset is copied.
    /// The readonly flag's seal/spill effects remain a separate capability.
    pub fn data_range(
        &self,
        start: i32,
        end: i32,
        readonly: bool,
    ) -> Result<FloatBuffer, ImageError> {
        let mut state = self
            .0
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if state.disposed {
            return Err(ImageError::Disposed);
        }
        if state.sealed && state.temporary_seal && !readonly {
            state.sealed = false;
        }
        let length = end.wrapping_sub(start);
        if start == 0 && end == self.0.length as i32 {
            return Ok(state.buffer.clone());
        }
        if length < 0 {
            return Err(ImageError::Overflow);
        }
        if start < 0 {
            return Err(ImageError::ArgumentOutOfRange);
        }
        state.buffer.copy_range(start as usize, length as usize)
    }
    // The original internal StorableFloatArray.a() exposes the raw field even
    // after disposal. Only internal alias inspection and private tests use it.
    fn array_reference(&self) -> FloatBuffer {
        self.0
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .buffer
            .clone()
    }
    pub fn len(&self) -> usize {
        self.0.length
    }
    pub fn is_empty(&self) -> bool {
        self.0.length == 0
    }
    /// Raw retained backing observation, including after disposal. This never
    /// invokes the source public data path or changes sealing/materialization.
    pub fn buffer_observation(&self) -> FloatBuffer {
        self.array_reference()
    }
    pub fn is_disposed(&self) -> bool {
        self.0
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .disposed
    }
    pub fn same_store(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    fn independent_copy(&self) -> Result<Self, ImageError> {
        // nz allocates its destination before attempting the source data read.
        let mut values = Vec::new();
        values
            .try_reserve_exact(self.len())
            .map_err(|_| ImageError::Allocation)?;
        let data = self.data(true)?;
        let data = data.0.lock().unwrap_or_else(|error| error.into_inner());
        values.extend_from_slice(data.values());
        Self::new(FloatBuffer::new(values))
    }
}

/// A named effect wrapper with independent wrapper, store and array identities.
#[derive(Clone, Debug)]
pub struct SharedNamedChannel(Arc<NamedChannelCore>);
#[derive(Debug)]
struct NamedChannelCore {
    name: Vec<u16>,
    data: Option<SharedFloatStore>,
}
impl SharedNamedChannel {
    pub fn new(name: Option<Vec<u16>>, data: Option<SharedFloatStore>) -> Self {
        let name = name
            .filter(|name| name.iter().any(|unit| !name_whitespace(*unit)))
            .unwrap_or_else(|| "Unnamed".encode_utf16().collect());
        Self(Arc::new(NamedChannelCore { name, data }))
    }
    pub fn name_utf16(&self) -> &[u16] {
        &self.0.name
    }
    pub fn data(&self) -> Option<SharedFloatStore> {
        self.0.data.clone()
    }
    pub fn same_wrapper(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
pub(crate) fn name_whitespace(unit: u16) -> bool {
    matches!(unit, 0x0009..=0x000d | 0x0020 | 0x0085 | 0x00a0 | 0x1680 |
        0x2000..=0x200a | 0x2028 | 0x2029 | 0x202f | 0x205f | 0x3000)
}

/// Explicit self-source preserves the original clear-before-read merge order.
#[derive(Clone, Copy, Debug)]
pub enum NamedChannelSource<'a> {
    Image(&'a ImageMap),
    Destination,
    Missing,
}

/// Source-compatible image metadata, including signed and wrapped coordinates.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ImageGeometry {
    width: i32,
    height: i32,
    x: i32,
    y: i32,
}

impl ImageGeometry {
    pub const fn new(width: i32, height: i32) -> Self {
        Self {
            width,
            height,
            x: 0,
            y: 0,
        }
    }
    pub const fn with_origin(mut self, x: i32, y: i32) -> Self {
        self.x = x;
        self.y = y;
        self
    }
    pub const fn width(self) -> i32 {
        self.width
    }
    pub const fn height(self) -> i32 {
        self.height
    }
    pub const fn origin(self) -> [i32; 2] {
        [self.x, self.y]
    }
    pub const fn maximum(self) -> [i32; 2] {
        [
            self.x.wrapping_add(self.width),
            self.y.wrapping_add(self.height),
        ]
    }
    pub const fn area(self) -> i32 {
        self.width.wrapping_mul(self.height)
    }
    fn without_origin(self) -> Self {
        Self::new(self.width, self.height)
    }
}

// Separate slot lengths and flags are observable after source failures and
// extended channel assignments. A single always-valid plane enum would lose
// the uses-map=true / missing-plane state after failed materialization.
#[derive(Debug)]
struct Channels {
    constants: Vec<Option<FloatBuffer>>,
    planes: Vec<Option<SharedFloatStore>>,
    materialized: Vec<bool>,
}
impl Default for Channels {
    fn default() -> Self {
        Self {
            constants: vec![None; 4],
            planes: vec![None; 4],
            materialized: vec![false; 4],
        }
    }
}

/// In-memory base channels, typed regions and ordered named effect channels.
/// Disk spill/sealing behavior remains unported.
#[derive(Debug)]
pub struct ImageMap {
    geometry: ImageGeometry,
    channels: Option<Channels>,
    region: Option<SharedFloatStore>,
    named: Option<Vec<Option<SharedNamedChannel>>>,
    anti_alias_type: i32,
    document_name: Option<Vec<u16>>,
    permanently_sealed: bool,
}

/// Read-only retained storage projection; it never calls a mutating image getter.
#[derive(Clone, Debug)]
pub struct ImageChannelObservation {
    constants: Option<Vec<Option<FloatBuffer>>>,
    planes: Option<Vec<Option<SharedFloatStore>>>,
    flags: Option<Vec<bool>>,
    named: Option<Vec<Option<SharedNamedChannel>>>,
}
impl ImageChannelObservation {
    pub fn constants(&self) -> Option<&[Option<FloatBuffer>]> {
        self.constants.as_deref()
    }
    pub fn planes(&self) -> Option<&[Option<SharedFloatStore>]> {
        self.planes.as_deref()
    }
    pub fn flags(&self) -> Option<&[bool]> {
        self.flags.as_deref()
    }
    pub fn named_channels(&self) -> Option<&[Option<SharedNamedChannel>]> {
        self.named.as_deref()
    }
}

pub(crate) enum RetainedNamedSource {
    Missing,
    Destination,
    Entries(Vec<Option<SharedNamedChannel>>),
}

impl ImageMap {
    pub fn channel_observation(&self) -> ImageChannelObservation {
        ImageChannelObservation {
            constants: self.channels.as_ref().map(|c| c.constants.clone()),
            planes: self.channels.as_ref().map(|c| c.planes.clone()),
            flags: self.channels.as_ref().map(|c| c.materialized.clone()),
            named: self.named.clone(),
        }
    }
    pub(crate) fn merge_retained_named_sources(
        &mut self,
        sources: [RetainedNamedSource; 3],
    ) -> Result<(), ImageError> {
        self.named.get_or_insert_with(Vec::new).clear();
        for (ix, source) in sources.into_iter().enumerate() {
            match source {
                RetainedNamedSource::Missing if ix == 0 => return Err(ImageError::NullReference),
                RetainedNamedSource::Missing => (),
                RetainedNamedSource::Destination => self
                    .named
                    .as_mut()
                    .ok_or(ImageError::NullReference)?
                    .extend_from_within(..),
                RetainedNamedSource::Entries(values) => self
                    .named
                    .as_mut()
                    .ok_or(ImageError::NullReference)?
                    .extend(values),
            }
        }
        self.deduplicate_named()
    }

    pub fn new(geometry: ImageGeometry) -> Self {
        Self {
            geometry,
            channels: Some(Channels::default()),
            region: None,
            named: None,
            anti_alias_type: 0,
            document_name: None,
            permanently_sealed: false,
        }
    }
    pub fn is_permanently_sealed(&self) -> bool {
        self.permanently_sealed
    }
    /// zl.o calls ImageMap.oc then nj on a newly computed (not reused cache) image.
    /// Disk-spill scheduling is separate; sealing here preserves the actual
    /// retained in-memory store state and never replaces caller arrays.
    pub(crate) fn finish_graph_render(&mut self) -> Result<(), ImageError> {
        if let Some(named) = &mut self.named {
            let excess = named.len().saturating_sub(10);
            named.drain(..excess);
        }
        self.seal_permanently()
    }
    /// ImageMap.nj alone. Unlike graph completion, this does not run oc or
    /// remove named channels. The flag commits before fallible store walks.
    pub fn seal_permanently(&mut self) -> Result<(), ImageError> {
        self.permanently_sealed = true;
        if let Some(channels) = &self.channels {
            for store in channels.planes.iter().flatten() {
                store.seal(false);
            }
        }
        if let Some(named) = &self.named {
            for entry in named {
                let entry = entry.as_ref().ok_or(ImageError::NullReference)?;
                entry.data().ok_or(ImageError::NullReference)?.seal(false);
            }
        }
        if let Some(region) = &self.region {
            region.seal(false);
        }
        Ok(())
    }
    pub const fn geometry(&self) -> ImageGeometry {
        self.geometry
    }
    pub fn is_disposed(&self) -> bool {
        self.channels.is_none()
    }
    pub fn dispose(&mut self) {
        if self.channels.take().is_some() {
            self.region = None;
            self.named = None;
        }
    }
    /// Actual ni: snapshot ng then test density in source order.
    pub(crate) fn uses_no_maps(&self) -> Result<bool, ImageError> {
        let count = self.channel_count()?;
        for channel in 0..count {
            if self.uses_plane(channel as i32)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
    /// Actual nk -> b(true): ordinary stores, named Data, then RegionsMap.
    pub(crate) fn seal_stores_temporarily(&self) -> Result<(), ImageError> {
        if let Some(state) = &self.channels {
            for store in state.planes.iter().flatten() {
                store.seal(true);
            }
        }
        if let Some(named) = &self.named {
            for named in named {
                named
                    .as_ref()
                    .ok_or(ImageError::NullReference)?
                    .data()
                    .ok_or(ImageError::NullReference)?
                    .seal(true);
            }
        }
        if let Some(region) = &self.region {
            region.seal(true);
        }
        Ok(())
    }
    pub fn geometry_copy(&self) -> Self {
        Self::new(self.geometry)
    }

    fn state(&self) -> Result<&Channels, ImageError> {
        self.channels.as_ref().ok_or(ImageError::Disposed)
    }
    fn state_mut(&mut self) -> Result<&mut Channels, ImageError> {
        self.channels.as_mut().ok_or(ImageError::Disposed)
    }
    fn index(index: i32) -> Result<usize, ImageError> {
        usize::try_from(index).map_err(|_| ImageError::Index)
    }

    pub fn uses_plane(&self, channel: i32) -> Result<bool, ImageError> {
        let state = self.state()?;
        Ok(state
            .materialized
            .get(Self::index(channel)?)
            .copied()
            .unwrap_or(false))
    }
    fn constant_value(&self, channel: usize) -> Result<f32, ImageError> {
        match self
            .state()?
            .constants
            .get(channel)
            .and_then(Option::as_ref)
        {
            Some(buffer) => buffer.read(0),
            None => Ok(0.0),
        }
    }
    pub fn channel_count(&self) -> Result<usize, ImageError> {
        let state = self.state()?;
        let mut count = state.constants.len().max(state.materialized.len());
        while count > 0
            && !self.uses_plane((count - 1) as i32)?
            && self.constant_value(count - 1)? == 0.0
        {
            count -= 1;
        }
        Ok(count)
    }
    pub fn is_empty(&self) -> Result<bool, ImageError> {
        Ok(self.channel_count()? == 0)
    }

    fn grow<T: Clone>(values: &mut Vec<T>, length: usize, value: T) -> Result<(), ImageError> {
        if length > values.len() {
            // The source adds one to a signed i32 channel before allocation.
            // Its wrapped negative length is rejected before touching slots.
            if length > i32::MAX as usize {
                return Err(ImageError::ArgumentOutOfRange);
            }
            values
                .try_reserve_exact(length - values.len())
                .map_err(|_| ImageError::Allocation)?;
            values.resize(length, value);
        }
        Ok(())
    }
    fn mark_plane(&mut self, channel: usize, used: bool) -> Result<(), ImageError> {
        let state = self.state_mut()?;
        if channel >= state.materialized.len() {
            if !used {
                return Ok(());
            }
            Self::grow(&mut state.materialized, channel + 1, false)?;
        }
        state.materialized[channel] = used;
        if !used {
            *state.planes.get_mut(channel).ok_or(ImageError::Index)? = None;
        }
        Ok(())
    }
    fn constant_handle(&mut self, channel: usize) -> Result<FloatBuffer, ImageError> {
        let state = self.state_mut()?;
        Self::grow(&mut state.constants, channel + 1, None)?;
        Ok(state.constants[channel]
            .get_or_insert_with(|| FloatBuffer::new(vec![0.0]))
            .clone())
    }
    pub fn set_constant(&mut self, channel: i32, value: f32) -> Result<(), ImageError> {
        self.state()?;
        let channel = Self::index(channel)?;
        self.mark_plane(channel, false)?;
        if channel >= self.state()?.constants.len() && value == 0.0 {
            return Ok(());
        }
        self.constant_handle(channel)?.write(0, value)
    }
    pub fn set_array(&mut self, channel: i32, values: Vec<f32>) -> Result<(), ImageError> {
        if values.is_empty() {
            return Err(ImageError::Argument);
        }
        self.assign_buffer(channel, FloatBuffer::new(values))
    }
    /// Retain the caller's dense storage. A one-element buffer is copied as a
    /// constant; an empty buffer is rejected before checking image disposal.
    pub fn set_buffer(&mut self, channel: i32, buffer: &FloatBuffer) -> Result<(), ImageError> {
        if buffer.is_empty() {
            return Err(ImageError::Argument);
        }
        self.assign_buffer(channel, buffer.clone())
    }
    fn assign_buffer(&mut self, channel: i32, buffer: FloatBuffer) -> Result<(), ImageError> {
        if buffer.len() == 1 {
            return self.set_constant(channel, buffer.read(0)?);
        }
        self.assign_store(channel, SharedFloatStore::new(buffer)?)
    }
    pub fn set_store(&mut self, channel: i32, store: &SharedFloatStore) -> Result<(), ImageError> {
        self.assign_store(channel, store.clone())
    }
    fn assign_store(&mut self, channel: i32, store: SharedFloatStore) -> Result<(), ImageError> {
        if store.len() == 1 {
            return self.set_constant(channel, store.data(true)?.read(0)?);
        }
        // Original nv accesses the cleared maps field directly after disposal.
        let state = self.channels.as_mut().ok_or(ImageError::NullReference)?;
        let channel = Self::index(channel)?;
        Self::grow(&mut state.planes, channel + 1, None)?;
        state.planes[channel] = Some(store);
        self.mark_plane(channel, true)
    }
    fn ensure_plane(&mut self, channel: i32) -> Result<SharedFloatStore, ImageError> {
        self.state()?;
        let index = Self::index(channel)?;
        if !self.uses_plane(channel)?
            || self
                .state()?
                .planes
                .get(index)
                .and_then(Option::as_ref)
                .is_none()
        {
            self.mark_plane(index, true)?;
            let value = self.constant_value(index)?;
            let area = self.geometry.area();
            let state = self.state_mut()?;
            Self::grow(&mut state.planes, index + 1, None)?;
            if area < 0 {
                return Err(ImageError::Overflow);
            }
            if area == 0 {
                return Err(ImageError::Argument);
            }
            let length = area as usize;
            let mut values = Vec::new();
            values
                .try_reserve_exact(length)
                .map_err(|_| ImageError::Allocation)?;
            // The source skips filling zero, including negative zero.
            values.resize(length, if value == 0.0 { 0.0 } else { value });
            state.planes[index] = Some(SharedFloatStore::new(FloatBuffer::new(values))?);
        }
        self.state()?
            .planes
            .get(index)
            .and_then(Option::as_ref)
            .cloned()
            .ok_or(ImageError::NullReference)
    }
    pub fn channel(&mut self, channel: i32, readonly: bool) -> Result<FloatBuffer, ImageError> {
        if channel >= 4 {
            return Err(ImageError::Argument);
        }
        self.state()?;
        if self.uses_plane(channel)? {
            self.range_with_readonly(channel, 0, self.geometry.area(), readonly)
        } else {
            self.constant_handle(Self::index(channel)?)
        }
    }
    pub fn materialize(&mut self, channel: i32) -> Result<FloatBuffer, ImageError> {
        self.range(channel, 0, self.geometry.area())
    }
    pub fn range(&mut self, channel: i32, start: i32, end: i32) -> Result<FloatBuffer, ImageError> {
        self.range_with_readonly(channel, start, end, false)
    }
    fn range_with_readonly(
        &mut self,
        channel: i32,
        start: i32,
        end: i32,
        readonly: bool,
    ) -> Result<FloatBuffer, ImageError> {
        self.ensure_plane(channel)?.data_range(start, end, readonly)
    }
    pub fn has_transparency(&mut self) -> Result<bool, ImageError> {
        if self.uses_plane(3)? {
            Ok(true)
        } else {
            Ok(f64::from(self.channel(3, true)?.read(0)?.abs()) > 1e-7)
        }
    }
    pub fn reset(&mut self) -> Result<(), ImageError> {
        let count = self.channel_count()?;
        for channel in 0..count {
            self.set_constant(channel as i32, 0.0)?;
        }
        self.named = None;
        self.region = None;
        Ok(())
    }
    pub fn swap_channels(&mut self, first: i32, second: i32) -> Result<(), ImageError> {
        self.state()?;
        let first_ix = Self::index(first)?;
        let second_ix = Self::index(second)?;
        let state = self.state_mut()?;
        if first_ix >= state.planes.len() || second_ix >= state.planes.len() {
            return Err(ImageError::Index);
        }
        state.planes.swap(first_ix, second_ix);
        let first_point = state.constants.get(first_ix).cloned().flatten();
        let second_point = state.constants.get(second_ix).cloned().flatten();
        if first_ix >= state.constants.len() && second_point.is_some() {
            Self::grow(&mut state.constants, first_ix + 1, None)?;
        }
        if first_ix < state.constants.len() {
            state.constants[first_ix] = second_point;
        }
        if second_ix >= state.constants.len() && first_point.is_some() {
            Self::grow(&mut state.constants, second_ix + 1, None)?;
        }
        if second_ix < state.constants.len() {
            state.constants[second_ix] = first_point;
        }
        let first_used = self.uses_plane(first)?;
        let second_used = self.uses_plane(second)?;
        self.mark_plane(first_ix, second_used)?;
        self.mark_plane(second_ix, first_used)
    }
    pub fn copy_channel_from(
        &mut self,
        destination: i32,
        source: &mut Self,
        channel: i32,
    ) -> Result<(), ImageError> {
        if source.uses_plane(channel)? {
            source
                .state()?
                .planes
                .get(Self::index(channel)?)
                .and_then(Option::as_ref)
                .cloned()
                .ok_or(ImageError::NullReference)
                .and_then(|store| self.assign_store(destination, store))
        } else {
            self.assign_buffer(destination, source.channel(channel, false)?)
        }
    }
    pub fn copy_channel_within(&mut self, destination: i32, source: i32) -> Result<(), ImageError> {
        if self.uses_plane(source)? {
            self.state()?
                .planes
                .get(Self::index(source)?)
                .and_then(Option::as_ref)
                .cloned()
                .ok_or(ImageError::NullReference)
                .and_then(|store| self.assign_store(destination, store))
        } else {
            let buffer = self.channel(source, false)?;
            self.assign_buffer(destination, buffer)
        }
    }
    pub fn shallow_copy(&mut self) -> Result<Self, ImageError> {
        let mut result = Self::new(self.geometry.without_origin());
        let count = self.channel_count()?;
        for channel in 0..count {
            let channel = channel as i32;
            if self.uses_plane(channel)? {
                self.state()?
                    .planes
                    .get(channel as usize)
                    .and_then(Option::as_ref)
                    .cloned()
                    .ok_or(ImageError::NullReference)
                    .and_then(|store| result.assign_store(channel, store))?;
            } else {
                result.assign_buffer(channel, self.channel(channel, true)?)?;
            }
        }
        result.region = self.region.clone();
        result.replace_named_channels_from(self)?;
        result.anti_alias_type = self.anti_alias_type;
        result.document_name = self.document_name.clone();
        Ok(result)
    }
    pub fn deep_copy(&mut self) -> Result<Self, ImageError> {
        let mut result = Self::new(self.geometry.without_origin());
        let count = self.channel_count()?;
        for channel in 0..count {
            let buffer = self.channel(channel as i32, true)?;
            result.assign_buffer(channel as i32, buffer.copy_range(0, buffer.len())?)?;
        }
        if let Some(named) = &self.named {
            for entry in named {
                let entry = entry.as_ref().ok_or(ImageError::NullReference)?;
                let store = entry
                    .data()
                    .ok_or(ImageError::NullReference)?
                    .independent_copy()?;
                result.add_named_channel(SharedNamedChannel::new(
                    Some(entry.name_utf16().to_vec()),
                    Some(store),
                ))?;
            }
        }
        result.region = self
            .region
            .as_ref()
            .map(SharedFloatStore::independent_copy)
            .transpose()?;
        result.anti_alias_type = self.anti_alias_type;
        result.document_name = self.document_name.clone();
        Ok(result)
    }
    pub fn region_store(&self) -> Option<SharedFloatStore> {
        self.region.clone()
    }
    pub fn set_region_store(&mut self, region: Option<SharedFloatStore>) {
        self.region = region;
    }
    pub const fn anti_alias_type(&self) -> i32 {
        self.anti_alias_type
    }
    pub fn set_anti_alias_type(&mut self, value: i32) {
        self.anti_alias_type = value;
    }
    pub fn document_name_utf16(&self) -> Option<&[u16]> {
        self.document_name.as_deref()
    }
    pub fn set_document_name_utf16(&mut self, value: Option<Vec<u16>>) {
        self.document_name = value;
    }
    /// Fresh container, retaining wrapper identities and any failed null append.
    /// Actual ImageMap.NumAllChannels; independent of disposal/plane use.
    pub fn all_channel_count(&self) -> i32 {
        4i32.wrapping_add(self.named.as_ref().map_or(0, |v| v.len() as i32))
    }
    pub fn named_channels(&self) -> Vec<Option<SharedNamedChannel>> {
        self.named.clone().unwrap_or_default()
    }
    fn deduplicate_named(&mut self) -> Result<(), ImageError> {
        let named = self.named.as_mut().ok_or(ImageError::NullReference)?;
        let mut first = 0;
        while first < named.len() {
            let data = named[first]
                .as_ref()
                .ok_or(ImageError::NullReference)?
                .data();
            let mut next = first + 1;
            while next < named.len() {
                let other = named[next]
                    .as_ref()
                    .ok_or(ImageError::NullReference)?
                    .data();
                let same = match (&data, &other) {
                    (Some(a), Some(b)) => a.same_store(b),
                    (None, None) => true,
                    _ => false,
                };
                if same {
                    named.remove(next);
                } else {
                    next += 1;
                }
            }
            first += 1;
        }
        Ok(())
    }
    pub fn add_named_channel(&mut self, channel: SharedNamedChannel) -> Result<(), ImageError> {
        self.add_optional_named_channel(Some(channel))
    }
    pub fn add_optional_named_channel(
        &mut self,
        channel: Option<SharedNamedChannel>,
    ) -> Result<(), ImageError> {
        self.named.get_or_insert_with(Vec::new).push(channel);
        self.deduplicate_named()
    }
    pub fn add_named_store(
        &mut self,
        name: Option<Vec<u16>>,
        store: Option<SharedFloatStore>,
    ) -> Result<(), ImageError> {
        self.add_named_channel(SharedNamedChannel::new(name, store))
    }
    pub fn replace_named_channels_from(&mut self, source: &Self) -> Result<(), ImageError> {
        self.merge_named_channels([
            NamedChannelSource::Image(source),
            NamedChannelSource::Missing,
            NamedChannelSource::Missing,
        ])
    }
    pub fn merge_named_channels(
        &mut self,
        sources: [NamedChannelSource<'_>; 3],
    ) -> Result<(), ImageError> {
        self.named.get_or_insert_with(Vec::new).clear();
        for (index, source) in sources.into_iter().enumerate() {
            match source {
                NamedChannelSource::Missing if index == 0 => return Err(ImageError::NullReference),
                NamedChannelSource::Missing => (),
                NamedChannelSource::Image(image) => {
                    if let Some(named) = &image.named {
                        self.named
                            .as_mut()
                            .ok_or(ImageError::NullReference)?
                            .extend_from_slice(named);
                    }
                }
                NamedChannelSource::Destination => {
                    let named = self.named.as_mut().ok_or(ImageError::NullReference)?;
                    // List.AddRange(this) duplicates the list at this point, including
                    // earlier sources; this is not a snapshot from before Clear.
                    named.extend_from_within(..);
                }
            }
        }
        self.deduplicate_named()
    }
    pub fn named_channel(&self, index: i32) -> Result<Option<SharedNamedChannel>, ImageError> {
        let Some(named) = &self.named else {
            return Ok(None);
        };
        if index >= named.len() as i32 {
            return Ok(None);
        }
        let index = usize::try_from(index).map_err(|_| ImageError::ArgumentOutOfRange)?;
        Ok(named[index].clone())
    }
    pub fn channel_with_effects(
        &mut self,
        index: i32,
        readonly: bool,
    ) -> Result<FloatBuffer, ImageError> {
        if index < 4 {
            return self.channel(index, readonly);
        }
        self.named_channel(index - 4)?
            .ok_or(ImageError::NullReference)?
            .data()
            .ok_or(ImageError::NullReference)
            .and_then(|store| store.data(readonly))
    }
    /// Source ImageMap.nl: temporarily seal an existing base store, or the
    /// selected named store. Named index/data failures are not silently skipped.
    pub fn seal_channel(&self, index: i32) -> Result<(), ImageError> {
        if index < 4 {
            // nl dereferences maps directly rather than using the disposed
            // public-channel gate. A disposed image has a null maps field.
            let planes = &self
                .channels
                .as_ref()
                .ok_or(ImageError::NullReference)?
                .planes;
            let index = usize::try_from(index).map_err(|_| ImageError::Index)?;
            if let Some(Some(store)) = planes.get(index) {
                store.seal(true);
            }
        } else {
            let named = self.named.as_ref().ok_or(ImageError::NullReference)?;
            let channel = named
                .get((index - 4) as usize)
                .ok_or(ImageError::ArgumentOutOfRange)?
                .as_ref()
                .ok_or(ImageError::NullReference)?;
            channel.data().ok_or(ImageError::NullReference)?.seal(true);
        }
        Ok(())
    }
    /// ImageMap.nm: seal each actual named store in list order.
    pub(crate) fn seal_named_channels(&self) -> Result<(), ImageError> {
        let named = self.named.as_ref().ok_or(ImageError::NullReference)?;
        for entry in named {
            entry
                .as_ref()
                .ok_or(ImageError::NullReference)?
                .data()
                .ok_or(ImageError::NullReference)?
                .seal(true);
        }
        Ok(())
    }
    /// ImageMap.i, inspecting existing raw stores without materializing channels.
    pub(crate) fn compress_uniform_channels(&mut self) -> Result<(), ImageError> {
        let count = self.channel_count()?;
        for channel in 0..count {
            let store = self
                .channels
                .as_ref()
                .ok_or(ImageError::NullReference)?
                .planes
                .get(channel)
                .ok_or(ImageError::Index)?
                .clone();
            if let Some(store) = store {
                let array = store.array_reference();
                let first = array.read(0)?;
                let mut end = 1;
                while end < array.len() && array.read(end)? == first {
                    end += 1;
                }
                if end == array.len() {
                    self.set_constant(channel as i32, first)?;
                }
            }
        }
        Ok(())
    }
    pub fn clear_named_channels(&mut self) {
        if let Some(named) = &mut self.named {
            named.clear();
        }
    }
    /// Source ImageMap.ob removes first, then adjusts/removes second. Failure
    /// in the second removal preserves the first mutation.
    pub fn remove_two_named_channels(&mut self, first: i32, second: i32) -> Result<(), ImageError> {
        if self.named.is_none() {
            return Ok(());
        }
        self.remove_named_channel(first)?;
        if first != second {
            self.remove_named_channel(if first < second {
                second.wrapping_sub(1)
            } else {
                second
            })?;
        }
        Ok(())
    }
    pub fn remove_named_channel(&mut self, index: i32) -> Result<(), ImageError> {
        if let Some(named) = &mut self.named {
            let index = usize::try_from(index).map_err(|_| ImageError::ArgumentOutOfRange)?;
            if index >= named.len() {
                return Err(ImageError::ArgumentOutOfRange);
            }
            named.remove(index);
        }
        Ok(())
    }
    pub fn remove_named_pair(&mut self, first: i32, second: i32) -> Result<(), ImageError> {
        self.remove_named_channel(first)?;
        if first != second {
            self.remove_named_channel(if first < second {
                second.wrapping_sub(1)
            } else {
                second
            })?;
        }
        Ok(())
    }
    pub fn earlier_channel_alias(&self, channel: i32) -> Result<Option<usize>, ImageError> {
        let state = self.channels.as_ref().ok_or(ImageError::NullReference)?;
        let channel = Self::index(channel)?;
        let store = state.planes.get(channel).ok_or(ImageError::Index)?;
        if let Some(store) = store {
            for index in 0..channel {
                if let Some(previous) = &state.planes[index]
                    && (previous.same_store(store)
                        || previous
                            .array_reference()
                            .shares_storage(&store.array_reference()))
                {
                    return Ok(Some(index));
                }
            }
        }
        Ok(None)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImageError {
    Disposed,
    Index,
    Argument,
    ArgumentOutOfRange,
    Overflow,
    NullReference,
    Allocation,
}
impl ImageError {
    pub const fn source_class(self) -> &'static str {
        match self {
            Self::Disposed => "ObjectDisposedException",
            Self::Index => "IndexOutOfRangeException",
            Self::Argument => "ArgumentException",
            Self::ArgumentOutOfRange => "ArgumentOutOfRangeException",
            Self::Overflow => "OverflowException",
            Self::NullReference => "NullReferenceException",
            Self::Allocation => "OutOfMemoryException",
        }
    }
}
impl fmt::Display for ImageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "image operation failed: {}", self.source_class())
    }
}
impl Error for ImageError {}

#[cfg(test)]
#[path = "image_map_test.rs"]
mod test;

#[cfg(test)]
#[path = "image_auxiliary_edges_test.rs"]
mod auxiliary_edges_test;
#[cfg(test)]
#[path = "store_lifecycle_test.rs"]
mod store_lifecycle_test;

#[path = "image_capture.rs"]
mod capture;
pub(crate) use capture::{ImageCapture, ImageValues, RawImage};

#[cfg(test)]
#[test]
fn a_read_view_keeps_its_pixels_when_a_writable_alias_changes() {
    let image = FloatBuffer::new(vec![0.25, -0.5]);
    let alias = image.clone();
    let original = image.shared_values();
    assert!(Arc::ptr_eq(&original, &alias.shared_values()));
    alias.write_block(0, 1, &[3., 4.]).unwrap();
    assert_eq!(&**original, &[0.25, -0.5]);
    assert_eq!(image.values(), vec![3., 4.]);
}
