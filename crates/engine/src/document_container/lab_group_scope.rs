use super::curve_adjusters::{self, CurveChange};
use super::environment_adjusters::{
    EnvironmentAdjusterBatch, enclosing_adjuster_group, enclosing_adjuster_group_with,
    environment_adjustment_key, graph_nodes,
};
use super::lab_adjuster_metadata::{LabLinkEdit, compatible};
use super::lab_preset_scope::{LabProjection, PresetRefresh};
use super::lab_presets::AffectedProperties;
use super::numeric_adjusters::{NumericCatalogUpdate, map_retained_number};
use super::style_synthesis_numbers::{link_key, matches_key};
use super::synthesis_adjusters::{RetiredSynthesisDefinitions, SynthesisReceiverEdit};
use super::*;
use crate::document_container::catalog::NumericCatalogValue;
use crate::history::HistoryGraphEdit;
use crate::pointwise::numeric_adjustment::NumericReceiver;
use crate::pointwise::property::{
    NumericAdjustment, NumericPropertyState, SynthesisLinkState, WeakPointwiseParameter,
};
use crate::pointwise::reference_adjustment::LabReferenceEdit;
use crate::pointwise::{EnvironmentPaintEdit, SynthesisPaint};
use crate::{
    HistoryDirection, HistoryError, PointwiseFilter, PointwiseKind, PointwiseParameter,
    PointwiseParameterValue, PointwiseSlot, ReferenceAdjusterKind, ReferenceAdjusterPayload,
    ReferenceAdjusterRecord, SharedCurve, SynthesisCatalogValue, SynthesisRecord,
    ValueAdjusterPayload, ValueAdjusterRecord,
};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};

#[path = "lab_group_environment.rs"]
mod environment;
use environment::{copy_gradient, find_control, gradient_error};

pub(super) enum LabAssignment<'a, E> {
    Branch(usize),
    Slot(&'a PointwiseSlot<E>),
}

