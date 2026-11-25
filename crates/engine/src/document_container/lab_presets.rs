use super::curve_adjusters::{self, CurveAdjusterBatch};
use super::environment_adjusters::{
    AffectedEnvironments, EnvironmentAdjusterBatch, MapReferenceValues, catching_group,
    enclosing_adjuster_group, graph_nodes,
};
use super::lab_preset_scope::capture;
use super::numeric_adjusters::{NumericAdjusterBatch, NumericCatalogUpdate};
use super::synthesis_adjusters::{PatchShapeInputs, SynthesisAdjusterBatch};
use super::*;
use crate::history::HistoryGraphEdit;
use crate::pointwise::reference_adjustment::{LabReferenceEdit, ReferenceReceiver};
use crate::{
    CurveRecord, EnvironmentRecord, HistoryDirection, HistoryError, PointwiseFilter,
    PointwiseParameter, ReferenceAdjusterPayload, ReferenceAdjusterRecord, ReferenceValueHandle,
    SharedGradientRecord, SynthesisCatalogValue, SynthesisRecord, ValueAdjusterPayload,
};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};

#[path = "lab_color_adjusters.rs"]
mod color_adjusters;
pub use color_adjusters::{
    ColorAdjusterLink, GradientAdjusterLink, LabColorAdjuster, LabColorValue,
};

#[derive(Clone, Debug)]
pub struct LabPreset {
    name: String,
    presence: Arc<AtomicBool>,
}

impl LabPreset {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn same_reference(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.presence, &other.presence)
    }
}

pub enum LabPresetOperation {
    Save(String),
    Replace(LabPreset),
    Remove(LabPreset),
    Apply(LabPreset),
}

#[non_exhaustive]
pub struct LabPresetSection {
    pub name: String,
    pub presets: Vec<LabPreset>,
    pub can_capture: bool,
    pub suggested_name: String,
}

struct Section<E> {
    group_id: [u8; 16],
    group: Option<PointwiseFilter<E>>,
    collection: Option<Arc<Mutex<GtxPresetCollection>>>,
}

pub(super) fn active_lab_group<E: 'static>(
    selected: Option<&PointwiseNode<E>>,
) -> Option<PointwiseFilter<E>> {
    selected.and_then(|node| catching_group(node).or_else(|| enclosing_adjuster_group(node)))
}

impl<E: 'static> Section<E> {
    fn new(selected: Option<&PointwiseNode<E>>) -> Self {
        let group = active_lab_group(selected);
        let collection = group.as_ref().and_then(PointwiseFilter::group_presets);
        let group_id = group
            .as_ref()
            .and_then(PointwiseFilter::group_guid)
            .unwrap_or_default();
        Self {
            group_id,
            group,
            collection,
        }
    }

    fn name(&self) -> String {
        let Some(group) = &self.group else {
            return "Document".into();
        };
        group
            .parameter(2)
            .ok()
            .and_then(|property| property.text().ok().flatten())
            .filter(|name| !name.is_empty())
            .map_or_else(|| "Group".into(), |name| String::from_utf16_lossy(&name))
    }

    fn groups(&self, doc: &Gtx375GraphDocument<E>) -> HashSet<[u8; 16]> {
        let roots = if let Some(group) = &self.group {
            Cow::Owned(
                group
                    .node()
                    .contents()
                    .iter()
                    .map(|slot| slot.node())
                    .collect(),
            )
        } else {
            doc.branches()
        };
        std::iter::once(self.group_id)
            .chain(graph_nodes(&roots).filter_map(|node| catching_group(&node)?.group_guid()))
            .collect()
    }
}

fn visible(collection: &GtxPresetCollection) -> impl Iterator<Item = &GtxPresetGroup> {
    collection
        .groups()
        .iter()
        .filter(|group| !group.is_hidden())
}

