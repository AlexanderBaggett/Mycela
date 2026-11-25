//! Retained source-shaped scene list roots. The list owns its actual elements;
//! an outward handle sponsors the scene whose internal Changed listener runs
//! before listeners registered directly on the list.
use super::{Scene, SceneChange, SceneError, guard};
use std::{
    fmt,
    sync::{Arc, Mutex},
};

type Callback<T> = dyn Fn(&SceneList<T>) -> Result<(), SceneError> + Send + Sync;

pub struct SceneListHandler<T>(Arc<Callback<T>>);
impl<T> Clone for SceneListHandler<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl<T> SceneListHandler<T> {
    pub fn new(
        handler: impl Fn(&SceneList<T>) -> Result<(), SceneError> + Send + Sync + 'static,
    ) -> Self {
        Self(Arc::new(handler))
    }
    pub fn same_reference(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

pub(super) struct ListData<T> {
    values: Vec<T>,
    handlers: Vec<SceneListHandler<T>>,
    revision: u64,
    source_capacity: usize,
}
impl<T> ListData<T> {
    pub(super) fn new(values: Vec<T>) -> Self {
        // nt() creates capacity 20; nh.a installs each generated array with
        // ChangableList's batch-add overload, not repeated single Add calls.
        let source_capacity = 20.max(values.len());
        Self::with_capacity(values, source_capacity)
    }
    pub(super) fn copied(values: Vec<T>) -> Self {
        let source_capacity = Self::capacity_after_adds(values.len(), values.len());
        Self::with_capacity(values, source_capacity)
    }
    fn capacity_after_adds(mut capacity: usize, count: usize) -> usize {
        for len in 0..count {
            capacity = capacity.max(len.saturating_add((capacity / 3).max(2)));
        }
        capacity
    }
    fn with_capacity(values: Vec<T>, source_capacity: usize) -> Self {
        Self {
            values,
            handlers: Vec::new(),
            revision: 0,
            source_capacity,
        }
    }
    fn reserve_source(&mut self, extra: usize, actual_add: usize) -> Result<(), SceneError> {
        let wanted = self
            .values
            .len()
            .checked_add(extra)
            .ok_or(SceneError::Allocation)?;
        let capacity = self.source_capacity.max(wanted);
        // Rust may reserve a different physical size; this field tracks the
        // source-visible Capacity, not an allocator identity or failure profile.
        let additional = capacity.saturating_sub(self.values.len()).max(actual_add);
        self.values
            .try_reserve_exact(additional)
            .map_err(|_| SceneError::Allocation)?;
        self.source_capacity = capacity;
        Ok(())
    }
}

/// One actual retained `ChangableList`-shaped owner. Enumeration is a snapshot
/// of the current roots, distinct from subsequent mutations of this list.
pub struct SceneList<T> {
    pub(super) core: Arc<Mutex<ListData<T>>>,
    pub(super) owner: Scene,
    pub(super) kind: SceneChange,
}
impl<T> Clone for SceneList<T> {
    fn clone(&self) -> Self {
        Self {
            core: self.core.clone(),
            owner: self.owner.clone(),
            kind: self.kind,
        }
    }
}
impl<T> fmt::Debug for SceneList<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SceneList({:p})", Arc::as_ptr(&self.core))
    }
}
impl<T: Clone> SceneList<T> {
    pub fn same_reference(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.core, &other.core)
    }
    pub fn len(&self) -> usize {
        guard(&self.core).values.len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Source-visible `ChangableList.Capacity`; Rust allocator identity and
    /// allocation-failure timing remain separate platform behavior.
    pub fn capacity(&self) -> usize {
        guard(&self.core).source_capacity
    }
    pub(super) fn snapshot(&self) -> Vec<T> {
        guard(&self.core).values.clone()
    }
    pub fn get(&self, index: i32) -> Result<T, SceneError> {
        let index = usize::try_from(index).map_err(|_| SceneError::IndexOutOfRange)?;
        guard(&self.core)
            .values
            .get(index)
            .cloned()
            .ok_or(SceneError::IndexOutOfRange)
    }
    pub fn try_snapshot(&self) -> Result<Vec<T>, SceneError> {
        let state = guard(&self.core);
        let mut values = Vec::new();
        values
            .try_reserve_exact(state.values.len())
            .map_err(|_| SceneError::Allocation)?;
        values.extend(state.values.iter().cloned());
        Ok(values)
    }
    pub fn add_changed(&self, handler: &SceneListHandler<T>) {
        guard(&self.core).handlers.push(handler.clone());
    }
    pub fn remove_changed(&self, handler: &SceneListHandler<T>) {
        let removed = {
            let mut state = guard(&self.core);
            state
                .handlers
                .iter()
                .rposition(|item| item.same_reference(handler))
                .map(|index| state.handlers.remove(index))
        };
        drop(removed);
    }
    fn emit(&self, handlers: Vec<SceneListHandler<T>>) -> Result<(), SceneError> {
        self.owner.notify(self.kind)?;
        for handler in handlers {
            (handler.0)(self)?;
        }
        Ok(())
    }
    /// Add a single retained root and notify after its committed insertion.
    pub fn add(&self, value: T) -> Result<(), SceneError> {
        let handlers = {
            let mut state = guard(&self.core);
            let extra = (state.source_capacity / 3).max(2);
            state.reserve_source(extra, 1)?;
            state.values.push(value);
            state.revision = state.revision.wrapping_add(1);
            state.handlers.clone()
        };
        self.emit(handlers)
    }
    /// Source batch-add sends one notification even when the input is empty.
    pub fn append(&self, values: &[T]) -> Result<(), SceneError> {
        let handlers = {
            let mut state = guard(&self.core);
            state.reserve_source(values.len(), values.len())?;
            state.values.extend(values.iter().cloned());
            state.revision = state.revision.wrapping_add(1);
            state.handlers.clone()
        };
        self.emit(handlers)
    }
    pub fn clear(&self) -> Result<(), SceneError> {
        let (mut removed, revision) = {
            let mut state = guard(&self.core);
            let removed = std::mem::take(&mut state.values);
            state.revision = state.revision.wrapping_add(1);
            (removed, state.revision)
        };
        // Retire roots without the list guard: a last owner's destructor can
        // reenter this list. Restore the original allocation only if that work
        // did not mutate the now-empty list.
        removed.clear();
        {
            let mut state = guard(&self.core);
            if state.revision == revision {
                std::mem::swap(&mut state.values, &mut removed);
            }
        }
        drop(removed);
        let handlers = guard(&self.core).handlers.clone();
        self.emit(handlers)
    }
    pub fn set(&self, index: i32, value: T) -> Result<(), SceneError> {
        let index = usize::try_from(index).map_err(|_| SceneError::IndexOutOfRange)?;
        let old = {
            let mut state = guard(&self.core);
            let slot = state
                .values
                .get_mut(index)
                .ok_or(SceneError::IndexOutOfRange)?;
            let old = std::mem::replace(slot, value);
            state.revision = state.revision.wrapping_add(1);
            old
        };
        drop(old);
        let handlers = guard(&self.core).handlers.clone();
        self.emit(handlers)
    }
    /// Out-of-range removal is a silent no-op, as in source `RemoveAt`.
    pub fn remove_at(&self, index: i32) -> Result<(), SceneError> {
        let Ok(index) = usize::try_from(index) else {
            return Ok(());
        };
        let removed = {
            let mut state = guard(&self.core);
            if index >= state.values.len() {
                return Ok(());
            }
            let removed = state.values.remove(index);
            state.revision = state.revision.wrapping_add(1);
            removed
        };
        drop(removed);
        let handlers = guard(&self.core).handlers.clone();
        self.emit(handlers)
    }
}

#[cfg(test)]
mod tests;
