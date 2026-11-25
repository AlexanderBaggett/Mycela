//! Graph connections that can enclose the current document in a Group.
use super::*;
use crate::document_container::LabLayoutEdit;
use std::collections::BTreeMap;

const LABEL: &str = "Reference-Copy";

impl<E: 'static> PointwiseNode<E> {
    pub(crate) fn prepare_document_input(
        &self,
        principal: Option<&Self>,
    ) -> Result<GraphConnectionEdit<E>, PointwiseError<E>> {
        PointwiseHistorySlot::validate_trees([self]).map_err(history_error)?;
        let group = PointwiseFilter::new(PointwiseKind::Group)?;
        let input = group.node().slot(0)?;
        Ok(GraphConnectionEdit {
            label: "Insert Document Input",
            slots: vec![(input, self.clone())],
            counts: Vec::new(),
            enclosure: Some(group),
            filter_root: principal.filter(|node| node.is_filter()).cloned(),
        })
    }

    pub(crate) fn group_branch_copy(source: Option<&Self>) -> Result<Self, PointwiseError<E>> {
        let group = PointwiseFilter::new(PointwiseKind::Group)?;
        let node = group.node();
        let ports = node.slots();
        let mut references = BTreeMap::<usize, Vec<PointwiseFilter<E>>>::new();
        if let Some(source) = source {
            if source.is_default_input() {
                return Err(PointwiseError::UnsupportedCapability(
                    "grouping a document preview sample",
                ));
            }
            let copy = source.source_clone()?;
            let mut pending = vec![copy.clone()];
            while let Some(child) = pending.pop() {
                if child.kind() == Some(PointwiseKind::Reference)
                    && let Some(index) = child.reference_slot_at(None)?
                    && index < ports.len()
                {
                    references
                        .entry(index)
                        .or_default()
                        .push(child.filter().ok_or(PointwiseError::InvalidCast)?);
                }
                // A nested Group's inputs still belong to this level.
                // Its contents select that nested Group's own ports.
                pending.extend(child.slots().iter().filter_map(PointwiseSlot::node));
            }
            node.content(0)?.set_node_quiet(Some(&copy))?;
        }
        group.parameter(0)?.set_number(references.len() as f64)?;
        for (compact, (outer, receivers)) in references.into_iter().enumerate() {
            ports[compact].set_node_quiet(Some(&numbered_reference(outer)?))?;
            for receiver in receivers {
                receiver.parameter(0)?.set_number((compact + 1) as f64)?;
            }
        }
        Ok(node)
    }

    pub(crate) fn connected_occurrence(&self) -> Option<PointwiseSlot<E>> {
        let parent = self.parent()?;
        parent
            .slots()
            .into_iter()
            .chain(parent.contents())
            .find(|slot| slot.node().is_some_and(|node| node.same_reference(self)))
    }
}

impl<E: 'static> PointwiseSlot<E> {
    pub(crate) fn validate_reference_copy(
        &self,
        destination: &Self,
    ) -> Result<(), PointwiseError<E>> {
        if self.same_reference(destination) {
            return Err(PointwiseError::InvalidOperation);
        }
        let owner = destination.parent();
        if !owner
            .slots()
            .into_iter()
            .chain(owner.contents())
            .any(|slot| slot.same_reference(destination))
        {
            return Err(PointwiseError::InvalidOperation);
        }
        let source = self.node().ok_or(PointwiseError::InvalidOperation)?;
        if source.is_default_input() {
            return Err(PointwiseError::InvalidOperation);
        }
        let mut current = Some(destination.parent());
        let mut visited = BTreeSet::new();
        while let Some(node) = current {
            if node.same_reference(&source) || !visited.insert(node.id) {
                return Err(PointwiseError::InvalidOperation);
            }
            current = node.parent();
        }
        if let Some(replaced) = destination.node() {
            current = Some(self.parent());
            visited.clear();
            while let Some(node) = current {
                if node.same_reference(&replaced) || !visited.insert(node.id) {
                    return Err(PointwiseError::InvalidOperation);
                }
                current = node.parent();
            }
        }
        Ok(())
    }

    pub(crate) fn prepare_reference_copy(
        &self,
        destination: &Self,
    ) -> Result<GraphConnectionEdit<E>, PointwiseError<E>> {
        let mut copy = GraphConnectionEdit {
            label: LABEL,
            slots: Vec::new(),
            counts: Vec::new(),
            enclosure: None,
            filter_root: None,
        };
        let reference = copy.share(self)?;
        copy.slots.push((destination.clone(), reference));
        PointwiseHistorySlot::validate_trees(copy.slots.iter().map(|(_, node)| node))
            .map_err(history_error)?;
        Ok(copy)
    }
}

