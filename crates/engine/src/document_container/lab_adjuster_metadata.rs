//! Names and display order for catalog-backed Lab controls.
use super::curve_adjusters::CurveChange;
use super::environment_adjusters::{EnvironmentAdjusterBatch, graph_nodes};
use super::lab_presets::{AffectedProperties, active_lab_group};
use super::numeric_adjusters::{NumericCatalogUpdate, map_property, map_retained_number};
use super::synthesis_adjusters::{SynthesisAdjusterBatch, SynthesisReceiverEdit};
use super::*;
use crate::document_container::catalog::NumericCatalogValue;
use crate::environment_record::EnvironmentLabReference;
use crate::history::HistoryGraphEdit;
use crate::pointwise::EnvironmentPaintEdit;
use crate::pointwise::numeric_adjustment::{
    NumericReceiver, NumericReceiverState, WeakNumericReceiver,
};
use crate::pointwise::property::SynthesisLinkState;
use crate::pointwise::reference_adjustment::LabReferenceEdit;
use crate::{
    HistoryDirection, HistoryError, ReferenceAdjusterPayload, SelectionOptionRecord,
    SharedPropertyAdjustment, SharedSelectionOptions, SynthesisCatalogValue, ValueAdjusterPayload,
};
use std::sync::atomic::{AtomicBool, Ordering};

#[path = "lab_adjuster_environment.rs"]
mod environment;
use environment::FieldRename;

/// A Lab entry's displayed name, order, and retained catalog identity.
#[derive(Clone)]
pub struct LabAdjusterMetadata {
    metadata: AdjusterMetadataRecord,
    presence: Arc<AtomicBool>,
    name: String,
}

impl LabAdjusterMetadata {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn order(&self) -> i32 {
        self.metadata.order
    }

    pub fn is_material(&self) -> bool {
        self.metadata.type_tag == 10
    }

    /// Identity of this catalog row while it is listed in Lab.
    pub fn identity(&self) -> usize {
        Arc::as_ptr(&self.presence) as usize
    }

    pub fn type_label(&self) -> &'static str {
        match self.metadata.type_tag {
            0 => "Number",
            1 => "Dual",
            4 => "Selection",
            6 => "Color",
            7 => "Gradient",
            8 => "Curve",
            9 => "Environment",
            10 => "Synthesis",
            _ => unreachable!("supported Lab entry"),
        }
    }
}

pub(super) fn display_order(order: i32, name: &str, type_tag: i32) -> (i32, String, String, i32) {
    let type_order = match type_tag {
        4 => -1,
        10 => 0,
        9 => 2,
        7 => 3,
        6 => 4,
        8 => 5,
        1 => 6,
        0 => 7,
        _ => type_tag,
    };
    (order, name.to_lowercase(), name.to_owned(), type_order)
}

pub(super) fn section_id<E: 'static>(selected: Option<&PointwiseNode<E>>) -> [u8; 16] {
    active_lab_group(selected)
        .and_then(|group| group.group_guid())
        .unwrap_or_default()
}

