//! Curve adjuster commands in the existing composite curve inspector.
use super::super::inspector::AnimationParameter;
use super::{adjuster_name::AdjusterNameKind, animation::AnimationTarget, *};
use gpui_kit::AnyElement;
use mycela_engine::CurveAdjusterLink;

impl GraphDocumentView {
    pub(super) fn render_curve_adjuster(
        &self,
        target: &AnimationTarget,
        disabled: bool,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let AnimationParameter::DirectCurve(property) = &target.item.parameter else {
            return None;
        };
        if !property.is_adjustable() {
            return None;
        }
        let (linked, choices) = {
            let document = guard(&self.document);
            (
                document.curve_adjuster(property),
                document.available_curve_adjusters(property),
            )
        };
        let has_link = property
            .retained_adjustment()
            .is_some_and(|adjustment| adjustment.is_active().unwrap_or(false));
        let label = linked.as_ref().map_or_else(
            || {
                if has_link {
                    "Missing Curve adjuster".to_owned()
                } else {
                    "Link Curve adjuster".to_owned()
                }
            },
            |adjuster| format!("Adjuster · {}", String::from_utf16_lossy(adjuster.name())),
        );
        let owner = cx.weak_entity();
        let target = target.clone();
        Some(
            Button::new("graph-curve-adjuster")
                .small()
                .ghost()
                .label(label)
                .disabled(disabled || self.has_field_drafts(cx))
                .dropdown_menu(move |mut menu, _, _| {
                    let create_owner = owner.clone();
                    let create_target = target.clone();
                    menu = menu.item(PopupMenuItem::new("New adjuster…").on_click(
                        move |_, window, cx| {
                            let _ = create_owner.update(cx, |view, cx| {
                                if !view.is_busy()
                                    && !view.has_field_drafts(cx)
                                    && view.is_original_selection(
                                        &create_target.selection,
                                        &create_target.node,
                                        create_target.revision,
                                    )
                                {
                                    let target = create_target.clone();
                                    view.open_adjuster_name(
                                        AdjusterNameKind::Curve,
                                        move |view, name, window, cx| {
                                            view.change_curve_adjuster(
                                                &target,
                                                CurveAdjusterLink::Create(name),
                                                window,
                                                cx,
                                            )
                                        },
                                        window,
                                        cx,
                                    );
                                }
                            });
                        },
                    ));
                    if !choices.is_empty() {
                        menu = menu.separator();
                    }
                    let command = |item: PopupMenuItem, link: CurveAdjusterLink| {
                        let owner = owner.clone();
                        let target = target.clone();
                        item.on_click(move |_, window, cx| {
                            let _ = owner.update(cx, |view, cx| {
                                if !view.is_busy() {
                                    view.error = view
                                        .change_curve_adjuster(&target, link.clone(), window, cx)
                                        .err();
                                    cx.notify();
                                }
                            });
                        })
                    };
                    for adjuster in &choices {
                        let checked = linked
                            .as_ref()
                            .is_some_and(|current| current.same_reference(adjuster));
                        menu = menu.item(command(
                            PopupMenuItem::new(String::from_utf16_lossy(adjuster.name()))
                                .checked(checked),
                            CurveAdjusterLink::Existing(adjuster.clone()),
                        ));
                    }
                    if has_link {
                        menu = menu.separator().item(command(
                            PopupMenuItem::new("Unlink adjuster"),
                            CurveAdjusterLink::Unlink,
                        ));
                    }
                    menu
                })
                .into_any_element(),
        )
    }

    fn change_curve_adjuster(
        &mut self,
        target: &AnimationTarget,
        link: CurveAdjusterLink,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if self.work.is_some()
            || !self.is_original_selection(&target.selection, &target.node, target.revision)
        {
            return Err("The selected Curve changed.".into());
        }
        if self.has_field_drafts(cx) {
            return Err("Correct or reset field text before changing the adjuster.".into());
        }
        let AnimationParameter::DirectCurve(property) = &target.item.parameter else {
            return Err("The selected Curve changed.".into());
        };
        self.ensure_history_binding();
        let dirty = self.dirty;
        let stamp = self.begin_history_edit();
        let result = guard(&self.document).set_curve_adjuster(property, link);
        match result {
            Ok(true) => {
                self.revision += 1;
                self.status = None;
                self.error = None;
                self.finish_history_edit(stamp, true);
                self.rebuild(window, cx);
                self.start_preview(cx);
            }
            Ok(false) => self.dirty = dirty,
            Err(error) => {
                if matches!(error, mycela_engine::GtxGraphError::CurveAdjuster(_)) {
                    self.dirty = dirty;
                }
                return Err(model::failure(error));
            }
        }
        Ok(())
    }
}
