//! Caller Byte[] authority and the actual auv StorableArray lifecycle.
use crate::BinaryError;
use std::{
    collections::HashMap,
    fmt,
    hash::BuildHasher,
    sync::{Arc, Mutex, MutexGuard, Weak},
};

fn guard<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|error| error.into_inner())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImportedImageError {
    Index,
    ArgumentOutOfRange,
    Allocation,
    CapacityQuery,
    Overflow,
    Io(std::io::ErrorKind),
    Argument,
    ArgumentNull,
    NullReference,
    ObjectDisposed,
    Binary(BinaryError),
    UnsupportedCapability(&'static str),
}
impl ImportedImageError {
    pub const fn source_class(self) -> &'static str {
        match self {
            Self::Index => "IndexOutOfRangeException",
            Self::ArgumentOutOfRange => "ArgumentOutOfRangeException",
            Self::Allocation => "OutOfMemoryException",
            Self::CapacityQuery => "CapacityQueryError",
            Self::Overflow => "OverflowException",
            Self::Io(kind) => match kind {
                std::io::ErrorKind::NotFound => "FileNotFoundException",
                std::io::ErrorKind::PermissionDenied => "UnauthorizedAccessException",
                _ => "IOException",
            },
            Self::Argument => "ArgumentException",
            Self::ArgumentNull => "ArgumentNullException",
            Self::NullReference => "NullReferenceException",
            Self::ObjectDisposed => "ObjectDisposedException",
            Self::Binary(error) => error.source_class(),
            Self::UnsupportedCapability(_) => "unported-capability",
        }
    }
}
impl From<std::io::Error> for ImportedImageError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.kind())
    }
}
impl From<BinaryError> for ImportedImageError {
    fn from(error: BinaryError) -> Self {
        Self::Binary(error)
    }
}
impl fmt::Display for ImportedImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Imported image {self:?}: {}", self.source_class())
    }
}
impl std::error::Error for ImportedImageError {}

/// Fixed-length mutable Byte[] authority. Clone shares the real caller array;
/// values() is an observation copy and never replaces that authority.
#[derive(Clone)]
pub struct ByteBuffer(Arc<Mutex<Arc<Vec<u8>>>>);
impl fmt::Debug for ByteBuffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ByteBuffer({:p})", Arc::as_ptr(&self.0))
    }
}
impl ByteBuffer {
    pub fn new(values: Vec<u8>) -> Self {
        Self(Arc::new(Mutex::new(Arc::new(values))))
    }
    /// A distinct array authority over an immutable borrowed data generation.
    pub(crate) fn frozen_copy(&self) -> Self {
        Self(Arc::new(Mutex::new(self.generation())))
    }
    pub(crate) fn generation(&self) -> Arc<Vec<u8>> {
        guard(&self.0).clone()
    }
    pub fn same_reference(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    pub fn len(&self) -> usize {
        guard(&self.0).len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn values(&self) -> Vec<u8> {
        guard(&self.0).as_ref().clone()
    }
    pub fn read(&self, ix: usize) -> Result<u8, ImportedImageError> {
        guard(&self.0)
            .get(ix)
            .copied()
            .ok_or(ImportedImageError::Index)
    }
    pub fn write(&self, ix: usize, value: u8) -> Result<(), ImportedImageError> {
        let mut values = guard(&self.0);
        if ix >= values.len() {
            return Err(ImportedImageError::Index);
        }
        Arc::make_mut(&mut values)[ix] = value;
        Ok(())
    }
}

/// Shares immutable storage during one document load. Each ByteBuffer keeps
/// its own mutable authority, and the index does not retain removed data.
#[derive(Default)]
pub(crate) struct EncodedImageGenerations {
    entries: HashMap<u64, Vec<Weak<Vec<u8>>>>,
}

impl EncodedImageGenerations {
    pub(crate) fn share(&mut self, bytes: &ByteBuffer) {
        let generation = bytes.generation();
        let hash = self.entries.hasher().hash_one(generation.as_slice());
        let candidates = self.entries.entry(hash).or_default();
        candidates.retain(|entry| entry.strong_count() > 0);
        if let Some(matched) = candidates
            .iter()
            .filter_map(Weak::upgrade)
            .find(|candidate| candidate.as_slice() == generation.as_slice())
        {
            let mut current = guard(&bytes.0);
            if Arc::ptr_eq(&current, &generation) {
                *current = matched;
            }
        } else {
            candidates.push(Arc::downgrade(&generation));
        }
    }
}

mod storage;
pub use storage::{
    EncodedByteStore, EncodedStorageEffects, EncodedStorageEnvironment, EncodedStorageNotice,
    EncodedStorageSnapshot,
};

/// The actual two BinaryWriter calls made by mn.Save. Byte-array commands carry
/// the live root, allowing caller streams to observe aliases during their write.
#[derive(Clone, Copy, Debug)]
pub enum ImportedImageWrite<'a> {
    Length(i32),
    Bytes(&'a ByteBuffer),
}
/// Payload retrieval and typed caller writer failures stay separate.
#[derive(Debug)]
pub enum ImportedImageSaveError<W> {
    Image(ImportedImageError),
    Writer(W),
}
