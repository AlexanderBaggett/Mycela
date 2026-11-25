//! Levels editing within the selected-node inspector.
use super::{
    GraphDocumentView,
    model::{self, Node, Ticket, failure},
};
use crate::ForegroundError;
use gpui_kit::base::{Disableable, Selectable, StyledExt, TestSupportExt};
use gpui_kit::component::{
    ActiveTheme, Sizable,
    button::{Button, ButtonVariants},
    form::Field,
    input::{Input, InputEvent, InputState},
};
use gpui_kit::{
    App, AppContext, Bounds, Context, DispatchPhase, Entity, EventEmitter, FocusHandle,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Render,
    Subscription, Task, WeakEntity, Window, canvas, div, fill, point, prelude::*, px, rems, size,
};
use mycela_engine::{
    EnvironmentResourceLibrary, ImageGeometry, LevelsChannel, LevelsControlEdit, LevelsDraft,
    LevelsInputHistogram, LevelsPoints, PointwiseParameter,
};
use std::{cell::Cell, rc::Rc, sync::Arc};

#[path = "graph_document_levels_batch.rs"]
mod batch;
#[path = "graph_document_levels_drag.rs"]
mod gesture;

const HISTOGRAM_SIZE: i32 = 168;
const CHANNELS: [(LevelsChannel, &str, &str); 4] = [
    (LevelsChannel::Rgb, "levels-channel-rgb", "RGB"),
    (LevelsChannel::Red, "levels-channel-red", "Red"),
    (LevelsChannel::Green, "levels-channel-green", "Green"),
    (LevelsChannel::Blue, "levels-channel-blue", "Blue"),
];

#[derive(Clone, Copy)]
enum PointControl {
    BlackPoint,
    Gamma,
    WhitePoint,
}

impl PointControl {
    const ALL: [Self; 3] = [Self::BlackPoint, Self::Gamma, Self::WhitePoint];

    fn label(self) -> &'static str {
        match self {
            Self::BlackPoint => "Black point",
            Self::Gamma => "Gamma",
            Self::WhitePoint => "White point",
        }
    }

    fn id(self) -> &'static str {
        match self {
            Self::BlackPoint => "levels-black-point",
            Self::Gamma => "levels-gamma",
            Self::WhitePoint => "levels-white-point",
        }
    }

    fn value(self, points: LevelsPoints) -> f32 {
        match self {
            Self::BlackPoint => points.black_point(),
            Self::Gamma => points.gamma(),
            Self::WhitePoint => points.white_point(),
        }
    }

    fn position(self, points: LevelsPoints) -> f32 {
        match self {
            Self::Gamma => points.midpoint(),
            _ => self.value(points),
        }
    }

    fn edit(self, value: f32) -> LevelsControlEdit {
        match self {
            Self::BlackPoint => LevelsControlEdit::BlackPoint(value),
            Self::Gamma => LevelsControlEdit::Gamma(value),
            Self::WhitePoint => LevelsControlEdit::WhitePoint(value),
        }
    }

    fn drag_edit(self, position: f32) -> LevelsControlEdit {
        match self {
            Self::Gamma => LevelsControlEdit::Midpoint(position),
            _ => self.edit(position),
        }
    }
}

#[derive(Clone, Copy)]
struct Plot {
    bounds: Bounds<Pixels>,
    extent: [f32; 2],
}

impl Plot {
    fn x(self, value: f32) -> Pixels {
        let [low, high] = self.extent;
        self.bounds.left() + self.bounds.size.width * ((value - low) / (high - low)).clamp(0., 1.)
    }

    fn position(self, x: Pixels) -> f32 {
        let [low, high] = self.extent;
        let ratio = ((x - self.bounds.left()) / self.bounds.size.width).clamp(0., 1.);
        low + ratio * (high - low)
    }
}

struct Drag {
    control: PointControl,
    plot: Plot,
    pointer: Pixels,
    start: LevelsPoints,
    request: Option<(LevelsControlEdit, LevelsPoints)>,
    range_task: Option<Task<()>>,
    preview: Option<gesture::DraftPreview>,
}

struct SampleRequest {
    ticket: Ticket,
    _task: Task<()>,
}

