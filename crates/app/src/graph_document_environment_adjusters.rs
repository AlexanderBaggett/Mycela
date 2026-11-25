use super::*;
use mycela_engine::{
    EnvironmentAdjusterLink, EnvironmentColorLink, EnvironmentMapTarget,
    EnvironmentNumericReceiver, GtxGraphError, LabColorAdjuster, LabNumericAdjuster,
    LabNumericLink,
};

use super::super::adjuster_name::AdjusterNameKind;

impl EnvironmentPanel {
    pub(super) fn map_target(&self) -> EnvironmentMapTarget<'_, ForegroundError> {
        match &self.owner {
            MapOwner::Parameter(property) => EnvironmentMapTarget::Property(property),
            MapOwner::Style(owner) => owner.map_target(),
        }
    }

    pub(super) fn can_link_adjuster(&self) -> bool {
        self.supplier.is_none()
            && self
                .parameter()
                .is_none_or(PointwiseParameter::is_adjustable)
    }

    fn available_adjusters(&self, document: &model::Document) -> Vec<EnvironmentAdjuster> {
        match &self.owner {
            MapOwner::Parameter(parameter) => document.available_environment_adjusters(parameter),
            MapOwner::Style(owner) => owner.available_adjusters(document),
        }
    }

    fn set_adjuster(
        &self,
        document: &mut model::Document,
        link: EnvironmentAdjusterLink,
    ) -> Result<bool, GtxGraphError<ForegroundError>> {
        match &self.owner {
            MapOwner::Parameter(parameter) => document.set_environment_adjuster(parameter, link),
            MapOwner::Style(owner) => owner.set_adjuster(document, self.value(), link),
        }
    }
}

impl GraphDocumentView {
    pub(super) fn render_environment_adjuster_menu(
        &self,
        target: &Target,
        busy: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let choices = target.panel.available_adjusters(&guard(&self.document));
        let linked = target.panel.has_adjuster_link();
        let target = target.clone();
        let owner = cx.weak_entity();
        Button::new("graph-environment-adjuster")
            .small()
            .ghost()
            .label(if linked { "Adjuster" } else { "Link adjuster" })
            .disabled(busy)
            .dropdown_menu(move |mut menu, _, _| {
                let open_owner = owner.clone();
                let open_target = target.clone();
                menu = menu.item(PopupMenuItem::new("New adjuster…").on_click(
                    move |_, window, cx| {
                        let _ = open_owner.update(cx, |view, cx| {
                            view.open_environment_adjuster_name(&open_target, window, cx);
                        });
                    },
                ));
                if !choices.is_empty() {
                    menu = menu.separator();
                }
                for adjuster in &choices {
                    let active = target
                        .panel
                        .adjuster
                        .as_ref()
                        .is_some_and(|current| current.same_reference(adjuster));
                    let label = String::from_utf16_lossy(adjuster.name());
                    let adjuster = adjuster.clone();
                    let owner = owner.clone();
                    let target = target.clone();
                    menu = menu.item(PopupMenuItem::new(label).checked(active).on_click(
                        move |_, window, cx| {
                            let _ = owner.update(cx, |view, cx| {
                                if !view.is_busy() {
                                    view.error = view
                                        .change_environment_adjuster(
                                            &target,
                                            EnvironmentAdjusterLink::Existing(adjuster.clone()),
                                            window,
                                            cx,
                                        )
                                        .err();
                                    cx.notify();
                                }
                            });
                        },
                    ));
                }
                if linked {
                    let owner = owner.clone();
                    let target = target.clone();
                    menu = menu
                        .separator()
                        .item(PopupMenuItem::new("Unlink adjuster").on_click(
                            move |_, window, cx| {
                                let _ = owner.update(cx, |view, cx| {
                                    if !view.is_busy() {
                                        view.error = view
                                            .change_environment_adjuster(
                                                &target,
                                                EnvironmentAdjusterLink::Unlink,
                                                window,
                                                cx,
                                            )
                                            .err();
                                        cx.notify();
                                    }
                                });
                            },
                        ));
                }
                menu
            })
    }

    fn check_environment_adjuster_target(&self, target: &Target, cx: &App) -> Result<(), String> {
        if self.closed || !target.current(self) {
            return Err("The selected Environment property changed.".into());
        }
        if self.inspector.is_changed(cx) {
            return Err("Correct or reset field text before changing the adjuster.".into());
        }
        Ok(())
    }

    fn open_environment_adjuster_name(
        &mut self,
        target: &Target,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_busy() {
            return;
        }
        if let Err(error) = self.check_environment_adjuster_target(target, cx) {
            self.error = Some(error);
            cx.notify();
            return;
        }
        let target = target.clone();
        self.open_adjuster_name(
            AdjusterNameKind::Environment,
            move |view, text, window, cx| {
                view.change_environment_adjuster(
                    &target,
                    EnvironmentAdjusterLink::Create(text),
                    window,
                    cx,
                )
            },
            window,
            cx,
        );
    }

