use super::super::lab_presets::AffectedProperties;
use super::super::synthesis_adjusters::{PatchShapeInputs, SynthesisAdjusterBatch};
use super::*;
use crate::history::HistoryGraphEdit;
use crate::pointwise::property::wire::framework_decimal_from_f64;
use crate::{LimitsRecord, RangeOptionsRecord, SelectionOptionRecord};
use std::sync::atomic::Ordering;

/// A scalar adjuster's mode and, for Selection, its ordered choices.
pub enum LabScalarDefinition {
    Number,
    Selection {
        options: Vec<SelectionOptionRecord>,
        selected: Option<usize>,
    },
}

impl LabScalarDefinition {
    pub fn default_selection() -> Self {
        Self::Selection {
            options: [0.0, 100.0]
                .into_iter()
                .enumerate()
                .map(|(index, value)| SelectionOptionRecord {
                    name: Some(format!("Option {}", index + 1).encode_utf16().collect()),
                    value,
                })
                .collect(),
            selected: Some(0),
        }
    }
}

impl<E: 'static> Gtx375GraphDocument<E> {
    /// Change a Number or Selection definition without changing its name or links.
    /// Each receiver and the catalog entry travel together through history.
    pub fn edit_lab_scalar_definition(
        &mut self,
        selected: Option<&PointwiseNode<E>>,
        control: &LabNumericAdjuster,
        definition: LabScalarDefinition,
    ) -> Result<bool, GtxGraphError<E>> {
        self.edit_lab_scalar_definition_with_patch_input_sizes(selected, control, definition, &[])
    }

    pub fn edit_lab_scalar_definition_with_patch_input_sizes(
        &mut self,
        selected: Option<&PointwiseNode<E>>,
        control: &LabNumericAdjuster,
        definition: LabScalarDefinition,
        input_sizes: &[(u64, crate::ImageGeometry)],
    ) -> Result<bool, GtxGraphError<E>> {
        let entry = self.catalog_adjuster(selected, control)?;
        let retired = entry.presence.clone();
        let mut metadata = entry.metadata.clone();
        let mut number = {
            let payload = entry.value.lock();
            let ValueAdjusterPayload::Number(number) = &*payload else {
                return Err(GtxGraphError::NumericAdjuster(
                    "Only Number and Selection adjusters have options.",
                ));
            };
            if control.before != CatalogState::read(&payload)
                || (metadata.type_tag == 4
                    && !number
                        .options
                        .iter()
                        .map(|option| (&option.name, option.value.to_bits()))
                        .eq(control
                            .selection_options
                            .iter()
                            .map(|option| (&option.name, option.value.to_bits()))))
            {
                return Err(GtxGraphError::NumericAdjuster(
                    "This adjuster changed. Reopen it to edit its current settings.",
                ));
            }
            number.clone()
        };
        match definition {
            LabScalarDefinition::Number => {
                if metadata.type_tag == 0 {
                    return Ok(false);
                }
                let range = limits(&number.numeric).ok_or(GtxGraphError::NumericAdjuster(
                    "This adjuster has no numeric limits.",
                ))?;
                number.value = normalize_adjuster_number(number.value, range, &number.numeric)?;
                number.options.clear();
                number.selected = -1;
                metadata.type_tag = 0;
            }
            LabScalarDefinition::Selection { options, selected } => {
                let selected = match selected {
                    Some(index) if index < options.len() => i32::try_from(index).map_err(|_| {
                        GtxGraphError::NumericAdjuster("This adjuster has too many options.")
                    })?,
                    Some(_) => {
                        return Err(GtxGraphError::NumericAdjuster(
                            "Choose an available option.",
                        ));
                    }
                    None => -1,
                };
                if options.iter().any(|option| {
                    !(0.0..=100.0).contains(&option.value) && !number.options.contains(option)
                }) {
                    return Err(GtxGraphError::NumericAdjuster(
                        "Enter option values from 0 to 100.",
                    ));
                }
                if metadata.type_tag == 4
                    && options == number.options
                    && selected == number.selected
                {
                    return Ok(false);
                }
                if let Some(active) = options.get(selected as usize)
                    && (metadata.type_tag != 4
                        || number.options.get(number.selected as usize) != Some(active))
                {
                    number.value = active.value;
                }
                number.options = options;
                number.selected = selected;
                metadata.type_tag = 4;
            }
        }
        number.previous = number.value;
        number.previous_selected = number.selected;

        self.replace_numeric_definition(
            metadata,
            retired,
            ValueAdjusterPayload::Number(number),
            control.name(),
            input_sizes,
        )
    }

    /// Edit a Number or Dual range. Applying a range to a Selection converts
    /// it to Number in the same history command as the range edit.
    pub fn edit_lab_numeric_range(
        &mut self,
        selected: Option<&PointwiseNode<E>>,
        control: &LabNumericAdjuster,
        range: LabNumericRange,
    ) -> Result<bool, GtxGraphError<E>> {
        self.edit_lab_numeric_range_with_patch_input_sizes(selected, control, range, &[])
    }

    pub fn edit_lab_numeric_range_with_patch_input_sizes(
        &mut self,
        selected: Option<&PointwiseNode<E>>,
        control: &LabNumericAdjuster,
        range: LabNumericRange,
        input_sizes: &[(u64, crate::ImageGeometry)],
    ) -> Result<bool, GtxGraphError<E>> {
        let entry = self.catalog_adjuster(selected, control)?;
        let retired = entry.presence.clone();
        let mut metadata = entry.metadata.clone();
        let mut payload = {
            let payload = entry.value.lock();
            if control.before != CatalogState::read(&payload) {
                return Err(GtxGraphError::NumericAdjuster(
                    "This adjuster changed. Reopen it to edit its current settings.",
                ));
            }
            match &*payload {
                ValueAdjusterPayload::Number(number) => {
                    ValueAdjusterPayload::Number(number.clone())
                }
                ValueAdjusterPayload::Dual(dual) => ValueAdjusterPayload::Dual(dual.clone()),
                _ => unreachable!("live numeric catalog entry"),
            }
        };
        let numeric = match &mut payload {
            ValueAdjusterPayload::Number(number) => &mut number.numeric,
            ValueAdjusterPayload::Dual(dual) => &mut dual.numeric,
            _ => unreachable!("live numeric catalog entry"),
        };
        if !update_range(numeric, range)? && metadata.type_tag != 4 {
            return Ok(false);
        }
        let bounds = limits(numeric).expect("validated numeric limits");
        match &mut payload {
            ValueAdjusterPayload::Number(number) => {
                number.value = normalize_adjuster_number(number.value, bounds, &number.numeric)?;
                number.previous = number.value;
                if metadata.type_tag == 4 {
                    number.options.clear();
                    number.selected = -1;
                    number.previous_selected = -1;
                    metadata.type_tag = 0;
                }
            }
            ValueAdjusterPayload::Dual(dual) => {
                for component in &mut dual.value {
                    *component = normalize_adjuster_number(*component, bounds, &dual.numeric)?;
                }
                dual.previous = dual.value;
            }
            _ => unreachable!("live numeric catalog entry"),
        }
        self.replace_numeric_definition(metadata, retired, payload, control.name(), input_sizes)
    }

    fn replace_numeric_definition(
        &mut self,
        metadata: AdjusterMetadataRecord,
        retired: Arc<AtomicBool>,
        payload: ValueAdjusterPayload,
        name: &str,
        input_sizes: &[(u64, crate::ImageGeometry)],
    ) -> Result<bool, GtxGraphError<E>> {
        let value = NumericCatalogValue::new(payload);
        let update = NumericCatalogUpdate::reconnect(&metadata, &value).ok_or(
            GtxGraphError::NumericAdjuster("The new definition could not be applied."),
        )?;
        let branches = self.branches();
        let mut environment =
            super::super::environment_adjusters::EnvironmentAdjusterBatch::empty();
        environment.stage_receivers(&branches, &self.catalog, std::slice::from_ref(&update))?;
        let synthesis = SynthesisAdjusterBatch::prepare(
            &branches,
            &self.catalog,
            &mut environment,
            Vec::new(),
            std::slice::from_ref(&update),
            None,
            PatchShapeInputs::Authoring(input_sizes),
        )?;
        let batch = NumericAdjusterBatch::prepare(&branches, vec![update])?;
        let replacement = GtxCatalogRecord::Numeric { metadata, value };
        let added = self.catalog.stage_replacement(replacement, &retired);
        let edit = DefinitionEdit {
            batch,
            environment,
            synthesis,
            added,
            retired,
        };
        let affected = edit
            .publish(HistoryDirection::Redo)
            .map_err(GtxGraphError::History)?;
        self.record_lab_edit(&format!("Configure {name} Adjuster"), edit)
            .and(affected.notify())
            .map_err(GtxGraphError::History)?;
        Ok(true)
    }
}

