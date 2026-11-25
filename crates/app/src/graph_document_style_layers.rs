//! Canvas layer commands share the retained layer handles used by Style and drawing.
use super::*;
use crate::graph_document::view::numeric_field::{NumericField, NumericRange};
use mycela_engine::{CanvasSymmetry, PointwiseKind};

#[derive(Clone)]
enum Action {
    Add,
    Select(CanvasLayer),
    ToggleEditor,
    Settings,
    Visibility(bool),
    Duplicate,
    Delete,
    Move(isize),
    Input(usize),
    Symmetry(CanvasSymmetry),
}

#[derive(Clone)]
struct LayerTarget {
    key: Option<String>,
    revision: u64,
    node: Node,
    layer: Option<CanvasLayer>,
}

impl LayerTarget {
    fn current(&self, view: &GraphDocumentView) -> bool {
        view.is_original_selection(&self.key, &Some(self.node.clone()), self.revision)
            && match (
                &self.layer,
                view.style
                    .as_ref()
                    .and_then(|style| style.canvas_layer.as_ref()),
            ) {
                (Some(a), Some(b)) => a.same_reference(b) && a.is_visible().is_some(),
                (None, None) => true,
                _ => false,
            }
    }

    fn edit(&self, action: Action, document: &LiveDocument) -> Result<Change, String> {
        let filter = self
            .node
            .filter()
            .ok_or("The Canvas node is unavailable.")?;
        if matches!(action, Action::Add) {
            return filter
                .edit_add_canvas_layer()
                .map(Change::Select)
                .map_err(model::failure);
        }
        let layer = self
            .layer
            .as_ref()
            .ok_or("The Canvas layer is unavailable.")?;
        let result = match action {
            Action::Visibility(visible) => layer.edit_visibility(visible),
            Action::Move(offset) => layer.edit_move(offset),
            Action::Input(index) => layer.edit_input(index),
            Action::Symmetry(mode) => layer.edit_symmetry(mode),
            Action::Duplicate => {
                return layer
                    .edit_duplicate()
                    .map(Change::Select)
                    .map_err(model::failure);
            }
            Action::Delete => {
                let index = filter
                    .canvas_layer_editors()
                    .iter()
                    .position(|candidate| layer.same_reference(candidate))
                    .ok_or("The Canvas layer was removed.")?;
                let handled = guard(document)
                    .remove_canvas_layer_with_synthesis_adjusters(&self.node, layer)
                    .map_err(model::failure)?;
                if !handled {
                    layer.edit_remove().map_err(model::failure)?;
                }
                let layers = filter.canvas_layer_editors();
                return Ok(layers
                    .get(index)
                    .or_else(|| layers.last())
                    .cloned()
                    .map_or(Change::Update, Change::Select));
            }
            _ => unreachable!(),
        };
        result.map(Change::from).map_err(model::failure)
    }
}

enum Change {
    Unchanged,
    Update,
    Select(CanvasLayer),
}

impl From<bool> for Change {
    fn from(changed: bool) -> Self {
        if changed {
            Self::Update
        } else {
            Self::Unchanged
        }
    }
}

#[derive(Clone)]
struct Actions {
    target: LayerTarget,
    owner: WeakEntity<GraphDocumentView>,
    disabled: bool,
}

impl Actions {
    fn run(&self, action: Action, window: &mut Window, cx: &mut App) {
        let _ = self.owner.update(cx, |view, cx| {
            view.canvas_layer_action(&self.target, action, window, cx)
        });
    }

    fn item(&self, label: impl Into<SharedString>, action: Action) -> PopupMenuItem {
        let actions = self.clone();
        PopupMenuItem::new(label)
            .on_click(move |_, window, cx| actions.run(action.clone(), window, cx))
    }

    fn button(&self, id: &'static str, label: &'static str, action: Action) -> Button {
        let actions = self.clone();
        Button::new(id)
            .small()
            .label(label)
            .disabled(self.disabled)
            .on_click(move |_, window, cx| actions.run(action.clone(), window, cx))
    }
}

fn layer_label(layer: &CanvasLayer) -> String {
    if layer.is_visible() == Some(false) {
        format!("{} · Hidden", layer.label())
    } else {
        layer.label().to_owned()
    }
}