fn supported_kind(kind: GtxPresetKind) -> bool {
    matches!(
        kind,
        GtxPresetKind::Number
            | GtxPresetKind::Selection
            | GtxPresetKind::Dual
            | GtxPresetKind::Environment
            | GtxPresetKind::Curve
            | GtxPresetKind::Synthesis
            | GtxPresetKind::Color
            | GtxPresetKind::Gradient
    )
}

fn contains_supported_values(group: &GtxPresetGroup) -> bool {
    group
        .presets()
        .iter()
        .any(|record| !record.is_hidden() && supported_kind(record.kind()))
}

fn selected_preset<'a, E>(
    collection: &'a GtxPresetCollection,
    preset: &LabPreset,
) -> Result<&'a GtxPresetGroup, GtxGraphError<E>> {
    visible(collection)
        .find(|group| {
            Arc::ptr_eq(group.presence(), &preset.presence) && contains_supported_values(group)
        })
        .ok_or(GtxGraphError::LabPreset(
            "This Lab preset is no longer available.",
        ))
}

impl<E: 'static> Gtx375GraphDocument<E> {
    pub fn environment_lab_presets(&self, selected: Option<&PointwiseNode<E>>) -> LabPresetSection {
        self.lab_presets(selected)
    }

    pub fn lab_presets(&self, selected: Option<&PointwiseNode<E>>) -> LabPresetSection {
        let section = Section::new(selected);
        let collect = |collection: &GtxPresetCollection| {
            let names: HashSet<_> = visible(collection)
                .filter_map(GtxPresetGroup::name)
                .collect();
            let suggested_name = (1..)
                .map(|number| format!("Variation {number}"))
                .find(|name| !names.contains(name.encode_utf16().collect::<Vec<_>>().as_slice()))
                .expect("some name is unused");
            let presets = visible(collection)
                .filter(|group| contains_supported_values(group))
                .map(|group| LabPreset {
                    name: String::from_utf16_lossy(group.name().unwrap_or_default()),
                    presence: group.presence().clone(),
                })
                .collect();
            (presets, suggested_name)
        };
        let (presets, suggested_name) = if let Some(collection) = &section.collection {
            collect(&collection.lock().unwrap_or_else(|error| error.into_inner()))
        } else {
            collect(&self.presets)
        };
        let ids = section.groups(self);
        let can_capture = self.catalog.entries().any(|(record, _)| {
            let metadata = record.metadata();
            ids.contains(&metadata.group_id)
                && metadata.name.as_ref().is_some_and(|name| !name.is_empty())
                && (matches!(
                    record,
                    GtxCatalogRecord::Environment { .. }
                        | GtxCatalogRecord::Numeric { .. }
                        | GtxCatalogRecord::Synthesis { .. }
                ) || CurveAdjuster::read(record).is_some()
                    || ReferenceCatalogUpdate::is_supported(record))
                && GtxPresetKind::from_tag(metadata.type_tag).is_some_and(supported_kind)
        });
        LabPresetSection {
            name: section.name(),
            presets,
            can_capture,
            suggested_name,
        }
    }

    pub fn edit_environment_lab_preset(
        &mut self,
        selected: Option<&PointwiseNode<E>>,
        operation: LabPresetOperation,
    ) -> Result<(), GtxGraphError<E>> {
        self.edit_lab_preset(selected, operation)
    }

    pub fn edit_lab_preset(
        &mut self,
        selected: Option<&PointwiseNode<E>>,
        operation: LabPresetOperation,
    ) -> Result<(), GtxGraphError<E>> {
        let section = self.checked_lab_preset_section(selected)?;
        if let LabPresetOperation::Apply(preset) = operation {
            return self.apply_lab_preset(&section, &preset, &[]);
        }

        let records = if matches!(operation, LabPresetOperation::Remove(_)) {
            Vec::new()
        } else {
            self.capture_lab_preset(&section)
        };
        let edit = if let Some(collection) = section.collection {
            stage_catalog_edit(
                &mut collection.lock().unwrap_or_else(|error| error.into_inner()),
                section.group_id,
                operation,
                records,
            )?
        } else {
            stage_catalog_edit(&mut self.presets, section.group_id, operation, records)?
        };
        edit.apply(HistoryDirection::Redo)
            .map_err(GtxGraphError::History)?;
        let label = edit.label;
        self.record_lab_edit(label, edit)
            .map_err(GtxGraphError::History)
    }

    /// Finds the upstream graph inputs needed to clamp the preset's connected Patches receivers.
    pub fn connected_patch_inputs_for_lab_preset(
        &self,
        selected: Option<&PointwiseNode<E>>,
        preset: &LabPreset,
    ) -> Result<Vec<PointwiseNode<E>>, GtxGraphError<E>> {
        let section = self.checked_lab_preset_section(selected)?;
        let values = self.prepare_selected_lab_preset(&section, preset)?;
        let keys: Vec<_> = values
            .numeric
            .iter()
            .filter_map(NumericCatalogUpdate::patch_source_key)
            .collect();
        Ok(
            super::synthesis_adjusters::connected_patch_adjuster_inputs_for_keys(
                &self.branches(),
                &keys,
            ),
        )
    }

    /// Applies all preset values together, using raw upstream dimensions for connected Patches.
    pub fn apply_lab_preset_with_patch_input_sizes(
        &mut self,
        selected: Option<&PointwiseNode<E>>,
        preset: &LabPreset,
        input_sizes: &[(u64, crate::ImageGeometry)],
    ) -> Result<(), GtxGraphError<E>> {
        let section = self.checked_lab_preset_section(selected)?;
        self.apply_lab_preset(&section, preset, input_sizes)
    }

    fn checked_lab_preset_section(
        &self,
        selected: Option<&PointwiseNode<E>>,
    ) -> Result<Section<E>, GtxGraphError<E>> {
        if selected.is_some_and(|selected| {
            !graph_nodes(&self.branches()).any(|node| node.same_reference(selected))
        }) {
            return Err(GtxGraphError::LabPreset(
                "The selected Lab section changed.",
            ));
        }
        Ok(Section::new(selected))
    }

    fn capture_lab_preset(&self, section: &Section<E>) -> Vec<GtxPresetRecord> {
        let groups = section.groups(self);
        self.catalog
            .entries()
            .filter_map(|(entry, _)| {
                let metadata = entry.metadata();
                let kind = GtxPresetKind::from_tag(metadata.type_tag)?;
                if !groups.contains(&metadata.group_id) || !supported_kind(kind) {
                    return None;
                }
                capture(entry)
            })
            .collect()
    }

    fn prepare_selected_lab_preset(
        &self,
        section: &Section<E>,
        preset: &LabPreset,
    ) -> Result<LabValues, GtxGraphError<E>> {
        let prepare = |collection: &GtxPresetCollection| {
            self.prepare_lab_values(selected_preset(collection, preset)?)
        };
        if let Some(collection) = &section.collection {
            prepare(&collection.lock().unwrap_or_else(|error| error.into_inner()))
        } else {
            prepare(&self.presets)
        }
    }

    fn apply_lab_preset(
        &mut self,
        section: &Section<E>,
        preset: &LabPreset,
        input_sizes: &[(u64, crate::ImageGeometry)],
    ) -> Result<(), GtxGraphError<E>> {
        let mut values = self.prepare_selected_lab_preset(section, preset)?;
        if values.maps.is_empty()
            && values.numeric.is_empty()
            && values.synthesis.is_empty()
            && values.curves.is_empty()
            && values.references.is_empty()
        {
            return Err(GtxGraphError::LabPreset(
                "This preset has no matching adjusters in the document.",
            ));
        }
        values.synthesis_input_sizes = input_sizes.to_vec();
        self.commit_lab_values_with_references(
            &format!("Apply {} Lab Preset", preset.name()),
            values,
        )
    }

    fn commit_lab_values_with_references(
        &mut self,
        label: &str,
        mut values: LabValues,
    ) -> Result<(), GtxGraphError<E>> {
        let references = std::mem::take(&mut values.references);
        let mut batch = self.prepare_lab_batch(values, &references)?;
        batch.reference_catalog = references
            .into_iter()
            .map(|update| {
                let after = self.catalog.stage_replacement(update.after, &update.before);
                ReferenceCatalogSwap {
                    before: update.before,
                    after,
                }
            })
            .collect();
        self.commit_lab_batch(label, batch)
    }

    pub(super) fn commit_lab_values(
        &self,
        label: &str,
        values: LabValues,
    ) -> Result<(), GtxGraphError<E>> {
        self.commit_lab_batch(label, self.prepare_lab_batch(values, &[])?)
    }

    fn prepare_lab_batch(
        &self,
        values: LabValues,
        reference_updates: &[ReferenceCatalogUpdate],
    ) -> Result<LabPresetBatch<E>, GtxGraphError<E>> {
        let branches = self.branches();
        let mut environment = self.stage_environment_adjuster_maps(values.maps);
        let mut references =
            MapReferenceValues::from_entries(reference_updates.iter().map(|update| &update.after));
        environment.stage_receiver_values(
            &branches,
            &self.catalog,
            &values.numeric,
            references.as_mut(),
        )?;
        let synthesis = SynthesisAdjusterBatch::prepare(
            &branches,
            &self.catalog,
            &mut environment,
            values.synthesis,
            &values.numeric,
            references.as_ref(),
            PatchShapeInputs::Authoring(&values.synthesis_input_sizes),
        )?;
        Ok(LabPresetBatch {
            environment,
            numeric: NumericAdjusterBatch::prepare(&branches, values.numeric)?,
            synthesis,
            curves: CurveAdjusterBatch::prepare(&branches, values.curves)?,
            references: prepare_reference_receivers(&branches, references.as_ref())?,
            reference_catalog: Vec::new(),
        })
    }

    fn commit_lab_batch(
        &self,
        label: &str,
        batch: LabPresetBatch<E>,
    ) -> Result<(), GtxGraphError<E>> {
        let affected = batch
            .publish(HistoryDirection::Redo)
            .map_err(GtxGraphError::History)?;
        self.record_lab_edit(label, batch)
            .and(affected.notify())
            .map_err(GtxGraphError::History)
    }

    fn prepare_lab_values(&self, preset: &GtxPresetGroup) -> Result<LabValues, GtxGraphError<E>> {
        let mut result = LabValues::default();
        for record in preset.presets().iter().filter(|record| !record.is_hidden()) {
            let Some((entry, marker)) = self.catalog.entries().find(|(entry, _)| {
                let metadata = entry.metadata();
                metadata.group_id == record.group_id()
                    && metadata.type_tag == record.type_tag()
                    && metadata.name.as_deref() == record.name()
            }) else {
                continue;
            };
            if let GtxCatalogRecord::Numeric { metadata, value } = entry {
                let update = NumericCatalogUpdate::prepare(metadata, value, record)?;
                if let Some(index) = result
                    .numeric
                    .iter()
                    .position(|previous| previous.identity() == update.identity())
                {
                    result.numeric.remove(index);
                }
                result.numeric.push(update);
                continue;
            }
            if let GtxCatalogRecord::Synthesis { value, .. } = entry {
                let GtxPresetValue::Synthesis(texture) = record.value() else {
                    return Err(GtxGraphError::LabPreset(
                        "The matching adjuster has an incompatible value.",
                    ));
                };
                result.synthesis.push((value.clone(), texture.clone()));
                continue;
            }
            if let Some(adjuster) = CurveAdjuster::read(entry) {
                let GtxPresetValue::Reference(ReferenceValueHandle::Curve(curve)) = record.value()
                else {
                    return Err(GtxGraphError::LabPreset(
                        "The matching Curve adjuster has an incompatible value.",
                    ));
                };
                let value = curve.snapshot();
                if let Some((_, pending)) = result
                    .curves
                    .iter_mut()
                    .find(|(previous, _)| previous.same_reference(&adjuster))
                {
                    *pending = value;
                } else {
                    result.curves.push((adjuster, value));
                }
                continue;
            }
            if let Some(update) = ReferenceCatalogUpdate::prepare(entry, marker, record)? {
                if let Some(previous) = result
                    .references
                    .iter_mut()
                    .find(|previous| Arc::ptr_eq(&previous.before, marker))
                {
                    *previous = update;
                } else {
                    result.references.push(update);
                }
                continue;
            }
            let GtxPresetValue::Environment(map) = record.value() else {
                return Err(GtxGraphError::LabPreset(
                    "This preset also controls a live adjuster type that Mycela cannot apply yet.",
                ));
            };
            if !matches!(entry, GtxCatalogRecord::Environment { .. }) {
                return Err(GtxGraphError::LabPreset(
                    "The matching adjuster has an incompatible value.",
                ));
            }
            let Some(adjuster) = self
                .adjusters_in_group(record.group_id())
                .find(|adjuster| Some(adjuster.name()) == record.name())
            else {
                continue;
            };
            let map = Arc::new((**map).clone());
            if let Some((_, pending)) = result
                .maps
                .iter_mut()
                .find(|(previous, _)| previous.same_reference(&adjuster))
            {
                *pending = map;
            } else {
                result.maps.push((adjuster, map));
            }
        }
        Ok(result)
    }
}

