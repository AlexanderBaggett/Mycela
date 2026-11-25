use super::super::adjuster_name::AdjusterNameKind;
use super::*;
use mycela_engine::{
    GtxGraphError, LabNumericAdjuster, LabNumericAxis, LabNumericLink, StyleSynthesisNumberTarget,
    SynthesisAdjusterLink, SynthesisNumberField, SynthesisPairField,
};

pub(super) struct NumberControls {
    pub(super) value: Option<AnyElement>,
    pub(super) menu: Option<AnyElement>,
}

#[derive(Clone, Copy)]
pub(super) enum Receiver {
    Scalar(SynthesisNumberField),
    Pair(SynthesisPairField, LabNumericAxis),
}

pub(super) struct AdjusterMenu {
    pub(super) receiver: Receiver,
    pub(super) id: String,
    pub(super) prompt: &'static str,
    pub(super) name: Option<String>,
    pub(super) current: Option<LabNumericAdjuster>,
    pub(super) choices: Vec<LabNumericAdjuster>,
    pub(super) inactive: bool,
    pub(super) disabled: bool,
}

impl Receiver {
    fn naming_kind(self) -> AdjusterNameKind {
        match self {
            Self::Pair(_, LabNumericAxis::Whole) => AdjusterNameKind::Dual,
            _ => AdjusterNameKind::Number,
        }
    }

    fn mappings(self) -> [Option<(Self, &'static str)>; 2] {
        match self {
            Self::Pair(field, LabNumericAxis::Whole) => [
                Some((Self::Pair(field, LabNumericAxis::X), "Edit X mapping…")),
                Some((Self::Pair(field, LabNumericAxis::Y), "Edit Y mapping…")),
            ],
            _ => [Some((self, "Edit mapping…")), None],
        }
    }

    fn conversion(self) -> Option<(SynthesisPairField, LabNumericAxis, &'static str)> {
        match self {
            Self::Scalar(_) => None,
            Self::Pair(field, LabNumericAxis::Whole) => {
                Some((field, LabNumericAxis::X, "Convert to Individual Sliders"))
            }
            Self::Pair(field, _) => Some((field, LabNumericAxis::Whole, "Convert to Dual Numeric")),
        }
    }
}

fn number_control_id(group: &str, field: SynthesisNumberField) -> String {
    let field = match field {
        SynthesisNumberField::Seed => "seed".into(),
        SynthesisNumberField::Scale => "texture-scale".into(),
        SynthesisNumberField::MasterColor(field) => format!("master-{}", field.id()),
        SynthesisNumberField::LayerColor(field) => format!("layer-{}", field.id()),
        SynthesisNumberField::PatchFrequency(axis) => format!("patch-frequency-{axis}"),
        SynthesisNumberField::PatchFeather(index) => format!("patch-feather-{index}"),
        SynthesisNumberField::PatchJitterShape(index) => format!("patch-jitter-shape-{index}"),
        SynthesisNumberField::PatchJitterFrequency(index) => {
            format!("patch-jitter-frequency-{index}")
        }
        SynthesisNumberField::PatchShadowStrength(index) => {
            format!("patch-shadow-strength-{index}")
        }
        SynthesisNumberField::PatchShadowFeather(index) => format!("patch-shadow-feather-{index}"),
        SynthesisNumberField::PositionJitterX => "position-jitter-x".into(),
        SynthesisNumberField::PositionJitterY => "position-jitter-y".into(),
        SynthesisNumberField::AngleJitter => "angle-jitter".into(),
        SynthesisNumberField::Density => "density".into(),
        SynthesisNumberField::Slide => "slide".into(),
        SynthesisNumberField::PatchScale => "patch-scale".into(),
        SynthesisNumberField::ScaleJitter => "scale-jitter".into(),
        SynthesisNumberField::PatchAngle => "patch-angle".into(),
        SynthesisNumberField::PatchSwitcher => "patch-switcher".into(),
        SynthesisNumberField::EqualizeStrength => "equalize-strength".into(),
        SynthesisNumberField::RepeatFrequency => "repeat-frequency".into(),
        SynthesisNumberField::RepeatFeather => "repeat-feather".into(),
        SynthesisNumberField::BlendOpacity => "blend-opacity".into(),
        SynthesisNumberField::BlendErode => "blend-erode".into(),
        SynthesisNumberField::BlendSharpness => "blend-sharpness".into(),
        SynthesisNumberField::BlendMode => "blend-mode".into(),
        SynthesisNumberField::ErodeMode => "erode-mode".into(),
        SynthesisNumberField::PhotoStretchX => "photo-stretch-x".into(),
        SynthesisNumberField::PhotoStretchY => "photo-stretch-y".into(),
        SynthesisNumberField::PhotoFeatherX => "photo-feather-x".into(),
        SynthesisNumberField::PhotoFeatherY => "photo-feather-y".into(),
        SynthesisNumberField::PhotoStarFeather => "photo-star-feather".into(),
    };
    format!("{group}-{field}")
}

fn selection_index(field: SynthesisNumberField) -> Option<usize> {
    match field {
        SynthesisNumberField::PatchFrequency(index)
        | SynthesisNumberField::PatchFeather(index)
        | SynthesisNumberField::PatchJitterShape(index)
        | SynthesisNumberField::PatchJitterFrequency(index)
        | SynthesisNumberField::PatchShadowStrength(index)
        | SynthesisNumberField::PatchShadowFeather(index) => Some(index),
        _ => None,
    }
}

impl Panel {
    pub(super) fn number_controls_disabled(&self, field: SynthesisField) -> bool {
        if field.requires_automatic_placement() && self.automatic_placement_off() {
            return true;
        }
        let has_source = self.has_layer_source();
        if let Some(index) = field.number_field().and_then(selection_index) {
            return !has_source
                || !matches!(self.geometry, Some(SynthesisGeometry::Patch { selection, .. }) if selection == index);
        }
        match field.number_field() {
            Some(SynthesisNumberField::EqualizeStrength) => {
                !has_source
                    || self
                        .layer
                        .and_then(|layer| self.edit.equalizes_source(layer))
                        != Some(true)
            }
            Some(SynthesisNumberField::RepeatFrequency | SynthesisNumberField::RepeatFeather) => {
                !has_source
            }
            _ => false,
        }
    }

