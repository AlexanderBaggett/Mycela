//! Actual in-memory StorableArray<T> owners and independently retained CLR-array
//! buffers for the three reached mesh element ABIs. Spill/registration is not a
//! successful substitute: its reached constructor branch stays unsupported.
use super::*;
mod element {
    pub trait Sealed {}
}
/// Closed mesh ABIs covered by this source slice; external element layouts are
/// not silently accepted as original StorableArray<T> structures.
pub trait MeshArrayElement: element::Sealed + Copy + Default {
    const KIND: SceneArrayKind;
}
impl element::Sealed for i32 {}
impl MeshArrayElement for i32 {
    const KIND: SceneArrayKind = SceneArrayKind::IndexI32;
}
impl element::Sealed for MeshVector {}
impl MeshArrayElement for MeshVector {
    const KIND: SceneArrayKind = SceneArrayKind::Vector3F;
}
impl element::Sealed for MeshUv {}
impl MeshArrayElement for MeshUv {
    const KIND: SceneArrayKind = SceneArrayKind::UvF32;
}
/// Fixed-length actual underlying array. Store sealing/disposal never revokes an
/// already-returned buffer; reads/writes act on this same independently held root.
pub struct MeshBuffer<T>(Arc<Mutex<Box<[T]>>>);
impl<T> Clone for MeshBuffer<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl<T> fmt::Debug for MeshBuffer<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MeshBuffer({:p})", Arc::as_ptr(&self.0))
    }
}
impl<T: MeshArrayElement> MeshBuffer<T> {
    pub fn from_values(values: Vec<T>) -> Self {
        Self(Arc::new(Mutex::new(values.into_boxed_slice())))
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
    pub fn read(&self, index: i32) -> Result<T, SceneError> {
        guard(&self.0)
            .get(index as usize)
            .copied()
            .ok_or(SceneError::IndexOutOfRange)
    }
    pub fn write(&self, index: i32, value: T) -> Result<(), SceneError> {
        *guard(&self.0)
            .get_mut(index as usize)
            .ok_or(SceneError::IndexOutOfRange)? = value;
        Ok(())
    }
    pub fn try_values(&self) -> Result<Vec<T>, SceneError> {
        let source = guard(&self.0);
        let mut values = Vec::new();
        values
            .try_reserve_exact(source.len())
            .map_err(|_| SceneError::Allocation)?;
        values.extend_from_slice(&source);
        Ok(values)
    }
    pub(super) fn update(&self, mut operation: impl FnMut(T) -> T) {
        for value in guard(&self.0).iter_mut() {
            *value = operation(*value);
        }
    }
}
struct StoreState<T> {
    buffer: MeshBuffer<T>,
    disposed: bool,
    sealed: bool,
    temporary: bool,
}
struct StoreCore<T> {
    length: i32,
    state: Mutex<StoreState<T>>,
}
/// One real StorableArray<T> owner. Its identity, fixed ArrayLength, two seal
/// flags and disposed flag are independent of the underlying array identity.
pub struct MeshArray<T>(Arc<StoreCore<T>>);
impl<T> Clone for MeshArray<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl<T> fmt::Debug for MeshArray<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MeshArray({:p})", Arc::as_ptr(&self.0))
    }
}
impl<T: MeshArrayElement> MeshArray<T> {
    /// Reads null-array length before the base constructor's zero check.
    pub fn from_optional_buffer(buffer: Option<MeshBuffer<T>>) -> Result<Self, SceneError> {
        let buffer = buffer.ok_or(SceneError::NullReference)?;
        let length = i32::try_from(buffer.len()).map_err(|_| SceneError::Allocation)?;
        if length == 0 {
            return Err(SceneError::ZeroLengthArray);
        }
        // Exact i(): floor(sqrt(ArrayLength))>1150. Actual arrays already exist
        // before this registration frontier; no successful missing host service.
        if length >= 1_324_801 {
            return Err(SceneError::MissingArrayStorageServices);
        }
        Ok(Self(Arc::new(StoreCore {
            length,
            state: Mutex::new(StoreState {
                buffer,
                disposed: false,
                sealed: false,
                temporary: false,
            }),
        })))
    }
    pub fn from_buffer(buffer: MeshBuffer<T>) -> Result<Self, SceneError> {
        Self::from_optional_buffer(Some(buffer))
    }
    pub fn same_reference(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    pub fn array_length(&self) -> i32 {
        self.0.length
    }
    pub fn is_disposed(&self) -> bool {
        guard(&self.0.state).disposed
    }
    pub fn is_sealed(&self) -> bool {
        guard(&self.0.state).sealed
    }
    /// Raw source _isSealedTemporarily, including true with IsSealed=false.
    pub fn is_temporarily_sealed(&self) -> bool {
        guard(&self.0.state).temporary
    }
    pub fn seal(&self, temporary: bool) {
        let mut state = guard(&self.0.state);
        if !state.sealed || state.temporary {
            state.sealed = true;
            state.temporary = temporary;
        }
    }
    pub fn dispose(&self) {
        let mut state = guard(&self.0.state);
        if !state.disposed {
            state.disposed = true;
            state.sealed = false;
        }
        // Ordinary ArrayReference, fixed length and temporary bit remain held.
        // No file was produced: stored states remain an explicit capability.
    }
    pub fn data(&self, readonly: bool) -> Result<MeshBuffer<T>, SceneError> {
        let mut state = guard(&self.0.state);
        if state.disposed {
            return Err(SceneError::Disposed);
        }
        if state.sealed && state.temporary && !readonly {
            state.sealed = false;
        }
        Ok(state.buffer.clone())
    }
    /// Fallible readonly snapshot over the same retained source buffer. It does
    /// not source-copy, invent an owner, unseal, or invoke host policy callbacks.
    pub fn try_snapshot(&self) -> Result<Vec<T>, SceneError> {
        self.data(true)?.try_values()
    }
    pub fn source_copy(&self) -> Result<Self, SceneError> {
        let _was_sealed = self.is_sealed();
        let _buffer = self.data(true)?;
        Err(SceneError::MissingAllocationServices)
    }
    pub fn source_copy_with_effects(
        &self,
        effects: &mut SceneAllocationEffects<'_>,
    ) -> Result<Self, SceneError> {
        let was_sealed = self.is_sealed();
        let buffer = self.data(true)?;
        let length = buffer.len();
        // Source retains this exact buffer before SafelyCreateArray. A policy
        // callback can edit it or dispose the store without revoking that root.
        effects.create::<T>(length, T::KIND)?;
        let mut values = Vec::new();
        values
            .try_reserve_exact(length)
            .map_err(|_| SceneError::Allocation)?;
        values.resize(length, T::default());
        for (target, value) in values.iter_mut().zip(guard(&buffer.0).iter()) {
            *target = *value;
        }
        if was_sealed {
            self.seal(false);
        }
        let copy = Self::from_buffer(MeshBuffer::from_values(values))?;
        copy.seal(true);
        Ok(copy)
    }
    pub(super) fn from_values(values: Vec<T>, seal: MeshSeal) -> Result<Self, SceneError> {
        let root = Self::from_buffer(MeshBuffer::from_values(values))?;
        match seal {
            MeshSeal::Open => {}
            MeshSeal::Temporary => root.seal(true),
            MeshSeal::Permanent => root.seal(false),
        }
        Ok(root)
    }
    pub(super) fn len(&self) -> usize {
        self.0.length as usize
    }
    pub(super) fn seal_state(&self) -> MeshSeal {
        let state = guard(&self.0.state);
        if !state.sealed {
            MeshSeal::Open
        } else if state.temporary {
            MeshSeal::Temporary
        } else {
            MeshSeal::Permanent
        }
    }
}
pub type MeshVectorStore = MeshArray<MeshVector>;
pub type MeshUvStore = MeshArray<MeshUv>;
pub type MeshIndexStore = MeshArray<i32>;
pub type MeshVectorBuffer = MeshBuffer<MeshVector>;
pub type MeshUvBuffer = MeshBuffer<MeshUv>;
pub type MeshIndexBuffer = MeshBuffer<i32>;
#[cfg(test)]
mod laws;