fn update_range<E>(
    numeric: &mut NumericAdjusterMetadata,
    requested: LabNumericRange,
) -> Result<bool, GtxGraphError<E>> {
    let existing = LabNumericRange::read(numeric).ok_or(GtxGraphError::NumericAdjuster(
        "This adjuster has no numeric limits.",
    ))?;
    if requested == existing {
        return Ok(false);
    }
    let accuracy_changed = requested.whole_numbers() != existing.whole_numbers();
    let current = numeric.limits.as_ref().expect("validated numeric limits");
    let places = if accuracy_changed {
        i32::from(!requested.whole_numbers())
    } else {
        current.decimal_places
    };
    let mut bounds = [current.minimum, current.maximum];
    for (axis, number) in requested.bounds().into_iter().enumerate() {
        let changed = number != existing.bounds()[axis];
        if changed && !(-1000.0..=1000.0).contains(&number) {
            return Err(GtxGraphError::NumericAdjuster(
                "Enter bounds from −1000 to 1000.",
            ));
        }
        if changed || (accuracy_changed && requested.whole_numbers()) {
            let rounded = crate::round_decimal_places(number, places).map_err(|error| {
                GtxGraphError::Node(PointwiseError::DecimalRounding(error).into())
            })?;
            bounds[axis] = framework_decimal_from_f64::<E>(rounded)
                .map_err(|error| GtxGraphError::Node(error.into()))?;
        }
    }
    let [minimum, maximum] = bounds;
    if minimum.to_framework_f64() >= maximum.to_framework_f64() {
        return Err(GtxGraphError::NumericAdjuster(
            "Maximum must be greater than minimum at the chosen accuracy.",
        ));
    }
    let default = LimitsRecord::default();
    numeric.limits = Some(LimitsRecord {
        decimal_places: places,
        minimum,
        maximum,
        increment: if accuracy_changed {
            default.increment
        } else {
            current.increment
        },
        big_increment: if accuracy_changed {
            default.big_increment
        } else {
            current.big_increment
        },
        use_range: Some(RangeOptionsRecord::new(minimum, maximum)),
        ..default
    });
    if accuracy_changed {
        numeric.accuracy = places;
    }
    Ok(true)
}

