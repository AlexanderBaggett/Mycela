//! Apply a complete preset as one retained graph operation.
use super::*;
use crate::document_container::PresetInputMove;

impl<E: 'static> PointwiseSlot<E> {
    pub(crate) fn notify_overview_input(&self) -> Result<(), PointwiseError<E>> {
        self.parent().notify_slot_change(self)
    }
}

impl<E: 'static> PointwiseHistorySlot<E> {
    pub(crate) fn overview_slot(input: &PointwiseSlot<E>) -> Result<Self, PointwiseError<E>> {
        input
            .parent()
            .child_history_slot(input)?
            .ok_or_else(|| history_error(missing()))
    }

    pub(crate) fn edit_overview_preset(
        &self,
        node: &PointwiseNode<E>,
        transfers: &[PresetInputMove<E>],
        auxiliary: &[(Self, PointwiseNode<E>)],
        order: Option<BranchOrderChange>,
        lab: Option<LabLayoutEdit<E>>,
    ) -> Result<(), PointwiseError<E>> {
        let history = self.history().ok_or_else(|| history_error(missing()))?;
        let outer = self.graph_occurrence()?;
        let mut owners = vec![node.clone()];
        for input in outer
            .iter()
            .chain(transfers.iter().filter_map(|edit| edit.from.as_ref()))
        {
            let parent = input.parent();
            if !owners.iter().any(|owner| owner.same_reference(&parent)) {
                owners.push(parent);
            }
        }
        let branches = outer
            .is_none()
            .then(|| self.clone())
            .into_iter()
            .chain(auxiliary.iter().map(|(slot, _)| slot.clone()))
            .collect::<Vec<_>>();

        self.sponsor.merge(&node.sponsor);
        let staged = Self::bind(node, &history, Vec::new());
        staged.set_node(Some(node));
        let applied = (|| {
            let before = GraphLayout::read(&owners, &branches).map_err(history_error)?;
            let prepared = transfers
                .iter()
                .map(|transfer| {
                    let source = match &transfer.from {
                        Some(input) => Self::overview_slot(input)?,
                        None => self.clone(),
                    };
                    let retained = source
                        .texture_node()
                        .filter(|logic| logic.node().same_reference(&transfer.node))
                        .ok_or_else(|| history_error(HistoryError::InputChanged))?;
                    let destination = Self::overview_slot(&transfer.to)?;
                    Ok((retained.core, destination))
                })
                .collect::<Result<Vec<_>, PointwiseError<E>>>()?;
            let affected = lab
                .as_ref()
                .map(|edit| edit.publish(false))
                .transpose()
                .map_err(history_error)?;

            if let Some(outer) = &outer {
                outer.set_node_quiet(Some(node))?;
            } else {
                self.replace_document_branch(Some(node), false)
                    .map_err(history_error)?;
            }
            for (transfer, (wrapper, destination)) in transfers.iter().zip(prepared) {
                if let Some(source) = &transfer.from {
                    source.set_node_quiet(None)?;
                }
                transfer.node.promote_restored_branch();
                destination.restore_wrapper(Some(wrapper));
                transfer.to.set_node_quiet(Some(&transfer.node))?;
            }
            for (slot, auxiliary) in auxiliary {
                // GraphEdit's anchor must continue to sponsor these nodes
                // after their physical branch occurrences are hidden by undo.
                self.sponsor.merge(&auxiliary.sponsor);
                slot.set_node(Some(auxiliary));
            }
            if let Some(outer) = &outer {
                let parent = outer.parent();
                if parent.is_filter() {
                    // A real replacement makes this former library filter an
                    // ordinary texture. Include removal of its other sample
                    // inputs in the same after-state captured for undo.
                    parent.leave_filter_mode_quiet(Some(outer))?;
                    parent.clear_mode_subtree(&mut BTreeSet::new())?;
                }
            }
            let after = GraphLayout::read(&owners, &branches).map_err(history_error)?;
            if let Some(order) = &order {
                order.publish(false);
            }
            history
                .record_graph_edit(
                    self.history_target(),
                    "Apply Overview Preset".encode_utf16().collect(),
                    Box::new(GraphEdit {
                        anchor: self.core.anchor.clone(),
                        before,
                        after,
                        order,
                        preview_sample: None,
                        lab,
                    }),
                )
                .map_err(history_error)?;
            for transfer in transfers {
                node.notify_slot_change(&transfer.to)?;
            }
            if let Some(outer) = &outer {
                outer.parent().notify_slot_change(outer)?;
            }
            affected
                .map_or(Ok(()), AffectedProperties::notify)
                .map_err(history_error)
        })();
        staged.release_document_binding();
        applied
    }
}