impl<E: 'static> Gtx375GraphDocument<E> {
    /// Resolves a visible Material row by its retained catalog identity in the current Lab section.
    pub fn lab_synthesis_adjuster(
        &self,
        selected: Option<&PointwiseNode<E>>,
        expected: &LabAdjusterMetadata,
    ) -> Option<SynthesisAdjuster> {
        if !expected.is_material() || expected.metadata.group_id != section_id(selected) {
            return None;
        }
        self.catalog.entries().find_map(|(entry, marker)| {
            if !Arc::ptr_eq(marker, &expected.presence) {
                return None;
            }
            let GtxCatalogRecord::Synthesis { metadata, value } = entry else {
                return None;
            };
            SynthesisAdjuster::from_catalog(metadata, value)
        })
    }

    pub fn lab_adjuster_metadata(
        &self,
        selected: Option<&PointwiseNode<E>>,
    ) -> Vec<LabAdjusterMetadata> {
        self.lab_metadata_in_section(section_id(selected), |_| true)
    }

    pub(super) fn lab_metadata_in_section(
        &self,
        group_id: [u8; 16],
        visible: impl Fn(&AdjusterMetadataRecord) -> bool,
    ) -> Vec<LabAdjusterMetadata> {
        let mut entries: Vec<_> = self
            .catalog
            .entries()
            .filter_map(|(entry, presence)| {
                let metadata = entry.metadata();
                if metadata.group_id != group_id
                    || !visible(metadata)
                    || !(matches!(
                        entry,
                        GtxCatalogRecord::Numeric { .. }
                            | GtxCatalogRecord::Environment { .. }
                            | GtxCatalogRecord::Synthesis { .. }
                    ) || CurveAdjuster::read(entry).is_some()
                        || entry.color_or_gradient().is_some())
                {
                    return None;
                }
                let name = metadata.name.as_deref().filter(|name| !name.is_empty())?;
                Some(LabAdjusterMetadata {
                    metadata: metadata.clone(),
                    presence: presence.clone(),
                    name: String::from_utf16_lossy(name),
                })
            })
            .collect();
        entries.sort_by_cached_key(|entry| {
            display_order(entry.order(), entry.name(), entry.metadata.type_tag)
        });
        entries
    }

    /// Rename or reorder a Lab entry, merging into an existing compatible name.
    pub fn edit_lab_adjuster_metadata(
        &mut self,
        selected: Option<&PointwiseNode<E>>,
        expected: &LabAdjusterMetadata,
        name: &str,
        order: i32,
    ) -> Result<bool, GtxGraphError<E>> {
        let branches = self.branches();
        if expected.metadata.group_id != section_id(selected)
            || selected.is_some_and(|selected| {
                !graph_nodes(&branches).any(|node| node.same_reference(selected))
            })
        {
            return Err(GtxGraphError::NumericAdjuster(
                "The selected Lab section changed.",
            ));
        }
        let current = self
            .catalog
            .entries()
            .find_map(|(entry, marker)| Arc::ptr_eq(marker, &expected.presence).then_some(entry))
            .ok_or(GtxGraphError::NumericAdjuster(
                "This adjuster changed. Reopen its name and order.",
            ))?;
        let metadata = current.metadata();
        let previous_name = metadata
            .name
            .as_deref()
            .ok_or(GtxGraphError::NumericAdjuster("This adjuster has no name."))?;
        let requested_name = if name == expected.name() {
            previous_name.to_vec()
        } else {
            name.split(' ')
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join(" ")
                .encode_utf16()
                .collect()
        };
        if requested_name.is_empty() {
            return Err(GtxGraphError::NumericAdjuster("Enter an adjuster name."));
        }
        if order != metadata.order && !(-100..=100).contains(&order) {
            return Err(GtxGraphError::NumericAdjuster(
                "Enter an order from −100 to 100.",
            ));
        }
        let rename = requested_name != previous_name;
        if !rename && order == metadata.order {
            return Ok(false);
        }
        let type_tag = metadata.type_tag;
        let group_id = metadata.group_id;
        let destination = rename
            .then(|| {
                self.catalog.entries().find(|(entry, marker)| {
                    let other = entry.metadata();
                    !Arc::ptr_eq(marker, &expected.presence)
                        && other.group_id == group_id
                        && compatible(type_tag, other.type_tag)
                        && other.name.as_deref() == Some(&requested_name)
                })
            })
            .flatten();
        let (merge_value, merge) = if let Some((destination, marker)) = destination {
            let value = MergeValue::read(destination)?;
            let selection = SelectionMerge::prepare(current, destination);
            (Some(value), Some((marker.clone(), selection)))
        } else {
            (None, None)
        };

        let field_rename = FieldRename {
            group: group_id,
            tag: type_tag,
            before: previous_name,
            after: &requested_name,
            destination: merge_value.as_ref(),
        };
        let links = if !rename {
            MetadataLinks::Properties(Vec::new())
        } else if matches!(type_tag, 6 | 7) {
            let mut references = Vec::new();
            let value = match &merge_value {
                Some(MergeValue::Reference(value)) => Some(value),
                _ => None,
            };
            for node in graph_nodes(&branches) {
                references.extend(
                    LabReferenceEdit::prepare_edited_node(&node, |before, name| {
                        if before.guid() != group_id
                            || before.property_type() != type_tag
                            || name != previous_name
                        {
                            return None;
                        }
                        let after = renamed_adjustment(
                            before,
                            previous_name,
                            &requested_name,
                            merge.is_some(),
                        )?;
                        Some((after, value.cloned()))
                    })
                    .map_err(|error| GtxGraphError::Node(error.into()))?,
                );
            }
            let mut maps = field_rename.stage_maps(&branches, &self.catalog)?;
            let synthesis = if type_tag == 6 {
                let rgb = match value {
                    Some(crate::environment_record::EnvironmentLabReference::Color(rgb)) => {
                        Some(*rgb)
                    }
                    _ => None,
                };
                Some(Box::new(SynthesisAdjusterBatch::prepare_mapped(
                    &branches,
                    &self.catalog,
                    &mut maps,
                    |record| {
                        Ok(record.remap_color_adjustments(|key| {
                            (key == (group_id, 6, previous_name))
                                .then(|| (group_id, requested_name.clone(), rgb))
                        }))
                    },
                )?))
            } else {
                None
            };
            MetadataLinks::Fields {
                properties: Vec::new(),
                references,
                maps,
                synthesis,
            }
        } else if let Some(curve) = CurveAdjuster::read(current) {
            let destination = match &merge_value {
                Some(MergeValue::Curve(destination)) => Some(destination),
                None => None,
                _ => unreachable!("compatible Curve destination"),
            };
            MetadataLinks::Curves(curve.renamed_receivers(
                &branches,
                &requested_name,
                destination,
            )?)
        } else if let GtxCatalogRecord::Synthesis { value, .. } = current {
            let destination = match &merge_value {
                Some(MergeValue::Synthesis(destination)) => destination.clone(),
                None => value.clone(),
                _ => unreachable!("compatible Synthesis destination"),
            };
            let properties = graph_nodes(&branches)
                .filter_map(|node| {
                    let filter = node.filter()?;
                    let before = filter.synthesis_link_state()?;
                    let adjustment = before.adjustment.as_ref()?;
                    if adjustment.guid() != group_id
                        || adjustment.property_type() != 10
                        || !before.catalog.as_ref()?.same_reference(value)
                    {
                        return None;
                    }
                    let adjustment = renamed_adjustment(
                        adjustment,
                        previous_name,
                        &requested_name,
                        merge.is_some(),
                    )?;
                    let after = SynthesisLinkState {
                        adjustment: Some(adjustment),
                        catalog: Some(destination.clone()),
                        local: before.local.clone(),
                    };
                    Some(SynthesisReceiverEdit::new(&filter, before, after))
                })
                .collect();
            let mut paints = Vec::new();
            for filter in graph_nodes(&branches).filter_map(|node| node.filter()) {
                EnvironmentPaintEdit::stage_synthesis_materials(&mut paints, &filter, |paint| {
                    if paint.key() != Some((group_id, previous_name))
                        || paint
                            .catalog()
                            .is_none_or(|current| !current.same_reference(value))
                    {
                        return Ok::<_, std::convert::Infallible>(None);
                    }
                    let mut paint = paint.clone();
                    let adjustment =
                        Arc::make_mut(paint.adjustment.as_mut().expect("matched Material"));
                    adjustment.components_mut()[0].name = requested_name.clone();
                    paint.resolved = Some((destination.clone(), destination.record()));
                    Ok(Some(paint))
                })
                .unwrap_or_else(|never| match never {});
            }
            MetadataLinks::Synthesis { properties, paints }
        } else {
            let mut links = Vec::new();
            let mut retained: Vec<NumericReceiver<E>> = Vec::new();
            for receiver in graph_nodes(&branches).flat_map(|node| NumericReceiver::for_node(&node))
            {
                if let Some(before) = receiver.retained_adjustment()
                    && before.guid() == group_id
                    && compatible(type_tag, before.property_type())
                    && let Some(after) =
                        renamed_adjustment(&before, previous_name, &requested_name, merge.is_some())
                    && !retained.iter().any(|other| other.same_reference(&receiver))
                {
                    let (numeric, environment) = match &merge_value {
                        Some(MergeValue::Numeric(update)) => (std::slice::from_ref(update), None),
                        Some(MergeValue::Environment(map)) => (&[][..], Some(map)),
                        Some(
                            MergeValue::Synthesis(_)
                            | MergeValue::Curve(_)
                            | MergeValue::Reference(_),
                        ) => {
                            unreachable!("compatible property destination")
                        }
                        None => (&[][..], None),
                    };
                    links.push(
                        LabLinkEdit::prepare(&receiver, before, after, numeric, environment)
                            .map_err(|error| GtxGraphError::Node(error.into()))?,
                    );
                    retained.push(receiver);
                }
            }
            if type_tag == 9 {
                let replacement = match &merge_value {
                    Some(MergeValue::Environment(map)) => Some(map),
                    _ => None,
                };
                let paints = graph_nodes(&branches)
                    .filter_map(|node| node.filter())
                    .filter_map(|filter| {
                        EnvironmentPaintEdit::rename(
                            &filter,
                            (group_id, previous_name),
                            &requested_name,
                            replacement,
                        )
                    })
                    .collect();
                MetadataLinks::Environment {
                    properties: links,
                    paints,
                }
            } else if matches!(type_tag, 0 | 1 | 4) {
                let numeric = match &merge_value {
                    Some(MergeValue::Numeric(update)) => std::slice::from_ref(update),
                    _ => &[],
                };
                let mut maps = if type_tag == 1 {
                    Box::new(EnvironmentAdjusterBatch::empty())
                } else {
                    field_rename.stage_maps(&branches, &self.catalog)?
                };
                let synthesis = SynthesisAdjusterBatch::prepare_mapped(
                    &branches,
                    &self.catalog,
                    &mut maps,
                    |record| {
                        record.rename_numeric_adjustments(
                            group_id,
                            type_tag,
                            previous_name,
                            &requested_name,
                            |adjustment, axis| {
                                map_retained_number::<E>(numeric, adjustment, axis)
                                    .map(|mapped| mapped.map(|(_, value)| value))
                                    .map_err(|error| GtxGraphError::Node(error.into()))
                            },
                        )
                    },
                )?;
                MetadataLinks::Fields {
                    properties: links,
                    references: Vec::new(),
                    maps,
                    synthesis: Some(Box::new(synthesis)),
                }
            } else {
                MetadataLinks::Properties(links)
            }
        };
        let (catalog, label) = if let Some((destination, selection)) = merge {
            (
                CatalogEdit::Merge {
                    destination,
                    selection,
                },
                format!("Merge {} Adjuster", expected.name()),
            )
        } else {
            let mut replacement = metadata.clone();
            replacement.name = Some(requested_name);
            replacement.order = order;
            let record = match current {
                GtxCatalogRecord::Numeric { value, .. } => GtxCatalogRecord::Numeric {
                    metadata: replacement,
                    value: value.clone(),
                },
                GtxCatalogRecord::Environment { value, .. } => GtxCatalogRecord::Environment {
                    metadata: replacement,
                    value: value.clone(),
                },
                GtxCatalogRecord::Synthesis { value, .. } => GtxCatalogRecord::Synthesis {
                    metadata: replacement,
                    value: value.clone(),
                },
                GtxCatalogRecord::Value(record) => {
                    let ValueAdjusterPayload::Color { value, previous } = record.payload else {
                        unreachable!("live editable Color entry");
                    };
                    GtxCatalogRecord::Value(Box::new(ValueAdjusterRecord {
                        metadata: replacement,
                        payload: ValueAdjusterPayload::Color { value, previous },
                    }))
                }
                GtxCatalogRecord::Reference(record) => match &record.payload {
                    ReferenceAdjusterPayload::Gradient { current, previous } => {
                        GtxCatalogRecord::Reference(Box::new(ReferenceAdjusterRecord {
                            metadata: replacement,
                            payload: ReferenceAdjusterPayload::Gradient {
                                current: current.clone(),
                                previous: previous.clone(),
                            },
                        }))
                    }
                    _ => CurveAdjuster::read(current)
                        .expect("live editable Curve entry")
                        .catalog_record(replacement),
                },
            };
            (
                CatalogEdit::Replace(self.catalog.stage_replacement(record, &expected.presence)),
                format!("Edit {} Adjuster", expected.name()),
            )
        };
        let edit = EditLabMetadata {
            source: expected.presence.clone(),
            catalog,
            links,
        };
        let affected = edit
            .publish(HistoryDirection::Redo)
            .map_err(GtxGraphError::History)?;
        self.record_lab_edit(&label, edit)
            .and(affected.notify())
            .map_err(GtxGraphError::History)?;
        Ok(true)
    }
}

