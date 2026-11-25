//! Composite Curve receivers of the existing tag-8 Lab catalog value.
use super::environment_adjusters::{enclosing_adjuster_group, graph_nodes};
use super::*;
use crate::history::HistoryGraphEdit;
use crate::pointwise::property::WeakPointwiseParameter;
use crate::{
    CurveRecord, HistoryDirection, HistoryError, PointwiseParameter, PointwiseParameterValue,
    ReferenceAdjusterKind, ReferenceAdjusterPayload, ReferenceAdjusterRecord, SharedCurve,
    SharedCurveRecord, SharedPropertyAdjustment,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

type Key = ([u8; 16], Vec<u16>);

#[derive(Clone, Debug)]
pub struct CurveAdjuster {
    key: Key,
    value: SharedCurveRecord,
}

impl CurveAdjuster {
    pub fn name(&self) -> &[u16] {
        &self.key.1
    }

    pub fn same_reference(&self, other: &Self) -> bool {
        self.key == other.key && self.value.same_reference(&other.value)
    }

    pub(super) fn catalog_record(&self, metadata: AdjusterMetadataRecord) -> GtxCatalogRecord {
        GtxCatalogRecord::Reference(Box::new(ReferenceAdjusterRecord {
            metadata,
            payload: ReferenceAdjusterPayload::Curve {
                current: Some(self.value.clone()),
                previous: Some(self.value.clone()),
            },
        }))
    }

    pub(super) fn detached_catalog_record(
        &self,
        metadata: AdjusterMetadataRecord,
    ) -> GtxCatalogRecord {
        Self {
            key: self.key.clone(),
            value: self.value.detached_copy(),
        }
        .catalog_record(metadata)
    }

    pub(super) fn materialize<E>(&self) -> Result<SharedCurve<E>, GtxGraphError<E>> {
        to_live(&self.value.snapshot())
    }

    fn live_value<E: 'static>(
        &self,
        branches: &[Option<PointwiseNode<E>>],
    ) -> Result<SharedCurve<E>, GtxGraphError<E>> {
        match curve_properties(branches)
            .filter(|property| adjustment_key(property).as_ref() == Some(&self.key))
            .find_map(|property| CurveState::read(&property)?.curve)
        {
            Some(curve) => Ok(curve),
            None => self.materialize(),
        }
    }

    pub(super) fn renamed_receivers<E: 'static>(
        &self,
        branches: &[Option<PointwiseNode<E>>],
        name: &[u16],
        destination: Option<&Self>,
    ) -> Result<Vec<CurveChange<E>>, GtxGraphError<E>> {
        let next = destination
            .map(|destination| destination.live_value(branches))
            .transpose()?;
        curve_properties(branches)
            .filter(|property| adjustment_key(property).as_ref() == Some(&self.key))
            .map(|property| {
                let before = CurveState::read(&property).expect("traversed Curve property");
                let adjustment = property
                    .new_curve_adjustment(name, self.key.0)
                    .map_err(|error| GtxGraphError::Node(NodeWireError::Property(error)))?;
                let after = CurveState {
                    adjustment: Some(adjustment),
                    curve: next.clone().or_else(|| before.curve.clone()),
                };
                Ok(CurveChange {
                    property: property.downgrade(),
                    before,
                    after,
                })
            })
            .collect()
    }

    pub(super) fn read(entry: &GtxCatalogRecord) -> Option<Self> {
        let record = entry.as_reference()?;
        let ReferenceAdjusterPayload::Curve {
            current: Some(value),
            ..
        } = &record.payload
        else {
            return None;
        };
        let name = record
            .metadata
            .name
            .as_ref()
            .filter(|name| !name.is_empty())?;
        Some(Self {
            key: (record.metadata.group_id, name.clone()),
            value: value.clone(),
        })
    }
}

#[derive(Clone)]
pub enum CurveAdjusterLink {
    Create(String),
    Existing(CurveAdjuster),
    Unlink,
}

