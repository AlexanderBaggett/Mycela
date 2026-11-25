//! Author Number and Selection links in retained Environment maps.
use super::super::super::numeric_adjusters::{
    NumericCatalogUpdate, environment_mapping_curve, map_environment_number,
};
use super::super::super::numeric_links::initial_value;
use super::*;
use crate::{
    BinaryCursor, BinaryWriter, CurveError, CurveRecord, EnvironmentNumericReceiver,
    LabNumericAdjuster, LabNumericLink, PropertyRecordError, SelectionOptionRecord, SharedCurve,
};

/// A detached mapping draft for one linked Number or Selection field in an Environment snapshot.
pub struct EnvironmentNumericMapping<E = ()> {
    owner: MappingOwner<E>,
    map: Arc<EnvironmentRecord>,
    receiver: EnvironmentNumericReceiver,
    control: NumericCatalogValue,
    initial: Vec<u8>,
    output: [f32; 2],
    bounds: [f32; 2],
    curve: SharedCurve<E>,
}

enum MappingOwner<E> {
    Property(PointwiseParameter<E>),
    Paint {
        owner: PointwiseNode<E>,
        layer: Option<CanvasLayerEditor<E>>,
        pass: usize,
    },
}

impl<E> MappingOwner<E> {
    fn capture(target: EnvironmentMapTarget<'_, E>) -> Self {
        match target {
            EnvironmentMapTarget::Property(property) => Self::Property(property.clone()),
            EnvironmentMapTarget::Paint { owner, layer, pass } => Self::Paint {
                owner: owner.clone(),
                layer: layer.cloned(),
                pass,
            },
        }
    }

    fn target(&self) -> EnvironmentMapTarget<'_, E> {
        match self {
            Self::Property(property) => EnvironmentMapTarget::Property(property),
            Self::Paint { owner, layer, pass } => EnvironmentMapTarget::Paint {
                owner,
                layer: layer.as_ref(),
                pass: *pass,
            },
        }
    }
}

impl<E> EnvironmentNumericMapping<E> {
    pub fn curve(&self) -> &SharedCurve<E> {
        &self.curve
    }

    pub fn bounds(&self) -> [f32; 2] {
        self.bounds
    }
}

