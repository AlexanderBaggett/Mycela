//! Pointer and keyboard handling for the legacy gradient strip.
use super::super::{NudgeLegacyStopLeft, NudgeLegacyStopRight, RemoveLegacyStop};
use super::{Action, GraphDocumentView, LegacyGradientEditor, STRIP_WIDTH, strip};
use gpui_kit::base::TestSupportExt;
use gpui_kit::component::ActiveTheme;
use gpui_kit::{
    App, Bounds, Context, DispatchPhase, Hsla, IntoElement, MouseButton, MouseMoveEvent,
    MouseUpEvent, Pixels, Point, Window, canvas, div, fill, point, prelude::*, px, size,
};
use std::{cell::Cell, rc::Rc, sync::Arc};

const RAMP_ID: &str = "graph-legacy-gradient-ramp";

#[derive(Clone, Copy)]
struct StripSpace(Bounds<Pixels>);

impl StripSpace {
    fn marker_x(self, position: f64) -> Pixels {
        self.0.left() + self.0.size.width * position.clamp(0., 1.) as f32
    }

    fn marker_bottom(self) -> Pixels {
        self.0.top() + px(12.)
    }

    fn position(self, x: Pixels) -> f64 {
        f64::from(((x - self.0.left()) / self.0.size.width).clamp(0., 1.))
    }

    fn outside_removal_band(self, y: Pixels) -> bool {
        y < self.0.top() - px(40.) || y > self.0.bottom() + px(40.)
    }
}

pub(super) struct StopDrag {
    space: StripSpace,
    pointer_x: Pixels,
    start: f64,
    moved: bool,
    outside: bool,
}

impl LegacyGradientEditor {
    fn press_strip(
        &mut self,
        space: StripSpace,
        pointer: Point<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(), String> {
        self.drag = None;
        self.focus.focus(window, cx);
        let selected = self.selected.as_ref().map(|(index, _)| *index);
        let hit = self
            .working
            .stops()
            .iter()
            .enumerate()
            .filter(|(_, stop)| stop[0].is_finite())
            .map(|(index, stop)| (index, (space.marker_x(stop[0]) - pointer.x).abs()))
            .filter(|(_, distance)| {
                *distance <= px(8.) && pointer.y < space.marker_bottom() + px(2.)
            })
            .min_by(|(a, x), (b, y)| {
                x.partial_cmp(y)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| (Some(*b) == selected).cmp(&(Some(*a) == selected)))
            });
        if let Some((index, _)) = hit {
            self.action(Action::Select(index), window, cx)?;
            self.drag = Some(StopDrag {
                space,
                pointer_x: pointer.x,
                start: self.working.stops()[index][0],
                moved: false,
                outside: false,
            });
        } else {
            self.action(Action::AddAt(space.position(pointer.x)), window, cx)?;
        }
        Ok(())
    }

    fn move_selected(&mut self, position: f64, window: &mut Window, cx: &mut App) -> bool {
        let Some((index, fields)) = &mut self.selected else {
            return false;
        };
        let position = position.clamp(0., 1.);
        if self.working.stops()[*index][0] == position
            || !self.working.set_stop_component(*index, 0, position)
        {
            return false;
        }
        self.requires_ordering = true;
        self.strip = strip(&self.working);
        fields[0].loaded = position.to_string();
        fields[0].input.update(cx, |input, cx| {
            input.set_value(fields[0].loaded.clone(), window, cx)
        });
        true
    }

    fn drag_strip(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut App) -> bool {
        let Some(drag) = &mut self.drag else {
            return false;
        };
        if event.pressed_button != Some(MouseButton::Left) || !self.focus.is_focused(window) {
            self.drag = None;
            return false;
        }
        let delta = event.position.x - drag.pointer_x;
        drag.moved |= delta.abs() >= px(3.);
        let outside = drag.space.outside_removal_band(event.position.y);
        let restore_marker = drag.outside && !outside;
        if outside {
            return !std::mem::replace(&mut drag.outside, true);
        }
        drag.outside = false;
        if !drag.moved {
            return restore_marker;
        }
        let position = drag.start + f64::from(delta / drag.space.0.size.width);
        self.move_selected(position, window, cx) || restore_marker
    }