pub(super) fn compatible(left: i32, right: i32) -> bool {
    left == right || (matches!(left, 0 | 4) && matches!(right, 0 | 4))
}

fn renamed_adjustment<E>(
    before: &SharedPropertyAdjustment<E>,
    old_name: &[u16],
    new_name: &[u16],
    merge: bool,
) -> Option<SharedPropertyAdjustment<E>> {
    let mut parts = before.capture_parts();
    let mut names = parts.names.as_ref()?.entries();
    let flags = parts.flags.as_ref()?;
    let count = parts.count.max(0) as usize;
    let enabled = |axis| flags.value(axis as i32).unwrap_or(false);
    if merge && !(0..count).any(enabled) {
        return None;
    }
    let mut changed = false;
    for (axis, name) in names.iter_mut().take(count).enumerate() {
        if name.as_deref() == Some(old_name) && (merge || enabled(axis)) {
            *name = Some(new_name.to_vec());
            changed = true;
        }
    }
    changed.then(|| {
        parts.names = Some(SharedSelectionOptions::new(names));
        SharedPropertyAdjustment::from_capture_parts(parts)
    })
}

pub(super) struct LabLinkEdit<E> {
    receiver: WeakNumericReceiver<E>,
    before: SharedPropertyAdjustment<E>,
    after: SharedPropertyAdjustment<E>,
    value: Option<LinkValue>,
}

