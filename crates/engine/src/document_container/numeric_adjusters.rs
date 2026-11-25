use super::environment_adjusters::graph_nodes;
use super::lab_presets::{LabValues, active_lab_group};
use super::*;
use crate::document_container::catalog::{GraphCatalog, NumericCatalogValue};
use crate::pointwise::choice_index_from_numeric;
use crate::pointwise::numeric_adjustment::{
    NumericReceiver, NumericReceiverShape, NumericReceiverState, WeakNumericReceiver,
};
use crate::pointwise::property::NumericAdjustment;
use crate::{
    HistoryDirection, HistoryError, NumericAdjusterMetadata, SharedPropertyAdjustment,
    ValueAdjusterPayload,
};
use std::sync::atomic::AtomicBool;

#[path = "numeric_adjuster_definition.rs"]
mod definition;
pub use definition::LabScalarDefinition;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LabNumericValue {
    Number(f64),
    Dual([f64; 2]),
    Selection(i32),
}

/// The bounds and precision shown in the Lab's numeric definition editor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LabNumericRange {
    bounds: [f64; 2],
    whole_numbers: bool,
}

impl LabNumericRange {
    pub fn new(bounds: [f64; 2], whole_numbers: bool) -> Self {
        Self {
            bounds,
            whole_numbers,
        }
    }

    pub fn bounds(self) -> [f64; 2] {
        self.bounds
    }

    pub fn whole_numbers(self) -> bool {
        self.whole_numbers
    }

    fn read(numeric: &NumericAdjusterMetadata) -> Option<Self> {
        Some(Self::new(limits(numeric)?, numeric.accuracy == 0))
    }
}

/// A snapshot of the selected Lab control, with its catalog identity and
/// committed state for validating a draft when it is applied.
#[derive(Clone, Debug)]
pub struct LabNumericAdjuster {
    name: String,
    catalog: NumericCatalogValue,
    before: CatalogState,
    value: LabNumericValue,
    range: Option<[f64; 2]>,
    range_settings: Option<LabNumericRange>,
    options: Vec<String>,
    selection_options: Vec<crate::SelectionOptionRecord>,
}

impl LabNumericAdjuster {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn value(&self) -> LabNumericValue {
        self.value
    }

    pub fn range(&self) -> Option<[f64; 2]> {
        self.range
    }

    pub fn range_settings(&self) -> Option<LabNumericRange> {
        self.range_settings
    }

    pub fn options(&self) -> &[String] {
        &self.options
    }

    pub fn selection_options(&self) -> &[crate::SelectionOptionRecord] {
        &self.selection_options
    }

    pub fn same_reference(&self, other: &Self) -> bool {
        self.matches(&other.catalog)
    }

    pub(super) fn matches(&self, catalog: &NumericCatalogValue) -> bool {
        self.catalog.lock_order() == catalog.lock_order()
    }

    pub(super) fn read(
        metadata: &AdjusterMetadataRecord,
        catalog: &NumericCatalogValue,
    ) -> Option<Self> {
        let name = metadata.name.as_ref().filter(|name| !name.is_empty())?;
        let payload = catalog.lock();
        let (value, range, options) = match (&*payload, metadata.type_tag) {
            (ValueAdjusterPayload::Number(number), 0) => (
                LabNumericValue::Number(number.value),
                Some(limits(&number.numeric)?),
                Vec::new(),
            ),
            (ValueAdjusterPayload::Dual(dual), 1) => (
                LabNumericValue::Dual(dual.value),
                Some(limits(&dual.numeric)?),
                Vec::new(),
            ),
            (ValueAdjusterPayload::Number(number), 4) => (
                LabNumericValue::Selection(number.selected),
                None,
                number
                    .options
                    .iter()
                    .enumerate()
                    .map(|(index, option)| {
                        option
                            .name
                            .as_deref()
                            .filter(|name| !name.is_empty())
                            .map_or_else(
                                || format!("Option {}", index + 1),
                                String::from_utf16_lossy,
                            )
                    })
                    .collect(),
            ),
            _ => return None,
        };
        Some(Self {
            name: String::from_utf16_lossy(name),
            catalog: catalog.clone(),
            before: CatalogState::read(&payload),
            value,
            range,
            range_settings: match &*payload {
                ValueAdjusterPayload::Number(number) => LabNumericRange::read(&number.numeric),
                ValueAdjusterPayload::Dual(dual) => LabNumericRange::read(&dual.numeric),
                _ => None,
            },
            options,
            selection_options: match (&*payload, metadata.type_tag) {
                (ValueAdjusterPayload::Number(number), 4) => number.options.clone(),
                _ => Vec::new(),
            },
        })
    }
}

