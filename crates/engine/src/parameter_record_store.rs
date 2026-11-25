use crate::live_double_property::{DoublePropertyCore, DoubleSponsor};
use crate::{
    BinaryCursor, BinaryWriter, ChangeDelegate, ChangeDispatchError, ChangeHandler, ChangeNotifier,
    ContextChangeArgument, DoublePropertyDelegate, DoublePropertyError, DoublePropertyHandler,
    ModernPropertyError, ModernPropertyRecord, SharedDoubleProperty, SharedLimits,
};
use std::{
    convert::Infallible,
    error::Error,
    fmt,
    sync::{Arc, Mutex, Weak},
};

#[derive(Clone)]
struct Handle {
    brand: Arc<()>,
    index: usize,
}
impl PartialEq for Handle {
    fn eq(&self, other: &Self) -> bool {
        self.index == other.index && Arc::ptr_eq(&self.brand, &other.brand)
    }
}
impl Eq for Handle {}
impl fmt::Debug for Handle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Handle({:p}, {})", Arc::as_ptr(&self.brand), self.index)
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParameterPropertyId(Handle);
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParameterSetId(Handle);

#[derive(Debug)]
enum Slot {
    Occupied {
        key: Vec<u16>,
        property: ParameterPropertyId,
    },
    Vacant {
        next: Option<usize>,
    },
}
#[derive(Debug, Default)]
struct Registry {
    name: Option<Vec<u16>>,
    slots: Vec<Slot>,
    free: Option<usize>,
    version: u32,
}
impl Registry {
    fn find(&self, key: &[u16]) -> Option<usize> {
        self.slots
            .iter()
            .position(|s| matches!(s, Slot::Occupied { key: k, .. } if k == key))
    }
    fn insert(
        &mut self,
        key: Vec<u16>,
        property: ParameterPropertyId,
    ) -> Result<(), ParameterRecordError> {
        self.version = self.version.wrapping_add(1);
        if self.find(&key).is_some() {
            return Err(ParameterRecordError::DuplicateKey);
        }
        let occupied = Slot::Occupied { key, property };
        if let Some(index) = self.free {
            let Slot::Vacant { next } = self.slots[index] else {
                unreachable!("private free-list invariant")
            };
            self.free = next;
            self.slots[index] = occupied;
        } else {
            self.slots
                .try_reserve(1)
                .map_err(|_| ParameterRecordError::Allocation)?;
            self.slots.push(occupied);
        }
        Ok(())
    }
    fn remove_slot(&mut self, index: usize) {
        self.slots[index] = Slot::Vacant { next: self.free };
        self.free = Some(index);
        self.version = self.version.wrapping_add(1);
    }
}

#[derive(Clone, Debug)]
pub enum ParameterSlotSnapshot {
    Occupied {
        key: Vec<u16>,
        property: ParameterPropertyId,
    },
    Vacant {
        next: Option<usize>,
    },
}
#[derive(Clone, Debug)]
pub struct ParameterRegistrySnapshot {
    pub name: Option<Vec<u16>>,
    pub slots: Vec<ParameterSlotSnapshot>,
    pub free: Option<usize>,
    pub version: u32,
    pub count: usize,
    pub allow: bool,
    pub pending: bool,
}
#[derive(Debug)]
pub struct ParameterValueCursor {
    set: ParameterSetId,
    version: u32,
    index: usize,
    current: Option<ParameterPropertyId>,
}

