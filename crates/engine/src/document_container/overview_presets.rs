//! Apply detached library documents to a retained Overview stage.
use super::*;
use super::{environment_adjusters::graph_nodes, lab_preset_scope::LabProjection};
use crate::{PointwiseHistorySlot, PointwiseSlot};
use std::collections::BTreeSet;
use std::sync::atomic::Ordering;

/// The graph change selected by an Overview preset action.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OverviewPresetMode {
    ReplaceTexture,
    AppendFilter,
    ReplaceFilter,
}

pub(crate) struct PresetInputMove<E> {
    pub(crate) from: Option<PointwiseSlot<E>>,
    pub(crate) to: PointwiseSlot<E>,
    pub(crate) node: PointwiseNode<E>,
}

impl<E: 'static> Gtx375GraphDocument<E> {
    /// Apply a freshly loaded preset to an occurrence on the specified branch's
    /// primary chain. `None` denotes the branch output.
    ///
    /// A filter insertion wraps the selected stage. Replacing a filter keeps
    /// its occupied inputs at compatible input indices. Auxiliary preset roots
    /// retain their authored offsets. This is one edit in a bound document.
    pub fn apply_overview_preset(
        &mut self,
        branch_index: usize,
        slot: Option<&PointwiseSlot<E>>,
        preset: Self,
        mode: OverviewPresetMode,
    ) -> Result<PointwiseNode<E>, GtxGraphError<E>> {
        let before = self.branches().into_owned();
        let branch = before.get(branch_index).ok_or(GtxGraphError::BranchIndex)?;
        if let Some(slot) = slot
            && !primary_chain_contains(branch.clone(), slot)?
        {
            return Err(GtxGraphError::BranchIndex);
        }
        let former = match slot {
            Some(slot) => slot.node(),
            None => branch.clone(),
        }
        .filter(|node| !node.is_default_input());
        let Branches::Unbound(preset_roots) = preset.branches else {
            return Err(unsupported("Overview needs a detached preset document"));
        };
        let mut roots = preset_roots.into_iter();
        let node = roots
            .next()
            .flatten()
            .ok_or_else(|| unsupported("Overview preset has no output"))?;
        let auxiliary = roots
            .zip(preset.positions)
            .filter_map(|(node, position)| node.map(|node| (node, position)))
            .collect::<Vec<_>>();
        count_i32(before.len() + auxiliary.len())?;
        if auxiliary
            .iter()
            .any(|(_, position)| position.iter().any(|v| !v.is_finite()))
        {
            return Err(unsupported(
                "Overview preset has an invalid branch position",
            ));
        }
        let imported_roots = std::iter::once(Some(node.clone()))
            .chain(auxiliary.iter().map(|(root, _)| Some(root.clone())))
            .collect::<Vec<_>>();
        let existing = graph_nodes(&before)
            .map(|node| node.occurrence_id())
            .collect::<BTreeSet<_>>();
        if graph_nodes(&imported_roots).any(|root| {
            existing.contains(&root.occurrence_id())
                || root
                    .texture_node_logic()
                    .and_then(|logic| logic.slot())
                    .is_some()
        }) {
            return Err(unsupported(
                "Overview preset must own fresh graph occurrences",
            ));
        }
        if matches!(self.branches, Branches::Bound { .. }) {
            PointwiseHistorySlot::validate_trees(imported_roots.iter().flatten())
                .map_err(GtxGraphError::History)?;
        }
        // A filter library document has synthetic preview inputs and a
        // filter-mode flag. Its installed stage is an ordinary texture node.
        if node.is_filter() {
            let first = node.slot(0).map_err(slot_edit_error)?;
            node.leave_filter_mode(&first).map_err(slot_edit_error)?;
            if first.node().is_some_and(|sample| sample.is_default_input()) {
                first.set_node_quiet(None).map_err(slot_edit_error)?;
            }
        }
        let transfers = preset_input_moves(&node, former, mode)?;
        node.set_document_preview_samples(&self.preview_samples);
        for (extra, _) in &auxiliary {
            extra.set_document_preview_samples(&self.preview_samples);
        }

        let mut projected = before.clone();
        if slot.is_none() {
            projected[branch_index] = Some(node.clone());
        }
        projected.extend(auxiliary.iter().map(|(root, _)| Some(root.clone())));
        let projection = LabProjection::read(&projected, None, None, |candidate| {
            if slot.is_some_and(|slot| slot.same_reference(candidate)) {
                Some(node.clone())
            } else if let Some(transfer) = transfers
                .iter()
                .find(|edit| edit.to.same_reference(candidate))
            {
                Some(transfer.node.clone())
            } else {
                candidate.node()
            }
        });
        let (incoming, reused) = self.stage_overview_catalog(preset.catalog);
        let staged = (|| {
            if reused {
                // Compatible document controls keep their established values
                // and drive new receivers immediately, including after reopen.
                numeric_adjusters::restore_lab_values(&imported_roots, &self.catalog, false)?;
            }
            self.stage_lab_layout(&before, &projection)
        })();
        // Reconciliation resolves incoming controls against a projected
        // catalog, but only the actual graph edit may publish them.
        for marker in &incoming {
            marker.store(false, Ordering::Release);
        }
        let lab = LabLayoutEdit::include_preset_controls(staged?, incoming);

        let bound_target = match (&self.branches, slot) {
            (Branches::Bound { .. }, Some(slot)) => {
                Some(PointwiseHistorySlot::overview_slot(slot).map_err(slot_edit_error)?)
            }
            (Branches::Bound { .. }, None) => Some(
                self.history_slot(branch_index)
                    .ok_or(GtxGraphError::BranchIndex)?,
            ),
            (Branches::Unbound(_), _) => None,
        };
        match &mut self.branches {
            Branches::Unbound(branches) => {
                reserve(branches, auxiliary.len())?;
                reserve(&mut self.positions, auxiliary.len())?;
                let affected = lab
                    .as_ref()
                    .map(|edit| edit.publish(false))
                    .transpose()
                    .map_err(GtxGraphError::History)?;
                if let Some(slot) = slot {
                    slot.set_node_quiet(Some(&node)).map_err(slot_edit_error)?;
                } else {
                    branches[branch_index] = Some(node.clone());
                }
                for transfer in &transfers {
                    if let Some(source) = &transfer.from {
                        source.set_node_quiet(None).map_err(slot_edit_error)?;
                    }
                    transfer
                        .to
                        .set_node_quiet(Some(&transfer.node))
                        .map_err(slot_edit_error)?;
                }
                for (extra, position) in auxiliary {
                    branches.push(Some(extra));
                    self.positions.push(position);
                }
                for transfer in &transfers {
                    transfer
                        .to
                        .notify_overview_input()
                        .map_err(slot_edit_error)?;
                }
                if let Some(input) = slot {
                    input.notify_overview_input().map_err(slot_edit_error)?;
                }
                if let Some(affected) = affected {
                    affected.notify().map_err(GtxGraphError::History)?;
                }
            }
            Branches::Bound {
                slots,
                history,
                order,
            } => {
                reserve(slots, auxiliary.len())?;
                let previous = slots.len();
                {
                    let mut layout = BranchOrder::lock(order);
                    reserve(&mut layout.positions, auxiliary.len())?;
                    layout
                        .positions
                        .extend(auxiliary.iter().map(|(_, position)| *position));
                }
                let ordering = (!auxiliary.is_empty()).then(|| {
                    BranchOrderChange::new(order, |active| {
                        active.extend(previous..previous + auxiliary.len())
                    })
                });
                let staged = auxiliary
                    .into_iter()
                    .map(|(extra, _)| {
                        let slot = PointwiseHistorySlot::bind(&node, history, Vec::new());
                        slots.push(slot.clone());
                        (slot, extra)
                    })
                    .collect::<Vec<_>>();
                let result = bound_target
                    .expect("bound document has its editor target")
                    .edit_overview_preset(&node, &transfers, &staged, ordering, lab);
                if result.is_err() {
                    let mut layout = BranchOrder::lock(order);
                    if !layout.slots.contains(&previous) {
                        layout.positions.truncate(previous);
                        drop(layout);
                        for slot in slots.drain(previous..) {
                            slot.release_document_binding();
                        }
                    }
                }
                result.map_err(slot_edit_error)?;
            }
        }
        Ok(node)
    }

    fn stage_overview_catalog(
        &mut self,
        catalog: GraphCatalog,
    ) -> (Vec<Arc<std::sync::atomic::AtomicBool>>, bool) {
        let mut markers = Vec::new();
        let mut reused = false;
        for entry in catalog.into_active() {
            let metadata = entry.metadata();
            if self.catalog.entries().any(|(current, _)| {
                let current = current.metadata();
                current.type_tag == metadata.type_tag
                    && current.group_id == metadata.group_id
                    && current.name == metadata.name
            }) {
                reused = true;
                continue;
            }
            let marker = self.catalog.stage(entry);
            marker.store(true, Ordering::Release);
            markers.push(marker);
        }
        (markers, reused)
    }
}

