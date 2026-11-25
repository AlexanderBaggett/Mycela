use super::{History, HistoryCore, HistoryDirection, HistoryEffect, HistoryError, guard};
use crate::{
    ColorUndoObjectValue, ContextPropertyChangeArgs, ContextUndoInfo, DefaultContext,
    UndoObjectValue,
};
use std::{
    fmt,
    sync::{Arc, Mutex, Weak},
};

/// An actual retained slot-owner binding. Implementors resolve the current real
/// property by name/change/lab state when called, after callback reentry. The
/// same Arc must represent the exact retained source asy body throughout its
/// lifetime; making fresh adapters for the same slot is not a valid binding.
/// There is no snapshot restoration API or caller merge-decision callback.
pub trait HistoryPropertyTarget<E>: Send + Sync {
    fn apply(
        &self,
        undo: &ContextUndoInfo<E>,
        direction: HistoryDirection,
    ) -> Result<(), HistoryError<E>>;
}
pub(crate) trait HistoryGraphEdit<E>: Send + Sync {
    fn apply(&self, direction: HistoryDirection) -> Result<(), HistoryError<E>>;
}
/// An outward target retains its exact source body and the connected owner.
/// Internal command roots retain only `TargetRoot`, so document ownership has
/// no history -> target -> region -> history backedge.
pub struct HistoryTarget<E = DefaultContext> {
    root: TargetRoot<E>,
    owner: Option<Arc<dyn Send + Sync>>,
}
pub(crate) struct TargetRoot<E>(Arc<dyn HistoryPropertyTarget<E>>);
impl<E> Clone for TargetRoot<E> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}
impl<E> Clone for HistoryTarget<E> {
    fn clone(&self) -> Self {
        Self {
            root: self.root.clone(),
            owner: self.owner.clone(),
        }
    }
}
impl<E, T: HistoryPropertyTarget<E> + 'static> From<Arc<T>> for HistoryTarget<E> {
    fn from(root: Arc<T>) -> Self {
        Self {
            root: TargetRoot(root),
            owner: None,
        }
    }
}
impl<E> From<Arc<dyn HistoryPropertyTarget<E>>> for HistoryTarget<E> {
    fn from(root: Arc<dyn HistoryPropertyTarget<E>>) -> Self {
        Self {
            root: TargetRoot(root),
            owner: None,
        }
    }
}
impl<E> HistoryTarget<E> {
    pub fn same_reference(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.root.0, &other.root.0)
    }
    pub fn apply(
        &self,
        undo: &ContextUndoInfo<E>,
        direction: HistoryDirection,
    ) -> Result<(), HistoryError<E>> {
        self.root.0.apply(undo, direction)
    }
    pub(crate) fn owner(&self) -> Option<Arc<dyn Send + Sync>> {
        self.owner.clone()
    }
    pub(crate) fn sponsored(
        root: Arc<dyn HistoryPropertyTarget<E>>,
        owner: Arc<dyn Send + Sync>,
    ) -> Self {
        Self {
            root: TargetRoot(root),
            owner: Some(owner),
        }
    }
}
impl<E> TargetRoot<E> {
    fn outward(&self, owner: Option<Arc<dyn Send + Sync>>) -> HistoryTarget<E> {
        HistoryTarget {
            root: self.clone(),
            owner,
        }
    }
}
pub(super) enum Parent<E> {
    None,
    History(Weak<HistoryCore<E>>),
}
pub(super) type CommandRoots<E> = Arc<[Arc<CommandCore<E>>]>;
struct CommandState<E> {
    initial: Option<Arc<ContextPropertyChangeArgs<E>>>,
    context_target: Option<TargetRoot<E>>,
    captured_contributors: Option<Vec<crate::Contributor>>,
    latest: Option<Arc<ContextPropertyChangeArgs<E>>>,
    pending: bool,
    executed: bool,
    child_only: bool,
    children: Vec<Arc<CommandCore<E>>>,
    cached: Option<CommandRoots<E>>,
    parent: Parent<E>,
}
pub(super) struct CommandCore<E> {
    state: Mutex<CommandState<E>>,
    target: TargetRoot<E>,
    graph_edit: Option<Box<dyn HistoryGraphEdit<E>>>,
    label: Vec<u16>,
}
/// One actual property command/argument/undo identity. Internal lists retain
/// naked cores; this outward handle additionally sponsors the history cohort.
pub struct HistoryCommand<E = DefaultContext> {
    pub(super) core: Arc<CommandCore<E>>,
    sponsor: History<E>,
}
impl<E> Clone for HistoryCommand<E> {
    fn clone(&self) -> Self {
        Self {
            core: Arc::clone(&self.core),
            sponsor: self.sponsor.clone(),
        }
    }
}
impl<E> fmt::Debug for HistoryCommand<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = guard(&self.core.state);
        f.debug_struct("HistoryCommand")
            .field("label", &self.core.label)
            .field("pending", &state.pending)
            .field("executed", &state.executed)
            .field("children", &state.children.len())
            .finish()
    }
}
/// A held real cached command-array root. Later list changes do not rewrite it.
pub struct HistoryCommandList<E = DefaultContext> {
    cores: CommandRoots<E>,
    sponsor: History<E>,
}
impl<E> Clone for HistoryCommandList<E> {
    fn clone(&self) -> Self {
        Self {
            cores: Arc::clone(&self.cores),
            sponsor: self.sponsor.clone(),
        }
    }
}
impl<E> fmt::Debug for HistoryCommandList<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HistoryCommandList")
            .field("len", &self.cores.len())
            .finish()
    }
}
impl<E: 'static> HistoryCommandList<E> {
    pub(super) fn new(cores: CommandRoots<E>, sponsor: History<E>) -> Self {
        Self { cores, sponsor }
    }
    pub fn len(&self) -> usize {
        self.cores.len()
    }
    pub fn is_empty(&self) -> bool {
        self.cores.is_empty()
    }
    pub fn same_reference(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.cores, &other.cores)
    }
    pub fn get(&self, ix: usize) -> Option<HistoryCommand<E>> {
        self.cores
            .get(ix)
            .map(|core| HistoryCommand::from_core(Arc::clone(core), self.sponsor.clone()))
    }
    pub(super) fn get_signed(&self, ix: i32) -> Option<HistoryCommand<E>> {
        usize::try_from(ix).ok().and_then(|ix| self.get(ix))
    }
}
fn undo<E>(args: &Arc<ContextPropertyChangeArgs<E>>) -> &ContextUndoInfo<E> {
    args.typed_undo_info()
        .expect("actual undo checked before command publication")
}
fn name<E>(value: &ContextUndoInfo<E>) -> Option<&[u16]> {
    match value {
        ContextUndoInfo::Double(v) => v.name(),
        ContextUndoInfo::Modern(v) => v.name(),
        ContextUndoInfo::Pair(v) => v.name(),
        ContextUndoInfo::Choice(v) => v.name(),
        ContextUndoInfo::Color(v) => v.name(),
        ContextUndoInfo::Animation(v) => v.name(),
        ContextUndoInfo::Environment(v) => v.name(),
    }
}
fn change<E>(value: &ContextUndoInfo<E>) -> i32 {
    match value {
        ContextUndoInfo::Double(v) => v.property_change(),
        ContextUndoInfo::Modern(v) => v.property_change(),
        ContextUndoInfo::Pair(v) => v.property_change(),
        ContextUndoInfo::Choice(v) => v.property_change(),
        ContextUndoInfo::Color(v) => v.property_change(),
        ContextUndoInfo::Animation(v) => v.property_change(),
        ContextUndoInfo::Environment(v) => v.property_change(),
    }
}
fn temporary<E>(value: &ContextUndoInfo<E>) -> bool {
    match value {
        ContextUndoInfo::Double(v) => v.temporary_change(),
        ContextUndoInfo::Modern(v) => v.temporary_change(),
        ContextUndoInfo::Pair(v) => v.temporary_change(),
        ContextUndoInfo::Choice(v) => v.temporary_change(),
        ContextUndoInfo::Color(v) => v.temporary_change(),
        ContextUndoInfo::Animation(v) => v.temporary_change(),
        ContextUndoInfo::Environment(v) => v.temporary_change(),
    }
}
fn set_temporary<E>(value: &ContextUndoInfo<E>, temporary: bool) {
    match value {
        ContextUndoInfo::Double(v) => v.set_temporary_change(temporary),
        ContextUndoInfo::Modern(v) => v.set_temporary_change(temporary),
        ContextUndoInfo::Pair(v) => v.set_temporary_change(temporary),
        ContextUndoInfo::Choice(v) => v.set_temporary_change(temporary),
        ContextUndoInfo::Color(v) => v.set_temporary_change(temporary),
        ContextUndoInfo::Animation(v) => v.set_temporary_change(temporary),
        ContextUndoInfo::Environment(v) => v.set_temporary_change(temporary),
    }
}
fn same_undo<E>(a: &ContextUndoInfo<E>, b: &ContextUndoInfo<E>) -> bool {
    match (a, b) {
        (ContextUndoInfo::Double(a), ContextUndoInfo::Double(b)) => Arc::ptr_eq(a, b),
        (ContextUndoInfo::Modern(a), ContextUndoInfo::Modern(b)) => Arc::ptr_eq(a, b),
        (ContextUndoInfo::Pair(a), ContextUndoInfo::Pair(b)) => Arc::ptr_eq(a, b),
        (ContextUndoInfo::Choice(a), ContextUndoInfo::Choice(b)) => Arc::ptr_eq(a, b),
        (ContextUndoInfo::Color(a), ContextUndoInfo::Color(b)) => Arc::ptr_eq(a, b),
        (ContextUndoInfo::Animation(a), ContextUndoInfo::Animation(b)) => Arc::ptr_eq(a, b),
        (ContextUndoInfo::Environment(a), ContextUndoInfo::Environment(b)) => Arc::ptr_eq(a, b),
        _ => false,
    }
}
fn net_zero<E: 'static>(
    first: &ContextUndoInfo<E>,
    latest: &ContextUndoInfo<E>,
) -> Result<bool, HistoryError<E>> {
    Ok(match (first, latest) {
        (ContextUndoInfo::Double(first), ContextUndoInfo::Double(latest)) => {
            first.equals_old_object(UndoObjectValue::Double(latest.new_value()))
        }
        (ContextUndoInfo::Modern(first), ContextUndoInfo::Modern(latest)) => {
            first.old_value() == latest.new_value()
        }
        (ContextUndoInfo::Pair(first), ContextUndoInfo::Pair(latest)) => {
            first.old_value().same_reference(&latest.new_value())
        }
        (ContextUndoInfo::Choice(first), ContextUndoInfo::Choice(latest)) => {
            first.old_value().source_equals(latest.new_value())
        }
        (ContextUndoInfo::Color(first), ContextUndoInfo::Color(latest)) => {
            first.equals_old_object(ColorUndoObjectValue::Color(latest.new_value()))
        }
        (ContextUndoInfo::Animation(first), ContextUndoInfo::Animation(latest)) => {
            match (first.old_value(), latest.new_value()) {
                (Some(old), Some(new)) => old
                    .source_equals_object(crate::SourceObject::Animation(&new))
                    .map_err(|error| HistoryError::Property(Box::new(error.into())))?,
                (None, None) => true,
                _ => false,
            }
        }
        (ContextUndoInfo::Environment(first), ContextUndoInfo::Environment(latest)) => {
            Arc::ptr_eq(&first.old_value(), &latest.new_value())
        }
        _ => false,
    })
}
impl<E: 'static> HistoryCommand<E> {
    pub(super) fn from_core(core: Arc<CommandCore<E>>, sponsor: History<E>) -> Self {
        Self { core, sponsor }
    }
    pub(super) fn property(
        history: History<E>,
        target: HistoryTarget<E>,
        args: Arc<ContextPropertyChangeArgs<E>>,
    ) -> Result<Self, HistoryError<E>> {
        let value = args.typed_undo_info().ok_or(HistoryError::NullUndo)?;
        let mut label: Vec<u16> = "Modify ".encode_utf16().collect();
        if let Some(name) = name(value) {
            label.extend_from_slice(name);
        }
        let suffix = if change(value) == 13 {
            " Animation"
        } else {
            " Property"
        };
        label.extend(suffix.encode_utf16());
        Self::recorded(history, target, label, Some(args), None)
    }
    pub(super) fn graph(
        history: History<E>,
        target: HistoryTarget<E>,
        label: Vec<u16>,
        edit: Box<dyn HistoryGraphEdit<E>>,
    ) -> Result<Self, HistoryError<E>> {
        Self::recorded(history, target, label, None, Some(edit))
    }
    fn recorded(
        history: History<E>,
        target: HistoryTarget<E>,
        label: Vec<u16>,
        args: Option<Arc<ContextPropertyChangeArgs<E>>>,
        graph_edit: Option<Box<dyn HistoryGraphEdit<E>>>,
    ) -> Result<Self, HistoryError<E>> {
        let command = Self {
            core: Arc::new(CommandCore {
                state: Mutex::new(CommandState {
                    initial: None,
                    context_target: None,
                    captured_contributors: None,
                    latest: None,
                    pending: false,
                    executed: true,
                    child_only: false,
                    children: Vec::new(),
                    cached: None,
                    parent: Parent::History(Arc::downgrade(&history.core)),
                }),
                target: target.root,
                graph_edit,
                label,
            }),
            sponsor: history.clone(),
        };
        let context = history
            .core
            .services
            .resolve_command_context(&history, &command)?;
        guard(&command.core.state).context_target = context.map(|target| target.root);
        if command.core.graph_edit.is_some() {
            // The graph change has already executed. abl.n captures the
            // resulting resource selection before adding its contributors.
            history.effect(Some(&command), HistoryEffect::CaptureResourceSelection)?;
            history.effect(Some(&command), HistoryEffect::CaptureContributors)?;
            return Ok(command);
        }
        // abl(false) contributor effects precede mt's assignment of its undo.
        history.effect(Some(&command), HistoryEffect::CaptureContributors)?;
        guard(&command.core.state).initial = args;
        history.effect(Some(&command), HistoryEffect::CaptureResourceSelection)?;
        let pending = history.core.services.pending_commands(&history, &command)?;
        for child in pending {
            command.add_child(&child)?;
        }
        Ok(command)
    }
    /// Retain the actual identities returned by `eb.d` on this command, in
    /// the source `abl.b` assignment order. The caller must resolve the real
    /// document and supply its actual contributor effects before calling.
    pub fn capture_document_contributors<F>(
        &self,
        owner: &crate::DocumentContributors,
        additional: Option<&[crate::Contributor]>,
        effects: &mut impl crate::ContributorEffects<F>,
    ) -> Result<(), crate::ContributorError<F>> {
        let inserted = owner.capture(additional, effects)?;
        guard(&self.core.state).captured_contributors = inserted;
        Ok(())
    }
    /// Source `abl.k`: if the last capture inserted identities, remove those
    /// exact roots from the resolved actual document and issue its notification.
    pub fn restore_document_contributors<F>(
        &self,
        owner: &crate::DocumentContributors,
        effects: &mut impl crate::ContributorEffects<F>,
    ) -> Result<(), crate::ContributorError<F>> {
        let entries = guard(&self.core.state).captured_contributors.clone();
        if let Some(entries) = entries {
            owner.remove_captured(&entries, effects)?;
        }
        Ok(())
    }
    pub fn captured_contributors(&self) -> Option<Vec<crate::Contributor>> {
        guard(&self.core.state).captured_contributors.clone()
    }
    pub fn same_reference(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.core, &other.core)
    }
    pub fn label(&self) -> &[u16] {
        &self.core.label
    }
    pub fn context_target(&self) -> Option<HistoryTarget<E>> {
        guard(&self.core.state)
            .context_target
            .as_ref()
            .map(|root| root.outward(self.sponsor.owner.clone()))
    }
    pub fn initial_arguments(&self) -> Option<Arc<ContextPropertyChangeArgs<E>>> {
        guard(&self.core.state).initial.clone()
    }
    pub fn latest_arguments(&self) -> Option<Arc<ContextPropertyChangeArgs<E>>> {
        guard(&self.core.state).latest.clone()
    }
    pub fn is_pending(&self) -> bool {
        guard(&self.core.state).pending
    }
    pub fn was_executed(&self) -> bool {
        guard(&self.core.state).executed
    }
    pub fn is_child_only(&self) -> bool {
        guard(&self.core.state).child_only
    }
    pub fn set_child_only(&self, value: bool) {
        guard(&self.core.state).child_only = value;
    }
    pub fn parent_history(&self) -> Option<History<E>> {
        match &guard(&self.core.state).parent {
            Parent::History(h) => Some(History {
                core: h
                    .upgrade()
                    .expect("outward command sponsorship retains actual parent history"),
                owner: self.sponsor.owner.clone(),
            }),
            _ => None,
        }
    }
    pub fn children(&self) -> HistoryCommandList<E> {
        let mut state = guard(&self.core.state);
        if state.cached.is_none() {
            state.cached = Some(state.children.clone().into());
        }
        HistoryCommandList::new(
            Arc::clone(state.cached.as_ref().expect("cache initialized")),
            self.sponsor.clone(),
        )
    }
    pub(super) fn require_cohort(&self, history: &History<E>) -> Result<(), HistoryError<E>> {
        if self.sponsor.same_reference(history) {
            Ok(())
        } else {
            Err(HistoryError::CrossHistory)
        }
    }
    pub(super) fn set_parent(&self, parent: Parent<E>) {
        let same = {
            let state = guard(&self.core.state);
            match (&state.parent, &parent) {
                (Parent::None, Parent::None) => true,
                (Parent::History(a), Parent::History(b)) => Weak::ptr_eq(a, b),
                _ => false,
            }
        };
        if same {
            return;
        }
        self.detach();
        guard(&self.core.state).parent = parent;
    }
    pub(super) fn detach(&self) {
        let parent = {
            let mut state = guard(&self.core.state);
            std::mem::replace(&mut state.parent, Parent::None)
        };
        match parent {
            Parent::None => {}
            Parent::History(h) => History {
                core: h
                    .upgrade()
                    .expect("dispatch sponsorship retains actual parent history"),
                owner: self.sponsor.owner.clone(),
            }
            .remove(self),
        }
    }
    /// Nonempty command trees require the real closed document ownership cohort.
    /// Refuse before mutation; a weak parent under a retired command is unsafe.
    pub fn add_child(&self, _child: &Self) -> Result<(), HistoryError<E>> {
        Err(HistoryError::MissingDependency(
            HistoryEffect::CommandTreeOwnership,
        ))
    }
    pub fn is_temporary(&self) -> bool {
        self.initial_arguments()
            .is_some_and(|a| temporary(undo(&a)))
    }
    pub(super) fn set_temporary(&self, value: bool) {
        if let Some(args) = self.initial_arguments() {
            set_temporary(undo(&args), value);
        }
    }
    pub(super) fn is_compatible(&self, incoming: &Self) -> bool {
        let (Some(a), Some(b)) = (self.initial_arguments(), incoming.initial_arguments()) else {
            return false;
        };
        Arc::ptr_eq(&self.core.target.0, &incoming.core.target.0)
            && name(undo(&a)) == name(undo(&b))
            && change(undo(&a)) == change(undo(&b))
    }
    pub(super) fn same_initial_undo(&self, other: &Self) -> bool {
        let (Some(a), Some(b)) = (self.initial_arguments(), other.initial_arguments()) else {
            return false;
        };
        same_undo(undo(&a), undo(&b))
    }
    pub(super) fn merge(&self, incoming: &Self) -> Result<(), HistoryError<E>> {
        let latest = incoming.initial_arguments();
        let old = {
            let mut state = guard(&self.core.state);
            std::mem::replace(&mut state.latest, latest)
        };
        drop(old);
        let children = incoming.children();
        for ix in 0..children.len() {
            self.add_child(&children.get(ix).expect("held child index"))?;
        }
        Ok(())
    }
    pub(super) fn is_net_zero(&self) -> Result<bool, HistoryError<E>> {
        let (Some(first), Some(latest)) = (self.initial_arguments(), self.latest_arguments())
        else {
            return Ok(false);
        };
        let change = change(undo(&first));
        if matches!(change, 10..=12) {
            return Ok(false);
        }
        Ok(net_zero(undo(&first), undo(&latest))? && (change != 4 || self.children().is_empty()))
    }
    pub fn redo(&self) -> Result<(), HistoryError<E>> {
        self.travel(HistoryDirection::Redo)
    }
    pub fn undo(&self) -> Result<(), HistoryError<E>> {
        self.travel(HistoryDirection::Undo)
    }
    fn travel(&self, direction: HistoryDirection) -> Result<(), HistoryError<E>> {
        self.sponsor.require_active()?;
        let pending = self.is_pending();
        if pending != (direction == HistoryDirection::Redo) {
            return Ok(());
        }
        if let Some(history) = self.parent_history() {
            history.effect(Some(self), HistoryEffect::SetGlobalBatch(true))?;
            // ali.c may reenter and detach the command. Source h() is reread
            // before writing d, and this occurs before entering its try/finally.
            let current = self.parent_history().ok_or(HistoryError::DetachedCommand)?;
            guard(&current.core.state).suppressed = true;
        }
        let body = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
            || -> Result<(), HistoryError<E>> {
                if !self.is_child_only() {
                    self.sponsor
                        .effect(Some(self), HistoryEffect::RestoreResourceSelection)?;
                }
                // jq.a is the actual property-command default: execute before children,
                // undo after children. Children always traverse the cached forward order.
                if direction == HistoryDirection::Redo {
                    self.apply(direction)?;
                }
                let children = self.children();
                for ix in 0..children.len() {
                    let child = children.get(ix).expect("held child index");
                    match direction {
                        HistoryDirection::Undo => child.undo()?,
                        HistoryDirection::Redo => child.redo()?,
                    }
                }
                if direction == HistoryDirection::Undo {
                    self.apply(direction)?;
                }
                if !self.is_child_only() {
                    self.sponsor
                        .effect(Some(self), HistoryEffect::CaptureResourceSelection)?;
                }
                guard(&self.core.state).pending = direction == HistoryDirection::Undo;
                Ok(())
            },
        ));
        // Actual finally rereads current h(); it is a bool, not a nesting counter.
        // A finally failure supersedes the earlier Result and retains publications.
        if let Some(history) = self.parent_history() {
            guard(&history.core.state).suppressed = false;
            history.effect(Some(self), HistoryEffect::SetGlobalBatch(false))?;
            history.effect(Some(self), HistoryEffect::FlushResourceSelection)?;
        }
        match body {
            Ok(result) => result?,
            Err(payload) => std::panic::resume_unwind(payload),
        }
        if direction == HistoryDirection::Undo {
            self.sponsor
                .effect(Some(self), HistoryEffect::RestoreContributors)?;
        } else {
            self.sponsor
                .effect(Some(self), HistoryEffect::CaptureContributors)?;
            guard(&self.core.state).executed = true;
        }
        Ok(())
    }
    fn apply(&self, direction: HistoryDirection) -> Result<(), HistoryError<E>> {
        if let Some(edit) = &self.core.graph_edit {
            return edit.apply(direction);
        }
        let args = match direction {
            HistoryDirection::Undo => self.initial_arguments(),
            HistoryDirection::Redo => self.latest_arguments().or_else(|| self.initial_arguments()),
        }
        .ok_or(HistoryError::NullUndo)?;
        self.core.target.0.apply(undo(&args), direction)
    }
}
