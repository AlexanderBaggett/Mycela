//! Ordered layer commands and native menus shared by the Gradient editors.
use crate::ForegroundError;
use gpui_kit::base::{Disableable, StyledExt};
use gpui_kit::component::{
    ActiveTheme,
    button::Button,
    checkbox::Checkbox,
    menu::{DropdownMenu, PopupMenuItem},
};
use gpui_kit::{App, IntoElement, Window, div, prelude::*};
use mycela_engine::{AnalyticCurveKind, CurveLayerHandle, SharedCurve};

pub(crate) type Layer = CurveLayerHandle<ForegroundError>;
#[derive(Clone)]
pub(crate) enum LayerAction {
    Add(AnalyticCurveKind),
    AddNoise,
    AddCustom,
    Edit(usize),
    Remove(usize),
    Move(usize, usize),
    Blend(usize, i32, i32),
    Wrap(bool, bool),
}
pub(crate) fn same_layers(a: &[Option<Layer>], b: &[Option<Layer>]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(a, b)| match (a, b) {
            (None, None) => true,
            (Some(a), Some(b)) => a.same_reference(b),
            _ => false,
        })
}
pub(crate) fn apply_layer_action(
    working_curve: &SharedCurve<ForegroundError>,
    action: LayerAction,
    after_remove: impl FnOnce(&[Option<Layer>]) -> Result<(), String>,
) -> Result<(), String> {
    let layers = working_curve.layers();
    match action {
        LayerAction::Edit(_) => unreachable!(),
        LayerAction::Add(kind) => working_curve
            .insert(0, Some(&Layer::new(kind)))
            .map_err(|e| e.to_string())?,
        LayerAction::AddNoise => working_curve
            .insert(0, Some(&Layer::new_noise().map_err(|e| e.to_string())?))
            .map_err(|e| e.to_string())?,
        LayerAction::AddCustom => working_curve
            .insert(0, Some(&Layer::new_custom()))
            .map_err(|e| e.to_string())?,
        LayerAction::Remove(index) => working_curve
            .remove(index as i32)
            .map_err(|e| e.to_string())?,
        LayerAction::Move(from, to) => {
            let layer = layers
                .get(from)
                .ok_or("The animation layer changed.")?
                .clone();
            working_curve
                .remove(from as i32)
                .map_err(|e| e.to_string())?;
            let mut remaining = layers;
            remaining.remove(from);
            after_remove(&remaining)?;
            working_curve
                .insert(to as i32, layer.as_ref())
                .map_err(|e| e.to_string())?;
        }
        LayerAction::Blend(index, expected, value) => {
            let layer = layers
                .get(index)
                .and_then(Option::as_ref)
                .ok_or("The animation layer is unavailable.")?;
            let control = layer
                .control(13)
                .map_err(|e| e.to_string())?
                .ok_or("The blend control is unavailable.")?;
            if control.enumeration().map_err(|e| e.to_string())? != expected {
                return Err("The layer blend changed. Open the menu again.".into());
            }
            control.set_enumeration(value).map_err(|e| e.to_string())?;
        }
        LayerAction::Wrap(expected, value) => {
            if working_curve.is_wrapped() != expected {
                return Err("The curve wrap changed. Try again.".into());
            }
            working_curve.set_wrapped(value);
        }
    }
    Ok(())
}

