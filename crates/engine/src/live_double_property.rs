use crate::{
    BinaryCursor, BinaryWriter, ChangeDelegate, ChangeDispatchError, ChangeHandler, ChangeNotifier,
    ContextChangeArgument, ContextPropertyChangeArgs, DecimalRoundingError, DoubleUndoInfo,
    ModernPropertyError, ModernPropertyRecord, SharedLimits, round_decimal_places,
};
use std::{
    any::Any,
    error::Error,
    fmt,
    sync::{Arc, Mutex, Weak},
};

struct State {
    record: ModernPropertyRecord,
    limits: Option<SharedLimits>,
}
struct Inner<E> {
    sponsor: Option<Weak<dyn Any + Send + Sync>>,
    state: Mutex<State>,
    notifier: ChangeNotifier<Envelope<E>, DoubleCallbackError<E>>,
}
// Private context transport. The outward callback receives the original args
// root, not this envelope; the weak sender prevents a framework owner cycle.
struct Envelope<E> {
    sender: Weak<Inner<E>>,
    args: Option<Arc<ContextChangeArgument<E>>>,
}

/// One retained modern Double property with a single wire/value/metadata record.
/// Cloning shares this root. Backing import and fixed-context wire operations do
/// not establish general constructors, dynamic wrapper aliases or evaluation.
pub(crate) type DoubleSponsor = Arc<dyn Any + Send + Sync>;
pub struct SharedDoubleProperty<E>(Arc<Inner<E>>, Option<DoubleSponsor>);
pub(crate) struct DoublePropertyCore<E>(Arc<Inner<E>>);
impl<E> Clone for DoublePropertyCore<E> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}
impl<E> Clone for SharedDoubleProperty<E> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0), self.1.clone())
    }
}
impl<E> fmt::Debug for SharedDoubleProperty<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "SharedDoubleProperty({:p})",
            Arc::as_ptr(&self.0)
        )
    }
}

#[derive(Clone, Debug)]
pub struct DoublePropertySnapshot {
    pub current: f64,
    pub previous: f64,
    pub name: Option<Vec<u16>>,
    pub adjusted: bool,
    pub limits: Option<SharedLimits>,
    pub allow: bool,
    pub pending: bool,
}

/// A stable host callback binding, independent of the publishing property root.
/// Construct once and clone/reuse when adding or removing subscriptions.
pub struct DoublePropertyHandler<E> {
    core: ChangeHandler<Envelope<E>, DoubleCallbackError<E>>,
    sponsors: Vec<DoubleSponsor>,
}
impl<E> Clone for DoublePropertyHandler<E> {
    fn clone(&self) -> Self {
        Self {
            core: self.core.clone(),
            sponsors: self.sponsors.clone(),
        }
    }
}
impl<E> fmt::Debug for DoublePropertyHandler<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.core.fmt(formatter)
    }
}
impl<E: 'static> DoublePropertyHandler<E> {
    pub fn new(
        callback: impl Fn(
            &SharedDoubleProperty<E>,
            Option<Arc<ContextChangeArgument<E>>>,
        ) -> Result<(), E>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            core: ChangeHandler::new(move |_, envelope: Option<Arc<Envelope<E>>>| {
                let envelope = envelope.ok_or(DoubleCallbackError::SenderReleased)?;
                let owner = envelope
                    .sender
                    .upgrade()
                    .ok_or(DoubleCallbackError::SenderReleased)?;
                let sponsor = match &owner.sponsor {
                    Some(sponsor) => Some(
                        sponsor
                            .upgrade()
                            .ok_or(DoubleCallbackError::SenderReleased)?,
                    ),
                    None => None,
                };
                callback(&SharedDoubleProperty(owner, sponsor), envelope.args.clone())
                    .map_err(DoubleCallbackError::Observer)
            }),
            sponsors: Vec::new(),
        }
    }

    pub(crate) fn with_sponsor(mut self, sponsor: DoubleSponsor) -> Self {
        self.sponsors.push(sponsor);
        self
    }

    pub fn same_binding(&self, other: &Self) -> bool {
        self.core.same_binding(&other.core)
    }
}

