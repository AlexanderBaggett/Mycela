use super::{
    GraphDocumentView, adjuster_name::AdjusterNameKind, gradient::ColorAdjusterMenu, guard,
    model::failure,
};
use crate::ForegroundError;
use gpui_kit::base::{Disableable, StyledExt, TestSupportExt};
use gpui_kit::component::{
    ActiveTheme, Sizable,
    button::{Button, ButtonVariants},
    color_picker::ColorPickerState,
    form::Field,
    popover::Popover,
    slider::Slider,
};
use gpui_kit::{
    App, AppContext, Context, Entity, Hsla, IntoElement, Rgba, Window, div, prelude::*,
};
use mycela_engine::{ColorAdjusterLink, PointwiseParameter};

pub(crate) struct SelectorColorEditor {
    property: PointwiseParameter<ForegroundError>,
    picker: Entity<ColorPickerState>,
}

impl SelectorColorEditor {
    pub(in super::super) fn new(
        property: PointwiseParameter<ForegroundError>,
        window: &mut Window,
        cx: &mut App,
    ) -> Self {
        Self {
            property,
            picker: cx.new(|cx| ColorPickerState::new(window, cx)),
        }
    }

    pub(super) fn picker(&self) -> &Entity<ColorPickerState> {
        &self.picker
    }

    fn is_available(&self) -> bool {
        Self::can_edit_owned_color(&self.property)
    }

    pub(in super::super) fn can_edit_owned_color(
        property: &PointwiseParameter<ForegroundError>,
    ) -> bool {
        !Self::has_lab_link(property) && !Self::is_animated(property)
    }

    pub(in super::super) fn has_lab_link(property: &PointwiseParameter<ForegroundError>) -> bool {
        property
            .retained_adjustment()
            .is_some_and(|adjustment| adjustment.is_active().unwrap_or(true))
    }

    fn is_animated(property: &PointwiseParameter<ForegroundError>) -> bool {
        property.retained_animation().map_or(true, |animation| {
            animation.is_some_and(|animation| animation.is_active().unwrap_or(true))
        })
    }

    fn color(&self) -> Result<Hsla, String> {
        let [r, g, b] = self
            .property
            .color()
            .map_err(failure)?
            .components()
            .map(|value| value.clamp(0., 1.) as f32);
        Ok(Rgba { r, g, b, a: 1. }.into())
    }
}

impl GraphDocumentView {
    pub(super) fn render_selector_color_adjuster(
        &self,
        editor: &SelectorColorEditor,
        busy: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let property = editor.property.clone();
        let (linked, choices) = {
            let document = guard(&self.document);
            (
                document.color_adjuster(&property),
                document.available_color_adjusters(&property),
            )
        };
        let has_link = SelectorColorEditor::has_lab_link(&property);
        let label = match &linked {
            Some(link) => format!("Adjuster · {}", link.name()),
            None if has_link => "Missing Color adjuster".into(),
            None => "Link Color adjuster".into(),
        };
        let selection = self.selection.clone();
        let node = self.inspector.owner.clone();
        let revision = self.revision;
        self.render_color_adjuster_menu(
            ColorAdjusterMenu {
                id: "selector-color-adjuster",
                kind: AdjusterNameKind::Color,
                label,
                linked,
                choices,
                has_link,
                disabled: busy || (!has_link && SelectorColorEditor::is_animated(&property)),
                edit_linked: false,
            },
            move |view, link, window, cx| {
                if view.closed
                    || view.work.is_some()
                    || view.has_field_drafts(cx)
                    || !view.is_original_selection(&selection, &node, revision)
                {
                    return Err("The selected Color changed. Reopen its adjuster menu.".into());
                }
                if !matches!(link, ColorAdjusterLink::Unlink)
                    && SelectorColorEditor::is_animated(&property)
                {
                    return Err("Turn off Color animation to link an adjuster.".into());
                }
                view.commit_lab_adjuster_change(window, cx, |document| {
                    document.set_color_adjuster(&property, link)
                })
            },
            cx,
        )
    }

