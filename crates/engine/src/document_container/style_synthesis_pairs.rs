use super::mapping::SynthesisMappingField;
use super::*;

/// A paired control in a standalone Texture or in a Style or Canvas pass.
#[derive(Clone, Copy)]
pub struct SynthesisPairTarget {
    pass: Option<usize>,
    layer: usize,
    field: SynthesisPairField,
    axis: LabNumericAxis,
}

impl SynthesisPairTarget {
    pub fn new(
        pass: Option<usize>,
        layer: usize,
        field: SynthesisPairField,
        axis: LabNumericAxis,
    ) -> Self {
        Self {
            pass,
            layer,
            field,
            axis,
        }
    }

    fn owner<E>(self, canvas: Option<&CanvasLayerEditor<E>>) -> SynthesisNumberOwner<E> {
        self.pass.map_or(SynthesisNumberOwner::Texture, |pass| {
            SynthesisNumberOwner::paint(canvas, pass)
        })
    }

    fn component_index(self) -> Option<usize> {
        self.axis
            .index()
            .map(|axis| self.field.component_index(axis))
    }
}

impl<E: 'static> Gtx375GraphDocument<E> {
    pub fn synthesis_pair_adjuster(
        &self,
        expected: &SynthesisEdit,
        target: SynthesisPairTarget,
    ) -> Option<LabNumericAdjuster> {
        let adjustment = expected.pair_adjustment(target.layer, target.field)?;
        let axis = match target.component_index() {
            Some(axis) if adjustment.property_type != 1 => axis,
            None if adjustment.property_type == 1 => 0,
            _ => return None,
        };
        let (metadata, value) = self.synthesis_mapping_control(adjustment, axis)?;
        LabNumericAdjuster::read(metadata, value)
    }

    pub fn available_synthesis_pair_adjusters(
        &self,
        owner: &PointwiseNode<E>,
        expected: &SynthesisEdit,
        target: SynthesisPairTarget,
    ) -> Vec<LabNumericAdjuster> {
        if expected.pair_value(target.layer, target.field).is_none() {
            return Vec::new();
        }
        self.available_texture_numeric_adjusters(owner, target.axis == LabNumericAxis::Whole)
    }

    pub fn set_synthesis_pair_adjuster(
        &mut self,
        owner: &PointwiseNode<E>,
        canvas: Option<&CanvasLayerEditor<E>>,
        expected: &SynthesisEdit,
        target: SynthesisPairTarget,
        link: LabNumericLink,
    ) -> Result<bool, GtxGraphError<E>> {
        self.edit_synthesis_pair_link(owner, canvas, expected, target, link, false)
    }

    /// Changes the mode while reusing the current linked name and both mapping curves.
    pub fn convert_synthesis_pair_adjuster(
        &mut self,
        owner: &PointwiseNode<E>,
        canvas: Option<&CanvasLayerEditor<E>>,
        expected: &SynthesisEdit,
        target: SynthesisPairTarget,
    ) -> Result<bool, GtxGraphError<E>> {
        let Some(record) = expected.pair_adjustment(target.layer, target.field) else {
            return Ok(false);
        };
        let dual = target.axis == LabNumericAxis::Whole;
        if dual == (record.property_type == 1) || target.axis == LabNumericAxis::Y {
            return Ok(false);
        }
        let Some((_, _, name)) = link_keys(record).next() else {
            return Ok(false);
        };
        let name = String::from_utf16_lossy(name);
        let existing = self
            .available_synthesis_pair_adjusters(owner, expected, target)
            .into_iter()
            .find(|control| control.name() == name);
        let link = existing.map_or(LabNumericLink::Create(name), LabNumericLink::Existing);
        self.edit_synthesis_pair_link(owner, canvas, expected, target, link, !dual)
    }

    fn edit_synthesis_pair_link(
        &mut self,
        owner: &PointwiseNode<E>,
        canvas: Option<&CanvasLayerEditor<E>>,
        expected: &SynthesisEdit,
        target: SynthesisPairTarget,
        link: LabNumericLink,
        both_individual: bool,
    ) -> Result<bool, GtxGraphError<E>> {
        let branches = self.branches();
        let mut draft =
            SynthesisNumberDraft::capture(&branches, owner, &target.owner(canvas), expected)?;
        let SynthesisPairTarget { layer, field, .. } = target;
        let component = target.component_index();
        if !matches!(link, LabNumericLink::Unlink)
            && SynthesisPairField::AUTOMATIC.contains(&field)
            && draft.synthesis.automatic_placement(layer) != Some(true)
        {
            return Err(GtxGraphError::NumericAdjuster(
                "Enable Automatic Placement before linking an adjuster.",
            ));
        }
        let current = draft
            .synthesis
            .pair_value(layer, field)
            .ok_or_else(stale_paint)?;
        let current = field.swap_axes_if_needed(current);
        let previous = draft.synthesis.pair_adjustment(layer, field);
        let paired = component.is_none();
        let group = environment_adjuster_scope(owner);
        let link = self.resolve_parked_numeric_link(link, group, i32::from(paired));
        let accepts = |metadata: &AdjusterMetadataRecord| {
            metadata.group_id == group
                && if paired {
                    metadata.type_tag == 1
                } else {
                    matches!(metadata.type_tag, 0 | 4)
                }
        };
        let (connection, created) = match link {
            LabNumericLink::Create(name) => {
                let name: Vec<_> = name.trim().encode_utf16().collect();
                if name.is_empty() {
                    return Err(GtxGraphError::NumericAdjuster("Enter an adjuster name."));
                }
                if self.catalog.entries().any(|(entry, _)| {
                    let info = entry.metadata();
                    accepts(info) && info.name.as_deref() == Some(&name)
                }) {
                    return Err(GtxGraphError::NumericAdjuster(
                        "A compatible adjuster already uses that name in this Lab section.",
                    ));
                }
                let kind = if paired {
                    ValueAdjusterKind::Dual
                } else {
                    ValueAdjusterKind::Number
                };
                let mut entry = ValueAdjusterRecord::new(kind, Some(name), group);
                entry.metadata.order = self
                    .catalog
                    .entries()
                    .filter_map(|(entry, _)| {
                        let info = entry.metadata();
                        (info.group_id == group).then_some(info.order)
                    })
                    .max()
                    .map_or(0, |order| order.saturating_add(1));
                let name = entry
                    .metadata
                    .name
                    .as_deref()
                    .ok_or_else(unavailable_control)?;
                let (record, axes) = pair_connection(
                    previous,
                    field,
                    component,
                    Some((group, name)),
                    both_individual,
                );
                initialize_pair(&record, component, current, &mut entry.payload)?;
                let value = NumericCatalogValue::new(entry.payload);
                let mapped = sample_pair(&record, axes, &entry.metadata, &value)?;
                let created = GtxCatalogRecord::Numeric {
                    metadata: entry.metadata,
                    value,
                };
                (Some((record, mapped)), Some(created))
            }
            LabNumericLink::Existing(control) => {
                let (metadata, value) = self
                    .catalog
                    .entries()
                    .find_map(|(entry, _)| match entry {
                        GtxCatalogRecord::Numeric { metadata, value }
                            if accepts(metadata) && control.matches(value) =>
                        {
                            Some((metadata, value))
                        }
                        _ => None,
                    })
                    .ok_or_else(unavailable_control)?;
                let name = metadata.name.as_deref().ok_or_else(unavailable_control)?;
                let (record, axes) = pair_connection(
                    previous,
                    field,
                    component,
                    Some((group, name)),
                    both_individual,
                );
                if same_pair_link(previous, &record) {
                    return Ok(false);
                }
                let mapped = sample_pair(&record, axes, metadata, value)?;
                (Some((record, mapped)), None)
            }
            LabNumericLink::Unlink => {
                let Some(previous) = previous else {
                    return Ok(false);
                };
                if paired != (previous.property_type == 1)
                    || link_key(previous, component.unwrap_or(0)).is_none()
                {
                    return Ok(false);
                }
                let (record, _) = pair_connection(Some(previous), field, component, None, false);
                let any_active = link_keys(&record).next().is_some();
                (any_active.then_some((record, [None; 2])), None)
            }
        };
        let removed: Vec<_> = previous
            .into_iter()
            .flat_map(|before| {
                (0..before.components().len()).filter_map(|axis| {
                    let key = link_key(before, axis)?;
                    let next = connection
                        .as_ref()
                        .and_then(|(after, _)| link_key(after, axis));
                    (Some(key) != next).then_some(key)
                })
            })
            .collect();
        let retired = self.retired_synthesis_number_links(&branches, &removed);
        let label = if created.is_some() {
            "Create Synthesis Pair Adjuster"
        } else if connection
            .as_ref()
            .is_some_and(|(_, values)| values.iter().any(Option::is_some))
        {
            "Link Synthesis Pair Adjuster"
        } else {
            "Unlink Synthesis Pair Adjuster"
        };
        if !draft
            .synthesis
            .set_pair_adjustment(layer, field, connection)
            .ok_or_else(stale_paint)?
        {
            return Ok(false);
        }
        drop(branches);
        self.commit_synthesis_number_change(draft.into_edit()?, created, retired, label)
    }

    pub fn synthesis_pair_mapping(
        &self,
        owner: &PointwiseNode<E>,
        canvas: Option<&CanvasLayerEditor<E>>,
        expected: &SynthesisEdit,
        target: SynthesisPairTarget,
    ) -> Result<StyleSynthesisNumberMapping<E>, GtxGraphError<E>> {
        let axis = target.component_index().ok_or_else(unavailable_control)?;
        self.texture_number_mapping(
            owner,
            target.owner(canvas),
            expected,
            SynthesisMappingField::Pair {
                layer: target.layer,
                field: target.field,
                axis,
            },
        )
    }
}

