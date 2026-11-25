//! Authoring for the coupled position and RGBA stops in Colorize G2.
use super::{GraphDocumentView, model::failure};
#[path = "graph_document_legacy_gradient_strip.rs"]
mod strip_view;
use crate::ForegroundError;
use gpui_kit::base::{Disableable, StyledExt, TestSupportExt};
use gpui_kit::component::{
    ActiveTheme, Sizable,
    button::Button,
    color_picker::ColorPickerState,
    form::Field,
    input::{Input, InputState},
    menu::{DropdownMenu, PopupMenuItem},
    popover::Popover,
    slider::Slider,
};
use gpui_kit::{
    App, AppContext, Context, Entity, FocusHandle, IntoElement, Rgba, Window, div, prelude::*,
};
use mycela_engine::{LegacyColorGradient, PointwiseParameter, round_decimal_places};
use std::sync::Arc;
use strip_view::StopDrag;

const STRIP_WIDTH: usize = 168;
const COMPONENTS: [&str; 5] = ["Position", "Red", "Green", "Blue", "Transparency"];
struct FieldDraft {
    input: Entity<InputState>,
    loaded: String,
}

impl FieldDraft {
    fn new(loaded: String, window: &mut Window, cx: &mut App) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).default_value(loaded.clone()));
        Self { input, loaded }
    }

    fn changed_text(&self, cx: &App) -> Option<gpui_kit::SharedString> {
        let text = self.input.read(cx).value();
        (text.as_ref() != self.loaded).then_some(text)
    }

    fn render(&self, id: impl Into<gpui_kit::ElementId>, label: &str, disabled: bool) -> Field {
        Field::new().label(label.to_owned()).child(
            Input::new(&self.input)
                .id(id)
                .aria_label(label.to_owned())
                .disabled(disabled),
        )
    }
}

pub(crate) struct LegacyGradientEditor {
    parameter: PointwiseParameter<ForegroundError>,
    baseline: LegacyColorGradient,
    working: LegacyColorGradient,
    steps: FieldDraft,
    new_position: FieldDraft,
    selected: Option<(usize, [FieldDraft; 5])>,
    requires_ordering: bool,
    strip: [u32; STRIP_WIDTH],
    focus: FocusHandle,
    drag: Option<StopDrag>,
    picker: Entity<ColorPickerState>,
}

pub(super) struct LegacyGradientEdit {
    parameter: PointwiseParameter<ForegroundError>,
    before: LegacyColorGradient,
    after: LegacyColorGradient,
}

impl LegacyGradientEdit {
    pub(super) fn commit(self) -> Result<bool, String> {
        self.parameter
            .edit_legacy_color_gradient(&self.before, self.after)
            .map_err(failure)
    }
}

#[derive(Clone, Copy)]
enum Action {
    Select(usize),
    Add,
    AddAt(f64),
    Remove,
    SetRgb([f64; 3]),
}

impl LegacyGradientEditor {
    pub(crate) fn new(
        parameter: PointwiseParameter<ForegroundError>,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<Self, String> {
        let baseline = parameter.legacy_color_gradient().map_err(failure)?;
        let mut editor = Self {
            parameter,
            working: baseline.clone(),
            steps: FieldDraft::new(baseline.steps().to_string(), window, cx),
            new_position: FieldDraft::new("0.5".into(), window, cx),
            selected: None,
            requires_ordering: false,
            strip: strip(&baseline),
            focus: cx.focus_handle(),
            drag: None,
            picker: cx.new(|cx| ColorPickerState::new(window, cx)),
            baseline,
        };
        editor.select(0, window, cx);
        Ok(editor)
    }

    fn select(&mut self, index: usize, window: &mut Window, cx: &mut App) {
        self.selected = self.working.stops().get(index).map(|stop| {
            (
                index,
                stop.map(|value| FieldDraft::new(value.to_string(), window, cx)),
            )
        });
    }

    pub(super) fn selection(&self, cx: &App) -> Option<(f64, FocusHandle)> {
        let (index, _) = self.selected.as_ref()?;
        let (gradient, _) = self.draft(cx).ok()?;
        let position = gradient.stops().get(*index)?[0];
        Some((position, self.focus.clone()))
    }

    pub(super) fn restore_selection(
        &mut self,
        position: f64,
        focus: FocusHandle,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.focus = focus;
        if let Some((index, _)) = self
            .working
            .stops()
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| (a[0] - position).abs().total_cmp(&(b[0] - position).abs()))
        {
            self.select(index, window, cx);
        }
    }