fn unsupported<E>(feature: &'static str) -> GtxGraphError<E> {
    GtxGraphError::Node(NodeWireError::UnsupportedCapability(feature))
}

fn overview_inputs<E: 'static>(
    node: &PointwiseNode<E>,
) -> Result<Vec<PointwiseSlot<E>>, GtxGraphError<E>> {
    if node.contents().is_empty() {
        Ok(node.slots())
    } else {
        node.reference_inputs().map_err(slot_edit_error)
    }
}

fn primary_chain_contains<E: 'static>(
    mut current: Option<PointwiseNode<E>>,
    target: &PointwiseSlot<E>,
) -> Result<bool, GtxGraphError<E>> {
    let mut seen = BTreeSet::new();
    while let Some(node) = current.filter(|node| !node.is_default_input()) {
        if !seen.insert(node.occurrence_id()) {
            break;
        }
        let Some(input) = overview_inputs(&node)?.into_iter().next() else {
            break;
        };
        if input.same_reference(target) {
            return Ok(true);
        }
        current = input.node();
    }
    Ok(false)
}

fn preset_input_moves<E: 'static>(
    node: &PointwiseNode<E>,
    former: Option<PointwiseNode<E>>,
    mode: OverviewPresetMode,
) -> Result<Vec<PresetInputMove<E>>, GtxGraphError<E>> {
    if mode == OverviewPresetMode::ReplaceTexture {
        return Ok(Vec::new());
    }
    let former = former.ok_or_else(|| unsupported("Choose a connected Overview stage"))?;
    let inputs = overview_inputs(node)?;
    if inputs.is_empty() {
        return Err(unsupported("Overview filter preset has no input"));
    }
    Ok(match mode {
        OverviewPresetMode::AppendFilter => {
            vec![PresetInputMove {
                from: None,
                to: inputs.into_iter().next().unwrap(),
                node: former,
            }]
        }
        OverviewPresetMode::ReplaceFilter => overview_inputs(&former)?
            .into_iter()
            .zip(inputs)
            .filter_map(|(from, to)| {
                let node = from.node().filter(|child| !child.is_default_input())?;
                Some(PresetInputMove {
                    from: Some(from),
                    to,
                    node,
                })
            })
            .collect(),
        OverviewPresetMode::ReplaceTexture => unreachable!(),
    })
}

