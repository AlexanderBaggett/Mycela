//! Shared point fields and actions for retained and detached Custom curves.
use crate::curve_layer_menu::Layer;
use gpui_kit::base::{Disableable, StyledExt};
use gpui_kit::component::{
    Sizable,
    button::Button,
    checkbox::Checkbox,
    form::{Field, Form},
    input::{Input, InputState},
    menu::{DropdownMenu, PopupMenuItem},
};
use gpui_kit::{AnyElement, App, AppContext, Entity, IntoElement, Window, div, prelude::*};
use mycela_engine::{CurveLayerKind, CustomPointHandle, CustomPointList};

pub(crate) struct CustomDraft {
    points: CustomPointList,
    entries: Vec<CustomPointHandle>,
    selected: i32,
    fields: [Entity<InputState>; 2],
    synced: [String; 2],
}

#[derive(Clone)]
pub(crate) enum CustomAction {
    Add,
    Move,
    AddAt([f32; 2]),
    MoveTo([f32; 2]),
    Remove,
    Select(CustomPointHandle),
    Loop(bool, bool),
    Tangent(bool, i32, i32),
}

impl CustomDraft {
    pub(crate) fn new(
        layer: &Layer,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<Option<Self>, String> {
        if layer.kind() != CurveLayerKind::Custom {
            return Ok(None);
        }
        let points = layer.custom_points().map_err(|e| e.to_string())?;
        let entries = points.entries();
        let selected = layer.custom_selected_index().map_err(|e| e.to_string())?;
        let synced = point_text(&entries, selected);
        let fields = std::array::from_fn(|i| {
            cx.new(|cx| InputState::new(window, cx).default_value(synced[i].clone()))
        });
        Ok(Some(Self {
            points,
            entries,
            selected,
            fields,
            synced,
        }))
    }

    pub(crate) fn is_changed(&self, cx: &App) -> bool {
        self.fields
            .iter()
            .zip(&self.synced)
            .any(|(field, initial)| field.read(cx).value() != *initial)
    }

    pub(crate) fn discard_draft(&mut self, cx: &App) {
        self.synced = std::array::from_fn(|i| self.fields[i].read(cx).value().to_string());
    }

    pub(crate) fn points(&self) -> &[CustomPointHandle] {
        &self.entries
    }

    pub(crate) fn selected_point(&self) -> Option<&CustomPointHandle> {
        selected_point(&self.entries, self.selected)
    }

    fn validate(&self, layer: &Layer) -> Result<(), String> {
        let points = layer.custom_points().map_err(|e| e.to_string())?;
        let entries = points.entries();
        let selected = layer.custom_selected_index().map_err(|e| e.to_string())?;
        if !self.points.same_reference(&points)
            || self.selected != selected
            || self.entries.len() != entries.len()
            || !self
                .entries
                .iter()
                .zip(&entries)
                .all(|(left, right)| left.same_reference(right))
        {
            return Err("The custom points changed. Revert point edits before continuing.".into());
        }
        Ok(())
    }

    pub(crate) fn sync(
        &mut self,
        layer: &Layer,
        window: &mut Window,
        cx: &mut App,
        force: bool,
    ) -> Result<(), String> {
        if !force && self.is_changed(cx) {
            return self.validate(layer);
        }
        let points = layer.custom_points().map_err(|e| e.to_string())?;
        let entries = points.entries();
        let selected = layer.custom_selected_index().map_err(|e| e.to_string())?;
        self.synced = point_text(&entries, selected);
        for (field, value) in self.fields.iter().zip(&self.synced) {
            field.update(cx, |field, cx| field.set_value(value.clone(), window, cx));
        }
        self.points = points;
        self.entries = entries;
        self.selected = selected;
        Ok(())
    }

    pub(crate) fn apply(
        &mut self,
        layer: &Layer,
        action: CustomAction,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(), String> {
        self.validate(layer)?;
        if self.is_changed(cx) && matches!(action, CustomAction::Select(_) | CustomAction::Remove) {
            return Err("Use Add point or Move point before changing the selection.".into());
        }
        match action {
            CustomAction::Add
            | CustomAction::Move
            | CustomAction::AddAt(_)
            | CustomAction::MoveTo(_) => {
                let [x, y] = match action {
                    CustomAction::AddAt(position) | CustomAction::MoveTo(position) => position,
                    _ => {
                        let [x, y] = self
                            .fields
                            .each_ref()
                            .map(|field| field.read(cx).value().trim().parse::<f32>());
                        [
                            x.map_err(|_| "Enter a number for point X.")?,
                            y.map_err(|_| "Enter a number for point Y.")?,
                        ]
                    }
                };
                if !(0.0..=1.0).contains(&x) || !(-1000.0..=1000.0).contains(&y) {
                    return Err("Point X must be from 0 to 1 and Y from −1000 to 1000.".into());
                }
                if matches!(action, CustomAction::Add | CustomAction::AddAt(_)) {
                    layer.add_custom_point(x, y)
                } else {
                    if self.selected < 0 {
                        return Err("Select a point or choose Add point.".into());
                    }
                    layer.move_selected_custom_point(x, y)
                }
                .map_err(|e| e.to_string())?;
            }
            CustomAction::Remove => layer
                .remove_selected_custom_point()
                .map_err(|e| e.to_string())?,
            CustomAction::Select(point) => layer
                .select_custom_point_handle(&point)
                .map_err(|_| "The custom point changed. Open the menu again.")?,
            CustomAction::Tangent(incoming, expected, value) => {
                let point =
                    selected_point(&self.entries, self.selected).ok_or("Select a point first.")?;
                let current = if incoming {
                    point.incoming_tangent()
                } else {
                    point.outgoing_tangent()
                };
                if current != expected {
                    return Err("The point tangent changed. Try again.".into());
                }
                if incoming {
                    layer.set_custom_incoming_tangent(value)
                } else {
                    layer.set_custom_outgoing_tangent(value)
                }
                .map_err(|e| e.to_string())?;
                self.points = layer.custom_points().map_err(|e| e.to_string())?;
                self.entries = self.points.entries();
                return self.sync(layer, window, cx, false);
            }
            CustomAction::Loop(expected, value) => {
                if layer.is_custom_looped().map_err(|e| e.to_string())? != expected {
                    return Err("The curve loop changed. Try again.".into());
                }
                layer.set_custom_looped(value).map_err(|e| e.to_string())?;
                return self.sync(layer, window, cx, false);
            }
        }
        self.sync(layer, window, cx, true)
    }

    pub(crate) fn render(
        &self,
        id: &str,
        layer: &Layer,
        disabled: bool,
        on_action: impl Fn(CustomAction, &mut Window, &mut App) + Clone + 'static,
    ) -> AnyElement {
        let looped = layer.is_custom_looped().unwrap_or(false);
        let loop_action = on_action.clone();
        let actions = [
            ("add", "Add point", CustomAction::Add, disabled),
            (
                "move",
                "Move point",
                CustomAction::Move,
                disabled || self.selected < 0,
            ),
            (
                "remove",
                "Remove point",
                CustomAction::Remove,
                disabled || self.selected < 0,
            ),
        ];
        let selected = self.selected;
        let entries = self.entries.clone();
        let select_action = on_action.clone();
        let tangents = selected_point(&self.entries, selected)
            .map(|p| [p.incoming_tangent(), p.outgoing_tangent()]);
        div()
            .v_flex()
            .gap_2()
            .child(div().font_semibold().child("Custom points"))
            .child(
                div()
                    .h_flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        Button::new(format!("{id}-select"))
                            .small()
                            .label(if selected < 0 {
                                "Select point".into()
                            } else {
                                format!("Point {}", selected + 1)
                            })
                            .disabled(disabled || entries.is_empty())
                            .dropdown_menu(move |mut menu, _, _| {
                                for (index, point) in entries.iter().enumerate() {
                                    let [x, y] = point.position();
                                    let label = format!("Point {} · {x}, {y}", index + 1);
                                    let point = point.clone();
                                    let select_action = select_action.clone();
                                    menu = menu.item(
                                        PopupMenuItem::new(label)
                                            .checked(selected == index as i32)
                                            .on_click(move |_, window, cx| {
                                                select_action(
                                                    CustomAction::Select(point.clone()),
                                                    window,
                                                    cx,
                                                );
                                            }),
                                    );
                                }
                                menu.scrollable(true)
                            }),
                    )
                    .children(
                        actions
                            .into_iter()
                            .map(|(suffix, label, action, disabled)| {
                                let on_action = on_action.clone();
                                Button::new(format!("{id}-{suffix}"))
                                    .small()
                                    .label(label)
                                    .disabled(disabled)
                                    .on_click(move |_, window, cx| {
                                        on_action(action.clone(), window, cx)
                                    })
                            }),
                    ),
            )
            .child(Form::new().columns(2).children(
                [("x", "X"), ("y", "Y")].into_iter().zip(&self.fields).map(
                    |((suffix, label), field)| {
                        Field::new().label(label).child(
                            Input::new(field)
                                .id(format!("{id}-{suffix}"))
                                .aria_label(format!("Point {label}"))
                                .disabled(disabled),
                        )
                    },
                ),
            ))
            .child(
                Checkbox::new(format!("{id}-loop"))
                    .label("Loop custom curve")
                    .checked(looped)
                    .disabled(disabled)
                    .on_change(move |checked, window, cx| {
                        loop_action(CustomAction::Loop(looped, *checked), window, cx);
                    }),
            )
            .children(tangents.into_iter().flat_map(|values| {
                [
                    (true, "incoming-flat", "Incoming linear tangent"),
                    (false, "outgoing-flat", "Outgoing linear tangent"),
                ]
                .into_iter()
                .zip(values)
                .map(|((incoming, suffix, label), current)| {
                    let on_action = on_action.clone();
                    Checkbox::new(format!("{id}-{suffix}"))
                        .label(label)
                        .checked(current == 0)
                        .disabled(disabled)
                        .on_change(move |checked, window, cx| {
                            on_action(
                                CustomAction::Tangent(incoming, current, i32::from(!checked)),
                                window,
                                cx,
                            );
                        })
                })
            }))
            .into_any_element()
    }
}

fn selected_point(entries: &[CustomPointHandle], index: i32) -> Option<&CustomPointHandle> {
    usize::try_from(index)
        .ok()
        .and_then(|index| entries.get(index))
}

fn point_text(entries: &[CustomPointHandle], selected: i32) -> [String; 2] {
    selected_point(entries, selected)
        .map(CustomPointHandle::position)
        .unwrap_or([0.5, 50.])
        .map(|coordinate| coordinate.to_string())
}