impl<E: 'static> Gtx375GraphDocument<E> {
    pub fn numeric_lab_adjusters(
        &self,
        selected: Option<&PointwiseNode<E>>,
    ) -> Vec<LabNumericAdjuster> {
        let group = active_lab_group(selected)
            .and_then(|group| group.group_guid())
            .unwrap_or_default();
        self.numeric_adjusters_in_section(group, |_| true)
    }

    pub(super) fn numeric_adjusters_in_section(
        &self,
        group: [u8; 16],
        visible: impl Fn(&AdjusterMetadataRecord) -> bool,
    ) -> Vec<LabNumericAdjuster> {
        let mut controls: Vec<_> = self
            .catalog
            .entries()
            .filter_map(|(entry, _)| match entry {
                GtxCatalogRecord::Numeric { metadata, value }
                    if metadata.group_id == group && visible(metadata) =>
                {
                    Some((metadata, LabNumericAdjuster::read(metadata, value)?))
                }
                _ => None,
            })
            .collect();
        controls.sort_by_cached_key(|(metadata, control)| {
            super::lab_adjuster_metadata::display_order(
                metadata.order,
                control.name(),
                metadata.type_tag,
            )
        });
        controls.into_iter().map(|(_, control)| control).collect()
    }

    pub fn edit_numeric_lab_adjuster(
        &self,
        selected: Option<&PointwiseNode<E>>,
        control: &LabNumericAdjuster,
        input: LabNumericValue,
    ) -> Result<bool, GtxGraphError<E>> {
        self.edit_numeric_lab_adjuster_with_patch_input_sizes(selected, control, input, &[])
    }

    /// Upstream graph occurrences used by connected Patches shape receivers of this control.
    pub fn connected_patch_inputs_for_numeric_adjuster(
        &self,
        selected: Option<&PointwiseNode<E>>,
        control: &LabNumericAdjuster,
    ) -> Result<Vec<PointwiseNode<E>>, GtxGraphError<E>> {
        let entry = self.catalog_adjuster(selected, control)?;
        let Some(name) = entry
            .metadata
            .name
            .as_deref()
            .filter(|_| matches!(entry.metadata.type_tag, 0 | 4))
        else {
            return Ok(Vec::new());
        };
        Ok(super::synthesis_adjusters::connected_patch_adjuster_inputs(
            &self.branches(),
            entry.metadata.group_id,
            name,
        ))
    }

    /// Applies this control to all receivers in one edit with raw graph source sizes.
    pub fn edit_numeric_lab_adjuster_with_patch_input_sizes(
        &self,
        selected: Option<&PointwiseNode<E>>,
        control: &LabNumericAdjuster,
        input: LabNumericValue,
        input_sizes: &[(u64, crate::ImageGeometry)],
    ) -> Result<bool, GtxGraphError<E>> {
        let entry = self.catalog_adjuster(selected, control)?;
        let update = NumericCatalogUpdate::prepare_value(
            entry.metadata,
            entry.value,
            input,
            Some(control.before),
        )?;
        if update.before == update.after || control.value == input {
            return Ok(false);
        }
        let mut values = LabValues::default();
        values.numeric.push(update);
        values.synthesis_input_sizes = input_sizes.to_vec();
        self.commit_lab_values(&format!("Modify {} Adjuster", control.name()), values)?;
        Ok(true)
    }

    fn catalog_adjuster(
        &self,
        selected: Option<&PointwiseNode<E>>,
        control: &LabNumericAdjuster,
    ) -> Result<CatalogAdjuster<'_>, GtxGraphError<E>> {
        if selected.is_some_and(|selected| {
            !graph_nodes(&self.branches()).any(|node| node.same_reference(selected))
        }) {
            return Err(GtxGraphError::NumericAdjuster(
                "The selected Lab section changed.",
            ));
        }
        let group = active_lab_group(selected)
            .and_then(|group| group.group_guid())
            .unwrap_or_default();
        self.catalog
            .entries()
            .find_map(|(entry, presence)| match entry {
                GtxCatalogRecord::Numeric { metadata, value }
                    if metadata.group_id == group
                        && value.lock_order() == control.catalog.lock_order() =>
                {
                    Some(CatalogAdjuster {
                        metadata,
                        value,
                        presence,
                    })
                }
                _ => None,
            })
            .ok_or(GtxGraphError::NumericAdjuster(
                "This adjuster is no longer available in the selected Lab section.",
            ))
    }
}

