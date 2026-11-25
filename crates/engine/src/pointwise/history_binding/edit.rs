//! Undo and redo for ordinary graph edits. Commands retain wrappers and links
//! without sponsoring the region.
use super::*;
use crate::document_container::{
    AffectedProperties, BranchOrderChange, LabLayoutEdit, PreviewSampleChange,
};
use crate::history::HistoryGraphEdit;
use crate::{PointwiseFilter, PointwiseKind};

#[path = "sharing.rs"]
mod sharing;
pub(crate) use sharing::GraphConnectionEdit;
#[path = "overview.rs"]
mod overview;

const DELETE_CURRENT: &str = "Delete Current";

struct Layout<E> {
    owner: Arc<RwCore<E>>,
    filter: bool,
    children: Vec<Option<NodeId>>,
    wrappers: Vec<Option<Arc<RwCore<E>>>>,
    contents: Vec<ContentLayout<E>>,
    positions: Vec<[f32; 2]>,
    group_inputs: Option<f64>,
}

struct ContentLayout<E> {
    occurrence: Arc<SlotCore>,
    child: Option<NodeId>,
    target: Arc<AsyCore<E>>,
    wrapper: Option<Arc<RwCore<E>>>,
}
impl<E: 'static> Layout<E> {
    fn read(node: &PointwiseNode<E>) -> Result<Self, HistoryError<E>> {
        let logic = node.texture_node_logic().ok_or_else(missing)?;
        let group = logic.input_group().ok_or_else(missing)?;
        let inputs = node.slots();
        if inputs.len() != guard(&group.core.slots).len() {
            return Err(missing());
        }
        let contents = node.contents();
        let saved = guard(&logic.core.contents);
        if contents.len() != saved.len() {
            return Err(missing());
        }
        let contents = contents.iter().zip(saved.iter()).map(|(occurrence, target)| {
            if !matches!(&target.parent, SlotParent::Contents { occurrence: raw, .. } if Arc::ptr_eq(raw, &occurrence.core)) {
                return Err(missing());
            }
            Ok(ContentLayout {
                occurrence: occurrence.core.clone(),
                child: occurrence.node().map(|node| node.id),
                target: target.clone(),
                wrapper: guard(&target.current).clone(),
            })
        }).collect::<Result<Vec<_>, _>>()?;
        drop(saved);
        let positions = if contents.is_empty() {
            Vec::new()
        } else {
            node.filter().ok_or_else(missing)?.group_content_positions()
        };
        if !contents.is_empty() && positions.len() + 1 != contents.len() {
            return Err(missing());
        }
        Ok(Self {
            owner: logic.core,
            filter: node.is_filter(),
            children: inputs
                .iter()
                .map(|slot| slot.node().map(|n| n.id))
                .collect(),
            wrappers: guard(&group.core.slots)
                .iter()
                .map(|slot| guard(&slot.current).clone())
                .collect(),
            contents,
            positions,
            group_inputs: node
                .kind()
                .filter(|kind| kind.is_group())
                .and_then(|_| node.filter())
                .map(|group| group.parameter(0).map(|value| value.number()))
                .transpose()
                .map_err(|error| HistoryError::Property(Box::new(error)))?,
        })
    }

    fn matches(&self, node: &PointwiseNode<E>) -> Result<bool, HistoryError<E>> {
        let current = Self::read(node)?;
        Ok(Arc::ptr_eq(&self.owner, &current.owner)
            && self.filter == current.filter
            && self.group_inputs == current.group_inputs
            && self.children == current.children
            && self
                .wrappers
                .iter()
                .zip(&current.wrappers)
                .all(|(a, b)| same_wrapper(a, b))
            && self.contents.len() == current.contents.len()
            && self.contents.iter().zip(&current.contents).all(|(a, b)| {
                Arc::ptr_eq(&a.occurrence, &b.occurrence)
                    && Arc::ptr_eq(&a.target, &b.target)
                    && a.child == b.child
                    && same_wrapper(&a.wrapper, &b.wrapper)
            })
            && self
                .positions
                .iter()
                .zip(&current.positions)
                .all(|(a, b)| a.map(f32::to_bits) == b.map(f32::to_bits)))
    }

    fn restore_links(&self, node: &PointwiseNode<E>) -> Result<(), HistoryError<E>> {
        node.restore_input_links(self.filter, &self.children);
        if !self.contents.is_empty() {
            let raw: Vec<_> = self
                .contents
                .iter()
                .map(|content| (content.occurrence.clone(), content.child))
                .collect();
            node.restore_content_links(&raw);
            node.filter()
                .ok_or_else(missing)?
                .replace_wizard_content_positions(self.positions.clone());
            *guard(&self.owner.contents) = self
                .contents
                .iter()
                .map(|content| content.target.clone())
                .collect();
        }
        Ok(())
    }

    fn slots(&self, sponsor: &Arc<Sponsor<E>>) -> Vec<PointwiseHistorySlot<E>> {
        let group = guard(&self.owner.input_group)
            .clone()
            .expect("captured input group");
        guard(&group.slots)
            .iter()
            .map(|slot| PointwiseHistorySlot {
                core: slot.clone(),
                sponsor: sponsor.clone(),
            })
            .collect()
    }
}