#[derive(Default)]
pub(super) struct LabValues {
    maps: Vec<(EnvironmentAdjuster, Arc<EnvironmentRecord>)>,
    pub(super) numeric: Vec<NumericCatalogUpdate>,
    pub(super) synthesis_input_sizes: Vec<(u64, crate::ImageGeometry)>,
    synthesis: Vec<(SynthesisCatalogValue, SynthesisRecord)>,
    curves: Vec<(CurveAdjuster, Arc<CurveRecord>)>,
    references: Vec<ReferenceCatalogUpdate>,
}

struct ReferenceCatalogUpdate {
    before: Arc<AtomicBool>,
    after: GtxCatalogRecord,
}

impl ReferenceCatalogUpdate {
    fn is_supported(entry: &GtxCatalogRecord) -> bool {
        match entry {
            GtxCatalogRecord::Value(value) if value.metadata.type_tag == 6 => {
                matches!(value.payload, ValueAdjusterPayload::Color { .. })
            }
            GtxCatalogRecord::Reference(value) if value.metadata.type_tag == 7 => {
                matches!(value.payload, ReferenceAdjusterPayload::Gradient { .. })
            }
            _ => false,
        }
    }

    fn prepare<E>(
        entry: &GtxCatalogRecord,
        marker: &Arc<AtomicBool>,
        preset: &GtxPresetRecord,
    ) -> Result<Option<Self>, GtxGraphError<E>> {
        if !Self::is_supported(entry) {
            return Ok(None);
        }
        Self::from_value(entry, marker, preset.value()).map(Some)
    }