enum MergeValue {
    Numeric(NumericCatalogUpdate),
    Environment(Arc<crate::EnvironmentRecord>),
    Synthesis(SynthesisCatalogValue),
    Curve(CurveAdjuster),
    Reference(EnvironmentLabReference),
}

impl MergeValue {
    fn read<E>(destination: &GtxCatalogRecord) -> Result<Self, GtxGraphError<E>> {
        if let Some(value) = destination.color_or_gradient() {
            return Ok(Self::Reference(value));
        }
        match destination {
            GtxCatalogRecord::Numeric { metadata, value } => {
                NumericCatalogUpdate::reconnect(metadata, value)
                    .map(Self::Numeric)
                    .ok_or(GtxGraphError::NumericAdjuster(
                        "The destination adjuster has invalid numeric limits.",
                    ))
            }
            GtxCatalogRecord::Environment { value, .. } => Ok(Self::Environment(value.value())),
            GtxCatalogRecord::Synthesis { value, .. } => Ok(Self::Synthesis(value.clone())),
            GtxCatalogRecord::Reference(_) => {
                CurveAdjuster::read(destination).map(Self::Curve).ok_or(
                    GtxGraphError::CurveAdjuster("The destination Curve adjuster has no value."),
                )
            }
            _ => unreachable!("compatible editable Lab entry"),
        }
    }
}

