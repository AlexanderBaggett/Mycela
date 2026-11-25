use super::environment_adjusters::{enclosing_adjuster_group, graph_nodes};
use super::numeric_adjusters::{NumericCatalogUpdate, limits, map_property};
use super::*;
use crate::history::HistoryGraphEdit;
use crate::pointwise::choice_index_from_numeric;
use crate::pointwise::numeric_adjustment::{
    NumericReceiver, NumericReceiverShape, NumericReceiverState, WeakNumericReceiver,
};
use crate::pointwise::property::NumericAdjustment;
use crate::{
    HistoryDirection, HistoryError, LabNumericProperty, SharedCurve, SharedPropertyAdjustment,
    ValueAdjusterPayload, round_decimal_places,
};
use std::sync::atomic::{AtomicBool, Ordering};

#[path = "numeric_mapping.rs"]
mod mapping;
pub use mapping::LabNumericMapping;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LabNumericAxis {
    Whole,
    X,
    Y,
}

impl LabNumericAxis {
    pub(super) fn index(self) -> Option<usize> {
        match self {
            Self::Whole => None,
            Self::X => Some(0),
            Self::Y => Some(1),
        }
    }
}

#[derive(Clone)]
pub enum LabNumericLink {
    Create(String),
    Existing(LabNumericAdjuster),
    Unlink,
}

#[derive(Default, PartialEq)]
struct Links {
    group: [u8; 16],
    paired: bool,
    names: [Option<Vec<u16>>; 2],
}

impl Links {
    fn read<E>(adjustment: Option<&SharedPropertyAdjustment<E>>) -> Self {
        let Some(adjustment) = adjustment else {
            return Self::default();
        };
        let parts = adjustment.capture_parts();
        let names = parts.names.map(|names| names.entries()).unwrap_or_default();
        Self {
            group: parts.guid,
            paired: parts.property_type == 1,
            names: std::array::from_fn(|axis| {
                if parts
                    .flags
                    .as_ref()
                    .is_some_and(|flags| flags.value(axis as i32).unwrap_or(false))
                {
                    names
                        .get(axis)
                        .cloned()
                        .flatten()
                        .filter(|name| !name.is_empty())
                } else {
                    None
                }
            }),
        }
    }

    fn includes(&self, entry: &AdjusterMetadataRecord) -> bool {
        self.group == entry.group_id
            && compatible(self.paired, entry.type_tag)
            && self
                .names
                .iter()
                .flatten()
                .any(|name| Some(name) == entry.name.as_ref())
    }

    fn active(&self) -> bool {
        self.names.iter().any(Option::is_some)
    }
}

fn compatible(paired: bool, tag: i32) -> bool {
    if paired {
        tag == 1
    } else {
        matches!(tag, 0 | 4)
    }
}

fn link_group<E: 'static>(receiver: &NumericReceiver<E>) -> [u8; 16] {
    enclosing_adjuster_group(&receiver.owner())
        .and_then(|group| group.group_guid())
        .unwrap_or_default()
}