    fn from_value<E>(
        entry: &GtxCatalogRecord,
        marker: &Arc<AtomicBool>,
        value: &GtxPresetValue,
    ) -> Result<Self, GtxGraphError<E>> {
        let metadata = entry.metadata().clone();
        let after = match (metadata.type_tag, value) {
            (6, GtxPresetValue::Color(value)) => {
                GtxCatalogRecord::Value(Box::new(ValueAdjusterRecord {
                    metadata,
                    payload: ValueAdjusterPayload::Color {
                        value: *value,
                        previous: *value,
                    },
                }))
            }
            (7, GtxPresetValue::Reference(ReferenceValueHandle::Gradient(value))) => {
                let snapshot = value
                    .live()
                    .wire_record()
                    .map_err(|error| GtxGraphError::Node(PointwiseError::Gradient(error).into()))?;
                let value = SharedGradientRecord::new(snapshot);
                GtxCatalogRecord::Reference(Box::new(ReferenceAdjusterRecord {
                    metadata,
                    payload: ReferenceAdjusterPayload::Gradient {
                        current: Some(value.clone()),
                        previous: Some(value),
                    },
                }))
            }
            _ => {
                return Err(GtxGraphError::LabPreset(
                    "The matching adjuster has an incompatible value.",
                ));
            }
        };
        Ok(Self {
            before: marker.clone(),
            after,
        })
    }
}

