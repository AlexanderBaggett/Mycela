//! Retained zu/abl/mt history state. Document/slot/catalog services are explicit
//! dependencies; this module never reconstructs a node or stores value snapshots.
mod clock;
mod command;
mod contributors;
mod workspace;
pub use contributors::{
    ContributorHistoryDocument, ContributorHistoryHost, ContributorHistoryServices,
};
pub use workspace::WorkspaceHistoryRouter;
#[cfg(test)]
mod laws;
use crate::{ContextPropertyChangeArgs, ContextUndoInfo, DefaultContext};
use clock::Clock;
pub use clock::HistoryTimestamp;
pub(crate) use command::HistoryGraphEdit;
use command::{CommandCore, CommandRoots, Parent};
pub use command::{HistoryCommand, HistoryCommandList, HistoryPropertyTarget, HistoryTarget};
use std::{
    fmt,
    sync::{Arc, Mutex, MutexGuard},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HistoryDirection {
    Undo,
    Redo,
}

/// Actual foreign effect sites, with no implicit successful implementation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HistoryEffect {
    ResolveCommandContext,
    RoutePropertyHistory,
    CaptureContributors,
    CaptureResourceSelection,
    RestoreResourceSelection,
    FlushResourceSelection,
    RestoreContributors,
    SetGlobalBatch(bool),
    BeforeTravel,
    RefreshHistory,
    DirtyChanged,
    PendingResourceCommands,
    PropertyTarget,
    InsertionSlotWrapper,
    CommandTreeOwnership,
}

#[derive(Clone, Debug, PartialEq)]
pub enum HistoryError<E> {
    MissingDependency(HistoryEffect),
    Clock {
        site: &'static str,
        reason: &'static str,
    },
    NullUndo,
    NullTarget,
    InputChanged,
    BranchChanged,
    HistoryRetired,
    DetachedCommand,
    NullDocument,
    NullContributorList,
    ContributorIndex,
    ContributorCollectionModified,
    ListIndex {
        operation: &'static str,
        index: i32,
        length: usize,
    },
    Index {
        operation: &'static str,
        index: i32,
        length: usize,
    },
    CrossHistory,
    Service(E),
    Property(Box<crate::PointwiseError<E>>),
}
impl<E> HistoryError<E> {
    pub fn source_class(&self) -> Option<&'static str> {
        match self {
            Self::NullUndo
            | Self::NullTarget
            | Self::DetachedCommand
            | Self::NullDocument
            | Self::NullContributorList => Some("System.NullReferenceException"),
            Self::ListIndex { .. } | Self::ContributorIndex => {
                Some("System.ArgumentOutOfRangeException")
            }
            Self::ContributorCollectionModified => Some("System.InvalidOperationException"),
            Self::Index { .. } => Some("System.IndexOutOfRangeException"),
            Self::MissingDependency(_)
            | Self::Clock { .. }
            | Self::InputChanged
            | Self::BranchChanged
            | Self::HistoryRetired
            | Self::CrossHistory
            | Self::Service(_)
            | Self::Property(_) => None,
        }
    }
}
impl<E: fmt::Display> fmt::Display for HistoryError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingDependency(effect) => {
                write!(f, "History requires actual {effect:?} services")
            }
            Self::Clock { site, reason } => write!(f, "History clock at {site}: {reason}"),
            Self::NullTarget => f.write_str("Property history target has no current node"),
            Self::InputChanged => f.write_str("The input changed outside document history"),
            Self::BranchChanged => f.write_str("The branch changed outside document history"),
            Self::HistoryRetired => f.write_str("This document history was released"),
            Self::NullUndo => f.write_str("Property history requires a real undo root"),
            Self::NullDocument => {
                f.write_str("History contributor operation lost its active document")
            }
            Self::NullContributorList => {
                f.write_str("History contributor operation lost its captured list")
            }
            Self::ContributorIndex => {
                f.write_str("Contributor list index is outside the active document")
            }
            Self::ContributorCollectionModified => {
                f.write_str("Contributor list changed during enumeration")
            }
            Self::DetachedCommand => {
                f.write_str("Command lost its source parent history before suppression publication")
            }
            Self::ListIndex {
                operation,
                index,
                length,
            } => write!(f, "History {operation} list index {index} outside {length}"),
            Self::Index {
                operation,
                index,
                length,
            } => write!(f, "History {operation} index {index} outside {length}"),
            Self::CrossHistory => {
                f.write_str("Cross-history ownership promotion requires the document cohort")
            }
            Self::Service(error) => error.fmt(f),
            Self::Property(_) => f.write_str("History property setter failed"),
        }
    }
}
impl<E: std::error::Error + 'static> std::error::Error for HistoryError<E> {}

