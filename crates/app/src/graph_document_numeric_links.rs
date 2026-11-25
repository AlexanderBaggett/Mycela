use super::*;
use gpui_kit::AnyElement;
use mycela_engine::{LabNumericAxis, LabNumericLink, LabNumericProperty};

#[path = "graph_document_numeric_mapping.rs"]
mod mapping;

type Property = LabNumericProperty<ForegroundError>;

pub(super) struct LinkControls {
    pub(super) linked: bool,
    pub(super) buttons: AnyElement,
}

#[derive(Clone)]
struct LinkTarget {
    node: Option<Node>,
    property: Property,
    axis: LabNumericAxis,
}

struct AdjusterName {
    input: Entity<InputState>,
    error: Option<String>,
}

impl Render for AdjusterName {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .v_flex()
            .gap_2()
            .child(
                Field::new().label("Name").child(
                    Input::new(&self.input)
                        .id("numeric-link-name")
                        .aria_label("Adjuster name"),
                ),
            )
            .when_some(self.error.as_ref(), |form, error| {
                form.child(
                    div()
                        .id("numeric-link-error")
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
    pub(super) fn numeric_link_controls(
        &self,
        property: Property,
        axis: LabNumericAxis,
        id: &str,
        busy: bool,
        cx: &Context<Self>,
    ) -> LinkControls {
        let document = guard(&self.document);
        let scope = |axis| document.numeric_property_link_name(&property, axis);
        let paired = property.is_pair();
        let pair_name = paired.then(|| scope(LabNumericAxis::Whole)).flatten();
        let name = scope(axis);
        let linked = name.is_some() || pair_name.is_some();
        let mut buttons = div().h_flex().flex_wrap().gap_1();
        if axis == LabNumericAxis::X {
            buttons = buttons.child(self.numeric_link_menu(
                LinkTarget {
                    node: self.selected_node(),
                    property: property.clone(),
                    axis: LabNumericAxis::Whole,
                },
                pair_name,
                document.numeric_property_adjusters(&property, LabNumericAxis::Whole),
                format!("{id}:lab-pair"),
                busy,
                cx,
            ));
        }
        let choices = document.numeric_property_adjusters(&property, axis);
        buttons = buttons.child(self.numeric_link_menu(
            LinkTarget {
                node: self.selected_node(),
                property,
                axis,
            },
            name,
            choices,
            format!("{id}:lab-link"),
            busy,
            cx,
        ));
        LinkControls {
            linked,
            buttons: buttons.into_any_element(),
        }
    }

    fn numeric_link_menu(
        &self,
        target: LinkTarget,
        name: Option<String>,
        choices: Vec<mycela_engine::LabNumericAdjuster>,
        id: String,
        busy: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let owner = cx.weak_entity();
        let prompt = match target.axis {
            LabNumericAxis::Whole if target.property.is_pair() => "Link pair",
            LabNumericAxis::Whole => "Link adjuster",
            LabNumericAxis::X => "Link X",
            LabNumericAxis::Y => "Link Y",
        };
        let label = name
            .as_ref()
            .map_or_else(|| prompt.to_owned(), |name| format!("Adjuster · {name}"));
        Button::new(id)
            .small()
            .ghost()
            .label(label)
            .disabled(busy)
            .dropdown_menu(move |mut menu, _, _| {
                let open = owner.clone();
                let new_target = target.clone();
                menu = menu.item(PopupMenuItem::new("New adjuster…").on_click(
                    move |_, window, cx| {
                        let _ = open.update(cx, |view, cx| {
                            view.open_numeric_link_name(new_target.clone(), window, cx)
                        });
                    },
                ));
                if !choices.is_empty() {
                    menu = menu.separator();
                }
                for control in &choices {
                    let checked = name.as_deref() == Some(control.name());
                    let owner = owner.clone();
                    let target = target.clone();
                    let control = control.clone();
                    menu = menu.item(
                        PopupMenuItem::new(control.name().to_owned())
                            .checked(checked)
                            .on_click(move |_, window, cx| {
                                let _ = owner.update(cx, |view, cx| {
                                    view.error = view
                                        .change_numeric_link(
                                            &target,
                                            LabNumericLink::Existing(control.clone()),
                                            window,
                                            cx,
                                        )
                                        .err();
                                    cx.notify();
                                });
                            }),
                    );
                }
                if name.is_some() {
                    let axes: &[_] = match target.axis {
                        LabNumericAxis::Whole if target.property.is_pair() => &[
                            (LabNumericAxis::X, "Edit X mapping…"),
                            (LabNumericAxis::Y, "Edit Y mapping…"),
                        ],
                        axis => &[(axis, "Edit mapping…")],
                    };
                    menu = menu.separator();
                    for &(axis, label) in axes {
                        let owner = owner.clone();
                        let target = target.clone();
                        menu =
                            menu.item(PopupMenuItem::new(label).on_click(move |_, window, cx| {
                                let _ = owner.update(cx, |view, cx| {
                                    view.open_numeric_mapping(&target, axis, window, cx)
                                });
                            }));
                    }
                    let owner = owner.clone();
                    let target = target.clone();
                    menu = menu
                        .separator()
                        .item(PopupMenuItem::new("Unlink adjuster").on_click(
                            move |_, window, cx| {
                                let _ = owner.update(cx, |view, cx| {
                                    view.error = view
                                        .change_numeric_link(
                                            &target,
                                            LabNumericLink::Unlink,
                                            window,
                                            cx,
                                        )
                                        .err();
                                    cx.notify();
                                });
                            },
                        ));
                }
                menu.scrollable(true)
            })
    }

    fn check_numeric_link(&self, target: &LinkTarget, cx: &App) -> Result<(), String> {
        self.check_lab_selection(target.node.as_ref())?;
        if self.inspector.is_changed(cx) {
            return Err("Correct or reset field text before changing a Lab link.".into());
        }
        Ok(())
    }

    fn change_numeric_link(
        &mut self,
        target: &LinkTarget,
        action: LabNumericLink,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        self.check_numeric_link(target, cx)?;
        self.commit_lab_adjuster_change(window, cx, |document| {
            document.set_numeric_property_link(&target.property, target.axis, action)
        })
    }

    fn open_numeric_link_name(
        &mut self,
        target: LinkTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_busy() {
            return;
        }
        if let Err(error) = self.check_numeric_link(&target, cx) {
            self.error = Some(error);
            cx.notify();
            return;
        }
        self.pause_preview();
        self.modal = true;
        let input = cx.new(|cx| InputState::new(window, cx));
        let form = cx.new(|_| AdjusterName { input, error: None });
        let owner = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let submit = form.clone();
            let edit = owner.clone();
            let close = owner.clone();
            let target = target.clone();
            dialog
                .title("New numeric adjuster")
                .child(form.clone())
                .footer(
                    DialogFooter::new()
                        .child(DialogClose::new().child(Button::new("cancel").label("Cancel")))
                        .child(
                            DialogAction::new().child(Button::new("ok").primary().label("Create")),
                        ),
                )
                .on_ok(move |_, window, cx| {
                    let name = submit.read(cx).input.read(cx).value().to_string();
                    let result = edit
                        .update(cx, |view, cx| {
                            view.change_numeric_link(
                                &target,
                                LabNumericLink::Create(name),
                                window,
                                cx,
                            )
                        })
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
