//! The source asy and rw identities joined to the existing pointwise region.
//! qi occurrences remain independent of asy. No node/property is copied here.
use super::graph::{NodeCore, Payload, SlotCore};
use super::region::{NodeId, Sponsor};
use super::{PointwiseError, PointwiseNode, PointwiseSlot, guard};
use crate::history::HistoryCore;
use crate::{
    ContextChangeArgument, ContextUndoInfo, History, HistoryDirection, HistoryEffect, HistoryError,
    HistoryPropertyTarget, HistoryTarget,
};
use std::{
    collections::BTreeSet,
    fmt,
    sync::{Arc, Mutex, Weak},
};
mod edit;
pub(crate) use edit::GraphConnectionEdit;

pub(super) struct NodeBinding<E> {
    // A detached node keeps the exact last rw installed in TextureNodeLogic.
    logic: Option<Arc<RwCore<E>>>,
    // Region-owned naked roots. None of these holds an outward sponsor.
    slots: Vec<Arc<AsyCore<E>>>,
    histories: Vec<Arc<HistoryCore<E>>>,
}
impl<E> NodeBinding<E> {
    pub(super) fn new() -> Self {
        Self {
            logic: None,
            slots: Vec::new(),
            histories: Vec::new(),
        }
    }
}
struct AsyCore<E> {
    anchor: Weak<NodeCore<E>>,
    name: Vec<u16>,
    current: Mutex<Option<Arc<RwCore<E>>>>,
    history: Weak<HistoryCore<E>>,
    parent: SlotParent<E>,
}
enum SlotParent<E> {
    Document,
    Input(Weak<AgdCore<E>>),
    Contents {
        owner: Weak<RwCore<E>>,
        occurrence: Arc<SlotCore>,
    },
}
struct AgdCore<E> {
    parent: Weak<RwCore<E>>,
    slots: Mutex<Vec<Arc<AsyCore<E>>>>,
}
impl<E> AsyCore<E> {
    fn core_parent_wrapper(&self) -> Option<Arc<RwCore<E>>> {
        match &self.parent {
            SlotParent::Document => None,
            SlotParent::Input(group) => group.upgrade()?.parent.upgrade(),
            SlotParent::Contents { owner, .. } => owner.upgrade(),
        }
    }

    fn is_document_bound(&self) -> bool {
        if matches!(self.parent, SlotParent::Document) {
            return true;
        }
        let mut wrapper = self.core_parent_wrapper();
        while let Some(parent) = wrapper {
            let Some(slot) = guard(&parent.parent).upgrade() else {
                return false;
            };
            if matches!(slot.parent, SlotParent::Document) {
                return true;
            }
            wrapper = slot.core_parent_wrapper();
        }
        false
    }