struct ReferenceCatalogSwap {
    before: Arc<AtomicBool>,
    after: Arc<AtomicBool>,
}

pub(super) fn prepare_reference_receivers<E: 'static>(
    branches: &[Option<PointwiseNode<E>>],
    values: Option<&MapReferenceValues<'_>>,
) -> Result<Vec<LabReferenceEdit<E>>, GtxGraphError<E>> {
    let mut changes = Vec::new();
    if let Some(values) = values {
        for node in graph_nodes(branches) {
            changes.extend(
                LabReferenceEdit::prepare_node(&node, |key| values.graph_value(key))
                    .map_err(|error| GtxGraphError::Node(error.into()))?,
            );
        }
    }
    Ok(changes)
}

struct LabPresetBatch<E> {
    environment: EnvironmentAdjusterBatch<E>,
    numeric: NumericAdjusterBatch<E>,
    synthesis: SynthesisAdjusterBatch<E>,
    curves: CurveAdjusterBatch<E>,
    references: Vec<LabReferenceEdit<E>>,
    reference_catalog: Vec<ReferenceCatalogSwap>,
}

pub(crate) struct AffectedProperties<E> {
    pub(super) environment: AffectedEnvironments<E>,
    pub(super) numeric: Vec<crate::pointwise::numeric_adjustment::NumericReceiver<E>>,
    pub(super) synthesis: Vec<PointwiseFilter<E>>,
    pub(super) curves: Vec<PointwiseParameter<E>>,
    pub(super) references: Vec<ReferenceReceiver<E>>,
}