#[cfg(test)]
mod tests {
    use super::super::environment_catalog_tests::LabTestHistory;
    use super::*;
    use crate::{History, PointwiseFilter, PointwiseKind};

    fn synthesis_preset(name: &str) -> (Gtx375GraphDocument<()>, History<()>, PointwiseNode<()>) {
        let install =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../genetica/Presets");
        let load = |path: &str| {
            Gtx375GraphDocument::load(&std::fs::read(install.join(path)).unwrap()).unwrap()
        };
        let mut document = Gtx375GraphDocument::new(vec![None]).unwrap();
        let history = document
            .bind_property_history(Arc::new(LabTestHistory))
            .unwrap();
        history.set_recording(true);
        let leather = document
            .apply_overview_preset(
                0,
                None,
                load("Texture/Miscellaneous/Leather.gtx"),
                OverviewPresetMode::ReplaceTexture,
            )
            .unwrap();
        let root = document
            .apply_overview_preset(
                0,
                None,
                load(&format!("Filter/Texture Effect/Print & Emboss/{name}.gtx")),
                OverviewPresetMode::AppendFilter,
            )
            .unwrap();
        assert!(
            root.slot(0)
                .unwrap()
                .node()
                .unwrap()
                .same_reference(&leather)
        );
        (document, history, synthesis_node(&[Some(root)]))
    }