impl<E: 'static> Gtx375GraphDocument<E> {
    pub(super) fn stage_lab_assignment(
        &mut self,
        destination: LabAssignment<'_, E>,
        node: Option<&PointwiseNode<E>>,
    ) -> Result<Option<LabLayoutEdit<E>>, GtxGraphError<E>> {
        let before = self.branches().into_owned();
        let mut roots = before.clone();
        if let LabAssignment::Slot(slot) = destination {
            let owner = slot.parent();
            if !owner
                .slots()
                .iter()
                .chain(&owner.contents())
                .any(|input| input.same_reference(slot))
                || !graph_nodes(&before).any(|candidate| candidate.same_reference(&owner))
            {
                return Err(node_error(PointwiseError::InvalidOperation));
            }
            let mut current = Some(owner);
            let mut visited = Vec::<PointwiseNode<E>>::new();
            while let Some(parent) = current {
                if node.is_some_and(|node| parent.same_reference(node))
                    || visited
                        .iter()
                        .any(|previous| parent.same_reference(previous))
                {
                    return Err(node_error(PointwiseError::UnsupportedCapability(
                        "cyclic property history input",
                    )));
                }
                current = parent.parent();
                visited.push(parent);
            }
        }
        let source = node.and_then(PointwiseNode::connected_occurrence);
        if let Some(node) = node {
            for root in &mut roots {
                if root.as_ref().is_some_and(|root| root.same_reference(node)) {
                    *root = None;
                }
            }
        }
        if let LabAssignment::Branch(index) = destination {
            *roots.get_mut(index).ok_or(GtxGraphError::BranchIndex)? = node.cloned();
        }
        let projection = LabProjection::read(&roots, None, None, |slot| {
            if let LabAssignment::Slot(target) = destination
                && target.same_reference(slot)
            {
                node.cloned()
            } else if source
                .as_ref()
                .is_some_and(|source| source.same_reference(slot))
            {
                None
            } else {
                slot.node()
            }
        });
        self.stage_lab_layout(&before, &projection)
    }

    pub(super) fn stage_lab_layout(
        &mut self,
        before: &[Option<PointwiseNode<E>>],
        projection: &LabProjection<E>,
    ) -> Result<Option<LabLayoutEdit<E>>, GtxGraphError<E>> {
        let mut controls = ControlMigration::new(before, &self.catalog);
        controls.record_environment_links(&self.catalog, before);
        controls.record_synthesis_lab_links(&self.catalog, before);
        let mut synthesis_receivers = Vec::new();
        for node in graph_nodes(before) {
            for adjustment in NumericReceiver::for_node(&node)
                .into_iter()
                .filter_map(|receiver| receiver.retained_adjustment())
            {
                if adjustment.is_active().map_err(node_error)? {
                    for (_, marker) in attached_entries(&self.catalog, &adjustment) {
                        controls.leave(marker);
                    }
                }
            }
            if let Some(filter) = node.filter() {
                if let Some(state) = filter.synthesis_link_state()
                    && let Some((entry, marker)) = attached_synthesis(&self.catalog, &state)
                {
                    controls.leave(marker);
                    synthesis_receivers.push((node.occurrence_id(), entry, marker));
                }
                filter.visit_synthesis_material_paints(|paint| {
                    if let Some((entry, marker)) = attached_synthesis_paint(&self.catalog, paint) {
                        controls.leave(marker);
                        synthesis_receivers.push((node.occurrence_id(), entry, marker));
                    }
                });
                filter.visit_environment_paint_adjusters(|key| {
                    if let Some((_, marker)) = attached_environment(&self.catalog, key) {
                        controls.leave(marker);
                    }
                });
            }
        }
        let retired_synthesis =
            RetiredSynthesisDefinitions::removed_by_layout(synthesis_receivers, &projection.nodes);
        let mut links = ScopeLinks::new();
        let mut seen = Vec::<NumericReceiver<E>>::new();
        for (node, section) in &projection.nodes {
            for receiver in NumericReceiver::for_node(node) {
                let Some(adjustment) = receiver.retained_adjustment() else {
                    continue;
                };
                if matches!(adjustment.property_type(), 6 | 7)
                    || !adjustment.is_active().map_err(node_error)?
                    || seen.iter().any(|other| other.same_reference(&receiver))
                {
                    continue;
                }
                if let NumericReceiver::Filter(property) = &receiver
                    && matches!(property.raw_value(), PointwiseParameterValue::Curve(_))
                {
                    links.curves.extend(controls.prepare_curve(
                        &self.catalog,
                        property,
                        *section,
                    )?);
                } else if adjustment.guid() == *section {
                    controls.keep(&self.catalog, &adjustment);
                } else {
                    links.properties.push(controls.prepare(
                        &self.catalog,
                        &receiver,
                        adjustment,
                        *section,
                    )?);
                }
                seen.push(receiver);
            }
            controls.prepare_local_environment_maps(
                &self.catalog,
                node,
                Some(*section),
                &mut links.maps,
            )?;
            links.references.extend(controls.prepare_references(
                &self.catalog,
                node,
                Some(*section),
            )?);
            links.synthesis.extend(controls.prepare_synthesis(
                &self.catalog,
                node,
                *section,
                true,
                Some(&retired_synthesis),
            )?);
            controls.prepare_texture_paints(
                &self.catalog,
                node,
                Some(*section),
                Some(&retired_synthesis),
                &mut links.paints,
            )?;
        }
        controls.keep_surviving_environment_links(&self.catalog);
        controls.keep_surviving_synthesis_lab_links(&self.catalog);
        let mut retired = controls.moved;
        retired.retain(|marker| !controls.kept.contains(&marker_id(marker)));
        if links.is_empty() && retired.is_empty() {
            return Ok(None);
        }
        let projected: Vec<_> = self
            .catalog
            .entries()
            .filter_map(|(entry, marker)| {
                (!retired.iter().any(|old| Arc::ptr_eq(old, marker))).then_some(entry)
            })
            .chain(&controls.staged)
            .collect();
        let refresh = PresetRefresh::stage_projected(&mut self.presets, projection, &projected);
        retired.extend(refresh.retired);
        let mut created = refresh.added;
        created.extend(
            controls
                .staged
                .into_iter()
                .map(|entry| self.catalog.stage(entry)),
        );
        Ok(Some(LabLayoutEdit {
            links,
            retired,
            created,
        }))
    }

    /// Move a Group's content links between its own Lab section and the enclosing one.
    pub fn set_group_catch_adjusters(
        &mut self,
        group: &PointwiseFilter<E>,
        catch: bool,
        new_name: Option<&str>,
    ) -> Result<bool, GtxGraphError<E>> {
        let branches = self.branches().into_owned();
        let node = group.node();
        if group.kind() != PointwiseKind::Group
            || !graph_nodes(&branches).any(|n| n.same_reference(&node))
        {
            return Err(GtxGraphError::NumericAdjuster(
                "The Group is no longer in this document.",
            ));
        }
        let toggle = group.parameter(4).map_err(node_error)?;
        let index = i32::from(!catch);
        if toggle.index() == index {
            return Ok(false);
        }
        let (before, after) = toggle
            .prepare_numeric_adjustment(NumericAdjustment::Choice(index))
            .map_err(node_error)?;
        let name = group.parameter(2).map_err(node_error)?;
        let old_name = name.text().map_err(node_error)?;
        let name_edit = if catch && old_name.as_ref().is_none_or(Vec::is_empty) {
            let requested: Vec<u16> = new_name
                .unwrap_or_default()
                .split(' ')
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join(" ")
                .encode_utf16()
                .collect();
            if requested.is_empty() || requested.len() > 40 {
                return Err(GtxGraphError::NumericAdjuster(
                    "Enter a Group name from 1 to 40 characters.",
                ));
            }
            Some(GroupNameEdit {
                property: name.downgrade(),
                before: old_name,
                after: requested,
            })
        } else {
            None
        };
        let mut move_controls = ControlMigration::new(&branches, &self.catalog);
        move_controls.record_environment_links(&self.catalog, &branches);
        move_controls.record_synthesis_lab_links(&self.catalog, &branches);
        let mut links = ScopeLinks::new();
        let mut seen = Vec::<NumericReceiver<E>>::new();
        for owner in graph_nodes(&branches) {
            let previous = enclosing_adjuster_group(&owner).and_then(|group| group.group_guid());
            let next = enclosing_adjuster_group_with(&owner, Some((&node, catch)))
                .and_then(|group| group.group_guid());
            for receiver in NumericReceiver::for_node(&owner) {
                let Some(adjustment) = receiver.retained_adjustment() else {
                    continue;
                };
                if matches!(adjustment.property_type(), 6 | 7)
                    || !adjustment.is_active().map_err(node_error)?
                    || seen.iter().any(|other| other.same_reference(&receiver))
                {
                    continue;
                }
                if let NumericReceiver::Filter(property) = &receiver
                    && matches!(property.raw_value(), PointwiseParameterValue::Curve(_))
                {
                    let target = if previous == next {
                        adjustment.guid()
                    } else {
                        next.unwrap_or_default()
                    };
                    links.curves.extend(move_controls.prepare_curve(
                        &self.catalog,
                        property,
                        target,
                    )?);
                } else if previous != next && adjustment.guid() != next.unwrap_or_default() {
                    links.properties.push(move_controls.prepare(
                        &self.catalog,
                        &receiver,
                        adjustment,
                        next.unwrap_or_default(),
                    )?);
                } else {
                    move_controls.keep(&self.catalog, &adjustment);
                }
                seen.push(receiver);
            }
            let projected_scope = (previous != next).then_some(next.unwrap_or_default());
            move_controls.prepare_local_environment_maps(
                &self.catalog,
                &owner,
                projected_scope,
                &mut links.maps,
            )?;
            links.references.extend(move_controls.prepare_references(
                &self.catalog,
                &owner,
                projected_scope,
            )?);
            links.synthesis.extend(move_controls.prepare_synthesis(
                &self.catalog,
                &owner,
                next.unwrap_or_default(),
                previous != next,
                None,
            )?);
            move_controls.prepare_texture_paints(
                &self.catalog,
                &owner,
                projected_scope,
                None,
                &mut links.paints,
            )?;
        }
        move_controls.keep_surviving_environment_links(&self.catalog);
        move_controls.keep_surviving_synthesis_lab_links(&self.catalog);
        let mut retired = move_controls.moved;
        retired.retain(|marker| !move_controls.kept.contains(&marker_id(marker)));
        let projected: Vec<_> = self
            .catalog
            .entries()
            .filter_map(|(entry, marker)| {
                (!retired.iter().any(|removed| Arc::ptr_eq(marker, removed))).then_some(entry)
            })
            .chain(&move_controls.staged)
            .collect();
        let refresh = PresetRefresh::stage(&mut self.presets, &branches, &node, catch, &projected);
        retired.extend(refresh.retired);
        if !catch && let Some(presets) = group.group_presets() {
            retired.extend(
                presets
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .groups()
                    .iter()
                    .map(GtxPresetGroup::presence)
                    .filter(|marker| marker.load(Ordering::Acquire))
                    .cloned(),
            );
        }
        let mut created = refresh.added;
        created.extend(
            move_controls
                .staged
                .into_iter()
                .map(|entry| self.catalog.stage(entry)),
        );
        let edit = GroupScopeEdit {
            toggle: toggle.downgrade(),
            before,
            after,
            name: name_edit,
            links,
            retired,
            created,
        };
        let affected = edit
            .publish(HistoryDirection::Redo)
            .map_err(GtxGraphError::History)?;
        let label = if catch {
            "Catch Group Adjusters"
        } else {
            "Release Group Adjusters"
        };
        self.record_lab_edit(label, edit)
            .and(notify_group(affected))
            .map_err(GtxGraphError::History)?;
        Ok(true)
    }
}

