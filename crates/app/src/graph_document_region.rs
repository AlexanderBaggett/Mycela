use super::{GraphDocumentView, model};
use crate::ForegroundError;
use gpui_kit::base::{Disableable, StyledExt, TestSupportExt};
use gpui_kit::component::{
    ActiveTheme, WindowExt,
    button::{Button, ButtonVariants},
    dialog::{DialogAction, DialogClose, DialogFooter},
};
use gpui_kit::{
    AnyElement, AppContext, Bounds, Context, MouseButton, PathBuilder, Pixels, Point, Render,
    RenderImage, Window, canvas, div, fill, img, point, prelude::*, px, rems, size,
};
use mycela_engine::{PointwiseKind, PointwiseRegion};
use std::{cell::Cell, rc::Rc, sync::Arc};

type Fields = (i32, f32, f32, f32);

struct RegionDrag {
    start: Point<Pixels>,
    fields: Fields,
    resize: bool,
}

struct RegionPicker {
    fields: Fields,
    surface: Rc<Cell<Bounds<Pixels>>>,
    drag: Option<RegionDrag>,
    raster: Option<Result<Arc<RenderImage>, String>>,
    error: Option<String>,
}

impl RegionPicker {
    fn handle(fields: Fields, bounds: Bounds<Pixels>) -> Point<Pixels> {
        let (mode, x, y, height) = fields;
        let aspect = PointwiseRegion::<ForegroundError>::aspect_for_mode(mode);
        bounds.origin
            + point(
                bounds.size.width * (x + height * aspect),
                bounds.size.height * y,
            )
    }

    fn move_pointer(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(drag) = &self.drag else { return };
        let bounds = self.surface.get();
        if bounds.size.width <= px(0.) || bounds.size.height <= px(0.) {
            return;
        }
        let delta = position - drag.start;
        let dx = delta.x / bounds.size.width;
        let dy = delta.y / bounds.size.height;
        let (mode, mut x, mut y, mut height) = drag.fields;
        let aspect = PointwiseRegion::<ForegroundError>::aspect_for_mode(mode);
        if drag.resize {
            let bottom = (y + height).clamp(0., 1.);
            let max_height = bottom.min(((1. - x) / aspect).max(0.));
            height = (height + (dx - dy) / 2.).clamp(0., max_height);
            y = bottom - height;
        } else {
            x = (x + dx).clamp(0., (1. - height * aspect).max(0.));
            y = (y + dy).clamp(0., (1. - height).max(0.));
        }
        self.fields = (mode, x, y, height);
        self.error = None;
        cx.notify();
    }
}

impl Render for RegionPicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let fields = self.fields;
        let surface = self.surface.clone();
        let zoom = window.rem_size();
        let grip = zoom * 0.625;
        let stroke = zoom * 0.0625;
        let theme = cx.theme();
        let outline = theme.foreground;
        let shadow = theme.background;
        let accent = theme.primary;
        let guide = canvas(
            move |bounds, _, _| surface.set(bounds),
            move |bounds, (), window, _| {
                let (mode, x, y, height) = fields;
                if ![x, y, height].into_iter().all(f32::is_finite) {
                    return;
                }
                let handle = Self::handle(fields, bounds);
                let left = bounds.origin.x + bounds.size.width * x;
                let bottom = handle.y + bounds.size.height * height;
                let upper = match mode {
                    2 => (left + handle.x) / 2.,
                    3 => handle.x,
                    _ => left,
                };
                for (width, color) in [(stroke * 3., shadow), (stroke, outline)] {
                    let mut path = PathBuilder::stroke(width);
                    path.move_to(point(upper, handle.y));
                    if mode == 0 {
                        path.line_to(handle);
                    }
                    path.line_to(point(handle.x, bottom));
                    path.line_to(point(left, bottom));
                    path.line_to(point(upper, handle.y));
                    if let Ok(path) = path.build() {
                        window.paint_path(path, color);
                    }
                }
                window.paint_quad(fill(
                    Bounds::new(handle - point(grip / 2., grip / 2.), size(grip, grip)),
                    shadow,
                ));
                window.paint_quad(fill(
                    Bounds::new(
                        handle - point(grip / 2. - stroke, grip / 2. - stroke),
                        size(grip - stroke * 2., grip - stroke * 2.),
                    ),
                    accent,
                ));
            },
        )
        .absolute()
        .inset_0()
        .size_full();
        let raster = self
            .raster
            .as_ref()
            .and_then(|result| result.as_ref().ok())
            .cloned();
        let message = match &self.raster {
            None => "Rendering Original…",
            Some(Err(error)) => error,
            Some(Ok(_)) => "Drag to move. Drag the corner handle to resize.",
        };
        div()
            .v_flex()
            .gap_2()
            .items_center()
            .child(
                div()
                    .id("kaleidoscope-region-surface")
                    .test_support()
                    .relative()
                    .w(rems(17.))
                    .max_w_full()
                    .aspect_ratio(1.)
                    .overflow_hidden()
                    .bg(theme.muted)
                    .cursor_move()
                    .when_some(raster, |element, raster| {
                        element.child(img(raster).size_full())
                    })
                    .child(guide)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |picker, event: &gpui_kit::MouseDownEvent, _, _| {
                            let bounds = picker.surface.get();
                            let grip_center = Self::handle(picker.fields, bounds);
                            let delta = event.position - grip_center;
                            if ![picker.fields.1, picker.fields.2, picker.fields.3]
                                .into_iter()
                                .all(f32::is_finite)
                            {
                                return;
                            }
                            picker.drag = Some(RegionDrag {
                                start: event.position,
                                fields: picker.fields,
                                resize: delta.x.abs() <= grip && delta.y.abs() <= grip,
                            });
                        }),
                    )
                    .on_mouse_move(cx.listener(
                        |picker, event: &gpui_kit::MouseMoveEvent, _, cx| {
                            if event.pressed_button == Some(MouseButton::Left) {
                                picker.move_pointer(event.position, cx);
                            } else {
                                picker.drag = None;
                            }
                        },
                    ))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|picker, event: &gpui_kit::MouseUpEvent, _, cx| {
                            picker.move_pointer(event.position, cx);
                            picker.drag = None;
                        }),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(|picker, event: &gpui_kit::MouseUpEvent, _, cx| {
                            picker.move_pointer(event.position, cx);
                            picker.drag = None;
                        }),
                    ),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(message.to_owned()),
            )
            .when_some(self.error.clone(), |element, error| {
                element.child(div().text_sm().text_color(theme.danger).child(error))
            })
    }
}

