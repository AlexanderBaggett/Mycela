//! Direct selection of the contours attached to one Patches source layer.
use super::{SynthesisEdit, SynthesisGeometry, synthesis_menu};
use gpui_kit::base::{
    StyledExt, TestSupportExt,
    actions::{SelectDown, SelectFirst, SelectLast, SelectUp},
};
use gpui_kit::component::{
    ActiveTheme, Sizable, button::Button, list::ListItem, menu::PopupMenuItem,
    scroll::ScrollableElement,
};
use gpui_kit::{
    App, FocusHandle, InteractiveElement, IntoElement, KeyBinding, ParentElement, Role,
    ScrollStrategy, StatefulInteractiveElement, Styled, UniformListScrollHandle, Window, div,
    prelude::*, rems, uniform_list,
};
use mycela_engine::SynthesisNumberField::PatchFrequency;
use std::rc::Rc;

const KEY_CONTEXT: &str = "SynthesisPatchList";

pub(super) struct PatchListView<'a> {
    pub(super) inline: bool,
    pub(super) edit: &'a SynthesisEdit,
    pub(super) layer: usize,
    pub(super) selection: Option<SynthesisGeometry>,
    pub(super) disabled: bool,
}

#[derive(Clone)]
pub(super) struct PatchList {
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
}

impl PatchList {
    pub(super) fn new(cx: &App) -> Self {
        Self {
            focus: cx.focus_handle().tab_stop(true),
            scroll: UniformListScrollHandle::new(),
        }
    }

    pub(super) fn bind_keys(cx: &mut App) {
        cx.bind_keys([
            KeyBinding::new("up", SelectUp, Some(KEY_CONTEXT)),
            KeyBinding::new("down", SelectDown, Some(KEY_CONTEXT)),
            KeyBinding::new("home", SelectFirst, Some(KEY_CONTEXT)),
            KeyBinding::new("end", SelectLast, Some(KEY_CONTEXT)),
        ]);
    }

    pub(super) fn selected(geometry: Option<SynthesisGeometry>) -> Option<usize> {
        match geometry {
            Some(SynthesisGeometry::Patch { selection, .. }) => Some(selection),
            _ => None,
        }
    }

    pub(super) fn reveal(&self, selected: Option<usize>) {
        if let Some(index) = selected {
            self.scroll.scroll_to_item(index, ScrollStrategy::Nearest);
        }
    }

    pub(super) fn focus(&self, window: &mut Window, cx: &mut App) {
        self.focus.focus(window, cx);
    }