impl Drop for SampleRequest {
    fn drop(&mut self) {
        self.ticket.retire();
    }
}

pub(crate) struct LevelsEditor {
    parameter: PointwiseParameter<ForegroundError>,
    channel: LevelsChannel,
    points: LevelsPoints,
    inputs: [Entity<InputState>; 3],
    _subscriptions: Vec<Subscription>,
    chart: Option<Arc<LevelsInputHistogram>>,
    sample: Option<SampleRequest>,
    sample_status: Option<String>,
    error: Option<String>,
    host: Option<WeakEntity<GraphDocumentView>>,
    plot_focus: FocusHandle,
    drag: Option<Drag>,
    batch: Option<batch::Batch>,
}

enum LevelsEditorEvent {
    Commit {
        channel: LevelsChannel,
        edit: LevelsControlEdit,
    },
    DraftChanged,
}

impl EventEmitter<LevelsEditorEvent> for LevelsEditor {}

impl LevelsEditor {
    pub(crate) fn create(
        parameter: PointwiseParameter<ForegroundError>,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<Entity<Self>, String> {
        let histogram = parameter.histogram().map_err(failure)?;
        let selected = usize::try_from(histogram.selected_channel()).unwrap_or(0);
        let channel = LevelsChannel::ALL
            .get(selected)
            .copied()
            .unwrap_or(LevelsChannel::Rgb);
        let points = histogram.levels_points(channel).map_err(failure)?;
        Ok(cx.new(|cx| Self::new(parameter, channel, points, window, cx)))
    }

    fn new(
        parameter: PointwiseParameter<ForegroundError>,
        channel: LevelsChannel,
        points: LevelsPoints,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let inputs = PointControl::ALL.map(|control| {
            cx.new(|cx| InputState::new(window, cx).default_value(display(control.value(points))))
        });
        let subscriptions = PointControl::ALL
            .into_iter()
            .zip(&inputs)
            .enumerate()
            .map(|(index, (control, input))| {
                cx.subscribe_in(input, window, move |editor, _, event, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                        editor.commit_field(index, control, window, cx);
                    }
                })
            })
            .collect();
        Self {
            parameter,
            channel,
            points,
            inputs,
            _subscriptions: subscriptions,
            chart: None,
            sample: None,
            sample_status: None,
            error: None,
            host: None,
            plot_focus: cx.focus_handle(),
            drag: None,
            batch: None,
        }
    }

    pub(super) fn parameter(&self) -> &PointwiseParameter<ForegroundError> {
        &self.parameter
    }

    pub(super) fn did_commit(
        &mut self,
        result: Result<(), String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.error = result.err();
        let error = self.sync_fields(window, cx).err();
        if self.error.is_none() {
            self.error = error;
        }
        cx.notify();
    }

    pub(super) fn attach_host(&mut self, host: WeakEntity<GraphDocumentView>) {
        self.host = Some(host);
    }

    fn is_disabled(&self, cx: &App) -> bool {
        self.host
            .as_ref()
            .and_then(WeakEntity::upgrade)
            .is_some_and(|host| {
                let view = host.read(cx);
                view.closed || view.work.is_some() || view.modal != self.batch.is_some()
            })
    }

    fn commit_field(
        &mut self,
        index: usize,
        control: PointControl,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = self.inputs[index].read(cx).value();
        if text == display(control.value(self.points)) {
            return;
        }
        match text.trim().parse::<f32>() {
            Ok(value) if value.is_finite() => self.request_commit(control.edit(value), window, cx),
            _ => {
                self.error = Some(format!(
                    "Enter a finite value for {}.",
                    control.label().to_lowercase()
                ));
                cx.notify();
            }
        }
    }

    fn request_commit(
        &mut self,
        edit: LevelsControlEdit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_disabled(cx) {
            return;
        }
        if self.batch.is_some() {
            self.edit_batch(edit, window, cx);
            return;
        }
        match self.points.edited(edit) {
            Ok(_) => {
                self.error = None;
                cx.emit(LevelsEditorEvent::Commit {
                    channel: self.channel,
                    edit,
                });
            }
            Err(_) => {
                self.error = Some(
                    "These Levels points are invalid. Reset this channel to restore its range."
                        .into(),
                );
            }
        }
        cx.notify();
    }

    fn set_channel(&mut self, channel: LevelsChannel, window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_disabled(cx) && self.channel != channel && self.stage_batch_fields(cx) {
            self.channel = channel;
            self.cancel_drag(cx);
            if self.batch.is_none()
                && let Ok(histogram) = self.parameter.histogram()
                && let Some(index) = LevelsChannel::ALL
                    .iter()
                    .position(|&entry| entry == channel)
            {
                histogram.set_channel(index as i32);
            }
            self.error = self.sync_fields(window, cx).err();
            cx.notify();
        }
    }

    fn sync_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        self.points = if let Some(batch) = &self.batch {
            batch.draft.points(self.channel)
        } else {
            self.parameter
                .histogram()
                .map_err(failure)?
                .levels_points(self.channel)
                .map_err(failure)?
        };
        for (control, input) in PointControl::ALL.into_iter().zip(&self.inputs) {
            let value = display(control.value(self.points));
            input.update(cx, |input, cx| input.set_value(value, window, cx));
        }
        Ok(())
    }

    pub(super) fn refresh_input(
        &mut self,
        input: Option<Node>,
        time: f64,
        library: EnvironmentResourceLibrary,
        cx: &mut Context<Self>,
    ) {
        self.cancel_drag(cx);
        self.sample = None;
        self.chart = None;
        let Some(input) = input.filter(|node| !node.is_default_input()) else {
            self.sample_status = Some("Connect an input to see its histogram.".into());
            cx.notify();
            return;
        };
        let pending = match model::freeze(&input, time) {
            Ok(pending) => pending,
            Err(error) => {
                self.sample_status = Some(format!("Couldn’t sample the Levels input. {error}"));
                cx.notify();
                return;
            }
        };
        self.sample_status = Some("Sampling input…".into());
        let ticket = Ticket::new();
        let worker = ticket.clone();
        let publication = ticket.clone();
        let task = cx.spawn(async move |owner, cx| {
            let result = cx
                .background_spawn(async move {
                    let rendered = crate::scene_backend::render_export(
                        &pending.materialize()?,
                        ImageGeometry::new(HISTOGRAM_SIZE, HISTOGRAM_SIZE),
                        &|| !worker.is_current(),
                        crate::scene_backend::GRAPH_PNG_OPTIONS,
                        None,
                        None,
                        Some(&library),
                    )
                    .map_err(failure)?;
                    let result = LevelsInputHistogram::from_image(&rendered).map_err(failure);
                    rendered.dispose();
                    result
                })
                .await;
            if publication.is_current() {
                let _ = owner.update(cx, |editor, cx| {
                    editor.sample = None;
                    match result {
                        Ok(chart) => {
                            if chart.auto_points(LevelsChannel::Rgb).is_none() {
                                editor.sample_status =
                                    Some("The input has no finite RGB samples.".into());
                            } else {
                                editor.chart = Some(Arc::new(chart));
                                editor.sample_status = None;
                            }
                        }
                        Err(error) => {
                            editor.sample_status =
                                Some(format!("Couldn’t sample the Levels input. {error}"))
                        }
                    }
                    cx.notify();
                });
            }
        });
        self.sample = Some(SampleRequest {
            ticket,
            _task: task,
        });
        cx.notify();
    }

    fn begin_drag(&mut self, plot: Plot, pointer: Pixels, cx: &mut Context<Self>) {
        if self.is_disabled(cx) || plot.bounds.size.width <= px(0.) {
            return;
        }
        let start = self.points;
        let control = PointControl::ALL
            .into_iter()
            .filter(|control| control.position(start).is_finite())
            .map(|control| {
                (
                    control,
                    f32::from(plot.x(control.position(start)) - pointer).abs(),
                )
            })
            .min_by(|left, right| left.1.total_cmp(&right.1))
            .filter(|(_, distance)| *distance <= 8.);
        if let Some((control, _)) = control {
            self.drag = Some(Drag {
                control,
                plot,
                pointer,
                start,
                request: None,
                range_task: None,
                preview: None,
            });
            cx.notify();
        }
    }

    fn move_drag(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if event.pressed_button == Some(MouseButton::Left) {
            self.update_drag(event.position.x, cx);
        } else {
            self.cancel_drag(cx);
        }
    }

    fn cancel_drag(&mut self, cx: &mut Context<Self>) -> bool {
        if self.drag.take().is_none() {
            return false;
        }
        if let Some(batch) = &self.batch {
            self.queue_batch_preview(batch.draft, cx);
        } else {
            cx.emit(LevelsEditorEvent::DraftChanged);
        }
        cx.notify();
        true
    }

    fn finish_drag(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(drag) = self.drag.take() {
            let request = drag.request;
            drop(drag);
            cx.emit(LevelsEditorEvent::DraftChanged);
            if let Some((edit, _)) = request {
                self.request_commit(edit, window, cx);
            }
            cx.notify();
        }
    }

    fn render_histogram(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let plot = Rc::new(Cell::new(None));
        let layout = plot.clone();
        let owner = cx.weak_entity();
        let drag = owner.clone();
        let mut extent = self.chart.as_ref().map_or([0., 1.], |chart| chart.bounds());
        for control in PointControl::ALL {
            let position = control.position(self.points);
            if position.is_finite() {
                extent[0] = extent[0].min(position.clamp(-10., 10.));
                extent[1] = extent[1].max(position.clamp(-10., 10.));
            }
        }
        if let Some(drag) = &self.drag {
            extent = drag.plot.extent;
        }
        let histogram = self.chart.clone();
        let channel = self.channel;
        let points = self
            .drag
            .as_ref()
            .and_then(|drag| drag.request.map(|(_, points)| points))
            .unwrap_or(self.points);
        let theme = cx.theme();
        let brush = theme.primary;
        let marker = theme.foreground;
        div()
            .id("levels-histogram")
            .test_support()
            .track_focus(&self.plot_focus)
            .role(gpui_kit::Role::Group)
            .aria_label("Levels input histogram")
            .h(rems(7.))
            .border_1()
            .border_color(theme.border)
            .overflow_hidden()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |editor, mouse: &MouseDownEvent, window, cx| {
                    if let Some(plot) = plot.get() {
                        editor.plot_focus.focus(window, cx);
                        editor.begin_drag(plot, mouse.position.x, cx);
                    }
                }),
            )
            .on_key_down(cx.listener(|editor, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" && editor.cancel_drag(cx) {
                    cx.stop_propagation();
                }
            }))
            .child(
                canvas(
                    move |bounds, _, _| {
                        let plot = Plot { bounds, extent };
                        layout.set(Some(plot));
                        plot
                    },
                    move |_, plot, window, _| {
                        if let Some(histogram) = &histogram {
                            let [low, high] = histogram.bounds();
                            let bins = histogram.bins(channel);
                            let step = (high - low) / (bins.len() - 1) as f32;
                            let maximum = bins.iter().copied().fold(0., f32::max);
                            if maximum > 0. {
                                for (index, &count) in bins.iter().enumerate() {
                                    let left = plot.x(low + step * index as f32);
                                    let right = plot.x(low + step * (index + 1) as f32);
                                    let height = plot.bounds.size.height * (count / maximum);
                                    if height > px(0.) {
                                        window.paint_quad(fill(
                                            Bounds::new(
                                                point(left, plot.bounds.bottom() - height),
                                                size((right - left).max(px(1.)), height),
                                            ),
                                            brush,
                                        ));
                                    }
                                }
                            }
                        }
                        for control in PointControl::ALL {
                            let position = control.position(points);
                            if position.is_finite() {
                                let x = plot.x(position).min(plot.bounds.right() - px(1.));
                                window.paint_quad(fill(
                                    Bounds::new(
                                        point(x, plot.bounds.top()),
                                        size(px(1.), plot.bounds.size.height),
                                    ),
                                    marker,
                                ));
                            }
                        }
                        let drag = drag.clone();
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                            if phase == DispatchPhase::Capture {
                                let _ = drag.update(cx, |editor, cx| editor.move_drag(event, cx));
                            }
                        });
                        let owner = owner.clone();
                        window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                            if phase == DispatchPhase::Capture && event.button == MouseButton::Left
                            {
                                let _ =
                                    owner.update(cx, |editor, cx| editor.finish_drag(window, cx));
                            }
                        });
                    },
                )
                .size_full(),
            )
    }
}

