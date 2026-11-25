use super::{
    numeric_adjusters::{PatchInputDraft, number_field},
    numeric_field::{NumericField, NumericRange},
    *,
};
use gpui_kit::component::radio::{Radio, RadioGroup};
use gpui_kit::rems;
use mycela_engine::{
    LabNumericAdjuster, LabNumericRange, LabNumericValue, LabScalarDefinition,
    SelectionOptionRecord,
};

struct OptionDraft {
    id: usize,
    name: Entity<InputState>,
    value: Entity<InputState>,
    slider_range: NumericRange,
    original_name: Option<Vec<u16>>,
}

impl OptionDraft {
    fn new(id: usize, record: SelectionOptionRecord, window: &mut Window, cx: &mut App) -> Self {
        let name = record
            .name
            .as_deref()
            .map_or_else(String::new, String::from_utf16_lossy);
        let value = record.value.to_string();
        let slider_range = NumericRange::new(0.0, 100.0, 0.1).including(record.value);
        Self {
            id,
            name: cx.new(|cx| InputState::new(window, cx).default_value(name)),
            value: cx.new(|cx| InputState::new(window, cx).default_value(value)),
            slider_range,
            original_name: record.name,
        }
    }

    fn number(&self, cx: &App) -> Option<f64> {
        self.value
            .read(cx)
            .value()
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|number| number.is_finite())
    }

    fn record(&self, cx: &App) -> Result<SelectionOptionRecord, String> {
        let text = self.name.read(cx).value();
        let original = self
            .original_name
            .as_deref()
            .map_or_else(String::new, String::from_utf16_lossy);
        let name = if original == text.as_ref() {
            self.original_name.clone()
        } else {
            Some(text.encode_utf16().collect())
        };
        let value = self
            .number(cx)
            .ok_or_else(|| "Enter a number for every option.".to_owned())?;
        Ok(SelectionOptionRecord { name, value })
    }
}

struct DefinitionForm {
    dual: bool,
    selection_mode: bool,
    bounds: [Entity<InputState>; 2],
    whole_numbers: bool,
    options: Vec<OptionDraft>,
    selected: Option<usize>,
    next_id: usize,
    error: Option<String>,
    patch_inputs: PatchInputDraft,
}

enum DefinitionChange {
    Range(LabNumericRange),
    Scalar(LabScalarDefinition),
}

impl DefinitionForm {
    fn new(control: &LabNumericAdjuster, window: &mut Window, cx: &mut App) -> Self {
        let (selection_mode, options, selected) = match control.value() {
            LabNumericValue::Selection(index) => {
                let options = control.selection_options().to_vec();
                let selected = usize::try_from(index)
                    .ok()
                    .filter(|index| *index < options.len());
                (true, options, selected)
            }
            LabNumericValue::Dual(_) => (false, Vec::new(), None),
            LabNumericValue::Number(_) => {
                let LabScalarDefinition::Selection { options, selected } =
                    LabScalarDefinition::default_selection()
                else {
                    unreachable!("selection defaults")
                };
                (false, options, selected)
            }
        };
        let range = control
            .range_settings()
            .unwrap_or(LabNumericRange::new([0., 100.], false));
        Self {
            dual: matches!(control.value(), LabNumericValue::Dual(_)),
            selection_mode,
            bounds: range.bounds().map(|value| {
                cx.new(|cx| InputState::new(window, cx).default_value(value.to_string()))
            }),
            whole_numbers: range.whole_numbers(),
            selected,
            next_id: options.len(),
            options: options
                .into_iter()
                .enumerate()
                .map(|(id, option)| OptionDraft::new(id, option, window, cx))
                .collect(),
            error: None,
            patch_inputs: PatchInputDraft::default(),
        }
    }

    fn definition(&self, cx: &App) -> Result<DefinitionChange, String> {
        if !self.selection_mode {
            let [minimum, maximum] = self
                .bounds
                .each_ref()
                .map(|input| input.read(cx).value().trim().parse::<f64>().ok());
            let minimum = minimum.ok_or_else(|| "Enter a minimum value.".to_owned())?;
            let maximum = maximum.ok_or_else(|| "Enter a maximum value.".to_owned())?;
            return Ok(DefinitionChange::Range(LabNumericRange::new(
                [minimum, maximum],
                self.whole_numbers,
            )));
        }
        let options = self
            .options
            .iter()
            .map(|option| option.record(cx))
            .collect::<Result<Vec<_>, _>>()?;
        let selected = self
            .selected
            .and_then(|id| self.options.iter().position(|option| option.id == id));
        Ok(DefinitionChange::Scalar(LabScalarDefinition::Selection {
            options,
            selected,
        }))
    }