    fn finish_drag(&mut self, y: Pixels, window: &mut Window, cx: &mut App) -> Result<(), String> {
        if self
            .drag
            .take()
            .is_some_and(|drag| drag.space.outside_removal_band(y))
            && self.focus.is_focused(window)
        {
            self.action(Action::Remove, window, cx)?;
        }
        Ok(())
    }

    fn nudge(&mut self, steps: f64, window: &mut Window, cx: &mut App) -> Result<(), String> {
        let Some((index, _)) = self.selected.as_ref() else {
            return Ok(());
        };
        let index = *index;
        self.action(Action::Select(index), window, cx)?;
        let position = self.working.stops()[index][0] + steps / STRIP_WIDTH as f64;
        self.move_selected(position, window, cx);
        Ok(())
    }
}

impl GraphDocumentView {
    pub(super) fn render_legacy_strip(
        &self,
        editor: &LegacyGradientEditor,
        positions: Arc<[f64]>,
        disabled: bool,
        cx: &Context<Self>,
    ) -> gpui_kit::AnyElement {
        let space = Rc::new(Cell::new(None));
        let layout = space.clone();
        let focus = editor.focus.clone();
        let (drag_focus, release_focus) = (focus.clone(), focus.clone());
        let (press_focus, delete_focus, left_focus, right_focus) =
            (focus.clone(), focus.clone(), focus.clone(), focus.clone());
        let (drag_owner, release_owner) = (cx.weak_entity(), cx.weak_entity());
        let selected = editor.selected.as_ref().map(|(index, _)| *index);
        let marker_hidden = editor.drag.as_ref().is_some_and(|drag| drag.outside);
        let samples = editor.strip;
        let theme = cx.theme();
        let (border, foreground, primary, background, muted) = (
            theme.border,
            theme.foreground,
            theme.primary,
            theme.background,
            theme.muted,
        );
        let surface =
            canvas(
                move |bounds, _, _| {
                    let geometry = StripSpace(bounds.inset(px(6.)));
                    let space = (geometry.0.size.width > px(0.)).then_some(geometry);
                    layout.set(space);
                    space
                },
                move |_, space, window, _| {
                    if let Some(space) = space {
                        paint_ramp(space, &samples, (background, muted), window);
                        for (index, &position) in positions.iter().enumerate() {
                            if Some(index) != selected && position.is_finite() {
                                paint_marker(space, position, foreground, background, window);
                            }
                        }
                        if !marker_hidden
                            && let Some(position) = selected.and_then(|index| positions.get(index))
                            && position.is_finite()
                        {
                            paint_marker(space, *position, primary, foreground, window);
                        }
                    }
                    if disabled {
                        return;
                    }
                    let focus = drag_focus.clone();
                    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                        if phase == DispatchPhase::Capture && focus.is_focused(window) {
                            let _ = drag_owner.update(cx, |view, cx| {
                                if !view.closed
                                    && !view.is_busy()
                                    && let Some(editor) = view.inspector.legacy_gradient.as_mut()
                                    && editor.focus == focus
                                    && editor.drag_strip(event, window, cx)
                                {
                                    cx.notify();
                                }
                            });
                        }
                    });
                    window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                        if phase == DispatchPhase::Capture && event.button == MouseButton::Left {
                            let _ =
                                release_owner.update(cx, |view, cx| {
                                    if view.inspector.legacy_gradient.as_ref().is_some_and(
                                        |editor| {
                                            editor.focus == release_focus && editor.drag.is_some()
                                        },
                                    ) {
                                        view.update_legacy_gradient(
                                            &release_focus,
                                            |editor, window, cx| {
                                                editor.finish_drag(event.position.y, window, cx)
                                            },
                                            window,
                                            cx,
                                        );
                                    }
                                });
                        }
                    });
                },
            )
            .size_full();
        div()
            .id(RAMP_ID)
            .test_support()
            .role(gpui_kit::Role::Group)
            .aria_label("Stepped color gradient. Click to add a stop. Drag a marker to move it or drag it away to remove it. Use Left and Right to nudge or Delete to remove the selected stop.")
            .track_focus(&focus)
            .tab_stop(!disabled)
            .key_context("LegacyColorGradient")
            .h(px(48.))
            .w_full()
            .rounded(theme.radius)
            .border_1()
            .border_color(border)
            .focus(|style| style.border_color(primary))
            .when(!disabled, |strip| {
                strip.cursor_crosshair()
                    .on_mouse_down(MouseButton::Left, cx.listener(move |view, event: &gpui_kit::MouseDownEvent, window, cx| {
                        if let Some(space) = space.get() {
                            view.update_legacy_gradient(&press_focus,
                                |editor, window, cx| editor.press_strip(space, event.position, window, cx), window, cx);
                        }
                    }))
                    .on_action(cx.listener(move |view, _: &RemoveLegacyStop, window, cx| {
                        view.update_legacy_gradient(&delete_focus,
                            |editor, window, cx| if editor.selected.is_some() { editor.action(Action::Remove, window, cx) } else { Ok(()) }, window, cx);
                        cx.stop_propagation();
                    }))
                    .on_action(cx.listener(move |view, _: &NudgeLegacyStopLeft, window, cx| {
                        view.update_legacy_gradient(&left_focus,
                            |editor, window, cx| editor.nudge(-1., window, cx), window, cx);
                        cx.stop_propagation();
                    }))
                    .on_action(cx.listener(move |view, _: &NudgeLegacyStopRight, window, cx| {
                        view.update_legacy_gradient(&right_focus,
                            |editor, window, cx| editor.nudge(1., window, cx), window, cx);
                        cx.stop_propagation();
                    }))
            })
            .child(surface)
            .into_any_element()
    }
}