impl GraphDocumentView {
    pub(in crate::graph_document::view) fn render_canvas_layer_controls(
        &self,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let node = self.inspector.owner.as_ref()?.clone();
        if node.kind() != Some(PointwiseKind::Canvas) {
            return None;
        }
        let layers = node.filter()?.canvas_layer_editors();
        let panel = self.style.as_ref();
        let selected = panel.and_then(|style| style.canvas_layer.as_ref()).cloned();
        let expanded = panel.is_some_and(|style| style.expanded);
        let position = selected.as_ref().and_then(|selected| {
            layers
                .iter()
                .position(|layer| layer.same_reference(selected))
        });
        let inputs = node.reference_inputs().map_or(0, |ports| ports.len());
        let actions = Actions {
            target: LayerTarget {
                key: self.selection.clone(),
                revision: self.revision,
                node,
                layer: selected.clone(),
            },
            owner: cx.weak_entity(),
            disabled: busy || self.has_field_drafts(cx),
        };

        let row = div().h_flex().gap_2().flex_wrap().child(actions.button(
            "canvas-add-layer",
            "Add layer",
            Action::Add,
        ));
        let Some((selected, position)) = selected.zip(position) else {
            return Some(row.into_any_element());
        };
        let visible = selected.is_visible()?;
        let symmetry = selected.symmetry()?;
        let input = selected.input_index();
        let input_label = match input {
            Some(index) if index < inputs => format!("Input {}", index + 1),
            Some(index) if inputs > 0 => format!("Input {} · Unavailable", index + 1),
            _ => "Input texture".into(),
        };
        let count = layers.len();
        let selection = actions.clone();
        let routing = actions.clone();
        let symmetry_actions = actions.clone();
        let management = actions.clone();
        Some(
            row.child(
                Button::new("canvas-style-layer")
                    .small()
                    .label(layer_label(&selected))
                    .disabled(actions.disabled || count < 2)
                    .dropdown_menu(move |mut menu, _, _| {
                        for layer in &layers {
                            let item =
                                selection.item(layer_label(layer), Action::Select(layer.clone()));
                            menu = menu.item(item.checked(layer.same_reference(&selected)));
                        }
                        menu.scrollable(true)
                    }),
            )
            .child(
                Button::new("canvas-layer-actions")
                    .small()
                    .label("Layer actions")
                    .disabled(actions.disabled)
                    .dropdown_menu(move |menu, _, _| {
                        menu.item(management.item("Layer properties…", Action::Settings))
                            .item(management.item(
                                if visible { "Hide layer" } else { "Show layer" },
                                Action::Visibility(!visible),
                            ))
                            .item(management.item("Duplicate layer", Action::Duplicate))
                            .item(
                                management
                                    .item("Move layer up", Action::Move(-1))
                                    .disabled(position == 0),
                            )
                            .item(
                                management
                                    .item("Move layer down", Action::Move(1))
                                    .disabled(position + 1 == count),
                            )
                            .separator()
                            .item(management.item("Delete layer", Action::Delete))
                    }),
            )
            .when(inputs > 0, |row| {
                row.child(
                    Button::new("canvas-layer-input")
                        .small()
                        .label(input_label)
                        .disabled(actions.disabled)
                        .dropdown_menu(move |mut menu, _, _| {
                            for index in 0..inputs {
                                let item = routing
                                    .item(format!("Input {}", index + 1), Action::Input(index));
                                menu = menu.item(item.checked(input == Some(index)));
                            }
                            menu.scrollable(true)
                        }),
                )
            })
            .child(
                Button::new("canvas-layer-symmetry")
                    .small()
                    .label(format!("Symmetry · {}", symmetry.label()))
                    .disabled(actions.disabled)
                    .dropdown_menu(move |mut menu, _, _| {
                        for mode in CanvasSymmetry::ALL {
                            let item = symmetry_actions.item(mode.label(), Action::Symmetry(mode));
                            menu = menu.item(item.checked(mode == symmetry));
                        }
                        menu.scrollable(true)
                    }),
            )
            .child(
                actions
                    .button(
                        "canvas-style-expanded",
                        if expanded {
                            "Close layer style"
                        } else {
                            "Edit layer style"
                        },
                        Action::ToggleEditor,
                    )
                    .ghost(),
            )
            .into_any_element(),
        )
    }

    fn canvas_layer_action(
        &mut self,
        target: &LayerTarget,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_busy() || !target.current(self) || self.has_field_drafts(cx) {
            return;
        }
        match action {
            Action::ToggleEditor => {
                if let Some(panel) = &mut self.style {
                    panel.expanded = !panel.expanded;
                    self.select(target.key.clone(), window, cx);
                }
            }
            Action::Select(layer) => {
                self.select_layer_handle(&layer);
                self.select(target.key.clone(), window, cx);
                self.select_layer_area(&layer, window, cx);
            }
            Action::Settings => self.open_layer_properties(target, window, cx),
            action => {
                let document = self.document.clone();
                if let Err(error) =
                    self.commit_canvas_layer_edit(|| target.edit(action, &document), window, cx)
                {
                    self.error = Some(error);
                    cx.notify();
                }
            }
        }
    }

    fn select_layer_handle(&mut self, layer: &CanvasLayer) {
        if let Some(panel) = &mut self.style {
            panel.canvas_layer = Some(layer.clone());
        }
    }