impl Render for LevelsEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let auto = self
            .chart
            .as_ref()
            .and_then(|chart| chart.auto_points(self.channel));
        let disabled = self.is_disabled(cx);
        let selected = self.channel;
        div()
            .v_flex()
            .gap_2()
            .child(div().h_flex().flex_wrap().gap_1().children(CHANNELS.map(
                |(channel, id, label)| {
                    Button::new(id)
                        .small()
                        .label(label)
                        .selected(channel == selected)
                        .disabled(disabled)
                        .on_click(cx.listener(move |editor, _, window, cx| {
                            editor.set_channel(channel, window, cx);
                        }))
                },
            )))
            .child(self.render_histogram(cx))
            .when_some(self.sample_status.clone(), |group, status| {
                group.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(status),
                )
            })
            .children(
                PointControl::ALL
                    .into_iter()
                    .zip(&self.inputs)
                    .map(|(control, input)| {
                        Field::new().label(control.label()).child(
                            Input::new(input)
                                .id(control.id())
                                .aria_label(control.label())
                                .small()
                                .disabled(disabled),
                        )
                    }),
            )
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(
                        Button::new("levels-auto")
                            .small()
                            .ghost()
                            .label("Auto levels")
                            .disabled(disabled || auto.is_none())
                            .on_click(cx.listener(move |editor, _, window, cx| {
                                if let Some(points) = auto {
                                    editor.request_commit(
                                        LevelsControlEdit::Auto(points),
                                        window,
                                        cx,
                                    );
                                }
                            })),
                    )
                    .child(
                        Button::new("levels-reset")
                            .small()
                            .ghost()
                            .label("Reset levels")
                            .disabled(disabled)
                            .on_click(cx.listener(|editor, _, window, cx| {
                                editor.request_commit(LevelsControlEdit::Reset, window, cx)
                            })),
                    ),
            )
            .when(self.batch.is_none(), |group| {
                group.child(
                    Button::new("levels-edit-channels")
                        .small()
                        .ghost()
                        .label("Edit channels…")
                        .disabled(disabled)
                        .on_click(
                            cx.listener(|editor, _, window, cx| editor.open_batch(window, cx)),
                        ),
                )
            })
            .children(self.batch_preview_panel(cx))
            .when_some(self.error.clone(), |group, error| {
                group.child(div().text_sm().text_color(cx.theme().danger).child(error))
            })
    }
}