    fn synthesis_node(roots: &[Option<PointwiseNode<()>>]) -> PointwiseNode<()> {
        graph_nodes(roots)
            .find(|node| node.kind() == Some(PointwiseKind::Synthesis))
            .unwrap()
    }

    fn synthesis_pixels(node: &PointwiseNode<()>, preview: bool) -> Vec<u32> {
        let snapshot = crate::RenderCaptureSession::new()
            .capture_node(node, 0.)
            .unwrap();
        let mut capacity = |_| Ok(true);
        let mut preflight = |_, _| Ok(());
        let library = crate::EnvironmentResourceLibrary::default();
        let mut effects = crate::PointwiseRenderEffects::new(&mut capacity, &mut preflight)
            .with_environment_library(Some(&library));
        let geometry = crate::ImageGeometry::new(64, 64);
        let execution = crate::ProceduralExecution::CallingThread;
        let result = if preview {
            snapshot.render_preview_with_effects(geometry, execution, &mut effects)
        } else {
            snapshot.render_export_with_effects(
                geometry,
                execution,
                crate::PointwiseRenderOptions::new(false, false),
                &mut effects,
            )
        }
        .unwrap();
        assert!(
            result.caught_errors().is_empty(),
            "{:?}",
            result.caught_errors()
        );
        let image = result.image().unwrap();
        (0..4)
            .flat_map(|channel| {
                let plane = image.channel(channel, true).unwrap();
                (0..4096).map(move |index| {
                    plane
                        .read(if plane.len() == 1 { 0 } else { index })
                        .unwrap()
                        .to_bits()
                })
            })
            .collect()
    }

    #[test]
    #[ignore = "requires the installed Genetica preset libraries"]
    fn overview_cracked_photo_frequency_controls_update_the_synthesis_stage() {
        let (document, history, synthesis) = synthesis_preset("Cracked Photo");
        let before = synthesis_pixels(&synthesis, false);
        assert_eq!(synthesis_pixels(&synthesis, true), before);
        for (name, authored) in [("Tears", 0.), ("Big Cracks", 30.), ("Fine Cracks", 30.)] {
            let controls = document.numeric_lab_adjusters(Some(&synthesis));
            let control = controls
                .iter()
                .find(|control| control.name() == name)
                .unwrap();
            assert_eq!(control.value(), crate::LabNumericValue::Number(authored));
            assert!(
                document
                    .edit_numeric_lab_adjuster(
                        Some(&synthesis),
                        control,
                        crate::LabNumericValue::Number(100.)
                    )
                    .unwrap()
            );
            let changed = synthesis_pixels(&synthesis, false);
            assert_ne!(changed, before, "{name}");
            assert_eq!(synthesis_pixels(&synthesis, true), changed);
            history.undo().unwrap();
            assert_eq!(synthesis_pixels(&synthesis, false), before);
            history.redo().unwrap();
            assert_eq!(synthesis_pixels(&synthesis, false), changed);
            history.undo().unwrap();
        }
    }

