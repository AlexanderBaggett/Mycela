//! Shared command, copy list, and transform controls for the Manual editor.
use gpui_kit::assets::IconName;
use gpui_kit::base::{
    StyledExt, TestSupportExt,
    actions::{SelectDown, SelectFirst, SelectLast, SelectUp},
};
use gpui_kit::component::{
    ActiveTheme, Disableable, Sizable,
    button::{Button, ButtonVariants},
    form::{Field, Form},
    input::{Input, InputEvent, InputState},
    list::ListItem,
    menu::{DropdownMenu, PopupMenuItem},
    scroll::ScrollableElement,
};
use gpui_kit::{
    App, AppContext, Context, Entity, FocusHandle, Focusable, InteractiveElement, IntoElement,
    KeyBinding, ParentElement, Role, ScrollStrategy, StatefulInteractiveElement, Styled,
    Subscription, UniformListScrollHandle, Window, div, prelude::FluentBuilder, rems, uniform_list,
};
use mycela_engine::{SynthesisPlacementField, SynthesisPlacementGeometry};
use std::{cell::Cell, rc::Rc};

const KEY_CONTEXT: &str = "SynthesisManualCopies";

gpui_kit::actions!(
    synthesis_manual,
    [
        CutPlacement,
        CopyPlacement,
        PastePlacement,
        DeletePlacement,
        RaisePlacement,
        LowerPlacement
    ]
);

#[derive(Clone, Copy)]
pub(super) enum ManualCommand {
    Select(usize),
    ChoosePatch(usize),
    NewCopy(usize),
    Delete(usize),
    MoveForward(usize),
    MoveBack(usize),
    Cut(usize),
    Copy(usize),
    Paste(Option<usize>),
}

pub(super) type ManualDispatch = Rc<dyn Fn(ManualCommand, &mut Window, &mut App)>;

pub(super) fn toolbar(
    inline: bool,
    patch_count: usize,
    source_patch: Option<usize>,
    disabled: bool,
    dispatch: ManualDispatch,
) -> impl IntoElement {
    let (picker_id, new_id) = if inline {
        ("synthesis-manual-use-patch", "synthesis-manual-new-copy")
    } else {
        (
            "synthesis-full-manual-use-patch",
            "synthesis-full-manual-new-copy",
        )
    };
    let patch = source_patch
        .filter(|&index| index < patch_count)
        .or_else(|| (patch_count > 0).then_some(0));
    let label = patch.map_or_else(|| "No patches".into(), patch_label);
    let picker = Button::new(picker_id)
        .small()
        .w(rems(6.))
        .accessibility_label(format!("Use patch · {label}"))
        .label(label)
        .disabled(disabled || patch.is_none());
    let choose_patch = dispatch.clone();
    let picker = if disabled || patch.is_none() {
        picker.into_any_element()
    } else {
        picker
            .dropdown_menu(move |menu, _, _| {
                (0..patch_count).fold(menu.scrollable(true), |menu, index| {
                    let active = patch == Some(index);
                    let dispatch = choose_patch.clone();
                    menu.item(
                        PopupMenuItem::new(patch_label(index))
                            .checked(active)
                            .on_click(move |_, window, cx| {
                                if !active {
                                    dispatch(ManualCommand::ChoosePatch(index), window, cx);
                                }
                            }),
                    )
                })
            })
            .into_any_element()
    };
    div()
        .h_flex()
        .flex_wrap()
        .gap_2()
        .child(
            div()
                .h_flex()
                .gap_1()
                .child(div().text_sm().child("Use patch"))
                .child(picker),
        )
        .child(
            Button::new(new_id)
                .small()
                .label("New copy")
                .disabled(disabled || patch.is_none())
                .on_click(move |_, window, cx| {
                    if let Some(patch) = patch {
                        dispatch(ManualCommand::NewCopy(patch), window, cx);
                    }
                }),
        )
}

pub(super) struct ManualCopyView<'a> {
    rows: &'a [(usize, SynthesisPlacementGeometry)],
    selected: Option<usize>,
    can_paste: bool,
    disabled: bool,
}

impl<'a> ManualCopyView<'a> {
    pub(super) fn new(
        rows: &'a [(usize, SynthesisPlacementGeometry)],
        selected: Option<usize>,
        can_paste: bool,
        disabled: bool,
    ) -> Self {
        Self {
            rows,
            selected,
            can_paste,
            disabled,
        }
    }
}

pub(super) struct ManualCopyList {
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
}

