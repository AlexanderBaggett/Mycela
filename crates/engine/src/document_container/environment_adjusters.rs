use super::numeric_adjusters::{NumericCatalogUpdate, map_environment_number};
use super::*;
use crate::history::HistoryGraphEdit;
use crate::pointwise::EnvironmentPaintEdit;
use crate::pointwise::property::WeakPointwiseParameter;
use crate::{
    CanvasLayerEditor, EnvironmentRecord, HistoryDirection, HistoryError, PointwiseFilter,
    PointwiseKind, PointwiseParameter, PointwiseParameterValue, SharedPropertyAdjustment,
};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};

#[path = "environment_adjuster_references.rs"]
mod references;
pub(super) use references::MapReferenceValues;
#[path = "environment_field_links.rs"]
mod field_links;
pub use field_links::{EnvironmentColorLink, EnvironmentMapTarget, EnvironmentNumericMapping};

#[derive(Clone, Debug)]
pub struct EnvironmentAdjuster {
    group_id: [u8; 16],
    name: Vec<u16>,
    value: EnvironmentCatalogValue,
}

pub enum EnvironmentAdjusterLink {
    Create(String),
    Existing(EnvironmentAdjuster),
    Unlink,
}

impl EnvironmentAdjuster {
    pub fn name(&self) -> &[u16] {
        &self.name
    }

    pub fn value(&self) -> Arc<EnvironmentRecord> {
        self.value.value()
    }

    pub fn same_reference(&self, other: &Self) -> bool {
        self.value.same_reference(&other.value)
    }
}

