use super::*;

#[derive(Clone, Copy)]
pub(super) enum AdjusterNameKind {
    Curve,
    Environment,
    Color,
    Gradient,
    Number,
    Dual,
    Selection,
    Synthesis,
}

struct AdjusterName {
    input: Entity<InputState>,
    error: Option<String>,
    input_id: &'static str,
    error_id: &'static str,
    input_label: &'static str,
}

impl Render for AdjusterName {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .v_flex()
            .gap_2()
            .child(
                Field::new().label("Name").child(
                    Input::new(&self.input)
                        .id(self.input_id)
                        .aria_label(self.input_label),
                ),
            )
            .when_some(self.error.as_ref(), |form, error| {
                form.child(
                    div()
                        .id(self.error_id)
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
    pub(super) fn open_adjuster_name(
        &mut self,
        kind: AdjusterNameKind,
        apply: impl Fn(&mut Self, String, &mut Window, &mut Context<Self>) -> Result<(), String>
        + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (title, input_id, error_id, input_label) = match kind {
            AdjusterNameKind::Curve => (
                "New Curve adjuster",
                "graph-curve-adjuster-name",
                "graph-curve-adjuster-error",
                "Curve adjuster name",
            ),
            AdjusterNameKind::Environment => (
                "New Environment adjuster",
                "graph-environment-adjuster-name",
                "graph-environment-adjuster-error",
                "Environment adjuster name",
            ),
            AdjusterNameKind::Color => (
                "New Color adjuster",
                "graph-environment-field-adjuster-name",
                "graph-environment-field-adjuster-error",
                "Color adjuster name",
            ),
            AdjusterNameKind::Gradient => (
                "New Gradient adjuster",
                "graph-environment-field-adjuster-name",
                "graph-environment-field-adjuster-error",
                "Gradient adjuster name",
            ),
            AdjusterNameKind::Number => (
                "New Number adjuster",
                "graph-environment-field-adjuster-name",
                "graph-environment-field-adjuster-error",
                "Number adjuster name",
            ),
            AdjusterNameKind::Dual => (
                "New Dual Numeric adjuster",
                "graph-environment-field-adjuster-name",
                "graph-environment-field-adjuster-error",
                "Dual Numeric adjuster name",
            ),
            AdjusterNameKind::Selection => (
                "New Selection adjuster",
                "graph-environment-field-adjuster-name",
                "graph-environment-field-adjuster-error",
                "Selection adjuster name",
            ),
            AdjusterNameKind::Synthesis => (
                "New Synthesis adjuster",
                "graph-synthesis-adjuster-name",
                "graph-synthesis-adjuster-error",
                "Synthesis adjuster name",
            ),
        };
        self.modal = true;
        let input = cx.new(|cx| InputState::new(window, cx));
        let name = cx.new(|_| AdjusterName {
            input,
            error: None,
            input_id,
            error_id,
            input_label,
        });
        let owner = cx.weak_entity();
        let apply = Arc::new(apply);
        window.open_dialog(cx, move |dialog, _, _| {
            let submit = name.clone();
            let owner = owner.clone();
            let close = owner.clone();
            let apply = apply.clone();
            dialog
                .title(title)
                .child(name.clone())
                .footer(
                    DialogFooter::new()
                        .child(DialogClose::new().child(Button::new("cancel").label("Cancel")))
                        .child(
                            DialogAction::new().child(Button::new("ok").primary().label("Create")),
                        ),
                )
                .on_ok(move |_, window, cx| {
                    let text = submit.read(cx).input.read(cx).value().to_string();
                    let result = owner
                        .update(cx, |view, cx| apply(view, text, window, cx))
                        .unwrap_or_else(|_| Err("The graph window closed.".into()));
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
