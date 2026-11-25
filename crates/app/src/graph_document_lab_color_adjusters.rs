//! Dialog drafts for Color and Gradient Lab controls.
use super::style::paint_gradient::{Action, Draft};
use super::*;
use mycela_engine::{LabColorAdjuster, LabColorValue, SharedGradient};

const CHANNELS: [(&str, &str); 3] = [
    ("lab-color-red", "Red"),
    ("lab-color-green", "Green"),
    ("lab-color-blue", "Blue"),
];

enum Fields {
    Color([Entity<InputState>; 3]),
    Gradient {
        draft: Box<Draft>,
        working: SharedGradient,
    },
}

struct ColorAdjusterForm {
    fields: Fields,
    error: Option<String>,
    observers: Vec<Subscription>,
}

impl ColorAdjusterForm {
    fn observe_inputs(&mut self, cx: &mut Context<Self>) {
        self.observers = match &self.fields {
            Fields::Color(inputs) => inputs
                .iter()
                .map(|input| cx.observe(input, |_, _, cx| cx.notify()))
                .collect(),
            Fields::Gradient { draft, .. } => draft
                .inputs()
                .map(|input| cx.observe(&input, |_, _, cx| cx.notify()))
                .collect(),
        };
    }

    fn value(&self, cx: &App) -> Result<LabColorValue, String> {
        match &self.fields {
            Fields::Color(fields) => {
                let mut rgb = [0.; 3];
                for ((channel, field), (_, label)) in rgb.iter_mut().zip(fields).zip(CHANNELS) {
                    *channel = field
                        .read(cx)
                        .value()
                        .trim()
                        .parse::<f64>()
                        .ok()
                        .filter(|value| value.is_finite())
                        .ok_or_else(|| format!("Enter a number for {label}."))?;
                }
                Ok(LabColorValue::Color(rgb))
            }
            Fields::Gradient { draft, working } => Ok(LabColorValue::Gradient(
                draft.staged(cx)?.unwrap_or_else(|| working.clone()),
            )),
        }
    }

    fn edit_gradient(
        &mut self,
        action: Option<Action>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Fields::Gradient { draft, working } = &mut self.fields else {
            return;
        };
        let result = (|| {
            let (next, selected) = match action {
                Some(action) if !draft.can_run(action, cx) => return Ok(()),
                Some(Action::Select(index)) => return draft.select(Some(index), window, cx),
                Some(action) => draft.change(action, cx)?,
                None => {
                    let Some(next) = draft.staged(cx)? else {
                        return Ok(());
                    };
                    (next, draft.selected())
                }
            };
            let mut next_draft = Draft::from_gradient(next.clone(), window, cx)?;
            next_draft.select(selected, window, cx)?;
            *draft.as_mut() = next_draft;
            *working = next;
            Ok(())
        })();
        self.error = result.err();
        self.observe_inputs(cx);
        cx.notify();
    }
}

impl Render for ColorAdjusterForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let fields = match &self.fields {
            Fields::Color(fields) => Form::new()
                .columns(3)
                .children(fields.iter().zip(CHANNELS).map(|(input, (id, label))| {
                    Field::new()
                        .label(label)
                        .child(super::numeric_field::NumericField::new(
                            input,
                            id,
                            label,
                            super::numeric_field::NumericRange::new(0.0, 1.0, 0.001),
                        ))
                }))
                .into_any_element(),
            Fields::Gradient { draft, .. } => {
                let owner = cx.weak_entity();
                div()
                    .v_flex()
                    .gap_2()
                    .child(draft.render_controls(
                        "lab-gradient",
                        false,
                        cx,
                        move |action, window, cx| {
                            let _ = owner.update(cx, |form, cx| {
                                form.edit_gradient(Some(action), window, cx)
                            });
                        },
                    ))
                    .child(
                        Button::new("lab-gradient-update-stop")
                            .small()
                            .label("Update stop")
                            .disabled(!draft.stop_pending(cx))
                            .on_click(cx.listener(|form, _, window, cx| {
                                form.edit_gradient(None, window, cx)
                            })),
                    )
                    .into_any_element()
            }
        };
        div()
            .v_flex()
            .gap_2()
            .child(fields)
            .when_some(self.error.as_ref(), |view, error| {
                view.child(
                    div()
                        .id("lab-color-error")
                        .role(gpui_kit::Role::Alert)
                        .aria_label(error.clone())
                        .test_support()
                        .text_sm()
                        .text_color(cx.theme().danger)
                        .child(error.clone()),
                )
            })
    }
}