    fn source_preview_changed(&self) {
        // asy.y constructs acs(null) and dispatches the base virtual b(acs),
        // whose installed IL is a single ret; no PreviewChanged delegate fires.
        let _argument = SourcePreviewChanged;
    }
}
struct SourcePreviewChanged;
struct RwCore<E> {
    node: NodeId,
    parent: Mutex<Weak<AsyCore<E>>>,
    input_group: Mutex<Option<Arc<AgdCore<E>>>>,
    contents: Mutex<Vec<Arc<AsyCore<E>>>>,
}
impl<E> RwCore<E> {
    fn content_slot(
        self: &Arc<Self>,
        anchor: Weak<NodeCore<E>>,
        history: Weak<HistoryCore<E>>,
        occurrence: &PointwiseSlot<E>,
    ) -> Arc<AsyCore<E>> {
        Arc::new(AsyCore {
            anchor,
            name: occurrence.name(),
            current: Mutex::new(None),
            history,
            parent: SlotParent::Contents {
                owner: Arc::downgrade(self),
                occurrence: occurrence.core.clone(),
            },
        })
    }
    fn tree(root: Option<&Arc<Self>>) -> Vec<Arc<Self>> {
        let mut nodes = Vec::new();
        let mut pending: Vec<_> = root.into_iter().cloned().collect();
        while let Some(wrapper) = pending.pop() {
            if let Some(group) = guard(&wrapper.input_group).as_ref() {
                pending.extend(
                    guard(&group.slots)
                        .iter()
                        .filter_map(|slot| guard(&slot.current).clone()),
                );
            }
            pending.extend(
                guard(&wrapper.contents)
                    .iter()
                    .filter_map(|slot| guard(&slot.current).clone()),
            );
            nodes.push(wrapper);
        }
        nodes
    }
}
/// One actual asy property-history target. Its rw may change independently of
/// any qi input occurrence. This handle sponsors the complete graph region.
pub struct PointwiseHistorySlot<E = ()> {
    core: Arc<AsyCore<E>>,
    sponsor: Arc<Sponsor<E>>,
}
impl<E> Clone for PointwiseHistorySlot<E> {
    fn clone(&self) -> Self {
        Self {
            core: self.core.clone(),
            sponsor: self.sponsor.clone(),
        }
    }
}
impl<E> fmt::Debug for PointwiseHistorySlot<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("PointwiseHistorySlot")
            .field(&Arc::as_ptr(&self.core))
            .finish()
    }
}
/// The actual rw installed in Node.TextureNodeLogic. Its node, inputs and
/// contents resolve through the same existing graph authority after detach.
pub struct PointwiseTextureNodeLogic<E = ()> {
    core: Arc<RwCore<E>>,
    sponsor: Arc<Sponsor<E>>,
}
impl<E> Clone for PointwiseTextureNodeLogic<E> {
    fn clone(&self) -> Self {
        Self {
            core: self.core.clone(),
            sponsor: self.sponsor.clone(),
        }
    }
}
impl<E> fmt::Debug for PointwiseTextureNodeLogic<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("PointwiseTextureNodeLogic")
            .field(&Arc::as_ptr(&self.core))
            .finish()
    }
}
impl<E: 'static> PointwiseHistorySlot<E> {
    pub(crate) fn bind(anchor: &PointwiseNode<E>, history: &History<E>, name: Vec<u16>) -> Self {
        let node = anchor.core();
        let core = Arc::new(AsyCore {
            anchor: Arc::downgrade(&node),
            name,
            current: Mutex::new(None),
            history: Arc::downgrade(&history.core),
            parent: SlotParent::Document,
        });
        {
            let mut bindings = guard(&node.history_binding);
            bindings.slots.push(core.clone());
            if !bindings
                .histories
                .iter()
                .any(|h| Arc::ptr_eq(h, &history.core))
            {
                bindings.histories.push(history.core.clone());
            }
        }
        Self {
            core,
            sponsor: anchor.sponsor.clone(),
        }
    }
    pub fn same_reference(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.core, &other.core)
    }
    pub fn name(&self) -> &[u16] {
        &self.core.name
    }
    pub fn texture_node(&self) -> Option<PointwiseTextureNodeLogic<E>> {
        guard(&self.core.current)
            .as_ref()
            .map(|core| PointwiseTextureNodeLogic {
                core: core.clone(),
                sponsor: self.sponsor.clone(),
            })
    }
    pub fn history_target(&self) -> HistoryTarget<E> {
        // This coercion clones the SAME asy Arc. Outward adapters are not roots.
        HistoryTarget::sponsored(self.core.clone(), self.sponsor.clone())
    }
    pub fn history(&self) -> Option<History<E>> {
        Some(History {
            core: self.core.history.upgrade()?,
            owner: Some(self.sponsor.clone()),
        })
    }
    pub(crate) fn release_document_binding(&self) -> Option<PointwiseNode<E>> {
        let logic = self.texture_node();
        for wrapper in RwCore::tree(logic.as_ref().map(|logic| &logic.core)) {
            let node = self.sponsor.node(wrapper.node);
            let mut binding = guard(&node.history_binding);
            if binding
                .logic
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, &wrapper))
            {
                binding.logic = None;
            }
        }
        self.replace_wrapper(None);
        if let Some(anchor) = self.core.anchor.upgrade() {
            let mut binding = guard(&anchor.history_binding);
            binding.slots.retain(|slot| !Arc::ptr_eq(slot, &self.core));
            if let Some(history) = self.core.history.upgrade()
                && !binding
                    .slots
                    .iter()
                    .any(|slot| slot.history.ptr_eq(&self.core.history))
            {
                binding
                    .histories
                    .retain(|current| !Arc::ptr_eq(current, &history));
            }
        }
        logic.map(|logic| logic.node())
    }
    /// Validate the node forest before publishing any binding or merging regions.
    /// One node has one TextureNodeLogic. Shared occurrences cannot each own it.
    pub(crate) fn validate_trees<'a>(
        roots: impl IntoIterator<Item = &'a PointwiseNode<E>>,
    ) -> Result<(), HistoryError<E>> {
        Self::validate_forest(roots.into_iter().map(|root| (None, root.clone())), None)
    }

    fn validate_forest(
        roots: impl IntoIterator<Item = (Option<NodeId>, PointwiseNode<E>)>,
        destination: Option<NodeId>,
    ) -> Result<(), HistoryError<E>> {
        let invalid = || HistoryError::MissingDependency(HistoryEffect::PropertyTarget);
        let mut visited = BTreeSet::new();
        let mut pending: Vec<_> = roots.into_iter().collect();
        while let Some((parent, node)) = pending.pop() {
            let contents = node.contents();
            let group = node
                .kind()
                .is_some_and(super::PointwiseKind::has_compact_inputs);
            if destination == Some(node.id)
                || node.parent().map(|p| p.id) != parent
                || !visited.insert(node.id)
                || (!group && !contents.is_empty())
                || node
                    .texture_node_logic()
                    .and_then(|logic| logic.slot())
                    .is_some_and(|slot| slot.core.is_document_bound())
                || node.is_clone_mode()
                || node.is_default_input()
            {
                return Err(invalid());
            }
            // Invert currently has no typed property-history producer. It can
            // still own a nested ordinary input's editor and render connection.
            if !node.is_scene_node()
                && !matches!(
                    &node.core().payload,
                    Payload::ImageGenerator { .. } | Payload::Invert { .. } | Payload::Solid { .. }
                )
            {
                let filter = node.filter().ok_or_else(invalid)?;
                if group
                    && (contents.is_empty()
                        || filter.group_content_positions().len() + 1 != contents.len())
                {
                    return Err(invalid());
                }
                if filter.parameters().iter().any(|p| {
                    let value = p.value();
                    let passive_custom_value =
                        matches!(value, crate::PointwiseParameterValue::Region { .. })
                            || matches!(value, crate::PointwiseParameterValue::Histogram(_))
                            || matches!(value, crate::PointwiseParameterValue::Matrix { .. })
                                && p.custom_object().is_ok_and(|object| {
                                    object.is_some_and(|object| {
                                        object.is_light_position()
                                            || object.legacy_color_gradient_summary().is_some()
                                    })
                                });
                    // These custom objects emit untyped notifications. Their
                    // presence does not prevent typed sibling controls from
                    // owning property history.
                    !(matches!(
                        value,
                        crate::PointwiseParameterValue::Number { .. }
                            | crate::PointwiseParameterValue::Choice { .. }
                            | crate::PointwiseParameterValue::Pair { .. }
                            | crate::PointwiseParameterValue::Color(_)
                            | crate::PointwiseParameterValue::Gradient(_)
                            | crate::PointwiseParameterValue::Curve(_)
                            | crate::PointwiseParameterValue::Environment(_)
                            | crate::PointwiseParameterValue::Text(_)
                    ) || passive_custom_value
                        || group && matches!(value, crate::PointwiseParameterValue::Variable(_)))
                }) {
                    return Err(invalid());
                }
            }
            pending.extend(
                contents
                    .into_iter()
                    .filter_map(|slot| slot.node().map(|child| (Some(node.id), child))),
            );
            pending.extend(node.slots().into_iter().rev().filter_map(|slot| {
                let child = slot.node()?;
                // agd constructs the named editor slot but skips z7 placeholders.
                (!child.is_default_input()).then_some((Some(node.id), child))
            }));
        }
        Ok(())
    }

    /// Build rw/agd/asy around the real, already validated qi occurrences.
    /// agd's construction path leaves the existing qi.Node links untouched.
    pub(crate) fn set_node(&self, node: Option<&PointwiseNode<E>>) {
        self.replace_wrapper(None);
        let Some(node) = node else {
            return;
        };
        self.sponsor.merge(&node.sponsor);
        let mut pending = vec![(self.core.clone(), node.clone())];
        while let Some((slot, node)) = pending.pop() {
            let core = node.core();
            let wrapper = Arc::new(RwCore {
                node: node.id,
                parent: Mutex::new(Arc::downgrade(&slot)),
                input_group: Mutex::new(None),
                contents: Mutex::new(Vec::new()),
            });
            let old = guard(&core.history_binding).logic.replace(wrapper.clone());
            drop(old);

            let inputs = node.slots();
            let group = Arc::new_cyclic(|group| AgdCore {
                parent: Arc::downgrade(&wrapper),
                slots: Mutex::new(
                    inputs
                        .iter()
                        .map(|input| {
                            Arc::new(AsyCore {
                                anchor: Arc::downgrade(&core),
                                name: input.name(),
                                current: Mutex::new(None),
                                history: slot.history.clone(),
                                parent: SlotParent::Input(group.clone()),
                            })
                        })
                        .collect(),
                ),
            });
            pending.extend(guard(&group.slots).iter().zip(inputs).rev().filter_map(
                |(slot, input)| {
                    let child = input.node()?;
                    (!child.is_default_input()).then(|| (slot.clone(), child))
                },
            ));
            *guard(&wrapper.input_group) = Some(group);
            let contents = node.contents();
            let content_slots: Vec<_> = contents
                .iter()
                .map(|occurrence| {
                    wrapper.content_slot(Arc::downgrade(&core), slot.history.clone(), occurrence)
                })
                .collect();
            pending.extend(content_slots.iter().zip(contents).rev().filter_map(
                |(slot, occurrence)| occurrence.node().map(|child| (slot.clone(), child)),
            ));
            *guard(&wrapper.contents) = content_slots;
            *guard(&slot.current) = Some(wrapper);
        }
    }

    fn replace_wrapper(&self, wrapper: Option<Arc<RwCore<E>>>) {
        if let Some(wrapper) = &wrapper {
            *guard(&wrapper.parent) = Arc::downgrade(&self.core);
        }
        let old = std::mem::replace(&mut *guard(&self.core.current), wrapper);
        if let Some(old) = old {
            *guard(&old.parent) = Weak::new();
        }
    }

    fn restore_wrapper(&self, wrapper: Option<Arc<RwCore<E>>>) {
        // An intervening insertion can have installed a new wrapper on the
        // same node. Reinstall this exact tree, including its input targets.
        for current in RwCore::tree(wrapper.as_ref()) {
            let node = self.sponsor.node(current.node);
            guard(&node.history_binding).logic = Some(current);
        }
        self.replace_wrapper(wrapper);
    }

    fn can_restore_wrappers(&self, wrapper: Option<&Arc<RwCore<E>>>) -> bool {
        RwCore::tree(wrapper).iter().all(|retained| {
            let node = self.sponsor.node(retained.node);
            let current = guard(&node.history_binding).logic.clone();
            current.is_none_or(|current| {
                let slot = guard(&current.parent).upgrade();
                Arc::ptr_eq(&current, retained) || slot.is_none_or(|slot| !slot.is_document_bound())
            })
        })
    }

    fn is_document_root(&self) -> bool {
        matches!(self.core.parent, SlotParent::Document)
    }

    /// Resolve the actual source occurrence for a move in this document.
    fn move_source(&self, node: &PointwiseNode<E>) -> Result<Option<Self>, PointwiseError<E>> {
        let Some(source) = node.texture_node_logic().and_then(|logic| logic.slot()) else {
            return Ok(None);
        };
        if !source.core.history.ptr_eq(&self.core.history) {
            return Ok(None);
        }
        let owns_node = if source.is_document_root() {
            node.parent().is_none()
                && source
                    .texture_node()
                    .is_some_and(|logic| logic.node().same_reference(node))
        } else {
            source.graph_occurrence()?.is_some_and(|input| {
                input
                    .node()
                    .is_some_and(|current| current.same_reference(node))
                    && node
                        .parent()
                        .is_some_and(|current| current.same_reference(&input.parent()))
            })
        };
        Ok(owns_node.then_some(source))
    }

    pub(super) fn prepare_input(
        &self,
        parent: &PointwiseNode<E>,
        node: Option<&PointwiseNode<E>>,
    ) -> Result<Option<Self>, PointwiseError<E>> {
        let Some(node) = node.filter(|n| !n.is_default_input()) else {
            return Ok(None);
        };
        let current_slot = node.texture_node_logic().and_then(|logic| logic.slot());
        if current_slot
            .as_ref()
            .is_some_and(|source| self.same_reference(source))
        {
            // Filter mode can replace the qi while its editor slot and rw stay.
            return Ok(current_slot);
        }
        if let Some(source) = self.move_source(node)? {
            let mut ancestor = self.core.core_parent_wrapper();
            while let Some(wrapper) = ancestor {
                if wrapper.node == node.id {
                    return Err(PointwiseError::UnsupportedCapability(
                        "cyclic property history input",
                    ));
                }
                let slot = guard(&wrapper.parent).upgrade();
                ancestor = slot.and_then(|slot| slot.core_parent_wrapper());
            }
            return Ok(Some(source));
        }
        Self::validate_forest(
            [(node.parent().map(|p| p.id), node.clone())],
            Some(parent.id),
        )
        .map_err(|error| PointwiseError::History(Box::new(error)))?;
        Ok(None)
    }

    pub(super) fn graph_occurrence(&self) -> Result<Option<PointwiseSlot<E>>, PointwiseError<E>> {
        let Some(parent) = self.core.core_parent_wrapper() else {
            return Ok(None);
        };
        let node = PointwiseNode {
            id: parent.node,
            sponsor: self.sponsor.clone(),
        };
        match &self.core.parent {
            SlotParent::Document => Ok(None),
            SlotParent::Input(group) => {
                let group = group.upgrade().ok_or(PointwiseError::NullReference)?;
                let index = guard(&group.slots)
                    .iter()
                    .position(|slot| Arc::ptr_eq(slot, &self.core))
                    .ok_or(PointwiseError::NullReference)?;
                node.slot(index as i32).map(Some)
            }
            SlotParent::Contents { occurrence, .. } => Ok(node
                .contents()
                .into_iter()
                .find(|slot| Arc::ptr_eq(&slot.core, occurrence))),
        }
    }

    pub(super) fn publish_input(&self, source: Option<&Self>, node: Option<&PointwiseNode<E>>) {
        if let Some(source) = source {
            if !self.same_reference(source) {
                let wrapper = guard(&source.core.current).take();
                self.replace_wrapper(wrapper);
            }
        } else {
            self.set_node(node.filter(|node| !node.is_default_input()));
        }
    }
}
/// Retained source agd for one rw's actual input-slot asy owners.
pub struct PointwiseInputGroup<E = ()> {
    core: Arc<AgdCore<E>>,
    owner: Arc<RwCore<E>>,
    sponsor: Arc<Sponsor<E>>,
}
impl<E> Clone for PointwiseInputGroup<E> {
    fn clone(&self) -> Self {
        Self {
            core: self.core.clone(),
            owner: self.owner.clone(),
            sponsor: self.sponsor.clone(),
        }
    }
}
impl<E: 'static> PointwiseInputGroup<E> {
    pub fn same_reference(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.core, &other.core)
    }
    pub fn parent(&self) -> PointwiseTextureNodeLogic<E> {
        PointwiseTextureNodeLogic {
            core: self
                .core
                .parent
                .upgrade()
                .expect("held input group retains its source rw"),
            sponsor: self.sponsor.clone(),
        }
    }
    pub fn slots(&self) -> Vec<PointwiseInputSlot<E>> {
        guard(&self.core.slots)
            .iter()
            .map(|slot| PointwiseInputSlot {
                core: slot.clone(),
                owner: self.owner.clone(),
                sponsor: self.sponsor.clone(),
            })
            .collect()
    }
    /// Source agd.yc resolves the actual current qi by matching the asy root.
    pub fn input_occurrence(
        &self,
        slot: &PointwiseInputSlot<E>,
    ) -> Result<Option<PointwiseSlot<E>>, PointwiseError<E>> {
        let Some(index) = guard(&self.core.slots)
            .iter()
            .position(|item| Arc::ptr_eq(item, &slot.core))
        else {
            return Ok(None);
        };
        Ok(Some(self.parent().node().slot(index as i32)?))
    }
}
/// An actual input asy. It retains its rw/agd ancestor just as source
/// asy.ao() does.
pub struct PointwiseInputSlot<E = ()> {
    core: Arc<AsyCore<E>>,
    owner: Arc<RwCore<E>>,
    sponsor: Arc<Sponsor<E>>,
}
impl<E> Clone for PointwiseInputSlot<E> {
    fn clone(&self) -> Self {
        Self {
            core: self.core.clone(),
            owner: self.owner.clone(),
            sponsor: self.sponsor.clone(),
        }
    }
}
impl<E: 'static> PointwiseInputSlot<E> {
    pub fn same_reference(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.core, &other.core)
    }
    pub fn name(&self) -> &[u16] {
        &self.core.name
    }
    pub fn texture_node(&self) -> Option<PointwiseTextureNodeLogic<E>> {
        guard(&self.core.current)
            .as_ref()
            .map(|core| PointwiseTextureNodeLogic {
                core: core.clone(),
                sponsor: self.sponsor.clone(),
            })
    }
    pub fn input_parent(&self) -> PointwiseInputGroup<E> {
        let SlotParent::Input(group) = &self.core.parent else {
            unreachable!("only input slots expose an input group");
        };
        let core = group
            .upgrade()
            .expect("held input slot retains its source group");
        PointwiseInputGroup {
            core,
            owner: self.owner.clone(),
            sponsor: self.sponsor.clone(),
        }
    }
}
impl<E: 'static> PointwiseTextureNodeLogic<E> {
    // The installed base rw.s0 returns literal zero. Render requests carry
    // their own explicit, unrelated ai5 sample time.
    pub(super) fn binding_sample_time(&self) -> f64 {
        0.
    }

    pub fn same_reference(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.core, &other.core)
    }
    pub fn node(&self) -> PointwiseNode<E> {
        PointwiseNode {
            id: self.core.node,
            sponsor: self.sponsor.clone(),
        }
    }
    pub fn slot(&self) -> Option<PointwiseHistorySlot<E>> {
        Some(PointwiseHistorySlot {
            core: guard(&self.core.parent).upgrade()?,
            sponsor: self.sponsor.clone(),
        })
    }
    /// rw.g: actual group is absent while the constructor is still building it.
    pub fn input_group(&self) -> Option<PointwiseInputGroup<E>> {
        Some(PointwiseInputGroup {
            core: guard(&self.core.input_group).clone()?,
            owner: self.core.clone(),
            sponsor: self.sponsor.clone(),
        })
    }
    pub fn inputs(&self) -> Vec<PointwiseSlot<E>> {
        self.node().slots()
    }
    pub fn contents(&self) -> Vec<PointwiseSlot<E>> {
        self.node().contents()
    }
}
impl<E: 'static> PointwiseNode<E> {
    pub(super) fn binding_sample_time(&self) -> f64 {
        self.texture_node_logic()
            .map_or(0., |logic| logic.binding_sample_time())
    }

    pub(in crate::pointwise) fn has_property_history_target(&self) -> bool {
        guard(&self.core().history_binding).logic.is_some()
    }
    pub(in crate::pointwise) fn has_live_property_history_binding(&self) -> bool {
        self.texture_node_logic()
            .and_then(|logic| logic.slot())
            .is_some()
    }
    pub(super) fn child_history_slot(
        &self,
        occurrence: &PointwiseSlot<E>,
    ) -> Result<Option<PointwiseHistorySlot<E>>, PointwiseError<E>> {
        let Some(logic) = self.texture_node_logic() else {
            return Ok(None);
        };
        if let Some(slot) = guard(&logic.core.contents).iter().find(|slot| {
            matches!(&slot.parent, SlotParent::Contents { occurrence: raw, .. } if Arc::ptr_eq(raw, &occurrence.core))
        }) {
            return Ok(Some(PointwiseHistorySlot { core: slot.clone(), sponsor: self.sponsor.clone() }));
        }
        let index = self
            .slots()
            .iter()
            .position(|input| input.same_reference(occurrence))
            .ok_or(PointwiseError::UnsupportedCapability(
                "retired history input occurrence",
            ))?;
        let group = logic.input_group().ok_or(PointwiseError::NullReference)?;
        let slot = guard(&group.core.slots)
            .get(index)
            .cloned()
            .ok_or(PointwiseError::Index)?;
        Ok(Some(PointwiseHistorySlot {
            core: slot.clone(),
            sponsor: self.sponsor.clone(),
        }))
    }

    // zl.sh calls sx only AFTER all ordinary node callbacks/parent propagation.
    // Both TextureNodeLogic and rw.Parent are reread now, after callback reentry.
    pub(in crate::pointwise) fn record_property_history(
        &self,
        args: &Arc<ContextChangeArgument<E>>,
    ) -> Result<(), PointwiseError<E>> {
        let args = match args.as_ref() {
            ContextChangeArgument::Property(args) => args.clone(),
            ContextChangeArgument::Modern(args) if self.is_scene_node() => {
                crate::ContextPropertyChangeArgs::from_modern(args)
            }
            ContextChangeArgument::Animation(args) if self.is_scene_node() => {
                crate::ContextPropertyChangeArgs::from_animation(args)
            }
            _ => return Ok(()),
        };
        let Some(logic) = self.texture_node_logic() else {
            return Ok(());
        };
        let Some(slot) = logic.slot() else {
            return Ok(());
        };
        let history = slot.history().ok_or_else(|| {
            PointwiseError::History(Box::new(HistoryError::MissingDependency(
                HistoryEffect::PropertyTarget,
            )))
        })?;
        let target = slot.history_target();
        let Some(history) = history
            .route_property_history(&target, args.typed_undo_info())
            .map_err(|e| PointwiseError::History(Box::new(e)))?
        else {
            return Ok(());
        };
        // asy.b performs the actual active-document/loading lookup BEFORE its
        // null-undo gate. No command/suppression effects follow a null undo.
        if args.typed_undo_info().is_none() {
            return Ok(());
        }
        if history.is_suppressed() {
            return Ok(());
        }
        let command = history
            .property_command(target, Arc::new(args))
            .map_err(|e| PointwiseError::History(Box::new(e)))?;
        history
            .record(&command)
            .map_err(|e| PointwiseError::History(Box::new(e)))
    }
}
impl<E: 'static> HistoryPropertyTarget<E> for AsyCore<E> {
    fn apply(
        &self,
        undo: &ContextUndoInfo<E>,
        direction: HistoryDirection,
    ) -> Result<(), HistoryError<E>> {
        let anchor = self
            .anchor
            .upgrade()
            .ok_or(HistoryError::MissingDependency(
                HistoryEffect::PropertyTarget,
            ))?;
        let sponsor = Sponsor::for_node(&anchor).ok_or(HistoryError::MissingDependency(
            HistoryEffect::PropertyTarget,
        ))?;
        // mt.b rereads asy.TextureNode.d().Properties on EVERY execution.
        let wrapper = guard(&self.current)
            .clone()
            .ok_or(HistoryError::NullTarget)?;
        let node = PointwiseNode {
            id: wrapper.node,
            sponsor,
        };
        let (name, lab) = match undo {
            ContextUndoInfo::Double(v) => (v.name(), v.is_lab_undo()),
            ContextUndoInfo::Modern(v) => (v.name(), v.is_lab_undo()),
            ContextUndoInfo::Pair(v) => (v.name(), v.is_lab_undo()),
            ContextUndoInfo::Choice(v) => (v.name(), v.is_lab_undo()),
            ContextUndoInfo::Color(v) => (v.name(), v.is_lab_undo()),
            ContextUndoInfo::Animation(v) => (v.name(), v.is_lab_undo()),
            ContextUndoInfo::Environment(v) => (v.name(), v.is_lab_undo()),
        };
        if !lab && node.is_scene_node() {
            let properties = node
                .scene_primitive()
                .map(|primitive| primitive.properties())
                .or_else(|| {
                    node.scene_transform_3d()
                        .map(|transform| transform.properties())
                })
                .or_else(|| node.uv_texture_properties())
                .or_else(|| node.import_3d_properties())
                .unwrap_or_default();
            let Some(name) = name else { return Ok(()) };
            let Some(property) = properties
                .into_iter()
                .find(|property| property.name().as_deref() == Some(name))
            else {
                return Ok(());
            };
            let result = match (undo, direction) {
                (ContextUndoInfo::Double(undo), HistoryDirection::Undo) => {
                    property.set_double(undo.old_value())
                }
                (ContextUndoInfo::Double(undo), HistoryDirection::Redo) => {
                    property.set_double(undo.new_value())
                }
                (ContextUndoInfo::Modern(undo), direction) => {
                    use crate::ModernUndoValue;
                    let value = match direction {
                        HistoryDirection::Undo => undo.old_value(),
                        HistoryDirection::Redo => undo.new_value(),
                    };
                    match value {
                        ModernUndoValue::Boolean(value) => property.set_boolean(value),
                        ModernUndoValue::Int32(value) => property.set_int32(value),
                        ModernUndoValue::Vector3D(value) => property.set_vector3d(value),
                    }
                }
                (ContextUndoInfo::Animation(undo), direction) => {
                    property.replace_animation(match direction {
                        HistoryDirection::Undo => undo.old_value(),
                        HistoryDirection::Redo => undo.new_value(),
                    })
                }
                _ => Err(PointwiseError::InvalidCast),
            };
            return result.map_err(|error| HistoryError::Property(Box::new(error)));
        }
        if !lab && let Some(generator) = node.image_generator_handle() {
            return apply_generator_undo(&generator, name, undo, direction);
        }
        if !lab && matches!(&node.core().payload, Payload::Solid { .. }) {
            use crate::SolidColorProperty;
            let Some(name) = name else { return Ok(()) };
            let property = if name.iter().copied().eq("Color".encode_utf16()) {
                SolidColorProperty::Color
            } else if name.iter().copied().eq("Transparency".encode_utf16()) {
                SolidColorProperty::Transparency
            } else {
                return Ok(());
            };
            let result = match (property, undo) {
                (_, ContextUndoInfo::Animation(undo)) => node.set_solid_animation(
                    property,
                    match direction {
                        HistoryDirection::Undo => undo.old_value(),
                        HistoryDirection::Redo => undo.new_value(),
                    },
                ),
                (SolidColorProperty::Color, ContextUndoInfo::Color(undo))
                    if undo.property_change() == 6 =>
                {
                    let value = match direction {
                        HistoryDirection::Undo => undo.old_color_value(),
                        HistoryDirection::Redo => undo.new_color_value(),
                    };
                    node.set_solid_color_value(&value)
                        .map_err(super::PointwiseSolidEditError::into_property_error)
                }
                (SolidColorProperty::Transparency, ContextUndoInfo::Double(undo))
                    if undo.property_change() == 0 =>
                {
                    let value = match direction {
                        HistoryDirection::Undo => undo.old_value(),
                        HistoryDirection::Redo => undo.new_value(),
                    };
                    node.set_solid_transparency(value)
                        .map_err(super::PointwiseSolidEditError::into_property_error)
                }
                _ => Err(PointwiseError::InvalidCast),
            };
            return result.map_err(|error| HistoryError::Property(Box::new(error)));
        }
        let properties = if lab {
            let Some(filter) = node.filter() else {
                return Ok(());
            };
            match filter.kind() {
                super::PointwiseKind::WizardGroup | super::PointwiseKind::WizardReference => filter
                    .wizard_dynamic_properties()
                    .map_err(|error| HistoryError::Property(Box::new(error)))?,
                super::PointwiseKind::TemplateLab(_) => {
                    filter.parameters().into_iter().skip(3).collect()
                }
                _ => return Ok(()),
            }
        } else {
            node.filter()
                .ok_or(HistoryError::MissingDependency(
                    HistoryEffect::PropertyTarget,
                ))?
                .parameters()
        };
        let Some(property) = properties
            .into_iter()
            .find(|property| Some(property.name().as_slice()) == name)
        else {
            return Ok(());
        };
        let result = match undo {
            ContextUndoInfo::Modern(_) => Err(PointwiseError::InvalidCast),
            ContextUndoInfo::Double(v) => {
                let value = match direction {
                    HistoryDirection::Undo => v.old_value(),
                    HistoryDirection::Redo => v.new_value(),
                };
                match v.property_change() {
                    0 => {
                        if matches!(
                            property.value(),
                            super::PointwiseParameterValue::Number { .. }
                        ) {
                            property.set_number(value)
                        } else {
                            Err(PointwiseError::InvalidCast)
                        }
                    }
                    1 | 2 => {
                        if matches!(
                            property.value(),
                            super::PointwiseParameterValue::Pair { .. }
                        ) {
                            property.set_axis((v.property_change() - 1) as usize, value)
                        } else {
                            Err(PointwiseError::InvalidCast)
                        }
                    }
                    _ => {
                        return Err(HistoryError::MissingDependency(
                            HistoryEffect::PropertyTarget,
                        ));
                    }
                }
            }
            ContextUndoInfo::Pair(v) => {
                let values = match direction {
                    HistoryDirection::Undo => v.old_value().values(),
                    HistoryDirection::Redo => v.new_value().values(),
                };
                if matches!(
                    property.value(),
                    super::PointwiseParameterValue::Pair { .. }
                ) {
                    property.set_pair(values)
                } else {
                    Err(PointwiseError::InvalidCast)
                }
            }
            ContextUndoInfo::Choice(v) => {
                let value = match direction {
                    HistoryDirection::Undo => v.old_value(),
                    HistoryDirection::Redo => v.new_value(),
                };
                if !matches!(
                    property.value(),
                    super::PointwiseParameterValue::Choice { .. }
                ) {
                    Err(PointwiseError::InvalidCast)
                } else {
                    property.apply_choice_history(value)
                }
            }
            ContextUndoInfo::Animation(v) => property.set_animation(match direction {
                HistoryDirection::Undo => v.old_value(),
                HistoryDirection::Redo => v.new_value(),
            }),
            ContextUndoInfo::Environment(v) => property.set_environment(match direction {
                HistoryDirection::Undo => v.old_value(),
                HistoryDirection::Redo => v.new_value(),
            }),
            ContextUndoInfo::Color(v) => {
                if v.property_change() != 6 {
                    return Err(HistoryError::MissingDependency(
                        HistoryEffect::PropertyTarget,
                    ));
                }
                let value = match direction {
                    HistoryDirection::Undo => v.old_color_value(),
                    HistoryDirection::Redo => v.new_color_value(),
                };
                if matches!(property.value(), super::PointwiseParameterValue::Color(_)) {
                    property.set_color(value)
                } else {
                    Err(PointwiseError::InvalidCast)
                }
            }
        };
        result.map_err(|error| HistoryError::Property(Box::new(error)))
    }
}

