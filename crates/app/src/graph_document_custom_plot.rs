//! In-place editing of the selected Custom layer inside a mapping draft.
use super::*;
use crate::curve_axes::{CurveAxes, PLOT_INSET};
use crate::custom_curve_plot::{
    CurvePlotColors, PlotSpace, PointDrag, SCROLL_INTERVAL, fitted_range, paint_custom_plot,
};
use gpui_kit::{
    DispatchPhase, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, canvas, px,
};
use std::{cell::Cell, rc::Rc};

const PLOT_ID: &str = "mapping-custom-plot";

impl MappingEditor {
    fn press_point_plot(
        &mut self,
        layer: &Layer,
        space: PlotSpace,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if space.area.size.width <= px(0.) || space.area.size.height <= px(0.) {
            return;
        }
        let result = (|| {
            let fields = self
                .layer
                .as_mut()
                .filter(|fields| fields.layer.same_reference(layer))
                .ok_or("The selected Custom layer changed.")?;
            let draft = fields
                .custom
                .as_mut()
                .ok_or("The selected Custom layer changed.")?;
            fields.point_drag = None;
            if draft.is_changed(cx) {
                return Err("Use Add point or Move point for the typed coordinates first.".into());
            }
            let hit = draft
                .points()
                .iter()
                .filter_map(|entry| {
                    let position = entry.position();
                    if !position.into_iter().all(f32::is_finite) {
                        return None;
                    }
                    let delta = space.to_screen(position) - event.position;
                    let [dx, dy] = [f32::from(delta.x), f32::from(delta.y)];
                    let distance = dx * dx + dy * dy;
                    (distance <= 36.).then_some((entry, distance))
                })
                .min_by(|left, right| left.1.total_cmp(&right.1));
            match hit {
                Some((entry, _)) => {
                    draft.apply(layer, CustomAction::Select(entry.clone()), window, cx)?;
                    if event.click_count == 2 {
                        return draft.apply(layer, CustomAction::Remove, window, cx);
                    }
                }
                None if event.click_count < 2 => {
                    let [x, y] = space.to_curve(event.position);
                    let [low, high] = space.vertical;
                    let position = [x.clamp(0., 1.), y.clamp(low, high)];
                    draft.apply(layer, CustomAction::AddAt(position), window, cx)?;
                }
                None => return Ok(()),
            }
            if let Some(selected) = draft.selected_point() {
                fields.point_drag = Some(PointDrag::new(
                    space,
                    event.position,
                    space.to_screen(selected.position()) - event.position,
                ));
            }
            Ok::<_, String>(())
        })();
        self.error = result.err();
        cx.notify();
    }