impl<E: 'static> LabLinkEdit<E> {
    pub(super) fn prepare(
        receiver: &NumericReceiver<E>,
        before: SharedPropertyAdjustment<E>,
        after: SharedPropertyAdjustment<E>,
        numeric: &[NumericCatalogUpdate],
        environment: Option<&Arc<crate::EnvironmentRecord>>,
    ) -> Result<Self, PointwiseError<E>> {
        let value = if let Some(map) = environment {
            let NumericReceiver::Filter(parameter) = receiver else {
                return Err(PointwiseError::InvalidCast);
            };
            let crate::PointwiseParameterValue::Environment(previous) = parameter.raw_value()
            else {
                return Err(PointwiseError::InvalidCast);
            };
            Some(LinkValue::Environment {
                before: previous,
                after: map.clone(),
            })
        } else if !numeric.is_empty()
            && let Some(shape) = receiver.shape()?
            && let Some(value) = map_property(numeric, shape, &after)?
        {
            let (before, after) = receiver.prepare(value)?;
            (before != after).then_some(LinkValue::Numeric { before, after })
        } else {
            None
        };
        Ok(Self {
            receiver: receiver.downgrade(),
            before,
            after,
            value,
        })
    }

    fn replace(&self, receiver: &NumericReceiver<E>, undo: bool) -> bool {
        let (previous, next) = if undo {
            (&self.after, &self.before)
        } else {
            (&self.before, &self.after)
        };
        if let Some(LinkValue::Environment { before, after }) = &self.value {
            let NumericReceiver::Filter(parameter) = receiver else {
                return false;
            };
            return parameter.replace_environment_adjustment(
                Some(previous),
                Some(next.clone()),
                (if undo { before } else { after }).clone(),
            );
        }
        receiver.compare_and_replace_adjustment(Some(previous), Some(next.clone()))
    }

    pub(super) fn publish(
        links: &[Self],
        undo: bool,
    ) -> Result<AffectedProperties<E>, HistoryError<E>> {
        let targets = links
            .iter()
            .map(|link| {
                let receiver = link.receiver.upgrade().ok_or(HistoryError::NullTarget)?;
                let before = if undo { &link.after } else { &link.before };
                if receiver
                    .retained_adjustment()
                    .is_none_or(|current| !current.same_reference(before))
                {
                    return Err(HistoryError::InputChanged);
                }
                Ok(receiver)
            })
            .collect::<Result<Vec<_>, _>>()?;
        for (index, (link, receiver)) in links.iter().zip(&targets).enumerate() {
            if !link.replace(receiver, undo) {
                for (link, receiver) in links[..index].iter().zip(&targets[..index]).rev() {
                    link.replace(receiver, !undo);
                }
                return Err(HistoryError::InputChanged);
            }
        }
        let mut affected = AffectedProperties::numeric_only(Vec::new());
        for (link, receiver) in links.iter().zip(targets) {
            match (&link.value, receiver) {
                (Some(LinkValue::Numeric { before, after }), receiver) => {
                    receiver.write(if undo { before } else { after });
                    affected.numeric.push(receiver);
                }
                (Some(LinkValue::Environment { .. }), NumericReceiver::Filter(parameter)) => {
                    affected.environment.properties.push(parameter);
                }
                _ => {}
            }
        }
        Ok(affected)
    }
}