/// An edit prepared on detached copies. All live occurrences are written only
/// after the dependencies fit in the enclosing Groups.
pub(crate) struct GraphConnectionEdit<E> {
    label: &'static str,
    slots: Vec<(PointwiseSlot<E>, PointwiseNode<E>)>,
    counts: Vec<(PointwiseFilter<E>, usize)>,
    enclosure: Option<PointwiseFilter<E>>,
    filter_root: Option<PointwiseNode<E>>,
}

impl<E: 'static> GraphConnectionEdit<E> {
    pub(crate) fn needs_enclosure(&self) -> bool {
        self.enclosure.is_some()
    }

    pub(crate) fn enclosure_node(&self) -> Option<PointwiseNode<E>> {
        self.enclosure.as_ref().map(PointwiseFilter::node)
    }

    pub(crate) fn validate_reference_enclosure(
        &self,
        roots: &[Option<PointwiseNode<E>>],
    ) -> Result<(), PointwiseError<E>> {
        roots
            .iter()
            .flatten()
            .try_for_each(require_closed_references)
    }

    pub(crate) fn projected_node(&self, occurrence: &PointwiseSlot<E>) -> Option<PointwiseNode<E>> {
        self.slots
            .iter()
            .find(|(slot, _)| slot.same_reference(occurrence))
            .map_or_else(|| occurrence.node(), |(_, node)| Some(node.clone()))
    }

    fn share(&mut self, source: &PointwiseSlot<E>) -> Result<PointwiseNode<E>, PointwiseError<E>> {
        if let Some((_, reference)) = self
            .slots
            .iter()
            .find(|(slot, _)| slot.same_reference(source))
        {
            // A repeated dependency copies the Reference already planned at its
            // original occurrence, as the subsequent source command would.
            return reference.source_clone();
        }
        let producer = source.node().ok_or(PointwiseError::InvalidOperation)?;
        if producer.kind() == Some(PointwiseKind::Reference) {
            return producer.source_clone();
        }
        let scope = source.reference_scope()?;
        let (group, reserved) = match &scope {
            Some(node) if node.kind() == Some(PointwiseKind::Group) => {
                let group = node.filter().ok_or(PointwiseError::InvalidCast)?;
                let reserved = if node.is_filter() {
                    self.wrap_filter(node)?
                } else {
                    0
                };
                (group, reserved)
            }
            Some(_) => {
                return Err(PointwiseError::UnsupportedCapability(
                    "sharing from this type of Group",
                ));
            }
            None => {
                if self.filter_root.is_some() {
                    return Err(PointwiseError::UnsupportedCapability(
                        "a filter document with a connected exterior dependency",
                    ));
                }
                if self.enclosure.is_none() {
                    self.enclosure = Some(PointwiseFilter::new(PointwiseKind::Group)?);
                }
                (self.enclosure.as_ref().unwrap().clone(), 0)
            }
        };
        let (index, exterior) = group
            .node()
            .slots()
            .into_iter()
            .enumerate()
            .skip(reserved)
            .find(|(_, slot)| {
                slot.node().is_none_or(|node| node.is_default_input())
                    && !self
                        .slots
                        .iter()
                        .any(|(reserved, _)| reserved.same_reference(slot))
            })
            .ok_or(PointwiseError::UnsupportedCapability(
                "the Group has no vacant inputs",
            ))?;
        if let Some((_, count)) = self
            .counts
            .iter_mut()
            .find(|(owner, _)| owner.same_reference(&group))
        {
            *count = (*count).max(index + 1);
        } else {
            self.counts.push((group, index + 1));
        }

        let copy = producer.source_clone()?;
        let reference = numbered_reference(index)?;
        self.slots.push((source.clone(), reference.clone()));
        self.slots.push((exterior, copy.clone()));

        if let Some(scope) = scope {
            let exterior = scope.reference_inputs()?;
            for (index, destination) in copied_references(&producer, &copy, &scope)? {
                if let Some(input) = exterior.get(index) {
                    let replacement = if index < reserved
                        || input.node().is_some_and(|node| !node.is_default_input())
                    {
                        Some(self.share(input)?)
                    } else {
                        None
                    };
                    destination.set_node_quiet(replacement.as_ref())?;
                }
            }
        }
        reference.source_clone()
    }

    fn wrap_filter(&mut self, root: &PointwiseNode<E>) -> Result<usize, PointwiseError<E>> {
        let reserved = root.reference_inputs()?.len().min(3);
        if self
            .filter_root
            .as_ref()
            .is_some_and(|wrapped| wrapped.same_reference(root))
        {
            return Ok(reserved);
        }
        if root.parent().is_some()
            || self.enclosure.is_some()
            || root
                .slots()
                .iter()
                .filter_map(|slot| slot.node())
                .any(|node| !node.is_default_input())
        {
            return Err(PointwiseError::UnsupportedCapability(
                "sharing with this filter Group layout",
            ));
        }
        let wrapper = PointwiseFilter::new(PointwiseKind::Group)?;
        wrapper.parameter(0)?.set_number(reserved as f64)?;
        wrapper.node().inherit_document_preview_samples(root);
        wrapper.node().convert_to_filter(false)?;
        for (index, slot) in root.slots().into_iter().take(reserved).enumerate() {
            self.slots.push((slot, numbered_reference(index)?));
        }
        self.filter_root = Some(root.clone());
        self.enclosure = Some(wrapper);
        Ok(reserved)
    }

    fn owners(&self) -> Vec<PointwiseNode<E>> {
        let mut seen = BTreeSet::new();
        self.slots
            .iter()
            .map(|(slot, _)| slot.parent())
            .chain(self.filter_root.iter().cloned())
            .filter(|node| seen.insert(node.id))
            .collect()
    }

    fn install(&self) -> Result<(), PointwiseError<E>> {
        if let Some(root) = &self.filter_root {
            root.leave_filter_mode_quiet(None)?;
        }
        for (slot, node) in &self.slots {
            slot.set_node_quiet(Some(node))?;
        }
        for (group, count) in &self.counts {
            let inputs = group.parameter(0)?;
            if inputs.number() < *count as f64 {
                inputs.set_group_inputs_for_graph_edit(*count as f64)?;
            }
        }
        Ok(())
    }

    pub(crate) fn apply(
        self,
        destination: &PointwiseSlot<E>,
        lab: Option<LabLayoutEdit<E>>,
    ) -> Result<(), PointwiseError<E>> {
        let owners = self.owners();
        let target = destination.parent().child_history_slot(destination)?;
        let before = target
            .as_ref()
            .map(|_| GraphLayout::read(&owners, &[]))
            .transpose()
            .map_err(history_error)?;
        let notify_lab = lab
            .as_ref()
            .map(|lab| lab.publish(false))
            .transpose()
            .map_err(history_error)?;
        if let Err(error) = self.install() {
            if let Some(lab) = &lab {
                let _ = lab.publish(true);
            }
            return Err(error);
        }
        if let Some((target, before)) = target.zip(before) {
            let after = GraphLayout::read(&owners, &[]).map_err(history_error)?;
            target.record_connections(self.label, before, after, None, lab)?;
        }
        notify_connections(self.slots.iter().map(|(slot, _)| slot))
            .and(notify_lab.map_or(Ok(()), |affected| affected.notify().map_err(history_error)))
    }

    pub(crate) fn enclose_roots(
        self,
        roots: &[Option<PointwiseNode<E>>],
        positions: &[[f32; 2]],
        preview_samples: &crate::document_container::DocumentPreviewSamples,
        bound: Option<(&[PointwiseHistorySlot<E>], BranchOrderChange)>,
        mut lab: Option<LabLayoutEdit<E>>,
        publish: impl FnOnce(PointwiseNode<E>),
    ) -> Result<(), PointwiseError<E>> {
        if let Some(filter) = &self.filter_root
            && !roots
                .first()
                .and_then(Option::as_ref)
                .is_some_and(|root| root.same_reference(filter))
        {
            return Err(PointwiseError::UnsupportedCapability(
                "the filter must be the principal branch",
            ));
        }
        let group = self
            .enclosure
            .as_ref()
            .ok_or(PointwiseError::InvalidOperation)?;
        for position in positions {
            group.append_group_content(*position)?;
        }
        let group = group.node();
        group.set_document_preview_samples(preview_samples);
        let contents = group.contents();
        let notify_lab = lab
            .as_ref()
            .map(|lab| lab.publish(false))
            .transpose()
            .map_err(history_error)?;
        if let Some((branches, order)) = bound {
            let branch = branches.first().ok_or(PointwiseError::InvalidOperation)?;
            let history = branch.history().ok_or_else(|| history_error(missing()))?;
            let mut owners = self.owners();
            if !owners.iter().any(|owner| owner.same_reference(&group)) {
                owners.push(group.clone());
            }
            branch.sponsor.merge(&group.sponsor);
            let temporary = PointwiseHistorySlot::bind(&group, &history, Vec::new());
            temporary.set_node(Some(&group));
            let result = (|| {
                let before = GraphLayout::read(&owners, branches).map_err(history_error)?;
                for (index, ((branch, input), root)) in
                    branches.iter().zip(&contents).zip(roots).enumerate()
                {
                    let previous = branch.texture_node();
                    let inner = group
                        .child_history_slot(input)?
                        .ok_or_else(|| history_error(missing()))?;
                    branch
                        .replace_document_branch((index == 0).then_some(&group), false)
                        .map_err(history_error)?;
                    inner.restore_wrapper(previous.map(|previous| previous.core));
                    input.set_node_quiet(root.as_ref())?;
                }
                self.install()?;
                let after = GraphLayout::read(&owners, branches).map_err(history_error)?;
                order.publish(false);
                branch.record_connections(self.label, before, after, Some(order), lab.take())
            })();
            temporary.release_document_binding();
            if result.is_err()
                && let Some(lab) = &lab
            {
                let _ = lab.publish(true);
            }
            result?;
        } else {
            let result = (|| {
                for (input, root) in contents.iter().zip(roots) {
                    input.set_node_quiet(root.as_ref())?;
                }
                self.install()
            })();
            if result.is_err()
                && let Some(lab) = &lab
            {
                let _ = lab.publish(true);
            }
            result?;
        }
        publish(group);
        if let Some(root) = &self.filter_root {
            root.clear_mode_subtree(&mut BTreeSet::new())?;
        }
        notify_connections(self.slots.iter().map(|(slot, _)| slot).chain(&contents))
            .and(notify_lab.map_or(Ok(()), |affected| affected.notify().map_err(history_error)))
    }
}