    fn change_environment_adjuster(
        &mut self,
        target: &Target,
        link: EnvironmentAdjusterLink,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        self.check_environment_adjuster_target(target, cx)?;
        self.ensure_history_binding();
        let dirty = self.dirty;
        let stamp = self.begin_history_edit();
        let result = target.panel.set_adjuster(&mut guard(&self.document), link);
        match result {
            Ok(false) => self.dirty = dirty,
            Ok(true) => {
                self.revision += 1;
                self.status = None;
                self.error = None;
                self.finish_history_edit(stamp, true);
                self.rebuild(window, cx);
                self.start_preview(cx);
            }
            Err(error) => {
                if matches!(error, GtxGraphError::EnvironmentAdjuster(_)) {
                    self.dirty = dirty;
                }
                return Err(model::failure(error));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum Receiver {
    Color(EnvironmentColorField),
    Numeric(EnvironmentNumericReceiver),
}

#[derive(Clone)]
enum FieldLink {
    Create(String),
    Color(LabColorAdjuster),
    Numeric(LabNumericAdjuster),
    Unlink,
}

impl GraphDocumentView {
    pub(super) fn render_environment_field_adjuster(
        &self,
        target: &Target,
        field: EnvironmentColorField,
        busy: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        self.render_field_link_menu(target, Receiver::Color(field), None, busy, cx)
    }

    pub(in crate::graph_document::view) fn render_environment_numeric_adjuster(
        &self,
        panel: &EnvironmentPanel,
        field: EnvironmentNumericReceiver,
        id: String,
        busy: bool,
        cx: &Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        if self.embedded_environment.is_some()
            || !panel.is_editable()
            || !panel.value().supports_numeric_adjuster(field)
        {
            return None;
        }
        let target = Target::for_panel(self, panel);
        Some(self.render_field_link_menu(&target, Receiver::Numeric(field), Some(id), busy, cx))
    }

    fn render_field_link_menu(
        &self,
        target: &Target,
        receiver: Receiver,
        numeric_id: Option<String>,
        busy: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let doc = guard(&self.document);
        let map = target.panel.value();
        let (id, field, key, choices, mapping) = match receiver {
            Receiver::Color(field) => {
                let (id, label) = match field {
                    EnvironmentColorField::LayerColor(_) => {
                        ("graph-environment-layer-color-adjuster", "Layer color")
                    }
                    EnvironmentColorField::Colorize(_) => {
                        ("graph-environment-colorize-adjuster", "Colorize")
                    }
                    EnvironmentColorField::Recolor(_) => {
                        ("graph-environment-recolor-adjuster", "Recolor")
                    }
                };
                let current = doc.environment_color_adjuster(map, field);
                let choices = doc
                    .available_environment_color_adjusters(target.panel.map_target(), field)
                    .into_iter()
                    .map(|control| {
                        (
                            control.name().to_owned(),
                            current
                                .as_ref()
                                .is_some_and(|current| current.same_reference(&control)),
                            FieldLink::Color(control),
                        )
                    })
                    .collect::<Vec<_>>();
                (
                    id.into(),
                    label,
                    map.color_adjuster_key(field),
                    choices,
                    None,
                )
            }
            Receiver::Numeric(field) => {
                let current = doc.environment_numeric_adjuster(map, field);
                let choices = doc
                    .available_environment_numeric_adjusters(target.panel.map_target(), field)
                    .into_iter()
                    .map(|control| {
                        (
                            control.name().to_owned(),
                            current
                                .as_ref()
                                .is_some_and(|current| current.same_reference(&control)),
                            FieldLink::Numeric(control),
                        )
                    })
                    .collect();
                (
                    numeric_id.expect("numeric field ID"),
                    field.label(),
                    map.numeric_adjuster_key(field),
                    choices,
                    current.map(|_| field),
                )
            }
        };
        let linked = key.is_some();
        let inline = matches!(receiver, Receiver::Numeric(field) if !field.is_choice());
        let label = key.map_or_else(
            || {
                if inline {
                    "Link adjuster".to_owned()
                } else {
                    format!("Link {field}")
                }
            },
            |(_, name)| {
                let field = if inline { "Adjuster" } else { field };
                format!("{field} · {}", String::from_utf16_lossy(name))
            },
        );
        let target = target.clone();
        let owner = cx.weak_entity();
        Button::new(id)
            .small()
            .ghost()
            .label(label)
            .when(inline, |button| button.max_w(rems(9.)))
            .disabled(busy)
            .dropdown_menu(move |mut menu, _, _| {
                let new_target = target.clone();
                let new_owner = owner.clone();
                menu = menu.item(PopupMenuItem::new("New adjuster…").on_click(
                    move |_, window, cx| {
                        let _ = new_owner.update(cx, |view, cx| {
                            view.open_environment_field_adjuster_name(
                                &new_target,
                                receiver,
                                window,
                                cx,
                            )
                        });
                    },
                ));
                if !choices.is_empty() {
                    menu = menu.separator();
                }
                let action = |link: FieldLink| {
                    let owner = owner.clone();
                    let target = target.clone();
                    move |_: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut App| {
                        let _ = owner.update(cx, |view, cx| {
                            if !view.is_busy() {
                                view.error = view
                                    .change_environment_field_adjuster(
                                        &target,
                                        receiver,
                                        link.clone(),
                                        window,
                                        cx,
                                    )
                                    .err();
                                cx.notify();
                            }
                        });
                    }
                };
                for (name, active, link) in &choices {
                    menu = menu.item(
                        PopupMenuItem::new(name.clone())
                            .checked(*active)
                            .on_click(action(link.clone())),
                    );
                }
                if linked {
                    menu = menu.separator();
                    if let Some(field) = mapping {
                        let owner = owner.clone();
                        let target = target.clone();
                        menu = menu.item(PopupMenuItem::new("Edit mapping…").on_click(
                            move |_, window, cx| {
                                let _ = owner.update(cx, |view, cx| {
                                    view.open_environment_field_mapping(&target, field, window, cx);
                                });
                            },
                        ));
                    }
                    menu = menu.item(
                        PopupMenuItem::new("Unlink adjuster").on_click(action(FieldLink::Unlink)),
                    );
                }
                menu.scrollable(true)
            })
    }

    fn open_environment_field_mapping(
        &mut self,
        target: &Target,
        field: EnvironmentNumericReceiver,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_busy() {
            return;
        }
        let result = self
            .check_environment_adjuster_target(target, cx)
            .and_then(|()| {
                guard(&self.document)
                    .environment_numeric_mapping(
                        target.panel.map_target(),
                        target.panel.value(),
                        field,
                    )
                    .map(Arc::new)
                    .map_err(model::failure)
            });
        let mapping = match result {
            Ok(mapping) => mapping,
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        let target = target.clone();
        self.open_adjuster_mapping_editor(
            mapping.curve().clone(),
            mapping.bounds(),
            "Edit adjuster mapping",
            move |view, _, window, cx| {
                view.check_environment_adjuster_target(&target, cx)?;
                view.commit_lab_adjuster_change(window, cx, |document| {
                    document.edit_environment_numeric_mapping(&mapping)
                })
            },
            window,
            cx,
        );
    }

    fn open_environment_field_adjuster_name(
        &mut self,
        target: &Target,
        receiver: Receiver,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_busy() {
            return;
        }
        if let Err(error) = self.check_environment_adjuster_target(target, cx) {
            self.error = Some(error);
            cx.notify();
            return;
        }
        let target = target.clone();
        let kind = match receiver {
            Receiver::Color(EnvironmentColorField::Recolor(_)) => AdjusterNameKind::Gradient,
            Receiver::Color(_) => AdjusterNameKind::Color,
            Receiver::Numeric(field) if field.is_choice() => AdjusterNameKind::Selection,
            Receiver::Numeric(_) => AdjusterNameKind::Number,
        };
        self.open_adjuster_name(
            kind,
            move |view, text, window, cx| {
                view.change_environment_field_adjuster(
                    &target,
                    receiver,
                    FieldLink::Create(text),
                    window,
                    cx,
                )
            },
            window,
            cx,
        );
    }

    fn change_environment_field_adjuster(
        &mut self,
        target: &Target,
        receiver: Receiver,
        link: FieldLink,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        self.check_environment_adjuster_target(target, cx)?;
        self.commit_lab_adjuster_change(window, cx, |document| {
            let owner = target.panel.map_target();
            let map = target.panel.value();
            match (receiver, link) {
                (Receiver::Color(field), FieldLink::Create(name)) => document
                    .set_environment_color_adjuster(
                        owner,
                        map,
                        field,
                        EnvironmentColorLink::Create(name),
                    ),
                (Receiver::Color(field), FieldLink::Color(control)) => document
                    .set_environment_color_adjuster(
                        owner,
                        map,
                        field,
                        EnvironmentColorLink::Existing(control),
                    ),
                (Receiver::Color(field), FieldLink::Unlink) => document
                    .set_environment_color_adjuster(
                        owner,
                        map,
                        field,
                        EnvironmentColorLink::Unlink,
                    ),
                (Receiver::Numeric(field), FieldLink::Create(name)) => document
                    .set_environment_numeric_adjuster(
                        owner,
                        map,
                        field,
                        LabNumericLink::Create(name),
                    ),
                (Receiver::Numeric(field), FieldLink::Numeric(control)) => document
                    .set_environment_numeric_adjuster(
                        owner,
                        map,
                        field,
                        LabNumericLink::Existing(control),
                    ),
                (Receiver::Numeric(field), FieldLink::Unlink) => document
                    .set_environment_numeric_adjuster(owner, map, field, LabNumericLink::Unlink),
                _ => Err(GtxGraphError::EnvironmentAdjuster(
                    "This Lab control is incompatible with the field.",
                )),
            }
        })
    }
}