    fn drag_point_plot(
        &mut self,
        layer: &Layer,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(fields) = self
            .layer
            .as_mut()
            .filter(|fields| fields.layer.same_reference(layer))
        else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) || !self.plot_focus.is_focused(window) {
            if fields.point_drag.take().is_some() {
                cx.notify();
            }
            return;
        }
        let Some(drag) = &mut fields.point_drag else {
            return;
        };
        drag.move_pointer(event.position);
        if !drag.moved {
            return;
        }
        if drag.scroll_edge().is_none() {
            drag.scroll = None;
        } else if drag.scroll.is_none() {
            let layer = layer.clone();
            drag.scroll = Some(cx.spawn_in(window, async move |owner, cx| {
                loop {
                    cx.background_executor().timer(SCROLL_INTERVAL).await;
                    let keep_scrolling = owner.update_in(cx, |editor, window, cx| {
                        editor.scroll_point_plot(&layer, window, cx)
                    });
                    if !matches!(keep_scrolling, Ok(true)) {
                        break;
                    }
                }
            }));
        }
        if self.update_dragged_point(window, cx) {
            cx.notify();
        }
    }

    fn scroll_point_plot(
        &mut self,
        layer: &Layer,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(fields) = self
            .layer
            .as_mut()
            .filter(|fields| fields.layer.same_reference(layer))
        else {
            return false;
        };
        if !self.plot_focus.is_focused(window) || !window.is_window_active() {
            fields.point_drag = None;
            cx.notify();
            return false;
        }
        let Some(drag) = &mut fields.point_drag else {
            return false;
        };
        let Some(edge) = drag.scroll_edge() else {
            drag.scroll = None;
            return false;
        };
        edge.expand(&mut drag.space.vertical);
        let keep_scrolling = drag.scroll_edge().is_some();
        if !keep_scrolling {
            drag.scroll = None;
        }
        self.update_dragged_point(window, cx);
        cx.notify();
        keep_scrolling
            && self
                .layer
                .as_ref()
                .is_some_and(|fields| fields.point_drag.is_some())
    }

    fn update_dragged_point(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(fields) = &mut self.layer else {
            return false;
        };
        let (Some(drag), Some(draft)) = (&fields.point_drag, &mut fields.custom) else {
            return false;
        };
        let layer = &fields.layer;
        let result = (|| {
            let looped = layer.is_custom_looped().map_err(model::failure)?;
            let position = drag.position(looped);
            if draft
                .selected_point()
                .is_some_and(|point| point.position() == position)
            {
                return Ok(false);
            }
            draft.apply(layer, CustomAction::MoveTo(position), window, cx)?;
            Ok::<_, String>(true)
        })();
        match result {
            Ok(changed) => changed,
            Err(error) => {
                self.error = Some(error);
                fields.point_drag = None;
                true
            }
        }
    }

    pub(super) fn render_custom_plot(
        &self,
        fields: &LayerFields,
        draft: &CustomDraft,
        cx: &Context<Self>,
    ) -> AnyElement {
        let layer = fields.layer.clone();
        let points: Vec<_> = draft
            .points()
            .iter()
            .filter_map(|entry| {
                let position = entry.position();
                position.into_iter().all(f32::is_finite).then_some((
                    position,
                    draft
                        .selected_point()
                        .is_some_and(|selected| selected.same_reference(entry)),
                ))
            })
            .collect();
        let samples: Option<Vec<_>> = (0..=96)
            .map(|index| {
                let x = index as f32 / 96.;
                let y = layer.sample_custom(x).ok()? * 100.;
                y.is_finite().then_some([x, y])
            })
            .collect();
        let vertical = fields
            .point_drag
            .as_ref()
            .map_or_else(|| fitted_range(&points), |drag| drag.space.vertical);
        let axes = CurveAxes::percent(vertical);
        let output_guides = axes.output_guides().to_vec();
        let geometry = Rc::new(Cell::new(None));
        let paint_geometry = geometry.clone();
        let drag_owner = cx.weak_entity();
        let drag_layer = layer.clone();
        let remove_layer = layer.clone();
        let focus = self.plot_focus.clone();
        let theme = cx.theme();
        let stroke = theme.primary;
        let border = theme.border;
        let marker = theme.foreground;
        let colors = CurvePlotColors {
            stroke,
            grid: border,
            marker,
            outside: theme.muted,
        };
        let canvas = canvas(
            move |bounds, _, _| {
                let space = PlotSpace {
                    area: bounds.inset(PLOT_INSET),
                    vertical,
                };
                paint_geometry.set(Some(space));
                space
            },
            move |_, space, window, _| {
                paint_custom_plot(
                    space,
                    &output_guides,
                    &points,
                    samples.as_deref(),
                    &colors,
                    window,
                );
                // During a drag the pointer can cross the plot without dropping capture.
                let owner = drag_owner.clone();
                let layer = drag_layer.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if phase == DispatchPhase::Capture {
                        let _ = owner.update(cx, |editor, cx| {
                            editor.drag_point_plot(&layer, event, window, cx)
                        });
                    }
                });
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture && event.button == MouseButton::Left {
                        let _ = drag_owner.update(cx, |editor, cx| {
                            if let Some(fields) = &mut editor.layer
                                && fields.layer.same_reference(&drag_layer)
                                && fields.point_drag.take().is_some()
                            {
                                cx.notify();
                            }
                        });
                    }
                });
            },
        )
        .size_full();
        let plot = div().id(PLOT_ID)
            .test_support()
            .role(gpui_kit::Role::Group)
            .aria_label("Custom curve plot. Click to add or select a point. Drag to move. Hold near the top or bottom to extend the range. Delete removes the selected point.")
            .track_focus(&self.plot_focus)
            .key_context("CustomCurvePlot")
            .size_full()
            .border_1()
            .border_color(border)
            .focus(|style| style.border_color(stroke))
            .cursor_crosshair()
            .overflow_hidden()
            .on_mouse_down(MouseButton::Left, cx.listener(move |editor, event, window, cx| {
                if let Some(space) = geometry.get() {
                    focus.focus(window, cx);
                    editor.press_point_plot(&layer, space, event, window, cx);
                }
            }))
            .on_action(cx.listener(move |editor, _: &RemoveCustomCurvePoint, window, cx| {
                editor.custom_action(&remove_layer, CustomAction::Remove, window, cx);
                cx.stop_propagation();
            }))
            .child(canvas);
        axes.frame(plot, px(1.), cx).h(rems(9.)).into_any_element()
    }
}