impl<E: 'static> Gtx375GraphDocument<E> {
    pub fn environment_numeric_adjuster(
        &self,
        map: &EnvironmentRecord,
        receiver: EnvironmentNumericReceiver,
    ) -> Option<LabNumericAdjuster> {
        let (group, name) = map.numeric_adjuster_key(receiver)?;
        let (metadata, value, _) = self.environment_numeric_entry(group, name)?;
        LabNumericAdjuster::read(metadata, value)
    }

    pub fn available_environment_numeric_adjusters(
        &self,
        target: EnvironmentMapTarget<'_, E>,
        receiver: EnvironmentNumericReceiver,
    ) -> Vec<LabNumericAdjuster> {
        let Some(target) = self
            .environment_map_target(target)
            .filter(|target| target.map.supports_numeric_adjuster(receiver))
        else {
            return Vec::new();
        };
        let group = target.group();
        let parked = self.parked_only_lab_names(&self.branches(), Some(group));
        let mut controls: Vec<_> = self
            .catalog
            .entries()
            .filter_map(|(entry, _)| match entry {
                GtxCatalogRecord::Numeric { metadata, value }
                    if metadata.group_id == group
                        && matches!(metadata.type_tag, 0 | 4)
                        && !parked.contains(metadata) =>
                {
                    Some((metadata, LabNumericAdjuster::read(metadata, value)?))
                }
                _ => None,
            })
            .collect();
        controls.sort_by_cached_key(|(meta, control)| {
            super::super::super::lab_adjuster_metadata::display_order(
                meta.order,
                control.name(),
                meta.type_tag,
            )
        });
        controls.into_iter().map(|(_, control)| control).collect()
    }

    pub fn environment_numeric_mapping(
        &self,
        owner: EnvironmentMapTarget<'_, E>,
        expected: &Arc<EnvironmentRecord>,
        receiver: EnvironmentNumericReceiver,
    ) -> Result<EnvironmentNumericMapping<E>, GtxGraphError<E>> {
        let owner = MappingOwner::capture(owner);
        let target = self.validate_environment_map_target(owner.target(), expected)?;
        let (group, name) =
            target
                .map
                .numeric_adjuster_key(receiver)
                .ok_or(GtxGraphError::EnvironmentAdjuster(
                    "Link an adjuster to this Environment field first.",
                ))?;
        let (_, control, _) = self
            .environment_numeric_entry(group, name)
            .ok_or_else(unavailable_control)?;
        let mut adjustment = receiver
            .scope_adjustment(&target.map, group)
            .ok_or_else(stale_map)?;
        let record = adjustment
            .components_mut()
            .first_mut()
            .ok_or_else(stale_map)?
            .curve_mut();
        let curve = environment_mapping_curve(record).map_err(numeric_error)?;
        record
            .restore_dormant_modulation(&curve)
            .map_err(|error| numeric_error(PointwiseError::from(error)))?;
        let initial = mapping_bytes(&curve)?;
        Ok(EnvironmentNumericMapping {
            map: target.map.clone(),
            owner,
            receiver,
            control: control.clone(),
            initial,
            output: [curve.minimum(), curve.maximum()],
            bounds: receiver.mapping_bounds(expected).ok_or_else(stale_map)?,
            curve,
        })
    }

    /// Replace the field's mapping and resample it from the current Lab value in one history action.
    pub fn edit_environment_numeric_mapping(
        &mut self,
        mapping: &EnvironmentNumericMapping<E>,
    ) -> Result<bool, GtxGraphError<E>> {
        let target = self.validate_environment_map_target(mapping.owner.target(), &mapping.map)?;
        let receiver = mapping.receiver;
        let (group, name) = target
            .map
            .numeric_adjuster_key(receiver)
            .ok_or_else(stale_map)?;
        let (metadata, control, _) = self
            .environment_numeric_entry(group, name)
            .filter(|(_, value, _)| value.lock_order() == mapping.control.lock_order())
            .ok_or_else(unavailable_control)?;
        let [minimum, maximum] = mapping.bounds;
        let output = [mapping.curve.minimum(), mapping.curve.maximum()];
        if output
            .into_iter()
            .zip(mapping.output)
            .any(|(value, before)| {
                !value.is_finite() || (value != before && !(minimum..=maximum).contains(&value))
            })
        {
            return Err(GtxGraphError::EnvironmentAdjuster(
                "Enter output bounds within the Environment field's range.",
            ));
        }

        let bytes = mapping_bytes(&mapping.curve)?;
        if bytes == mapping.initial {
            return Ok(false);
        }
        let mut reader = BinaryCursor::new(&bytes)
            .map_err(|error| numeric_error(PointwiseError::from(CurveError::Binary(error))))?;
        let mut curve = CurveRecord::new(output[0], output[1], mapping.curve.is_wrapped());
        curve.load(&mut reader, 3.75).map_err(|error| {
            numeric_error(PointwiseError::ModernProperty(
                PropertyRecordError::Curve(error).into(),
            ))
        })?;
        curve
            .capture_dormant_modulation(&mapping.curve)
            .map_err(|error| numeric_error(PointwiseError::from(error)))?;
        let mut adjustment = receiver
            .scope_adjustment(&target.map, group)
            .ok_or_else(stale_map)?;
        adjustment
            .components_mut()
            .first_mut()
            .ok_or_else(stale_map)?
            .curve = Some(curve);
        let mut replacement = target.map.as_ref().clone();
        connect(&mut replacement, receiver, adjustment, metadata, control)?;
        let batch = target.into_batch(self, replacement)?;
        self.commit_environment_field_link(batch, None, Vec::new(), "Edit Adjuster Mapping")
    }

    pub fn set_environment_numeric_adjuster(
        &mut self,
        owner: EnvironmentMapTarget<'_, E>,
        expected: &Arc<EnvironmentRecord>,
        receiver: EnvironmentNumericReceiver,
        link: LabNumericLink,
    ) -> Result<bool, GtxGraphError<E>> {
        let target = self.validate_environment_map_target(owner, expected)?;
        let old = target.map.numeric_adjuster_key(receiver);
        let current = receiver
            .current(&target.map)
            .ok_or(GtxGraphError::EnvironmentAdjuster(
                "The selected Environment field is unavailable.",
            ))?;
        let group = target.group();
        let mut replacement = expected.as_ref().clone();
        let link = self.resolve_parked_numeric_link(link, group, 0);
        let (created, label) = match link {
            LabNumericLink::Create(name) => {
                let name: Vec<_> = name.trim().encode_utf16().collect();
                if name.is_empty() {
                    return Err(GtxGraphError::EnvironmentAdjuster(
                        "Enter an adjuster name.",
                    ));
                }
                if self.environment_numeric_entry(group, &name).is_some() {
                    return Err(GtxGraphError::EnvironmentAdjuster(
                        "A numeric adjuster already uses that name in this Lab section.",
                    ));
                }
                let order = self
                    .catalog
                    .entries()
                    .filter(|(entry, _)| entry.metadata().group_id == group)
                    .map(|(entry, _)| entry.metadata().order)
                    .max()
                    .map_or(0, |order| order.saturating_add(1));
                let adjustment = receiver
                    .new_adjustment(expected, group, &name)
                    .ok_or_else(stale_map)?;
                let entry =
                    create_entry(receiver, &adjustment, current, order).map_err(numeric_error)?;
                let GtxCatalogRecord::Numeric { metadata, value } = &entry else {
                    unreachable!("new numeric catalog entry");
                };
                connect(&mut replacement, receiver, adjustment, metadata, value)?;
                (Some(entry), "Create Environment Field Adjuster")
            }
            LabNumericLink::Existing(control) => {
                let (metadata, value) = self
                    .catalog
                    .entries()
                    .find_map(|(entry, _)| match entry {
                        GtxCatalogRecord::Numeric { metadata, value }
                            if metadata.group_id == group
                                && matches!(metadata.type_tag, 0 | 4)
                                && control.matches(value) =>
                        {
                            Some((metadata, value))
                        }
                        _ => None,
                    })
                    .ok_or(GtxGraphError::EnvironmentAdjuster(
                        "This adjuster is no longer available in this Lab section.",
                    ))?;
                let name = metadata.name.as_deref().ok_or_else(stale_map)?;
                if old == Some((group, name)) {
                    return Ok(false);
                }
                let adjustment = receiver
                    .new_adjustment(expected, group, name)
                    .ok_or_else(stale_map)?;
                connect(&mut replacement, receiver, adjustment, metadata, value)?;
                (None, "Link Environment Field Adjuster")
            }
            LabNumericLink::Unlink if old.is_some() => {
                receiver
                    .set_adjustment(&mut replacement, None)
                    .ok_or_else(stale_map)?;
                (None, "Unlink Environment Field Adjuster")
            }
            LabNumericLink::Unlink => return Ok(false),
        };
        let retired = old.and_then(|(group, name)| {
            let (metadata, _, marker) = self.environment_numeric_entry(group, name)?;
            let branches = self.branches();
            (!Self::graph_uses_numeric_adjuster(&branches, metadata)
                && self.count_synthesis_number_links(&branches, metadata) == 0
                && self.count_environment_links(&branches, |map| {
                    map.count_numeric_adjuster(group, name)
                }) <= 1)
                .then(|| marker.clone())
        });
        let batch = target.into_batch(self, replacement)?;
        self.commit_environment_field_link(batch, created, retired.into_iter().collect(), label)
    }

    fn environment_numeric_entry(
        &self,
        group: [u8; 16],
        name: &[u16],
    ) -> Option<(
        &AdjusterMetadataRecord,
        &NumericCatalogValue,
        &Arc<AtomicBool>,
    )> {
        self.catalog
            .entries()
            .find_map(|(entry, presence)| match entry {
                GtxCatalogRecord::Numeric { metadata, value }
                    if metadata.group_id == group
                        && matches!(metadata.type_tag, 0 | 4)
                        && metadata.name.as_deref() == Some(name) =>
                {
                    Some((metadata, value, presence))
                }
                _ => None,
            })
    }
}

