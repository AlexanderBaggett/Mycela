use super::*;
use mycela_engine::LabAdjusterMetadata;

struct MetadataForm {
    name: Entity<InputState>,
    order: Entity<InputState>,
    error: Option<String>,
}

impl MetadataForm {
    fn values(&self, cx: &App) -> Result<(String, i32), String> {
        let order = self
            .order
            .read(cx)
            .value()
            .trim()
            .parse::<i32>()
            .map_err(|_| "Enter a whole number for the order.".to_owned())?;
        Ok((self.name.read(cx).value().to_string(), order))
    }
}

impl Render for MetadataForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .v_flex()
            .gap_2()
            .child(
                Form::new()
                    .child(
                        Field::new().label("Name").child(
                            Input::new(&self.name)
                                .id("lab-metadata-name")
                                .aria_label("Name"),
                        ),
                    )
                    .child(super::numeric_adjusters::number_field(
                        &self.order,
                        "lab-metadata-order",
                        "Order",
                    )),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child("Lower order numbers appear first. Using an existing compatible name moves this adjuster's links to that control."),
            )
            .when_some(self.error.as_ref(), |form, error| {
                form.child(
                    div()
                        .id("lab-metadata-error")
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
    pub(super) fn open_adjuster_metadata(
        &mut self,
        node: Option<Node>,
        entry: LabAdjusterMetadata,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.closed || self.is_busy() {
            return;
        }
        if self.has_field_drafts(cx) {
            self.error = Some("Correct or reset field text before editing the adjuster.".into());
            cx.notify();
            return;
        }
        self.pause_preview();
        self.modal = true;
        let form = cx.new(|cx| MetadataForm {
            name: cx.new(|cx| InputState::new(window, cx).default_value(entry.name().to_owned())),
            order: cx
                .new(|cx| InputState::new(window, cx).default_value(entry.order().to_string())),
            error: None,
        });
        let owner = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let submit = form.clone();
            let edit = owner.clone();
            let close = owner.clone();
            let entry = entry.clone();
            let node = node.clone();
            dialog
                .title(format!("Name and order for {}", entry.name()))
                .child(form.clone())
                .footer(
                    DialogFooter::new()
                        .child(DialogClose::new().child(Button::new("cancel").label("Cancel")))
                        .child(
                            DialogAction::new().child(Button::new("ok").primary().label("Apply")),
                        ),
                )
                .on_ok(move |_, window, cx| {
                    let result = submit.read(cx).values(cx).and_then(|(name, order)| {
                        edit.update(cx, |view, cx| {
                            view.check_lab_selection(node.as_ref())?;
                            view.commit_lab_adjuster_change(window, cx, |document| {
                                document.edit_lab_adjuster_metadata(
                                    node.as_ref(),
                                    &entry,
                                    &name,
                                    order,
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
                    true
                })
                .on_close(move |_, _, cx| {
                    let _ = close.update(cx, |view, cx| {
                        view.modal = false;
                        cx.notify();
                    });
                })
        });
        cx.notify();
    }
}