fn node_error<E>(error: PointwiseError<E>) -> GtxGraphError<E> {
    GtxGraphError::Node(error.into())
}

fn marker_id(marker: &Arc<AtomicBool>) -> usize {
    Arc::as_ptr(marker) as usize
}

struct ControlMigration<E> {
    staged: Vec<GtxCatalogRecord>,
    moved: Vec<Arc<AtomicBool>>,
    kept: HashSet<usize>,
    curves: HashMap<([u8; 16], Vec<u16>), SharedCurve<E>>,
}

fn matches(metadata: &AdjusterMetadataRecord, guid: [u8; 16], name: &[u16], kind: i32) -> bool {
    metadata.group_id == guid
        && metadata.name.as_deref() == Some(name)
        && compatible(metadata.type_tag, kind)
}

fn attached_entries<'a, E>(
    catalog: &'a GraphCatalog,
    adjustment: &crate::SharedPropertyAdjustment<E>,
) -> impl Iterator<Item = (&'a GtxCatalogRecord, &'a Arc<AtomicBool>)> {
    let parts = adjustment.capture_parts();
    let names = parts.names.map(|names| names.entries()).unwrap_or_default();
    catalog.entries().filter(move |(record, _)| {
        names
            .iter()
            .take(parts.count.max(0) as usize)
            .flatten()
            .any(|name| {
                !name.is_empty()
                    && matches(record.metadata(), parts.guid, name, parts.property_type)
            })
    })
}

fn attached_synthesis<'a, E>(
    catalog: &'a GraphCatalog,
    state: &SynthesisLinkState<E>,
) -> Option<(&'a GtxCatalogRecord, &'a Arc<AtomicBool>)> {
    let parts = state.adjustment.as_ref()?.capture_parts();
    if parts.property_type != 10 || !parts.flags?.value(0).ok()? {
        return None;
    }
    let name = parts.names?.entries().into_iter().next().flatten()?;
    if name.is_empty() {
        return None;
    }
    catalog
        .entries()
        .filter_map(|(entry, marker)| {
            let GtxCatalogRecord::Synthesis { metadata, value } = entry else {
                return None;
            };
            matches(metadata, parts.guid, &name, 10).then_some((entry, marker, value))
        })
        .min_by_key(|(_, _, value)| {
            state
                .catalog
                .as_ref()
                .is_none_or(|current| !current.same_reference(value))
        })
        .map(|(entry, marker, _)| (entry, marker))
}