/// Real document/resource effects. Implementations must preserve their reached
/// source prefixes; an unavailable binding returns MissingDependency. These are
/// not notification callbacks and have no default/no-op success behavior.
pub trait HistoryServices<E>: Send + Sync {
    /// asy.b rereads the active editor's actual document and loading state
    /// after ordinary callbacks, INCLUDING events whose undo is null. Only
    /// after those reads and a nonnull undo may this service retrieve the real
    /// source document history. Null undo, absent document or loading returns
    /// None; attachment alone must not infer those ambient states.
    fn route_property_history(
        &self,
        owner: &History<E>,
        target: &HistoryTarget<E>,
        undo: Option<&ContextUndoInfo<E>>,
    ) -> Result<Option<History<E>>, HistoryError<E>>;
    /// abl constructor's actual NodesViewSlot/ancestor path, before contributor
    /// capture. Returned handle retains that exact source asy body (abl.b).
    fn resolve_command_context(
        &self,
        history: &History<E>,
        command: &HistoryCommand<E>,
    ) -> Result<Option<HistoryTarget<E>>, HistoryError<E>>;
    fn effect(
        &self,
        history: &History<E>,
        command: Option<&HistoryCommand<E>>,
        effect: HistoryEffect,
    ) -> Result<(), HistoryError<E>>;
    fn pending_commands(
        &self,
        history: &History<E>,
        command: &HistoryCommand<E>,
    ) -> Result<Vec<HistoryCommand<E>>, HistoryError<E>>;
}
#[derive(Debug)]
pub struct UnavailableHistoryServices;
impl<E> HistoryServices<E> for UnavailableHistoryServices {
    fn route_property_history(
        &self,
        _: &History<E>,
        _: &HistoryTarget<E>,
        _: Option<&ContextUndoInfo<E>>,
    ) -> Result<Option<History<E>>, HistoryError<E>> {
        Err(HistoryError::MissingDependency(
            HistoryEffect::RoutePropertyHistory,
        ))
    }
    fn resolve_command_context(
        &self,
        _: &History<E>,
        _: &HistoryCommand<E>,
    ) -> Result<Option<HistoryTarget<E>>, HistoryError<E>> {
        Err(HistoryError::MissingDependency(
            HistoryEffect::ResolveCommandContext,
        ))
    }
    fn effect(
        &self,
        _: &History<E>,
        _: Option<&HistoryCommand<E>>,
        effect: HistoryEffect,
    ) -> Result<(), HistoryError<E>> {
        Err(HistoryError::MissingDependency(effect))
    }
    fn pending_commands(
        &self,
        _: &History<E>,
        _: &HistoryCommand<E>,
    ) -> Result<Vec<HistoryCommand<E>>, HistoryError<E>> {
        Err(HistoryError::MissingDependency(
            HistoryEffect::PendingResourceCommands,
        ))
    }
}