    fn select_layer_area(
        &mut self,
        layer: &CanvasLayer,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(area) = self
            .inspector
            .canvas_areas
            .iter()
            .find(|candidate| candidate.layer().same_reference(layer))
            .cloned()
        {
            self.error = self.inspector.select_canvas_area(area, window, cx).err();
        } else {
            self.inspector.canvas_area = None;
            self.inspector
                .drafts
                .retain(|draft| draft.canvas_area_field().is_none());
        }
        self.observe_drafts(cx);
        cx.notify();
    }

    fn commit_canvas_layer_edit(
        &mut self,
        edit: impl FnOnce() -> Result<Change, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        self.ensure_history_binding();
        let was_dirty = self.dirty;
        let stamp = self.begin_history_edit();
        let change = match edit() {
            Ok(Change::Unchanged) => {
                self.dirty = was_dirty;
                return Ok(());
            }
            Err(error) => {
                self.dirty = was_dirty;
                return Err(error);
            }
            Ok(change) => change,
        };
        self.revision += 1;
        self.error = None;
        self.status = None;
        if let Change::Select(layer) = &change {
            self.select_layer_handle(layer);
        }
        self.finish_history_edit(stamp, true);
        self.rebuild(window, cx);
        if let Change::Select(layer) = &change {
            self.select_layer_area(layer, window, cx);
        }
        self.start_preview(cx);
        Ok(())
    }

    fn open_layer_properties(
        &mut self,
        target: &LayerTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((name, scale)) = target
            .layer
            .as_ref()
            .and_then(|layer| layer.name().zip(layer.scale()))
        else {
            return;
        };
        let form = cx.new(|cx| Properties {
            name: cx.new(|cx| InputState::new(window, cx).default_value(name)),
            scale: cx.new(|cx| InputState::new(window, cx).default_value(scale.to_string())),
            error: None,
        });
        let owner = cx.weak_entity();
        let target = target.clone();
        self.pause_preview();
        self.modal = true;
        window.open_dialog(cx, move |dialog, _, _| {
            let inputs = form.clone();
            let edit = owner.clone();
            let close = owner.clone();
            let target = target.clone();
            dialog
                .title("Canvas layer properties")
                .child(form.clone())
                .footer(
                    DialogFooter::new()
                        .child(DialogClose::new().child(Button::new("cancel").label("Cancel")))
                        .child(
                            DialogAction::new().child(Button::new("ok").primary().label("Apply")),
                        ),
                )
                .on_ok(move |_, window, cx| {
                    let result = inputs.read(cx).values(scale, cx).and_then(|(name, scale)| {
                        edit.update(cx, |view, cx| {
                            let layer = target
                                .layer
                                .as_ref()
                                .filter(|_| target.current(view))
                                .ok_or("The selected layer changed. Open its properties again.")?;
                            view.commit_canvas_layer_edit(
                                || {
                                    layer
                                        .edit_properties(&name, scale)
                                        .map(Change::from)
                                        .map_err(model::failure)
                                },
                                window,
                                cx,
                            )
                        })
                        .map_err(model::failure)?
                    });
                    if let Err(error) = result {
                        inputs.update(cx, |form, cx| {
                            form.error = Some(error);
                            cx.notify();
                        });
                        return false;
                    }
                    true
                })
                .on_close(move |_, _, cx| {
                    let _ = close.update(cx, |view, cx| {
                        view.modal = false;
                        cx.notify();
                    });
                })
        });
        cx.notify();
    }
}

struct Properties {
    name: Entity<InputState>,
    scale: Entity<InputState>,
    error: Option<String>,
}

impl Properties {
    fn values(&self, original: f32, cx: &App) -> Result<(String, f32), String> {
        let scale: f32 = self
            .scale
            .read(cx)
            .value()
            .trim()
            .parse()
            .map_err(|_| "Enter a style scale from −20 to 20.")?;
        if scale.to_bits() != original.to_bits() && !(-20. ..=20.).contains(&scale) {
            return Err("Enter a style scale from −20 to 20.".into());
        }
        Ok((self.name.read(cx).value().to_string(), scale))
    }
}

impl Render for Properties {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .v_flex()
            .gap_3()
            .child(
                Form::new()
                    .child(
                        Field::new().label("Name").child(
                            Input::new(&self.name)
                                .id("canvas-layer-name")
                                .aria_label("Name"),
                        ),
                    )
                    .child(Field::new().label("Style scale").child(NumericField::new(
                        &self.scale,
                        "canvas-layer-scale",
                        "Style scale",
                        NumericRange::new(-20.0, 20.0, 0.01),
                    ))),
            )
            .when_some(self.error.as_ref(), |form, error| {
                form.child(
                    div()
                        .id("canvas-layer-properties-error")
                        .role(gpui_kit::Role::Alert)
                        .aria_label(error.clone())
                        .test_support()
                        .text_sm()
                        .text_color(cx.theme().danger)
                        .child(error.clone()),
                )
            })
    }
}
