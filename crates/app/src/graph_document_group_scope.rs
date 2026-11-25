use super::*;
use mycela_engine::PointwiseFilter;

impl GraphDocumentView {
    pub(super) fn choose_group_scope(
        &mut self,
        group: &PointwiseFilter<ForegroundError>,
        catch: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !catch
            || group
                .parameter(2)
                .and_then(|name| name.text())
                .is_ok_and(|name| name.is_some_and(|name| !name.is_empty()))
        {
            if let Err(error) = self.commit_group_scope(group, catch, None, window, cx) {
                self.error = Some(error);
                cx.notify();
            }
            return;
        }
        self.pause_preview();
        self.modal = true;
        let owner = cx.weak_entity();
        let group = group.clone();
        let name = cx.new(|cx| InputState::new(window, cx));
        let error = cx.new(|_| GroupNameError(None));
        window.open_dialog(cx, move |dialog, _, _| {
            let edit = owner.clone();
            let close = owner.clone();
            let group = group.clone();
            let field = name.clone();
            let feedback = error.clone();
            dialog
                .title("Name the Lab section")
                .child(
                    Form::new().child(
                        Field::new().label("Group name").child(
                            Input::new(&name)
                                .id("group-scope-name")
                                .aria_label("Group name"),
                        ),
                    ),
                )
                .child(error.clone())
                .footer(
                    DialogFooter::new()
                        .child(DialogClose::new().child(Button::new("cancel").label("Cancel")))
                        .child(
                            DialogAction::new()
                                .child(Button::new("ok").primary().label("Catch adjusters")),
                        ),
                )
                .on_ok(move |_, window, cx| {
                    let name = field.read(cx).value().to_string();
                    let result = edit
                        .update(cx, |view, cx| {
                            view.commit_group_scope(&group, true, Some(&name), window, cx)
                        })
                        .unwrap_or_else(|_| Err("The graph window closed.".into()));
                    if let Err(error) = result {
                        feedback.update(cx, |feedback, cx| {
                            feedback.0 = Some(error);
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

    fn commit_group_scope(
        &mut self,
        group: &PointwiseFilter<ForegroundError>,
        catch: bool,
        name: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        self.check_lab_selection(Some(&group.node()))?;
        self.commit_lab_adjuster_change(window, cx, |document| {
            document.set_group_catch_adjusters(group, catch, name)
        })
    }
}

struct GroupNameError(Option<String>);

impl Render for GroupNameError {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().when_some(self.0.as_ref(), |element, error| {
            element.child(
                div()
                    .id("group-scope-error")
                    .role(gpui_kit::Role::Alert)
                    .aria_label(error.clone())
                    .text_sm()
                    .text_color(cx.theme().danger)
                    .child(error.clone()),
            )
        })
    }
}
