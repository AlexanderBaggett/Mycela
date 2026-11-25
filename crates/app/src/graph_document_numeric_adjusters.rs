use super::numeric_field::{NumericField, NumericRange};
use super::synthesis::{PatchInputRequests, PatchInputSizes};
use super::*;
use gpui_kit::SharedString;
use gpui_kit::component::{
    IndexPath,
    select::{Select, SelectItem, SelectState},
};
use mycela_engine::{
    EnvironmentResourceLibrary, GtxGraphError, ImageGeometry, LabNumericAdjuster, LabNumericValue,
};

enum AdjusterFields {
    Number(Entity<InputState>),
    Dual([Entity<InputState>; 2]),
    Selection(Entity<SelectState<Vec<AdjusterOption>>>),
}

#[derive(Clone)]
struct AdjusterOption {
    title: SharedString,
    index: i32,
}

impl SelectItem for AdjusterOption {
    type Value = i32;

    fn title(&self) -> SharedString {
        self.title.clone()
    }

    fn value(&self) -> &i32 {
        &self.index
    }
}

struct AdjusterForm {
    fields: AdjusterFields,
    range: Option<[f64; 2]>,
    slider_range: Option<NumericRange>,
    error: Option<String>,
    patch_inputs: PatchInputDraft,
}

#[derive(Default)]
pub(super) struct PatchInputDraft {
    sizes: Option<PatchInputSizes>,
    ticket: Option<Ticket>,
}

impl Drop for PatchInputDraft {
    fn drop(&mut self) {
        self.cancel();
    }
}

impl PatchInputDraft {
    pub(super) fn prepare<T: 'static>(
        &mut self,
        requests: PatchInputRequests,
        library: EnvironmentResourceLibrary,
        window: AnyWindowHandle,
        draft: fn(&mut T) -> &mut Self,
        cx: &mut Context<T>,
    ) {
        self.cancel();
        let ticket = Ticket::new();
        if requests.is_ready() {
            self.sizes = Some(requests.render(&library, &ticket));
        } else {
            self.sizes = None;
            self.ticket = Some(ticket.clone());
            let worker = ticket.clone();
            cx.spawn(async move |owner, cx| {
                let sizes = cx
                    .background_spawn(async move { requests.render(&library, &worker) })
                    .await;
                if ticket.is_current() {
                    let _ = window.update(cx, |_, window, cx| {
                        let _ = owner.update(cx, |form, cx| {
                            let draft = draft(form);
                            draft.ticket = None;
                            draft.sizes = Some(sizes);
                            cx.notify();
                        });
                        window.refresh();
                    });
                }
            })
            .detach();
        }
        cx.notify();
    }

    pub(super) fn cancel(&mut self) {
        if let Some(ticket) = self.ticket.take() {
            ticket.retire();
        }
    }

    pub(super) fn sizes(&self) -> Option<PatchInputSizes> {
        self.sizes.clone()
    }

    pub(super) fn status(&self, cx: &App) -> Option<gpui_kit::AnyElement> {
        self.sizes.is_none().then(|| {
            div()
                .id("lab-adjuster-patch-input-status")
                .test_support()
                .role(gpui_kit::Role::Status)
                .aria_label("Preparing patch source previews…")
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("Preparing patch source previews…")
                .into_any_element()
        })
    }

    pub(super) fn apply_button(&self) -> gpui_kit::AnyElement {
        let ready = self.sizes.is_some();
        let apply = Button::new("ok").primary().label("Apply").disabled(!ready);
        if ready {
            DialogAction::new().child(apply).into_any_element()
        } else {
            apply.into_any_element()
        }
    }
}