impl<E: 'static> Gtx375GraphDocument<E> {
    pub(super) fn graph_uses_numeric_adjuster(
        branches: &[Option<PointwiseNode<E>>],
        entry: &AdjusterMetadataRecord,
    ) -> bool {
        graph_nodes(branches)
            .flat_map(|node| NumericReceiver::for_node(&node))
            .any(|receiver| Links::read(receiver.retained_adjustment().as_ref()).includes(entry))
    }

    fn contains_numeric_receiver(&self, receiver: &NumericReceiver<E>) -> bool {
        let owner = receiver.owner();
        graph_nodes(&self.branches())
            .filter(|node| node.same_reference(&owner))
            .flat_map(|node| NumericReceiver::for_node(&node))
            .any(|value| value.same_reference(receiver))
    }

    pub fn numeric_property_adjusters(
        &self,
        property: &LabNumericProperty<E>,
        axis: LabNumericAxis,
    ) -> Vec<LabNumericAdjuster> {
        let group = link_group(&property.0);
        let paired = property.is_pair() && axis == LabNumericAxis::Whole;
        let parked = self.parked_only_lab_names(&self.branches(), Some(group));
        self.catalog
            .entries()
            .filter_map(|(entry, _)| match entry {
                GtxCatalogRecord::Numeric { metadata, value }
                    if metadata.group_id == group
                        && compatible(paired, metadata.type_tag)
                        && !parked.contains(metadata) =>
                {
                    LabNumericAdjuster::read(metadata, value)
                }
                _ => None,
            })
            .collect()
    }

    pub fn numeric_property_link_name(
        &self,
        property: &LabNumericProperty<E>,
        axis: LabNumericAxis,
    ) -> Option<String> {
        let links = Links::read(property.0.retained_adjustment().as_ref());
        if property.is_pair() && links.paired != (axis == LabNumericAxis::Whole) {
            return None;
        }
        let name = match axis.index() {
            Some(index) => links.names[index].as_deref(),
            None => links.names.iter().find_map(|name| name.as_deref()),
        };
        name.map(String::from_utf16_lossy)
    }

    /// Connect a whole property or one axis to a named adjuster in its Lab
    /// section. The last disconnect retires the entry and preserves the value.
    pub fn set_numeric_property_link(
        &mut self,
        property: &LabNumericProperty<E>,
        axis: LabNumericAxis,
        action: LabNumericLink,
    ) -> Result<bool, GtxGraphError<E>> {
        let receiver = &property.0;
        if !self.contains_numeric_receiver(receiver) {
            return Err(GtxGraphError::NumericAdjuster(
                "This property is no longer available.",
            ));
        }
        let branches = self.branches();
        let pair = property.is_pair();
        if !pair && axis != LabNumericAxis::Whole {
            return Err(GtxGraphError::NumericAdjuster(
                "This property has one value.",
            ));
        }
        let paired = pair && axis == LabNumericAxis::Whole;
        let group = link_group(receiver);
        let action = self.resolve_parked_numeric_link(action, group, i32::from(paired));
        let previous = receiver.retained_adjustment();
        let before_links = Links::read(previous.as_ref());
        let mut created = None;
        let mut destination = None;
        let name = match action {
            LabNumericLink::Create(name) => {
                let name: Vec<_> = name.trim().encode_utf16().collect();
                if name.is_empty() {
                    return Err(GtxGraphError::NumericAdjuster("Enter an adjuster name."));
                }
                if self.catalog.entries().any(|(entry, _)| {
                    let info = entry.metadata();
                    info.group_id == group
                        && compatible(paired, info.type_tag)
                        && info.name.as_ref() == Some(&name)
                }) {
                    return Err(GtxGraphError::NumericAdjuster(
                        "An adjuster already uses that name in this section.",
                    ));
                }
                created = Some(name.clone());
                Some(name)
            }
            LabNumericLink::Existing(control) => {
                let entry = self.catalog.entries().find_map(|(entry, _)| match entry {
                    GtxCatalogRecord::Numeric { metadata, value }
                        if metadata.group_id == group
                            && compatible(paired, metadata.type_tag)
                            && control.matches(value) =>
                    {
                        Some((metadata, value))
                    }
                    _ => None,
                });
                let (metadata, value) = entry.ok_or(GtxGraphError::NumericAdjuster(
                    "This adjuster is no longer available for the property.",
                ))?;
                destination = NumericCatalogUpdate::reconnect(metadata, value);
                Some(
                    metadata
                        .name
                        .clone()
                        .ok_or(GtxGraphError::NumericAdjuster("This adjuster has no name."))?,
                )
            }
            LabNumericLink::Unlink => {
                if !before_links.active() {
                    return Ok(false);
                }
                None
            }
        };

        let linking = name.is_some();
        let mut after_links = Links {
            group,
            paired,
            names: [None, None],
        };
        if let Some(index) = axis.index() {
            if !before_links.paired && (name.is_none() || before_links.group == group) {
                after_links.names = before_links.names.clone();
                if name.is_none() {
                    after_links.group = before_links.group;
                }
            }
            after_links.names[index] = name;
        } else {
            after_links.names[0] = name.clone();
            if paired {
                after_links.names[1] = name;
            }
        }
        if after_links == before_links {
            return Ok(false);
        }
        let adjustment = if after_links.active() {
            Some(
                receiver
                    .new_adjustment(
                        previous.as_ref(),
                        after_links.paired,
                        after_links.group,
                        &after_links.names[..if pair { 2 } else { 1 }],
                    )
                    .map_err(|error| GtxGraphError::Node(NodeWireError::Property(error)))?,
            )
        } else {
            None
        };
        let created_entry = created
            .as_ref()
            .map(|name| {
                let order = self
                    .catalog
                    .entries()
                    .filter_map(|(entry, _)| {
                        let info = entry.metadata();
                        (info.group_id == group).then_some(info.order)
                    })
                    .max()
                    .map_or(0, |order| order.saturating_add(1));
                let kind = if paired {
                    ValueAdjusterKind::Dual
                } else {
                    ValueAdjusterKind::Number
                };
                let mut entry = ValueAdjusterRecord::new(kind, Some(name.clone()), group);
                entry.metadata.order = order;
                initialize_adjuster(
                    receiver,
                    adjustment.as_ref().expect("new numeric link"),
                    &mut entry.payload,
                    axis,
                )
                .map_err(|error| GtxGraphError::Node(error.into()))?;
                let value = NumericCatalogValue::new(entry.payload);
                destination = NumericCatalogUpdate::reconnect(&entry.metadata, &value);
                Ok::<_, GtxGraphError<E>>(GtxCatalogRecord::Numeric {
                    metadata: entry.metadata,
                    value,
                })
            })
            .transpose()?;
        let state = match (destination, &adjustment) {
            (Some(destination), Some(adjustment)) => {
                let shape = receiver
                    .shape()
                    .map_err(|error| GtxGraphError::Node(error.into()))?
                    .ok_or(GtxGraphError::NumericAdjuster(
                        "This property cannot use a numeric adjuster.",
                    ))?;
                map_property(&[destination], shape, adjustment)
                    .and_then(|value| value.map(|value| receiver.prepare(value)).transpose())
                    .map_err(|error| GtxGraphError::Node(error.into()))?
            }
            _ => None,
        };
        let retired: Vec<_> = self
            .catalog
            .entries()
            .filter_map(|(entry, active)| {
                let info = entry.metadata();
                (matches!(entry, GtxCatalogRecord::Numeric { .. })
                    && before_links.includes(info)
                    && !after_links.includes(info)
                    && (info.type_tag == 1
                        || self.count_environment_links(&branches, |map| {
                            info.name
                                .as_deref()
                                .map_or(0, |name| map.count_numeric_adjuster(info.group_id, name))
                        }) == 0)
                    && self.count_synthesis_number_links(&branches, info) == 0
                    && !graph_nodes(&branches)
                        .flat_map(|node| NumericReceiver::for_node(&node))
                        .any(|candidate| {
                            !candidate.same_reference(receiver)
                                && Links::read(candidate.retained_adjustment().as_ref())
                                    .includes(info)
                        }))
                .then(|| active.clone())
            })
            .collect();
        let label = if created_entry.is_some() {
            "Create Numeric Adjuster"
        } else if linking {
            "Link Numeric Adjuster"
        } else {
            "Unlink Numeric Adjuster"
        };
        drop(branches);
        let edit = NumericLinkEdit {
            property: receiver.downgrade(),
            before: previous,
            after: adjustment,
            state,
            created: created_entry.map(|entry| self.catalog.stage(entry)),
            retired,
        };
        let property = edit
            .publish(HistoryDirection::Redo)
            .map_err(GtxGraphError::History)?;
        self.record_lab_edit(label, edit)
            .and(notify(&property))
            .map_err(GtxGraphError::History)?;
        Ok(true)
    }
}