    pub(super) fn render_selector_color(
        &self,
        editor: &SelectorColorEditor,
        busy: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let disabled = busy || !editor.is_available() || self.has_field_drafts(cx);
        let picker = editor.picker.clone();
        let commit_picker = picker.clone();
        let owner = cx.weak_entity();
        let state = editor.picker.read(cx);
        let sliders = state.sliders();
        Popover::new("selector-color-picker")
            .open(state.is_open() && !disabled)
            .on_open_change(move |open, window, cx| {
                if !open {
                    picker.update(cx, |state, cx| state.set_open(false, cx));
                    return;
                }
                let _ = owner.update(cx, |view, cx| {
                    if view.is_busy() || view.has_field_drafts(cx) {
                        return;
                    }
                    if let Some(editor) = view.inspector.selector_color.as_ref()
                        && editor.picker == picker
                        && editor.is_available()
                    {
                        match editor.color() {
                            Ok(color) => picker.update(cx, |state, cx| {
                                state.set_value(color, window, cx);
                                state.set_open(true, cx);
                            }),
                            Err(error) => {
                                view.error = Some(error);
                                cx.notify();
                            }
                        }
                    }
                });
            })
            .trigger(
                Button::new("selector-pick-color")
                    .small()
                    .label("Pick color…")
                    .disabled(disabled),
            )
            .when(state.is_open() && !disabled, |popover| {
                popover.child(
                    div()
                        .v_flex()
                        .w_56()
                        .gap_2()
                        .child(
                            div()
                                .h_6()
                                .rounded(cx.theme().radius)
                                .bg(state.value().unwrap_or(cx.theme().background)),
                        )
                        .children(
                            [
                                ("Hue", "selector-picker-hue", sliders.hue()),
                                (
                                    "Saturation",
                                    "selector-picker-saturation",
                                    sliders.saturation(),
                                ),
                                (
                                    "Lightness",
                                    "selector-picker-lightness",
                                    sliders.lightness(),
                                ),
                            ]
                            .map(|(label, id, state)| {
                                Field::new().label(label).child(
                                    div()
                                        .id(id)
                                        .test_support()
                                        .w_full()
                                        .child(Slider::new(state).w_full()),
                                )
                            }),
                        )
                        .child(
                            Button::new("selector-picker-use")
                                .label("Use color")
                                .primary()
                                .on_click(cx.listener(move |view, _, window, cx| {
                                    view.use_selector_color(&commit_picker, window, cx);
                                })),
                        ),
                )
            })
    }

    fn use_selector_color(
        &mut self,
        picker: &Entity<ColorPickerState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.closed || self.is_busy() || self.has_field_drafts(cx) {
            return;
        }
        let Some(editor) = self
            .inspector
            .selector_color
            .as_ref()
            .filter(|editor| &editor.picker == picker && editor.is_available())
        else {
            return;
        };
        let Some(color) = picker.read(cx).value() else {
            return;
        };
        picker.update(cx, |state, cx| state.set_open(false, cx));
        let property = editor.property.clone();
        let current = match editor.color().and_then(|displayed| {
            if color == displayed {
                Ok(None)
            } else {
                property.color().map(Some).map_err(failure)
            }
        }) {
            Ok(Some(current)) => current,
            Ok(None) => return,
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        let Rgba { r, g, b, .. } = color.into();
        if let Some(history) = self.history() {
            history.separate_next_edit();
        }
        let stamp = self.begin_history_edit();
        self.error = property
            .set_color(current.with_components([r, g, b].map(f64::from)))
            .map_err(failure)
            .and_then(|()| self.inspector.sync_unchanged(window, cx))
            .err();
        self.finish_history_edit(stamp, self.error.is_none());
        self.revision += 1;
        self.status = None;
        self.retire_stale_preview();
        self.start_preview(cx);
        cx.notify();
    }
}