impl AdjusterFields {
    fn new(control: &LabNumericAdjuster, window: &mut Window, cx: &mut App) -> Self {
        let mut number =
            |value: f64| cx.new(|cx| InputState::new(window, cx).default_value(value.to_string()));
        match control.value() {
            LabNumericValue::Number(value) => Self::Number(number(value)),
            LabNumericValue::Dual(values) => Self::Dual(values.map(number)),
            LabNumericValue::Selection(index) => {
                let options = control
                    .options()
                    .iter()
                    .cloned()
                    .enumerate()
                    .map(|(index, title)| AdjusterOption {
                        title: title.into(),
                        index: index as i32,
                    })
                    .collect::<Vec<_>>();
                let selected = usize::try_from(index)
                    .ok()
                    .filter(|index| *index < options.len());
                Self::Selection(
                    cx.new(|cx| {
                        SelectState::new(options, selected.map(IndexPath::new), window, cx)
                    }),
                )
            }
        }
    }

    fn value(&self, cx: &App) -> Result<LabNumericValue, String> {
        let number = |input: &Entity<InputState>, error: &str| {
            input
                .read(cx)
                .value()
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|value| value.is_finite())
                .ok_or_else(|| error.to_owned())
        };
        Ok(match self {
            Self::Number(input) => LabNumericValue::Number(number(input, "Enter a number.")?),
            Self::Dual([x, y]) => LabNumericValue::Dual([
                number(x, "Enter a number for X.")?,
                number(y, "Enter a number for Y.")?,
            ]),
            Self::Selection(input) => {
                let index = input
                    .read(cx)
                    .selected_value()
                    .copied()
                    .ok_or_else(|| "Choose an available option.".to_owned())?;
                LabNumericValue::Selection(index)
            }
        })
    }
}

pub(super) fn number_field(
    input: &Entity<InputState>,
    id: &'static str,
    label: &'static str,
) -> Field {
    Field::new()
        .label(label)
        .child(Input::new(input).id(id).aria_label(label))
}