impl<E: 'static> Gtx375GraphDocument<E> {
    pub fn environment_adjuster(
        &self,
        property: &PointwiseParameter<E>,
    ) -> Option<EnvironmentAdjuster> {
        let (group_id, name) = environment_adjustment_key(property)?;
        self.adjusters_in_group(group_id)
            .find(|adjuster| adjuster.name == name)
    }

    /// Resolves the saved whole-map Lab link of an Environment paint.
    pub fn style_environment_adjuster(
        &self,
        style: &crate::StyleEdit,
        pass: usize,
    ) -> Option<EnvironmentAdjuster> {
        self.adjuster_for_key(style.environment_adjuster_key(pass)?)
    }

    pub fn available_style_environment_adjusters(
        &self,
        owner: &PointwiseNode<E>,
    ) -> Vec<EnvironmentAdjuster> {
        self.adjusters_in_group(environment_adjuster_scope(owner))
            .collect()
    }

    fn adjuster_for_key(&self, (group, name): ([u8; 16], &[u16])) -> Option<EnvironmentAdjuster> {
        self.adjusters_in_group(group)
            .find(|adjuster| adjuster.name == name)
    }

    /// Shows a working shared map on every connected property and paint.
    pub fn preview_environment_adjuster(
        &self,
        adjuster: &EnvironmentAdjuster,
        map: Arc<EnvironmentRecord>,
    ) -> crate::EnvironmentRenderOverride<E> {
        crate::EnvironmentRenderOverride::for_adjuster(
            self.environment_adjuster_properties(adjuster),
            adjuster.group_id,
            adjuster.name.clone(),
            map,
        )
    }

    pub fn available_environment_adjusters(
        &self,
        property: &PointwiseParameter<E>,
    ) -> Vec<EnvironmentAdjuster> {
        self.adjusters_in_group(environment_adjuster_scope(&property.owner().node()))
            .collect()
    }

    /// The current graph properties that receive this shared map.
    pub fn environment_adjuster_properties(
        &self,
        adjuster: &EnvironmentAdjuster,
    ) -> Vec<PointwiseParameter<E>> {
        adjusted_environment_properties(&self.branches())
            .filter(|(key, _)| key.0 == adjuster.group_id && key.1 == adjuster.name)
            .map(|(_, property)| property)
            .collect()
    }

    pub(super) fn adjusters_in_group(
        &self,
        group_id: [u8; 16],
    ) -> impl Iterator<Item = EnvironmentAdjuster> {
        self.catalog
            .entries()
            .filter_map(move |(entry, _)| match entry {
                GtxCatalogRecord::Environment { metadata, value }
                    if metadata.group_id == group_id =>
                {
                    let name = metadata
                        .name
                        .as_ref()
                        .filter(|name| !name.is_empty())?
                        .clone();
                    Some(EnvironmentAdjuster {
                        group_id,
                        name,
                        value: value.clone(),
                    })
                }
                _ => None,
            })
    }

    /// Connect to a catalog adjuster, starting a new shared value from this
    /// property if requested. Disconnecting keeps the last applied map.
    pub fn set_environment_adjuster(
        &mut self,
        property: &PointwiseParameter<E>,
        link: EnvironmentAdjusterLink,
    ) -> Result<bool, GtxGraphError<E>> {
        let PointwiseParameterValue::Environment(before_map) = property.raw_value() else {
            return Err(GtxGraphError::EnvironmentAdjuster(
                "Select an Environment map.",
            ));
        };
        let branches = self.branches();
        if !property.is_adjustable()
            || !environment_properties(&branches).any(|target| target.same_reference(property))
        {
            return Err(GtxGraphError::EnvironmentAdjuster(
                "This Environment property is no longer available.",
            ));
        }
        let before_adjustment = property.retained_adjustment();
        let before_key = environment_adjustment_key(property);
        let old = self.environment_adjuster(property);
        let group = environment_adjuster_scope(&property.owner().node());
        let Some(destination) = self.choose_environment_link(
            group,
            &before_map,
            old.as_ref(),
            before_key.is_some(),
            link,
        )?
        else {
            return Ok(false);
        };

        let after_adjustment = destination
            .adjuster
            .as_ref()
            .map(|adjuster| property.new_environment_adjustment(&adjuster.name, adjuster.group_id))
            .transpose()
            .map_err(|error| GtxGraphError::Node(NodeWireError::Property(error)))?;
        let after_map = destination.map(&before_map);
        let retired = old.and_then(|adjuster| {
            self.retiring_environment_adjuster(&branches, &adjuster, Some(property))
        });
        drop(branches);
        self.commit_environment_link(
            destination,
            retired,
            EnvironmentLinkTarget::Property {
                property: property.downgrade(),
                before: LinkState {
                    adjustment: before_adjustment,
                    map: before_map,
                },
                after: LinkState {
                    adjustment: after_adjustment,
                    map: after_map,
                },
            },
        )
    }

    /// Connects an active Environment paint to a whole-map Lab adjuster.
    pub fn set_style_environment_adjuster(
        &mut self,
        owner: &PointwiseNode<E>,
        layer: Option<&CanvasLayerEditor<E>>,
        pass: usize,
        expected: &Arc<EnvironmentRecord>,
        link: EnvironmentAdjusterLink,
    ) -> Result<bool, GtxGraphError<E>> {
        let unavailable = || {
            GtxGraphError::EnvironmentAdjuster(
                "The selected Environment paint changed. Select its pass again.",
            )
        };
        let branches = self.branches();
        if !graph_nodes(&branches).any(|node| node.same_reference(owner)) {
            return Err(unavailable());
        }
        let owner = owner.filter().ok_or_else(unavailable)?;
        let mut edit = match layer {
            Some(layer) => layer.style_edit(),
            None => owner.style_edit(),
        }
        .ok_or(GtxGraphError::EnvironmentAdjuster(
            "The selected Style pass is unavailable.",
        ))?;
        let Some(before) = edit
            .environment_map(pass)
            .filter(|map| Arc::ptr_eq(map, expected))
        else {
            return Err(unavailable());
        };
        let key = edit.environment_adjuster_key(pass);
        let old = key.and_then(|key| self.adjuster_for_key(key));
        let group = environment_adjuster_scope(&owner.node());
        let Some(destination) =
            self.choose_environment_link(group, before, old.as_ref(), key.is_some(), link)?
        else {
            return Ok(false);
        };
        let after_map = destination.map(before);
        let after_key = destination
            .adjuster
            .as_ref()
            .map(|adjuster| (adjuster.group_id, adjuster.name.as_slice()));
        edit.replace_environment_link(pass, after_key, after_map)
            .ok_or(GtxGraphError::EnvironmentAdjuster(
                "The selected Style pass is unavailable.",
            ))?;
        let paint =
            EnvironmentPaintEdit::from_style_edit(&owner, layer, edit).ok_or_else(unavailable)?;
        let retired =
            old.and_then(|adjuster| self.retiring_environment_adjuster(&branches, &adjuster, None));
        drop(branches);
        self.commit_environment_link(destination, retired, EnvironmentLinkTarget::Paint(paint))
    }

    fn choose_environment_link(
        &self,
        group_id: [u8; 16],
        before: &Arc<EnvironmentRecord>,
        old: Option<&EnvironmentAdjuster>,
        linked: bool,
        link: EnvironmentAdjusterLink,
    ) -> Result<Option<EnvironmentLinkDestination>, GtxGraphError<E>> {
        let destination = match link {
            EnvironmentAdjusterLink::Create(name) => {
                let name: Vec<u16> = name.trim().encode_utf16().collect();
                if name.is_empty() {
                    return Err(GtxGraphError::EnvironmentAdjuster(
                        "Enter a name for the Environment adjuster.",
                    ));
                }
                if self.adjuster_for_key((group_id, &name)).is_some() {
                    return Err(GtxGraphError::EnvironmentAdjuster(
                        "An Environment adjuster already uses that name in this group.",
                    ));
                }
                EnvironmentLinkDestination {
                    adjuster: Some(EnvironmentAdjuster {
                        group_id,
                        name,
                        value: EnvironmentCatalogValue::from_snapshot(before.clone()),
                    }),
                    create: true,
                }
            }
            EnvironmentAdjusterLink::Existing(adjuster) => {
                let adjuster = self
                    .adjusters_in_group(group_id)
                    .find(|other| other.same_reference(&adjuster))
                    .ok_or(GtxGraphError::EnvironmentAdjuster(
                        "This adjuster is no longer available in this group.",
                    ))?;
                if old.is_some_and(|current| current.same_reference(&adjuster)) {
                    return Ok(None);
                }
                EnvironmentLinkDestination {
                    adjuster: Some(adjuster),
                    create: false,
                }
            }
            EnvironmentAdjusterLink::Unlink if linked => EnvironmentLinkDestination {
                adjuster: None,
                create: false,
            },
            EnvironmentAdjusterLink::Unlink => return Ok(None),
        };
        Ok(Some(destination))
    }

    fn retiring_environment_adjuster(
        &self,
        branches: &[Option<PointwiseNode<E>>],
        adjuster: &EnvironmentAdjuster,
        property: Option<&PointwiseParameter<E>>,
    ) -> Option<Arc<AtomicBool>> {
        let key = (adjuster.group_id, adjuster.name.as_slice());
        let property_remains = adjusted_environment_properties(branches).any(|(other, target)| {
            (other.0, other.1.as_slice()) == key
                && property.is_none_or(|property| !target.same_reference(property))
        });
        if property_remains {
            return None;
        }
        let mut remaining = usize::from(property.is_none());
        for filter in graph_nodes(branches).filter_map(|node| node.filter()) {
            let count = filter.environment_paint_adjuster_count(key);
            if count > remaining {
                return None;
            }
            remaining -= count;
        }
        self.catalog
            .entries()
            .find_map(|(entry, active)| match entry {
                GtxCatalogRecord::Environment { value, .. }
                    if value.same_reference(&adjuster.value) =>
                {
                    Some(active.clone())
                }
                _ => None,
            })
    }

    fn commit_environment_link(
        &mut self,
        destination: EnvironmentLinkDestination,
        retired: Option<Arc<AtomicBool>>,
        target: EnvironmentLinkTarget<E>,
    ) -> Result<bool, GtxGraphError<E>> {
        let label = if destination.create {
            "Create Environment Adjuster"
        } else if destination.adjuster.is_some() {
            "Link Environment Adjuster"
        } else {
            "Unlink Environment Adjuster"
        };
        let created = destination
            .adjuster
            .filter(|_| destination.create)
            .map(|adjuster| {
                let group_id = adjuster.group_id;
                let order = self
                    .catalog
                    .entries()
                    .filter(|(entry, _)| entry.metadata().group_id == group_id)
                    .map(|(entry, _)| entry.metadata().order)
                    .max()
                    .map_or(0, |last| last.saturating_add(1));
                self.catalog.stage(GtxCatalogRecord::Environment {
                    metadata: AdjusterMetadataRecord {
                        name: Some(adjuster.name),
                        group_id,
                        type_tag: 9,
                        order,
                        loaded: false,
                    },
                    value: adjuster.value,
                })
            });
        let edit = EnvironmentLinkEdit {
            target,
            created,
            retired: retired.into_iter().collect(),
        };
        let affected = edit
            .publish(HistoryDirection::Redo)
            .map_err(GtxGraphError::History)?;
        self.record_lab_edit(label, edit)
            .and(affected.notify())
            .map_err(GtxGraphError::History)?;
        Ok(true)
    }

    /// Replaces the shared adjuster and publishes the same value to all of its
    /// current properties in one document history command. A stale inspector
    /// can no longer publish after its property or adjuster has been replaced.
    pub fn edit_environment_adjuster(
        &mut self,
        property: &PointwiseParameter<E>,
        adjuster: &EnvironmentAdjuster,
        expected: &Arc<EnvironmentRecord>,
        replacement: EnvironmentRecord,
    ) -> Result<bool, GtxGraphError<E>> {
        if !self
            .environment_adjuster(property)
            .is_some_and(|actual| actual.same_reference(adjuster))
            || !self
                .environment_adjuster_properties(adjuster)
                .iter()
                .any(|target| target.same_reference(property))
        {
            return Ok(false);
        }
        self.edit_environment_adjuster_value(adjuster, expected, replacement)
    }

    /// Replaces a still-current Lab value and all of its live receivers.
    pub fn edit_environment_adjuster_value(
        &self,
        adjuster: &EnvironmentAdjuster,
        expected: &Arc<EnvironmentRecord>,
        replacement: EnvironmentRecord,
    ) -> Result<bool, GtxGraphError<E>> {
        let Some(actual) = self
            .adjusters_in_group(adjuster.group_id)
            .find(|actual| actual.name == adjuster.name && actual.same_reference(adjuster))
        else {
            return Ok(false);
        };
        let label = format!("Edit {}", String::from_utf16_lossy(&actual.name));
        let mut batch = self.stage_environment_adjuster_maps(vec![(actual, Arc::new(replacement))]);
        if !Arc::ptr_eq(expected, &batch.shared[0].before) {
            return Ok(false);
        }
        batch.stage_receivers(&self.branches(), &self.catalog, &[])?;
        let affected = batch
            .publish(HistoryDirection::Redo)
            .map_err(GtxGraphError::History)?;
        self.record_lab_edit(&label, batch)
            .and(affected.notify())
            .map_err(GtxGraphError::History)?;
        Ok(true)
    }

    pub(super) fn stage_environment_adjuster_maps(
        &self,
        replacements: Vec<(EnvironmentAdjuster, Arc<EnvironmentRecord>)>,
    ) -> EnvironmentAdjusterBatch<E> {
        let mut changes: Vec<_> = replacements
            .into_iter()
            .map(|(adjuster, after)| EnvironmentAdjusterEdit {
                key: (adjuster.group_id, adjuster.name),
                before: adjuster.value.value(),
                value: adjuster.value,
                after,
                targets: Vec::new(),
            })
            .collect();
        if !changes.is_empty() {
            for (key, property) in adjusted_environment_properties(&self.branches()) {
                if let Some(change) = changes.iter_mut().find(|change| change.key == key)
                    && let Some(target) = AdjustedEnvironment::capture(&property)
                {
                    change.targets.push(target);
                }
            }
        }
        EnvironmentAdjusterBatch::new(changes)
    }

    pub(super) fn record_lab_edit(
        &self,
        label: &str,
        edit: impl HistoryGraphEdit<E> + 'static,
    ) -> Result<(), HistoryError<E>> {
        let Some(history) = self.property_history() else {
            return Ok(());
        };
        let target = self
            .history_slot(0)
            .ok_or(HistoryError::NullTarget)?
            .history_target();
        history.record_graph_edit(target, label.encode_utf16().collect(), Box::new(edit))
    }
}