    #[test]
    #[ignore = "requires the installed Genetica preset libraries"]
    fn overview_cracked_photo_renames_and_merges_synthesis_frequency_links() {
        for shared in [false, true] {
            let (mut document, history, synthesis) = synthesis_preset("Cracked Photo");
            if shared {
                document
                    .set_synthesis_adjuster(
                        &synthesis.filter().unwrap(),
                        crate::SynthesisAdjusterLink::Create("Cracked texture".into()),
                    )
                    .unwrap();
            }
            let metadata = |document: &Gtx375GraphDocument<()>, name: &str| {
                document
                    .lab_adjuster_metadata(Some(&synthesis))
                    .into_iter()
                    .find(|control| control.name() == name)
            };
            let numeric = |document: &Gtx375GraphDocument<()>, name: &str| {
                document
                    .numeric_lab_adjusters(Some(&synthesis))
                    .into_iter()
                    .find(|control| control.name() == name)
                    .unwrap()
            };

            let initial = synthesis_pixels(&synthesis, false);
            let tears = metadata(&document, "Tears").unwrap();
            document
                .edit_lab_adjuster_metadata(Some(&synthesis), &tears, "Patch Tears", tears.order())
                .unwrap();
            assert!(metadata(&document, "Tears").is_none());
            assert_eq!(synthesis_pixels(&synthesis, false), initial);

            let renamed = numeric(&document, "Patch Tears");
            let destination = numeric(&document, "Big Cracks");
            document
                .edit_numeric_lab_adjuster(Some(&synthesis), &renamed, destination.value())
                .unwrap();
            let merged = synthesis_pixels(&synthesis, false);
            assert_ne!(merged, initial);
            history.undo().unwrap();
            assert_eq!(synthesis_pixels(&synthesis, false), initial);

            let renamed = metadata(&document, "Patch Tears").unwrap();
            document
                .edit_lab_adjuster_metadata(
                    Some(&synthesis),
                    &renamed,
                    "Big Cracks",
                    renamed.order(),
                )
                .unwrap();
            assert!(metadata(&document, "Patch Tears").is_none());
            assert_eq!(synthesis_pixels(&synthesis, true), merged);

            history.undo().unwrap();
            assert!(metadata(&document, "Patch Tears").is_some());
            assert_eq!(synthesis_pixels(&synthesis, false), initial);
            history.undo().unwrap();
            assert!(metadata(&document, "Tears").is_some());
            history.redo().unwrap();
            history.redo().unwrap();
            assert_eq!(synthesis_pixels(&synthesis, false), merged);

            let final_destination = numeric(&document, "Big Cracks");
            document
                .edit_numeric_lab_adjuster(
                    Some(&synthesis),
                    &final_destination,
                    crate::LabNumericValue::Number(100.),
                )
                .unwrap();
            let final_pixels = synthesis_pixels(&synthesis, false);
            assert_ne!(final_pixels, merged);
            for native in [false, true] {
                let mut saved = BinaryWriter::default();
                let reopened = if native {
                    document.save_project(&mut saved).unwrap();
                    Gtx375GraphDocument::<()>::load_project(
                        saved.bytes(),
                        crate::NodeWireReadPolicy::new(128, 8192),
                    )
                    .unwrap()
                } else {
                    document.save(&mut saved).unwrap();
                    Gtx375GraphDocument::<()>::load(saved.bytes()).unwrap()
                };
                let loaded = synthesis_node(&reopened.branches());
                assert_eq!(synthesis_pixels(&loaded, false), final_pixels);
            }
        }
    }

