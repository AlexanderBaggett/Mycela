//! Lab controls for paired Synthesis fields.
use super::adjusters::{AdjusterMenu, NumberControls, Receiver, render_effective_value};
use super::*;
use mycela_engine::{CanvasLayerEditor, LabNumericAxis, SynthesisPairField, SynthesisPairTarget};

impl Target {
    pub(super) fn pair_receiver(
        &self,
        field: SynthesisPairField,
        axis: LabNumericAxis,
    ) -> Option<(
        &Node,
        Option<&CanvasLayerEditor<ForegroundError>>,
        SynthesisPairTarget,
    )> {
        let layer = self.layer?;
        let (node, canvas, pass) = if let Some(style) = &self.style {
            let (node, canvas, pass) = style.synthesis_number_receiver()?;
            (node, canvas, Some(pass))
        } else {
            (&self.node, None, None)
        };
        Some((
            node,
            canvas,
            SynthesisPairTarget::new(pass, layer, field, axis),
        ))
    }
}

impl Panel {
    pub(super) fn linked_automatic_pairs(&self, layer: usize) -> bool {
        SynthesisPairField::AUTOMATIC.into_iter().any(|field| {
            self.edit
                .pair_adjustment(layer, field)
                .is_some_and(|record| {
                    record
                        .components()
                        .iter()
                        .any(|component| component.active && !component.name.is_empty())
                })
        })
    }
}

fn pair_id(field: SynthesisPairField, axis: LabNumericAxis) -> String {
    let field = match field {
        SynthesisPairField::GridFrequency => "grid-frequency",
        SynthesisPairField::Move => "move",
        SynthesisPairField::PhotoRepeat => "photo-repeat",
        SynthesisPairField::ManualRepeat => "manual-repeat",
    };
    let axis = match axis {
        LabNumericAxis::X => "x",
        LabNumericAxis::Y => "y",
        LabNumericAxis::Whole => "dual",
    };
    format!("synthesis-control-{field}-{axis}")
}

impl GraphDocumentView {
    pub(super) fn render_synthesis_pair_adjusters(
        &self,
        panel: &Panel,
        target: &Target,
        (field, coordinate): (SynthesisPairField, usize),
        disabled: bool,
        cx: &Context<Self>,
    ) -> Option<NumberControls> {
        let layer = target.layer?;
        let axis = [LabNumericAxis::X, LabNumericAxis::Y][coordinate];
        let raw = panel.edit.pair_adjustment(layer, field);
        let names: [Option<String>; 2] = std::array::from_fn(|axis| {
            raw.and_then(|record| record.components().get(field.component_index(axis)))
                .filter(|component| component.active && !component.name.is_empty())
                .map(|component| String::from_utf16_lossy(&component.name))
        });
        let dual = raw
            .is_some_and(|record| record.property_type == 1)
            .then(|| names.iter().flatten().next().cloned())
            .flatten();
        let any_linked = names.iter().any(Option::is_some);
        let material = target.material_linked();
        let linked_axis = if dual.is_some() {
            LabNumericAxis::Whole
        } else {
            axis
        };
        let axis_linked = dual.is_some() || names[coordinate].is_some();
        let resolved = material
            || !axis_linked
            || target
                .pair_receiver(field, linked_axis)
                .is_some_and(|(_, _, receiver)| {
                    guard(&self.document)
                        .synthesis_pair_adjuster(&panel.edit, receiver)
                        .is_some()
                });
        let value = any_linked.then(|| {
            let value = panel
                .edit
                .pair_value(layer, field)
                .map(|values| number_text(values[coordinate]))
                .unwrap_or_else(|| "Unavailable".into());
            render_effective_value(&pair_id(field, axis), value, resolved, cx)
        });
        if material {
            return Some(NumberControls { value, menu: None });
        }

        let menu = |axis, name, prompt| {
            let (node, _, receiver) = target.pair_receiver(field, axis)?;
            let (current, choices) = {
                let document = guard(&self.document);
                (
                    document.synthesis_pair_adjuster(&panel.edit, receiver),
                    document.available_synthesis_pair_adjusters(node, &panel.edit, receiver),
                )
            };
            Some(self.render_synthesis_link_menu(
                target,
                AdjusterMenu {
                    receiver: Receiver::Pair(field, axis),
                    id: pair_id(field, axis),
                    prompt,
                    name,
                    current,
                    choices,
                    inactive: false,
                    disabled: disabled
                        || (SynthesisPairField::AUTOMATIC.contains(&field)
                            && panel.automatic_placement_off()),
                },
                cx,
            ))
        };
        let slider_name = dual.is_none().then(|| names[coordinate].clone()).flatten();
        let slider = menu(
            axis,
            slider_name,
            if coordinate == 0 {
                "Link X slider"
            } else {
                "Link Y slider"
            },
        );
        let dual = (coordinate == 0)
            .then(|| menu(LabNumericAxis::Whole, dual, "Link Dual Numeric"))
            .flatten();
        let menus = if matches!(
            field,
            SynthesisPairField::PhotoRepeat | SynthesisPairField::ManualRepeat
        ) {
            div().v_flex().w_full().min_w_0()
        } else {
            div().h_flex().flex_wrap()
        };
        Some(NumberControls {
            value,
            menu: Some(
                menus
                    .gap_1()
                    .children(dual)
                    .children(slider)
                    .into_any_element(),
            ),
        })
    }
}