type AdjustmentKey = ([u8; 16], Vec<u16>);

pub(super) fn environment_adjuster_scope<E: 'static>(owner: &PointwiseNode<E>) -> [u8; 16] {
    enclosing_adjuster_group(owner)
        .and_then(|group| group.group_guid())
        .unwrap_or_default()
}

pub(super) fn catching_group<E: 'static>(node: &PointwiseNode<E>) -> Option<PointwiseFilter<E>> {
    let filter = node.filter()?;
    (filter.kind() == PointwiseKind::Group
        && filter.parameter(4).is_ok_and(|catch| catch.index() == 0))
    .then_some(filter)
}

pub(super) fn enclosing_adjuster_group<E: 'static>(
    node: &PointwiseNode<E>,
) -> Option<PointwiseFilter<E>> {
    enclosing_adjuster_group_with(node, None)
}

pub(super) fn enclosing_adjuster_group_with<E: 'static>(
    node: &PointwiseNode<E>,
    changed: Option<(&PointwiseNode<E>, bool)>,
) -> Option<PointwiseFilter<E>> {
    let mut child = node.clone();
    let mut visited = Vec::<PointwiseNode<E>>::new();
    while let Some(parent) = child.parent() {
        if visited.iter().any(|node| node.same_reference(&parent)) {
            break;
        }
        let group = match changed {
            Some((target, catches)) if parent.same_reference(target) => {
                if catches {
                    parent.filter()
                } else {
                    None
                }
            }
            _ => catching_group(&parent),
        };
        if let Some(group) = group
            && parent
                .contents()
                .iter()
                .any(|slot| slot.node().is_some_and(|node| node.same_reference(&child)))
        {
            return Some(group);
        }
        visited.push(parent.clone());
        child = parent;
    }
    None
}