struct CatalogAdjuster<'a> {
    metadata: &'a AdjusterMetadataRecord,
    value: &'a NumericCatalogValue,
    presence: &'a Arc<AtomicBool>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum CatalogState {
    Number {
        current: f64,
        previous: f64,
        selected: i32,
        previous_selected: i32,
    },
    Dual {
        current: [f64; 2],
        previous: [f64; 2],
    },
}

impl CatalogState {
    fn read(payload: &ValueAdjusterPayload) -> Self {
        match payload {
            ValueAdjusterPayload::Number(number) => Self::Number {
                current: number.value,
                previous: number.previous,
                selected: number.selected,
                previous_selected: number.previous_selected,
            },
            ValueAdjusterPayload::Dual(dual) => Self::Dual {
                current: dual.value,
                previous: dual.previous,
            },
            _ => unreachable!("live numeric catalog entry"),
        }
    }

    fn write(self, payload: &mut ValueAdjusterPayload) {
        match (self, payload) {
            (
                Self::Number {
                    current,
                    previous,
                    selected,
                    previous_selected,
                },
                ValueAdjusterPayload::Number(number),
            ) => {
                number.value = current;
                number.previous = previous;
                number.selected = selected;
                number.previous_selected = previous_selected;
            }
            (Self::Dual { current, previous }, ValueAdjusterPayload::Dual(dual)) => {
                dual.value = current;
                dual.previous = previous;
            }
            _ => unreachable!("live numeric catalog entry changed type"),
        }
    }
}

pub(super) struct NumericCatalogUpdate {
    group_id: [u8; 16],
    name: Vec<u16>,
    value: NumericCatalogValue,
    range: [f64; 2],
    before: CatalogState,
    after: CatalogState,
}

impl NumericCatalogUpdate {
    pub(super) fn patch_source_key(&self) -> Option<([u8; 16], &[u16])> {
        matches!(self.after, CatalogState::Number { .. })
            .then_some((self.group_id, self.name.as_slice()))
    }

    pub(super) fn reconnect(
        metadata: &AdjusterMetadataRecord,
        value: &NumericCatalogValue,
    ) -> Option<Self> {
        let payload = value.lock();
        let range = match (&*payload, metadata.type_tag) {
            (ValueAdjusterPayload::Number(_), 4) => [0.0, 100.0],
            (ValueAdjusterPayload::Number(number), 0) => limits(&number.numeric)?,
            (ValueAdjusterPayload::Dual(dual), 1) => limits(&dual.numeric)?,
            _ => return None,
        };
        let state = CatalogState::read(&payload);
        Some(Self {
            group_id: metadata.group_id,
            name: metadata.name.as_ref()?.clone(),
            value: value.clone(),
            range,
            before: state,
            after: state,
        })
    }

    pub(super) fn prepare<E>(
        metadata: &AdjusterMetadataRecord,
        value: &NumericCatalogValue,
        preset: &GtxPresetRecord,
    ) -> Result<Self, GtxGraphError<E>> {
        let input = match (preset.kind(), preset.value()) {
            (GtxPresetKind::Number, GtxPresetValue::Number(value)) => {
                LabNumericValue::Number(*value)
            }
            (GtxPresetKind::Dual, GtxPresetValue::Dual(value)) => LabNumericValue::Dual(*value),
            (GtxPresetKind::Selection, GtxPresetValue::Number(value)) if value.is_finite() => {
                LabNumericValue::Selection(crate::framework_cast::integer(*value))
            }
            _ => {
                return Err(GtxGraphError::LabPreset(
                    "This preset has an invalid numeric adjuster value.",
                ));
            }
        };
        Self::prepare_value(metadata, value, input, None)
    }