impl<E: 'static> Gtx375GraphDocument<E> {
    pub fn curve_adjuster(&self, property: &PointwiseParameter<E>) -> Option<CurveAdjuster> {
        let key = adjustment_key(property)?;
        self.curve_adjusters(key.0)
            .find(|adjuster| adjuster.key == key)
    }

    pub fn available_curve_adjusters(
        &self,
        property: &PointwiseParameter<E>,
    ) -> Vec<CurveAdjuster> {
        let group = adjuster_scope(property);
        let mut adjusters: Vec<_> = self
            .catalog
            .entries()
            .filter(|(entry, _)| entry.metadata().group_id == group)
            .filter_map(|(entry, _)| Some((entry.metadata().order, CurveAdjuster::read(entry)?)))
            .collect();
        adjusters.sort_by_cached_key(|(order, adjuster)| {
            let name = String::from_utf16_lossy(adjuster.name());
            super::lab_adjuster_metadata::display_order(
                *order,
                &name,
                ReferenceAdjusterKind::Curve.tag(),
            )
        });
        adjusters
            .into_iter()
            .map(|(_, adjuster)| adjuster)
            .collect()
    }

    fn curve_adjusters(&self, group: [u8; 16]) -> impl Iterator<Item = CurveAdjuster> {
        self.catalog.entries().filter_map(move |(entry, _)| {
            (entry.metadata().group_id == group)
                .then(|| CurveAdjuster::read(entry))
                .flatten()
        })
    }

    /// Link a Curve to the current Lab section. Disconnecting preserves the last value.
    pub fn set_curve_adjuster(
        &mut self,
        property: &PointwiseParameter<E>,
        link: CurveAdjusterLink,
    ) -> Result<bool, GtxGraphError<E>> {
        if !property.is_adjustable()
            || !curve_properties(&self.branches())
                .any(|candidate| candidate.same_reference(property))
        {
            return Err(GtxGraphError::CurveAdjuster(
                "This Curve is no longer available.",
            ));
        }
        let group = adjuster_scope(property);
        let old_key = adjustment_key(property);
        let mut before = CurveState::read(property).expect("validated Curve property");
        let (destination, created) = match link {
            CurveAdjusterLink::Create(name) => {
                let name: Vec<u16> = name.trim().encode_utf16().collect();
                if name.is_empty() {
                    return Err(GtxGraphError::CurveAdjuster(
                        "Enter a name for the Curve adjuster.",
                    ));
                }
                if self
                    .curve_adjusters(group)
                    .any(|current| current.key.1 == name)
                {
                    return Err(GtxGraphError::CurveAdjuster(
                        "A Curve adjuster already uses this name in the Lab section.",
                    ));
                }
                let curve = property.curve().map_err(slot_edit_error)?;
                before.curve = Some(curve.clone());
                let value = SharedCurveRecord::new(to_record(&curve)?);
                let adjuster = CurveAdjuster {
                    key: (group, name),
                    value,
                };
                (Some(adjuster), true)
            }
            CurveAdjusterLink::Existing(selected) => {
                let current = self
                    .curve_adjusters(group)
                    .find(|candidate| candidate.same_reference(&selected))
                    .ok_or(GtxGraphError::CurveAdjuster(
                        "This adjuster is no longer available in the Lab section.",
                    ))?;
                if old_key.as_ref() == Some(&current.key) {
                    return Ok(false);
                }
                (Some(current), false)
            }
            CurveAdjusterLink::Unlink => {
                if old_key.is_none() {
                    return Ok(false);
                }
                (None, false)
            }
        };
        let after = if let Some(adjuster) = &destination {
            let replacement = if created {
                before.curve.clone()
            } else {
                Some(adjuster.live_value(&self.branches())?)
            };
            CurveState {
                adjustment: Some(
                    property
                        .new_curve_adjustment(&adjuster.key.1, group)
                        .map_err(|error| GtxGraphError::Node(NodeWireError::Property(error)))?,
                ),
                curve: replacement,
            }
        } else {
            CurveState {
                adjustment: None,
                curve: before.curve.clone(),
            }
        };
        let retired = old_key.and_then(|key| {
            if curve_properties(&self.branches()).any(|target| {
                !target.same_reference(property) && adjustment_key(&target).as_ref() == Some(&key)
            }) {
                return None;
            }
            self.catalog.entries().find_map(|(entry, active)| {
                CurveAdjuster::read(entry)
                    .is_some_and(|adjuster| adjuster.key == key)
                    .then(|| active.clone())
            })
        });
        let added = destination.filter(|_| created).map(|adjuster| {
            let order = self
                .catalog
                .entries()
                .filter(|(entry, _)| entry.metadata().group_id == group)
                .map(|(entry, _)| entry.metadata().order)
                .max()
                .map_or(0, |order| order.saturating_add(1));
            self.catalog
                .stage(adjuster.catalog_record(AdjusterMetadataRecord {
                    name: Some(adjuster.key.1.clone()),
                    group_id: group,
                    type_tag: ReferenceAdjusterKind::Curve.tag(),
                    order,
                    loaded: false,
                }))
        });
        let label = if created {
            "Create Curve Adjuster"
        } else if after.adjustment.is_some() {
            "Link Curve Adjuster"
        } else {
            "Unlink Curve Adjuster"
        };
        let edit = CurveAdjusterEdit {
            changes: vec![CurveChange {
                property: property.downgrade(),
                before,
                after,
            }],
            record: None,
            added,
            retired,
        };
        self.commit_curve_edit(label, edit)?;
        Ok(true)
    }

    pub fn apply_composite_curve_preset(
        &self,
        property: &PointwiseParameter<E>,
        expected: &SharedCurve<E>,
        preset: &SharedCurve<E>,
        insert: bool,
    ) -> Result<(), GtxGraphError<E>> {
        let next = expected
            .with_preset_layers(preset, insert)
            .map_err(|error| slot_edit_error(error.into()))?;
        let label = if insert {
            "Insert Curve Preset"
        } else {
            "Replace Curve Layers"
        };
        self.edit_composite_curve(property, expected, next, label)
    }

    /// Publish an edited composite through the Lab catalog when it has a link.
    pub fn edit_composite_curve(
        &self,
        property: &PointwiseParameter<E>,
        expected: &SharedCurve<E>,
        next: SharedCurve<E>,
        label: &str,
    ) -> Result<(), GtxGraphError<E>> {
        let Some(key) = adjustment_key(property) else {
            return property
                .edit_curve_with_label(expected, next, label)
                .map_err(slot_edit_error);
        };
        let adjuster = self
            .curve_adjusters(key.0)
            .find(|adjuster| adjuster.key == key)
            .ok_or(GtxGraphError::CurveAdjuster(
                "The linked Curve adjuster is missing.",
            ))?;
        let branches = self.branches();
        // The same shared Curve root is published to each linked node.
        let mut target_present = false;
        let mut changes = Vec::new();
        for candidate in curve_properties(&branches) {
            if adjustment_key(&candidate).as_ref() != Some(&key) {
                continue;
            }
            let before = CurveState::read(&candidate).expect("traversed Curve property");
            if !before
                .curve
                .as_ref()
                .is_some_and(|curve| curve.same_reference(expected))
            {
                return Err(GtxGraphError::History(HistoryError::InputChanged));
            }
            target_present |= candidate.same_reference(property);
            let after = CurveState {
                adjustment: before.adjustment.clone(),
                curve: Some(next.clone()),
            };
            changes.push(CurveChange {
                property: candidate.downgrade(),
                before,
                after,
            });
        }
        if !target_present {
            return Err(GtxGraphError::History(HistoryError::InputChanged));
        }
        if next
            .source_equals_object(crate::SourceObject::Curve(expected))
            .map_err(|error| slot_edit_error(error.into()))?
        {
            return Ok(());
        }
        let record = CurveRecordChange {
            before: adjuster.value.snapshot(),
            after: Arc::new(to_record(&next)?),
            value: adjuster.value,
        };
        self.commit_curve_edit(
            label,
            CurveAdjusterEdit {
                changes,
                record: Some(record),
                added: None,
                retired: None,
            },
        )
    }

    fn commit_curve_edit(
        &self,
        label: &str,
        edit: CurveAdjusterEdit<E>,
    ) -> Result<(), GtxGraphError<E>> {
        let affected = edit
            .publish(HistoryDirection::Redo)
            .map_err(GtxGraphError::History)?;
        self.record_lab_edit(label, edit)
            .and(notify(&affected))
            .map_err(GtxGraphError::History)
    }
}