struct BranchLayout<E> {
    slot: Arc<AsyCore<E>>,
    wrapper: Option<Arc<RwCore<E>>>,
}

struct GraphLayout<E> {
    inputs: Vec<Layout<E>>,
    branches: Vec<BranchLayout<E>>,
}
type SlotAssignment<'a, E> = (PointwiseHistorySlot<E>, &'a Option<Arc<RwCore<E>>>);

impl<E: 'static> GraphLayout<E> {
    fn read(
        inputs: &[PointwiseNode<E>],
        branches: &[PointwiseHistorySlot<E>],
    ) -> Result<Self, HistoryError<E>> {
        Ok(Self {
            inputs: inputs.iter().map(Layout::read).collect::<Result<_, _>>()?,
            branches: branches
                .iter()
                .map(|branch| BranchLayout {
                    slot: branch.core.clone(),
                    wrapper: guard(&branch.core.current).clone(),
                })
                .collect(),
        })
    }

    fn slots(&self, sponsor: &Arc<Sponsor<E>>) -> Vec<SlotAssignment<'_, E>> {
        self.inputs
            .iter()
            .flat_map(|layout| layout.slots(sponsor).into_iter().zip(&layout.wrappers))
            .chain(self.inputs.iter().flat_map(|layout| {
                layout.contents.iter().map(|content| {
                    (
                        PointwiseHistorySlot {
                            core: content.target.clone(),
                            sponsor: sponsor.clone(),
                        },
                        &content.wrapper,
                    )
                })
            }))
            .chain(self.branches.iter().map(|branch| {
                (
                    PointwiseHistorySlot {
                        core: branch.slot.clone(),
                        sponsor: sponsor.clone(),
                    },
                    &branch.wrapper,
                )
            }))
            .collect()
    }
}

struct GraphEdit<E> {
    anchor: Weak<NodeCore<E>>,
    before: GraphLayout<E>,
    after: GraphLayout<E>,
    order: Option<BranchOrderChange>,
    preview_sample: Option<PreviewSampleChange>,
    lab: Option<crate::document_container::LabLayoutEdit<E>>,
}

fn missing<E>() -> HistoryError<E> {
    HistoryError::MissingDependency(HistoryEffect::InsertionSlotWrapper)
}

fn history_error<E>(error: HistoryError<E>) -> PointwiseError<E> {
    PointwiseError::History(Box::new(error))
}