    pub(crate) fn inputs(&self) -> impl Iterator<Item = Entity<InputState>> + '_ {
        std::iter::once(self.steps.input.clone()).chain(
            self.selected
                .iter()
                .flat_map(|(_, fields)| fields.iter().map(|draft| draft.input.clone())),
        )
    }

    pub(super) fn picker(&self) -> &Entity<ColorPickerState> {
        &self.picker
    }

    pub(crate) fn is_changed(&self, cx: &App) -> bool {
        self.steps.changed_text(cx).is_some()
            || self.selected.as_ref().is_some_and(|(_, fields)| {
                fields.iter().any(|field| field.changed_text(cx).is_some())
            })
            || !self.working.same_values(&self.baseline)
    }

    fn draft(&self, cx: &App) -> Result<(LegacyColorGradient, bool), String> {
        let mut draft = self.working.clone();
        let mut requires_ordering = self.requires_ordering;
        if let Some(text) = self.steps.changed_text(cx) {
            let steps = text.trim().parse().unwrap_or(0);
            if !draft.set_steps(steps) {
                return Err("Enter a whole number from 1 to 10,000 for Steps.".into());
            }
        }
        if let Some((index, fields)) = &self.selected {
            for (component, (label, field)) in COMPONENTS.iter().zip(fields).enumerate() {
                let Some(text) = field.changed_text(cx) else {
                    continue;
                };
                let raw = text.trim().parse().unwrap_or(f64::NAN);
                let range = if component == 0 {
                    0.0..=1.0
                } else {
                    -100.0..=100.0
                };
                let value = if component == 0 {
                    raw
                } else {
                    round_decimal_places(raw, 3).map_err(failure)?
                };
                if !range.contains(&raw) || !draft.set_stop_component(*index, component, value) {
                    let range = if component == 0 {
                        "0 to 1"
                    } else {
                        "−100 to 100"
                    };
                    return Err(format!("Enter a number from {range} for {label}."));
                }
                requires_ordering |=
                    value.to_bits() != self.working.stops()[*index][component].to_bits();
            }
        }
        Ok((draft, requires_ordering))
    }

    pub(super) fn stage(&self, cx: &App) -> Result<Option<LegacyGradientEdit>, String> {
        if !self.is_changed(cx) {
            return Ok(None);
        }
        let (mut after, requires_ordering) = self.draft(cx)?;
        if requires_ordering {
            after.sort_stops();
        }
        if self.baseline.same_values(&after) {
            return Ok(None);
        }
        Ok(Some(LegacyGradientEdit {
            parameter: self.parameter.clone(),
            before: self.baseline.clone(),
            after,
        }))
    }

    fn action(&mut self, action: Action, window: &mut Window, cx: &mut App) -> Result<(), String> {
        let (mut draft, mut requires_ordering) = self.draft(cx)?;
        let index = match action {
            Action::Select(index) => index,
            Action::Add | Action::AddAt(_) => {
                let position = match action {
                    Action::AddAt(position) => position,
                    _ => self
                        .new_position
                        .input
                        .read(cx)
                        .value()
                        .trim()
                        .parse()
                        .unwrap_or(f64::NAN),
                };
                if !(0.0..=1.0).contains(&position) {
                    return Err("Enter a new stop position from 0 to 1.".into());
                }
                requires_ordering = true;
                draft
                    .add_sampled_stop(position, STRIP_WIDTH)
                    .ok_or("The gradient could not be sampled at that stop position.")?
            }
            Action::Remove => {
                let (index, _) = self.selected.as_ref().ok_or("Choose a stop to remove.")?;
                if !draft.remove_stop(*index) {
                    return Err(
                        "The selected stop changed. Revert fields before continuing.".into(),
                    );
                }
                (*index).min(draft.stops().len().saturating_sub(1))
            }
            Action::SetRgb(rgb) => {
                let index = self.selected.as_ref().ok_or("Choose a color stop.")?.0;
                for (channel, value) in rgb.into_iter().enumerate() {
                    let rounded = round_decimal_places(value, 3).map_err(failure)?;
                    if !draft.set_stop_component(index, channel + 1, rounded) {
                        return Err(
                            "The selected stop changed. Revert fields before continuing.".into(),
                        );
                    }
                }
                requires_ordering = true;
                index
            }
        };
        self.strip = strip(&draft);
        self.working = draft;
        self.steps.loaded = self.steps.input.read(cx).value().to_string();
        self.requires_ordering = requires_ordering;
        self.select(index, window, cx);
        Ok(())
    }

    fn set_picker_open(&self, open: bool, window: &mut Window, cx: &mut App) -> Result<(), String> {
        if !open {
            self.picker
                .update(cx, |picker, cx| picker.set_open(false, cx));
            return Ok(());
        }
        let (draft, _) = self.draft(cx)?;
        let index = self.selected.as_ref().ok_or("Choose a color stop.")?.0;
        let [_, r, g, b, _] = draft.stops()[index];
        let [r, g, b] = [r, g, b].map(|value| value.clamp(0., 1.) as f32);
        self.picker.update(cx, |picker, cx| {
            picker.set_value(Rgba { r, g, b, a: 1. }, window, cx);
            picker.set_open(true, cx);
        });
        Ok(())
    }

    fn use_picked_rgb(&mut self, window: &mut Window, cx: &mut App) -> Result<(), String> {
        if let Some(color) = self.picker.read(cx).value() {
            let Rgba { r, g, b, .. } = color.into();
            self.action(Action::SetRgb([r, g, b].map(f64::from)), window, cx)?;
        }
        self.set_picker_open(false, window, cx)
    }
}