fn attached_synthesis_paint<'a>(
    catalog: &'a GraphCatalog,
    paint: &SynthesisPaint,
) -> Option<(&'a GtxCatalogRecord, &'a Arc<AtomicBool>)> {
    let (scope, name) = paint.key()?;
    catalog
        .entries()
        .filter_map(|(entry, marker)| {
            let GtxCatalogRecord::Synthesis { metadata, value } = entry else {
                return None;
            };
            matches(metadata, scope, name, 10).then_some((entry, marker, value))
        })
        .min_by_key(|(_, _, value)| {
            paint
                .catalog()
                .is_none_or(|current| !current.same_reference(value))
        })
        .map(|(entry, marker, _)| (entry, marker))
}

fn attached_synthesis_number<'a>(
    catalog: &'a GraphCatalog,
    adjustment: &crate::AdjustmentRecord,
    axis: usize,
) -> Option<(&'a GtxCatalogRecord, &'a Arc<AtomicBool>)> {
    let key = link_key(adjustment, axis)?;
    catalog.entries().find(|(entry, _)| {
        matches!(entry, GtxCatalogRecord::Numeric { .. }) && matches_key(entry.metadata(), key)
    })
}

fn attached_environment<'a>(
    catalog: &'a GraphCatalog,
    (group, name): ([u8; 16], &[u16]),
) -> Option<(&'a GtxCatalogRecord, &'a Arc<AtomicBool>)> {
    catalog
        .entries()
        .find(|(entry, _)| matches(entry.metadata(), group, name, 9))
}