enum PropertyValue<E> {
    Record(Box<Mutex<ModernPropertyRecord>>),
    Double(DoublePropertyCore<ParameterObserverError<E>>),
}
struct Property<E> {
    value: PropertyValue<E>,
    subscriptions: Mutex<Vec<ParameterSetId>>,
}
struct SetCore<E> {
    registry: Mutex<Registry>,
    notifier: ChangeNotifier<SetEnvelope<E>, ParameterCallbackError<E>>,
    forwarder: DoublePropertyDelegate<ParameterObserverError<E>>,
}
struct Storage<E> {
    properties: Vec<Arc<Property<E>>>,
    sets: Vec<Arc<SetCore<E>>>,
}
struct Arena<E> {
    brand: Arc<()>,
    storage: Mutex<Storage<E>>,
}
struct SetEnvelope<E> {
    owner: Weak<Arena<E>>,
    id: ParameterSetId,
    args: Option<Arc<ContextChangeArgument<ParameterObserverError<E>>>>,
}
/// One retained arena. Internal cells own only cores/coordinates; outward live
/// handles sponsor the arena, including sender clones escaping callbacks.
pub struct ParameterStore<E>(Arc<Arena<E>>);
/// Existing data-only facade over the same implementation.
pub type ParameterRecordStore = ParameterStore<Infallible>;
impl<E> Clone for ParameterStore<E> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}
impl<E> Default for ParameterStore<E> {
    fn default() -> Self {
        Self(Arc::new(Arena {
            brand: Arc::new(()),
            storage: Mutex::new(Storage {
                properties: Vec::new(),
                sets: Vec::new(),
            }),
        }))
    }
}
impl<E> fmt::Debug for ParameterStore<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ParameterStore({:p})", Arc::as_ptr(&self.0))
    }
}

/// Actual parent sender and arena sponsor. Clone preserves the set root.
pub struct SharedParameterSet<E> {
    owner: Arc<Arena<E>>,
    core: Arc<SetCore<E>>,
    id: ParameterSetId,
}
impl<E> Clone for SharedParameterSet<E> {
    fn clone(&self) -> Self {
        Self {
            owner: Arc::clone(&self.owner),
            core: Arc::clone(&self.core),
            id: self.id.clone(),
        }
    }
}
impl<E> fmt::Debug for SharedParameterSet<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SharedParameterSet({:p})", Arc::as_ptr(&self.core))
    }
}
pub struct ParameterSetHandler<E> {
    core: ChangeHandler<SetEnvelope<E>, ParameterCallbackError<E>>,
    sponsors: Vec<Arc<Arena<E>>>,
}
impl<E> Clone for ParameterSetHandler<E> {
    fn clone(&self) -> Self {
        Self {
            core: self.core.clone(),
            sponsors: self.sponsors.clone(),
        }
    }
}
impl<E> fmt::Debug for ParameterSetHandler<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.core.fmt(f)
    }
}
impl<E: 'static> ParameterSetHandler<E> {
    pub fn new(
        callback: impl Fn(
            &SharedParameterSet<E>,
            Option<Arc<ContextChangeArgument<ParameterObserverError<E>>>>,
        ) -> Result<(), E>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            core: ChangeHandler::new(move |_, envelope: Option<Arc<SetEnvelope<E>>>| {
                let envelope = envelope.ok_or(ParameterCallbackError::SenderReleased)?;
                let owner = envelope
                    .owner
                    .upgrade()
                    .ok_or(ParameterCallbackError::SenderReleased)?;
                let sender = ParameterStore(owner)
                    .live_set(&envelope.id)
                    .map_err(|_| ParameterCallbackError::SenderReleased)?;
                callback(&sender, envelope.args.clone()).map_err(ParameterCallbackError::Observer)
            }),
            sponsors: Vec::new(),
        }
    }
    pub fn same_binding(&self, other: &Self) -> bool {
        self.core.same_binding(&other.core)
    }
}
pub struct ParameterSetDelegate<E> {
    core: ChangeDelegate<SetEnvelope<E>, ParameterCallbackError<E>>,
    sponsors: Vec<Arc<Arena<E>>>,
}
impl<E> Clone for ParameterSetDelegate<E> {
    fn clone(&self) -> Self {
        Self {
            core: self.core.clone(),
            sponsors: self.sponsors.clone(),
        }
    }
}
impl<E> Default for ParameterSetDelegate<E> {
    fn default() -> Self {
        Self {
            core: ChangeDelegate::default(),
            sponsors: Vec::new(),
        }
    }
}
impl<E> fmt::Debug for ParameterSetDelegate<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.core.fmt(f)
    }
}
impl<E> ParameterSetDelegate<E> {
    pub fn from_handlers(handlers: &[ParameterSetHandler<E>]) -> Self {
        let cores: Vec<_> = handlers.iter().map(|h| h.core.clone()).collect();
        let mut sponsors: Vec<Arc<Arena<E>>> = Vec::new();
        for handler in handlers {
            for sponsor in &handler.sponsors {
                if !sponsors.iter().any(|held| Arc::ptr_eq(held, sponsor)) {
                    sponsors.push(Arc::clone(sponsor));
                }
            }
        }
        Self {
            core: ChangeDelegate::from_handlers(&cores),
            sponsors,
        }
    }
    pub fn is_present(&self) -> bool {
        self.core.is_present()
    }
    pub fn is_multicast(&self) -> bool {
        self.core.is_multicast()
    }
    pub fn handlers(&self) -> Vec<ParameterSetHandler<E>> {
        self.core
            .handlers()
            .iter()
            .map(|core| ParameterSetHandler {
                core: core.clone(),
                sponsors: self.sponsors.clone(),
            })
            .collect()
    }
    pub fn same_invocation_array(&self, other: &Self) -> bool {
        self.core.same_invocation_array(&other.core)
    }
}