impl GraphDocumentView {
    pub(super) fn attach_levels_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.levels_subscription = self.inspector.levels.as_ref().map(|levels| {
            let host = cx.weak_entity();
            levels.update(cx, |editor, _| editor.attach_host(host));
            cx.subscribe_in(levels, window, |document, editor, request, window, cx| {
                if document.closed
                    || document.is_busy()
                    || !document
                        .inspector
                        .levels
                        .as_ref()
                        .is_some_and(|current| current.entity_id() == editor.entity_id())
                {
                    return;
                }
                let &LevelsEditorEvent::Commit { channel, edit } = request else {
                    cx.notify();
                    return;
                };
                let parameter = editor.read(cx).parameter().clone();
                let result =
                    document.record_levels_edit(|| parameter.edit_levels(channel, edit), cx);
                editor.update(cx, |editor, cx| editor.did_commit(result, window, cx));
                cx.notify();
            })
        });
        self.refresh_levels_input(cx);
    }

    pub(super) fn refresh_levels_input(&self, cx: &mut Context<Self>) {
        if let Some(editor) = &self.inspector.levels {
            let input = editor
                .read(cx)
                .parameter()
                .owner()
                .node()
                .slot(0)
                .ok()
                .and_then(|slot| slot.node());
            let time = self.sample_time;
            let library = self.environment_library.clone();
            editor.update(cx, |editor, cx| {
                editor.refresh_input(input, time, library, cx)
            });
        }
    }
}

fn display(value: f32) -> String {
    if value.is_finite() {
        value.to_string()
    } else {
        String::new()
    }
}

#[cfg(test)]
#[path = "graph_document_levels_tests.rs"]
mod tests;
