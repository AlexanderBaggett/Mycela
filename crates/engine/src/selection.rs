//! Retained ordinary legacy selection properties owned by genuine Invert nodes.
//! Animation/adjustment wrappers and ancestor resolution are separate required
//! capabilities; this layer exposes only the characterized unattached lifecycle.
use crate::{
    DefaultContext,
    ordinary_events::{DelegateCore, HandlerCore},
    selection_region::{NodeId, Sponsor},
};
use std::{
    fmt,
    sync::{Arc, Mutex},
};
pub type SelectionText = Option<Vec<u16>>;
#[derive(Debug)]
pub enum SelectionRenderError<E = DefaultContext> {
    Property(SelectionError<E>),
    Image(crate::ImageError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SelectionCloneMode {
    Original,
    Clone,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SelectionError<E = DefaultContext> {
    Index,
    NullReference,
    InvalidCast,
    UnrecognizedText,
    Observer(E),
}
impl<E: fmt::Debug> fmt::Display for SelectionError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl<E: fmt::Debug> std::error::Error for SelectionError<E> {}
/// Shared mutable nullable UTF16 option array. Cloning retains the same array.
#[derive(Clone, Debug)]
pub struct SharedSelectionOptions(Arc<Mutex<Vec<SelectionText>>>);
impl SharedSelectionOptions {
    pub fn new(entries: Vec<SelectionText>) -> Self {
        Self(Arc::new(Mutex::new(entries)))
    }
    pub fn entries(&self) -> Vec<SelectionText> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    pub fn same_reference(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    pub fn set_entry(&self, index: i32, text: SelectionText) -> Result<(), SelectionError> {
        let mut entries = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let slot = usize::try_from(index)
            .ok()
            .and_then(|i| entries.get_mut(i))
            .ok_or(SelectionError::Index)?;
        *slot = text;
        Ok(())
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SelectionValue {
    Integer(i32),
    Stamp(SelectionStamp),
    Text(Vec<u16>),
    Null,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SelectionStamp {
    current: i32,
    tracked_index: i32,
    tracked_text: SelectionText,
}
impl SelectionStamp {
    pub fn new(current: i32, tracked_index: i32, tracked_text: SelectionText) -> Self {
        Self {
            current,
            tracked_index,
            tracked_text,
        }
    }
    pub fn current(&self) -> i32 {
        self.current
    }
    pub fn tracked_index(&self) -> i32 {
        self.tracked_index
    }
    pub fn tracked_text(&self) -> SelectionText {
        self.tracked_text.clone()
    }
}
/// Copied observation of one authoritative ordinary selection state.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct SelectionSnapshot {
    current: i32,
    previous: i32,
    tracking: bool,
    tracked_index: i32,
    tracked_text: SelectionText,
    options: SharedSelectionOptions,
    name: Vec<u16>,
    allow_changes: bool,
    enabled: bool,
    binding_index: i32,
    binding_name: SelectionText,
}

#[derive(PartialEq)]
pub(crate) struct SelectionPosition {
    current: i32,
    previous: i32,
    tracked_index: i32,
    tracked_text: SelectionText,
}

impl SelectionSnapshot {
    pub fn current(&self) -> i32 {
        self.current
    }
    pub fn previous(&self) -> i32 {
        self.previous
    }
    pub fn is_tracking(&self) -> bool {
        self.tracking
    }
    pub fn tracked_index(&self) -> i32 {
        self.tracked_index
    }
    pub fn tracked_text(&self) -> SelectionText {
        self.tracked_text.clone()
    }
    pub fn options(&self) -> SharedSelectionOptions {
        self.options.clone()
    }
    pub fn name(&self) -> &[u16] {
        &self.name
    }
    pub fn allows_changes(&self) -> bool {
        self.allow_changes
    }
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }
    pub fn binding_index(&self) -> i32 {
        self.binding_index
    }
    pub fn binding_name(&self) -> SelectionText {
        self.binding_name.clone()
    }

    fn position(&self) -> SelectionPosition {
        SelectionPosition {
            current: self.current,
            previous: self.previous,
            tracked_index: self.tracked_index,
            tracked_text: self.tracked_text.clone(),
        }
    }

    fn commit_index(&mut self, value: i32) -> bool {
        if value < 0 || (self.current == value && self.previous == value) {
            return false;
        }
        self.current = value;
        self.previous = value;
        if self.tracking {
            self.tracked_index = value;
            if let Some(text) = self.options.entries().get(value as usize) {
                self.tracked_text = text.clone();
            }
        }
        true
    }
}
#[derive(Debug)]
pub struct SelectionEventArgs {
    speed: bool,
    controls_only: bool,
}
impl SelectionEventArgs {
    pub fn is_speed_rendering(&self) -> bool {
        self.speed
    }
    pub fn is_update_controls_only(&self) -> bool {
        self.controls_only
    }
}
type Events<E> = DelegateCore<SelectionSender<E>, Arc<SelectionEventArgs>, E>;
pub struct SelectionHandler<E = DefaultContext>(
    Arc<HandlerCore<SelectionSender<E>, Arc<SelectionEventArgs>, E>>,
);
impl<E> Clone for SelectionHandler<E> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}
impl<E> SelectionHandler<E> {
    pub fn new(
        callback: impl Fn(&SelectionSender<E>, Arc<SelectionEventArgs>) -> Result<(), E>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self(Arc::new(HandlerCore {
            callback: Box::new(callback),
        }))
    }
    pub fn same_binding(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
pub struct SelectionDelegate<E = DefaultContext> {
    core: Events<E>,
    owner: SharedInvertNode<E>,
}
impl<E> Clone for SelectionDelegate<E> {
    fn clone(&self) -> Self {
        Self {
            core: self.core.clone(),
            owner: self.owner.clone(),
        }
    }
}
impl<E> SelectionDelegate<E> {
    pub fn same_reference(&self, other: &Self) -> bool {
        self.core.same(&other.core)
    }
    pub fn is_null(&self) -> bool {
        self.core.is_absent()
    }
}
pub enum SelectionSender<E = DefaultContext> {
    Node(SharedInvertNode<E>),
    Property(SharedSelectionProperty<E>),
}
impl<E> Clone for SelectionSender<E> {
    fn clone(&self) -> Self {
        match self {
            Self::Node(n) => Self::Node(n.clone()),
            Self::Property(p) => Self::Property(p.clone()),
        }
    }
}
struct PropertyState<E> {
    snapshot: SelectionSnapshot,
    parent: Option<NodeId>,
    changed: Events<E>,
}
pub(crate) struct NodeCore<E> {
    pub(crate) id: NodeId,
    mode: SelectionCloneMode,
    state: Mutex<NodeState<E>>,
}
struct NodeState<E> {
    properties: [PropertyState<E>; 4],
    changed: Events<E>,
}
pub struct SharedInvertNode<E = DefaultContext> {
    core: Arc<NodeCore<E>>,
    sponsor: Arc<Sponsor<E>>,
}
pub struct SharedSelectionProperty<E = DefaultContext> {
    owner: SharedInvertNode<E>,
    channel: usize,
}
impl<E> Clone for SharedInvertNode<E> {
    fn clone(&self) -> Self {
        Self {
            core: Arc::clone(&self.core),
            sponsor: Arc::clone(&self.sponsor),
        }
    }
}
impl<E> Clone for SharedSelectionProperty<E> {
    fn clone(&self) -> Self {
        Self {
            owner: self.owner.clone(),
            channel: self.channel,
        }
    }
}
impl<E> fmt::Debug for SharedInvertNode<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("SharedInvertNode")
            .field(&self.core.id)
            .finish()
    }
}
impl<E> fmt::Debug for SharedSelectionProperty<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("SharedSelectionProperty")
            .field(&self.owner.core.id)
            .field(&self.channel)
            .finish()
    }
}
fn text(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}
fn forwarder<E>() -> Events<E> {
    Events::Single(Arc::new(HandlerCore {
        callback: Box::new(|sender, args| {
            if let SelectionSender::Property(p) = sender
                && p.owner.mode() == SelectionCloneMode::Original
                && !args.speed
                && !args.controls_only
            {
                let callbacks = {
                    p.owner
                        .core
                        .state
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .changed
                        .clone()
                };
                return callbacks.invoke(&SelectionSender::Node(p.owner.clone()), args);
            }
            Ok(())
        }),
    }))
}

impl<E> SharedInvertNode<E> {
    pub fn new(mode: SelectionCloneMode) -> Self {
        let id = NodeId::new();
        let properties = std::array::from_fn(|channel| {
            let value = i32::from(channel == 3);
            PropertyState {
                snapshot: SelectionSnapshot {
                    current: value,
                    previous: value,
                    tracking: false,
                    tracked_index: 0,
                    tracked_text: None,
                    options: SharedSelectionOptions::new(vec![Some(text("Yes")), Some(text("No"))]),
                    name: text(
                        [
                            "Affect Red",
                            "Affect Green",
                            "Affect Blue",
                            "Affect Transparency",
                        ][channel],
                    ),
                    allow_changes: true,
                    enabled: true,
                    binding_index: -1,
                    binding_name: None,
                },
                parent: Some(id),
                changed: forwarder(),
            }
        });
        let core = Arc::new(NodeCore {
            id,
            mode,
            state: Mutex::new(NodeState {
                properties,
                changed: Events::Absent,
            }),
        });
        let sponsor = Sponsor::publish(&core);
        Self { core, sponsor }
    }
    /// Raw detached render owner. Parent links are copied through sponsored IDs,
    /// before any selection resolution; callbacks and caches are not frame data.
    pub(crate) fn capture_raw(
        &self,
        memo: &mut Vec<(Self, Self)>,
        options: &mut impl FnMut(&SharedSelectionOptions) -> SharedSelectionOptions,
    ) -> Self {
        if let Some((_, copy)) = memo.iter().find(|(old, _)| old.same_reference(self)) {
            return copy.clone();
        }
        let (snapshots, parents) = {
            let state = self.core.state.lock().unwrap_or_else(|e| e.into_inner());
            (
                std::array::from_fn::<_, 4, _>(|i| state.properties[i].snapshot.clone()),
                std::array::from_fn::<_, 4, _>(|i| state.properties[i].parent),
            )
        };
        let id = NodeId::new();
        let properties = snapshots.map(|mut snapshot| {
            snapshot.options = options(&snapshot.options);
            PropertyState {
                snapshot,
                parent: None,
                changed: Events::Absent,
            }
        });
        let core = Arc::new(NodeCore {
            id,
            mode: self.core.mode,
            state: Mutex::new(NodeState {
                properties,
                changed: Events::Absent,
            }),
        });
        let copy = Self {
            sponsor: Sponsor::publish(&core),
            core,
        };
        memo.push((self.clone(), copy.clone()));
        let mut copied = [None; 4];
        for (i, parent) in parents.into_iter().enumerate() {
            if let Some(parent) = parent {
                let parent = Self {
                    core: self.sponsor.node(parent),
                    sponsor: self.sponsor.clone(),
                };
                let target = parent.capture_raw(memo, options);
                copy.sponsor.merge(&target.sponsor);
                copied[i] = Some(target.core.id);
            }
        }
        let mut state = copy.core.state.lock().unwrap_or_else(|e| e.into_inner());
        for (p, parent) in state.properties.iter_mut().zip(copied) {
            p.parent = parent;
        }
        drop(state);
        copy
    }

    pub(crate) fn copy_for_final_render(&self) -> Self {
        let copy = Self::new(SelectionCloneMode::Clone);
        let source = self.core.state.lock().unwrap_or_else(|e| e.into_inner());
        let mut target = copy.core.state.lock().unwrap_or_else(|e| e.into_inner());
        for (source, target) in source.properties.iter().zip(&mut target.properties) {
            let from = &source.snapshot;
            let to = &mut target.snapshot;
            // zl.s8 creates xe with literal options, then os.CloneFrom
            // applies the stored index and the original adjustment binding.
            if from.current >= 0 {
                to.current = from.current;
                to.previous = from.current;
            }
            to.binding_index = from.binding_index;
            to.binding_name.clone_from(&from.binding_name);
        }
        drop(target);
        copy
    }

    pub(crate) fn raw_capture_values(
        &self,
        options: &mut impl FnMut(&SharedSelectionOptions, &mut Vec<u64>),
    ) -> Vec<u64> {
        fn visit<E>(
            node: &SharedInvertNode<E>,
            seen: &mut Vec<NodeId>,
            out: &mut Vec<u64>,
            options: &mut impl FnMut(&SharedSelectionOptions, &mut Vec<u64>),
        ) {
            if let Some(id) = seen.iter().position(|v| *v == node.core.id) {
                out.extend([0, id as u64]);
                return;
            }
            seen.push(node.core.id);
            out.extend([1, (seen.len() - 1) as u64, node.core.mode as u64]);
            let (snapshots, parents) = {
                let state = node.core.state.lock().unwrap_or_else(|e| e.into_inner());
                (
                    std::array::from_fn::<_, 4, _>(|i| state.properties[i].snapshot.clone()),
                    std::array::from_fn::<_, 4, _>(|i| state.properties[i].parent),
                )
            };
            fn text(t: &Option<Vec<u16>>, out: &mut Vec<u64>) {
                match t {
                    None => out.push(0),
                    Some(t) => {
                        out.push(t.len() as u64 + 1);
                        out.extend(t.iter().copied().map(u64::from));
                    }
                }
            }
            for (s, parent) in snapshots.into_iter().zip(parents) {
                out.extend([
                    s.current as u64,
                    s.previous as u64,
                    s.tracking as u64,
                    s.tracked_index as u64,
                    s.allow_changes as u64,
                    s.enabled as u64,
                    s.binding_index as u64,
                ]);
                text(&s.tracked_text, out);
                text(&s.binding_name, out);
                out.push(s.name.len() as u64);
                out.extend(s.name.into_iter().map(u64::from));
                options(&s.options, out);
                match parent {
                    None => out.push(0),
                    Some(id) => {
                        out.push(1);
                        visit(
                            &SharedInvertNode {
                                core: node.sponsor.node(id),
                                sponsor: node.sponsor.clone(),
                            },
                            seen,
                            out,
                            options,
                        );
                    }
                }
            }
        }
        let mut out = Vec::new();
        visit(self, &mut Vec::new(), &mut out, options);
        out
    }
    /// CPU transform over an explicitly resolved input. Full graph evaluation is
    /// deliberately a separate capability. Exact resolved text controls image
    /// channels; numeric scalar selection continues to compare index zero.
    pub fn render(
        &self,
        input: Option<&mut crate::ImageMap>,
        width: i32,
        height: i32,
    ) -> Result<crate::ImageMap, SelectionRenderError<E>> {
        let ignored = self.resolved_channels()?;
        crate::invert::render_resolved_channels(input, width, height, ignored)
            .map_err(SelectionRenderError::Image)
    }
    /// Transform an actual retained image without copying its map or exposing
    /// a guard. Resolve all property texts before borrowing the input image.
    pub fn render_shared(
        &self,
        input: Option<&crate::SharedImageMap>,
        width: i32,
        height: i32,
    ) -> Result<crate::SharedImageMap, SelectionRenderError<E>> {
        let ignored = self.resolved_channels()?;
        crate::SharedImageMap::render_invert_resolved(input, width, height, ignored)
            .map_err(SelectionRenderError::Image)
    }
    fn resolved_channels(&self) -> Result<[bool; 4], SelectionRenderError<E>> {
        let mut ignored = [false; 4];
        for (ix, slot) in ignored.iter_mut().enumerate() {
            let value = self
                .property(ix as i32)
                .map_err(SelectionRenderError::Property)?
                .text()
                .map_err(SelectionRenderError::Property)?;
            *slot = value.as_deref() == Some(text("No").as_slice());
        }
        Ok(ignored)
    }
    pub fn scalar(&self, channel: i32, input: f32) -> f32 {
        match self.property(channel) {
            Ok(p) if p.current() == 0 => {
                if input.is_nan() {
                    f32::from_bits(input.to_bits() | 0x0040_0000)
                } else {
                    1.0 - input
                }
            }
            _ => input,
        }
    }
    pub fn same_reference(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.core, &other.core)
    }
    pub fn mode(&self) -> SelectionCloneMode {
        self.core.mode
    }
    pub fn property(&self, channel: i32) -> Result<SharedSelectionProperty<E>, SelectionError<E>> {
        let channel = usize::try_from(channel)
            .ok()
            .filter(|&c| c < 4)
            .ok_or(SelectionError::Index)?;
        Ok(SharedSelectionProperty {
            owner: self.clone(),
            channel,
        })
    }
    pub fn source_clone(&self) -> Self {
        let id = NodeId::new();
        let properties = {
            let state = self.core.state.lock().unwrap_or_else(|e| e.into_inner());
            std::array::from_fn(|channel| {
                let mut snapshot = state.properties[channel].snapshot.clone();
                snapshot.allow_changes = true;
                if !snapshot.tracking {
                    snapshot.tracked_index = 0;
                    snapshot.tracked_text = None;
                }
                PropertyState {
                    snapshot,
                    parent: Some(id),
                    changed: forwarder(),
                }
            })
        };
        let core = Arc::new(NodeCore {
            id,
            mode: self.core.mode,
            state: Mutex::new(NodeState {
                properties,
                changed: Events::Absent,
            }),
        });
        let sponsor = Sponsor::publish(&core);
        Self { core, sponsor }
    }
    pub fn add_changed(&self, handler: &SelectionHandler<E>) {
        let old = {
            let mut s = self.core.state.lock().unwrap_or_else(|e| e.into_inner());
            let next = s.changed.combine(&Events::Single(Arc::clone(&handler.0)));
            std::mem::replace(&mut s.changed, next)
        };
        drop(old);
    }
    pub fn remove_changed(&self, handler: &SelectionHandler<E>) {
        let old = {
            let mut s = self.core.state.lock().unwrap_or_else(|e| e.into_inner());
            let next = s.changed.remove(&Events::Single(Arc::clone(&handler.0)));
            std::mem::replace(&mut s.changed, next)
        };
        drop(old);
    }
}
impl<E> SharedSelectionProperty<E> {
    pub fn same_reference(&self, other: &Self) -> bool {
        self.channel == other.channel && self.owner.same_reference(&other.owner)
    }
    pub fn constructor_owner(&self) -> SharedInvertNode<E> {
        self.owner.clone()
    }
    pub fn parent(&self) -> Option<SharedInvertNode<E>> {
        let id = {
            self.owner
                .core
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .properties[self.channel]
                .parent
        };
        id.map(|id| SharedInvertNode {
            core: self.owner.sponsor.node(id),
            sponsor: Arc::clone(&self.owner.sponsor),
        })
    }
    pub fn set_parent(&self, parent: Option<&SharedInvertNode<E>>) {
        if let Some(parent) = parent {
            self.owner.sponsor.merge(&parent.sponsor);
        }
        self.owner
            .core
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .properties[self.channel]
            .parent = parent.map(|p| p.core.id);
    }
    pub fn snapshot(&self) -> SelectionSnapshot {
        self.owner
            .core
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .properties[self.channel]
            .snapshot
            .clone()
    }
    pub fn options(&self) -> SharedSelectionOptions {
        self.snapshot().options
    }
    pub fn changed_snapshot(&self) -> SelectionDelegate<E> {
        let core = {
            self.owner
                .core
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .properties[self.channel]
                .changed
                .clone()
        };
        SelectionDelegate {
            core,
            owner: self.owner.clone(),
        }
    }
    pub fn current(&self) -> i32 {
        self.snapshot().current
    }
    /// Resolves the characterized unattached, unanimated property at any time.
    /// No ancestor or animation producer is accepted by this ordinary owner type.
    pub fn resolve_unattached_index(&self) -> i32 {
        self.current()
    }
    pub fn resolve_unattached_text(&self) -> Result<SelectionText, SelectionError<E>> {
        self.text()
    }
    pub fn text(&self) -> Result<SelectionText, SelectionError<E>> {
        let s = self.snapshot();
        let values = s.options.entries();
        usize::try_from(s.current)
            .ok()
            .and_then(|i| values.get(i))
            .cloned()
            .ok_or(SelectionError::Index)
    }
    pub fn set_allow_changes(&self, allow: bool) {
        self.owner
            .core
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .properties[self.channel]
            .snapshot
            .allow_changes = allow;
    }
    pub fn set_enabled(&self, enabled: bool) {
        self.owner
            .core
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .properties[self.channel]
            .snapshot
            .enabled = enabled;
    }
    pub fn enable_tracking(&self) {
        let mut state = self
            .owner
            .core
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let s = &mut state.properties[self.channel].snapshot;
        s.tracking = true;
        s.tracked_index = s.current;
    }
    pub fn set_index(&self, value: i32) -> Result<(), SelectionError<E>> {
        let changed = {
            let mut state = self
                .owner
                .core
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            state.properties[self.channel].snapshot.commit_index(value)
        };
        if changed {
            self.notify_adjusted_index()?;
        }
        Ok(())
    }