impl<E: 'static> HistoryGraphEdit<E> for GraphEdit<E> {
    fn apply(&self, direction: HistoryDirection) -> Result<(), HistoryError<E>> {
        let undo = matches!(direction, HistoryDirection::Undo);
        if self
            .order
            .as_ref()
            .is_some_and(|order| !order.matches(undo))
        {
            return Err(HistoryError::BranchChanged);
        }
        if self
            .preview_sample
            .as_ref()
            .is_some_and(|sample| !sample.matches(undo))
        {
            return Err(HistoryError::InputChanged);
        }
        let (expected, restored) = match direction {
            HistoryDirection::Undo => (&self.after, &self.before),
            HistoryDirection::Redo => (&self.before, &self.after),
        };
        let anchor = self.anchor.upgrade().ok_or_else(missing)?;
        let sponsor = Sponsor::for_node(&anchor).ok_or_else(missing)?;
        let node = |id| PointwiseNode {
            id,
            sponsor: sponsor.clone(),
        };
        for layout in &expected.inputs {
            if !layout.matches(&node(layout.owner.node))? {
                return Err(HistoryError::InputChanged);
            }
        }
        for branch in &expected.branches {
            if !same_wrapper(&guard(&branch.slot.current), &branch.wrapper) {
                return Err(HistoryError::BranchChanged);
            }
        }

        let current_slots = expected.slots(&sponsor);
        let restored_slots = restored.slots(&sponsor);
        for (slot, wrapper) in &restored_slots {
            let old_slot = wrapper
                .as_ref()
                .and_then(|wrapper| guard(&wrapper.parent).upgrade());
            if !slot.can_restore_wrappers(wrapper.as_ref())
                || old_slot.is_some_and(|old| {
                    !current_slots
                        .iter()
                        .any(|(current, _)| Arc::ptr_eq(&current.core, &old))
                })
            {
                return Err(if slot.is_document_root() {
                    HistoryError::BranchChanged
                } else {
                    HistoryError::InputChanged
                });
            }
        }
        for wrapper in restored
            .branches
            .iter()
            .filter_map(|branch| branch.wrapper.as_ref())
        {
            if let Some(parent) = node(wrapper.node).parent()
                && !expected.inputs.iter().any(|input| {
                    input.owner.node == parent.id
                        && (input.children.contains(&Some(wrapper.node))
                            || input
                                .contents
                                .iter()
                                .any(|content| content.child == Some(wrapper.node)))
                })
            {
                return Err(HistoryError::BranchChanged);
            }
        }

        let notify_lab = self.lab.as_ref().map(|lab| lab.publish(undo)).transpose()?;
        // A move reuses the same wrapper at a different slot. Detach all slots
        // in the command and restore the real links before notifying observers.
        for (slot, _) in current_slots {
            slot.replace_wrapper(None);
        }
        for layout in &restored.inputs {
            layout.restore_links(&node(layout.owner.node))?;
        }
        for wrapper in restored
            .branches
            .iter()
            .filter_map(|branch| branch.wrapper.as_ref())
        {
            node(wrapper.node).promote_restored_branch();
        }
        for (slot, wrapper) in restored_slots {
            slot.restore_wrapper(wrapper.clone());
        }
        if let Some(order) = &self.order {
            order.publish(undo);
        }
        if let Some(sample) = &self.preview_sample {
            sample.publish(undo);
        }
        for (before, after) in expected.inputs.iter().zip(&restored.inputs) {
            if before.group_inputs != after.group_inputs
                && let Some(count) = after.group_inputs
            {
                node(after.owner.node)
                    .filter()
                    .ok_or_else(missing)?
                    .parameter(0)
                    .and_then(|parameter| parameter.set_group_inputs_for_graph_edit(count))
                    .map_err(|error| HistoryError::Property(Box::new(error)))?;
            }
        }
        for wrapper in restored
            .branches
            .iter()
            .filter_map(|branch| branch.wrapper.as_ref())
        {
            node(wrapper.node)
                .clear_mode_subtree(&mut BTreeSet::new())
                .map_err(|error| HistoryError::Property(Box::new(error)))?;
        }
        for (before, after) in expected.inputs.iter().zip(&restored.inputs) {
            node(after.owner.node)
                .notify_restored_inputs(
                    before
                        .children
                        .iter()
                        .zip(&after.children)
                        .enumerate()
                        .filter_map(|(index, (a, b))| (a != b).then_some(index)),
                )
                .map_err(|error| HistoryError::Property(Box::new(error)))?;
        }
        notify_lab.map_or(Ok(()), AffectedProperties::notify)
    }
}