pub(crate) fn render_curve_layer_menu(
    id: &str,
    curve: &SharedCurve<ForegroundError>,
    layers: &[Option<Layer>],
    disabled: bool,
    cx: &App,
    on_action: impl Fn(LayerAction, &mut Window, &mut App) + Clone + 'static,
) -> gpui_kit::AnyElement {
    let description = if layers.is_empty() {
        "No layers".to_string()
    } else {
        layers
            .iter()
            .enumerate()
            .map(|(i, layer)| {
                format!(
                    "{}: {}",
                    i + 1,
                    layer
                        .as_ref()
                        .map(|v| format!("{:?}", v.kind()))
                        .unwrap_or_else(|| "Empty slot".into())
                )
            })
            .collect::<Vec<_>>()
            .join(" · ")
    };
    let callback = |action: LayerAction| {
        let on_action = on_action.clone();
        move |_: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut App| {
            on_action(action.clone(), window, cx);
        }
    };
    let mut additions: Vec<_> = [
        ("Sine", AnalyticCurveKind::Sine),
        ("Linear", AnalyticCurveKind::Linear),
        ("Arc", AnalyticCurveKind::Arc),
        ("Bell", AnalyticCurveKind::Bell),
        ("Gain", AnalyticCurveKind::Gain),
        ("Steps", AnalyticCurveKind::Steps),
    ]
    .into_iter()
    .map(|(name, kind)| (name, callback(LayerAction::Add(kind))))
    .collect();
    additions.push(("Noise", callback(LayerAction::AddNoise)));
    additions.push(("Custom", callback(LayerAction::AddCustom)));
    let mut edits = Vec::new();
    let mut removals = Vec::new();
    let mut moves = Vec::new();
    let mut blends = Vec::new();
    for (index, layer) in layers.iter().enumerate() {
        let label = layer
            .as_ref()
            .map(|v| format!("{:?}", v.kind()))
            .unwrap_or_else(|| "Empty slot".into());
        if layer.is_some() {
            edits.push((
                format!("Edit {}: {label}", index + 1),
                callback(LayerAction::Edit(index)),
            ));
        }
        removals.push((
            format!("Remove {}: {label}", index + 1),
            callback(LayerAction::Remove(index)),
        ));
        if index > 0 {
            moves.push((
                format!("Move {}: {label} up", index + 1),
                callback(LayerAction::Move(index, index - 1)),
            ));
        }
        if index + 1 < layers.len() {
            moves.push((
                format!("Move {}: {label} down", index + 1),
                callback(LayerAction::Move(index, index + 1)),
            ));
        }
        if let Some(layer) = layer
            && let Ok(Some(control)) = layer.control(13)
            && let Ok(old) = control.enumeration()
        {
            for (value, name) in [
                "Replace",
                "Add",
                "Multiply",
                "Scale from 0.5",
                "Scale from 1",
                "Minimum",
                "Maximum",
                "Compose",
            ]
            .iter()
            .enumerate()
            {
                blends.push((
                    format!("{}: {label} · {name}", index + 1),
                    old == value as i32,
                    callback(LayerAction::Blend(index, old, value as i32)),
                ));
            }
        }
    }
    let wrap = curve.is_wrapped();
    let wrap_action = on_action.clone();
    div()
        .v_flex()
        .gap_2()
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(description),
        )
        .child(
            Checkbox::new(format!("{id}:wrap"))
                .label("Wrap output")
                .checked(wrap)
                .disabled(disabled)
                .on_change(move |value, window, cx| {
                    wrap_action(LayerAction::Wrap(wrap, *value), window, cx);
                }),
        )
        .child(
            div()
                .h_flex()
                .flex_wrap()
                .gap_2()
                .child(
                    Button::new(format!("{id}:add-layer"))
                        .label("Add layer")
                        .disabled(disabled)
                        .dropdown_menu(move |mut menu, _, _| {
                            for (name, cb) in &additions {
                                menu = menu.item(PopupMenuItem::new(*name).on_click(cb.clone()));
                            }
                            menu
                        }),
                )
                .child(
                    Button::new(format!("{id}:edit-layer"))
                        .label("Edit layer")
                        .disabled(disabled || edits.is_empty())
                        .dropdown_menu(move |mut menu, _, _| {
                            for (name, cb) in &edits {
                                menu = menu
                                    .item(PopupMenuItem::new(name.clone()).on_click(cb.clone()));
                            }
                            menu.scrollable(true)
                        }),
                )
                .child(
                    Button::new(format!("{id}:remove-layer"))
                        .label("Remove layer")
                        .disabled(disabled || layers.is_empty())
                        .dropdown_menu(move |mut menu, _, _| {
                            for (name, cb) in &removals {
                                menu = menu
                                    .item(PopupMenuItem::new(name.clone()).on_click(cb.clone()));
                            }
                            menu.scrollable(true)
                        }),
                )
                .child(
                    Button::new(format!("{id}:move-layer"))
                        .label("Move layer")
                        .disabled(disabled || moves.is_empty())
                        .dropdown_menu(move |mut menu, _, _| {
                            for (name, cb) in &moves {
                                menu = menu
                                    .item(PopupMenuItem::new(name.clone()).on_click(cb.clone()));
                            }
                            menu.scrollable(true)
                        }),
                )
                .child(
                    Button::new(format!("{id}:blend-layer"))
                        .label("Layer blend")
                        .disabled(disabled || blends.is_empty())
                        .dropdown_menu(move |mut menu, _, _| {
                            for (name, selected, cb) in &blends {
                                menu = menu.item(
                                    PopupMenuItem::new(name.clone())
                                        .checked(*selected)
                                        .on_click(cb.clone()),
                                );
                            }
                            menu.scrollable(true)
                        }),
                ),
        )
        .into_any_element()
}