    pub(crate) fn prepare_adjusted_index(
        &self,
        value: i32,
    ) -> (SelectionPosition, SelectionPosition) {
        let mut snapshot = self.snapshot();
        let before = snapshot.position();
        snapshot.commit_index(value);
        (before, snapshot.position())
    }

    pub(crate) fn write_adjusted_index(&self, value: &SelectionPosition) {
        let mut state = self
            .owner
            .core
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let snapshot = &mut state.properties[self.channel].snapshot;
        snapshot.current = value.current;
        snapshot.previous = value.previous;
        snapshot.tracked_index = value.tracked_index;
        snapshot.tracked_text.clone_from(&value.tracked_text);
    }

    pub(crate) fn notify_adjusted_index(&self) -> Result<(), SelectionError<E>> {
        if self.owner.core.mode == SelectionCloneMode::Original {
            self.emit(false, false)?;
        }
        Ok(())
    }
    pub fn set_index_speed(&self, value: i32) -> Result<(), SelectionError<E>> {
        let changed = {
            let mut state = self
                .owner
                .core
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let s = &mut state.properties[self.channel].snapshot;
            if value < 0 || s.current == value {
                false
            } else {
                s.current = value;
                true
            }
        };
        if changed {
            self.emit(true, false)?;
        }
        Ok(())
    }
    pub fn set_text(&self, value: SelectionText) -> Result<(), SelectionError<E>> {
        if self.text()? == value {
            return Ok(());
        }
        let options = self.options().entries();
        let at = options
            .iter()
            .position(|v| *v == value)
            .ok_or(SelectionError::UnrecognizedText)?;
        self.set_index(at as i32)
    }
    pub fn set_stamp(&self, value: Option<&SelectionStamp>) -> Result<(), SelectionError<E>> {
        let value = value.ok_or(SelectionError::NullReference)?;
        {
            let mut state = self
                .owner
                .core
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let s = &mut state.properties[self.channel].snapshot;
            s.current = value.current;
            s.tracked_index = value.tracked_index;
            s.tracked_text = value.tracked_text.clone();
        }
        if self.owner.core.mode == SelectionCloneMode::Original {
            self.emit(false, false)?;
        }
        Ok(())
    }
    /// Generic integer SetValue ignores the temporary flag in original os.
    pub fn set_value(&self, value: i32, _temporary: bool) -> Result<(), SelectionError<E>> {
        self.set_index(value)
    }
    pub fn set_object_value(&self, value: &SelectionValue) -> Result<(), SelectionError<E>> {
        match value {
            SelectionValue::Integer(i) => self.set_index(*i),
            SelectionValue::Stamp(stamp) => self.set_stamp(Some(stamp)),
            SelectionValue::Null => self.set_stamp(None),
            SelectionValue::Text(_) => Err(SelectionError::InvalidCast),
        }
    }
    pub fn set_object_value_temporary(
        &self,
        value: &SelectionValue,
        _temporary: bool,
    ) -> Result<(), SelectionError<E>> {
        match value {
            SelectionValue::Integer(i) => self.set_index(*i),
            SelectionValue::Null => Err(SelectionError::NullReference),
            _ => Err(SelectionError::InvalidCast),
        }
    }
    pub fn replace_options(
        &self,
        options: Option<SharedSelectionOptions>,
    ) -> Result<(), SelectionError<E>> {
        let options = options.ok_or(SelectionError::NullReference)?;
        let entries = options.entries();
        let old = {
            let mut state = self
                .owner
                .core
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let s = &mut state.properties[self.channel].snapshot;
            if entries == s.options.entries() {
                return Ok(());
            }
            std::mem::replace(&mut s.options, options)
        };
        drop(old);
        {
            let mut state = self
                .owner
                .core
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let s = &mut state.properties[self.channel].snapshot;
            let found = if s.tracked_index < (entries.len() as i32) {
                let ix = usize::try_from(s.tracked_index).map_err(|_| SelectionError::Index)?;
                if entries.get(ix).ok_or(SelectionError::Index)? == &s.tracked_text {
                    Some(ix)
                } else {
                    None
                }
            } else {
                None
            };
            let found = found.or_else(|| entries.iter().position(|v| *v == s.tracked_text));
            s.current = found.map_or(s.tracked_index, |i| i as i32);
        }
        self.emit(false, true)
    }
    pub fn set_binding_index(&self, value: i32) {
        self.owner
            .core
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .properties[self.channel]
            .snapshot
            .binding_index = value;
    }
    pub fn set_binding_name(&self, value: SelectionText) {
        let mut state = self
            .owner
            .core
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let name = &mut state.properties[self.channel].snapshot.binding_name;
        let equivalent = |v: &SelectionText| v.as_ref().is_none_or(Vec::is_empty);
        if *name != value && !(equivalent(name) && equivalent(&value)) {
            *name = value;
        }
    }
    pub fn clear_binding(&self) {
        let mut state = self
            .owner
            .core
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let s = &mut state.properties[self.channel].snapshot;
        s.binding_index = -1;
        s.binding_name = None;
    }
    pub fn add_changed(&self, handler: &SelectionHandler<E>) {
        let old = {
            let mut s = self
                .owner
                .core
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let p = &mut s.properties[self.channel];
            let next = p.changed.combine(&Events::Single(Arc::clone(&handler.0)));
            std::mem::replace(&mut p.changed, next)
        };
        drop(old);
    }
    pub fn remove_changed(&self, handler: &SelectionHandler<E>) {
        let old = {
            let mut s = self
                .owner
                .core
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let p = &mut s.properties[self.channel];
            let next = p.changed.remove(&Events::Single(Arc::clone(&handler.0)));
            std::mem::replace(&mut p.changed, next)
        };
        drop(old);
    }
    fn emit(&self, speed: bool, controls: bool) -> Result<(), SelectionError<E>> {
        let (handlers, allow) = {
            let state = self
                .owner
                .core
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let p = &state.properties[self.channel];
            (p.changed.clone(), p.snapshot.allow_changes)
        };
        let args = Arc::new(SelectionEventArgs {
            speed: speed && allow,
            controls_only: controls || !allow,
        });
        handlers
            .invoke(&SelectionSender::Property(self.clone()), args)
            .map_err(SelectionError::Observer)
    }
}

#[cfg(test)]
mod shared_render_tests {
    use super::*;
    use crate::{FloatBuffer, ImageError, ImageGeometry, SharedFloatStore, SharedImageMap};