pub(super) fn environment_adjustment_key<E>(
    property: &PointwiseParameter<E>,
) -> Option<AdjustmentKey> {
    let adjustment = property.retained_adjustment()?;
    if adjustment.property_type() != 9
        || !adjustment.is_active().ok()?
        || !matches!(
            property.raw_value(),
            PointwiseParameterValue::Environment(_)
        )
    {
        return None;
    }
    let name = adjustment.names().entries().into_iter().next().flatten()?;
    (!name.is_empty()).then_some((adjustment.guid(), name))
}

fn adjusted_environment_properties<E: 'static>(
    branches: &[Option<PointwiseNode<E>>],
) -> impl Iterator<Item = (AdjustmentKey, PointwiseParameter<E>)> {
    environment_properties(branches)
        .filter_map(|property| environment_adjustment_key(&property).map(|key| (key, property)))
}

fn environment_properties<E: 'static>(
    branches: &[Option<PointwiseNode<E>>],
) -> impl Iterator<Item = PointwiseParameter<E>> {
    graph_nodes(branches)
        .filter_map(|node| node.filter())
        .flat_map(|filter| filter.retained_parameters())
        .filter(|property| {
            matches!(
                property.raw_value(),
                PointwiseParameterValue::Environment(_)
            )
        })
}
pub(super) fn graph_nodes<E: 'static>(
    branches: &[Option<PointwiseNode<E>>],
) -> impl Iterator<Item = PointwiseNode<E>> {
    let mut pending: Vec<_> = branches.iter().rev().flatten().cloned().collect();
    let mut visited = BTreeSet::new();
    std::iter::from_fn(move || {
        loop {
            let node = pending.pop()?;
            if !visited.insert(node.graph_identity()) {
                continue;
            }
            pending.extend(
                node.slots()
                    .into_iter()
                    .chain(node.contents())
                    .rev()
                    .filter_map(|slot| slot.node()),
            );
            return Some(node);
        }
    })
}

