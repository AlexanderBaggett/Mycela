//! Pointer editing of a retained Custom layer. Only release publishes the edit.
use super::super::inspector::LayerSelection;
use super::{
    GraphDocumentView, RemoveCustomCurvePoint,
    animation::AnimationTarget,
    custom::{CustomAction, CustomDraft, PlotAction},
};
use crate::ForegroundError;
use crate::curve_axes::{CurveAxes, PLOT_INSET};
use crate::custom_curve_plot::{
    CurvePlotColors, PlotSpace, PointDrag, SCROLL_INTERVAL, fitted_range, paint_custom_plot,
};
use gpui_kit::base::TestSupportExt;
use gpui_kit::component::ActiveTheme;
use gpui_kit::{
    AnyElement, Context, DispatchPhase, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Pixels, Point, Window, canvas, div, point, prelude::*, px, rems,
};
use mycela_engine::CurveLayerHandle;
use std::{cell::Cell, rc::Rc};

type Layer = CurveLayerHandle<ForegroundError>;

pub(super) struct CurvePlotDrag {
    selection: LayerSelection,
    target: AnimationTarget,
    draft: CustomDraft,
    sample: Layer,
    pointer: PointDrag,
    hit: Option<usize>,
    start: [f32; 2],
    looped: bool,
    remove: bool,
}

impl CurvePlotDrag {
    fn action(&self) -> CustomAction {
        match self.hit {
            Some(index) if self.remove => CustomAction::Plot(PlotAction::Remove(index)),
            Some(index) if self.pointer.moved => CustomAction::Plot(PlotAction::Move {
                index,
                position: self.pointer.position(self.looped),
            }),
            Some(index) => CustomAction::Select(index),
            None => CustomAction::Plot(PlotAction::Add {
                start: self.start,
                end: self
                    .pointer
                    .moved
                    .then(|| self.pointer.position(self.looped)),
            }),
        }
    }

    fn refresh_sample(&mut self) -> Result<(), String> {
        if let CustomAction::Plot(action) = self.action() {
            let sample = self
                .selection
                .layer
                .source_clone()
                .map_err(|e| e.to_string())?;
            action.apply(&sample)?;
            self.sample = sample;
        }
        Ok(())
    }
}