fn initialize_adjuster<E: 'static>(
    receiver: &NumericReceiver<E>,
    adjustment: &SharedPropertyAdjustment<E>,
    payload: &mut ValueAdjusterPayload,
    axis: LabNumericAxis,
) -> Result<(), PointwiseError<E>> {
    let current = receiver.current()?;
    let option_count = match receiver.shape()? {
        Some(NumericReceiverShape::Choice(count)) => Some(count),
        _ => None,
    };
    let initialize = |axis, target, numeric: &crate::NumericAdjusterMetadata| {
        let range = limits(numeric).ok_or(PointwiseError::NullReference)?;
        let places = numeric
            .limits
            .as_ref()
            .ok_or(PointwiseError::NullReference)?
            .decimal_places;
        initial_value(
            &adjustment.curve(axis)?,
            target,
            option_count,
            range,
            places,
        )
    };
    match payload {
        ValueAdjusterPayload::Number(number) => {
            let index = axis.index().unwrap_or(0);
            let target = match current {
                NumericAdjustment::Number(value) => value,
                NumericAdjustment::Choice(index) => f64::from(index),
                NumericAdjustment::Pair(values) => values[index],
            };
            number.value = initialize(index as i32, target, &number.numeric)?;
            number.previous = number.value;
        }
        ValueAdjusterPayload::Dual(dual) => {
            let NumericAdjustment::Pair(values) = current else {
                return Err(PointwiseError::InvalidCast);
            };
            dual.value = [
                initialize(0, values[0], &dual.numeric)?,
                initialize(1, values[1], &dual.numeric)?,
            ];
            dual.previous = dual.value;
        }
        _ => return Err(PointwiseError::InvalidCast),
    }
    Ok(())
}