fn numbered_reference<E: 'static>(index: usize) -> Result<PointwiseNode<E>, PointwiseError<E>> {
    let reference = PointwiseFilter::new(PointwiseKind::Reference)?;
    reference.parameter(0)?.set_number((index + 1) as f64)?;
    Ok(reference.node())
}

fn copied_references<E: 'static>(
    producer: &PointwiseNode<E>,
    copy: &PointwiseNode<E>,
    scope: &PointwiseNode<E>,
) -> Result<Vec<(usize, PointwiseSlot<E>)>, PointwiseError<E>> {
    let mut pending = vec![(producer.clone(), copy.clone(), None)];
    let mut references = Vec::new();
    while let Some((old, new, occurrence)) = pending.pop() {
        if old.kind() == Some(PointwiseKind::Reference) {
            if let Some(occurrence) = occurrence
                && old
                    .reference_group()?
                    .is_some_and(|group| group.same_reference(scope))
                && let Some(index) = old.reference_slot_at(None)?
            {
                references.push((index, occurrence));
            }
            continue;
        }
        let inputs = if old.kind().is_some_and(PointwiseKind::is_group) {
            old.reference_inputs()?
        } else {
            old.slots()
        };
        // Genetica's editor address walks visible inputs. A nested Group's
        // contents belong to that Group and keep their own References.
        for (original, copied) in inputs.into_iter().zip(new.slots()).rev() {
            if let (Some(original), Some(new)) = (original.node(), copied.node()) {
                pending.push((original, new, Some(copied)));
            }
        }
    }
    references.sort_by_key(|(index, _)| *index);
    Ok(references)
}