fn paint_ramp(
    space: StripSpace,
    samples: &[u32; STRIP_WIDTH],
    (background, muted): (Hsla, Hsla),
    window: &mut Window,
) {
    let bounds = space.0;
    let top = space.marker_bottom() + px(2.);
    let height = bounds.bottom() - top;
    if height <= px(0.) {
        return;
    }
    let tile = height / 2.;
    let columns = (bounds.size.width / tile).ceil() as usize;
    for row in 0..2 {
        for column in 0..columns {
            let x = tile * column;
            window.paint_quad(fill(
                Bounds::new(
                    point(bounds.left() + x, top + tile * row),
                    size(tile.min(bounds.size.width - x), tile),
                ),
                if (column + row) % 2 == 0 {
                    background
                } else {
                    muted
                },
            ));
        }
    }
    let width = bounds.size.width / STRIP_WIDTH as f32;
    for (column, color) in samples.iter().enumerate() {
        window.paint_quad(fill(
            Bounds::new(
                point(bounds.left() + width * column, top),
                size(width, height),
            ),
            gpui_kit::rgba(*color),
        ));
    }
}

fn paint_marker(
    space: StripSpace,
    position: f64,
    fill_color: Hsla,
    edge: Hsla,
    window: &mut Window,
) {
    let center = space.marker_x(position);
    let y = space.0.top();
    window.paint_quad(fill(
        Bounds::new(point(center - px(5.), y), size(px(10.), px(10.))),
        edge,
    ));
    window.paint_quad(fill(
        Bounds::new(point(center - px(3.), y + px(2.)), size(px(6.), px(6.))),
        fill_color,
    ));
    window.paint_quad(fill(
        Bounds::new(point(center - px(1.), y + px(10.)), size(px(2.), px(4.))),
        edge,
    ));
}