fn to_record<E>(curve: &SharedCurve<E>) -> Result<CurveRecord, GtxGraphError<E>> {
    let mut writer = BinaryWriter::default();
    curve
        .save(&mut writer)
        .map_err(|error| slot_edit_error(error.into()))?;
    let mut record = CurveRecord::new(0., 1., false);
    record
        .load(&mut BinaryCursor::new(writer.bytes())?, 3.83)
        .map_err(|error| GtxReadError::ReferenceCatalog(error.into()))?;
    Ok(record)
}

fn to_live<E>(record: &CurveRecord) -> Result<SharedCurve<E>, GtxGraphError<E>> {
    let mut writer = BinaryWriter::default();
    record
        .save(&mut writer)
        .map_err(|error| GtxReadError::ReferenceCatalog(error.into()))?;
    let curve = SharedCurve::new(0., 1., false);
    curve
        .load(&mut BinaryCursor::new(writer.bytes())?, 3.83)
        .map_err(|error| slot_edit_error(error.into()))?;
    Ok(curve)
}

fn adjuster_scope<E: 'static>(property: &PointwiseParameter<E>) -> [u8; 16] {
    enclosing_adjuster_group(&property.owner().node())
        .and_then(|group| group.group_guid())
        .unwrap_or_default()
}

pub(super) fn adjustment_key<E>(property: &PointwiseParameter<E>) -> Option<Key> {
    let adjustment = property.retained_adjustment()?;
    if adjustment.property_type() != ReferenceAdjusterKind::Curve.tag()
        || !adjustment.is_active().ok()?
    {
        return None;
    }
    let name = adjustment.names().entries().into_iter().next().flatten()?;
    (!name.is_empty()).then_some((adjustment.guid(), name))
}