/// Retained source-profile delegate identities over stable property handlers.
pub struct DoublePropertyDelegate<E> {
    core: ChangeDelegate<Envelope<E>, DoubleCallbackError<E>>,
    sponsors: Vec<DoubleSponsor>,
}
impl<E> Clone for DoublePropertyDelegate<E> {
    fn clone(&self) -> Self {
        Self {
            core: self.core.clone(),
            sponsors: self.sponsors.clone(),
        }
    }
}
impl<E> Default for DoublePropertyDelegate<E> {
    fn default() -> Self {
        Self {
            core: ChangeDelegate::default(),
            sponsors: Vec::new(),
        }
    }
}
impl<E> fmt::Debug for DoublePropertyDelegate<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.core.fmt(formatter)
    }
}
impl<E> DoublePropertyDelegate<E> {
    pub fn from_handlers(handlers: &[DoublePropertyHandler<E>]) -> Self {
        let cores: Vec<_> = handlers
            .iter()
            .map(|handler| handler.core.clone())
            .collect();
        let mut sponsors: Vec<DoubleSponsor> = Vec::new();
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
    pub fn handlers(&self) -> Vec<DoublePropertyHandler<E>> {
        self.core
            .handlers()
            .iter()
            .map(|core| DoublePropertyHandler {
                core: core.clone(),
                sponsors: self.sponsors.clone(),
            })
            .collect()
    }

    pub fn same_invocation_array(&self, other: &Self) -> bool {
        self.core.same_invocation_array(&other.core)
    }
}

impl<E: 'static> SharedDoubleProperty<E> {
    /// Import already-resolved backing data, including nullable Limits and
    /// distinct current/previous values. No source constructor/setter is invoked.
    pub fn from_backing(
        record: ModernPropertyRecord,
        limits: Option<SharedLimits>,
    ) -> Result<Self, DoublePropertyError<E>> {
        Self::create(record, limits, None)
    }

    pub(crate) fn from_backing_in_owner(
        record: ModernPropertyRecord,
        limits: Option<SharedLimits>,
        sponsor: DoubleSponsor,
    ) -> Result<Self, DoublePropertyError<E>> {
        Self::create(record, limits, Some(sponsor))
    }

    fn create(
        record: ModernPropertyRecord,
        limits: Option<SharedLimits>,
        sponsor: Option<DoubleSponsor>,
    ) -> Result<Self, DoublePropertyError<E>> {
        if record.double_pair().is_none() {
            return Err(DoublePropertyError::BackingKind);
        }
        let owner = sponsor.as_ref().map(Arc::downgrade);
        Ok(Self(
            Arc::new_cyclic(|sender: &Weak<Inner<E>>| Inner {
                sponsor: owner,
                state: Mutex::new(State { record, limits }),
                notifier: ChangeNotifier::new(Arc::new(Envelope {
                    sender: sender.clone(),
                    args: Some(ContextChangeArgument::<E>::shared_empty()),
                })),
            }),
            sponsor,
        ))
    }

    pub(crate) fn unsponsored_core(&self) -> DoublePropertyCore<E> {
        DoublePropertyCore(Arc::clone(&self.0))
    }

    pub fn changed_delegate(&self) -> DoublePropertyDelegate<E> {
        DoublePropertyDelegate {
            core: self.0.notifier.snapshot().delegate,
            sponsors: self.1.iter().cloned().collect(),
        }
    }

    pub fn set_name(&self, name: Option<Vec<u16>>) -> Result<(), DoublePropertyError<E>> {
        let retired = {
            let mut state = self
                .0
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if state.record.name == name {
                return Ok(());
            }
            std::mem::replace(&mut state.record.name, name)
        };
        drop(retired);
        self.emit(Some(Arc::new(ContextChangeArgument::Property(
            ContextPropertyChangeArgs::<E>::with_flags(false, true),
        ))))
    }