enum LinkValue {
    Numeric {
        before: NumericReceiverState,
        after: NumericReceiverState,
    },
    Environment {
        before: Arc<crate::EnvironmentRecord>,
        after: Arc<crate::EnvironmentRecord>,
    },
}

enum CatalogEdit {
    Replace(Arc<AtomicBool>),
    Merge {
        destination: Arc<AtomicBool>,
        selection: Option<SelectionMerge>,
    },
}

struct SelectionMerge {
    value: NumericCatalogValue,
    before: Vec<SelectionOptionRecord>,
    append: Vec<SelectionOptionRecord>,
}

impl SelectionMerge {
    fn prepare(source: &GtxCatalogRecord, destination: &GtxCatalogRecord) -> Option<Self> {
        let options = |record: &GtxCatalogRecord| {
            let GtxCatalogRecord::Numeric { metadata, value } = record else {
                return None;
            };
            if metadata.type_tag != 4 {
                return None;
            }
            let ValueAdjusterPayload::Number(number) = &*value.lock() else {
                return None;
            };
            Some((value.clone(), number.options.clone()))
        };
        let (_, source) = options(source)?;
        let (value, before) = options(destination)?;
        let append: Vec<_> = source
            .into_iter()
            .filter(|entry| !before.contains(entry))
            .collect();
        (!append.is_empty()).then_some(Self {
            value,
            before,
            append,
        })
    }

    fn validate(&self, actual: &[SelectionOptionRecord], undo: bool) -> bool {
        let tail = if undo { self.append.as_slice() } else { &[] };
        actual.len() == self.before.len() + tail.len()
            && actual
                .iter()
                .zip(self.before.iter().chain(tail))
                .all(|(a, b)| a.name == b.name && a.value.to_bits() == b.value.to_bits())
    }
}

struct EditLabMetadata<E> {
    source: Arc<AtomicBool>,
    catalog: CatalogEdit,
    links: MetadataLinks<E>,
}