fn notify_connections<'a, E: 'static>(
    slots: impl IntoIterator<Item = &'a PointwiseSlot<E>>,
) -> Result<(), PointwiseError<E>> {
    for slot in slots {
        slot.parent().notify_slot_change(slot)?;
    }
    Ok(())
}

fn require_closed_references<E: 'static>(root: &PointwiseNode<E>) -> Result<(), PointwiseError<E>> {
    let mut visited = BTreeSet::new();
    let mut pending = vec![root.clone()];
    while let Some(node) = pending.pop() {
        if !visited.insert(node.id) {
            continue;
        }
        if node.kind() == Some(PointwiseKind::Reference) && node.reference_group()?.is_none() {
            return Err(PointwiseError::UnsupportedCapability(
                "sharing a branch with external References",
            ));
        }
        pending.extend(
            node.slots()
                .into_iter()
                .chain(node.contents())
                .filter_map(|slot| slot.node()),
        );
    }
    Ok(())
}

impl<E: 'static> PointwiseHistorySlot<E> {
    fn record_connections(
        &self,
        label: &str,
        before: GraphLayout<E>,
        after: GraphLayout<E>,
        order: Option<BranchOrderChange>,
        lab: Option<LabLayoutEdit<E>>,
    ) -> Result<(), PointwiseError<E>> {
        self.history()
            .ok_or_else(|| history_error(missing()))?
            .record_graph_edit(
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
            )
            .map_err(history_error)
    }
}