struct LinkState<E> {
    adjustment: Option<SharedPropertyAdjustment<E>>,
    map: Arc<EnvironmentRecord>,
}

struct EnvironmentLinkDestination {
    adjuster: Option<EnvironmentAdjuster>,
    create: bool,
}

impl EnvironmentLinkDestination {
    fn map(&self, before: &Arc<EnvironmentRecord>) -> Arc<EnvironmentRecord> {
        self.adjuster
            .as_ref()
            .map_or_else(|| before.clone(), EnvironmentAdjuster::value)
    }
}

enum EnvironmentLinkTarget<E> {
    Property {
        property: WeakPointwiseParameter<E>,
        before: LinkState<E>,
        after: LinkState<E>,
    },
    Paint(EnvironmentPaintEdit<E>),
    Map(EnvironmentAdjusterBatch<E>),
}

struct EnvironmentLinkEdit<E> {
    target: EnvironmentLinkTarget<E>,
    created: Option<Arc<AtomicBool>>,
    retired: Vec<Arc<AtomicBool>>,
}

impl<E: 'static> EnvironmentLinkEdit<E> {
    fn publish(
        &self,
        direction: HistoryDirection,
    ) -> Result<AffectedEnvironments<E>, HistoryError<E>> {
        let undo = matches!(direction, HistoryDirection::Undo);
        if self
            .created
            .as_ref()
            .is_some_and(|active| active.load(Ordering::Acquire) != undo)
            || self
                .retired
                .iter()
                .any(|active| active.load(Ordering::Acquire) == undo)
        {
            return Err(HistoryError::InputChanged);
        }
        let mut affected = AffectedEnvironments::default();
        match &self.target {
            EnvironmentLinkTarget::Property {
                property,
                before,
                after,
            } => {
                let property = property.upgrade().ok_or(HistoryError::NullTarget)?;
                let (current, next) = if undo {
                    (after, before)
                } else {
                    (before, after)
                };
                if !property.replace_environment_adjustment(
                    current.adjustment.as_ref(),
                    next.adjustment.clone(),
                    next.map.clone(),
                ) {
                    return Err(HistoryError::InputChanged);
                }
                affected.properties.push(property);
            }
            EnvironmentLinkTarget::Paint(paint) => {
                affected.paints =
                    EnvironmentPaintEdit::publish_all(std::slice::from_ref(paint), undo)?;
            }
            EnvironmentLinkTarget::Map(batch) => affected = batch.publish(direction)?,
        }
        if let Some(active) = &self.created {
            active.store(!undo, Ordering::Release);
        }
        for active in &self.retired {
            active.store(undo, Ordering::Release);
        }
        Ok(affected)
    }
}

impl<E: 'static> HistoryGraphEdit<E> for EnvironmentLinkEdit<E> {
    fn apply(&self, direction: HistoryDirection) -> Result<(), HistoryError<E>> {
        self.publish(direction)?.notify()
    }
}

/// Genetica reconnects loaded adjusters by group ID, name and type. The saved
/// catalog value supplies the whole map to each matching property.
pub(super) fn restore_environment_catalog<E: 'static>(
    branches: &[Option<PointwiseNode<E>>],
    catalog: &[GtxCatalogRecord],
) -> Result<(), PointwiseError<E>> {
    let mut maps = HashMap::new();
    for entry in catalog {
        let GtxCatalogRecord::Environment { metadata, value } = entry else {
            continue;
        };
        if let Some(name) = metadata.name.as_ref().filter(|name| !name.is_empty()) {
            maps.entry((metadata.group_id, name.as_slice()))
                .or_insert_with(|| value.value());
        }
    }
    if !maps.is_empty() {
        for (key, property) in adjusted_environment_properties(branches) {
            if let Some(value) = maps.get(&(key.0, key.1.as_slice())) {
                property.set_environment(value.clone())?;
            }
        }
        let mut paints = Vec::new();
        for filter in graph_nodes(branches).filter_map(|node| node.filter()) {
            if let Some(edit) = EnvironmentPaintEdit::stage(&filter, |_, key| {
                Ok::<_, PointwiseError<E>>(key.and_then(|key| maps.get(&key).cloned()))
            })? {
                paints.push(edit);
            }
        }
        let owners = EnvironmentPaintEdit::publish_all(&paints, false)
            .map_err(|error| PointwiseError::History(Box::new(error)))?;
        for owner in owners {
            EnvironmentPaintEdit::notify(&owner)
                .map_err(|error| PointwiseError::History(Box::new(error)))?;
        }
    }
    Ok(())
}

struct AdjustedEnvironment<E> {
    property: WeakPointwiseParameter<E>,
    adjustment: SharedPropertyAdjustment<E>,
    before: Arc<EnvironmentRecord>,
}

impl<E> AdjustedEnvironment<E> {
    fn capture(property: &PointwiseParameter<E>) -> Option<Self> {
        let PointwiseParameterValue::Environment(before) = property.raw_value() else {
            return None;
        };
        Some(Self {
            property: property.downgrade(),
            adjustment: property.retained_adjustment()?,
            before,
        })
    }
}