    #[test]
    #[ignore = "requires the installed Genetica preset libraries"]
    fn overview_crumpled_paper_jitter_follows_catch_adjusters_and_reopens() {
        let (mut document, history, synthesis) = synthesis_preset("Crumpled Paper");
        let group_name = "Crumpled Paper".encode_utf16().collect::<Vec<_>>();
        let group = std::iter::successors(synthesis.parent(), PointwiseNode::parent)
            .filter_map(|node| node.filter())
            .find(|filter| {
                filter.kind() == PointwiseKind::Group
                    && filter.parameter(2).unwrap().text().unwrap().as_deref()
                        == Some(group_name.as_slice())
            })
            .unwrap();
        let before = synthesis_pixels(&synthesis, false);
        assert!(
            document
                .set_group_catch_adjusters(&group, true, None)
                .unwrap()
        );
        assert_eq!(synthesis_pixels(&synthesis, false), before);
        let irr = |document: &Gtx375GraphDocument<()>, node: Option<&PointwiseNode<()>>| {
            document
                .numeric_lab_adjusters(node)
                .into_iter()
                .find(|control| control.name() == "Irregularity")
        };
        let moved = irr(&document, Some(&synthesis)).unwrap();
        assert!(irr(&document, None).is_none());
        assert_eq!(moved.value(), crate::LabNumericValue::Number(100.));
        assert!(
            document
                .edit_numeric_lab_adjuster(
                    Some(&synthesis),
                    &moved,
                    crate::LabNumericValue::Number(0.)
                )
                .unwrap()
        );
        let changed = synthesis_pixels(&synthesis, false);
        assert_ne!(changed, before);
        assert_eq!(synthesis_pixels(&synthesis, true), changed);

        for native in [true, false] {
            let mut wire = BinaryWriter::default();
            let reopened = if native {
                document.save_project(&mut wire).unwrap();
                Gtx375GraphDocument::<()>::load_project(
                    wire.bytes(),
                    crate::NodeWireReadPolicy::new(128, 8192),
                )
                .unwrap()
            } else {
                document.save(&mut wire).unwrap();
                Gtx375GraphDocument::<()>::load(wire.bytes()).unwrap()
            };
            let loaded = synthesis_node(&reopened.branches());
            assert_eq!(
                irr(&reopened, Some(&loaded)).unwrap().value(),
                crate::LabNumericValue::Number(0.)
            );
            assert_eq!(synthesis_pixels(&loaded, false), changed);
        }
        history.undo().unwrap();
        assert_eq!(synthesis_pixels(&synthesis, false), before);
        history.undo().unwrap();
        assert_eq!(
            irr(&document, None).unwrap().value(),
            crate::LabNumericValue::Number(100.)
        );
        assert_eq!(synthesis_pixels(&synthesis, false), before);
        history.redo().unwrap();
        history.redo().unwrap();
        assert_eq!(synthesis_pixels(&synthesis, false), changed);
    }