fn pair_connection(
    previous: Option<&AdjustmentRecord>,
    field: SynthesisPairField,
    component_index: Option<usize>,
    destination: Option<([u8; 16], &[u16])>,
    both_individual: bool,
) -> (AdjustmentRecord, [bool; 2]) {
    let paired = component_index.is_none();
    let mut record = previous
        .cloned()
        .unwrap_or_else(|| AdjustmentRecord::new(&[field.range().map(|value| value as f32); 2]));
    let group = destination.map_or(record.guid, |(group, _)| group);
    let retain = group == record.guid && paired == (record.property_type == 1);
    record.guid = group;
    if !retain {
        record.property_type = i32::from(paired);
    }
    record.mode = i32::from(paired);
    let mut selected = [false; 2];
    for (index, component) in record.components_mut().iter_mut().enumerate() {
        let edit_axis = both_individual || component_index.is_none_or(|axis| axis == index);
        if edit_axis || !retain {
            let name = destination
                .filter(|_| edit_axis)
                .map_or(&[][..], |(_, name)| name);
            component.name = name.to_vec();
            component.active = !name.is_empty();
            component.curve_mut();
            selected[index] = component.active;
        }
    }
    (record, selected)
}

fn same_pair_link(previous: Option<&AdjustmentRecord>, after: &AdjustmentRecord) -> bool {
    previous.is_some_and(|before| {
        before.mode == after.mode
            && (0..2).all(|axis| link_key(before, axis) == link_key(after, axis))
    })
}