    pub fn same_reference(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    pub fn snapshot(&self) -> Result<DoublePropertySnapshot, DoublePropertyError<E>> {
        let (current, previous, name, adjusted, limits) = {
            let state = self
                .0
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let (current, previous) = state
                .record
                .double_pair()
                .ok_or(DoublePropertyError::BackingKind)?;
            (
                current,
                previous,
                state.record.name.clone(),
                Self::is_adjusted(&state.record),
                state.limits.clone(),
            )
        };
        let notification = self.0.notifier.snapshot();
        Ok(DoublePropertySnapshot {
            current,
            previous,
            name,
            adjusted,
            limits,
            allow: notification.allow,
            pending: notification.pending,
        })
    }

    /// Add retained callback bindings. Integrated framework forwarding is scoped
    /// to one sponsoring arena; transferring diagnostic forwarding bindings to
    /// an unrelated arena or standalone publisher is not a supported ownership
    /// bridge. The notifier stores cores, not diagnostic handle sponsors.
    pub fn add(&self, delegate: &DoublePropertyDelegate<E>) {
        self.0.notifier.add(&delegate.core);
    }

    pub fn remove(
        &self,
        delegate: &DoublePropertyDelegate<E>,
    ) -> Result<(), DoublePropertyError<E>> {
        self.0.notifier.remove(&delegate.core).map_err(|error| {
            DoublePropertyError::Notification(ChangeDispatchError::Framework(error))
        })
    }

    pub fn lock_changes(&self) {
        self.0.notifier.lock_changes();
    }

    pub fn unlock(&self, process_pending: bool) -> Result<(), DoublePropertyError<E>> {
        // The default envelope holds a weak sender. This frame keeps that sender
        // alive even if callbacks release every externally retained handle.
        let frame = self.clone();
        frame
            .0
            .notifier
            .unlock(process_pending)
            .map_err(DoublePropertyError::Notification)
    }

    pub fn set_value(&self, value: f64) -> Result<(), DoublePropertyError<E>> {
        if !self.within_bounds(value)? || self.pair()?.1 == value {
            return Ok(());
        }
        let capped = self.cap_value(value)?;
        let args = {
            let mut state = self
                .0
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let (_, previous) = state
                .record
                .double_pair()
                .ok_or(DoublePropertyError::BackingKind)?;
            if previous == capped {
                None
            } else {
                let undo = if Self::is_adjusted(&state.record) {
                    None
                } else {
                    Some(Arc::new(DoubleUndoInfo::new(
                        state.record.name.clone(),
                        0,
                        previous,
                        capped,
                    )))
                };
                if !state.record.overwrite_double_current(capped) {
                    return Err(DoublePropertyError::BackingKind);
                }
                Some(Arc::new(ContextChangeArgument::Property(
                    ContextPropertyChangeArgs::<E>::with_undo(undo),
                )))
            }
        };
        if let Some(args) = args {
            self.emit(Some(args))?;
        }
        // Even a rounded-equals-previous base no-op reaches this outer commit.
        self.commit_previous()
    }

    pub fn set_temporary(&self, value: f64, temporary: bool) -> Result<(), DoublePropertyError<E>> {
        if !self.within_bounds(value)? || self.pair()?.1 == value {
            return Ok(());
        }
        let args = {
            let mut state = self
                .0
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let (_, previous) = state
                .record
                .double_pair()
                .ok_or(DoublePropertyError::BackingKind)?;
            if previous == value {
                None
            } else {
                let undo = Arc::new(DoubleUndoInfo::with_temporary(
                    state.record.name.clone(),
                    0,
                    previous,
                    value,
                    temporary,
                ));
                if !state.record.overwrite_double_current(value) {
                    return Err(DoublePropertyError::BackingKind);
                }
                Some(Arc::new(ContextChangeArgument::Property(
                    ContextPropertyChangeArgs::<E>::with_undo(Some(undo)),
                )))
            }
        };
        if let Some(args) = args {
            self.emit(Some(args))?;
        }
        self.commit_previous()
    }

    pub fn set_speed(&self, value: f64) -> Result<(), DoublePropertyError<E>> {
        if !self.within_bounds(value)? {
            return Ok(());
        }
        let changed = {
            let mut state = self
                .0
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let (current, _) = state
                .record
                .double_pair()
                .ok_or(DoublePropertyError::BackingKind)?;
            if current == value {
                false
            } else if state.record.overwrite_double_current(value) {
                true
            } else {
                return Err(DoublePropertyError::BackingKind);
            }
        };
        if changed {
            self.emit(Some(Arc::new(ContextChangeArgument::Property(
                ContextPropertyChangeArgs::<E>::with_speed(true),
            ))))?;
        }
        Ok(())
    }

    pub fn set_limits(&self, limits: Option<SharedLimits>) -> Result<(), DoublePropertyError<E>> {
        let old = self.limits();
        let equal = match &old {
            Some(old) => old.source_equals(limits.as_ref()),
            None => limits.is_none(),
        };
        if equal {
            return Ok(());
        }
        let retired = {
            let mut state = self
                .0
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            std::mem::replace(&mut state.limits, limits)
        };
        drop(retired);
        let clipped = self.clip_value(self.pair()?.0)?;
        {
            let mut state = self
                .0
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if !state.record.overwrite_double_current(clipped) {
                return Err(DoublePropertyError::BackingKind);
            }
        }
        self.emit_empty()
    }

    pub fn cap_value(&self, value: f64) -> Result<f64, DoublePropertyError<E>> {
        let Some(limits) = self.limits() else {
            return Ok(value);
        };
        round_decimal_places(value, limits.snapshot().decimal_places)
            .map_err(DoublePropertyError::Rounding)
    }

    pub fn clip_value(&self, value: f64) -> Result<f64, DoublePropertyError<E>> {
        if self.limits().is_none() {
            return Ok(value);
        }
        let mut capped = self.cap_value(value)?;
        let limits = self.limits().ok_or(DoublePropertyError::MissingLimits)?;
        let minimum = limits.snapshot().minimum.to_f64();
        if capped < minimum {
            capped = minimum;
        }
        let maximum = limits.snapshot().maximum.to_f64();
        if capped > maximum {
            capped = maximum;
        }
        Ok(capped)
    }

    /// Wire reads mutate this same record in its characterized fixed creation
    /// context. Dynamic wrapper aliases/getters and wire callbacks are separate.
    pub fn load_wire(
        &self,
        reader: &mut BinaryCursor<'_>,
        version: f64,
    ) -> Result<(), DoublePropertyError<E>> {
        self.0
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .record
            .load(reader, version)
            .map_err(DoublePropertyError::Wire)
    }

    pub fn save_wire(&self, writer: &mut BinaryWriter) -> Result<(), DoublePropertyError<E>> {
        self.0
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .record
            .save(writer)
            .map_err(DoublePropertyError::Wire)
    }

    fn pair(&self) -> Result<(f64, f64), DoublePropertyError<E>> {
        self.0
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .record
            .double_pair()
            .ok_or(DoublePropertyError::BackingKind)
    }

    fn limits(&self) -> Option<SharedLimits> {
        self.0
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .limits
            .clone()
    }

    fn within_bounds(&self, value: f64) -> Result<bool, DoublePropertyError<E>> {
        let limits = self.limits().ok_or(DoublePropertyError::MissingLimits)?;
        if limits.snapshot().minimum.to_f64() <= value {
            Ok(value <= limits.snapshot().maximum.to_f64())
        } else {
            Ok(false)
        }
    }

    fn is_adjusted(record: &ModernPropertyRecord) -> bool {
        record.adjustment.as_ref().is_some_and(|adjustment| {
            adjustment
                .components()
                .iter()
                .any(|component| component.active)
        })
    }

    fn commit_previous(&self) -> Result<(), DoublePropertyError<E>> {
        let mut state = self
            .0
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if state.record.copy_double_current_to_previous() {
            Ok(())
        } else {
            Err(DoublePropertyError::BackingKind)
        }
    }

    fn emit(
        &self,
        args: Option<Arc<ContextChangeArgument<E>>>,
    ) -> Result<(), DoublePropertyError<E>> {
        let frame = self.clone();
        let envelope = Arc::new(Envelope {
            sender: Arc::downgrade(&frame.0),
            args,
        });
        frame
            .0
            .notifier
            .emit(Some(envelope))
            .map_err(DoublePropertyError::Notification)
    }

    fn emit_empty(&self) -> Result<(), DoublePropertyError<E>> {
        let frame = self.clone();
        frame
            .0
            .notifier
            .emit_empty()
            .map_err(DoublePropertyError::Notification)
    }
}

impl<E: 'static> DoublePropertyCore<E> {
    pub(crate) fn retain(&self) -> Result<SharedDoubleProperty<E>, DoublePropertyError<E>> {
        let sponsor = match &self.0.sponsor {
            Some(owner) => Some(owner.upgrade().ok_or(DoublePropertyError::Notification(
                ChangeDispatchError::Handler(DoubleCallbackError::SenderReleased),
            ))?),
            None => None,
        };
        Ok(SharedDoubleProperty(Arc::clone(&self.0), sponsor))
    }
    pub(crate) fn set_adjustment_bounds(&self, bounds: Vec<[f32; 2]>) {
        self.0
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .record
            .set_adjustment_bounds(bounds);
    }
    pub(crate) fn with_scene_record<T>(&self, f: impl FnOnce(&mut ModernPropertyRecord) -> T) -> T {
        f(&mut self
            .0
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .record)
    }
    pub(crate) fn record_snapshot(&self) -> ModernPropertyRecord {
        self.0
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .record
            .copy_record_snapshot()
    }
    pub(crate) fn set_raw_name(&self, name: Option<Vec<u16>>) {
        let retired = {
            let mut state = self
                .0
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            std::mem::replace(&mut state.record.name, name)
        };
        drop(retired);
    }
    pub(crate) fn overwrite_current(&self, value: f64) -> bool {
        self.0
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .record
            .overwrite_double_current(value)
    }
    pub(crate) fn load(
        &self,
        reader: &mut BinaryCursor<'_>,
        version: f64,
    ) -> Result<(), ModernPropertyError> {
        self.0
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .record
            .load(reader, version)
    }
    pub(crate) fn save(&self, writer: &mut BinaryWriter) -> Result<(), ModernPropertyError> {
        self.0
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .record
            .save(writer)
    }
}