impl<E: 'static> AffectedProperties<E> {
    pub(super) fn numeric_only(
        numeric: Vec<crate::pointwise::numeric_adjustment::NumericReceiver<E>>,
    ) -> Self {
        Self {
            environment: AffectedEnvironments::default(),
            numeric,
            synthesis: Vec::new(),
            curves: Vec::new(),
            references: Vec::new(),
        }
    }

    pub(crate) fn notify(self) -> Result<(), HistoryError<E>> {
        let mut result = Ok(());
        for property in &self.numeric {
            result = result.and(
                property
                    .normalize()
                    .map_err(|error| HistoryError::Property(Box::new(error))),
            );
        }
        result = result.and(self.environment.notify());
        result = result.and(curve_adjusters::notify(&self.curves));
        for receiver in self.references {
            result = result.and(receiver.notify());
        }
        for property in self.numeric {
            result = result.and(
                property
                    .notify()
                    .map_err(|error| HistoryError::Property(Box::new(error))),
            );
        }
        for filter in self.synthesis {
            result = result.and(
                filter
                    .notify_synthesis_receiver_change()
                    .map_err(|error| HistoryError::Property(Box::new(error))),
            );
        }
        result
    }
}

impl<E: 'static> LabPresetBatch<E> {
    fn publish(
        &self,
        direction: HistoryDirection,
    ) -> Result<AffectedProperties<E>, HistoryError<E>> {
        let undo = direction == HistoryDirection::Undo;
        if self.reference_catalog.iter().any(|swap| {
            swap.before.load(Ordering::Acquire) == undo
                || swap.after.load(Ordering::Acquire) != undo
        }) {
            return Err(HistoryError::InputChanged);
        }
        let ((((environment, references), curves), synthesis), numeric) =
            self.numeric.publish(direction, || {
                self.synthesis.publish(direction, || {
                    self.curves.publish(direction, || {
                        LabReferenceEdit::publish_with(&self.references, undo, || {
                            self.environment.publish(direction)
                        })
                    })
                })
            })?;
        for swap in &self.reference_catalog {
            swap.before.store(undo, Ordering::Release);
            swap.after.store(!undo, Ordering::Release);
        }
        Ok(AffectedProperties {
            environment,
            numeric,
            synthesis,
            curves,
            references,
        })
    }
}