    #[test]
    fn overview_preset_transfers_the_primary_chain_auxiliary_roots_and_history() {
        let previous = PointwiseNode::<()>::solid_color().unwrap();
        let mut document = Gtx375GraphDocument::new(vec![Some(previous.clone())]).unwrap();
        let history = document
            .bind_property_history(Arc::new(LabTestHistory))
            .unwrap();
        history.set_recording(true);

        let texture = PointwiseNode::solid_color().unwrap();
        let extra = PointwiseNode::solid_color().unwrap();
        let id = [11; 16];
        let mut preset =
            Gtx375GraphDocument::new(vec![Some(texture.clone()), Some(extra.clone())]).unwrap();
        preset.positions_mut()[0] = [-33., 18.];
        let definition = ValueAdjusterRecord::new(
            ValueAdjusterKind::Number,
            Some("Detail".encode_utf16().collect()),
            id,
        );
        preset.catalog = GraphCatalog::new(vec![GtxCatalogRecord::Value(Box::new(definition))]);

        let installed = document
            .apply_overview_preset(0, None, preset, OverviewPresetMode::ReplaceTexture)
            .unwrap();
        assert!(installed.same_reference(&texture));
        assert!(document.branch(1).flatten().unwrap().same_reference(&extra));
        assert_eq!(document.positions().as_ref(), &[[-33., 18.]]);
        assert_eq!(document.catalog().unwrap()[0].metadata().group_id, id);
        assert_eq!(history.commands().len(), 1);
        history.undo().unwrap();
        assert!(
            document
                .branch(0)
                .flatten()
                .unwrap()
                .same_reference(&previous)
        );
        assert_eq!(document.branches().len(), 1);
        assert!(document.catalog().is_none());
        history.redo().unwrap();
        assert!(
            document
                .branch(0)
                .flatten()
                .unwrap()
                .same_reference(&texture)
        );
        assert!(document.branch(1).flatten().unwrap().same_reference(&extra));
        assert_eq!(document.catalog().unwrap()[0].metadata().group_id, id);

        let filter = PointwiseNode::invert();
        let preset = Gtx375GraphDocument::new(vec![Some(filter.clone())]).unwrap();
        document
            .apply_overview_preset(0, None, preset, OverviewPresetMode::AppendFilter)
            .unwrap();
        assert!(
            document
                .branch(0)
                .flatten()
                .unwrap()
                .same_reference(&filter)
        );
        assert!(
            filter
                .slot(0)
                .unwrap()
                .node()
                .unwrap()
                .same_reference(&texture)
        );

        let replacement = PointwiseFilter::new(PointwiseKind::ChangeBci)
            .unwrap()
            .node();
        let preset = Gtx375GraphDocument::new(vec![Some(replacement.clone())]).unwrap();
        document
            .apply_overview_preset(0, None, preset, OverviewPresetMode::ReplaceFilter)
            .unwrap();
        assert!(
            document
                .branch(0)
                .flatten()
                .unwrap()
                .same_reference(&replacement)
        );
        assert!(
            replacement
                .slot(0)
                .unwrap()
                .node()
                .unwrap()
                .same_reference(&texture)
        );
        assert!(filter.slot(0).unwrap().node().is_none());
        assert_eq!(history.commands().len(), 3);

        let base_input = replacement.slot(0).unwrap();
        let new_texture = PointwiseNode::solid_color().unwrap();
        let preset = Gtx375GraphDocument::new(vec![Some(new_texture.clone())]).unwrap();
        document
            .apply_overview_preset(
                0,
                Some(&base_input),
                preset,
                OverviewPresetMode::ReplaceTexture,
            )
            .unwrap();
        assert!(
            document
                .branch(0)
                .flatten()
                .unwrap()
                .same_reference(&replacement)
        );
        assert!(base_input.node().unwrap().same_reference(&new_texture));
        assert_eq!(history.commands().len(), 4);

        history.undo().unwrap();
        assert!(base_input.node().unwrap().same_reference(&texture));
        history.undo().unwrap();
        assert!(
            document
                .branch(0)
                .flatten()
                .unwrap()
                .same_reference(&filter)
        );
        assert!(
            filter
                .slot(0)
                .unwrap()
                .node()
                .unwrap()
                .same_reference(&texture)
        );
        history.undo().unwrap();
        assert!(
            document
                .branch(0)
                .flatten()
                .unwrap()
                .same_reference(&texture)
        );
        history.redo().unwrap();
        history.redo().unwrap();
        history.redo().unwrap();
        assert!(
            document
                .branch(0)
                .flatten()
                .unwrap()
                .same_reference(&replacement)
        );
        assert!(base_input.node().unwrap().same_reference(&new_texture));
    }