impl<E: 'static> PointwiseSlot<E> {
    /// Place a fresh downstream node at this occurrence, keeping its old child
    /// as the inserted node's first input.
    pub fn insert_below(&self, node: &PointwiseNode<E>) -> Result<(), PointwiseError<E>> {
        let old = self.node().ok_or(PointwiseError::InvalidOperation)?;
        if let Some(target) = self.parent().child_history_slot(self)? {
            return target.insert_below(node);
        }
        let input = node.prepare_insert_below(&old)?;
        self.set_node_quiet(Some(node))?;
        old.promote_restored_branch();
        input.set_node_quiet(Some(&old))?;
        node.notify_slot_change(&input)?;
        self.parent().notify_slot_change(self)
    }

    /// Apply an editor connection change and record it in the owning document's
    /// history. Unbound inputs use the ordinary slot setter.
    pub fn edit_node(&self, node: Option<&PointwiseNode<E>>) -> Result<(), PointwiseError<E>> {
        self.edit_node_with_lab(node, None, None)
    }

    /// Remove the selected node, promoting its first connected editor input.
    pub fn delete_current(&self) -> Result<(), PointwiseError<E>> {
        self.delete_current_with_lab(None)
    }

    pub(crate) fn delete_current_with_lab(
        &self,
        lab: Option<LabLayoutEdit<E>>,
    ) -> Result<(), PointwiseError<E>> {
        let old = self.node().ok_or(PointwiseError::InvalidOperation)?;
        let retained = old.first_connected_editor_input()?;
        if retained.is_none() {
            let parent = self.parent();
            if let Some(group) = parent.filter().filter(|filter| filter.kind().is_group())
                && parent
                    .contents()
                    .iter()
                    .skip(1)
                    .any(|slot| slot.same_reference(self))
            {
                return group.edit_remove_group_content_with_label(self, DELETE_CURRENT, lab);
            }
        }
        self.edit_node_with_lab(retained.as_ref(), Some(DELETE_CURRENT), lab)
    }

    pub(crate) fn edit_node_with_lab(
        &self,
        node: Option<&PointwiseNode<E>>,
        label: Option<&str>,
        lab: Option<LabLayoutEdit<E>>,
    ) -> Result<(), PointwiseError<E>> {
        let parent = self.parent();
        let Some(target) = parent.child_history_slot(self)? else {
            return self
                .set_node_and_publish_lab(node, lab.as_ref())?
                .map_or(Ok(()), |affected| affected.notify().map_err(history_error));
        };
        let history = target.history().ok_or_else(|| history_error(missing()))?;
        if history.is_suppressed()
            || !history.is_recording()
            || same_node(self.node().as_ref(), node)
        {
            return self
                .set_node_and_publish_lab(node, lab.as_ref())?
                .map_or(Ok(()), |affected| affected.notify().map_err(history_error));
        }
        let source = target.prepare_input(&parent, node)?;
        let old_parent = if let Some(source) = &source {
            source.graph_occurrence()?.map(|input| input.parent())
        } else {
            if let Some(node) = node
                && node.parent().is_some_and(|parent| {
                    parent
                        .slots()
                        .into_iter()
                        .chain(parent.contents())
                        .any(|slot| same_node(slot.node().as_ref(), Some(node)))
                })
            {
                return Err(PointwiseError::UnsupportedCapability(
                    "moving from an unbound input into document history",
                ));
            }
            None
        };
        let mut owners = Vec::with_capacity(2);
        if let Some(old_parent) = old_parent.filter(|old| !old.same_reference(&parent)) {
            owners.push(old_parent);
        }
        owners.push(parent);
        let branches: Vec<_> = source
            .iter()
            .filter(|slot| slot.is_document_root())
            .cloned()
            .collect();
        let before = GraphLayout::read(&owners, &branches).map_err(history_error)?;
        let label = label.unwrap_or_else(|| {
            match (self.node().is_some(), node.is_some(), source.is_some()) {
                (_, _, true) => "Move Node",
                (_, false, _) => "Disconnect Node",
                (true, true, _) => "Replace Node",
                (false, true, _) => "Connect Node",
            }
        });
        let notify_lab = self.set_node_and_publish_lab(node, lab.as_ref())?;
        let after = GraphLayout::read(&owners, &branches).map_err(history_error)?;
        let recorded = history
            .record_graph_edit(
                target.history_target(),
                label.encode_utf16().collect(),
                Box::new(GraphEdit {
                    anchor: target.core.anchor.clone(),
                    before,
                    after,
                    order: None,
                    preview_sample: None,
                    lab,
                }),
            )
            .map_err(history_error);
        recorded.and(notify_lab.map_or(Ok(()), |affected| affected.notify().map_err(history_error)))
    }

    fn set_node_and_publish_lab(
        &self,
        node: Option<&PointwiseNode<E>>,
        lab: Option<&LabLayoutEdit<E>>,
    ) -> Result<Option<AffectedProperties<E>>, PointwiseError<E>> {
        let previous = self.node();
        let notify_lab = lab
            .map(|lab| lab.publish(false))
            .transpose()
            .map_err(history_error)?;
        if let Err(error) = self.set_node(node) {
            if same_node(self.node().as_ref(), previous.as_ref())
                && let Some(lab) = lab
            {
                let _ = lab.publish(true);
            }
            return Err(error);
        }
        Ok(notify_lab)
    }
}