struct DefinitionEdit<E> {
    batch: NumericAdjusterBatch<E>,
    environment: super::super::environment_adjusters::EnvironmentAdjusterBatch<E>,
    synthesis: SynthesisAdjusterBatch<E>,
    added: Arc<AtomicBool>,
    retired: Arc<AtomicBool>,
}

impl<E: 'static> DefinitionEdit<E> {
    fn publish(
        &self,
        direction: HistoryDirection,
    ) -> Result<AffectedProperties<E>, HistoryError<E>> {
        let undo = matches!(direction, HistoryDirection::Undo);
        let ((environment, synthesis), numeric) = self.batch.publish(direction, || {
            if self.added.load(Ordering::Acquire) != undo
                || self.retired.load(Ordering::Acquire) == undo
            {
                return Err(HistoryError::InputChanged);
            }
            let affected = self
                .synthesis
                .publish(direction, || self.environment.publish(direction))?;
            self.retired.store(undo, Ordering::Release);
            self.added.store(!undo, Ordering::Release);
            Ok(affected)
        })?;
        let mut affected = AffectedProperties::numeric_only(numeric);
        affected.environment = environment;
        affected.synthesis = synthesis;
        Ok(affected)
    }
}

impl<E: 'static> HistoryGraphEdit<E> for DefinitionEdit<E> {
    fn apply(&self, direction: HistoryDirection) -> Result<(), HistoryError<E>> {
        self.publish(direction)?.notify()
    }
}
