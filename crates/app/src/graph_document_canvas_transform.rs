//! Numeric and immediate transforms of the active Canvas selection.
use super::*;
use crate::graph_document::view::numeric_field::{NumericField, NumericRange};
use gpui_kit::component::{
    WindowExt,
    dialog::{DialogAction, DialogClose, DialogFooter},
    input::InputState,
    menu::PopupMenu,
};
use gpui_kit::{App, Entity, Role, WeakEntity};
use mycela_engine::{
    CanvasAlignment as Alignment, CanvasSelectionEdit, CanvasSelectionField as SelectionField,
    CanvasSelectionTransform as Transform, CanvasTransformOrientation,
};

#[derive(Clone, Copy)]
enum Command {
    Change(Transform),
    Enter(SelectionField),
}

#[derive(Clone)]
struct Target {
    owner: WeakEntity<GraphDocumentView>,
    node: Node,
    selection: Option<String>,
    revision: u64,
    shapes: Vec<CanvasShape>,
    items: Vec<selection::SelectionItem>,
    orientation: CanvasTransformOrientation,
}

impl Target {
    fn current(&self, view: &GraphDocumentView, cx: &Context<GraphDocumentView>) -> bool {
        view.is_original_selection(&self.selection, &Some(self.node.clone()), self.revision)
            && view.drawing_current(cx)
            && !view.has_field_drafts(cx)
            && view.drawing.as_ref().is_some_and(|tool| {
                tool.is_selecting()
                    && tool.transform_orientation() == self.orientation
                    && tool.selection.items.len() == self.items.len()
                    && tool
                        .selection
                        .items
                        .iter()
                        .zip(&self.items)
                        .all(|((_, a), (_, b))| a.identity() == b.identity())
                    && tool.selection.shapes.len() == self.shapes.len()
                    && tool
                        .selection
                        .shapes
                        .iter()
                        .zip(&self.shapes)
                        .all(|(a, b)| a.same_reference(b))
            })
    }

    fn item(&self, label: &'static str, command: Command) -> PopupMenuItem {
        let target = self.clone();
        PopupMenuItem::new(label).on_click(move |_, window, cx| {
            let _ = target.owner.update(cx, |view, cx| {
                if target.current(view, cx) {
                    view.transform_canvas_selection(&target, command, window, cx);
                }
            });
        })
    }
}

impl GraphDocumentView {
    pub(super) fn render_canvas_transform_menu(
        &self,
        tool: &DrawingTool,
        busy: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let target = Target {
            owner: cx.weak_entity(),
            node: tool.target.clone(),
            selection: self.selection.clone(),
            revision: self.revision,
            shapes: tool.selection.shapes.clone(),
            items: tool.selection.items.clone(),
            orientation: tool.transform_orientation(),
        };
        Button::new("texture-canvas-transform")
            .small()
            .label("Transform")
            .disabled(busy || self.has_field_drafts(cx))
            .dropdown_menu(move |menu, _, _| transform_menu(menu, &target))
    }

    fn transform_canvas_selection(
        &mut self,
        target: &Target,
        command: Command,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(filter) = target.node.filter() else {
            return;
        };
        if let Command::Change(Transform::Align(alignment)) = command {
            self.apply_canvas_shape_change(
                false,
                |_| filter.edit_align_canvas_items(&target.items, alignment),
                window,
                cx,
            );
            return;
        }
        let edit = match filter.canvas_selection_edit_for(&target.shapes, target.orientation) {
            Ok(edit) => edit,
            Err(error) => {
                self.error = Some(super::super::model::failure(error));
                cx.notify();
                return;
            }
        };
        match command {
            Command::Change(transform) => {
                self.apply_canvas_shape_change(false, |_| edit.apply(transform), window, cx);
            }
            Command::Enter(field) => {
                self.open_canvas_transform_value(target, field, edit, window, cx)
            }
        }
    }

    fn open_canvas_transform_value(
        &mut self,
        target: &Target,
        field: SelectionField,
        edit: CanvasSelectionEdit<ForegroundError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let initial = edit.value(field);
        let target = target.clone();
        self.open_canvas_value_dialog(
            ValuePrompt {
                title: format!("Selection {}", field.label().to_lowercase()),
                label: field.label(),
                bounds: field.bounds(),
                input_id: "canvas-transform-value",
                error_id: "canvas-transform-error",
            },
            initial,
            move |view, value, window, cx| {
                if target.current(view, cx) {
                    view.apply_canvas_shape_change(
                        false,
                        |_| edit.apply(Transform::Set(field, value)),
                        window,
                        cx,
                    );
                }
            },
            window,
            cx,
        );
    }