struct EnvironmentAdjusterEdit<E> {
    key: AdjustmentKey,
    value: EnvironmentCatalogValue,
    before: Arc<EnvironmentRecord>,
    after: Arc<EnvironmentRecord>,
    targets: Vec<AdjustedEnvironment<E>>,
}

struct LocalEnvironmentEdit<E> {
    property: WeakPointwiseParameter<E>,
    before: Arc<EnvironmentRecord>,
    after: Arc<EnvironmentRecord>,
}

pub(super) struct EnvironmentAdjusterBatch<E> {
    shared: Vec<EnvironmentAdjusterEdit<E>>,
    local: Vec<LocalEnvironmentEdit<E>>,
    paints: Vec<EnvironmentPaintEdit<E>>,
}

pub(super) struct AffectedEnvironments<E> {
    pub(super) properties: Vec<PointwiseParameter<E>>,
    pub(super) paints: Vec<PointwiseFilter<E>>,
}

impl<E> Default for AffectedEnvironments<E> {
    fn default() -> Self {
        Self {
            properties: Vec::new(),
            paints: Vec::new(),
        }
    }
}

impl<E: 'static> AffectedEnvironments<E> {
    pub(super) fn notify(&self) -> Result<(), HistoryError<E>> {
        let mut result = notify_targets(&self.properties);
        for owner in &self.paints {
            result = result.and(EnvironmentPaintEdit::notify(owner));
        }
        result
    }
}