impl<E: 'static> PointwiseNode<E> {
    pub(crate) fn edit_filter_preview_sample(
        &self,
        branch: Option<&PointwiseHistorySlot<E>>,
        change: PreviewSampleChange,
    ) -> Result<(), PointwiseError<E>> {
        let input = self.slot(0)?;
        if !input.node().is_some_and(|child| child.is_default_input()) {
            return Err(PointwiseError::InvalidOperation);
        }
        let sample = Self::new_preview_sample(change.sample())?;
        let owners = std::slice::from_ref(self);
        if let Some(branch) = branch {
            let history = branch.history().ok_or_else(|| history_error(missing()))?;
            let before = GraphLayout::read(owners, &[]).map_err(history_error)?;
            input.set_node_quiet(Some(&sample))?;
            let after = GraphLayout::read(owners, &[]).map_err(history_error)?;
            change.publish(false);
            let result = history.record_graph_edit(
                branch.history_target(),
                "Change Preview Sample".encode_utf16().collect(),
                Box::new(GraphEdit {
                    anchor: branch.core.anchor.clone(),
                    before,
                    after,
                    order: None,
                    preview_sample: Some(change),
                    lab: None,
                }),
            );
            let notification = self.notify_restored_inputs([0]);
            result.map_err(history_error).and(notification)
        } else {
            input.set_node_quiet(Some(&sample))?;
            change.publish(false);
            self.notify_restored_inputs([0])
        }
    }

    pub(crate) fn first_connected_editor_input(&self) -> Result<Option<Self>, PointwiseError<E>> {
        if self.is_default_input() {
            return Err(PointwiseError::InvalidOperation);
        }
        let slots = if self.kind().is_some_and(PointwiseKind::is_group) {
            self.reference_inputs()?
        } else {
            self.slots()
        };
        Ok(slots
            .into_iter()
            .filter_map(|slot| slot.node())
            .find(|node| !node.is_default_input()))
    }

    pub(crate) fn prepare_insert_below(
        &self,
        old: &Self,
    ) -> Result<PointwiseSlot<E>, PointwiseError<E>> {
        if self.same_reference(old)
            || self.parent().is_some()
            || self.texture_node_logic().is_some()
            || old.is_default_input()
        {
            return Err(PointwiseError::InvalidOperation);
        }
        let input = self.slot(0).map_err(|_| {
            PointwiseError::UnsupportedCapability("Insert Below requires a node with an input")
        })?;
        if input.node().is_some_and(|node| !node.is_default_input()) {
            return Err(PointwiseError::InvalidOperation);
        }
        // TexProgLogic xq.d prepares the new node using the old node's
        // retained channel guarantees.
        if self.kind().is_some_and(PointwiseKind::is_group) {
            let filter = self.filter().ok_or(PointwiseError::InvalidCast)?;
            let inputs = filter.parameter(0)?;
            if inputs.number() == 0. {
                inputs.set_number(1.)?;
            }
        }
        let heightmap = old.prepared_image_guarantees().is_some_and(|guarantees| {
            let [red, green, blue, transparency] = guarantees.constants();
            red != Some(0.) && [green, blue, transparency] == [Some(0.); 3]
        });
        if heightmap {
            if matches!(&self.core().payload, Payload::Invert { .. }) {
                self.set_invert_red_only()
                    .map_err(crate::pointwise::invert_node::graph_error)?;
            } else if self.kind() == Some(PointwiseKind::ToneCurve) {
                let filter = self.filter().ok_or(PointwiseError::InvalidCast)?;
                for (index, choice) in [(1, 0), (2, 1), (3, 1), (4, 1)] {
                    filter.parameter(index)?.set_index(choice)?;
                }
            }
        }
        Ok(input)
    }
}