fn connect<E: 'static>(
    replacement: &mut EnvironmentRecord,
    receiver: EnvironmentNumericReceiver,
    adjustment: crate::AdjustmentRecord,
    metadata: &AdjusterMetadataRecord,
    value: &NumericCatalogValue,
) -> Result<(), GtxGraphError<E>> {
    let update = NumericCatalogUpdate::reconnect(metadata, value).ok_or(
        GtxGraphError::EnvironmentAdjuster("This adjuster has an unavailable numeric value."),
    )?;
    if let Some((_, value)) =
        map_environment_number(&[update], &adjustment).map_err(numeric_error)?
    {
        receiver.apply(replacement, value).map_err(numeric_error)?;
    }
    receiver
        .set_adjustment(replacement, Some(adjustment))
        .ok_or_else(stale_map)
}

fn create_entry<E: 'static>(
    receiver: EnvironmentNumericReceiver,
    adjustment: &crate::AdjustmentRecord,
    current: f64,
    order: i32,
) -> Result<GtxCatalogRecord, PointwiseError<E>> {
    let component = &adjustment.components()[0];
    let curve = environment_mapping_curve(
        component
            .curve
            .as_ref()
            .ok_or(PointwiseError::NullReference)?,
    )?;
    let kind = if receiver.is_choice() {
        ValueAdjusterKind::Selection
    } else {
        ValueAdjusterKind::Number
    };
    let mut entry = ValueAdjusterRecord::new(kind, Some(component.name.clone()), adjustment.guid);
    entry.metadata.order = order;
    let ValueAdjusterPayload::Number(number) = &mut entry.payload else {
        unreachable!("Number and Selection share the same payload");
    };
    let labels = receiver.options();
    if labels.is_empty() {
        number.value = initial_value(&curve, current, None, [0., 100.], 1)?;
    } else {
        number.options = labels
            .iter()
            .enumerate()
            .map(|(index, label)| {
                let value = initial_value(&curve, index as f64, Some(labels.len()), [0., 100.], 1)?;
                Ok(SelectionOptionRecord {
                    name: Some(label.encode_utf16().collect()),
                    value,
                })
            })
            .collect::<Result<_, PointwiseError<E>>>()?;
        number.selected = current as i32;
        number.previous_selected = number.selected;
        number.value = number
            .options
            .get(number.selected as usize)
            .ok_or(PointwiseError::Index)?
            .value;
    }
    number.previous = number.value;
    Ok(GtxCatalogRecord::Numeric {
        metadata: entry.metadata,
        value: NumericCatalogValue::new(entry.payload),
    })
}

fn mapping_bytes<E>(curve: &SharedCurve<E>) -> Result<Vec<u8>, GtxGraphError<E>> {
    let mut writer = BinaryWriter::default();
    curve
        .save(&mut writer)
        .map_err(|error| numeric_error(PointwiseError::from(error)))?;
    Ok(writer.bytes().to_vec())
}

fn unavailable_control<E>() -> GtxGraphError<E> {
    GtxGraphError::EnvironmentAdjuster("The linked adjuster is no longer available.")
}

fn numeric_error<E>(error: PointwiseError<E>) -> GtxGraphError<E> {
    GtxGraphError::Node(error.into())
}