impl<E: 'static> ControlMigration<E> {
    fn new(before: &[Option<PointwiseNode<E>>], catalog: &GraphCatalog) -> Self {
        let mut curves = HashMap::new();
        if catalog
            .entries()
            .any(|(entry, _)| entry.metadata().type_tag == ReferenceAdjusterKind::Curve.tag())
        {
            for property in curve_adjusters::curve_properties(before) {
                if let Some(key) = curve_adjusters::adjustment_key(&property)
                    && let PointwiseParameterValue::Curve(Some(curve)) = property.raw_value()
                {
                    curves.entry(key).or_insert(curve);
                }
            }
        }
        Self {
            staged: Vec::new(),
            moved: Vec::new(),
            kept: HashSet::new(),
            curves,
        }
    }

    fn leave(&mut self, marker: &Arc<AtomicBool>) {
        if !self.moved.iter().any(|other| Arc::ptr_eq(other, marker)) {
            self.moved.push(marker.clone());
        }
    }

    fn keep(&mut self, catalog: &GraphCatalog, adjustment: &crate::SharedPropertyAdjustment<E>) {
        self.kept
            .extend(attached_entries(catalog, adjustment).map(|(_, marker)| marker_id(marker)));
    }

    fn record_synthesis_lab_links(
        &mut self,
        catalog: &GraphCatalog,
        roots: &[Option<PointwiseNode<E>>],
    ) {
        let shared = catalog.entries().filter_map(|(entry, _)| match entry {
            GtxCatalogRecord::Synthesis { value, .. } => Some(value.record()),
            _ => None,
        });
        let mut scan = |record: &SynthesisRecord| {
            record.visit_color_adjustments(|key| {
                if let Some((_, marker)) = find_control(catalog, key) {
                    self.leave(marker);
                }
            });
            record.visit_numeric_adjustments(|adjustment| {
                for axis in 0..adjustment.components().len() {
                    if let Some((_, marker)) = attached_synthesis_number(catalog, adjustment, axis)
                    {
                        self.leave(marker);
                    }
                }
            });
        };
        for record in shared {
            scan(&record);
        }
        for filter in graph_nodes(roots).filter_map(|node| node.filter()) {
            if let Some(state) = filter.synthesis_link_state()
                && state.catalog.is_none()
            {
                scan(&state.local);
            }
            filter.visit_retained_synthesis_paints(&mut scan);
        }
    }

    fn keep_surviving_synthesis_lab_links(&mut self, catalog: &GraphCatalog) {
        for (entry, marker) in catalog.entries() {
            if let GtxCatalogRecord::Synthesis { value, .. } = entry
                && (self.kept.contains(&marker_id(marker))
                    || !self.moved.iter().any(|old| Arc::ptr_eq(old, marker)))
            {
                self.keep_synthesis_lab_links(catalog, &value.record(), None);
            }
        }
        // Reconciled shared textures can still contain links to an existing
        // section. Retain those definitions until the last texture releases them.
        let staged = self
            .staged
            .iter()
            .filter_map(|entry| match entry {
                GtxCatalogRecord::Synthesis { value, .. } => Some(value.record()),
                _ => None,
            })
            .collect::<Vec<_>>();
        for record in staged {
            self.keep_synthesis_lab_links(catalog, &record, None);
        }
    }

    fn keep_synthesis_lab_links(
        &mut self,
        catalog: &GraphCatalog,
        record: &SynthesisRecord,
        scope: Option<[u8; 16]>,
    ) {
        record.visit_color_adjustments(|key| {
            if scope.is_none_or(|scope| key.0 == scope)
                && let Some((_, marker)) = find_control(catalog, key)
            {
                self.kept.insert(marker_id(marker));
            }
        });
        record.visit_numeric_adjustments(|adjustment| {
            if scope.is_none_or(|scope| adjustment.guid == scope) {
                for axis in 0..adjustment.components().len() {
                    if let Some((_, marker)) = attached_synthesis_number(catalog, adjustment, axis)
                    {
                        self.kept.insert(marker_id(marker));
                    }
                }
            }
        });
    }

    fn relocate_synthesis_lab_links(
        &mut self,
        catalog: &GraphCatalog,
        record: &SynthesisRecord,
        scope: Option<[u8; 16]>,
    ) -> Result<Option<SynthesisRecord>, GtxGraphError<E>> {
        self.keep_synthesis_lab_links(catalog, record, scope);
        let Some(scope) = scope else {
            return Ok(None);
        };
        let numbers = record.relocate_numeric_adjustments(scope, |adjustment, axis| {
            let Some((source, marker)) = attached_synthesis_number(catalog, adjustment, axis)
            else {
                return Ok(None);
            };
            let GtxCatalogRecord::Numeric { metadata, value } =
                self.destination(catalog, source, marker, scope)?
            else {
                unreachable!("compatible Synthesis numeric destination");
            };
            let update = NumericCatalogUpdate::reconnect(metadata, value).ok_or(
                GtxGraphError::NumericAdjuster(
                    "A destination adjuster has invalid numeric limits.",
                ),
            )?;
            let mut adjusted = adjustment.clone();
            adjusted.guid = scope;
            map_retained_number(&[update], &adjusted, axis)
                .map(|mapped| mapped.map(|(_, value)| value))
                .map_err(node_error)
        })?;
        let mut linked = Vec::new();
        record.visit_color_adjustments(|key| {
            if key.0 != scope {
                linked.push((key.0, key.1, key.2.to_vec()));
            }
        });
        let mut colors = HashMap::<_, HashMap<_, _>>::new();
        for (group, tag, name) in linked {
            if let Some((source, marker)) = find_control(catalog, (group, tag, &name))
                && let Some(crate::environment_record::EnvironmentLabReference::Color(rgb)) = self
                    .destination(catalog, source, marker, scope)?
                    .color_or_gradient()
            {
                colors.entry(group).or_default().insert(name, rgb);
            }
        }
        let colors = if colors.is_empty() {
            None
        } else {
            numbers
                .as_ref()
                .unwrap_or(record)
                .remap_color_adjustments(|(group, _, name)| {
                    Some((scope, name.to_vec(), Some(*colors.get(&group)?.get(name)?)))
                })
        };
        Ok(colors.or(numbers))
    }

    fn destination<'a>(
        &'a mut self,
        catalog: &'a GraphCatalog,
        source: &GtxCatalogRecord,
        source_marker: &Arc<AtomicBool>,
        guid: [u8; 16],
    ) -> Result<&'a GtxCatalogRecord, GtxGraphError<E>> {
        self.leave(source_marker);
        let source_meta = source.metadata();
        let name = source_meta.name.as_deref().expect("matched named control");
        let matching =
            |entry: &GtxCatalogRecord| matches(entry.metadata(), guid, name, source_meta.type_tag);
        Ok(
            if let Some((entry, marker)) = catalog.entries().find(|(entry, _)| matching(entry)) {
                self.kept.insert(marker_id(marker));
                entry
            } else if let Some(index) = self.staged.iter().position(matching) {
                &self.staged[index]
            } else {
                let mut entry = copy_control(source, guid)?;
                if let GtxCatalogRecord::Environment { value, .. } = &mut entry
                    && let Some(after) =
                        self.relocate_environment_map(catalog, &value.value(), Some(guid))?
                {
                    *value = EnvironmentCatalogValue::from_snapshot(after);
                }
                self.staged.push(entry);
                self.staged.last().expect("staged control")
            },
        )
    }

    fn prepare_references(
        &mut self,
        catalog: &GraphCatalog,
        node: &PointwiseNode<E>,
        scope: Option<[u8; 16]>,
    ) -> Result<Vec<LabReferenceEdit<E>>, GtxGraphError<E>> {
        let mut linked = Vec::new();
        LabReferenceEdit::visit_node_links(node, |(group, tag, name)| {
            linked.push((
                (group, tag, name.to_vec()),
                find_control(catalog, (group, tag, name)),
            ));
        });
        let mut values = HashMap::new();
        for (key, control) in linked {
            let Some((source, marker)) = control else {
                continue;
            };
            if let Some(scope) = scope.filter(|scope| *scope != key.0) {
                let destination = self.destination(catalog, source, marker, scope)?;
                if let Some(value) = destination.color_or_gradient() {
                    let ((group, tag), name) = ((key.0, key.1), key.2);
                    values
                        .entry((group, tag))
                        .or_insert_with(HashMap::new)
                        .insert(name, value);
                }
            } else {
                self.kept.insert(marker_id(marker));
            }
        }
        if values.is_empty() {
            return Ok(Vec::new());
        }
        LabReferenceEdit::prepare_scoped_node(node, scope, |(group, tag, name)| {
            values.get(&(group, tag))?.get(name).cloned()
        })
        .map_err(node_error)
    }

    fn prepare_curve(
        &mut self,
        catalog: &GraphCatalog,
        property: &PointwiseParameter<E>,
        guid: [u8; 16],
    ) -> Result<Option<CurveChange<E>>, GtxGraphError<E>> {
        let Some((before, name)) = curve_adjusters::adjustment_key(property) else {
            return Ok(None);
        };
        let tag = ReferenceAdjusterKind::Curve.tag();
        let Some((source, marker)) = catalog
            .entries()
            .find(|(entry, _)| matches(entry.metadata(), before, &name, tag))
        else {
            return Ok(None);
        };
        let key = (guid, name);
        // Existing destinations keep their current live curve. A newly staged
        // catalog copy has no receiver yet, so materialize it once for the move.
        let current = self.curves.get(&key).cloned();
        let destination = if before != guid {
            self.destination(catalog, source, marker, guid)?
        } else {
            self.kept.insert(marker_id(marker));
            source
        };
        let curve = match current {
            Some(current) => current,
            None => {
                let curve = CurveAdjuster::read(destination)
                    .ok_or(GtxGraphError::CurveAdjuster(
                        "The destination Curve adjuster has no value.",
                    ))?
                    .materialize()?;
                self.curves.insert(key, curve.clone());
                curve
            }
        };
        Ok(CurveChange::relocate(property, guid, &curve))
    }

    fn prepare_synthesis(
        &mut self,
        catalog: &GraphCatalog,
        node: &PointwiseNode<E>,
        guid: [u8; 16],
        reconcile: bool,
        retired: Option<&RetiredSynthesisDefinitions<'_>>,
    ) -> Result<Option<SynthesisReceiverEdit<E>>, GtxGraphError<E>> {
        let Some(receiver) = node.filter() else {
            return Ok(None);
        };
        let Some(before) = receiver.synthesis_link_state() else {
            return Ok(None);
        };
        let Some((source, marker)) = attached_synthesis(catalog, &before) else {
            let Some(after) = self.relocate_synthesis_lab_links(
                catalog,
                &before.local,
                reconcile.then_some(guid),
            )?
            else {
                return Ok(None);
            };
            let state = SynthesisLinkState {
                adjustment: before.adjustment.clone(),
                catalog: before.catalog.clone(),
                local: after,
            };
            return Ok(Some(SynthesisReceiverEdit::new(&receiver, before, state)));
        };
        let wire = before
            .adjustment
            .as_ref()
            .expect("matched Synthesis wire link");
        let moves = reconcile && wire.guid() != guid;
        let target = if moves { guid } else { wire.guid() };
        let revived = retired.and_then(|definitions| {
            definitions.stage_destination(&mut self.staged, source, target)
        });
        let destination = if let Some(index) = revived {
            self.leave(marker);
            &self.staged[index]
        } else if moves {
            self.destination(catalog, source, marker, guid)?
        } else {
            self.kept.insert(marker_id(marker));
            source
        };
        let GtxCatalogRecord::Synthesis { value, .. } = destination else {
            unreachable!("compatible Synthesis destination");
        };
        if !moves
            && before
                .catalog
                .as_ref()
                .is_some_and(|current| current.same_reference(value))
        {
            return Ok(None);
        }
        let adjustment = if moves {
            let mut parts = wire.capture_parts();
            parts.guid = guid;
            crate::SharedPropertyAdjustment::from_capture_parts(parts)
        } else {
            wire.clone()
        };
        let after = SynthesisLinkState {
            adjustment: Some(adjustment),
            catalog: Some(value.clone()),
            local: before.local.clone(),
        };
        Ok(Some(SynthesisReceiverEdit::new(&receiver, before, after)))
    }

    fn prepare_texture_paints(
        &mut self,
        catalog: &GraphCatalog,
        node: &PointwiseNode<E>,
        section: Option<[u8; 16]>,
        retired: Option<&RetiredSynthesisDefinitions<'_>>,
        edits: &mut Vec<EnvironmentPaintEdit<E>>,
    ) -> Result<(), GtxGraphError<E>> {
        let Some(receiver) = node.filter() else {
            return Ok(());
        };
        let environment = EnvironmentPaintEdit::relocate(&receiver, section, |key, map, scope| {
            let Some(key) = key else {
                return self.relocate_environment_map(catalog, map, scope);
            };
            let Some((source, marker)) = attached_environment(catalog, key) else {
                return self.relocate_environment_map(catalog, map, scope);
            };
            let target = scope.unwrap_or(key.0);
            let destination = if key.0 == target {
                self.kept.insert(marker_id(marker));
                source
            } else {
                self.destination(catalog, source, marker, target)?
            };
            let GtxCatalogRecord::Environment { value, .. } = destination else {
                unreachable!("compatible Environment destination");
            };
            Ok(Some(value.value()))
        })?;
        edits.extend(environment);
        EnvironmentPaintEdit::stage_synthesis_materials::<GtxGraphError<E>>(
            edits,
            &receiver,
            |paint| {
                let Some((current, _)) = paint.key() else {
                    return Ok(None);
                };
                let Some((source, marker)) = attached_synthesis_paint(catalog, paint) else {
                    return Ok(None);
                };
                let target = section.unwrap_or(current);
                let revived = retired.and_then(|definitions| {
                    definitions.stage_destination(&mut self.staged, source, target)
                });
                let destination = if let Some(index) = revived {
                    self.leave(marker);
                    &self.staged[index]
                } else if target != current {
                    self.destination(catalog, source, marker, target)?
                } else {
                    self.kept.insert(marker_id(marker));
                    source
                };
                let GtxCatalogRecord::Synthesis { value, .. } = destination else {
                    unreachable!("compatible Material destination");
                };
                if target == current && paint.catalog().is_some_and(|old| old.same_reference(value))
                {
                    return Ok(None);
                }
                let mut paint = paint.clone();
                Arc::make_mut(paint.adjustment.as_mut().expect("matched Material")).guid = target;
                paint.resolved = Some((value.clone(), value.record()));
                Ok(Some(paint))
            },
        )?;
        EnvironmentPaintEdit::stage_synthesis(edits, &receiver, |record| {
            self.relocate_synthesis_lab_links(catalog, record, section)
        })
    }

    fn prepare(
        &mut self,
        catalog: &GraphCatalog,
        receiver: &NumericReceiver<E>,
        before: crate::SharedPropertyAdjustment<E>,
        guid: [u8; 16],
    ) -> Result<LabLinkEdit<E>, GtxGraphError<E>> {
        let mut numeric = Vec::new();
        let mut environment = None;
        for (source, marker) in attached_entries(catalog, &before) {
            let destination = self.destination(catalog, source, marker, guid)?;
            match destination {
                GtxCatalogRecord::Numeric { metadata, value } => {
                    numeric.push(NumericCatalogUpdate::reconnect(metadata, value).ok_or(
                        GtxGraphError::NumericAdjuster(
                            "A destination adjuster has invalid numeric limits.",
                        ),
                    )?);
                }
                GtxCatalogRecord::Environment { value, .. } => environment = Some(value.value()),
                _ => {
                    return Err(GtxGraphError::NumericAdjuster(
                        "A linked Group adjuster has an unsupported type.",
                    ));
                }
            }
        }
        let mut parts = before.capture_parts();
        parts.guid = guid;
        let after = crate::SharedPropertyAdjustment::from_capture_parts(parts);
        LabLinkEdit::prepare(receiver, before, after, &numeric, environment.as_ref())
            .map_err(node_error)
    }
}