pub(super) fn guard<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}
struct State<E> {
    commands: Vec<Arc<CommandCore<E>>>,
    cached: Option<CommandRoots<E>>,
    cursor: i32,
    recording: bool,
    retired: bool,
    suppressed: bool,
    stamp: HistoryTimestamp,
    dirty: i32,
    edit_sequence: u64,
    merge_boundary: bool,
}
pub(crate) struct HistoryCore<E> {
    state: Mutex<State<E>>,
    services: Arc<dyn HistoryServices<E>>,
    clock: Clock,
}
/// The retained history authority; cloning a handle preserves actual roots.
pub struct History<E = DefaultContext> {
    pub(crate) core: Arc<HistoryCore<E>>,
    pub(crate) owner: Option<Arc<dyn Send + Sync>>,
}
impl<E> Clone for History<E> {
    fn clone(&self) -> Self {
        Self {
            core: Arc::clone(&self.core),
            owner: self.owner.clone(),
        }
    }
}
impl<E> fmt::Debug for History<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = guard(&self.core.state);
        f.debug_struct("History")
            .field("cursor", &state.cursor)
            .field("length", &state.commands.len())
            .field("dirty", &state.dirty)
            .field("suppressed", &state.suppressed)
            .finish()
    }
}
impl<E: 'static> History<E> {
    pub fn new(services: Arc<dyn HistoryServices<E>>) -> Result<Self, HistoryError<E>> {
        Self::with_clock(services, Clock::Local)
    }
    fn with_clock(
        services: Arc<dyn HistoryServices<E>>,
        clock: Clock,
    ) -> Result<Self, HistoryError<E>> {
        // zu field initializer runs before the avh base constructor.
        let stamp = clock.now().map_err(|reason| HistoryError::Clock {
            site: "ConstructorNow",
            reason,
        })?;
        Ok(Self {
            core: Arc::new(HistoryCore {
                state: Mutex::new(State {
                    commands: Vec::new(),
                    cached: None,
                    cursor: 0,
                    recording: true,
                    retired: false,
                    suppressed: false,
                    stamp,
                    dirty: 0,
                    edit_sequence: 0,
                    merge_boundary: false,
                }),
                services,
                clock,
            }),
            owner: None,
        })
    }
    pub fn same_reference(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.core, &other.core)
    }
    pub fn cursor(&self) -> i32 {
        guard(&self.core.state).cursor
    }
    pub fn dirty_count(&self) -> i32 {
        guard(&self.core.state).dirty
    }
    pub fn is_dirty(&self) -> bool {
        self.dirty_count() != 0
    }
    /// Changes for an appended or merged edit, including a net-zero merge.
    pub fn edit_sequence(&self) -> u64 {
        guard(&self.core.state).edit_sequence
    }
    /// Keep the next edit independent of the currently saved document state.
    pub fn preserve_savepoint(&self) -> Result<(), HistoryError<E>> {
        self.finish_temporary()?;
        self.separate_next_edit();
        self.mark_saved()
    }
    /// Keep a discrete user action from merging with the previous command.
    pub fn separate_next_edit(&self) {
        guard(&self.core.state).merge_boundary = true;
    }
    pub fn is_recording(&self) -> bool {
        guard(&self.core.state).recording
    }
    pub fn is_suppressed(&self) -> bool {
        guard(&self.core.state).suppressed
    }
    pub fn stamp(&self) -> HistoryTimestamp {
        guard(&self.core.state).stamp
    }
    pub fn set_recording(&self, value: bool) {
        let mut state = guard(&self.core.state);
        state.recording = value && !state.retired;
    }
    pub(crate) fn retire_document_binding(&self) {
        let mut state = guard(&self.core.state);
        state.recording = false;
        state.retired = true;
    }
    pub(super) fn require_active(&self) -> Result<(), HistoryError<E>> {
        if guard(&self.core.state).retired {
            Err(HistoryError::HistoryRetired)
        } else {
            Ok(())
        }
    }
    pub fn commands(&self) -> HistoryCommandList<E> {
        let mut state = guard(&self.core.state);
        if state.cached.is_none() {
            state.cached = Some(state.commands.clone().into());
        }
        HistoryCommandList::new(
            Arc::clone(state.cached.as_ref().expect("cache initialized")),
            self.clone(),
        )
    }
    pub(crate) fn route_property_history(
        &self,
        target: &HistoryTarget<E>,
        undo: Option<&ContextUndoInfo<E>>,
    ) -> Result<Option<Self>, HistoryError<E>> {
        let current = self
            .core
            .services
            .route_property_history(self, target, undo)?;
        // The source null-undo gate follows ambient lookup, before history
        // cohort/suppression/command use, including a foreign service result.
        if undo.is_none() {
            return Ok(None);
        }
        if current
            .as_ref()
            .is_some_and(|current| !self.same_reference(current))
        {
            return Err(HistoryError::CrossHistory);
        }
        Ok(current.map(|mut current| {
            current.owner = self.owner.clone();
            current
        }))
    }
    pub fn property_command(
        &self,
        target: impl Into<HistoryTarget<E>>,
        args: Arc<ContextPropertyChangeArgs<E>>,
    ) -> Result<HistoryCommand<E>, HistoryError<E>> {
        let target = target.into();
        let mut history = self.clone();
        if history.owner.is_none() {
            history.owner = target.owner();
        }
        HistoryCommand::property(history, target, args)
    }
    pub(crate) fn record_graph_edit(
        &self,
        target: HistoryTarget<E>,
        label: Vec<u16>,
        edit: Box<dyn HistoryGraphEdit<E>>,
    ) -> Result<(), HistoryError<E>> {
        if self.is_suppressed() || !self.is_recording() {
            return Ok(());
        }
        let command = HistoryCommand::graph(self.clone(), target, label, edit)?;
        self.record(&command)
    }
    /// Remaining source insertion effects require a document-level host.
    pub fn insertion_unavailable(&self) -> HistoryError<E> {
        HistoryError::MissingDependency(HistoryEffect::InsertionSlotWrapper)
    }
    pub(super) fn effect(
        &self,
        command: Option<&HistoryCommand<E>>,
        effect: HistoryEffect,
    ) -> Result<(), HistoryError<E>> {
        self.core.services.effect(self, command, effect)
    }
    fn now(&self, site: &'static str) -> Result<HistoryTimestamp, HistoryError<E>> {
        self.core
            .clock
            .now()
            .map_err(|reason| HistoryError::Clock { site, reason })
    }
    fn at(
        list: &HistoryCommandList<E>,
        index: i32,
        operation: &'static str,
    ) -> Result<HistoryCommand<E>, HistoryError<E>> {
        list.get_signed(index).ok_or(HistoryError::Index {
            operation,
            index,
            length: list.len(),
        })
    }
    fn set_dirty(&self, value: i32) -> Result<(), HistoryError<E>> {
        let changed = {
            let mut state = guard(&self.core.state);
            if state.dirty == value {
                return Ok(());
            }
            let was_dirty = state.dirty != 0;
            state.dirty = value;
            was_dirty != (value != 0)
        };
        if changed {
            self.effect(None, HistoryEffect::DirtyChanged)?;
        }
        Ok(())
    }
    pub fn mark_saved(&self) -> Result<(), HistoryError<E>> {
        self.set_dirty(0)
    }
    pub fn finish_temporary(&self) -> Result<(), HistoryError<E>> {
        let cursor = self.cursor();
        if cursor > 0 {
            Self::at(&self.commands(), cursor - 1, "finish temporary")?.set_temporary(false);
        }
        Ok(())
    }
    pub fn record(&self, incoming: &HistoryCommand<E>) -> Result<(), HistoryError<E>> {
        {
            let state = guard(&self.core.state);
            if !state.recording || state.suppressed {
                return Ok(());
            }
        }
        incoming.require_cohort(self)?;
        let cursor = self.cursor();
        let previous = if cursor > 0 {
            Some(Self::at(&self.commands(), cursor - 1, "record previous")?)
        } else {
            None
        };
        let merge_window = if let Some(previous) = &previous {
            if previous.is_temporary() {
                true
            } else {
                // Clock occurs before incoming temporary/cursor-end/compatibility.
                let elapsed = self.now("MergeCheckNow")?.seconds_since(self.stamp());
                elapsed < 15.0 && !incoming.is_temporary()
            }
        } else {
            false
        };
        let merge_allowed = !guard(&self.core.state).merge_boundary;
        if merge_window
            && merge_allowed
            && self.cursor() as i64 == self.commands().len() as i64
            && previous.as_ref().is_some_and(|p| p.is_compatible(incoming))
        {
            let previous = previous.expect("merge predicate has previous");
            previous.merge(incoming)?;
            {
                let mut state = guard(&self.core.state);
                state.edit_sequence = state.edit_sequence.wrapping_add(1);
            }
            if previous.is_net_zero()? {
                self.remove(&previous);
                {
                    let mut state = guard(&self.core.state);
                    state.cursor = state.cursor.wrapping_sub(1);
                }
                self.set_dirty(self.dirty_count().wrapping_sub(1))?;
                self.effect(None, HistoryEffect::RefreshHistory)?;
            }
            return Ok(());
        }
        // Duplicate suppression is after failed merge and compares actual initial
        // undo references, independently of property/slot compatibility.
        if self.cursor() > 0 {
            let previous = Self::at(&self.commands(), self.cursor() - 1, "duplicate previous")?;
            if incoming.same_initial_undo(&previous) {
                return Ok(());
            }
        }
        if let Some(previous) = previous {
            previous.set_temporary(false);
        }
        let now = self.now("AppendStampNow")?;
        guard(&self.core.state).stamp = now;
        self.truncate_redo()?;
        self.append(incoming)?;
        {
            let mut state = guard(&self.core.state);
            state.cursor = state.cursor.wrapping_add(1);
            state.edit_sequence = state.edit_sequence.wrapping_add(1);
            state.merge_boundary = false;
        }
        if self.commands().len() > 40 {
            let first = Self::at(&self.commands(), 0, "capacity")?;
            self.remove(&first);
            let mut state = guard(&self.core.state);
            state.cursor = state.cursor.wrapping_sub(1);
        }
        // Refresh precedes both negative-dirty sentinel and increment.
        self.effect(None, HistoryEffect::RefreshHistory)?;
        if self.dirty_count() < 0 {
            self.set_dirty(10_000_000)?;
        }
        self.set_dirty(self.dirty_count().wrapping_add(1))
    }
    fn truncate_redo(&self) -> Result<(), HistoryError<E>> {
        let count = (self.commands().len() as i32).wrapping_sub(self.cursor());
        // avh.b loops its signed count: a negative count is an empty loop.
        for _ in 0..count {
            let cursor = self.cursor();
            let command = {
                let state = guard(&self.core.state);
                usize::try_from(cursor)
                    .ok()
                    .and_then(|ix| state.commands.get(ix))
                    .map(|core| HistoryCommand::from_core(Arc::clone(core), self.clone()))
                    .ok_or(HistoryError::ListIndex {
                        operation: "truncate redo",
                        index: cursor,
                        length: state.commands.len(),
                    })?
            };
            // Actual e(null) removes through the current parent; do not replace
            // this with truncating a snapshot of values/commands.
            command.detach();
        }
        Ok(())
    }
    fn append(&self, command: &HistoryCommand<E>) -> Result<(), HistoryError<E>> {
        command.require_cohort(self)?;
        let cached = {
            let mut state = guard(&self.core.state);
            state.commands.push(Arc::clone(&command.core));
            state.cached.take()
        };
        drop(cached);
        command.set_parent(Parent::History(Arc::downgrade(&self.core)));
        Ok(())
    }
    pub(super) fn remove(&self, command: &HistoryCommand<E>) {
        let (retired, cached) = {
            let mut state = guard(&self.core.state);
            let retired = state
                .commands
                .iter()
                .position(|c| Arc::ptr_eq(c, &command.core))
                .map(|ix| state.commands.remove(ix));
            (retired, state.cached.take())
        };
        command.detach();
        drop(cached);
        drop(retired);
    }
    pub fn undo(&self) -> Result<(), HistoryError<E>> {
        self.require_active()?;
        self.effect(None, HistoryEffect::BeforeTravel)?;
        let list = self.commands();
        let cursor = {
            let mut state = guard(&self.core.state);
            state.cursor = state.cursor.wrapping_sub(1);
            state.cursor
        };
        let command = Self::at(&list, cursor, "undo")?;
        command.set_temporary(false);
        command.undo()?;
        self.effect(None, HistoryEffect::RefreshHistory)?;
        self.set_dirty(self.dirty_count().wrapping_sub(1))
    }
    pub fn redo(&self) -> Result<(), HistoryError<E>> {
        self.require_active()?;
        self.effect(None, HistoryEffect::BeforeTravel)?;
        let list = self.commands();
        let cursor = {
            let mut state = guard(&self.core.state);
            let cursor = state.cursor;
            state.cursor = state.cursor.wrapping_add(1);
            cursor
        };
        let command = Self::at(&list, cursor, "redo")?;
        command.redo()?;
        self.effect(None, HistoryEffect::RefreshHistory)?;
        self.set_dirty(self.dirty_count().wrapping_add(1))
    }
}