impl ManualCopyList {
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
            KeyBinding::new("secondary-x", CutPlacement, Some(KEY_CONTEXT)),
            KeyBinding::new("shift-delete", CutPlacement, Some(KEY_CONTEXT)),
            KeyBinding::new("secondary-c", CopyPlacement, Some(KEY_CONTEXT)),
            KeyBinding::new("secondary-v", PastePlacement, Some(KEY_CONTEXT)),
            KeyBinding::new("delete", DeletePlacement, Some(KEY_CONTEXT)),
            KeyBinding::new("alt-up", RaisePlacement, Some(KEY_CONTEXT)),
            KeyBinding::new("alt-down", LowerPlacement, Some(KEY_CONTEXT)),
        ]);
    }

    pub(super) fn focus(&self, window: &mut Window, cx: &mut App) {
        self.focus.focus(window, cx);
    }

    pub(super) fn reveal(&self, row: Option<usize>) {
        if let Some(row) = row {
            self.scroll.scroll_to_item(row, ScrollStrategy::Nearest);
        }
    }

    pub(super) fn render(
        &self,
        inline: bool,
        ManualCopyView {
            rows,
            selected,
            can_paste,
            disabled,
        }: ManualCopyView<'_>,
        dispatch: ManualDispatch,
        cx: &App,
    ) -> impl IntoElement {
        let (list_id, rows_id, row_id) = if inline {
            (
                "synthesis-manual-copies",
                "synthesis-manual-copy-rows",
                "synthesis-manual-copy",
            )
        } else {
            (
                "synthesis-full-manual-copies",
                "synthesis-full-manual-copy-rows",
                "synthesis-full-manual-copy",
            )
        };
        let count = rows.len();
        let selected_row =
            selected.and_then(|selected| rows.iter().position(|(index, _)| *index == selected));
        let selected = selected_row.and(selected);
        let cut = selected.map(ManualCommand::Cut);
        let copy = selected.map(ManualCommand::Copy);
        let paste = can_paste.then_some(ManualCommand::Paste(selected));
        let delete = selected.map(ManualCommand::Delete);
        let forward = selected
            .zip(selected_row)
            .filter(|(_, row)| *row > 0)
            .map(|(index, _)| ManualCommand::MoveForward(index));
        let back = selected
            .zip(selected_row)
            .filter(|(_, row)| row + 1 < count)
            .map(|(index, _)| ManualCommand::MoveBack(index));
        let invoke = |command: Option<ManualCommand>| {
            let dispatch = dispatch.clone();
            move |window: &mut Window, cx: &mut App| {
                if !disabled && let Some(command) = command {
                    dispatch(command, window, cx);
                }
            }
        };
        let select = |row: usize| {
            rows.get(row)
                .map(|(index, _)| *index)
                .filter(|&index| Some(index) != selected)
                .map(ManualCommand::Select)
        };
        let previous = invoke(select(selected_row.unwrap_or(0).saturating_sub(1)));
        let next = invoke(select(
            selected_row.map_or(0, |row| (row + 1).min(count.saturating_sub(1))),
        ));
        let first = invoke(select(0));
        let last = invoke(count.checked_sub(1).and_then(select));
        let cut_key = invoke(cut);
        let copy_key = invoke(copy);
        let paste_key = invoke(paste);
        let delete_key = invoke(delete);
        let forward_key = invoke(forward);
        let back_key = invoke(back);

        let button = |id, label, icon: Option<IconName>, command: Option<ManualCommand>| {
            let run = invoke(command);
            Button::new(format!("{list_id}-{id}"))
                .small()
                .ghost()
                .accessibility_label(label)
                .tooltip(label)
                .disabled(disabled || command.is_none())
                .map(|button| match icon {
                    Some(icon) => button.icon(icon),
                    None => button.label(label),
                })
                .on_click(move |_, window, cx| run(window, cx))
        };
        let clipboard_disabled = disabled || (selected.is_none() && !can_paste);
        let clipboard = Button::new(format!("{list_id}-clipboard"))
            .small()
            .ghost()
            .icon(IconName::Ellipsis)
            .accessibility_label("Clipboard actions")
            .tooltip("Clipboard actions")
            .disabled(clipboard_disabled);
        let clipboard = if clipboard_disabled {
            clipboard.into_any_element()
        } else {
            let dispatch = dispatch.clone();
            clipboard
                .dropdown_menu(move |menu, _, _| {
                    [("Cut", cut), ("Copy", copy), ("Paste", paste)]
                        .into_iter()
                        .fold(menu, |menu, (label, command)| {
                            let dispatch = dispatch.clone();
                            menu.item(
                                PopupMenuItem::new(label)
                                    .disabled(command.is_none())
                                    .on_click(move |_, window, cx| {
                                        if let Some(command) = command {
                                            dispatch(command, window, cx);
                                        }
                                    }),
                            )
                        })
                })
                .into_any_element()
        };
        let actions = div()
            .h_flex()
            .flex_wrap()
            .gap_0p5()
            .child(clipboard)
            .child(button("delete", "Delete", None, delete))
            .child(button(
                "forward",
                "Move placement toward the front",
                Some(IconName::ArrowUp),
                forward,
            ))
            .child(button(
                "back",
                "Move placement toward the back",
                Some(IconName::ArrowDown),
                back,
            ));
        let rows = rows.to_vec();
        let focus = self.focus.clone();
        let list = uniform_list(rows_id, count, move |range, _, _| {
            range
                .map(|row| {
                    let (index, geometry) = rows[row];
                    let active = selected == Some(index);
                    let label = format!(
                        "{}, Copy {}",
                        patch_label(geometry.patch()),
                        geometry.copy_number()
                    );
                    let focus = focus.clone();
                    let dispatch = dispatch.clone();
                    ListItem::new((row_id, geometry.identity()))
                        .role(Role::ListBoxOption)
                        .aria_selected(active)
                        .accessibility_label(label.clone())
                        .selected(active)
                        .disabled(disabled)
                        .w_full()
                        .h(rems(1.75))
                        .child(div().text_sm().truncate().child(label))
                        .on_click(move |_, window, cx| {
                            focus.focus(window, cx);
                            if !active {
                                dispatch(ManualCommand::Select(index), window, cx);
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
        let body = div()
            .id(list_id)
            .test_support()
            .role(Role::ListBox)
            .aria_label("Copies")
            .track_focus(&self.focus)
            .key_context(KEY_CONTEXT)
            .on_action(move |_: &SelectUp, window, cx| previous(window, cx))
            .on_action(move |_: &SelectDown, window, cx| next(window, cx))
            .on_action(move |_: &SelectFirst, window, cx| first(window, cx))
            .on_action(move |_: &SelectLast, window, cx| last(window, cx))
            .on_action(move |_: &CutPlacement, window, cx| cut_key(window, cx))
            .on_action(move |_: &CopyPlacement, window, cx| copy_key(window, cx))
            .on_action(move |_: &PastePlacement, window, cx| paste_key(window, cx))
            .on_action(move |_: &DeletePlacement, window, cx| delete_key(window, cx))
            .on_action(move |_: &RaisePlacement, window, cx| forward_key(window, cx))
            .on_action(move |_: &LowerPlacement, window, cx| back_key(window, cx))
            .relative()
            .flex_1()
            .min_h_0()
            .rounded_md()
            .border_1()
            .border_color(cx.theme().border)
            .focus_visible(move |style| style.border_color(ring))
            .when(count == 0, |body| {
                body.child(
                    div()
                        .px_3()
                        .py_1()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("No copies"),
                )
            })
            .when(count > 0, |body| {
                body.when(inline, |body| {
                    body.on_scroll_wheel(move |_, _, cx| {
                        if scroll.is_scrollable() {
                            cx.stop_propagation();
                        }
                    })
                })
                .child(list)
                .vertical_scrollbar(&scrollbar)
            });

        div()
            .v_flex()
            .min_w_0()
            .min_h_0()
            .gap_1()
            .w_full()
            .when(inline, |rail| rail.h(rems(11.)))
            .when(!inline, |rail| rail.h_full())
            .child(
                div()
                    .h_flex()
                    .flex_wrap()
                    .gap_2()
                    .justify_between()
                    .child(div().text_sm().font_semibold().child("Copies"))
                    .child(actions),
            )
            .child(body)
    }
}

fn patch_label(index: usize) -> String {
    format!("Patch {}", index + 1)
}

#[derive(Clone, Copy)]
struct DisplayedPlacement {
    geometry: SynthesisPlacementGeometry,
    raw_center: [bool; 2],
}

impl DisplayedPlacement {
    fn field(self, field: SynthesisPlacementField) -> f64 {
        use SynthesisPlacementField::*;
        let raw = match field {
            CenterX => self.raw_center[0],
            CenterY => self.raw_center[1],
            _ => false,
        };
        if raw {
            self.geometry.authored_field(field)
        } else {
            self.geometry.field(field)
        }
    }
}

pub(super) struct ManualNumbers {
    inputs: [(SynthesisPlacementField, Entity<InputState>); 5],
    displayed: Rc<Cell<Option<DisplayedPlacement>>>,
    _subscriptions: Vec<Subscription>,
}

impl ManualNumbers {
    pub(super) fn new<T: 'static>(
        value: Option<SynthesisPlacementGeometry>,
        dispatch: impl Fn(&mut T, SynthesisPlacementField, f64, &mut Window, &mut Context<T>) + 'static,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> Self {
        use SynthesisPlacementField::*;
        let value = value.map(|geometry| DisplayedPlacement {
            geometry,
            raw_center: [false; 2],
        });
        let inputs = [CenterX, CenterY, Width, Height, Angle].map(|field| {
            let [min, max] = field.limits();
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(field_text(value, field))
                    .min(min)
                    .max(max)
                    .step(0.01)
            });
            (field, input)
        });
        let dispatch = Rc::new(dispatch);
        let displayed = Rc::new(Cell::new(value));
        let subscriptions = inputs
            .iter()
            .map(|(field, input)| {
                let field = *field;
                let dispatch = dispatch.clone();
                let displayed = displayed.clone();
                cx.subscribe_in(input, window, move |editor, input, event, window, cx| {
                    if !matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                        return;
                    }
                    let Some(current) = displayed.get() else {
                        return;
                    };
                    if let Some(value) = changed_value(input.read(cx), current, field) {
                        dispatch(editor, field, value, window, cx);
                    } else {
                        let text = field_text(Some(current), field);
                        if input.read(cx).value().as_ref() != text {
                            input.update(cx, |input, cx| input.set_value(text, window, cx));
                        }
                    }
                })
            })
            .collect();
        Self {
            inputs,
            displayed,
            _subscriptions: subscriptions,
        }
    }

    pub(super) fn sync(
        &self,
        value: Option<SynthesisPlacementGeometry>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.sync_with_center_fields(value, [false; 2], window, cx);
    }

    pub(super) fn sync_with_center_fields(
        &self,
        value: Option<SynthesisPlacementGeometry>,
        raw_center: [bool; 2],
        window: &mut Window,
        cx: &mut App,
    ) {
        let value = value.map(|geometry| DisplayedPlacement {
            geometry,
            raw_center,
        });
        self.displayed.set(value);
        for (field, input) in &self.inputs {
            let text = field_text(value, *field);
            if input.read(cx).value().as_ref() != text {
                input.update(cx, |input, cx| input.set_value(text, window, cx));
            }
        }
    }

    pub(super) fn focused(
        &self,
        window: &Window,
        cx: &App,
    ) -> Option<(SynthesisPlacementField, Option<f64>)> {
        let current = self.displayed.get()?;
        self.inputs.iter().find_map(|&(field, ref input)| {
            let input = input.read(cx);
            input
                .focus_handle(cx)
                .is_focused(window)
                .then(|| (field, changed_value(input, current, field)))
        })
    }

    pub(super) fn render(&self, inline: bool, disabled: bool) -> Option<Form> {
        self.displayed.get()?;
        let id = if inline {
            "synthesis-manual-field"
        } else {
            "synthesis-full-manual-field"
        };
        Some(
            Form::new().columns(if inline { 2 } else { 5 }).children(
                self.inputs
                    .iter()
                    .enumerate()
                    .map(|(index, (field, input))| {
                        use SynthesisPlacementField::*;
                        let label = match field {
                            CenterX => "Center X (%)",
                            CenterY => "Center Y (%)",
                            Width => "Width (%)",
                            Height => "Height (%)",
                            Angle => "Angle (°)",
                        };
                        Field::new().label(label).child(
                            Input::new(input)
                                .id((id, index))
                                .small()
                                .aria_label(label)
                                .disabled(disabled),
                        )
                    }),
            ),
        )
    }
}

fn field_text(value: Option<DisplayedPlacement>, field: SynthesisPlacementField) -> String {
    value.map_or_else(String::new, |value| format!("{:.2}", value.field(field)))
}

fn changed_value(
    input: &InputState,
    current: DisplayedPlacement,
    field: SynthesisPlacementField,
) -> Option<f64> {
    input
        .value()
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|&value| value.is_finite() && value != current.field(field))
}