impl<E: 'static> EnvironmentAdjusterBatch<E> {
    pub(super) fn collect_draft_preview(
        &self,
        preview: &mut crate::pointwise::property::capture::DraftRenderOverrides,
    ) {
        for (property, after) in self
            .local
            .iter()
            .map(|edit| (&edit.property, &edit.after))
            .chain(self.shared.iter().flat_map(|edit| {
                edit.targets
                    .iter()
                    .map(|target| (&target.property, &edit.after))
            }))
        {
            if let Some(property) = property.upgrade() {
                preview.environments.insert(
                    (
                        property.owner().node().occurrence_id(),
                        property.index() as usize,
                    ),
                    after.clone(),
                );
            }
        }
        for paint in &self.paints {
            paint.collect_draft_preview(preview);
        }
    }

    fn new(shared: Vec<EnvironmentAdjusterEdit<E>>) -> Self {
        Self {
            shared,
            local: Vec::new(),
            paints: Vec::new(),
        }
    }

    pub(super) fn empty() -> Self {
        Self::new(Vec::new())
    }

    pub(super) fn stage_local(
        &mut self,
        property: &PointwiseParameter<E>,
        before: Arc<EnvironmentRecord>,
        after: Arc<EnvironmentRecord>,
    ) {
        self.local.push(LocalEnvironmentEdit {
            property: property.downgrade(),
            before,
            after,
        });
    }

    pub(super) fn has_local_changes(&self) -> bool {
        !self.local.is_empty()
    }

    pub(super) fn stage_synthesis_paints(
        &mut self,
        owner: &PointwiseFilter<E>,
        transform: impl FnMut(
            &crate::SynthesisRecord,
            Option<i32>,
        ) -> Result<Option<crate::SynthesisRecord>, GtxGraphError<E>>,
    ) -> Result<(), GtxGraphError<E>> {
        EnvironmentPaintEdit::stage_synthesis_in_inputs(&mut self.paints, owner, transform)
    }

    pub(super) fn stage_synthesis_materials(
        &mut self,
        owner: &PointwiseFilter<E>,
        transform: impl FnMut(
            &crate::pointwise::SynthesisPaint,
        )
            -> Result<Option<crate::pointwise::SynthesisPaint>, GtxGraphError<E>>,
    ) -> Result<(), GtxGraphError<E>> {
        EnvironmentPaintEdit::stage_synthesis_materials(&mut self.paints, owner, transform)
    }

    pub(super) fn stage_receivers(
        &mut self,
        branches: &[Option<PointwiseNode<E>>],
        catalog: &GraphCatalog,
        updates: &[NumericCatalogUpdate],
    ) -> Result<(), GtxGraphError<E>> {
        self.stage_receiver_values(branches, catalog, updates, None)
    }

    pub(super) fn stage_receiver_values(
        &mut self,
        branches: &[Option<PointwiseNode<E>>],
        catalog: &GraphCatalog,
        updates: &[NumericCatalogUpdate],
        mut references: Option<&mut MapReferenceValues<'_>>,
    ) -> Result<(), GtxGraphError<E>> {
        if updates.is_empty() && references.is_none() {
            if self.shared.is_empty() {
                return Ok(());
            }
            return self.stage_paints(branches, |_, _| Ok(None));
        }
        let mapped = |map: &EnvironmentRecord| {
            let numbers = if updates.is_empty() {
                None
            } else {
                map.map_numeric_adjustments(|adjustment| {
                    map_environment_number(updates, adjustment)
                })
                .map_err(|error| GtxGraphError::Node(error.into()))?
            };
            let mapped = if let Some(references) = &mut references {
                numbers
                    .as_ref()
                    .unwrap_or(map)
                    .map_reference_adjustments(|adjustment| references.resolve(adjustment))
                    .map_err(|error| GtxGraphError::Node(PointwiseError::Gradient(error).into()))?
                    .or(numbers)
            } else {
                numbers
            };
            Ok(mapped.map(Arc::new))
        };
        self.stage_mapped_receivers(branches, catalog, false, mapped)
    }

    pub(super) fn stage_mapped_receivers(
        &mut self,
        branches: &[Option<PointwiseNode<E>>],
        catalog: &GraphCatalog,
        include_bound: bool,
        mut mapped: impl FnMut(
            &EnvironmentRecord,
        ) -> Result<Option<Arc<EnvironmentRecord>>, GtxGraphError<E>>,
    ) -> Result<(), GtxGraphError<E>> {
        let mut shared_keys = HashSet::new();
        let mut added = HashMap::new();
        for (entry, _) in catalog.entries() {
            let GtxCatalogRecord::Environment { metadata, value } = entry else {
                continue;
            };
            let Some(name) = metadata.name.as_ref().filter(|name| !name.is_empty()) else {
                continue;
            };
            let key = (metadata.group_id, name.as_slice());
            if !shared_keys.insert(key) {
                continue;
            }
            if let Some(change) = self
                .shared
                .iter_mut()
                .find(|change| change.value.same_reference(value))
            {
                if let Some(map) = mapped(&change.after)? {
                    change.after = map;
                }
                continue;
            }
            let before = value.value();
            if let Some(after) = mapped(&before)? {
                added.insert(key, self.shared.len());
                self.shared.push(EnvironmentAdjusterEdit {
                    key: (key.0, key.1.to_vec()),
                    value: value.clone(),
                    before,
                    after,
                    targets: Vec::new(),
                });
            }
        }
        for property in environment_properties(branches) {
            if let Some(key) = environment_adjustment_key(&property) {
                let key = (key.0, key.1.as_slice());
                if let Some(index) = added.get(&key)
                    && let Some(target) = AdjustedEnvironment::capture(&property)
                {
                    self.shared[*index].targets.push(target);
                }
                if shared_keys.contains(&key) {
                    continue;
                }
            }
            let PointwiseParameterValue::Environment(before) = property.raw_value() else {
                continue;
            };
            if let Some(after) = mapped(&before)?
                && (include_bound
                    || property
                        .environment_binding_source()
                        .map_err(|error| GtxGraphError::Node(error.into()))?
                        .is_none())
            {
                self.local.push(LocalEnvironmentEdit {
                    property: property.downgrade(),
                    before,
                    after,
                });
            }
        }
        self.stage_paints(branches, |map, key| {
            if key.is_some_and(|key| shared_keys.contains(&key)) {
                Ok(None)
            } else {
                mapped(map)
            }
        })
    }

    fn stage_paints(
        &mut self,
        branches: &[Option<PointwiseNode<E>>],
        mut nested: impl FnMut(
            &EnvironmentRecord,
            Option<([u8; 16], &[u16])>,
        ) -> Result<Option<Arc<EnvironmentRecord>>, GtxGraphError<E>>,
    ) -> Result<(), GtxGraphError<E>> {
        let shared: HashMap<_, _> = self
            .shared
            .iter()
            .map(|change| ((change.key.0, change.key.1.as_slice()), &change.after))
            .collect();
        for filter in graph_nodes(branches).filter_map(|node| node.filter()) {
            let edit = EnvironmentPaintEdit::stage(&filter, |map, key| {
                if let Some(replacement) = key.and_then(|key| shared.get(&key)) {
                    return Ok((!Arc::ptr_eq(replacement, map)).then(|| Arc::clone(replacement)));
                }
                nested(map, key)
            })?;
            if let Some(edit) = edit {
                self.paints.push(edit);
            }
        }
        Ok(())
    }

    pub(super) fn publish(
        &self,
        direction: HistoryDirection,
    ) -> Result<AffectedEnvironments<E>, HistoryError<E>> {
        let undo = matches!(direction, HistoryDirection::Undo);
        let mut order: Vec<_> = (0..self.shared.len()).collect();
        order.sort_unstable_by_key(|&index| self.shared[index].value.lock_order());
        if order.windows(2).any(|pair| {
            self.shared[pair[0]]
                .value
                .same_reference(&self.shared[pair[1]].value)
        }) {
            return Err(HistoryError::InputChanged);
        }
        let mut locked: Vec<_> = order
            .iter()
            .map(|&index| (index, self.shared[index].value.lock()))
            .collect();
        for (index, current) in &locked {
            let change = &self.shared[*index];
            let expected = if undo { &change.after } else { &change.before };
            if !Arc::ptr_eq(current, expected) {
                return Err(HistoryError::InputChanged);
            }
        }
        let mut targets = Vec::with_capacity(self.shared.len());
        for change in &self.shared {
            let mut properties = Vec::with_capacity(change.targets.len());
            for entry in &change.targets {
                let property = entry.property.upgrade().ok_or(HistoryError::NullTarget)?;
                // A Wizard binding can refresh the receiver's stored map after
                // publication. Its adjustment record still identifies the link.
                if !matches!(
                    property.raw_value(),
                    PointwiseParameterValue::Environment(_)
                ) || property
                    .retained_adjustment()
                    .is_none_or(|current| !current.same_reference(&entry.adjustment))
                {
                    return Err(HistoryError::InputChanged);
                }
                properties.push(property);
            }
            targets.push(properties);
        }
        let local = self.local.iter().map(|entry| {
            let property = entry.property.upgrade().ok_or(HistoryError::NullTarget)?;
            let expected = if undo { &entry.after } else { &entry.before };
            if !matches!(property.raw_value(), PointwiseParameterValue::Environment(current) if Arc::ptr_eq(&current, expected)) {
                return Err(HistoryError::InputChanged);
            }
            Ok(property)
        }).collect::<Result<Vec<_>, _>>()?;
        let paints = EnvironmentPaintEdit::publish_all(&self.paints, undo)?;
        for (change, properties) in self.shared.iter().zip(&targets) {
            for (property, entry) in properties.iter().zip(&change.targets) {
                let value = if undo { &entry.before } else { &change.after };
                property
                    .write_environment(value.clone(), false)
                    .map_err(|error| HistoryError::Property(Box::new(error)))?;
            }
        }
        for (entry, property) in self.local.iter().zip(&local) {
            let value = if undo { &entry.before } else { &entry.after };
            property
                .write_environment(value.clone(), false)
                .map_err(|error| HistoryError::Property(Box::new(error)))?;
        }
        for (index, current) in &mut locked {
            let change = &self.shared[*index];
            **current = if undo { &change.before } else { &change.after }.clone();
        }
        Ok(AffectedEnvironments {
            properties: targets.into_iter().flatten().chain(local).collect(),
            paints,
        })
    }
}