impl Render for AdjusterForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let numeric = |input, id, label| {
            Field::new()
                .label(label)
                .child(NumericField::new(input, id, label, self.slider_range))
        };
        let fields = match &self.fields {
            AdjusterFields::Number(input) => {
                Form::new().child(numeric(input, "lab-adjuster-number", "Value"))
            }
            AdjusterFields::Dual([x, y]) => Form::new()
                .child(numeric(x, "lab-adjuster-x", "X"))
                .child(numeric(y, "lab-adjuster-y", "Y")),
            AdjusterFields::Selection(input) => Form::new().child(
                Field::new().label("Option").child(
                    Select::new(input)
                        .id("lab-adjuster-option")
                        .accessibility_label("Adjuster option")
                        .placeholder("Choose option"),
                ),
            ),
        };
        div()
            .v_flex()
            .gap_2()
            .child(fields)
            .children(self.patch_inputs.status(cx))
            .when_some(self.range, |view, [min, max]| {
                view.child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(format!("Range {min} to {max}")),
                )
            })
            .when_some(self.error.as_ref(), |view, error| {
                view.child(
                    div()
                        .id("lab-adjuster-error")
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
    pub(super) fn lab_adjuster_menu(
        &self,
        busy: bool,
        cx: &Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let node = self.selected_node();
        let document = guard(&self.document);
        let (choices, metadata) = document.lab_menu_adjusters(node.as_ref());
        let colors = document.color_lab_adjusters(node.as_ref());
        if choices.is_empty() && colors.is_empty() && metadata.is_empty() {
            return None;
        }
        let materials: Vec<_> = metadata
            .iter()
            .filter(|entry| entry.is_material())
            .map(|entry| {
                let label = format!("Edit {}…", entry.name());
                let selected = node.clone();
                let entry = entry.clone();
                Button::new(("graph-lab-edit-material", entry.identity()))
                    .small()
                    .min_w_0()
                    .max_w_full()
                    .label(label.clone())
                    .tooltip(label)
                    .disabled(busy)
                    .on_click(cx.listener(move |view, _, window, cx| {
                        view.open_lab_material(selected.as_ref(), &entry, window, cx);
                    }))
            })
            .collect();
        let owner = cx.weak_entity();
        Some(
            div()
                .h_flex()
                .flex_wrap()
                .max_w_full()
                .gap_2()
                .children(materials)
                .child(
                    Button::new("graph-lab-adjusters")
                        .small()
                        .ghost()
                        .label("Lab adjusters")
                        .disabled(busy)
                        .dropdown_menu(move |mut menu, _, _| {
                            for control in &choices {
                                let suffix = if choices
                                    .iter()
                                    .filter(|other| other.name() == control.name())
                                    .count()
                                    > 1
                                    || colors.iter().any(|other| other.name() == control.name())
                                {
                                    match control.value() {
                                        LabNumericValue::Number(_) => " (Number)",
                                        LabNumericValue::Dual(_) => " (Dual)",
                                        LabNumericValue::Selection(_) => " (Selection)",
                                    }
                                } else {
                                    ""
                                };
                                let label = format!("{}{suffix}…", control.name());
                                let can_edit =
                                    !matches!(control.value(), LabNumericValue::Selection(_))
                                        || !control.options().is_empty();
                                let control = control.clone();
                                let owner = owner.clone();
                                let node = node.clone();
                                menu = menu.item(
                                    PopupMenuItem::new(label).disabled(!can_edit).on_click(
                                        move |_, window, cx| {
                                            let _ = owner.update(cx, |view, cx| {
                                                view.open_numeric_adjuster(
                                                    node.clone(),
                                                    control.clone(),
                                                    window,
                                                    cx,
                                                );
                                            });
                                        },
                                    ),
                                );
                            }
                            for control in &colors {
                                let repeated = colors
                                    .iter()
                                    .filter(|other| other.name() == control.name())
                                    .count()
                                    > 1
                                    || choices.iter().any(|other| other.name() == control.name());
                                let label = if repeated {
                                    format!("{} ({})…", control.name(), control.type_label())
                                } else {
                                    format!("{}…", control.name())
                                };
                                let owner = owner.clone();
                                let node = node.clone();
                                let control = control.clone();
                                menu = menu.item(PopupMenuItem::new(label).on_click(
                                    move |_, window, cx| {
                                        let _ = owner.update(cx, |view, cx| {
                                            view.open_color_lab_adjuster(
                                                node.clone(),
                                                control.clone(),
                                                window,
                                                cx,
                                            );
                                        });
                                    },
                                ));
                            }
                            if !choices.is_empty() {
                                menu = menu.separator();
                            }
                            for control in &choices {
                                let label = format!("Configure {}…", control.name());
                                let owner = owner.clone();
                                let node = node.clone();
                                let control = control.clone();
                                menu = menu.item(PopupMenuItem::new(label).on_click(
                                    move |_, window, cx| {
                                        let _ = owner.update(cx, |view, cx| {
                                            view.open_adjuster_definition(
                                                node.clone(),
                                                control.clone(),
                                                window,
                                                cx,
                                            );
                                        });
                                    },
                                ));
                            }
                            if !metadata.is_empty() && (!choices.is_empty() || !colors.is_empty()) {
                                menu = menu.separator();
                            }
                            for entry in &metadata {
                                let label = if metadata
                                    .iter()
                                    .filter(|other| other.name() == entry.name())
                                    .count()
                                    > 1
                                {
                                    format!(
                                        "Name and order for {} ({})…",
                                        entry.name(),
                                        entry.type_label()
                                    )
                                } else {
                                    format!("Name and order for {}…", entry.name())
                                };
                                let owner = owner.clone();
                                let node = node.clone();
                                let entry = entry.clone();
                                menu = menu.item(PopupMenuItem::new(label).on_click(
                                    move |_, window, cx| {
                                        let _ = owner.update(cx, |view, cx| {
                                            view.open_adjuster_metadata(
                                                node.clone(),
                                                entry.clone(),
                                                window,
                                                cx,
                                            )
                                        });
                                    },
                                ));
                            }
                            menu.scrollable(true)
                        }),
                ),
        )
    }

    fn open_numeric_adjuster(
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
            self.error = Some("Correct or reset field text before editing a Lab adjuster.".into());
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
        let fields = AdjusterFields::new(&control, window, cx);
        let form = cx.new(|cx| {
            let mut form = AdjusterForm {
                fields,
                range: control.range(),
                slider_range: control.range_settings().map(|settings| {
                    NumericRange::bounds(
                        settings.bounds(),
                        if settings.whole_numbers() { 1.0 } else { 0.01 },
                    )
                }),
                error: None,
                patch_inputs: PatchInputDraft::default(),
            };
            form.patch_inputs.prepare(
                requests,
                self.environment_library.clone(),
                self.window,
                |form: &mut AdjusterForm| &mut form.patch_inputs,
                cx,
            );
            form
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
            let apply = form.read(cx).patch_inputs.apply_button();
            dialog
                .title(format!("Edit {}", control.name()))
                .child(form.clone())
                .footer(
                    DialogFooter::new()
                        .child(DialogClose::new().child(Button::new("cancel").label("Cancel")))
                        .child(apply),
                )
                .on_ok(move |_, window, cx| {
                    let Some(sizes) = submit.read(cx).patch_inputs.sizes() else {
                        return false;
                    };
                    let value = submit.read(cx).fields.value(cx);
                    let result = value.and_then(|value| {
                        edit.update(cx, |view, cx| {
                            if value != control.value() {
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
                                            |form: &mut AdjusterForm| &mut form.patch_inputs,
                                            cx,
                                        );
                                    });
                                    return Ok(false);
                                }
                            }
                            view.apply_numeric_adjuster(
                                node.as_ref(),
                                &control,
                                value,
                                &sizes.dimensions(),
                                window,
                                cx,
                            )?;
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

    pub(super) fn numeric_patch_inputs(
        &self,
        node: Option<&Node>,
        control: &LabNumericAdjuster,
    ) -> Result<PatchInputRequests, String> {
        let inputs = guard(&self.document)
            .connected_patch_inputs_for_numeric_adjuster(node, control)
            .map_err(model::failure)?;
        Ok(self.capture_synthesis_patch_inputs(inputs))
    }

    fn apply_numeric_adjuster(
        &mut self,
        node: Option<&Node>,
        control: &LabNumericAdjuster,
        input: LabNumericValue,
        input_sizes: &[(u64, ImageGeometry)],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        self.check_lab_selection(node)?;
        self.commit_lab_adjuster_change(window, cx, |document| {
            document.edit_numeric_lab_adjuster_with_patch_input_sizes(
                node,
                control,
                input,
                input_sizes,
            )
        })
    }

    pub(super) fn commit_lab_adjuster_change(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut model::Document) -> Result<bool, GtxGraphError<ForegroundError>>,
    ) -> Result<(), String> {
        self.ensure_history_binding();
        if let Some(history) = self.history() {
            history.separate_next_edit();
        }
        let dirty = self.dirty;
        let stamp = self.begin_history_edit();
        let result = change(&mut guard(&self.document));
        match result {
            Ok(false) => self.dirty = dirty,
            Ok(true) => {
                self.finish_history_edit(stamp, true);
                self.revision += 1;
                self.error = None;
                self.status = None;
                self.rebuild(window, cx);
                self.start_preview(cx);
            }
            Err(error) => {
                if matches!(
                    error,
                    GtxGraphError::NumericAdjuster(_)
                        | GtxGraphError::EnvironmentAdjuster(_)
                        | GtxGraphError::LabColorAdjuster(_)
                        | GtxGraphError::SynthesisAdjuster(_)
                        | GtxGraphError::Node(_)
                ) {
                    self.dirty = dirty;
                }
                return Err(model::failure(error));
            }
        }
        cx.notify();
        Ok(())
    }
}
