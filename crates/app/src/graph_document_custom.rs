//! Custom curve points bound to their retained list and selected point.
use super::super::inspector::LayerSelection;
use super::{GraphDocumentView, animation::AnimationTarget};
use crate::ForegroundError;
use gpui_kit::base::{Disableable, StyledExt};
use gpui_kit::component::{
    button::Button,
    checkbox::Checkbox,
    form::{Field, Form},
    input::{Input, InputState},
    menu::{DropdownMenu, PopupMenuItem},
};
use gpui_kit::{App, AppContext, Context, Entity, IntoElement, Window, div, prelude::*};
use mycela_engine::{CurveLayerHandle, CurveLayerKind, CustomPointHandle, CustomPointList};

type Layer = CurveLayerHandle<ForegroundError>;
#[derive(Clone)]
pub(crate) struct CustomDraft {
    points: CustomPointList,
    entries: Vec<CustomPointHandle>,
    positions: Vec<[u32; 2]>,
    selected: i32,
    pub fields: [Entity<InputState>; 2],
    synced: [String; 2],
}
impl CustomDraft {
    pub fn new(layer: &Layer, window: &mut Window, cx: &mut App) -> Result<Option<Self>, String> {
        if layer.kind() != CurveLayerKind::Custom {
            return Ok(None);
        }
        let points = layer.custom_points().map_err(|e| e.to_string())?;
        let entries = points.entries();
        let selected = layer.custom_selected_index().map_err(|e| e.to_string())?;
        let values = point(&entries, selected)
            .map(|p| p.position())
            .unwrap_or([0.5, 50.0]);
        let synced = values.map(|v| v.to_string());
        let fields = std::array::from_fn(|i| {
            cx.new(|cx| InputState::new(window, cx).default_value(synced[i].clone()))
        });
        let positions = entries
            .iter()
            .map(|p| p.position().map(f32::to_bits))
            .collect();
        Ok(Some(Self {
            points,
            entries,
            positions,
            selected,
            fields,
            synced,
        }))
    }
    pub fn is_changed(&self, cx: &App) -> bool {
        self.fields
            .iter()
            .zip(&self.synced)
            .any(|(field, value)| field.read(cx).value() != *value)
    }
    pub(crate) fn validate(&self, layer: &Layer) -> Result<(), String> {
        let points = layer.custom_points().map_err(|e| e.to_string())?;
        let entries = points.entries();
        if !self.points.same_reference(&points)
            || self.selected != layer.custom_selected_index().map_err(|e| e.to_string())?
            || entries.len() != self.entries.len()
            || !entries
                .iter()
                .zip(&self.entries)
                .zip(&self.positions)
                .all(|((a, b), p)| a.same_reference(b) && a.position().map(f32::to_bits) == *p)
        {
            return Err("The custom points changed. Revert point edits before continuing.".into());
        }
        Ok(())
    }
}
fn point(entries: &[CustomPointHandle], selected: i32) -> Option<&CustomPointHandle> {
    usize::try_from(selected)
        .ok()
        .and_then(|index| entries.get(index))
}
fn select_point(layer: &Layer, index: usize) -> Result<(), String> {
    let points = layer.custom_points().map_err(|error| error.to_string())?;
    let entries = points.entries();
    let point = entries.get(index).ok_or("The custom point changed.")?;
    layer
        .select_custom_point_handle(point)
        .map_err(|error| error.to_string())
}
#[derive(Clone)]
pub(crate) enum CustomAction {
    Add,
    Move,
    Remove,
    Select(usize),
    Plot(PlotAction),
    Loop(bool, bool),
    Tangent(bool, i32, i32),
}
// The pointer gesture records its starting point. Add first, then move, so a
// newly created point inherits the tangents at its original insertion position.
#[derive(Clone, Copy)]
pub(crate) enum PlotAction {
    Add {
        start: [f32; 2],
        end: Option<[f32; 2]>,
    },
    Move {
        index: usize,
        position: [f32; 2],
    },
    Remove(usize),
}
impl PlotAction {
    pub(super) fn apply(self, layer: &Layer) -> Result<(), String> {
        let result = match self {
            Self::Add { start: [x, y], end } => {
                layer
                    .add_custom_point(x, y)
                    .map_err(|error| error.to_string())?;
                match end {
                    Some([x, y]) => layer.move_selected_custom_point(x, y),
                    None => return Ok(()),
                }
            }
            Self::Move {
                index,
                position: [x, y],
            } => {
                select_point(layer, index)?;
                layer.move_selected_custom_point(x, y)
            }
            Self::Remove(index) => {
                select_point(layer, index)?;
                layer.remove_selected_custom_point()
            }
        };
        result.map_err(|error| error.to_string())
    }
}
impl GraphDocumentView {
    pub(super) fn graph_custom_action(
        &mut self,
        selection: &LayerSelection,
        target: &AnimationTarget,
        expected: &CustomDraft,
        action: CustomAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.closed
            || self.is_busy()
            || !self.is_original_selection(&target.selection, &target.node, target.revision)
        {
            return;
        }
        let valid = (|| -> Result<(), String> {
            if self.inspector.fields_changed(cx) {
                return Err("Correct or reset field text before editing points.".into());
            }
            let current = self
                .inspector
                .layer
                .as_ref()
                .ok_or("Choose a custom layer first.")?;
            if !current.same_path(selection) {
                return Err("The selected layer changed. Choose it again.".into());
            }
            selection.validate(None)?;
            expected.validate(&selection.layer)?;
            let draft = self
                .inspector
                .custom
                .as_ref()
                .ok_or("Custom points are unavailable.")?;
            if !draft.points.same_reference(&expected.points) {
                return Err("The custom points changed. Choose the layer again.".into());
            }
            if !matches!(action, CustomAction::Add | CustomAction::Move) && draft.is_changed(cx) {
                return Err("Add or move the point, or revert point edits first.".into());
            }
            if matches!(action, CustomAction::Add | CustomAction::Move) {
                let mut values = [0.0; 2];
                for (index, value) in values.iter_mut().enumerate() {
                    *value = draft.fields[index]
                        .read(cx)
                        .value()
                        .parse::<f32>()
                        .map_err(|_| "Point coordinates need numbers.")?;
                    if !value.is_finite() {
                        return Err("Point coordinates must be finite.".into());
                    }
                }
                if !(0.0..=1.0).contains(&values[0]) || !(-1000.0..=1000.0).contains(&values[1]) {
                    return Err("Point X must be from 0 to 1 and Y from -1000 to 1000.".into());
                }
                if matches!(action, CustomAction::Move) && draft.selected < 0 {
                    return Err("Select a point first.".into());
                }
            }
            Ok(())
        })();
        if let Err(error) = valid {
            self.error = Some(error);
            cx.notify();
            return;
        }
        if matches!(action, CustomAction::Remove) && expected.selected < 0 {
            return;
        }
        let changes_document = !matches!(action, CustomAction::Select(_));
        if changes_document && selection.parameter.records_curve_history() {
            self.ensure_history_binding();
        }
        let stamp = changes_document.then(|| self.begin_history_edit());
        if changes_document {
            self.revision += 1;
            self.status = None;
        }
        let revision = self.revision;
        let result = (|| -> Result<(), String> {
            let edit = if changes_document {
                selection
                    .parameter
                    .animation_edit(Some(&selection.animation))?
            } else {
                None
            };
            let copy = edit
                .as_ref()
                .map(|edit| selection.on_animation(&edit.owner))
                .transpose()?;
            let layer = copy.as_ref().map_or(&selection.layer, |copy| &copy.layer);
            expected.apply(layer, action, copy.is_some(), cx)?;
            if let Some(edit) = edit {
                edit.publish(Some(&self.document))?;
                self.inspector.layer = copy;
            } else {
                selection.validate(None)?;
            }
            Ok(())
        })();
        if !self.is_original_selection(&target.selection, &target.node, revision) {
            self.error = Some(result.err().unwrap_or_else(|| {
                "The selected node changed while editing. Earlier changes were kept.".into()
            }));
            self.retire_stale_preview();
            cx.notify();
            return;
        }
        if let Err(error) = result {
            self.error = Some(error);
            if changes_document {
                self.start_preview(cx);
            }
            cx.notify();
            return;
        }
        self.error = None;
        if let Some(stamp) = stamp {
            self.finish_history_edit(stamp, true);
        }
        self.rebuild(window, cx);
        if changes_document {
            self.start_preview(cx);
        }
    }
    pub(super) fn render_graph_custom(
        &self,
        selection: &LayerSelection,
        draft: &CustomDraft,
        target: &AnimationTarget,
        disabled: bool,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let owner = cx.entity().downgrade();
        let plot = selection
            .parameter
            .records_curve_history()
            .then(|| self.render_custom_curve_plot(selection, draft, target, disabled, cx));
        render_custom_points("graph-custom", &selection.layer, draft, plot, disabled, {
            let selection = selection.clone();
            let target = target.clone();
            let draft = draft.clone();
            move |action, window, cx| {
                let _ = owner.update(cx, |view, cx| {
                    view.graph_custom_action(&selection, &target, &draft, action, window, cx)
                });
            }
        })
    }
}