    fn prepare_value<E>(
        metadata: &AdjusterMetadataRecord,
        value: &NumericCatalogValue,
        input: LabNumericValue,
        expected: Option<CatalogState>,
    ) -> Result<Self, GtxGraphError<E>> {
        let payload = value.lock();
        let before = CatalogState::read(&payload);
        if expected.is_some_and(|expected| expected != before) {
            return Err(GtxGraphError::NumericAdjuster(
                "This adjuster changed. Reopen it to edit its current value.",
            ));
        }
        let direct = expected.is_some();
        let invalid = || {
            if direct {
                GtxGraphError::NumericAdjuster("Enter a valid adjuster value.")
            } else {
                GtxGraphError::LabPreset("This preset has an invalid numeric adjuster value.")
            }
        };
        let range_of = |numeric: &NumericAdjusterMetadata| {
            limits(numeric).ok_or_else(|| {
                if direct {
                    GtxGraphError::NumericAdjuster("This numeric adjuster has no limits.")
                } else {
                    GtxGraphError::LabPreset("The matching numeric adjuster has no limits.")
                }
            })
        };
        let number_in_range = |number: f64, range: [f64; 2], numeric: &NumericAdjusterMetadata| {
            if !number.is_finite() {
                return Err(invalid());
            }
            if direct && !(range[0]..=range[1]).contains(&number) {
                return Err(GtxGraphError::NumericAdjuster(
                    "Enter a value within the adjuster's range.",
                ));
            }
            if direct {
                normalize_adjuster_number(number, range, numeric)
            } else {
                Ok(clamp_to_range(number, range))
            }
        };
        let (range, after) = match (metadata.type_tag, input, &*payload) {
            (0, LabNumericValue::Number(input), ValueAdjusterPayload::Number(n)) => {
                let range = range_of(&n.numeric)?;
                let number = number_in_range(input, range, &n.numeric)?;
                (
                    range,
                    CatalogState::Number {
                        current: number,
                        previous: number,
                        selected: n.selected,
                        previous_selected: n.previous_selected,
                    },
                )
            }
            (4, LabNumericValue::Selection(input), ValueAdjusterPayload::Number(n)) => {
                let count = n.options.len() as i64;
                if direct && i64::from(input) >= count {
                    return Err(GtxGraphError::NumericAdjuster(
                        "Choose an available option.",
                    ));
                }
                let selected = if i64::from(input) <= count {
                    input
                } else {
                    n.selected
                };
                if selected < -1 {
                    return Err(invalid());
                }
                let number = n
                    .options
                    .get(selected as usize)
                    .map_or(n.value, |option| option.value);
                if !number.is_finite() {
                    return Err(invalid());
                }
                (
                    [0.0, 100.0],
                    CatalogState::Number {
                        current: number,
                        previous: number,
                        selected,
                        previous_selected: selected,
                    },
                )
            }
            (1, LabNumericValue::Dual(input), ValueAdjusterPayload::Dual(d)) => {
                let range = range_of(&d.numeric)?;
                let pair = [
                    number_in_range(input[0], range, &d.numeric)?,
                    number_in_range(input[1], range, &d.numeric)?,
                ];
                (
                    range,
                    CatalogState::Dual {
                        current: pair,
                        previous: pair,
                    },
                )
            }
            _ => return Err(invalid()),
        };
        Ok(Self {
            group_id: metadata.group_id,
            name: metadata.name.clone().unwrap_or_default(),
            value: value.clone(),
            range,
            before,
            after,
        })
    }

    pub(super) fn identity(&self) -> usize {
        self.value.lock_order()
    }