impl GraphDocumentView {
    fn press_custom_curve_plot(
        &mut self,
        selection: &LayerSelection,
        target: &AnimationTarget,
        draft: &CustomDraft,
        space: PlotSpace,
        event: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        if self.is_busy()
            || !self.is_original_selection(&target.selection, &target.node, target.revision)
            || space.area.size.width <= px(0.)
            || space.area.size.height <= px(0.)
        {
            return;
        }
        self.curve_plot_drag = None;
        if self.inspector.fields_changed(cx) || draft.is_changed(cx) {
            self.error = Some("Correct or reset field text before editing points.".into());
            cx.notify();
            return;
        }
        let result = (|| {
            selection.validate(None)?;
            draft.validate(&selection.layer)?;
            let sample = selection.layer.source_clone().map_err(|e| e.to_string())?;
            let looped = sample.is_custom_looped().map_err(|e| e.to_string())?;
            let screen = event.position;
            let [left, top] = space.to_curve(screen - point(px(6.), px(6.)));
            let [right, bottom] = space.to_curve(screen + point(px(7.), px(7.)));
            let start = space.to_curve(screen);
            let hit = if sample
                .select_custom_point([left, bottom, right - left, top - bottom], start)
                .map_err(|e| e.to_string())?
            {
                usize::try_from(sample.custom_selected_index().map_err(|e| e.to_string())?).ok()
            } else {
                None
            };
            let remove = event.click_count == 2;
            if remove && hit.is_none() {
                return Ok(None);
            }
            let offset = hit
                .and_then(|index| sample.custom_points().ok()?.entries().get(index).cloned())
                .map_or(point(px(0.), px(0.)), |point| {
                    space.to_screen(point.position()) - screen
                });
            let pointer = PointDrag::new(space, screen, offset);
            let mut drag = CurvePlotDrag {
                selection: selection.clone(),
                target: target.clone(),
                draft: draft.clone(),
                sample,
                start: pointer.position(false),
                pointer,
                hit,
                looped,
                remove,
            };
            drag.refresh_sample()?;
            Ok::<_, String>(Some(drag))
        })();
        match result {
            Ok(drag) => {
                self.curve_plot_drag = drag;
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
        cx.notify();
    }

    fn move_custom_curve_plot(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(mut drag) = self.curve_plot_drag.take() else {
            return;
        };
        if self.is_busy()
            || event.pressed_button != Some(MouseButton::Left)
            || !self.curve_plot_focus.is_focused(window)
        {
            cx.notify();
            return;
        }
        if drag.remove {
            self.curve_plot_drag = Some(drag);
            return;
        }
        drag.pointer.move_pointer(event.position);
        if drag.pointer.scroll_edge().is_none() {
            drag.pointer.scroll = None;
        } else if drag.pointer.scroll.is_none() {
            drag.pointer.scroll = Some(cx.spawn_in(window, async move |owner, cx| {
                loop {
                    cx.background_executor().timer(SCROLL_INTERVAL).await;
                    let keep_scrolling = owner.update_in(cx, |view, window, cx| {
                        view.scroll_custom_curve_plot(window, cx)
                    });
                    if !matches!(keep_scrolling, Ok(true)) {
                        break;
                    }
                }
            }));
        }
        self.refresh_custom_curve_plot(drag, cx);
    }

    fn scroll_custom_curve_plot(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(mut drag) = self.curve_plot_drag.take() else {
            return false;
        };
        if self.is_busy() || !self.curve_plot_focus.is_focused(window) || !window.is_window_active()
        {
            cx.notify();
            return false;
        }
        let Some(edge) = drag.pointer.scroll_edge() else {
            drag.pointer.scroll = None;
            self.curve_plot_drag = Some(drag);
            return false;
        };
        edge.expand(&mut drag.pointer.space.vertical);
        self.refresh_custom_curve_plot(drag, cx);
        self.curve_plot_drag.is_some()
    }

    fn refresh_custom_curve_plot(&mut self, mut drag: CurvePlotDrag, cx: &mut Context<Self>) {
        match drag.refresh_sample() {
            Ok(()) => self.curve_plot_drag = Some(drag),
            Err(error) => self.error = Some(error),
        }
        cx.notify();
    }

    fn release_custom_curve_plot(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(mut drag) = self.curve_plot_drag.take() else {
            return;
        };
        cx.notify();
        if !self.curve_plot_focus.is_focused(window) {
            return;
        }
        drag.pointer.move_pointer(position);
        self.graph_custom_action(
            &drag.selection,
            &drag.target,
            &drag.draft,
            drag.action(),
            window,
            cx,
        );
    }

    pub(super) fn render_custom_curve_plot(
        &self,
        selection: &LayerSelection,
        draft: &CustomDraft,
        target: &AnimationTarget,
        disabled: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let drag = self
            .curve_plot_drag
            .as_ref()
            .filter(|drag| drag.selection.same_path(selection));
        let layer = drag.map_or(&selection.layer, |drag| &drag.sample);
        let selected = layer.custom_selected_index().ok();
        let points: Vec<_> = layer
            .custom_points()
            .into_iter()
            .flat_map(|points| points.entries())
            .enumerate()
            .filter_map(|(index, point)| {
                let position = point.position();
                position
                    .into_iter()
                    .all(f32::is_finite)
                    .then_some((position, i32::try_from(index).ok() == selected))
            })
            .collect();
        let samples: Option<Vec<_>> = (0..=96)
            .map(|index| {
                let x = index as f32 / 96.;
                let y = layer.sample_custom(x).ok()? * 100.;
                y.is_finite().then_some([x, y])
            })
            .collect();
        let vertical =
            drag.map_or_else(|| fitted_range(&points), |drag| drag.pointer.space.vertical);
        let axes = CurveAxes::percent(vertical);
        let output_guides = axes.output_guides().to_vec();
        let geometry = Rc::new(Cell::new(None));
        let paint_geometry = geometry.clone();
        let drag_owner = cx.weak_entity();
        let theme = cx.theme();
        let stroke = if disabled {
            theme.muted_foreground
        } else {
            theme.primary
        };
        let grid = theme.border;
        let marker = if disabled {
            theme.muted_foreground
        } else {
            theme.foreground
        };
        let colors = CurvePlotColors {
            stroke,
            grid,
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
                let owner = drag_owner.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if phase == DispatchPhase::Capture {
                        let _ = owner.update(cx, |view, cx| {
                            view.move_custom_curve_plot(event, window, cx)
                        });
                    }
                });
                window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                    if phase == DispatchPhase::Capture && event.button == MouseButton::Left {
                        let _ = drag_owner.update(cx, |view, cx| {
                            view.release_custom_curve_plot(event.position, window, cx)
                        });
                    }
                });
            },
        )
        .size_full();
        let selection = selection.clone();
        let target = target.clone();
        let draft = draft.clone();
        let plot = div().id("graph-custom-plot")
            .test_support()
            .role(gpui_kit::Role::Group)
            .aria_label("Custom curve. Click to add or select. Drag to move. Delete removes the selected point.")
            .size_full()
            .border_1().border_color(grid)
            .overflow_hidden()
            .when(!disabled, |plot| {
                let (remove_selection, remove_target, remove_draft) = (selection.clone(), target.clone(), draft.clone());
                plot.track_focus(&self.curve_plot_focus)
                    .key_context("CustomCurvePlot")
                    .focus_visible(|style| style.border_color(stroke))
                    .cursor_crosshair()
                    .on_mouse_down(MouseButton::Left, cx.listener(move |view, event, window, cx| {
                        if let Some(space) = geometry.get() {
                            view.curve_plot_focus.focus(window, cx);
                            view.press_custom_curve_plot(&selection, &target, &draft, space, event, cx);
                        }
                    }))
                    .on_action(cx.listener(move |view, _: &RemoveCustomCurvePoint, window, cx| {
                        view.curve_plot_drag = None;
                        view.graph_custom_action(&remove_selection, &remove_target, &remove_draft, CustomAction::Remove, window, cx);
                        cx.stop_propagation();
                    }))
            })
            .child(canvas);
        axes.frame(plot, px(1.), cx)
            .id("graph-custom-axes")
            .h(rems(9.))
            .into_any_element()
    }
}