struct ScopeLinks<E> {
    properties: Vec<LabLinkEdit<E>>,
    synthesis: Vec<SynthesisReceiverEdit<E>>,
    curves: Vec<CurveChange<E>>,
    paints: Vec<EnvironmentPaintEdit<E>>,
    maps: EnvironmentAdjusterBatch<E>,
    references: Vec<LabReferenceEdit<E>>,
}

impl<E: 'static> ScopeLinks<E> {
    fn new() -> Self {
        Self {
            properties: Vec::new(),
            synthesis: Vec::new(),
            curves: Vec::new(),
            paints: Vec::new(),
            maps: EnvironmentAdjusterBatch::empty(),
            references: Vec::new(),
        }
    }

    fn is_empty(&self) -> bool {
        self.properties.is_empty()
            && self.synthesis.is_empty()
            && self.curves.is_empty()
            && self.paints.is_empty()
            && !self.maps.has_local_changes()
            && self.references.is_empty()
    }

    fn publish(&self, undo: bool) -> Result<AffectedProperties<E>, HistoryError<E>> {
        let synthesis = SynthesisReceiverEdit::publish_batch(&self.synthesis, undo)?;
        let direction = if undo {
            HistoryDirection::Undo
        } else {
            HistoryDirection::Redo
        };
        let rollback_direction = if undo {
            HistoryDirection::Redo
        } else {
            HistoryDirection::Undo
        };
        let ((mut affected, references), curves) =
            CurveChange::publish_with(&self.curves, direction, || {
                LabReferenceEdit::publish_with(&self.references, undo, || {
                    let paints = EnvironmentPaintEdit::publish_all(&self.paints, undo)?;
                    let maps = self.maps.publish(direction).inspect_err(|_| {
                        EnvironmentPaintEdit::rollback_batch(&self.paints, &paints, undo);
                    })?;
                    let mut affected =
                        LabLinkEdit::publish(&self.properties, undo).inspect_err(|_| {
                            let _ = self.maps.publish(rollback_direction);
                            EnvironmentPaintEdit::rollback_batch(&self.paints, &paints, undo);
                        })?;
                    affected.environment.properties.extend(maps.properties);
                    affected.environment.paints = paints;
                    Ok(affected)
                })
            })
            .inspect_err(|_| {
                SynthesisReceiverEdit::rollback_batch(&self.synthesis, &synthesis, undo);
            })?;
        affected.synthesis = synthesis;
        affected.curves = curves;
        affected.references = references;
        Ok(affected)
    }
}