impl GraphDocumentView {
    pub(super) fn render_region_picker(
        &self,
        busy: bool,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        (self.inspector.owner.as_ref()?.kind() == Some(PointwiseKind::Kaleidoscope)).then(|| {
            Button::new("graph-pick-region")
                .label("Select source region…")
                .disabled(busy || self.has_field_drafts(cx))
                .on_click(cx.listener(|view, _, window, cx| view.open_region_picker(window, cx)))
                .into_any_element()
        })
    }

    fn open_region_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_busy() || self.has_field_drafts(cx) {
            return;
        }
        let Some(node) = self.inspector.owner.clone() else {
            return;
        };
        let Some(filter) = node
            .filter()
            .filter(|filter| filter.kind() == PointwiseKind::Kaleidoscope)
        else {
            return;
        };
        let Ok(property) = filter.parameter(1) else {
            return;
        };
        let Ok(region) = property.region() else {
            return;
        };
        let before = region.fields();
        let pending = node
            .slot(0)
            .ok()
            .and_then(|slot| slot.node())
            .map(|original| self.freeze_preview(&original, self.sample_time));
        let ticket = model::Ticket::new();
        let picker = cx.new(|_| RegionPicker {
            fields: before,
            surface: Rc::new(Cell::new(Bounds::default())),
            drag: None,
            raster: pending
                .is_none()
                .then(|| Err("Connect Original to see its image.".into())),
            error: None,
        });
        if let Some(pending) = pending {
            let work = ticket.clone();
            let library = self.environment_library.clone();
            let render = cx.background_spawn(async move {
                model::render(pending?.materialize()?, &work, Some(&library))?
                    .map(|preview| preview.raster)
                    .ok_or_else(|| "The original image is unavailable.".to_string())
            });
            picker.update(cx, |_, cx| {
                cx.spawn(async move |picker, cx| {
                    let result = render.await;
                    let _ = picker.update(cx, |picker, cx| {
                        picker.raster = Some(result);
                        cx.notify();
                    });
                })
                .detach();
            });
        }
        let owner = cx.weak_entity();
        let selection = self.selection.clone();
        let revision = self.revision;
        self.pause_preview();
        self.modal = true;
        window.open_dialog(cx, move |dialog, _, _| {
            let apply = owner.clone();
            let close = owner.clone();
            let picker_state = picker.clone();
            let selection = selection.clone();
            let node = Some(node.clone());
            let property = property.clone();
            let region = region.clone();
            let ticket = ticket.clone();
            dialog
                .title("Source region")
                .child(picker.clone())
                .footer(
                    DialogFooter::new()
                        .child(DialogClose::new().child(Button::new("cancel").label("Cancel")))
                        .child(
                            DialogAction::new().child(Button::new("ok").label("Apply").primary()),
                        ),
                )
                .on_ok(move |_, window, cx| {
                    let after = picker_state.read(cx).fields;
                    apply
                        .update(cx, |view, cx| {
                            if !view.is_original_selection(&selection, &node, revision)
                                || before == after
                            {
                                return true;
                            }
                            view.ensure_history_binding();
                            let stamp = view.begin_history_edit();
                            if let Err(error) = property.edit_region(&region, before, after) {
                                view.finish_history_edit(stamp, false);
                                picker_state.update(cx, |picker, cx| {
                                    picker.error = Some(model::failure(error));
                                    cx.notify();
                                });
                                return false;
                            }
                            view.revision += 1;
                            view.error = None;
                            view.status = None;
                            view.finish_history_edit(stamp, true);
                            view.rebuild(window, cx);
                            view.start_preview(cx);
                            true
                        })
                        .unwrap_or(true)
                })
                .on_close(move |_, _, cx| {
                    ticket.retire();
                    let _ = close.update(cx, |view, cx| {
                        view.modal = false;
                        cx.notify();
                    });
                })
        });
        cx.notify();
    }
}