    pub(super) fn open_canvas_value_dialog(
        &mut self,
        prompt: ValuePrompt,
        initial: f32,
        apply: impl Fn(&mut Self, f32, &mut Window, &mut Context<Self>) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title = prompt.title.clone();
        let form = cx.new(|cx| ValueForm {
            prompt,
            input: cx.new(|cx| InputState::new(window, cx).default_value(initial.to_string())),
            error: None,
        });
        let owner = cx.weak_entity();
        let apply = Rc::new(apply);
        self.pause_preview();
        self.modal = true;
        window.open_dialog(cx, move |dialog, _, _| {
            let inputs = form.clone();
            let apply = apply.clone();
            let close = owner.clone();
            let owner = owner.clone();
            dialog
                .title(title.clone())
                .child(form.clone())
                .footer(
                    DialogFooter::new()
                        .child(DialogClose::new().child(Button::new("cancel").label("Cancel")))
                        .child(
                            DialogAction::new().child(Button::new("ok").primary().label("Apply")),
                        ),
                )
                .on_ok(move |_, window, cx| {
                    let value = inputs.read(cx).value(cx);
                    let Ok(value) = value else {
                        inputs.update(cx, |form, cx| {
                            form.error = value.err();
                            cx.notify();
                        });
                        return false;
                    };
                    let _ = owner.update(cx, |view, cx| {
                        view.modal = false;
                        if value != initial {
                            apply(view, value, window, cx);
                        }
                    });
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

fn transform_menu(mut menu: PopupMenu, target: &Target) -> PopupMenu {
    for (label, operation) in [
        ("Rotate 45° clockwise", Transform::Rotate(-45.)),
        ("Rotate 45° counterclockwise", Transform::Rotate(45.)),
        ("Flip horizontally", Transform::FlipHorizontal),
        ("Flip vertically", Transform::FlipVertical),
    ] {
        menu = menu.item(target.item(label, Command::Change(operation)));
    }
    if target.shapes.len() == 1 && target.orientation == CanvasTransformOrientation::Shape {
        return menu;
    }
    menu = menu.separator();
    for (label, field) in [
        ("Set selection center X…", SelectionField::CenterX),
        ("Set selection center Y…", SelectionField::CenterY),
        ("Set selection width…", SelectionField::Width),
        ("Set selection height…", SelectionField::Height),
        ("Set selection skew X…", SelectionField::SkewX),
        ("Set selection skew Y…", SelectionField::SkewY),
        ("Set selection angle…", SelectionField::Angle),
    ] {
        menu = menu.item(target.item(label, Command::Enter(field)));
    }
    if target.shapes.len() == 1 {
        return menu.scrollable(true);
    }
    menu = menu.separator();
    for (label, alignment) in [
        ("Align left", Alignment::Left),
        ("Align horizontal centers", Alignment::HorizontalCenter),
        ("Align right", Alignment::Right),
        ("Align top", Alignment::Top),
        ("Align vertical centers", Alignment::VerticalCenter),
        ("Align bottom", Alignment::Bottom),
    ] {
        menu = menu.item(target.item(label, Command::Change(Transform::Align(alignment))));
    }
    menu.scrollable(true)
}

pub(super) struct ValuePrompt {
    pub(super) title: String,
    pub(super) label: &'static str,
    pub(super) bounds: [f32; 2],
    pub(super) input_id: &'static str,
    pub(super) error_id: &'static str,
}

struct ValueForm {
    prompt: ValuePrompt,
    input: Entity<InputState>,
    error: Option<String>,
}

impl ValueForm {
    fn value(&self, cx: &App) -> Result<f32, String> {
        let [min, max] = self.prompt.bounds;
        self.input
            .read(cx)
            .value()
            .trim()
            .parse::<f32>()
            .ok()
            .filter(|value| (min..=max).contains(value))
            .ok_or_else(|| format!("Enter a value from {min} to {max}."))
    }
}

impl Render for ValueForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .v_flex()
            .gap_2()
            .child(
                Form::new().child(
                    Field::new()
                        .label(self.prompt.label)
                        .child(NumericField::new(
                            &self.input,
                            self.prompt.input_id,
                            self.prompt.label,
                            NumericRange::bounds(self.prompt.bounds, 0.01),
                        )),
                ),
            )
            .when_some(self.error.as_ref(), |form, error| {
                form.child(
                    div()
                        .id(self.prompt.error_id)
                        .role(Role::Alert)
                        .aria_label(error.clone())
                        .test_support()
                        .text_sm()
                        .text_color(cx.theme().danger)
                        .child(error.clone()),
                )
            })
    }
}