fn same_node<E>(a: Option<&PointwiseNode<E>>, b: Option<&PointwiseNode<E>>) -> bool {
    a.map(|n| n.id) == b.map(|n| n.id)
}

fn edit_contents_unrecorded<E: 'static, T>(
    lab: Option<&LabLayoutEdit<E>>,
    change: impl FnOnce() -> Result<T, PointwiseError<E>>,
) -> Result<T, PointwiseError<E>> {
    let notify = lab
        .map(|lab| lab.publish(false))
        .transpose()
        .map_err(history_error)?;
    let result = change()?;
    if let Some(notify) = notify {
        notify.notify().map_err(history_error)?;
    }
    Ok(result)
}

impl<E: 'static> PointwiseFilter<E> {
    /// Append a group contents branch as an editor history command.
    pub fn edit_add_group_content(
        &self,
        position: [f32; 2],
    ) -> Result<PointwiseSlot<E>, PointwiseError<E>> {
        self.edit_contents("Add Contents Branch", || {
            self.append_group_content(position)
        })
    }

    /// Add a fresh branch to a Group's contents in one topology edit.
    pub fn edit_append_group_branch(
        &self,
        position: [f32; 2],
        node: &PointwiseNode<E>,
    ) -> Result<PointwiseSlot<E>, PointwiseError<E>> {
        if node.parent().is_some() || node.has_live_property_history_binding() {
            return Err(PointwiseError::InvalidOperation);
        }
        self.edit_contents("Add Contents Branch", || {
            let slot = self.append_group_content(position)?;
            if let Err(error) = slot.set_node(Some(node)) {
                if slot.node().is_none() {
                    self.remove_empty_group_content(&slot)?;
                }
                return Err(error);
            }
            Ok(slot)
        })
    }

    /// Clear and remove an extra contents branch in one editor history command.
    pub fn edit_remove_group_content(
        &self,
        slot: &PointwiseSlot<E>,
    ) -> Result<(), PointwiseError<E>> {
        self.edit_remove_group_content_with_lab(slot, None)
    }

    /// Move an extra contents branch without changing the node connections.
    pub fn edit_group_content_position(
        &self,
        target: &PointwiseSlot<E>,
        position: [f32; 2],
    ) -> Result<bool, PointwiseError<E>> {
        if !position.into_iter().all(f32::is_finite) {
            return Err(PointwiseError::InvalidOperation);
        }
        let index = self
            .node()
            .contents()
            .iter()
            .skip(1)
            .position(|slot| slot.same_reference(target))
            .ok_or(PointwiseError::InvalidOperation)?;
        let mut positions = self.group_content_positions();
        let current = positions
            .get_mut(index)
            .ok_or(PointwiseError::InvalidOperation)?;
        if current.map(f32::to_bits) == position.map(f32::to_bits) {
            return Ok(false);
        }
        *current = position;
        self.edit_contents("Move Disconnected Branch", || {
            self.replace_wizard_content_positions(positions);
            Ok(true)
        })
    }

    pub(crate) fn edit_remove_group_content_with_lab(
        &self,
        slot: &PointwiseSlot<E>,
        lab: Option<LabLayoutEdit<E>>,
    ) -> Result<(), PointwiseError<E>> {
        self.edit_remove_group_content_with_label(slot, "Remove Contents Branch", lab)
    }

    fn edit_remove_group_content_with_label(
        &self,
        slot: &PointwiseSlot<E>,
        label: &str,
        lab: Option<LabLayoutEdit<E>>,
    ) -> Result<(), PointwiseError<E>> {
        if !self
            .node()
            .contents()
            .iter()
            .skip(1)
            .any(|current| current.same_reference(slot))
        {
            return Err(PointwiseError::InvalidOperation);
        }
        self.edit_contents_with_lab(label, lab, || {
            slot.set_node(None)?;
            self.remove_empty_group_content(slot)
        })
    }

    fn edit_contents<T>(
        &self,
        label: &str,
        change: impl FnOnce() -> Result<T, PointwiseError<E>>,
    ) -> Result<T, PointwiseError<E>> {
        self.edit_contents_with_lab(label, None, change)
    }

    fn edit_contents_with_lab<T>(
        &self,
        label: &str,
        lab: Option<LabLayoutEdit<E>>,
        change: impl FnOnce() -> Result<T, PointwiseError<E>>,
    ) -> Result<T, PointwiseError<E>> {
        let node = self.node();
        let Some(target) = node.texture_node_logic().and_then(|logic| logic.slot()) else {
            return edit_contents_unrecorded(lab.as_ref(), change);
        };
        let history = target.history().ok_or_else(|| history_error(missing()))?;
        if history.is_suppressed() || !history.is_recording() {
            return edit_contents_unrecorded(lab.as_ref(), change);
        }
        let owners = [node];
        let before = GraphLayout::read(&owners, &[]).map_err(history_error)?;
        let notify_lab = lab
            .as_ref()
            .map(|lab| lab.publish(false))
            .transpose()
            .map_err(history_error)?;
        let result = change()?;
        let after = GraphLayout::read(&owners, &[]).map_err(history_error)?;
        let recorded = history
            .record_graph_edit(
                target.history_target(),
                label.encode_utf16().collect(),
                Box::new(GraphEdit {
                    anchor: target.core.anchor.clone(),
                    before,
                    after,
                    order: None,
                    preview_sample: None,
                    lab,
                }),
            )
            .map_err(history_error);
        recorded
            .and(notify_lab.map_or(Ok(()), |affected| affected.notify().map_err(history_error)))?;
        Ok(result)
    }
}