pub(super) fn curve_properties<E: 'static>(
    branches: &[Option<PointwiseNode<E>>],
) -> impl Iterator<Item = PointwiseParameter<E>> {
    graph_nodes(branches)
        .filter_map(|node| node.filter())
        .flat_map(|filter| filter.retained_parameters())
        .filter(|property| matches!(property.raw_value(), PointwiseParameterValue::Curve(_)))
}

pub(super) fn restore_curve_catalog<E: 'static>(
    branches: &[Option<PointwiseNode<E>>],
    catalog: &[GtxCatalogRecord],
) -> Result<(), GtxGraphError<E>> {
    let mut values = HashMap::new();
    for adjuster in catalog.iter().filter_map(CurveAdjuster::read) {
        values.entry(adjuster.key).or_insert((adjuster.value, None));
    }
    if values.is_empty() {
        return Ok(());
    }
    for property in curve_properties(branches) {
        let Some((record, curve)) = adjustment_key(&property).and_then(|key| values.get_mut(&key))
        else {
            continue;
        };
        if curve.is_none() {
            *curve = Some(to_live(&record.snapshot())?);
        }
        property.set_curve(curve.clone()).map_err(slot_edit_error)?;
    }
    Ok(())
}

struct CurveState<E> {
    adjustment: Option<SharedPropertyAdjustment<E>>,
    curve: Option<SharedCurve<E>>,
}