impl<E: 'static> HistoryGraphEdit<E> for LabPresetBatch<E> {
    fn apply(&self, direction: HistoryDirection) -> Result<(), HistoryError<E>> {
        self.publish(direction)?.notify()
    }
}

fn stage_catalog_edit<E>(
    collection: &mut GtxPresetCollection,
    group_id: [u8; 16],
    operation: LabPresetOperation,
    records: Vec<GtxPresetRecord>,
) -> Result<LabPresetCatalogEdit, GtxGraphError<E>> {
    let (name, retired, label) = match operation {
        LabPresetOperation::Save(name) => {
            let name: Vec<_> = name.trim().encode_utf16().collect();
            if name.is_empty() {
                return Err(GtxGraphError::LabPreset("Enter a name for the Lab preset."));
            }
            if visible(collection).any(|group| group.name() == Some(&name)) {
                return Err(GtxGraphError::LabPreset(
                    "A Lab preset already uses that name in this section.",
                ));
            }
            (name, None, "Save Lab Preset")
        }
        LabPresetOperation::Replace(preset) => {
            let existing = selected_preset(collection, &preset)?;
            if existing
                .presets()
                .iter()
                .any(|record| !supported_kind(record.kind()))
            {
                return Err(GtxGraphError::LabPreset(
                    "This preset contains other adjuster types. Save the current values under a new name.",
                ));
            }
            (
                existing.name().unwrap_or_default().to_vec(),
                Some(existing.presence().clone()),
                "Replace Lab Preset",
            )
        }
        LabPresetOperation::Remove(preset) => {
            let retired = selected_preset(collection, &preset)?.presence().clone();
            return Ok(LabPresetCatalogEdit {
                label: "Remove Lab Preset",
                added: None,
                retired: Some(retired),
            });
        }
        LabPresetOperation::Apply(_) => unreachable!("apply uses the live catalog"),
    };
    if records.is_empty() {
        return Err(GtxGraphError::LabPreset(
            "Link an adjuster in this Lab section first.",
        ));
    }
    let snapshot = GtxPresetGroup::lab_snapshot(name, group_id, records);
    let added = snapshot.presence().clone();
    collection.stage_lab_snapshot(snapshot, retired.as_ref());
    Ok(LabPresetCatalogEdit {
        label,
        added: Some(added),
        retired,
    })
}

struct LabPresetCatalogEdit {
    label: &'static str,
    added: Option<Arc<AtomicBool>>,
    retired: Option<Arc<AtomicBool>>,
}

impl<E> HistoryGraphEdit<E> for LabPresetCatalogEdit {
    fn apply(&self, direction: HistoryDirection) -> Result<(), HistoryError<E>> {
        let undo = matches!(direction, HistoryDirection::Undo);
        if self
            .added
            .as_ref()
            .is_some_and(|present| present.load(Ordering::Acquire) != undo)
            || self
                .retired
                .as_ref()
                .is_some_and(|present| present.load(Ordering::Acquire) == undo)
        {
            return Err(HistoryError::InputChanged);
        }
        if let Some(present) = &self.added {
            present.store(!undo, Ordering::Release);
        }
        if let Some(present) = &self.retired {
            present.store(undo, Ordering::Release);
        }
        Ok(())
    }
}