fn apply_generator_undo<E: 'static>(
    generator: &crate::ImageGenerator<E>,
    name: Option<&[u16]>,
    undo: &ContextUndoInfo<E>,
    direction: HistoryDirection,
) -> Result<(), HistoryError<E>> {
    let Some(property) = generator.parameters().into_iter().find(|property| {
        name.is_some_and(|name| name.iter().copied().eq(property.name().encode_utf16()))
    }) else {
        return Ok(());
    };
    let result = match (undo, direction) {
        (ContextUndoInfo::Double(undo), direction)
            if property.is_scalar() && undo.property_change() == 0 =>
        {
            property.set_number(match direction {
                HistoryDirection::Undo => undo.old_value(),
                HistoryDirection::Redo => undo.new_value(),
            })
        }
        (ContextUndoInfo::Double(undo), direction)
            if property.is_paired() && matches!(undo.property_change(), 1 | 2) =>
        {
            let value = match direction {
                HistoryDirection::Undo => undo.old_value(),
                HistoryDirection::Redo => undo.new_value(),
            };
            property.set_axis((undo.property_change() - 1) as usize, value)
        }
        (ContextUndoInfo::Pair(undo), direction) if property.is_paired() => {
            let values = match direction {
                HistoryDirection::Undo => undo.old_value().values(),
                HistoryDirection::Redo => undo.new_value().values(),
            };
            property.set_pair(values)
        }
        (ContextUndoInfo::Choice(undo), direction) if property.is_choice() => {
            let value = match direction {
                HistoryDirection::Undo => undo.old_value(),
                HistoryDirection::Redo => undo.new_value(),
            };
            match value {
                crate::ChoiceUndoValue::Index(index) => property.set_index(*index),
                crate::ChoiceUndoValue::Remembered(_) => Err(PointwiseError::InvalidCast),
            }
        }
        (ContextUndoInfo::Animation(undo), direction) => property.set_animation(match direction {
            HistoryDirection::Undo => undo.old_value(),
            HistoryDirection::Redo => undo.new_value(),
        }),
        _ => Err(PointwiseError::InvalidCast),
    };
    result.map_err(|error| HistoryError::Property(Box::new(error)))
}