impl<E: 'static> SharedParameterSet<E> {
    pub fn same_reference(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.core, &other.core)
    }
    pub fn id(&self) -> &ParameterSetId {
        &self.id
    }
    pub fn snapshot(&self) -> ParameterRegistrySnapshot {
        let (name, slots, free, version, count) = {
            let registry = self.core.registry.lock().unwrap_or_else(|e| e.into_inner());
            let slots: Vec<_> = registry
                .slots
                .iter()
                .map(|slot| match slot {
                    Slot::Occupied { key, property } => ParameterSlotSnapshot::Occupied {
                        key: key.clone(),
                        property: property.clone(),
                    },
                    Slot::Vacant { next } => ParameterSlotSnapshot::Vacant { next: *next },
                })
                .collect();
            let count = registry
                .slots
                .iter()
                .filter(|slot| matches!(slot, Slot::Occupied { .. }))
                .count();
            (
                registry.name.clone(),
                slots,
                registry.free,
                registry.version,
                count,
            )
        };
        let notifier = self.core.notifier.snapshot();
        ParameterRegistrySnapshot {
            name,
            slots,
            free,
            version,
            count,
            allow: notifier.allow,
            pending: notifier.pending,
        }
    }
    /// Retain the actual invocation array and this arena for observation/reuse
    /// inside the same arena. Cross-arena ownership transfer is not supplied.
    pub fn changed_delegate(&self) -> ParameterSetDelegate<E> {
        ParameterSetDelegate {
            core: self.core.notifier.snapshot().delegate,
            sponsors: vec![Arc::clone(&self.owner)],
        }
    }
    pub fn forwarding_handler(&self) -> DoublePropertyHandler<ParameterObserverError<E>> {
        // Public diagnostic handles sponsor the receiver. The core's binding is
        // unsponsored and stable; no adapter is rebuilt here.
        let mut handlers = self.core.forwarder.handlers();
        let handler = handlers.remove(0);
        let sponsor: DoubleSponsor = self.owner.clone();
        handler.with_sponsor(sponsor)
    }
    /// Integrated diagnostic binding reuse is scoped to the same arena. Core
    /// dispatch stores bindings, not outward snapshot sponsors; cross-arena
    /// transfer requires a separate characterized ownership bridge.
    pub fn add(&self, delegate: &ParameterSetDelegate<E>) {
        self.core.notifier.add(&delegate.core);
    }
    pub fn remove(&self, delegate: &ParameterSetDelegate<E>) -> Result<(), ParameterStoreError<E>> {
        self.core
            .notifier
            .remove(&delegate.core)
            .map_err(|e| ParameterStoreError::Parent(ChangeDispatchError::Framework(e)))
    }
    pub fn lock_changes(&self) {
        self.core.notifier.lock_changes();
    }
    pub fn unlock(&self, pending: bool) -> Result<(), ParameterStoreError<E>> {
        let frame = self.clone();
        frame
            .core
            .notifier
            .unlock(pending)
            .map_err(ParameterStoreError::Parent)
    }
    fn emit(
        &self,
        args: Option<Arc<ContextChangeArgument<ParameterObserverError<E>>>>,
    ) -> Result<(), ChangeDispatchError<ParameterCallbackError<E>>> {
        let frame = self.clone();
        frame.core.notifier.emit(Some(Arc::new(SetEnvelope {
            owner: Arc::downgrade(&frame.owner),
            id: frame.id.clone(),
            args,
        })))
    }
}