fn strip(value: &LegacyColorGradient) -> [u32; STRIP_WIDTH] {
    let mut ordered = value.clone();
    ordered.sort_stops();
    std::array::from_fn(|column| {
        let [red, green, blue, transparency] = ordered.strip_sample(column, STRIP_WIDTH);
        u32::from_be_bytes(
            [red, green, blue, 1.0 - transparency]
                .map(|value| (value.clamp(0.0, 1.0) * 255.0).round() as u8),
        )
    })
}

impl GraphDocumentView {
    fn update_legacy_gradient(
        &mut self,
        focus: &FocusHandle,
        update: impl FnOnce(&mut LegacyGradientEditor, &mut Window, &mut App) -> Result<(), String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.closed || self.is_busy() {
            return;
        }
        if let Some(editor) = self
            .inspector
            .legacy_gradient
            .as_mut()
            .filter(|editor| &editor.focus == focus)
        {
            self.error = update(editor, window, cx).err();
            if self.error.is_none() && editor.drag.is_none() && editor.is_changed(cx) {
                self.commit_staged_fields(window, cx);
            }
            self.observe_drafts(cx);
            cx.notify();
        }
    }

    fn render_legacy_picker(
        &self,
        editor: &LegacyGradientEditor,
        disabled: bool,
        cx: &Context<Self>,
    ) -> Popover {
        let open_owner = cx.weak_entity();
        let focus = editor.focus.clone();
        let use_focus = focus.clone();
        let picker = editor.picker.read(cx);
        let sliders = picker.sliders();
        let color = picker.value().unwrap_or(cx.theme().background);
        Popover::new("graph-legacy-rgb-picker")
            .open(picker.is_open())
            .on_open_change(move |open, window, cx| {
                let _ = open_owner.update(cx, |view, cx| {
                    view.update_legacy_gradient(
                        &focus,
                        |editor, window, cx| editor.set_picker_open(*open, window, cx),
                        window,
                        cx,
                    );
                });
            })
            .trigger(
                Button::new("graph-legacy-pick-color")
                    .small()
                    .label("Pick RGB color")
                    .disabled(disabled),
            )
            .when(picker.is_open(), |popover| {
                popover.child(
                    div()
                        .v_flex()
                        .w_56()
                        .gap_2()
                        .child(div().h_6().rounded(cx.theme().radius).bg(color))
                        .children(
                            [
                                ("Hue", "graph-legacy-picker-hue", sliders.hue()),
                                (
                                    "Saturation",
                                    "graph-legacy-picker-saturation",
                                    sliders.saturation(),
                                ),
                                (
                                    "Lightness",
                                    "graph-legacy-picker-lightness",
                                    sliders.lightness(),
                                ),
                            ]
                            .map(|(label, id, state)| {
                                Field::new().label(label).child(
                                    div()
                                        .id(id)
                                        .test_support()
                                        .w_full()
                                        .child(Slider::new(state).w_full().disabled(disabled)),
                                )
                            }),
                        )
                        .child(
                            Button::new("graph-legacy-picker-use")
                                .label("Use color")
                                .disabled(disabled)
                                .on_click(cx.listener(move |view, _, window, cx| {
                                    view.update_legacy_gradient(
                                        &use_focus,
                                        LegacyGradientEditor::use_picked_rgb,
                                        window,
                                        cx,
                                    );
                                })),
                        ),
                )
            })
    }