fn initialize_pair<E: 'static>(
    record: &AdjustmentRecord,
    component_index: Option<usize>,
    current: [f64; 2],
    payload: &mut ValueAdjusterPayload,
) -> Result<(), GtxGraphError<E>> {
    let assign = |axis: usize, numeric: &crate::NumericAdjusterMetadata| {
        let component = &record.components()[axis];
        let curve =
            environment_mapping_curve(component.curve.as_ref().ok_or_else(unavailable_control)?)
                .map_err(|error| GtxGraphError::Node(error.into()))?;
        let range = limits(numeric).ok_or_else(unavailable_control)?;
        let places = numeric
            .limits
            .as_ref()
            .ok_or_else(unavailable_control)?
            .decimal_places;
        initial_value(&curve, current[axis], None, range, places)
            .map_err(|error| GtxGraphError::Node(error.into()))
    };
    match payload {
        ValueAdjusterPayload::Number(number) => {
            let value = assign(
                component_index.ok_or_else(unavailable_control)?,
                &number.numeric,
            )?;
            number.value = value;
            number.previous = value;
        }
        ValueAdjusterPayload::Dual(dual) => {
            let value = [assign(0, &dual.numeric)?, assign(1, &dual.numeric)?];
            dual.value = value;
            dual.previous = value;
        }
        _ => return Err(unavailable_control()),
    }
    Ok(())
}

fn sample_pair<E: 'static>(
    record: &AdjustmentRecord,
    axes: [bool; 2],
    metadata: &AdjusterMetadataRecord,
    value: &NumericCatalogValue,
) -> Result<[Option<f64>; 2], GtxGraphError<E>> {
    let mut mapped = [None; 2];
    for axis in 0..2 {
        if axes[axis] {
            mapped[axis] = Some(sample_connection(record, axis, metadata, value)?);
        }
    }
    Ok(mapped)
}