impl CustomDraft {
    pub(crate) fn apply(
        &self,
        layer: &Layer,
        action: CustomAction,
        copied: bool,
        cx: &App,
    ) -> Result<(), String> {
        if !matches!(action, CustomAction::Add | CustomAction::Move) && self.is_changed(cx) {
            return Err("Add or move the point, or revert point edits first.".into());
        }
        if copied
            && !matches!(action, CustomAction::Plot(_))
            && let Ok(index) = usize::try_from(self.selected)
        {
            select_point(layer, index)?;
        }
        match action {
            CustomAction::Add | CustomAction::Move => {
                let mut values = [0.0; 2];
                for (index, value) in values.iter_mut().enumerate() {
                    *value = self.fields[index]
                        .read(cx)
                        .value()
                        .parse::<f32>()
                        .map_err(|_| "Point coordinates need numbers.")?;
                    if !value.is_finite() {
                        return Err("Point coordinates must be finite.".into());
                    }
                }
                if !(0.0..=1.0).contains(&values[0]) || !(-1000.0..=1000.0).contains(&values[1]) {
                    return Err("Point X must be from 0 to 1 and Y from -1000 to 1000.".into());
                }
                if matches!(action, CustomAction::Add) {
                    layer
                        .add_custom_point(values[0], values[1])
                        .map_err(|e| e.to_string())?;
                } else {
                    if self.selected < 0 {
                        return Err("Select a point first.".into());
                    }
                    layer
                        .move_selected_custom_point(values[0], values[1])
                        .map_err(|e| e.to_string())?;
                }
            }
            CustomAction::Remove => layer
                .remove_selected_custom_point()
                .map_err(|e| e.to_string())?,
            CustomAction::Select(index) => select_point(layer, index)?,
            CustomAction::Plot(action) => action.apply(layer)?,
            CustomAction::Loop(old, value) => {
                if layer.is_custom_looped().map_err(|e| e.to_string())? != old {
                    return Err("The curve loop changed. Try again.".into());
                }
                layer.set_custom_looped(value).map_err(|e| e.to_string())?;
            }
            CustomAction::Tangent(incoming, old, value) => {
                let p = point(&self.entries, self.selected).ok_or("Select a point first.")?;
                if (if incoming {
                    p.incoming_tangent()
                } else {
                    p.outgoing_tangent()
                }) != old
                {
                    return Err("The point tangent changed. Try again.".into());
                }
                if incoming {
                    layer.set_custom_incoming_tangent(value)
                } else {
                    layer.set_custom_outgoing_tangent(value)
                }
                .map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }
}

pub(crate) fn render_custom_points(
    id: &str,
    layer: &Layer,
    draft: &CustomDraft,
    plot: Option<gpui_kit::AnyElement>,
    disabled: bool,
    on_action: impl Fn(CustomAction, &mut Window, &mut App) + Clone + 'static,
) -> gpui_kit::AnyElement {
    let callback = |action: CustomAction| {
        let on_action = on_action.clone();
        move |_: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut App| {
            on_action(action.clone(), window, cx);
        }
    };
    let menu: Vec<_> = draft
        .entries
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let [x, y] = p.position();
            (
                format!("Point {} · {x}, {y}", i + 1),
                draft.selected == i as i32,
                callback(CustomAction::Select(i)),
            )
        })
        .collect();
    let mut content = div()
        .v_flex()
        .gap_2()
        .child(div().font_semibold().child("Custom points"))
        .children(plot);
    let looped = layer.is_custom_looped().unwrap_or(false);
    let on_loop = on_action.clone();
    content = content.child(
        Checkbox::new(format!("{id}-loop"))
            .label("Loop custom curve")
            .checked(looped)
            .disabled(disabled)
            .on_change(move |value, window, cx| {
                on_loop(CustomAction::Loop(looped, *value), window, cx);
            }),
    );
    if let Some(selected) = point(&draft.entries, draft.selected) {
        for (incoming, old) in [
            (true, selected.incoming_tangent()),
            (false, selected.outgoing_tangent()),
        ] {
            let on_tangent = on_action.clone();
            content = content.child(
                Checkbox::new(format!(
                    "{id}-{}-flat",
                    if incoming { "incoming" } else { "outgoing" }
                ))
                .label(if incoming {
                    "Incoming flat tangent"
                } else {
                    "Outgoing flat tangent"
                })
                .checked(old == 0)
                .disabled(disabled)
                .on_change(move |value, window, cx| {
                    on_tangent(
                        CustomAction::Tangent(incoming, old, if *value { 0 } else { 1 }),
                        window,
                        cx,
                    );
                }),
            );
        }
    }
    content = content.child(Form::new().columns(2).children(
        ["X", "Y"].into_iter().enumerate().map(|(i, label)| {
            Field::new().label(label).child(
                Input::new(&draft.fields[i])
                    .id(format!("{id}-{}", label.to_lowercase()))
                    .aria_label(format!("Point {label}"))
                    .disabled(disabled),
            )
        }),
    ));
    let add = callback(CustomAction::Add);
    let mov = callback(CustomAction::Move);
    let remove = callback(CustomAction::Remove);
    content
        .child(
            div()
                .h_flex()
                .flex_wrap()
                .gap_2()
                .child(
                    Button::new(format!("{id}-select"))
                        .label(if draft.selected < 0 {
                            "Select point".to_string()
                        } else {
                            format!("Point {}", draft.selected + 1)
                        })
                        .disabled(disabled || menu.is_empty())
                        .dropdown_menu(move |mut menu_view, _, _| {
                            for (name, selected, cb) in &menu {
                                menu_view = menu_view.item(
                                    PopupMenuItem::new(name.clone())
                                        .checked(*selected)
                                        .on_click(cb.clone()),
                                );
                            }
                            menu_view.scrollable(true)
                        }),
                )
                .child(
                    Button::new(format!("{id}-add"))
                        .label("Add point")
                        .disabled(disabled)
                        .on_click(add),
                )
                .child(
                    Button::new(format!("{id}-move"))
                        .label("Move point")
                        .disabled(disabled || draft.selected < 0)
                        .on_click(mov),
                )
                .child(
                    Button::new(format!("{id}-remove"))
                        .label("Remove point")
                        .disabled(disabled || draft.selected < 0)
                        .on_click(remove),
                ),
        )
        .into_any_element()
}