pub(crate) struct LabLayoutEdit<E> {
    links: ScopeLinks<E>,
    retired: Vec<Arc<AtomicBool>>,
    created: Vec<Arc<AtomicBool>>,
}

impl<E: 'static> LabLayoutEdit<E> {
    pub(super) fn include_preset_controls(
        layout: Option<Self>,
        imported: Vec<Arc<AtomicBool>>,
    ) -> Option<Self> {
        if imported.is_empty() {
            return layout;
        }
        let mut layout = layout.unwrap_or_else(|| Self {
            links: ScopeLinks::new(),
            retired: Vec::new(),
            created: Vec::new(),
        });
        for marker in imported {
            marker.store(false, Ordering::Release);
            let retained = !layout.retired.iter().any(|old| Arc::ptr_eq(old, &marker));
            if retained {
                layout.created.push(marker);
            } else {
                // A scope migration already staged a replacement for this
                // incoming control. Undo must not re-create its source entry.
                layout.retired.retain(|old| !Arc::ptr_eq(old, &marker));
            }
        }
        Some(layout)
    }

    pub(crate) fn publish(&self, undo: bool) -> Result<AffectedProperties<E>, HistoryError<E>> {
        if self
            .retired
            .iter()
            .any(|marker| marker.load(Ordering::Acquire) == undo)
            || self
                .created
                .iter()
                .any(|marker| marker.load(Ordering::Acquire) != undo)
        {
            return Err(HistoryError::InputChanged);
        }
        let affected = self.links.publish(undo)?;
        for marker in &self.retired {
            marker.store(undo, Ordering::Release);
        }
        for marker in &self.created {
            marker.store(!undo, Ordering::Release);
        }
        Ok(affected)
    }
}