    fn map<E: 'static>(
        &self,
        number: f64,
        parts: &crate::pointwise::AdjustmentCaptureParts<E>,
        axis: usize,
    ) -> Result<f64, PointwiseError<E>> {
        let Some(curves) = &parts.curves else {
            return Ok(-1.0);
        };
        let Some(curve) = curves.value(axis as i32)? else {
            return Ok(-1.0);
        };
        let [minimum, maximum] = self.range;
        let input = ((number - minimum) / (maximum - minimum)) as f32;
        Ok(f64::from(curve.sample(input)?))
    }
}

pub(super) fn limits(metadata: &NumericAdjusterMetadata) -> Option<[f64; 2]> {
    let limits = metadata.limits.as_ref()?;
    Some([
        limits.minimum.to_framework_f64(),
        limits.maximum.to_framework_f64(),
    ])
}

fn clamp_to_range(number: f64, [minimum, maximum]: [f64; 2]) -> f64 {
    number.min(maximum).max(minimum)
}

fn normalize_adjuster_number<E>(
    number: f64,
    range: [f64; 2],
    numeric: &NumericAdjusterMetadata,
) -> Result<f64, GtxGraphError<E>> {
    let limits = numeric
        .limits
        .as_ref()
        .ok_or(GtxGraphError::NumericAdjuster(
            "This adjuster has no numeric limits.",
        ))?;
    let value = crate::round_decimal_places(number, limits.decimal_places)
        .map_err(|error| GtxGraphError::Node(PointwiseError::DecimalRounding(error).into()))?;
    Ok(clamp_to_range(value, range))
}

struct NumericTarget<E> {
    property: WeakNumericReceiver<E>,
    adjustment: SharedPropertyAdjustment<E>,
    before: NumericReceiverState,
    after: NumericReceiverState,
}

pub(super) struct NumericAdjusterBatch<E> {
    updates: Vec<NumericCatalogUpdate>,
    targets: Vec<NumericTarget<E>>,
}

impl<E: 'static> NumericAdjusterBatch<E> {
    pub(super) fn prepare(
        branches: &[Option<PointwiseNode<E>>],
        mut updates: Vec<NumericCatalogUpdate>,
    ) -> Result<Self, GtxGraphError<E>> {
        let mut targets = Vec::new();
        if !updates.is_empty() {
            for node in graph_nodes(branches) {
                for property in NumericReceiver::for_node(&node) {
                    let Some(adjustment) = property.retained_adjustment() else {
                        continue;
                    };
                    let Some(shape) = property
                        .shape()
                        .map_err(|error| GtxGraphError::Node(error.into()))?
                    else {
                        continue;
                    };
                    let Some(value) = map_property(&updates, shape, &adjustment)
                        .map_err(|error| GtxGraphError::Node(error.into()))?
                    else {
                        continue;
                    };
                    let (before, after) = property
                        .prepare(value)
                        .map_err(|error| GtxGraphError::Node(error.into()))?;
                    if before != after {
                        targets.push(NumericTarget {
                            property: property.downgrade(),
                            adjustment,
                            before,
                            after,
                        });
                    }
                }
            }
        }
        updates.sort_unstable_by_key(NumericCatalogUpdate::identity);
        Ok(Self { updates, targets })
    }

    /// Validate every numeric target before publishing the other part of the
    /// preset. Keep the catalog locks until both sets of leaf writes finish.
    pub(super) fn publish<T>(
        &self,
        direction: HistoryDirection,
        publish_environment: impl FnOnce() -> Result<T, HistoryError<E>>,
    ) -> Result<(T, Vec<NumericReceiver<E>>), HistoryError<E>> {
        let undo = matches!(direction, HistoryDirection::Undo);
        let mut values = Vec::with_capacity(self.updates.len());
        for update in &self.updates {
            let value = update.value.lock();
            let expected = if undo { update.after } else { update.before };
            if CatalogState::read(&value) != expected {
                return Err(HistoryError::InputChanged);
            }
            values.push(value);
        }
        let mut targets = Vec::with_capacity(self.targets.len());
        for target in &self.targets {
            let property = target.property.upgrade().ok_or(HistoryError::NullTarget)?;
            if property
                .retained_adjustment()
                .is_none_or(|adjustment| !adjustment.same_reference(&target.adjustment))
            {
                return Err(HistoryError::InputChanged);
            }
            targets.push(property);
        }
        let environment = publish_environment()?;
        for (property, target) in targets.iter().zip(&self.targets) {
            property.write(if undo { &target.before } else { &target.after });
        }
        for (update, payload) in self.updates.iter().zip(&mut values) {
            let next = if undo { update.before } else { update.after };
            next.write(payload);
        }
        Ok((environment, targets))
    }
}