    pub(super) fn linked_automatic_numbers(&self, layer: usize) -> bool {
        self.edit
            .fields(Some(layer))
            .into_iter()
            .chain(self.edit.patch_switcher_field(layer))
            .filter(|field| field.requires_automatic_placement())
            .filter_map(SynthesisField::number_field)
            .any(|field| linked_number_name(&self.edit, Some(layer), field).is_some())
    }

    pub(super) fn number_linked(&self, field: SynthesisNumberField) -> bool {
        linked_number_name(&self.edit, self.layer, field).is_some()
    }
}

fn linked_number_name(
    edit: &SynthesisEdit,
    layer: Option<usize>,
    field: SynthesisNumberField,
) -> Option<String> {
    edit.number_adjustment(layer, field)
        .and_then(|record| record.components().first())
        .filter(|component| component.active && !component.name.is_empty())
        .map(|component| String::from_utf16_lossy(&component.name))
}

fn effective_number_text(field: SynthesisNumberField, value: f64) -> String {
    let precision = match field {
        SynthesisNumberField::MasterColor(_)
        | SynthesisNumberField::LayerColor(_)
        | SynthesisNumberField::EqualizeStrength
        | SynthesisNumberField::RepeatFeather
        | SynthesisNumberField::BlendOpacity
        | SynthesisNumberField::BlendErode
        | SynthesisNumberField::BlendSharpness
        | SynthesisNumberField::PhotoStretchX
        | SynthesisNumberField::PhotoStretchY
        | SynthesisNumberField::PhotoFeatherX
        | SynthesisNumberField::PhotoFeatherY
        | SynthesisNumberField::PhotoStarFeather
        | SynthesisNumberField::PatchFeather(_)
        | SynthesisNumberField::PatchJitterShape(_)
        | SynthesisNumberField::PatchShadowStrength(_)
        | SynthesisNumberField::PatchShadowFeather(_) => 1,
        SynthesisNumberField::Scale => 3,
        _ => return number_text(value),
    };
    let text = format!("{value:.precision$}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" {
        "0".into()
    } else {
        text.into()
    }
}

pub(super) fn render_effective_value(
    id: &str,
    value: String,
    resolved: bool,
    cx: &App,
) -> AnyElement {
    let description = if resolved {
        "Effective value"
    } else {
        "Fallback value"
    };
    render_synthesis_readout(
        format!("{id}-value"),
        format!("{description}: {value}"),
        if resolved {
            value
        } else {
            format!("Fallback · {value}")
        },
        cx,
    )
}

pub(super) fn render_synthesis_readout(
    id: impl Into<gpui_kit::ElementId>,
    label: impl Into<gpui_kit::SharedString>,
    value: impl Into<gpui_kit::SharedString>,
    cx: &App,
) -> AnyElement {
    let value: gpui_kit::SharedString = value.into();
    div()
        .id(id)
        .aria_label(label)
        .test_support()
        .h_flex()
        .min_h_6()
        .px_2()
        .rounded_sm()
        .bg(cx.theme().muted)
        .text_sm()
        .child(value)
        .into_any_element()
}

impl GraphDocumentView {
    pub(super) fn render_synthesis_number_adjuster(
        &self,
        panel: &Panel,
        target: &Target,
        field: SynthesisField,
        group: &str,
        disabled: bool,
        cx: &Context<Self>,
    ) -> Option<NumberControls> {
        let automatic_off = field.requires_automatic_placement() && panel.automatic_placement_off();
        let inactive = panel.number_controls_disabled(field);
        let field = field.number_field()?;
        let material = target.material_linked();
        let (current, choices) = if material {
            (None, Vec::new())
        } else {
            let doc = guard(&self.document);
            if let Some(style) = &target.style {
                let (node, _, _) = style.synthesis_number_receiver()?;
                (
                    doc.style_synthesis_number_adjuster(&panel.edit, target.layer, field),
                    doc.available_style_synthesis_number_adjusters(
                        node,
                        &panel.edit,
                        target.layer,
                        field,
                    ),
                )
            } else {
                let node = &target.node;
                (
                    doc.synthesis_number_adjuster(node, &panel.edit, target.layer, field),
                    doc.available_synthesis_number_adjusters(
                        node,
                        &panel.edit,
                        target.layer,
                        field,
                    ),
                )
            }
        };
        let name = linked_number_name(&panel.edit, target.layer, field);
        let resolved = material || current.is_some();
        let id = number_control_id(group, field);
        let value = name
            .as_ref()
            .filter(|_| {
                !matches!(
                    field,
                    SynthesisNumberField::PatchSwitcher
                        | SynthesisNumberField::BlendMode
                        | SynthesisNumberField::ErodeMode
                )
            })
            .map(|_| {
                let number = panel
                    .edit
                    .number_value(target.layer, field)
                    .map(|value| effective_number_text(field, value))
                    .unwrap_or_else(|| "Unavailable".into());
                render_effective_value(&id, number, resolved, cx)
            });
        if material {
            return Some(NumberControls { value, menu: None });
        }
        let menu = self.render_synthesis_link_menu(
            target,
            AdjusterMenu {
                receiver: Receiver::Scalar(field),
                id,
                prompt: "Link adjuster",
                name,
                current,
                choices,
                inactive,
                disabled: disabled
                    || automatic_off
                    || (inactive && selection_index(field).is_some()),
            },
            cx,
        );
        Some(NumberControls {
            value,
            menu: Some(menu),
        })
    }

    pub(super) fn render_synthesis_link_menu(
        &self,
        target: &Target,
        menu: AdjusterMenu,
        cx: &Context<Self>,
    ) -> AnyElement {
        let AdjusterMenu {
            receiver,
            id,
            prompt,
            name,
            current,
            choices,
            inactive,
            disabled,
        } = menu;
        let resolved = current.is_some();
        let label = match &name {
            Some(name) if resolved => match receiver {
                Receiver::Pair(_, LabNumericAxis::Whole) => format!("Dual Numeric · {name}"),
                _ => format!("Adjuster · {name}"),
            },
            Some(name) => format!("Missing adjuster · {name}"),
            None => prompt.into(),
        };
        let owner = cx.weak_entity();
        let target = target.clone();
        let menu_disabled = disabled || (inactive && name.is_none());
        let repeat = matches!(
            receiver,
            Receiver::Pair(
                SynthesisPairField::PhotoRepeat | SynthesisPairField::ManualRepeat,
                _
            )
        );
        let button = Button::new(format!("{id}-adjuster"))
            .small()
            .ghost()
            .when(repeat, |button| button.w_full().min_w_0().max_w_full())
            .label(label.clone())
            .tooltip(label)
            .tab_index(0)
            .disabled(menu_disabled);
        if menu_disabled {
            return button.into_any_element();
        }
        button
            .dropdown_menu(move |mut menu, _, _| {
                let open_owner = owner.clone();
                let open_target = target.clone();
                menu = menu.item(
                    PopupMenuItem::new("New adjuster…")
                        .disabled(inactive)
                        .on_click(move |_, window, cx| {
                            let _ = open_owner.update(cx, |view, cx| {
                                if !view.is_busy()
                                    && open_target.current(view)
                                    && !view.has_field_drafts(cx)
                                {
                                    let target = open_target.clone();
                                    view.open_adjuster_name(
                                        receiver.naming_kind(),
                                        move |view, name, window, cx| {
                                            view.change_synthesis_link(
                                                &target,
                                                receiver,
                                                LabNumericLink::Create(name),
                                                window,
                                                cx,
                                            )
                                        },
                                        window,
                                        cx,
                                    );
                                }
                            });
                        }),
                );
                if !choices.is_empty() {
                    menu = menu.separator();
                }
                let action = |item: PopupMenuItem, link: LabNumericLink| {
                    let owner = owner.clone();
                    let target = target.clone();
                    item.on_click(move |_, window, cx| {
                        let _ = owner.update(cx, |view, cx| {
                            if !view.is_busy() {
                                view.error = view
                                    .change_synthesis_link(
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
                    })
                };
                for control in &choices {
                    menu = menu.item(action(
                        PopupMenuItem::new(control.name().to_owned())
                            .disabled(inactive)
                            .checked(
                                current
                                    .as_ref()
                                    .is_some_and(|active| active.same_reference(control)),
                            ),
                        LabNumericLink::Existing(control.clone()),
                    ));
                }
                if name.is_some() {
                    menu = menu.separator();
                    for (mapping, label) in receiver
                        .mappings()
                        .into_iter()
                        .flatten()
                        .filter(|_| resolved && !inactive)
                    {
                        let owner = owner.clone();
                        let target = target.clone();
                        menu =
                            menu.item(PopupMenuItem::new(label).on_click(move |_, window, cx| {
                                let _ = owner.update(cx, |view, cx| {
                                    view.open_synthesis_mapping(&target, mapping, window, cx);
                                });
                            }));
                    }
                    if resolved
                        && !inactive
                        && let Some((field, axis, label)) = receiver.conversion()
                    {
                        let owner = owner.clone();
                        let target = target.clone();
                        menu =
                            menu.item(PopupMenuItem::new(label).on_click(move |_, window, cx| {
                                let _ = owner.update(cx, |view, cx| {
                                    view.error = view
                                        .convert_synthesis_pair_link(
                                            &target,
                                            (field, axis),
                                            window,
                                            cx,
                                        )
                                        .err();
                                    cx.notify();
                                });
                            }));
                    }
                    menu = menu.item(action(
                        PopupMenuItem::new("Unlink adjuster"),
                        LabNumericLink::Unlink,
                    ));
                }
                menu.scrollable(true)
            })
            .into_any_element()
    }

    fn convert_synthesis_pair_link(
        &mut self,
        target: &Target,
        (field, axis): (SynthesisPairField, LabNumericAxis),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        self.check_synthesis_number_target(target, cx)?;
        if target.material_linked() {
            return Err("The shared Material controls this field.".into());
        }
        let expected = self
            .synthesis
            .as_ref()
            .ok_or("The selected Synthesis field is unavailable.")?
            .edit
            .clone();
        let (node, canvas, receiver) = target
            .pair_receiver(field, axis)
            .ok_or("The selected Synthesis field is unavailable.")?;
        self.commit_lab_adjuster_change(window, cx, |document| {
            document.convert_synthesis_pair_adjuster(node, canvas, &expected, receiver)
        })
    }

    fn change_synthesis_link(
        &mut self,
        target: &Target,
        receiver: Receiver,
        link: LabNumericLink,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        self.check_synthesis_number_target(target, cx)?;
        if target.material_linked() {
            return Err("The shared Material controls this field.".into());
        }
        let expected = self
            .synthesis
            .as_ref()
            .ok_or("The selected Synthesis field is unavailable.")?
            .edit
            .clone();
        let field = match receiver {
            Receiver::Scalar(field) => field,
            Receiver::Pair(field, axis) => {
                let (node, canvas, receiver) = target
                    .pair_receiver(field, axis)
                    .ok_or("The selected Synthesis field is unavailable.")?;
                return self.commit_lab_adjuster_change(window, cx, |document| {
                    document.set_synthesis_pair_adjuster(node, canvas, &expected, receiver, link)
                });
            }
        };
        if let Some(style) = &target.style {
            let (node, canvas, pass) = style
                .synthesis_number_receiver()
                .ok_or("The selected Style pass no longer has Texture Synthesis.")?;
            let receiver = StyleSynthesisNumberTarget::new(pass, target.layer, field);
            self.commit_lab_adjuster_change(window, cx, |document| {
                document
                    .set_style_synthesis_number_adjuster(node, canvas, &expected, receiver, link)
            })
        } else {
            self.commit_lab_adjuster_change(window, cx, |document| {
                document.set_synthesis_number_adjuster(
                    &target.node,
                    &expected,
                    target.layer,
                    field,
                    link,
                )
            })
        }
    }

    fn check_synthesis_number_target(&self, target: &Target, cx: &App) -> Result<(), String> {
        if self.closed || self.work.is_some() || !target.current(self) {
            return Err("The selected Synthesis field changed.".into());
        }
        if self.has_field_drafts(cx) {
            return Err("Correct or reset field text before changing the adjuster.".into());
        }
        Ok(())
    }

    fn open_synthesis_mapping(
        &mut self,
        target: &Target,
        receiver: Receiver,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_busy() {
            return;
        }
        let result = (|| {
            self.check_synthesis_number_target(target, cx)?;
            if target.material_linked() {
                return Err("The shared Material controls this field.".into());
            }
            let expected = &self
                .synthesis
                .as_ref()
                .ok_or("The selected Synthesis field is unavailable.")?
                .edit;
            let document = guard(&self.document);
            let field = match receiver {
                Receiver::Scalar(field) => field,
                Receiver::Pair(field, axis) => {
                    let (node, canvas, receiver) = target
                        .pair_receiver(field, axis)
                        .ok_or("The selected Synthesis field is unavailable.")?;
                    return document
                        .synthesis_pair_mapping(node, canvas, expected, receiver)
                        .map(Arc::new)
                        .map_err(model::failure);
                }
            };
            let mapping = if let Some(style) = &target.style {
                let (node, canvas, pass) = style
                    .synthesis_number_receiver()
                    .ok_or("The selected Style pass no longer has Texture Synthesis.")?;
                document.style_synthesis_number_mapping(
                    node,
                    canvas,
                    expected,
                    StyleSynthesisNumberTarget::new(pass, target.layer, field),
                )
            } else {
                document.synthesis_number_mapping(&target.node, expected, target.layer, field)
            };
            mapping.map(Arc::new).map_err(model::failure)
        })();
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
                view.check_synthesis_number_target(&target, cx)?;
                let input = view
                    .synthesis
                    .as_ref()
                    .and_then(|panel| view.synthesis_patch_input_geometry(panel));
                view.commit_lab_adjuster_change(window, cx, |document| {
                    document.edit_synthesis_number_mapping_with_input_geometry(&mapping, input)
                })
            },
            window,
            cx,
        );
    }

    pub(super) fn disable_linked_synthesis_placement(
        &mut self,
        target: &Target,
        layer: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        self.check_synthesis_number_target(target, cx)?;
        let expected = self
            .synthesis
            .as_ref()
            .ok_or("The selected Synthesis layer is unavailable.")?
            .edit
            .clone();
        if let Some(style) = &target.style {
            let (node, canvas, pass) = style
                .synthesis_number_receiver()
                .ok_or("The selected Style pass no longer has Texture Synthesis.")?;
            self.commit_lab_adjuster_change(window, cx, |document| {
                document.set_style_synthesis_automatic_placement(
                    node, canvas, &expected, pass, layer, false,
                )
            })
        } else if let Some(material) = &target.editing_material {
            self.commit_lab_adjuster_change(window, cx, |document| {
                document.set_material_synthesis_automatic_placement(
                    &target.node,
                    &expected,
                    layer,
                    false,
                    material,
                )
            })
        } else {
            self.commit_lab_adjuster_change(window, cx, |document| {
                document.set_synthesis_automatic_placement(&target.node, &expected, layer, false)
            })
        }
    }

    pub(super) fn selected_synthesis_material(&self, node: &Node) -> Option<SynthesisAdjuster> {
        let document = guard(&self.document);
        if let Some(filter) = node
            .filter()
            .filter(|filter| filter.is_synthesis_adjusted())
        {
            return document.synthesis_adjuster(&filter);
        }
        let (style, pass) = self.style.as_ref()?.synthesis_style_draft()?;
        document.style_synthesis_adjuster(style, pass)
    }

    pub(super) fn render_synthesis_adjuster(
        &self,
        target: &Target,
        disabled: bool,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let linked = self.selected_synthesis_material(&target.node);
        let choices = {
            let document = guard(&self.document);
            if target.style.is_some() {
                document.available_style_synthesis_adjusters(&target.node)
            } else {
                document.available_synthesis_adjusters(&target.node.filter()?)
            }
        };
        let has_link = target.material_linked();
        let label = match &linked {
            Some(material) => format!("Material · {}", String::from_utf16_lossy(material.name())),
            None if has_link => "Missing Material adjuster".into(),
            None => "Link Material".into(),
        };
        let target = target.clone();
        let owner = cx.weak_entity();
        let button = Button::new("graph-synthesis-adjuster")
            .small()
            .ghost()
            .w_full()
            .min_w_0()
            .max_w_full()
            .label(label.clone())
            .tooltip(label);
        Some(synthesis_menu(button, disabled, move |mut menu, _, _| {
            if linked.is_some() && target.editing_material.is_none() {
                let owner = owner.clone();
                let target = target.clone();
                menu = menu
                    .item(
                        PopupMenuItem::new("Edit shared Material").on_click(move |_, _, cx| {
                            let _ = owner.update(cx, |view, cx| {
                                view.begin_shared_material_edit(&target, cx)
                            });
                        }),
                    )
                    .separator();
            }
            let open_owner = owner.clone();
            let open_target = target.clone();
            menu = menu.item(
                PopupMenuItem::new("New adjuster…").on_click(move |_, window, cx| {
                    let _ = open_owner.update(cx, |view, cx| {
                        if !view.is_busy()
                            && open_target.current(view)
                            && !view.has_field_drafts(cx)
                        {
                            let target = open_target.clone();
                            view.open_adjuster_name(
                                AdjusterNameKind::Synthesis,
                                move |view, name, window, cx| {
                                    view.change_synthesis_adjuster(
                                        &target,
                                        SynthesisAdjusterLink::Create(name),
                                        window,
                                        cx,
                                    )
                                },
                                window,
                                cx,
                            );
                        }
                    });
                }),
            );
            if !choices.is_empty() {
                menu = menu.separator();
            }
            let command = |item: PopupMenuItem, link: SynthesisAdjusterLink| {
                let target = target.clone();
                let owner = owner.clone();
                item.on_click(move |_, window, cx| {
                    let _ = owner.update(cx, |view, cx| {
                        if !view.is_busy() {
                            view.error = view
                                .change_synthesis_adjuster(&target, link.clone(), window, cx)
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
                    PopupMenuItem::new(String::from_utf16_lossy(adjuster.name())).checked(checked),
                    SynthesisAdjusterLink::Existing(adjuster.clone()),
                ));
            }
            if has_link {
                menu = menu.separator().item(command(
                    PopupMenuItem::new("Unlink adjuster"),
                    SynthesisAdjusterLink::Unlink,
                ));
            }
            menu
        }))
    }

    fn begin_shared_material_edit(&mut self, target: &Target, cx: &mut Context<Self>) {
        if self.is_busy()
            || !target.current(self)
            || self.has_field_drafts(cx)
            || !target.material_locked()
        {
            return;
        }
        let linked = self.selected_synthesis_material(&target.node);
        if let Some(panel) = self.synthesis.as_mut() {
            panel.editing_material = linked;
            cx.notify();
        }
    }

    fn change_synthesis_adjuster(
        &mut self,
        target: &Target,
        link: SynthesisAdjusterLink,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if self.closed || self.work.is_some() || !target.current(self) {
            return Err("The selected Texture Synthesis changed.".into());
        }
        if self.has_field_drafts(cx) {
            return Err("Correct or reset field text before changing the adjuster.".into());
        }
        let filter = target
            .node
            .filter()
            .ok_or("The Synthesis node is unavailable.")?;
        self.ensure_history_binding();
        let dirty = self.dirty;
        let stamp = self.begin_history_edit();
        let result = {
            let mut document = guard(&self.document);
            if let Some(style) = &target.style {
                let (owner, canvas, pass) = style
                    .synthesis_number_receiver()
                    .ok_or("The selected Texture Synthesis changed.")?;
                let (expected, _) = self
                    .style
                    .as_ref()
                    .and_then(style::Panel::synthesis_style_draft)
                    .ok_or("The selected Texture Synthesis changed.")?;
                document.set_style_synthesis_adjuster(owner, canvas, expected, pass, link)
            } else {
                document.set_synthesis_adjuster(&filter, link)
            }
        };
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
                if matches!(error, GtxGraphError::SynthesisAdjuster(_)) {
                    self.dirty = dirty;
                }
                return Err(model::failure(error));
            }
        }
        Ok(())
    }
}