#[cfg(test)]
mod laws;

impl<E> PointwiseNode<E> {
    pub(super) fn register_history_content(&self, occurrence: &PointwiseSlot<E>) {
        let node = self.core();
        let Some(logic) = guard(&node.history_binding).logic.clone() else {
            return;
        };
        let Some(parent) = guard(&logic.parent).upgrade() else {
            return;
        };
        let slot = logic.content_slot(Arc::downgrade(&node), parent.history.clone(), occurrence);
        guard(&logic.contents).push(slot);
    }

    pub(super) fn unregister_history_content(&self, occurrence: &PointwiseSlot<E>) {
        let logic = guard(&self.core().history_binding).logic.clone();
        if let Some(logic) = logic {
            guard(&logic.contents).retain(|slot| {
                !matches!(&slot.parent, SlotParent::Contents { occurrence: raw, .. } if Arc::ptr_eq(raw, &occurrence.core))
            });
        }
    }
}

impl<E> PointwiseTextureNodeLogic<E> {
    /// Source rw.s4: check the actual current asy parent, reread it, and call
    /// that base asy's y/b(acs) dispatch. The modeled base asy.b(acs) has an
    /// empty body in the installed source; there is no synthesized UI event.
    /// Allocation failure and unknown derived asy overrides remain outside this
    /// base-owner implementation.
    pub fn notify_preparation_change(&self) -> Result<(), PointwiseError<E>> {
        if guard(&self.core.parent).upgrade().is_none() {
            return Ok(());
        }
        let parent = guard(&self.core.parent)
            .upgrade()
            .ok_or(PointwiseError::NullReference)?;
        parent.source_preview_changed();
        Ok(())
    }
}