    // Composition checks over the existing source-backed Invert and ImageMap
    // laws; these do not add original runtime golden observations.
    #[test]
    fn shared_invert_uses_actual_option_text_and_retains_ignored_planes_and_auxiliary_roots() {
        let node = SharedInvertNode::<()>::new(SelectionCloneMode::Original);
        let options = SharedSelectionOptions::new(vec![Some(text("No")), Some(text("Yes"))]);
        let red = node.property(0).unwrap();
        red.replace_options(Some(options)).unwrap();
        red.set_index(0).unwrap(); // Text No means pass through despite index zero.
        let input = SharedImageMap::new(ImageGeometry::new(2, 2));
        let plane = FloatBuffer::new(vec![0.125, 0.25, 0.5, 0.75]);
        input.set_buffer(0, Some(&plane)).unwrap();
        input.set_constant(1, 0.25).unwrap();
        input.set_constant(2, 0.75).unwrap();
        input.set_constant(3, 0.5).unwrap();
        let region = SharedFloatStore::new(FloatBuffer::new(vec![1., 2., 3., 4.])).unwrap();
        input.set_region_store(Some(region.clone()));
        input
            .add_named_store(Some(text("height")), Some(region.clone()))
            .unwrap();
        let original_named = input.named_channels();
        let output = node.render_shared(Some(&input), 2, 2).unwrap();
        assert_eq!(output.channel(0, true).unwrap().values(), plane.values());
        assert_eq!(output.channel(1, true).unwrap().read(0).unwrap(), 0.75);
        assert_eq!(output.channel(2, true).unwrap().read(0).unwrap(), 0.25);
        assert_eq!(output.channel(3, true).unwrap().read(0).unwrap(), 0.5);
        assert_eq!(input.channel(1, true).unwrap().read(0).unwrap(), 0.25);
        plane.write(2, 0.875).unwrap();
        assert_eq!(output.channel(0, true).unwrap().read(2).unwrap(), 0.875);
        assert!(output.region_store().unwrap().same_store(&region));
        assert!(
            output.named_channels()[0]
                .as_ref()
                .unwrap()
                .same_wrapper(original_named[0].as_ref().unwrap())
        );
    }
    #[test]
    fn shared_invert_property_failure_precedes_null_or_disposed_map_access() {
        let node = SharedInvertNode::<()>::new(SelectionCloneMode::Original);
        let input = SharedImageMap::new(ImageGeometry::new(1, 1));
        input.dispose();
        node.property(3).unwrap().set_index(99).unwrap();
        for map in [None, Some(&input)] {
            assert!(matches!(
                node.render_shared(map, 1, 1),
                Err(SelectionRenderError::Property(SelectionError::Index))
            ));
        }
        node.property(3).unwrap().set_index(1).unwrap();
        assert!(matches!(
            node.render_shared(None, 1, 1),
            Err(SelectionRenderError::Image(ImageError::NullReference))
        ));
        assert!(matches!(
            node.render_shared(Some(&input), 1, 1),
            Err(SelectionRenderError::Image(ImageError::Disposed))
        ));
    }
}

/// Click-time resolved Invert operands. No selection/options/owner is retained.
#[derive(Clone, Debug)]
pub struct InvertRenderSnapshot {
    ignored: [bool; 4],
}
impl InvertRenderSnapshot {
    /// Compare the resolved operands consumed by rendering. Captures hold no
    /// mutable options: distinct source roots with the same resolved choices
    /// have the same render values, and later option edits require recapture.
    pub fn same_render_values(&self, other: &Self) -> bool {
        self.ignored == other.ignored
    }
    pub fn render_shared(
        &self,
        input: Option<&crate::SharedImageMap>,
        width: i32,
        height: i32,
    ) -> Result<crate::SharedImageMap, crate::ImageError> {
        crate::SharedImageMap::render_invert_resolved(input, width, height, self.ignored)
    }
}
impl<E> SharedInvertNode<E> {
    /// Reads actual option text in source order; no callbacks or mutations.
    pub fn capture_render(&self) -> Result<InvertRenderSnapshot, SelectionRenderError<E>> {
        Ok(InvertRenderSnapshot {
            ignored: self.resolved_channels()?,
        })
    }
}