fn same_wrapper<E>(a: &Option<Arc<RwCore<E>>>, b: &Option<Arc<RwCore<E>>>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        (None, None) => true,
        _ => false,
    }
}

impl<E: 'static> PointwiseHistorySlot<E> {
    pub(crate) fn insert_below(&self, node: &PointwiseNode<E>) -> Result<(), PointwiseError<E>> {
        let previous = self
            .texture_node()
            .ok_or(PointwiseError::InvalidOperation)?;
        let old = previous.node();
        let input = node.prepare_insert_below(&old)?;
        Self::validate_trees(Some(node)).map_err(history_error)?;
        let history = self.history().ok_or_else(|| history_error(missing()))?;
        let outer = self.graph_occurrence()?;
        let mut owners = vec![node.clone()];
        let mut branches = Vec::new();
        if let Some(outer) = &outer {
            owners.push(outer.parent());
        } else {
            branches.push(self.clone());
        }

        // Stage the new node's editor wrapper so the ordinary outer setter
        // can transfer it. This slot never belongs to the document's roots.
        self.sponsor.merge(&node.sponsor);
        let temporary = Self::bind(node, &history, Vec::new());
        temporary.set_node(Some(node));
        let result = (|| {
            let before = GraphLayout::read(&owners, &branches).map_err(history_error)?;
            let inner = node
                .child_history_slot(&input)?
                .ok_or_else(|| history_error(missing()))?;
            if let Some(outer) = &outer {
                outer.set_node_quiet(Some(node))?;
            } else {
                self.replace_document_branch(Some(node), false)
                    .map_err(history_error)?;
            }
            // Keep the original subtree's editor wrappers and publish both
            // real links before notifying observers.
            old.promote_restored_branch();
            inner.restore_wrapper(Some(previous.core.clone()));
            input.set_node_quiet(Some(&old))?;
            node.notify_slot_change(&input)?;
            if let Some(outer) = self.graph_occurrence()? {
                outer.parent().notify_slot_change(&outer)?;
            }
            let after = GraphLayout::read(&owners, &branches).map_err(history_error)?;
            history
                .record_graph_edit(
                    self.history_target(),
                    "Insert Below".encode_utf16().collect(),
                    Box::new(GraphEdit {
                        anchor: self.core.anchor.clone(),
                        before,
                        after,
                        order: None,
                        preview_sample: None,
                        lab: None,
                    }),
                )
                .map_err(history_error)
        })();
        temporary.release_document_binding();
        result
    }

    pub(crate) fn replace_document_branch(
        &self,
        node: Option<&PointwiseNode<E>>,
        record: bool,
    ) -> Result<(), HistoryError<E>> {
        self.replace_document_branch_with_lab(node, record, None, None)
    }

    pub(crate) fn replace_document_branch_with_lab(
        &self,
        node: Option<&PointwiseNode<E>>,
        record: bool,
        label: Option<&str>,
        lab: Option<LabLayoutEdit<E>>,
    ) -> Result<(), HistoryError<E>> {
        self.replace_document_branch_with_label(node, record, label, None, lab)
    }

    pub(crate) fn append_document_branch(
        &self,
        node: &PointwiseNode<E>,
        order: BranchOrderChange,
    ) -> Result<(), HistoryError<E>> {
        self.replace_document_branch_with_label(
            Some(node),
            true,
            Some("Add Branch"),
            Some(order),
            None,
        )
    }

    pub(crate) fn delete_current(
        &self,
        retained: Option<&PointwiseNode<E>>,
        order: Option<BranchOrderChange>,
        lab: Option<LabLayoutEdit<E>>,
    ) -> Result<(), HistoryError<E>> {
        self.replace_document_branch_with_label(retained, true, Some(DELETE_CURRENT), order, lab)
    }

    fn replace_document_branch_with_label(
        &self,
        node: Option<&PointwiseNode<E>>,
        record: bool,
        label: Option<&str>,
        order: Option<BranchOrderChange>,
        lab: Option<LabLayoutEdit<E>>,
    ) -> Result<(), HistoryError<E>> {
        let previous = guard(&self.core.current).clone();
        if previous.as_ref().map(|wrapper| wrapper.node) == node.map(|node| node.id) {
            return Ok(());
        }
        let source = node
            .map(|node| self.move_source(node))
            .transpose()
            .map_err(|error| HistoryError::Property(Box::new(error)))?
            .flatten();
        if source.is_none() {
            Self::validate_trees(node)?;
        }
        let inputs: Vec<_> = if let Some(source) = &source {
            source
                .graph_occurrence()
                .map_err(|error| HistoryError::Property(Box::new(error)))?
                .map(|input| input.parent())
                .into_iter()
                .collect()
        } else {
            Vec::new()
        };
        let mut branches = Vec::with_capacity(2);
        branches.extend(
            source
                .iter()
                .filter(|slot| slot.is_document_root())
                .cloned(),
        );
        branches.push(self.clone());
        let history = if record {
            Some(self.history().ok_or_else(missing)?)
        } else {
            None
        };
        let before = history
            .as_ref()
            .map(|_| GraphLayout::read(&inputs, &branches))
            .transpose()?;

        let notify_lab = lab.as_ref().map(|lab| lab.publish(false)).transpose()?;
        self.publish_input(source.as_ref(), node);
        if let Some(node) = node {
            node.assign_parent(None)
                .map_err(|error| HistoryError::Property(Box::new(error)))?;
        }
        if let Some(order) = &order {
            order.publish(false);
        }
        if let Some((history, before)) = history.zip(before) {
            let label = label.unwrap_or(
                match (previous.is_some(), node.is_some(), source.is_some()) {
                    (_, _, true) => "Move Node",
                    (_, false, _) => "Clear Branch",
                    (false, true, _) => "Set Branch",
                    (true, true, _) => "Replace Branch",
                },
            );
            let after = GraphLayout::read(&inputs, &branches)?;
            history.record_graph_edit(
                self.history_target(),
                label.encode_utf16().collect(),
                Box::new(GraphEdit {
                    anchor: self.core.anchor.clone(),
                    before,
                    after,
                    order,
                    preview_sample: None,
                    lab,
                }),
            )?;
        }
        notify_lab.map_or(Ok(()), AffectedProperties::notify)
    }
}