pub(super) fn numeric_catalog_values(catalog: &GraphCatalog) -> Vec<NumericCatalogUpdate> {
    catalog
        .entries()
        .filter_map(|(entry, _)| match entry {
            GtxCatalogRecord::Numeric { metadata, value } => {
                NumericCatalogUpdate::reconnect(metadata, value)
            }
            _ => None,
        })
        .collect()
}

pub(super) fn restore_lab_values<E: 'static>(
    branches: &[Option<PointwiseNode<E>>],
    catalog: &GraphCatalog,
    preserve_graph_shapes: bool,
) -> Result<(), GtxGraphError<E>> {
    use super::synthesis_adjusters::PatchShapeInputs;
    let updates = numeric_catalog_values(catalog);
    let mut environment = super::environment_adjusters::EnvironmentAdjusterBatch::empty();
    let mut references = super::environment_adjusters::MapReferenceValues::read(catalog);
    environment.stage_receiver_values(branches, catalog, &updates, references.as_mut())?;
    let synthesis = super::synthesis_adjusters::SynthesisAdjusterBatch::prepare(
        branches,
        catalog,
        &mut environment,
        Vec::new(),
        &updates,
        references.as_ref(),
        if preserve_graph_shapes {
            PatchShapeInputs::SavedValues
        } else {
            PatchShapeInputs::Authoring(&[])
        },
    )?;
    let references =
        super::lab_presets::prepare_reference_receivers(branches, references.as_ref())?;
    let batch = NumericAdjusterBatch::prepare(branches, updates)?;
    // The loading graph has no external editor and its catalog is already set.
    // Restore each receiver from the recorded numeric value without a preset's
    // clamp or a Selection option lookup.
    let (((maps, references), synthesis), targets) = batch
        .publish(HistoryDirection::Redo, || {
            synthesis.publish(HistoryDirection::Redo, || {
                crate::pointwise::reference_adjustment::LabReferenceEdit::publish_with(
                    &references,
                    false,
                    || environment.publish(HistoryDirection::Redo),
                )
            })
        })
        .map_err(GtxGraphError::History)?;
    maps.notify().map_err(GtxGraphError::History)?;
    for receiver in references {
        receiver.notify().map_err(GtxGraphError::History)?;
    }
    for property in &targets {
        property
            .normalize()
            .map_err(|error| GtxGraphError::Node(error.into()))?;
    }
    for property in targets {
        property
            .notify()
            .map_err(|error| GtxGraphError::Node(error.into()))?;
    }
    for filter in synthesis {
        filter
            .notify_synthesis_receiver_change()
            .map_err(|error| GtxGraphError::Node(error.into()))?;
    }
    Ok(())
}

pub(super) fn map_environment_number<E: 'static>(
    updates: &[NumericCatalogUpdate],
    adjustment: &crate::AdjustmentRecord,
) -> Result<Option<(usize, f64)>, PointwiseError<E>> {
    if !matches!(adjustment.property_type, 0 | 4) {
        return Ok(None);
    }
    map_retained_number(updates, adjustment, 0)
}

pub(super) fn map_retained_number<E: 'static>(
    updates: &[NumericCatalogUpdate],
    adjustment: &crate::AdjustmentRecord,
    axis: usize,
) -> Result<Option<(usize, f64)>, PointwiseError<E>> {
    let Some(component) = adjustment
        .components()
        .get(axis)
        .filter(|component| component.active && !component.name.is_empty())
    else {
        return Ok(None);
    };
    let Some((order, update)) = updates.iter().enumerate().rev().find(|(_, update)| {
        update.group_id == adjustment.guid
            && update.name == component.name
            && compatible(adjustment.property_type, update.after)
    }) else {
        return Ok(None);
    };
    let current = match update.after {
        CatalogState::Number { current, .. } => current,
        CatalogState::Dual { current, .. } => {
            let Some(current) = current.get(axis) else {
                return Ok(None);
            };
            *current
        }
    };
    let Some(record) = &component.curve else {
        return Ok(Some((order, -1.)));
    };
    let curve = environment_mapping_curve(record)?;
    let [minimum, maximum] = update.range;
    let value = curve.sample(((current - minimum) / (maximum - minimum)) as f32)?;
    Ok(Some((order, f64::from(value))))
}