    #[test]
    #[ignore = "requires the installed Genetica preset libraries"]
    fn overview_preset_installed_leather_and_emboss_render_and_reopen() {
        let install =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../genetica/Presets");
        let load = |path| {
            let bytes = std::fs::read(install.join(path)).unwrap();
            Gtx375GraphDocument::<()>::load(&bytes).unwrap()
        };
        let mut document = Gtx375GraphDocument::new(vec![None]).unwrap();
        let history = document
            .bind_property_history(Arc::new(LabTestHistory))
            .unwrap();
        history.set_recording(true);
        let leather = document
            .apply_overview_preset(
                0,
                None,
                load("Texture/Miscellaneous/Leather.gtx"),
                OverviewPresetMode::ReplaceTexture,
            )
            .unwrap();
        let emboss = document
            .apply_overview_preset(
                0,
                None,
                load("Filter/Texture Effect/Print & Emboss/Emboss.gtx"),
                OverviewPresetMode::AppendFilter,
            )
            .unwrap();
        assert!(!emboss.is_filter());
        assert!(
            emboss
                .slot(0)
                .unwrap()
                .node()
                .unwrap()
                .same_reference(&leather)
        );
        history.undo().unwrap();
        assert!(
            document
                .branch(0)
                .flatten()
                .unwrap()
                .same_reference(&leather)
        );
        history.redo().unwrap();
        assert!(
            document
                .branch(0)
                .flatten()
                .unwrap()
                .same_reference(&emboss)
        );
        let mut wire = BinaryWriter::default();
        document.save_project(&mut wire).unwrap();
        let reopened = Gtx375GraphDocument::<()>::load_project(
            wire.bytes(),
            crate::NodeWireReadPolicy::new(128, 8192),
        )
        .unwrap();
        let reopened = reopened.branch(0).flatten().unwrap();
        assert_eq!(reopened.identity_name(), emboss.identity_name());
        assert_eq!(
            reopened.slot(0).unwrap().node().unwrap().identity_name(),
            leather.identity_name()
        );
        let render = |root: &PointwiseNode<()>| {
            let snapshot = crate::RenderCaptureSession::new()
                .capture_node(root, 0.)
                .unwrap();
            let mut capacity = |_| Ok(true);
            let mut preflight = |_, _| Ok(());
            let mut effects = crate::PointwiseRenderEffects::new(&mut capacity, &mut preflight);
            let rendered = snapshot
                .render_export_with_effects(
                    crate::ImageGeometry::new(16, 16),
                    crate::ProceduralExecution::CallingThread,
                    crate::PointwiseRenderOptions::new(false, false),
                    &mut effects,
                )
                .unwrap();
            assert!(
                rendered.caught_errors().is_empty(),
                "{:?}",
                rendered.caught_errors()
            );
            let image = rendered.image().unwrap();
            (0..4)
                .flat_map(|channel| {
                    let channel = image.channel(channel, true).unwrap();
                    (0..256).map(move |index| {
                        channel
                            .read(if channel.len() == 1 { 0 } else { index })
                            .unwrap()
                    })
                })
                .collect::<Vec<_>>()
        };
        let rendered = render(&emboss);
        assert!(rendered.iter().all(|pixel| pixel.is_finite()));
        assert_eq!(render(&reopened), rendered);

        let mut filter_document = load("Filter/Texture Effect/Print & Emboss/Emboss.gtx");
        let filter = filter_document.branch(0).flatten().unwrap();
        let input = filter.slot(0).unwrap();
        assert!(filter.is_filter());
        assert!(input.node().unwrap().is_default_input());
        let history = filter_document
            .bind_property_history(Arc::new(LabTestHistory))
            .unwrap();
        history.set_recording(true);
        let texture = filter_document
            .apply_overview_preset(
                0,
                Some(&input),
                load("Texture/Miscellaneous/Leather.gtx"),
                OverviewPresetMode::ReplaceTexture,
            )
            .unwrap();
        assert!(!filter.is_filter());
        assert!(input.node().unwrap().same_reference(&texture));
        history.undo().unwrap();
        assert!(filter.is_filter());
        assert!(filter.slot(0).unwrap().node().unwrap().is_default_input());
        history.redo().unwrap();
        assert!(!filter.is_filter());
        assert!(
            filter
                .slot(0)
                .unwrap()
                .node()
                .unwrap()
                .same_reference(&texture)
        );
    }
}