impl<E> CurveState<E> {
    fn read(property: &PointwiseParameter<E>) -> Option<Self> {
        let PointwiseParameterValue::Curve(curve) = property.raw_value() else {
            return None;
        };
        Some(Self {
            adjustment: property.retained_adjustment(),
            curve,
        })
    }

    fn replace(&self, property: &PointwiseParameter<E>, next: &Self) -> bool {
        property.replace_curve_adjustment(
            (self.adjustment.as_ref(), self.curve.as_ref()),
            (next.adjustment.clone(), next.curve.clone()),
        )
    }
}

pub(super) struct CurveChange<E> {
    property: WeakPointwiseParameter<E>,
    before: CurveState<E>,
    after: CurveState<E>,
}

impl<E> CurveChange<E> {
    fn states(&self, undo: bool) -> (&CurveState<E>, &CurveState<E>) {
        if undo {
            (&self.after, &self.before)
        } else {
            (&self.before, &self.after)
        }
    }
}

impl<E: 'static> CurveChange<E> {
    pub(super) fn relocate(
        property: &PointwiseParameter<E>,
        group: [u8; 16],
        curve: &SharedCurve<E>,
    ) -> Option<Self> {
        let before = CurveState::read(property)?;
        let adjustment = before.adjustment.as_ref()?;
        let mut parts = adjustment.capture_parts();
        let adjustment = if parts.guid == group {
            if before
                .curve
                .as_ref()
                .is_some_and(|current| current.same_reference(curve))
            {
                return None;
            }
            adjustment.clone()
        } else {
            parts.guid = group;
            SharedPropertyAdjustment::from_capture_parts(parts)
        };
        let after = CurveState {
            adjustment: Some(adjustment),
            curve: Some(curve.clone()),
        };
        Some(Self {
            property: property.downgrade(),
            before,
            after,
        })
    }

    pub(super) fn publish_batch(
        changes: &[Self],
        direction: HistoryDirection,
    ) -> Result<Vec<PointwiseParameter<E>>, HistoryError<E>> {
        Self::publish_with(changes, direction, || Ok(())).map(|(_, targets)| targets)
    }

    pub(super) fn publish_with<T>(
        changes: &[Self],
        direction: HistoryDirection,
        other: impl FnOnce() -> Result<T, HistoryError<E>>,
    ) -> Result<(T, Vec<PointwiseParameter<E>>), HistoryError<E>> {
        publish_changes(changes, &[], direction, other)
    }
}

struct CurveRecordChange {
    value: SharedCurveRecord,
    before: Arc<CurveRecord>,
    after: Arc<CurveRecord>,
}

struct CurveAdjusterEdit<E> {
    changes: Vec<CurveChange<E>>,
    record: Option<CurveRecordChange>,
    added: Option<Arc<AtomicBool>>,
    retired: Option<Arc<AtomicBool>>,
}

impl<E: 'static> CurveAdjusterEdit<E> {
    fn publish(
        &self,
        direction: HistoryDirection,
    ) -> Result<Vec<PointwiseParameter<E>>, HistoryError<E>> {
        let (_, targets) =
            publish_changes(&self.changes, self.record.as_slice(), direction, || Ok(()))?;
        let undo = matches!(direction, HistoryDirection::Undo);
        if let Some(active) = &self.added {
            active.store(!undo, Ordering::Release);
        }
        if let Some(active) = &self.retired {
            active.store(undo, Ordering::Release);
        }
        Ok(targets)
    }
}

pub(super) struct CurveAdjusterBatch<E> {
    changes: Vec<CurveChange<E>>,
    records: Vec<CurveRecordChange>,
}