fn notify_targets<E: 'static>(properties: &[PointwiseParameter<E>]) -> Result<(), HistoryError<E>> {
    let mut failure = None;
    for property in properties {
        if let Err(error) = property.notify_environment_change() {
            failure.get_or_insert(error);
        }
    }
    failure.map_or(Ok(()), |error| Err(HistoryError::Property(Box::new(error))))
}

impl<E: 'static> HistoryGraphEdit<E> for EnvironmentAdjusterBatch<E> {
    fn apply(&self, direction: HistoryDirection) -> Result<(), HistoryError<E>> {
        self.publish(direction)?.notify()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EnvironmentField, EnvironmentLayerKind};

    #[test]
    fn preset_history_follows_the_adjustment_link_when_a_wizard_refreshes_its_map() {
        let red = |value| {
            let mut map = EnvironmentRecord::default();
            map.add_layer(EnvironmentLayerKind::Color).unwrap();
            map.set_field(EnvironmentField::Red(0), value).unwrap();
            Arc::new(map)
        };
        let wizard = PointwiseFilter::<()>::new(PointwiseKind::WizardGroup).unwrap();
        wizard.add_wizard_environment_variable().unwrap();
        let source = wizard.wizard_dynamic_properties().unwrap().remove(0);
        let make = PointwiseFilter::new(PointwiseKind::MakeEnvironment).unwrap();
        wizard
            .node()
            .content(0)
            .unwrap()
            .set_node(Some(&make.node()))
            .unwrap();
        let receiver = make.parameter(0).unwrap();
        source.set_environment(red(0.25)).unwrap();
        receiver.set_binding_index_notifying(0).unwrap();
        let mut doc = Gtx375GraphDocument::new(vec![Some(wizard.node())]).unwrap();
        for (property, name) in [(&source, "Wizard"), (&receiver, "Receiver")] {
            doc.set_environment_adjuster(property, EnvironmentAdjusterLink::Create(name.into()))
                .unwrap();
        }
        let changes = [(&source, 0.5), (&receiver, 0.75)].map(|(property, channel)| {
            let adjuster = doc.environment_adjuster(property).unwrap();
            EnvironmentAdjusterEdit {
                key: (adjuster.group_id, adjuster.name),
                before: adjuster.value.value(),
                value: adjuster.value,
                after: red(channel),
                targets: vec![AdjustedEnvironment::capture(property).unwrap()],
            }
        });
        let batch = EnvironmentAdjusterBatch::new(Vec::from(changes));
        batch.apply(HistoryDirection::Redo).unwrap();
        assert!(
            matches!(receiver.raw_value(), PointwiseParameterValue::Environment(map)
                if map.field(EnvironmentField::Red(0)) == Some(0.5))
        );
        batch.apply(HistoryDirection::Undo).unwrap();
        assert_eq!(
            source
                .effective_environment()
                .unwrap()
                .field(EnvironmentField::Red(0)),
            Some(0.25)
        );
        assert_eq!(
            receiver
                .effective_environment()
                .unwrap()
                .field(EnvironmentField::Red(0)),
            Some(0.25)
        );
    }
}