    pub(super) fn compact_picker(
        PatchListView {
            edit,
            layer,
            selection,
            disabled,
            ..
        }: PatchListView<'_>,
        select: impl Fn(usize, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        let selected = Self::selected(selection);
        let count = edit.patch_counts(layer).map_or(0, |(patches, _)| patches);
        let (label, accessibility_label) = if let Some(index) = selected {
            let (patch, frequency) = patch_labels(edit, layer, index);
            let accessibility_label = format!("Select patch. {patch}, Frequency {frequency}");
            (patch, accessibility_label)
        } else {
            let label = if count == 0 {
                "No patches"
            } else {
                "Choose patch…"
            };
            (label.into(), label.into())
        };
        let button = Button::new("synthesis-patch-picker")
            .small()
            .w(rems(6.))
            .label(label)
            .accessibility_label(accessibility_label);
        let edit = edit.clone();
        let select = Rc::new(select);
        synthesis_menu(button, disabled || count == 0, move |menu, _, _| {
            (0..count).fold(menu.scrollable(true), |menu, index| {
                let (patch, frequency) = patch_labels(&edit, layer, index);
                let active = selected == Some(index);
                // PopupMenu's checkmark is visual. Include selection in its label.
                let status = if active { " (selected)" } else { "" };
                let select = select.clone();
                menu.item(
                    PopupMenuItem::new(format!("{patch} · {frequency}{status}"))
                        .checked(active)
                        .on_click(move |_, window, cx| {
                            if !active {
                                select(index, window, cx);
                            }
                        }),
                )
            })
        })
    }

    pub(super) fn render(
        &self,
        PatchListView {
            inline,
            edit,
            layer,
            selection,
            disabled,
        }: PatchListView<'_>,
        select: impl Fn(usize, &mut Window, &mut App) + 'static,
        cx: &App,
    ) -> impl IntoElement {
        let selected = Self::selected(selection);
        let (id, rows_id, row_id) = if inline {
            (
                "synthesis-patch-list",
                "synthesis-patch-list-rows",
                "synthesis-patch-row",
            )
        } else {
            (
                "synthesis-full-patch-list",
                "synthesis-full-patch-list-rows",
                "synthesis-full-patch-row",
            )
        };
        let count = edit.patch_counts(layer).map_or(0, |(patches, _)| patches);
        let edit = edit.clone();
        let select = Rc::new(select);
        let focus = self.focus.clone();
        let row_select = select.clone();
        let select_index = move |index: Option<usize>| {
            let select = select.clone();
            move |window: &mut Window, cx: &mut App| {
                if let Some(index) = index.filter(|&index| !disabled && index < count)
                    && Some(index) != selected
                {
                    select(index, window, cx);
                }
            }
        };
        let previous = select_index(Some(selected.unwrap_or(0).saturating_sub(1)));
        let next = select_index(Some(
            selected.map_or(0, |index| (index + 1).min(count.saturating_sub(1))),
        ));
        let first = select_index(Some(0));
        let last = select_index(count.checked_sub(1));
        let rows = uniform_list(rows_id, count, move |range, _, _| {
            range
                .map(|index| {
                    let active = selected == Some(index);
                    let (patch, frequency) = patch_labels(&edit, layer, index);
                    let select = row_select.clone();
                    let focus = focus.clone();
                    ListItem::new((row_id, index))
                        .selected(active)
                        .disabled(disabled)
                        .accessibility_label(format!("{patch}, Frequency {frequency}"))
                        .role(Role::ListBoxOption)
                        .aria_selected(active)
                        .w_full()
                        .h(rems(1.75))
                        .child(
                            div()
                                .h_flex()
                                .w_full()
                                .gap_2()
                                .justify_between()
                                .text_sm()
                                .child(patch)
                                .child(frequency),
                        )
                        .on_click(move |_, window, cx| {
                            focus.focus(window, cx);
                            if !active {
                                select(index, window, cx);
                            }
                        })
                })
                .collect()
        })
        .track_scroll(&self.scroll)
        .size_full()
        .min_h_0()
        .test_support();
        let scroll = self.scroll.clone();
        let scrollbar = scroll.0.borrow().base_handle.clone();
        let ring = cx.theme().ring;

        div()
            .id(id)
            .test_support()
            .role(Role::ListBox)
            .aria_label("Patches")
            .track_focus(&self.focus)
            .key_context(KEY_CONTEXT)
            .on_action(move |_: &SelectUp, window, cx| previous(window, cx))
            .on_action(move |_: &SelectDown, window, cx| next(window, cx))
            .on_action(move |_: &SelectFirst, window, cx| first(window, cx))
            .on_action(move |_: &SelectLast, window, cx| last(window, cx))
            .v_flex()
            .min_h_0()
            .min_w_0()
            .size_full()
            .rounded_md()
            .border_1()
            .border_color(cx.theme().border)
            .focus_visible(move |style| style.border_color(ring))
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .justify_between()
                    .px_3()
                    .py_1()
                    .text_sm()
                    .child("Patches")
                    .child(
                        div()
                            .id("patch-list-frequency-heading")
                            .role(Role::Label)
                            .aria_label("Frequency")
                            .child(if inline { "Freq." } else { "Frequency" }),
                    ),
            )
            .when(count == 0, |list| {
                list.child(div().px_3().py_1().text_sm().child("No patches"))
            })
            .when(count > 0, |list| {
                list.child(
                    div()
                        .relative()
                        .min_h_0()
                        .flex_1()
                        .when(inline, |body| {
                            body.on_scroll_wheel(move |_, _, cx| {
                                // The list consumes motion before this wrapper sees the event.
                                if scroll.is_scrollable() {
                                    cx.stop_propagation();
                                }
                            })
                        })
                        .child(rows)
                        .vertical_scrollbar(&scrollbar),
                )
            })
    }
}

fn patch_labels(edit: &SynthesisEdit, layer: usize, index: usize) -> (String, String) {
    let patch = format!("Patch {}", index + 1);
    let frequency = edit
        .number_value(Some(layer), PatchFrequency(index))
        .map_or_else(|| "—".into(), |value| format!("{value:.0}%"));
    (patch, frequency)
}