    fn add_option(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let count = self.options.len();
        let evenly_spaced = |index: usize, intervals: usize| {
            (index as f64 * 1000.0 / intervals.max(1) as f64).round_ties_even() / 10.0
        };
        let default_layout = self.options.iter().enumerate().all(|(index, option)| {
            option.name.read(cx).value().as_ref() == format!("Option {}", index + 1)
                && option.number(cx).is_some_and(|value| {
                    (value - evenly_spaced(index, count.saturating_sub(1))).abs() <= 0.01
                })
        });
        let value = self
            .options
            .last()
            .and_then(|option| option.number(cx))
            .unwrap_or(0.0);
        if default_layout {
            for (index, option) in self.options.iter().enumerate() {
                option.value.update(cx, |input, cx| {
                    input.set_value(evenly_spaced(index, count).to_string(), window, cx);
                });
            }
        }
        let id = self.next_id;
        self.next_id += 1;
        let option = SelectionOptionRecord {
            name: Some(format!("Option {}", count + 1).encode_utf16().collect()),
            value,
        };
        self.options.push(OptionDraft::new(id, option, window, cx));
        if count == 0 {
            self.selected = Some(id);
        }
        self.error = None;
        cx.notify();
    }

    fn move_option(&mut self, id: usize, offset: isize, cx: &mut Context<Self>) {
        if let Some(index) = self.options.iter().position(|option| option.id == id)
            && let Some(other) = index
                .checked_add_signed(offset)
                .filter(|other| *other < self.options.len())
        {
            self.options.swap(index, other);
            self.error = None;
            cx.notify();
        }
    }
}

impl Render for DefinitionForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let count = self.options.len();
        div()
            .v_flex()
            .gap_3()
            .when(!self.dual, |body| {
                body.child(
                    Field::new().label("Type").child(
                        RadioGroup::horizontal("lab-definition-mode")
                            .child("Number")
                            .child("Selection")
                            .selected_index(Some(usize::from(self.selection_mode)))
                            .on_change(cx.listener(|view, index, _, cx| {
                                view.selection_mode = *index == 1;
                                view.error = None;
                                cx.notify();
                            })),
                    ),
                )
            })
            .when(!self.selection_mode, |body| {
                body.child(
                    Field::new().label("Accuracy").child(
                        RadioGroup::horizontal("lab-range-accuracy")
                            .child("Whole numbers")
                            .child("One decimal place")
                            .selected_index(Some(usize::from(!self.whole_numbers)))
                            .on_change(cx.listener(|view, index, _, cx| {
                                view.whole_numbers = *index == 0;
                                view.error = None;
                                cx.notify();
                            })),
                    ),
                )
                .child(
                    Form::new()
                        .columns(2)
                        .child(number_field(
                            &self.bounds[0],
                            "lab-range-minimum",
                            "Minimum",
                        ))
                        .child(number_field(
                            &self.bounds[1],
                            "lab-range-maximum",
                            "Maximum",
                        )),
                )
            })
            .when(self.selection_mode, |body| {
                body.child(
                    div()
                        .id("lab-definition-options")
                        .v_flex()
                        .gap_2()
                        .max_h(rems(18.))
                        .overflow_y_scrollbar()
                        .children(self.options.iter().enumerate().map(|(index, option)| {
                            let id = option.id;
                            div()
                                .v_flex()
                                .gap_1()
                                .p_2()
                                .border_1()
                                .border_color(theme.border)
                                .rounded(theme.radius)
                                .child(
                                    Form::new()
                                        .columns(2)
                                        .child(
                                            Field::new().label("Name").child(
                                                Input::new(&option.name)
                                                    .id(format!("lab-option-name-{id}"))
                                                    .aria_label(format!(
                                                        "Option {} name",
                                                        index + 1
                                                    )),
                                            ),
                                        )
                                        .child(Field::new().label("Value").child(
                                            NumericField::new(
                                                &option.value,
                                                format!("lab-option-value-{id}"),
                                                format!("Option {} value", index + 1),
                                                option.slider_range,
                                            ),
                                        )),
                                )
                                .child(
                                    div()
                                        .h_flex()
                                        .flex_wrap()
                                        .gap_1()
                                        .child(
                                            Radio::new(format!("lab-option-active-{id}"))
                                                .label("Active option")
                                                .accessibility_label(format!(
                                                    "Make option {} active",
                                                    index + 1
                                                ))
                                                .checked(self.selected == Some(id))
                                                .on_change(cx.listener(move |view, _, _, cx| {
                                                    view.selected = Some(id);
                                                    view.error = None;
                                                    cx.notify();
                                                })),
                                        )
                                        .child(
                                            Button::new(format!("lab-option-up-{id}"))
                                                .small()
                                                .ghost()
                                                .label("Move up")
                                                .disabled(index == 0)
                                                .on_click(cx.listener(move |view, _, _, cx| {
                                                    view.move_option(id, -1, cx)
                                                })),
                                        )
                                        .child(
                                            Button::new(format!("lab-option-down-{id}"))
                                                .small()
                                                .ghost()
                                                .label("Move down")
                                                .disabled(index + 1 == count)
                                                .on_click(cx.listener(move |view, _, _, cx| {
                                                    view.move_option(id, 1, cx)
                                                })),
                                        )
                                        .child(
                                            Button::new(format!("lab-option-remove-{id}"))
                                                .small()
                                                .ghost()
                                                .label("Remove")
                                                .disabled(count <= 2)
                                                .on_click(cx.listener(move |view, _, _, cx| {
                                                    if view.options.len() > 2 {
                                                        view.options
                                                            .retain(|option| option.id != id);
                                                        if view.selected == Some(id) {
                                                            view.selected = None;
                                                        }
                                                        view.error = None;
                                                        cx.notify();
                                                    }
                                                })),
                                        ),
                                )
                        })),
                )
                .child(
                    Button::new("lab-option-add")
                        .small()
                        .label("Add option")
                        .on_click(cx.listener(|view, _, window, cx| view.add_option(window, cx))),
                )
            })
            .children(self.patch_inputs.status(cx))
            .when_some(self.error.as_ref(), |body, error| {
                body.child(
                    div()
                        .id("lab-definition-error")
                        .role(gpui_kit::Role::Alert)
                        .aria_label(error.clone())
                        .test_support()
                        .text_sm()
                        .text_color(theme.danger)
                        .child(error.clone()),
                )
            })
    }
}

