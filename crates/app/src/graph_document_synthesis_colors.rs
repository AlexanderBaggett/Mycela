//! Typed Lab Color and direct RGB authoring for Synthesis.
use super::super::gradient::ColorAdjusterMenu;
use super::*;
use crate::graph_document::view::numeric_field::{NumericField, NumericRange};
use gpui_kit::{Rgba, component::IconName};
use mycela_engine::{ColorAdjusterLink, SynthesisColorTarget};

impl GraphDocumentView {
    pub(super) fn synthesis_layer_color_button(
        &self,
        target: &Target,
        open: bool,
        busy: bool,
        cx: &Context<Self>,
    ) -> Button {
        let target = target.clone();
        Button::new("synthesis-layer-color")
            .label("Color")
            .tooltip("Layer color")
            .icon(if open {
                IconName::ChevronUp
            } else {
                IconName::ChevronDown
            })
            .small()
            .flex_shrink_0()
            .tab_index(0)
            .selected(open)
            .disabled(busy)
            .on_click(cx.listener(move |view, _, _, cx| {
                if view.is_busy() || view.has_field_drafts(cx) || !target.current(view) {
                    return;
                }
                if let Some(panel) = view.synthesis.as_mut() {
                    panel.layer_color_open = !panel.layer_color_open;
                    cx.notify();
                }
            }))
    }

    pub(super) fn render_synthesis_color(
        &self,
        panel: &Panel,
        target: &Target,
        (busy, disabled): (bool, bool),
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let color = panel
            .fields
            .iter()
            .find_map(|(field, _)| field.color_target())?;
        let rgb = panel.edit.color_value(color)?;
        let (label, adjuster_id) = match color {
            SynthesisColorTarget::Master => ("Colorize", "synthesis-master-color-adjuster"),
            SynthesisColorTarget::Layer(_) => ("Colorize", "synthesis-layer-color-adjuster"),
            SynthesisColorTarget::Solid(_) => ("Color", "synthesis-solid-color-adjuster"),
        };
        let [r, g, b] = rgb.map(|value| value.clamp(0., 1.) as f32);
        let channels = [
            ("synthesis-color-red-value", "Red"),
            ("synthesis-color-green-value", "Green"),
            ("synthesis-color-blue-value", "Blue"),
        ];
        let has_link = panel.edit.is_color_linked(color);
        let form = Form::new().columns(3).children(
            panel
                .fields
                .iter()
                .enumerate()
                .filter(|(_, (field, _))| field.color_target() == Some(color))
                .zip(channels.into_iter().zip(rgb))
                .map(|((index, (field, input)), ((channel_id, label), value))| {
                    let control = if has_link {
                        let text = format!("{value:.6}");
                        let text = text.trim_end_matches('0').trim_end_matches('.');
                        adjusters::render_synthesis_readout(
                            channel_id,
                            format!("{label} {text}"),
                            text.to_owned(),
                            cx,
                        )
                    } else {
                        NumericField::new(
                            input,
                            ("synthesis-control", index),
                            label,
                            NumericRange::bounds(field.range(), 0.001),
                        )
                        .disabled(busy)
                        .into_any_element()
                    };
                    Field::new().label(label).child(control)
                }),
        );
        let menu = if target.material_linked() {
            None
        } else {
            let (linked, choices) = {
                let document = guard(&self.document);
                (
                    document.synthesis_color_adjuster(&panel.edit, color),
                    document.available_synthesis_color_adjusters(&target.node),
                )
            };
            let label = match &linked {
                Some(control) => format!("Adjuster · {}", control.name()),
                None if has_link => "Missing Color adjuster".into(),
                None => "Link Color adjuster".into(),
            };
            let target = target.clone();
            Some(self.render_color_adjuster_menu(
                ColorAdjusterMenu {
                    id: adjuster_id,
                    kind: super::super::adjuster_name::AdjusterNameKind::Color,
                    label,
                    linked,
                    choices,
                    has_link,
                    disabled,
                    edit_linked: true,
                },
                move |view, link, window, cx| {
                    view.change_synthesis_color_link(&target, color, link, window, cx)
                },
                cx,
            ))
        };
        Some(
            div()
                .v_flex()
                .gap_1()
                .child(
                    div()
                        .h_flex()
                        .items_center()
                        .gap_2()
                        .min_w_0()
                        .child(
                            div()
                                .id("synthesis-color-swatch")
                                .test_support()
                                .role(gpui_kit::Role::Image)
                                .aria_label(label)
                                .h_5()
                                .w_7()
                                .flex_shrink_0()
                                .rounded_sm()
                                .border_1()
                                .border_color(cx.theme().border)
                                .bg(Rgba { r, g, b, a: 1. }),
                        )
                        .child(div().flex_shrink_0().text_sm().font_semibold().child(label))
                        .when_some(menu, |row, menu| row.child(div().min_w_0().child(menu))),
                )
                .child(form)
                .into_any_element(),
        )
    }

    fn change_synthesis_color_link(
        &mut self,
        target: &Target,
        color: SynthesisColorTarget,
        link: ColorAdjusterLink,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if self.closed || self.work.is_some() || !target.current(self) {
            return Err("The selected Synthesis Color changed.".into());
        }
        if self.has_field_drafts(cx) {
            return Err("Correct or reset field text before changing the adjuster.".into());
        }
        if target.material_linked() {
            return Err("The shared Material controls this color.".into());
        }
        let expected = self
            .synthesis
            .as_ref()
            .ok_or("The selected Synthesis Color is unavailable.")?
            .edit
            .clone();
        let (node, canvas, pass) = match target.style.as_ref() {
            Some(style) => {
                let (node, canvas, pass) = style
                    .synthesis_number_receiver()
                    .ok_or("The selected Style pass no longer has Texture Synthesis.")?;
                (node, canvas, Some(pass))
            }
            None => (&target.node, None, None),
        };
        self.commit_lab_adjuster_change(window, cx, |document| {
            document.set_synthesis_color_adjuster(node, canvas, pass, &expected, color, link)
        })
    }
}