enum MetadataLinks<E> {
    Properties(Vec<LabLinkEdit<E>>),
    Fields {
        properties: Vec<LabLinkEdit<E>>,
        references: Vec<LabReferenceEdit<E>>,
        maps: Box<EnvironmentAdjusterBatch<E>>,
        synthesis: Option<Box<SynthesisAdjusterBatch<E>>>,
    },
    Environment {
        properties: Vec<LabLinkEdit<E>>,
        paints: Vec<EnvironmentPaintEdit<E>>,
    },
    Synthesis {
        properties: Vec<SynthesisReceiverEdit<E>>,
        paints: Vec<EnvironmentPaintEdit<E>>,
    },
    Curves(Vec<CurveChange<E>>),
}

impl<E: 'static> EditLabMetadata<E> {
    fn publish(
        &self,
        direction: HistoryDirection,
    ) -> Result<AffectedProperties<E>, HistoryError<E>> {
        let undo = matches!(direction, HistoryDirection::Undo);
        let (destination, active, selection) = match &self.catalog {
            CatalogEdit::Replace(after) => (after, undo, None),
            CatalogEdit::Merge {
                destination,
                selection,
            } => (destination, true, selection.as_ref()),
        };
        if self.source.load(Ordering::Acquire) == undo
            || destination.load(Ordering::Acquire) != active
        {
            return Err(HistoryError::InputChanged);
        }
        let mut payload = selection.map(|edit| edit.value.lock());
        let options = match (selection, payload.as_deref_mut()) {
            (Some(edit), Some(ValueAdjusterPayload::Number(number)))
                if edit.validate(&number.options, undo) =>
            {
                Some((edit, &mut number.options))
            }
            (None, None) => None,
            _ => return Err(HistoryError::InputChanged),
        };
        let affected = match &self.links {
            MetadataLinks::Properties(links) => LabLinkEdit::publish(links, undo)?,
            MetadataLinks::Fields {
                properties,
                references,
                maps,
                synthesis,
            } => {
                let publish_fields = || {
                    let (mut affected, receivers) =
                        LabReferenceEdit::publish_with(references, undo, || {
                            let environment = maps.publish(direction)?;
                            let mut affected =
                                LabLinkEdit::publish(properties, undo).inspect_err(|_| {
                                    let reverse = if undo {
                                        HistoryDirection::Redo
                                    } else {
                                        HistoryDirection::Undo
                                    };
                                    let _ = maps.publish(reverse);
                                })?;
                            affected.environment = environment;
                            Ok(affected)
                        })?;
                    affected.references = receivers;
                    Ok(affected)
                };
                let (mut affected, owners) = match synthesis {
                    Some(batch) => batch.publish(direction, publish_fields)?,
                    None => (publish_fields()?, Vec::new()),
                };
                affected.synthesis = owners;
                affected
            }
            MetadataLinks::Environment { properties, paints } => {
                let owners = EnvironmentPaintEdit::publish_all(paints, undo)?;
                let mut affected = LabLinkEdit::publish(properties, undo).inspect_err(|_| {
                    EnvironmentPaintEdit::rollback_batch(paints, &owners, undo);
                })?;
                affected.environment.paints = owners;
                affected
            }
            MetadataLinks::Curves(links) => {
                let mut affected = AffectedProperties::numeric_only(Vec::new());
                affected.curves = CurveChange::publish_batch(links, direction)?;
                affected
            }
            MetadataLinks::Synthesis { properties, paints } => {
                let mut affected = AffectedProperties::numeric_only(Vec::new());
                let receivers = SynthesisReceiverEdit::publish_batch(properties, undo)?;
                let owners = EnvironmentPaintEdit::publish_all(paints, undo).inspect_err(|_| {
                    SynthesisReceiverEdit::rollback_batch(properties, &receivers, undo);
                })?;
                affected.synthesis = receivers;
                affected.environment.paints = owners;
                affected
            }
        };
        if let Some((edit, options)) = options {
            if undo {
                options.truncate(edit.before.len());
            } else {
                options.extend(edit.append.iter().cloned());
            }
        }
        self.source.store(undo, Ordering::Release);
        if let CatalogEdit::Replace(after) = &self.catalog {
            after.store(!undo, Ordering::Release);
        }
        Ok(affected)
    }
}

impl<E: 'static> HistoryGraphEdit<E> for EditLabMetadata<E> {
    fn apply(&self, direction: HistoryDirection) -> Result<(), HistoryError<E>> {
        self.publish(direction)?.notify()
    }
}