impl<E: 'static> ParameterStore<E> {
    fn check(&self, handle: &Handle, len: usize) -> Result<usize, ParameterRecordError> {
        if !Arc::ptr_eq(&self.0.brand, &handle.brand) || handle.index >= len {
            Err(ParameterRecordError::ForeignHandle)
        } else {
            Ok(handle.index)
        }
    }
    fn property_core(
        &self,
        id: &ParameterPropertyId,
    ) -> Result<Arc<Property<E>>, ParameterStoreError<E>> {
        let storage = self.0.storage.lock().unwrap_or_else(|e| e.into_inner());
        Ok(Arc::clone(
            &storage.properties[self.check(&id.0, storage.properties.len())?],
        ))
    }
    fn set_core(&self, id: &ParameterSetId) -> Result<Arc<SetCore<E>>, ParameterStoreError<E>> {
        let storage = self.0.storage.lock().unwrap_or_else(|e| e.into_inner());
        Ok(Arc::clone(
            &storage.sets[self.check(&id.0, storage.sets.len())?],
        ))
    }
    pub fn new_property(&self, record: ModernPropertyRecord) -> ParameterPropertyId {
        let mut storage = self.0.storage.lock().unwrap_or_else(|e| e.into_inner());
        let id = ParameterPropertyId(Handle {
            brand: Arc::clone(&self.0.brand),
            index: storage.properties.len(),
        });
        storage.properties.push(Arc::new(Property {
            value: PropertyValue::Record(Box::new(Mutex::new(record))),
            subscriptions: Mutex::new(Vec::new()),
        }));
        id
    }
    /// Integrated creation only; standalone root adoption is not supplied.
    pub fn new_live_double(
        &self,
        record: ModernPropertyRecord,
        limits: Option<SharedLimits>,
    ) -> Result<ParameterPropertyId, ParameterStoreError<E>> {
        let sponsor: DoubleSponsor = self.0.clone();
        let root = SharedDoubleProperty::from_backing_in_owner(record, limits, sponsor)
            .map_err(ParameterStoreError::Property)?;
        let core = root.unsponsored_core();
        let id = {
            let mut storage = self.0.storage.lock().unwrap_or_else(|e| e.into_inner());
            let id = ParameterPropertyId(Handle {
                brand: Arc::clone(&self.0.brand),
                index: storage.properties.len(),
            });
            storage.properties.push(Arc::new(Property {
                value: PropertyValue::Double(core),
                subscriptions: Mutex::new(Vec::new()),
            }));
            id
        };
        drop(root);
        Ok(id)
    }
    pub fn new_set(&self) -> ParameterSetId {
        let mut storage = self.0.storage.lock().unwrap_or_else(|e| e.into_inner());
        let id = ParameterSetId(Handle {
            brand: Arc::clone(&self.0.brand),
            index: storage.sets.len(),
        });
        let owner = Arc::downgrade(&self.0);
        let target = id.clone();
        let handler = DoublePropertyHandler::new(move |_, args| {
            let owner = owner
                .upgrade()
                .ok_or(ParameterObserverError::SenderReleased)?;
            let sender = ParameterStore(owner)
                .live_set(&target)
                .map_err(|_| ParameterObserverError::SenderReleased)?;
            sender.emit(args).map_err(ParameterObserverError::Parent)
        });
        let core = Arc::new(SetCore {
            registry: Mutex::new(Registry::default()),
            notifier: ChangeNotifier::new(Arc::new(SetEnvelope {
                owner: Arc::downgrade(&self.0),
                id: id.clone(),
                args: Some(ContextChangeArgument::<ParameterObserverError<E>>::shared_empty()),
            })),
            forwarder: DoublePropertyDelegate::from_handlers(&[handler]),
        });
        storage.sets.push(core);
        id
    }
    pub fn live_set(
        &self,
        id: &ParameterSetId,
    ) -> Result<SharedParameterSet<E>, ParameterStoreError<E>> {
        Ok(SharedParameterSet {
            owner: Arc::clone(&self.0),
            core: self.set_core(id)?,
            id: id.clone(),
        })
    }
    pub fn live_double(
        &self,
        id: &ParameterPropertyId,
    ) -> Result<SharedDoubleProperty<ParameterObserverError<E>>, ParameterStoreError<E>> {
        let core = self.property_core(id)?;
        match &core.value {
            PropertyValue::Double(root) => root.retain().map_err(ParameterStoreError::Property),
            PropertyValue::Record(_) => Err(ParameterRecordError::BackingValueKind.into()),
        }
    }
    /// Explicit owned observation, not a borrow, source Clone or wire Save.
    pub fn property(
        &self,
        id: &ParameterPropertyId,
    ) -> Result<ModernPropertyRecord, ParameterStoreError<E>> {
        let core = self.property_core(id)?;
        Ok(match &core.value {
            PropertyValue::Record(record) => record
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .copy_record_snapshot(),
            PropertyValue::Double(root) => root.record_snapshot(),
        })
    }
    pub fn set_raw_name(
        &self,
        id: &ParameterPropertyId,
        name: Option<Vec<u16>>,
    ) -> Result<(), ParameterStoreError<E>> {
        let core = self.property_core(id)?;
        let retired = match &core.value {
            PropertyValue::Record(record) => {
                let mut record = record.lock().unwrap_or_else(|e| e.into_inner());
                std::mem::replace(&mut record.name, name)
            }
            PropertyValue::Double(root) => {
                root.set_raw_name(name);
                None
            }
        };
        drop(retired);
        Ok(())
    }
    pub fn overwrite_double_current(
        &self,
        id: &ParameterPropertyId,
        value: f64,
    ) -> Result<(), ParameterStoreError<E>> {
        let core = self.property_core(id)?;
        let changed = match &core.value {
            PropertyValue::Record(record) => record
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .overwrite_double_current(value),
            PropertyValue::Double(root) => root.overwrite_current(value),
        };
        if changed {
            Ok(())
        } else {
            Err(ParameterRecordError::BackingValueKind.into())
        }
    }
    pub fn subscription_targets(
        &self,
        id: &ParameterPropertyId,
    ) -> Result<Vec<ParameterSetId>, ParameterStoreError<E>> {
        let core = self.property_core(id)?;
        let targets = core
            .subscriptions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        Ok(targets)
    }
    pub fn set_name(
        &self,
        id: &ParameterSetId,
        name: Option<Vec<u16>>,
    ) -> Result<(), ParameterStoreError<E>> {
        let core = self.set_core(id)?;
        let retired = {
            let mut r = core.registry.lock().unwrap_or_else(|e| e.into_inner());
            std::mem::replace(&mut r.name, name)
        };
        drop(retired);
        Ok(())
    }
    pub fn name(&self, id: &ParameterSetId) -> Result<Option<Vec<u16>>, ParameterStoreError<E>> {
        let core = self.set_core(id)?;
        let name = core
            .registry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .name
            .clone();
        Ok(name)
    }
    pub fn registry_snapshot(
        &self,
        id: &ParameterSetId,
    ) -> Result<ParameterRegistrySnapshot, ParameterStoreError<E>> {
        Ok(self.live_set(id)?.snapshot())
    }
    pub fn entries(
        &self,
        id: &ParameterSetId,
    ) -> Result<Vec<(Vec<u16>, ParameterPropertyId)>, ParameterStoreError<E>> {
        let core = self.set_core(id)?;
        let r = core.registry.lock().unwrap_or_else(|e| e.into_inner());
        Ok(r.slots
            .iter()
            .filter_map(|slot| match slot {
                Slot::Occupied { key, property } => Some((key.clone(), property.clone())),
                _ => None,
            })
            .collect())
    }
    pub fn lookup(
        &self,
        id: &ParameterSetId,
        key: Option<&[u16]>,
    ) -> Result<Option<ParameterPropertyId>, ParameterStoreError<E>> {
        let core = self.set_core(id)?;
        let key = key.ok_or(ParameterRecordError::NullKey)?;
        let r = core.registry.lock().unwrap_or_else(|e| e.into_inner());
        Ok(r.find(key).and_then(|i| match &r.slots[i] {
            Slot::Occupied { property, .. } => Some(property.clone()),
            _ => None,
        }))
    }
    pub fn indexed(
        &self,
        id: &ParameterSetId,
        index: i32,
    ) -> Result<Option<ParameterPropertyId>, ParameterStoreError<E>> {
        let core = self.set_core(id)?;
        if index < 0 {
            return Ok(None);
        }
        let r = core.registry.lock().unwrap_or_else(|e| e.into_inner());
        Ok(r.slots
            .iter()
            .filter_map(|slot| match slot {
                Slot::Occupied { property, .. } => Some(property.clone()),
                _ => None,
            })
            .nth(index as usize))
    }
    pub fn add_property(
        &self,
        set: &ParameterSetId,
        property: Option<&ParameterPropertyId>,
    ) -> Result<(), ParameterStoreError<E>> {
        let parent = self.set_core(set)?;
        let id = property.ok_or(ParameterRecordError::NullProperty)?;
        let child = self.property_core(id)?;
        let key = self
            .property(id)?
            .name
            .ok_or(ParameterRecordError::NullKey)?;
        {
            let mut registry = parent.registry.lock().unwrap_or_else(|e| e.into_inner());
            if registry.find(&key).is_some() {
                return Ok(());
            }
            registry.insert(key, id.clone())?;
        }
        if let PropertyValue::Double(root) = &child.value {
            root.retain()
                .map_err(ParameterStoreError::Property)?
                .add(&parent.forwarder);
        }
        child
            .subscriptions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(set.clone());
        Ok(())
    }
    pub fn direct_insert(
        &self,
        set: &ParameterSetId,
        key: Option<Vec<u16>>,
        property: &ParameterPropertyId,
    ) -> Result<(), ParameterStoreError<E>> {
        let parent = self.set_core(set)?;
        self.property_core(property)?;
        parent
            .registry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key.ok_or(ParameterRecordError::NullKey)?, property.clone())?;
        Ok(())
    }
    fn detach(
        &self,
        set: &ParameterSetId,
        child: &Arc<Property<E>>,
        parent: &Arc<SetCore<E>>,
    ) -> Result<(), ParameterStoreError<E>> {
        if let PropertyValue::Double(root) = &child.value {
            root.retain()
                .map_err(ParameterStoreError::Property)?
                .remove(&parent.forwarder)
                .map_err(ParameterStoreError::Property)?;
        }
        let mut targets = child
            .subscriptions
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(i) = targets.iter().rposition(|id| id == set) {
            targets.remove(i);
        }
        Ok(())
    }
    pub fn remove_key(
        &self,
        set: &ParameterSetId,
        key: Option<&[u16]>,
    ) -> Result<bool, ParameterStoreError<E>> {
        let parent = self.set_core(set)?;
        let key = key.ok_or(ParameterRecordError::NullKey)?;
        let property = {
            let r = parent.registry.lock().unwrap_or_else(|e| e.into_inner());
            let Some(i) = r.find(key) else {
                return Ok(false);
            };
            match &r.slots[i] {
                Slot::Occupied { property, .. } => property.clone(),
                _ => return Err(ParameterRecordError::DefinitionLayout.into()),
            }
        };
        self.detach(set, &self.property_core(&property)?, &parent)?;
        let mut r = parent.registry.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(i) = r.find(key) {
            r.remove_slot(i);
            Ok(true)
        } else {
            Ok(false)
        }
    }
    pub fn remove_property(
        &self,
        set: &ParameterSetId,
        property: Option<&ParameterPropertyId>,
    ) -> Result<bool, ParameterStoreError<E>> {
        self.set_core(set)?;
        let id = property.ok_or(ParameterRecordError::NullProperty)?;
        let name = self.property(id)?.name;
        self.remove_key(set, name.as_deref())
    }
    pub fn remove_all(&self, set: &ParameterSetId) -> Result<(), ParameterStoreError<E>> {
        let parent = self.set_core(set)?;
        let roots: Vec<_> = self.entries(set)?.into_iter().map(|(_, id)| id).collect();
        for id in roots {
            self.detach(set, &self.property_core(&id)?, &parent)?;
            let name = self
                .property(&id)?
                .name
                .ok_or(ParameterRecordError::NullKey)?;
            let mut r = parent.registry.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(i) = r.find(&name) {
                r.remove_slot(i);
            }
        }
        Ok(())
    }
    pub fn begin_values(
        &self,
        set: &ParameterSetId,
    ) -> Result<ParameterValueCursor, ParameterStoreError<E>> {
        let parent = self.set_core(set)?;
        let version = parent
            .registry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .version;
        Ok(ParameterValueCursor {
            set: set.clone(),
            version,
            index: 0,
            current: None,
        })
    }
    pub fn advance(
        &self,
        cursor: &mut ParameterValueCursor,
    ) -> Result<bool, ParameterStoreError<E>> {
        let parent = self.set_core(&cursor.set)?;
        let r = parent.registry.lock().unwrap_or_else(|e| e.into_inner());
        if cursor.version != r.version {
            return Err(ParameterRecordError::InvalidEnumeration.into());
        }
        while cursor.index < r.slots.len() {
            let i = cursor.index;
            cursor.index += 1;
            if let Slot::Occupied { property, .. } = &r.slots[i] {
                cursor.current = Some(property.clone());
                return Ok(true);
            }
        }
        cursor.index = r.slots.len() + 1;
        cursor.current = None;
        Ok(false)
    }
    pub fn reset_values(
        &self,
        cursor: &mut ParameterValueCursor,
    ) -> Result<(), ParameterStoreError<E>> {
        let parent = self.set_core(&cursor.set)?;
        let r = parent.registry.lock().unwrap_or_else(|e| e.into_inner());
        if cursor.version != r.version {
            return Err(ParameterRecordError::InvalidEnumeration.into());
        }
        cursor.index = 0;
        cursor.current = None;
        Ok(())
    }
    pub fn current_value(
        &self,
        cursor: &ParameterValueCursor,
    ) -> Result<Option<ParameterPropertyId>, ParameterStoreError<E>> {
        self.set_core(&cursor.set)?;
        Ok(cursor.current.clone())
    }
    pub fn current_nongeneric(
        &self,
        cursor: &ParameterValueCursor,
    ) -> Result<Option<ParameterPropertyId>, ParameterStoreError<E>> {
        let parent = self.set_core(&cursor.set)?;
        let r = parent.registry.lock().unwrap_or_else(|e| e.into_inner());
        if cursor.index == 0 || cursor.index == r.slots.len() + 1 {
            return Err(ParameterRecordError::InvalidEnumeration.into());
        }
        Ok(cursor.current.clone())
    }
    pub fn load(
        &self,
        set: &ParameterSetId,
        reader: &mut BinaryCursor<'_>,
        version: f64,
    ) -> Result<(), ParameterStoreError<E>> {
        let mut cursor = self.begin_values(set)?;
        while self.advance(&mut cursor)? {
            if let Some(id) = self.current_value(&cursor)? {
                let child = self.property_core(&id)?;
                match &child.value {
                    PropertyValue::Record(record) => record
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .load(reader, version)?,
                    PropertyValue::Double(root) => root.load(reader, version)?,
                }
            }
        }
        Ok(())
    }
    pub fn save(
        &self,
        set: &ParameterSetId,
        writer: &mut BinaryWriter,
    ) -> Result<(), ParameterStoreError<E>> {
        let mut cursor = self.begin_values(set)?;
        while self.advance(&mut cursor)? {
            if let Some(id) = self.current_value(&cursor)? {
                let child = self.property_core(&id)?;
                match &child.value {
                    PropertyValue::Record(record) => record
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .save(writer)?,
                    PropertyValue::Double(root) => root.save(writer)?,
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum ParameterCallbackError<E> {
    Observer(E),
    SenderReleased,
}
#[derive(Debug)]
pub enum ParameterObserverError<E> {
    Observer(E),
    Parent(ChangeDispatchError<ParameterCallbackError<E>>),
    SenderReleased,
}
#[derive(Debug)]
pub enum ParameterStoreError<E> {
    Record(ParameterRecordError),
    Property(DoublePropertyError<ParameterObserverError<E>>),
    Parent(ChangeDispatchError<ParameterCallbackError<E>>),
}
impl<E> From<ParameterRecordError> for ParameterStoreError<E> {
    fn from(error: ParameterRecordError) -> Self {
        Self::Record(error)
    }
}
impl<E> From<ModernPropertyError> for ParameterStoreError<E> {
    fn from(error: ModernPropertyError) -> Self {
        Self::Record(ParameterRecordError::Property(error))
    }
}
impl<E: fmt::Display> fmt::Display for ParameterCallbackError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Observer(e) => e.fmt(f),
            Self::SenderReleased => f.write_str("parameter sender released"),
        }
    }
}
impl<E: Error + 'static> Error for ParameterCallbackError<E> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Observer(e) => Some(e),
            Self::SenderReleased => None,
        }
    }
}
impl<E: fmt::Display> fmt::Display for ParameterObserverError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Observer(e) => e.fmt(f),
            Self::Parent(e) => e.fmt(f),
            Self::SenderReleased => f.write_str("parameter receiver released"),
        }
    }
}
impl<E: Error + 'static> Error for ParameterObserverError<E> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Observer(e) => Some(e),
            Self::Parent(e) => Some(e),
            Self::SenderReleased => None,
        }
    }
}
impl<E: fmt::Display> fmt::Display for ParameterStoreError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Record(e) => e.fmt(f),
            Self::Property(e) => e.fmt(f),
            Self::Parent(e) => e.fmt(f),
        }
    }
}
impl<E: Error + 'static> Error for ParameterStoreError<E> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Record(e) => Some(e),
            Self::Property(e) => Some(e),
            Self::Parent(e) => Some(e),
        }
    }
}
impl ParameterStoreError<Infallible> {
    pub fn source_class(&self) -> &'static str {
        match self {
            Self::Record(error) => error.source_class(),
            Self::Property(DoublePropertyError::Rounding(error)) => error.source_class(),
            Self::Property(DoublePropertyError::MissingLimits) => "NullReferenceException",
            Self::Property(DoublePropertyError::Wire(error)) => error.source_class(),
            Self::Property(DoublePropertyError::BackingKind) => "ArgumentException",
            Self::Property(DoublePropertyError::Notification(ChangeDispatchError::Framework(
                error,
            ))) => error.source_class(),
            Self::Property(DoublePropertyError::Notification(ChangeDispatchError::Handler(
                crate::DoubleCallbackError::Observer(ParameterObserverError::Observer(never)),
            ))) => match *never {},
            Self::Property(DoublePropertyError::Notification(ChangeDispatchError::Handler(
                crate::DoubleCallbackError::Observer(ParameterObserverError::Parent(error)),
            ))) => Self::parent_source_class(error),
            Self::Property(DoublePropertyError::Notification(ChangeDispatchError::Handler(
                crate::DoubleCallbackError::Observer(ParameterObserverError::SenderReleased),
            )))
            | Self::Property(DoublePropertyError::Notification(ChangeDispatchError::Handler(
                crate::DoubleCallbackError::SenderReleased,
            ))) => "InvalidOperationException",
            Self::Parent(error) => Self::parent_source_class(error),
        }
    }
    fn parent_source_class(
        error: &ChangeDispatchError<ParameterCallbackError<Infallible>>,
    ) -> &'static str {
        match error {
            ChangeDispatchError::Framework(error) => error.source_class(),
            ChangeDispatchError::Handler(ParameterCallbackError::Observer(never)) => {
                match *never {}
            }
            ChangeDispatchError::Handler(ParameterCallbackError::SenderReleased) => {
                "InvalidOperationException"
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParameterRecordError {
    NullKey,
    NullProperty,
    DuplicateKey,
    InvalidEnumeration,
    ForeignHandle,
    BackingValueKind,
    /// Rust fixed-definition invariant, not an original constructor error.
    DefinitionLayout,
    Allocation,
    Property(ModernPropertyError),
}
impl ParameterRecordError {
    pub const fn source_class(self) -> &'static str {
        match self {
            Self::NullKey => "ArgumentNullException",
            Self::NullProperty => "NullReferenceException",
            Self::DuplicateKey
            | Self::ForeignHandle
            | Self::BackingValueKind
            | Self::DefinitionLayout => "ArgumentException",
            Self::InvalidEnumeration => "InvalidOperationException",
            Self::Allocation => "OutOfMemoryException",
            Self::Property(e) => e.source_class(),
        }
    }
}
impl From<ModernPropertyError> for ParameterRecordError {
    fn from(e: ModernPropertyError) -> Self {
        Self::Property(e)
    }
}
impl fmt::Display for ParameterRecordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "parameter record {self:?}: {}", self.source_class())
    }
}
impl Error for ParameterRecordError {}