impl<E> PointwiseNode<E> {
    pub fn texture_node_logic(&self) -> Option<PointwiseTextureNodeLogic<E>> {
        guard(&self.core().history_binding)
            .logic
            .as_ref()
            .map(|core| PointwiseTextureNodeLogic {
                core: core.clone(),
                sponsor: self.sponsor.clone(),
            })
    }
}

/// Shader code changes the actual input occurrences and their history anchors
/// in the same command. The saved anchors do not sponsor the graph region.
pub(super) struct ShaderHistoryInputs<E> {
    group: Arc<AgdCore<E>>,
    slots: Vec<Arc<AsyCore<E>>>,
}

impl<E> Clone for ShaderHistoryInputs<E> {
    fn clone(&self) -> Self {
        Self {
            group: self.group.clone(),
            slots: self.slots.clone(),
        }
    }
}

impl<E> ShaderHistoryInputs<E> {
    pub(super) fn replace(&self) {
        guard(&self.group.slots).clone_from(&self.slots);
    }
}

impl<E: 'static> PointwiseNode<E> {
    pub(super) fn shader_history_inputs(
        &self,
        before: &[Arc<SlotCore>],
        after: &[Arc<SlotCore>],
    ) -> Option<(ShaderHistoryInputs<E>, ShaderHistoryInputs<E>)> {
        let node = self.core();
        let wrapper = self.texture_node_logic()?;
        let parent = wrapper.slot()?;
        let group = wrapper.input_group()?.core;
        let slots = guard(&group.slots).clone();
        let next = after
            .iter()
            .map(|input| {
                before
                    .iter()
                    .position(|old| Arc::ptr_eq(old, input))
                    .and_then(|index| slots.get(index).cloned())
                    .unwrap_or_else(|| {
                        Arc::new(AsyCore {
                            anchor: Arc::downgrade(&node),
                            name: input.name.clone(),
                            current: Mutex::new(None),
                            history: parent.core.history.clone(),
                            parent: SlotParent::Input(Arc::downgrade(&group)),
                        })
                    })
            })
            .collect();
        Some((
            ShaderHistoryInputs {
                group: group.clone(),
                slots,
            },
            ShaderHistoryInputs { group, slots: next },
        ))
    }
}