impl<E: 'static> CurveAdjusterBatch<E> {
    pub(super) fn prepare(
        branches: &[Option<PointwiseNode<E>>],
        updates: Vec<(CurveAdjuster, Arc<CurveRecord>)>,
    ) -> Result<Self, GtxGraphError<E>> {
        let mut curves = HashMap::new();
        let mut records = Vec::with_capacity(updates.len());
        for (adjuster, after) in updates {
            curves.insert(adjuster.key, to_live(&after)?);
            records.push(CurveRecordChange {
                before: adjuster.value.snapshot(),
                after,
                value: adjuster.value,
            });
        }
        records.sort_unstable_by_key(|record| record.value.lock_order());
        let mut changes = Vec::new();
        if !curves.is_empty() {
            for property in curve_properties(branches) {
                let Some(curve) = adjustment_key(&property).and_then(|key| curves.get(&key)) else {
                    continue;
                };
                let before = CurveState::read(&property).expect("traversed Curve property");
                let after = CurveState {
                    adjustment: before.adjustment.clone(),
                    curve: Some(curve.clone()),
                };
                changes.push(CurveChange {
                    property: property.downgrade(),
                    before,
                    after,
                });
            }
        }
        Ok(Self { changes, records })
    }

    pub(super) fn publish<T>(
        &self,
        direction: HistoryDirection,
        publish_other: impl FnOnce() -> Result<T, HistoryError<E>>,
    ) -> Result<(T, Vec<PointwiseParameter<E>>), HistoryError<E>> {
        publish_changes(&self.changes, &self.records, direction, publish_other)
    }
}

fn publish_changes<E: 'static, T>(
    changes: &[CurveChange<E>],
    records: &[CurveRecordChange],
    direction: HistoryDirection,
    publish_other: impl FnOnce() -> Result<T, HistoryError<E>>,
) -> Result<(T, Vec<PointwiseParameter<E>>), HistoryError<E>> {
    if records
        .windows(2)
        .any(|pair| pair[0].value.same_reference(&pair[1].value))
    {
        return Err(HistoryError::InputChanged);
    }
    let undo = matches!(direction, HistoryDirection::Undo);
    let mut values = Vec::with_capacity(records.len());
    for record in records {
        let current = record.value.lock();
        let expected = if undo { &record.after } else { &record.before };
        if !Arc::ptr_eq(&current, expected) {
            return Err(HistoryError::InputChanged);
        }
        values.push(current);
    }
    let targets = changes
        .iter()
        .map(|change| change.property.upgrade().ok_or(HistoryError::NullTarget))
        .collect::<Result<Vec<_>, _>>()?;
    let rollback = |count: usize| {
        for (target, change) in targets[..count].iter().zip(changes).rev() {
            let (after, before) = change.states(!undo);
            after.replace(target, before);
        }
    };
    for (index, (target, change)) in targets.iter().zip(changes).enumerate() {
        let (before, after) = change.states(undo);
        if !before.replace(target, after) {
            rollback(index);
            return Err(HistoryError::InputChanged);
        }
    }
    let other = match publish_other() {
        Ok(other) => other,
        Err(error) => {
            rollback(targets.len());
            return Err(error);
        }
    };
    for (record, current) in records.iter().zip(&mut values) {
        **current = if undo {
            record.before.clone()
        } else {
            record.after.clone()
        };
    }
    Ok((other, targets))
}

impl<E: 'static> HistoryGraphEdit<E> for CurveAdjusterEdit<E> {
    fn apply(&self, direction: HistoryDirection) -> Result<(), HistoryError<E>> {
        notify(&self.publish(direction)?)
    }
}

pub(super) fn notify<E: 'static>(targets: &[PointwiseParameter<E>]) -> Result<(), HistoryError<E>> {
    for target in targets {
        target
            .notify_curve_adjustment()
            .map_err(|error| HistoryError::Property(Box::new(error)))?;
    }
    Ok(())
}