fn copy_control<E>(
    source: &GtxCatalogRecord,
    guid: [u8; 16],
) -> Result<GtxCatalogRecord, GtxGraphError<E>> {
    let mut metadata = source.metadata().clone();
    metadata.group_id = guid;
    Ok(match source {
        GtxCatalogRecord::Numeric { value, .. } => {
            let payload = match &*value.lock() {
                ValueAdjusterPayload::Number(value) => ValueAdjusterPayload::Number(value.clone()),
                ValueAdjusterPayload::Dual(value) => ValueAdjusterPayload::Dual(value.clone()),
                _ => unreachable!("numeric control payload"),
            };
            GtxCatalogRecord::Numeric {
                metadata,
                value: NumericCatalogValue::new(payload),
            }
        }
        GtxCatalogRecord::Environment { value, .. } => GtxCatalogRecord::Environment {
            metadata,
            value: EnvironmentCatalogValue::from_snapshot(value.value()),
        },
        GtxCatalogRecord::Synthesis { value, .. } => GtxCatalogRecord::Synthesis {
            metadata,
            value: SynthesisCatalogValue::new(value.record()),
        },
        GtxCatalogRecord::Value(record) if metadata.type_tag == ValueAdjusterKind::Color.tag() => {
            let ValueAdjusterPayload::Color { value, previous } = record.payload else {
                unreachable!("Color control payload");
            };
            GtxCatalogRecord::Value(Box::new(ValueAdjusterRecord {
                metadata,
                payload: ValueAdjusterPayload::Color { value, previous },
            }))
        }
        GtxCatalogRecord::Reference(record)
            if metadata.type_tag == ReferenceAdjusterKind::Gradient.tag() =>
        {
            let ReferenceAdjusterPayload::Gradient { current, previous } = &record.payload else {
                unreachable!("Gradient control payload");
            };
            let copied = current
                .as_ref()
                .map(copy_gradient)
                .transpose()
                .map_err(gradient_error)?;
            let previous = match (current, previous) {
                (Some(current), Some(previous)) if current.same_reference(previous) => {
                    copied.clone()
                }
                (_, previous) => previous
                    .as_ref()
                    .map(copy_gradient)
                    .transpose()
                    .map_err(gradient_error)?,
            };
            GtxCatalogRecord::Reference(Box::new(ReferenceAdjusterRecord {
                metadata,
                payload: ReferenceAdjusterPayload::Gradient {
                    current: copied,
                    previous,
                },
            }))
        }
        GtxCatalogRecord::Reference(_)
            if metadata.type_tag == ReferenceAdjusterKind::Curve.tag() =>
        {
            CurveAdjuster::read(source)
                .ok_or(GtxGraphError::CurveAdjuster(
                    "The source Curve adjuster has no value.",
                ))?
                .detached_catalog_record(metadata)
        }
        _ => {
            return Err(GtxGraphError::NumericAdjuster(
                "A linked Group adjuster has an unsupported type.",
            ));
        }
    })
}

struct GroupNameEdit<E> {
    property: WeakPointwiseParameter<E>,
    before: Option<Vec<u16>>,
    after: Vec<u16>,
}

struct GroupScopeEdit<E> {
    toggle: WeakPointwiseParameter<E>,
    before: NumericPropertyState,
    after: NumericPropertyState,
    name: Option<GroupNameEdit<E>>,
    links: ScopeLinks<E>,
    retired: Vec<Arc<AtomicBool>>,
    created: Vec<Arc<AtomicBool>>,
}

impl<E: 'static> GroupScopeEdit<E> {
    fn publish(
        &self,
        direction: HistoryDirection,
    ) -> Result<(PointwiseParameter<E>, AffectedProperties<E>), HistoryError<E>> {
        let undo = matches!(direction, HistoryDirection::Undo);
        let toggle = self.toggle.upgrade().ok_or(HistoryError::NullTarget)?;
        let (expected, next) = if undo {
            (&self.after, &self.before)
        } else {
            (&self.before, &self.after)
        };
        let (actual, _) = toggle
            .prepare_numeric_adjustment(NumericAdjustment::Choice(toggle.index()))
            .map_err(|error| HistoryError::Property(Box::new(error)))?;
        if &actual != expected
            || self
                .retired
                .iter()
                .any(|marker| marker.load(Ordering::Acquire) == undo)
            || self
                .created
                .iter()
                .any(|marker| marker.load(Ordering::Acquire) != undo)
        {
            return Err(HistoryError::InputChanged);
        }
        let name = self
            .name
            .as_ref()
            .map(|edit| {
                let property = edit.property.upgrade().ok_or(HistoryError::NullTarget)?;
                let (before, after) = (edit.before.as_deref(), Some(edit.after.as_slice()));
                let (before, after) = if undo {
                    (after, before)
                } else {
                    (before, after)
                };
                if !property.compare_and_replace_text(before, after) {
                    return Err(HistoryError::InputChanged);
                }
                Ok((property, before, after))
            })
            .transpose()?;
        let affected = self.links.publish(undo).inspect_err(|_| {
            if let Some((property, before, after)) = name {
                property.compare_and_replace_text(after, before);
            }
        })?;
        toggle.write_numeric_adjustment(next);
        for marker in &self.retired {
            marker.store(undo, Ordering::Release);
        }
        for marker in &self.created {
            marker.store(!undo, Ordering::Release);
        }
        Ok((toggle, affected))
    }
}

fn notify_group<E: 'static>(
    (toggle, affected): (PointwiseParameter<E>, AffectedProperties<E>),
) -> Result<(), HistoryError<E>> {
    affected.notify().and(
        toggle
            .notify_numeric_adjustment()
            .map_err(|error| HistoryError::Property(Box::new(error))),
    )
}

impl<E: 'static> HistoryGraphEdit<E> for GroupScopeEdit<E> {
    fn apply(&self, direction: HistoryDirection) -> Result<(), HistoryError<E>> {
        notify_group(self.publish(direction)?)
    }
}