#[derive(Debug)]
pub enum DoubleCallbackError<E> {
    Observer(E),
    SenderReleased,
}
impl<E: fmt::Display> fmt::Display for DoubleCallbackError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Observer(error) => error.fmt(formatter),
            Self::SenderReleased => {
                formatter.write_str("property sender released during notification")
            }
        }
    }
}
impl<E: Error + 'static> Error for DoubleCallbackError<E> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Observer(error) => Some(error),
            Self::SenderReleased => None,
        }
    }
}

#[derive(Debug)]
pub enum DoublePropertyError<E> {
    BackingKind,
    MissingLimits,
    Rounding(DecimalRoundingError),
    Notification(ChangeDispatchError<DoubleCallbackError<E>>),
    Wire(ModernPropertyError),
}
impl<E: fmt::Display> fmt::Display for DoublePropertyError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BackingKind => formatter.write_str("invalid Double property backing kind"),
            Self::MissingLimits => formatter.write_str("NullReferenceException"),
            Self::Rounding(error) => error.fmt(formatter),
            Self::Notification(error) => error.fmt(formatter),
            Self::Wire(error) => error.fmt(formatter),
        }
    }
}
impl<E: Error + 'static> Error for DoublePropertyError<E> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Rounding(error) => Some(error),
            Self::Notification(error) => Some(error),
            Self::Wire(error) => Some(error),
            Self::BackingKind | Self::MissingLimits => None,
        }
    }
}