    pub(super) fn render_legacy_gradient(
        &self,
        editor: &LegacyGradientEditor,
        disabled: bool,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let focus = editor.focus.clone();
        let owner = cx.weak_entity();
        let callback = move |action| {
            let (focus, owner) = (focus.clone(), owner.clone());
            move |_: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut App| {
                let _ = owner.update(cx, |view, cx| {
                    view.update_legacy_gradient(
                        &focus,
                        |editor, window, cx| editor.action(action, window, cx),
                        window,
                        cx,
                    );
                });
            }
        };
        let positions: Arc<[_]> = editor.working.stops().iter().map(|stop| stop[0]).collect();
        let selected_label = editor.selected.as_ref().map_or_else(
            || "No stops".to_owned(),
            |(index, _)| format!("Stop {} · {}", index + 1, positions[*index]),
        );
        let select = callback.clone();
        div()
            .v_flex()
            .gap_2()
            .child(div().font_semibold().child("Color Gradient"))
            .child(self.render_legacy_strip(editor, positions.clone(), disabled, cx))
            .child(
                editor
                    .steps
                    .render("graph-legacy-gradient-steps", "Steps", disabled),
            )
            .child(
                div()
                    .h_flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        Button::new("graph-legacy-gradient-select")
                            .small()
                            .label(selected_label)
                            .disabled(disabled || positions.is_empty())
                            .dropdown_menu(move |mut menu, _, _| {
                                for (index, position) in positions.iter().enumerate() {
                                    menu = menu.item(
                                        PopupMenuItem::new(format!(
                                            "Stop {} · {position}",
                                            index + 1
                                        ))
                                        .on_click(select(Action::Select(index))),
                                    );
                                }
                                menu.scrollable(true)
                            }),
                    )
                    .child(
                        Button::new("graph-legacy-gradient-remove")
                            .small()
                            .label("Remove stop")
                            .disabled(disabled || editor.selected.is_none())
                            .on_click(callback(Action::Remove)),
                    ),
            )
            .when_some(editor.selected.as_ref(), |group, (_, fields)| {
                group
                    .child(self.render_legacy_picker(editor, disabled, cx))
                    .children(COMPONENTS.iter().zip(fields).map(|(label, field)| {
                        field.render(
                            format!("graph-legacy-gradient-{}", label.to_ascii_lowercase()),
                            label,
                            disabled,
                        )
                    }))
            })
            .child(editor.new_position.render(
                "graph-legacy-gradient-new-position",
                "New stop position",
                disabled,
            ))
            .child(
                Button::new("graph-legacy-gradient-add")
                    .small()
                    .label("Add stop")
                    .disabled(disabled)
                    .on_click(callback(Action::Add)),
            )
            .into_any_element()
    }
}