impl GraphDocumentView {
    pub(super) fn open_adjuster_definition(
        &mut self,
        node: Option<Node>,
        control: LabNumericAdjuster,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.closed || self.is_busy() {
            return;
        }
        if self.inspector.is_changed(cx) {
            self.error =
                Some("Correct or reset field text before configuring a Lab adjuster.".into());
            cx.notify();
            return;
        }
        let requests = match self.numeric_patch_inputs(node.as_ref(), &control) {
            Ok(requests) => requests,
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        let mut draft = DefinitionForm::new(&control, window, cx);
        let form = cx.new(|cx| {
            draft.patch_inputs.prepare(
                requests,
                self.environment_library.clone(),
                self.window,
                |form: &mut DefinitionForm| &mut form.patch_inputs,
                cx,
            );
            draft
        });
        self.pause_preview();
        self.modal = true;
        let owner = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _, cx| {
            let submit = form.clone();
            let cancel = form.clone();
            let edit = owner.clone();
            let close = owner.clone();
            let control = control.clone();
            let node = node.clone();
            dialog
                .title(format!("Configure {}", control.name()))
                .child(form.clone())
                .footer(
                    DialogFooter::new()
                        .child(DialogClose::new().child(Button::new("cancel").label("Cancel")))
                        .child(form.read(cx).patch_inputs.apply_button()),
                )
                .on_ok(move |_, window, cx| {
                    let Some(sizes) = submit.read(cx).patch_inputs.sizes() else {
                        return false;
                    };
                    let result =
                        submit.read(cx).definition(cx).and_then(|definition| {
                            edit.update(cx, |view, cx| {
                                view.check_lab_selection(node.as_ref())?;
                                let current = view.numeric_patch_inputs(node.as_ref(), &control)?;
                                if !current.matches(&sizes) {
                                    let library = view.environment_library.clone();
                                    let handle = view.window;
                                    submit.update(cx, |form, cx| {
                                        form.error = None;
                                        form.patch_inputs.prepare(
                                            current,
                                            library,
                                            handle,
                                            |form: &mut DefinitionForm| &mut form.patch_inputs,
                                            cx,
                                        );
                                    });
                                    return Ok(false);
                                }
                                let sizes = sizes.dimensions();
                                view.commit_lab_adjuster_change(window, cx, |document| {
                                    match definition {
                                        DefinitionChange::Range(range) => document
                                            .edit_lab_numeric_range_with_patch_input_sizes(
                                                node.as_ref(),
                                                &control,
                                                range,
                                                &sizes,
                                            ),
                                        DefinitionChange::Scalar(definition) => document
                                            .edit_lab_scalar_definition_with_patch_input_sizes(
                                                node.as_ref(),
                                                &control,
                                                definition,
                                                &sizes,
                                            ),
                                    }
                                })?;
                                Ok(true)
                            })
                            .unwrap_or_else(|_| Err("The graph window closed.".into()))
                        });
                    match result {
                        Ok(close) => close,
                        Err(error) => {
                            submit.update(cx, |form, cx| {
                                form.error = Some(error);
                                cx.notify();
                            });
                            false
                        }
                    }
                })
                .on_close(move |_, _, cx| {
                    cancel.update(cx, |form, _| form.patch_inputs.cancel());
                    let _ = close.update(cx, |view, cx| {
                        view.modal = false;
                        cx.notify();
                    });
                })
        });
        cx.notify();
    }
}