pub(super) fn environment_mapping_curve<E: 'static>(
    record: &crate::CurveRecord,
) -> Result<crate::SharedCurve<E>, PointwiseError<E>> {
    let mut writer = crate::BinaryWriter::default();
    record.save(&mut writer).map_err(|error| {
        PointwiseError::ModernProperty(crate::PropertyRecordError::Curve(error).into())
    })?;
    let mut reader = crate::BinaryCursor::new(writer.bytes()).map_err(crate::CurveError::Binary)?;
    let curve = crate::SharedCurve::new(record.minimum, record.maximum, record.wrap);
    curve.load(&mut reader, 3.75)?;
    Ok(curve)
}

fn compatible(property_type: i32, state: CatalogState) -> bool {
    matches!(
        (property_type, state),
        (0 | 4, CatalogState::Number { .. }) | (1, CatalogState::Dual { .. })
    )
}

pub(super) fn map_property<E: 'static>(
    updates: &[NumericCatalogUpdate],
    shape: NumericReceiverShape,
    adjustment: &SharedPropertyAdjustment<E>,
) -> Result<Option<NumericAdjustment>, PointwiseError<E>> {
    let parts = adjustment.capture_parts();
    let (Some(names), Some(flags)) = (&parts.names, &parts.flags) else {
        return Ok(None);
    };
    let names = names.entries();
    let find_update = |axis: usize| -> Result<Option<&NumericCatalogUpdate>, PointwiseError<E>> {
        if axis as i32 >= parts.count
            || !flags
                .value(axis as i32)
                .map_err(|_| PointwiseError::Index)?
        {
            return Ok(None);
        }
        Ok(names.get(axis).and_then(Option::as_ref).and_then(|name| {
            updates.iter().rev().find(|update| {
                update.group_id == parts.guid
                    && update.name == *name
                    && compatible(parts.property_type, update.after)
            })
        }))
    };
    let first = find_update(0)?;
    let value = match shape {
        NumericReceiverShape::Number | NumericReceiverShape::Choice(_)
            if matches!(parts.property_type, 0 | 4) =>
        {
            let Some(update) = first else { return Ok(None) };
            let CatalogState::Number { current, .. } = update.after else {
                return Ok(None);
            };
            let mapped = update.map(current, &parts, 0)?;
            if let NumericReceiverShape::Choice(option_count) = shape {
                let index = choice_index_from_numeric(mapped, option_count)?;
                if index < 0 {
                    return Ok(None);
                }
                NumericAdjustment::Choice(index)
            } else {
                NumericAdjustment::Number(mapped)
            }
        }
        NumericReceiverShape::Pair(_) if parts.property_type == 1 => {
            let Some(update) = first.or(find_update(1)?) else {
                return Ok(None);
            };
            let CatalogState::Dual { current, .. } = update.after else {
                return Ok(None);
            };
            NumericAdjustment::Pair([
                update.map(current[0], &parts, 0)?,
                update.map(current[1], &parts, 1)?,
            ])
        }
        NumericReceiverShape::Pair(mut current) if matches!(parts.property_type, 0 | 4) => {
            let mut changed = false;
            for (axis, update) in [first, find_update(1)?].into_iter().enumerate() {
                if let Some(update) = update
                    && let CatalogState::Number {
                        current: number, ..
                    } = update.after
                {
                    current[axis] = update.map(number, &parts, axis)?;
                    changed = true;
                }
            }
            if !changed {
                return Ok(None);
            }
            NumericAdjustment::Pair(current)
        }
        _ => return Ok(None),
    };
    Ok(Some(value))
}