pub(super) fn initial_value<E: 'static>(
    curve: &SharedCurve<E>,
    target: f64,
    choices: Option<usize>,
    [min, max]: [f64; 2],
    places: i32,
) -> Result<f64, PointwiseError<E>> {
    let mut target = target;
    let step = if choices.is_some() {
        0.5
    } else {
        0.5 * 0.1_f64.powi(places)
    };
    let [a, b] = [f64::from(curve.minimum()), f64::from(curve.maximum())];
    if choices.is_none() {
        target = target.min(a.max(b)).max(a.min(b));
    }
    let mut selected = min;
    let mut best = max;
    let mut sample = min;
    while sample < max {
        let raw = f64::from(curve.sample(((sample - min) / (max - min)) as f32)?);
        let mapped = match choices {
            Some(count) => f64::from(choice_index_from_numeric::<E>(raw, count)?),
            None => raw,
        };
        let error = (mapped - target).abs();
        if error - best < -0.00001 {
            selected = sample;
            best = error;
        }
        sample += step;
    }
    round_decimal_places(selected.min(max).max(min), places)
        .map_err(PointwiseError::DecimalRounding)
}

struct NumericLinkEdit<E> {
    property: WeakNumericReceiver<E>,
    before: Option<SharedPropertyAdjustment<E>>,
    after: Option<SharedPropertyAdjustment<E>>,
    state: Option<(NumericReceiverState, NumericReceiverState)>,
    created: Option<Arc<AtomicBool>>,
    retired: Vec<Arc<AtomicBool>>,
}

impl<E: 'static> NumericLinkEdit<E> {
    fn publish(&self, direction: HistoryDirection) -> Result<NumericReceiver<E>, HistoryError<E>> {
        let undo = matches!(direction, HistoryDirection::Undo);
        let (before, after) = if undo {
            (&self.after, &self.before)
        } else {
            (&self.before, &self.after)
        };
        let property = self.property.upgrade().ok_or(HistoryError::NullTarget)?;
        if self
            .created
            .as_ref()
            .is_some_and(|entry| entry.load(Ordering::Acquire) != undo)
            || self
                .retired
                .iter()
                .any(|entry| entry.load(Ordering::Acquire) == undo)
            || !property.compare_and_replace_adjustment(before.as_ref(), after.clone())
        {
            return Err(HistoryError::InputChanged);
        }
        if let Some((before, after)) = &self.state {
            property.write(if undo { before } else { after });
        }
        if let Some(entry) = &self.created {
            entry.store(!undo, Ordering::Release);
        }
        for entry in &self.retired {
            entry.store(undo, Ordering::Release);
        }
        Ok(property)
    }
}

fn notify<E: 'static>(property: &NumericReceiver<E>) -> Result<(), HistoryError<E>> {
    property
        .normalize()
        .and_then(|()| property.notify())
        .map_err(|error| HistoryError::Property(Box::new(error)))
}

impl<E: 'static> HistoryGraphEdit<E> for NumericLinkEdit<E> {
    fn apply(&self, direction: HistoryDirection) -> Result<(), HistoryError<E>> {
        notify(&self.publish(direction)?)
    }
}