impl GraphDocumentView {
    fn freeze_color_draft(
        &self,
        node: Option<&Node>,
        control: &LabColorAdjuster,
        before: &LabColorValue,
        rgb: [f64; 3],
    ) -> Result<Option<draft_preview::DraftPreviewJob>, String> {
        self.check_lab_selection(node)?;
        let session = guard(&self.document)
            .preview_color_lab_adjuster(node, control, before, rgb)
            .map_err(model::failure)?;
        session
            .map(|session| self.freeze_draft_preview(session, None, false))
            .transpose()
    }

    pub(super) fn open_color_lab_adjuster(
        &mut self,
        node: Option<Node>,
        control: LabColorAdjuster,
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
        let result = (|| {
            let before = control.draft_value().map_err(|error| error.to_string())?;
            let fields = match &before {
                LabColorValue::Color(rgb) => Fields::Color(rgb.map(|value| {
                    cx.new(|cx| InputState::new(window, cx).default_value(value.to_string()))
                })),
                LabColorValue::Gradient(gradient) => Fields::Gradient {
                    draft: Box::new(Draft::from_gradient(gradient.clone(), window, cx)?),
                    working: gradient.clone(),
                },
            };
            Ok::<_, String>((before, fields))
        })();
        let (before, fields) = match result {
            Ok(draft) => draft,
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        let form = cx.new(|cx| {
            let mut form = ColorAdjusterForm {
                fields,
                error: None,
                observers: Vec::new(),
            };
            form.observe_inputs(cx);
            form
        });
        self.pause_preview();
        self.modal = true;
        let editor = form.entity_id();
        let color_preview = matches!(&before, LabColorValue::Color(_));
        if let LabColorValue::Color(original) = &before {
            let draft_node = node.clone();
            let draft_control = control.clone();
            let draft_before = before.clone();
            let original = original.map(f64::to_bits);
            let mut seen = None;
            let subscription = cx.observe(&form, move |view, form, cx| {
                let rgb = match form.read(cx).value(cx) {
                    Ok(LabColorValue::Color(rgb)) => Some(rgb),
                    _ => None,
                };
                let fingerprint = rgb.map(|rgb| rgb.map(f64::to_bits));
                if fingerprint == seen {
                    return;
                }
                seen = fingerprint;
                let Some(rgb) = rgb else {
                    view.retire_draft_render(editor, false, cx);
                    return;
                };
                if fingerprint == Some(original) {
                    view.retire_draft_render(editor, true, cx);
                    return;
                }
                let node = draft_node.clone();
                let control = draft_control.clone();
                let before = draft_before.clone();
                view.queue_draft_preview(
                    editor,
                    move |view| view.freeze_color_draft(node.as_ref(), &control, &before, rgb),
                    cx,
                );
            });
            self.draft_preview = Some(draft_preview::DraftPreview::new(editor, subscription));
        }
        let owner = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let submit = form.clone();
            let edit = owner.clone();
            let close = owner.clone();
            let control = control.clone();
            let before = before.clone();
            let node = node.clone();
            dialog
                .title(format!("Edit {}", control.name()))
                .child(form.clone())
                .footer(
                    DialogFooter::new()
                        .child(DialogClose::new().child(Button::new("cancel").label("Cancel")))
                        .child(
                            DialogAction::new().child(Button::new("ok").primary().label("Apply")),
                        ),
                )
                .on_ok(move |_, window, cx| {
                    let result = submit.read(cx).value(cx).and_then(|value| {
                        edit.update(cx, |view, cx| {
                            view.check_lab_selection(node.as_ref())?;
                            view.commit_lab_adjuster_change(window, cx, |document| {
                                document.edit_color_lab_adjuster(
                                    node.as_ref(),
                                    &control,
                                    &before,
                                    value,
                                )
                            })
                        })
                        .unwrap_or_else(|_| Err("The graph window closed.".into()))
                    });
                    if let Err(error) = result {
                        submit.update(cx, |form, cx| {
                            form.error = Some(error);
                            cx.notify();
                        });
                        return false;
                    }
                    window.close_dialog(cx);
                    let _ = edit.update(cx, |view, cx| {
                        view.close_draft_preview(editor);
                        view.modal = false;
                        view.focus.focus(window, cx);
                        cx.notify();
                    });
                    false
                })
                .on_close(move |_, _, cx| {
                    let _ = close.update(cx, |view, cx| {
                        if !color_preview || view.close_draft_preview(editor) {
                            view.modal = false;
                            cx.notify();
                        }
                    });
                })
        });
        cx.notify();
    }
}
