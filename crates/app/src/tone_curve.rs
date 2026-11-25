//! Editing the actual retained Tone Curve property. Draft text and selected
//! endpoint are presentation state; SharedCurve remains the curve authority.
use crate::{
    ForegroundError,
    color_binding_effects::BindingRuntime,
    curve_axes::{CurveAxes, INPUT_TICKS, PLOT_INSET, output_fraction},
};
use gpui_kit::base::{Disableable, Selectable, StyledExt, TestSupportExt};
use gpui_kit::component::{
    ActiveTheme, Sizable,
    button::{Button, ButtonVariants},
    form::{Field, Form},
    input::{Input, InputState},
};
use gpui_kit::{
    AnyWindowHandle, App, AppContext, Context, Entity, FocusHandle, Focusable, IntoElement,
    PathBuilder, Render, Window, canvas, div, point, prelude::*, px,
};
use mycela_engine::{
    AnalyticCurveKind, CurveControlKind, CurveLayerHandle, CurveLayerKind, PointwiseParameter,
    PointwiseParameterValue, SharedCurve,
};
use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, Ordering},
};

gpui_kit::actions!(
    tone_curve,
    [
        ApplyToneCurve,
        RevertToneCurve,
        SelectBlackEndpoint,
        SelectWhiteEndpoint,
        RaiseEndpoint,
        LowerEndpoint
    ]
);
#[cfg(feature = "texture-workspace-test")]
pub(crate) fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        gpui_kit::KeyBinding::new("secondary-enter", ApplyToneCurve, Some("ToneCurveEditor")),
        gpui_kit::KeyBinding::new("escape", RevertToneCurve, Some("ToneCurveEditor")),
        gpui_kit::KeyBinding::new("left", SelectBlackEndpoint, Some("ToneCurvePoints")),
        gpui_kit::KeyBinding::new("right", SelectWhiteEndpoint, Some("ToneCurvePoints")),
        gpui_kit::KeyBinding::new("home", SelectBlackEndpoint, Some("ToneCurvePoints")),
        gpui_kit::KeyBinding::new("end", SelectWhiteEndpoint, Some("ToneCurvePoints")),
        gpui_kit::KeyBinding::new("up", RaiseEndpoint, Some("ToneCurvePoints")),
        gpui_kit::KeyBinding::new("down", LowerEndpoint, Some("ToneCurvePoints")),
    ]);
}
type Curve = SharedCurve<ForegroundError>;

fn retained_curve(parameter: &PointwiseParameter<ForegroundError>) -> Option<Curve> {
    match parameter.value() {
        PointwiseParameterValue::Curve(curve) => curve,
        _ => None,
    }
}
// Endpoints coincide with the real range bounds only for this simple Linear
// shape. Other stored layer/control arrangements remain intact and visible.
fn has_linear_endpoints(curve: &Curve) -> bool {
    let layers = curve.layers();
    let [Some(layer)] = layers.as_slice() else {
        return false;
    };
    if layer.kind() != CurveLayerKind::Linear || layer.mode() != 2 || curve.is_wrapped() {
        return false;
    }
    let Some(direction) = layer.shape_control() else {
        return false;
    };
    if direction.kind() != CurveControlKind::Direction || direction.enumeration().ok() != Some(0) {
        return false;
    }
    let Some(mirror) = layer.control_at(0).ok().flatten() else {
        return false;
    };
    if mirror.kind() != CurveControlKind::Toggle || mirror.toggle().ok() != Some(false) {
        return false;
    }
    // qy.c uses concrete Scalar controls for these slots. Modulated also
    // exposes scalar(), but is not accepted by the actual common_scalar cast.
    for (ix, expected) in [(1, 1.), (2, 0.), (3, 0.), (4, 0.), (10, 0.), (11, 100.)] {
        let Some(control) = layer.control_at(ix).ok().flatten() else {
            return false;
        };
        if control.kind() != CurveControlKind::Scalar || control.scalar().ok() != Some(expected) {
            return false;
        }
    }
    // Phase/amplitude/shift are always evaluated; active childless controls
    // fail in evaluate(). Remap X/Y are only neutral while inactive. Require
    // the actual constructor's simple arrangement, without rewriting it.
    for (ix, expected, mode) in [
        (5, 0., 0),
        (6, 100., 2),
        (7, 50., 2),
        (8, 0., 0),
        (9, 0., 2),
        (13, 100., 1),
    ] {
        let Some(control) = layer.control_at(ix).ok().flatten() else {
            return false;
        };
        if control.kind() != CurveControlKind::Modulated
            || control.scalar().ok() != Some(expected)
            || control.is_active()
            || control.retained_child().is_some()
            || control.modulation_mode() != Some(mode)
        {
            return false;
        }
    }
    let Some(blend) = layer.control_at(12).ok().flatten() else {
        return false;
    };
    if blend.kind() != CurveControlKind::Blend || blend.enumeration().ok() != Some(0) {
        return false;
    }
    true
}
fn curve_name(curve: &Curve) -> String {
    let layers = curve.layers();
    if layers.is_empty() {
        return "Empty curve · input passes through".into();
    }
    if let [Some(layer)] = layers.as_slice() {
        format!("{:?} curve", layer.kind())
    } else {
        format!("Curve with {} layers", layers.len())
    }
}
pub(crate) fn samples(curve: &Curve) -> Result<Vec<f32>, String> {
    (0..=64)
        .map(|ix| {
            let value = curve.sample(ix as f32 / 64.).map_err(|e| e.to_string())?;
            if value.is_finite() {
                Ok(value)
            } else {
                Err("This curve has a nonfinite output that the graph cannot display.".into())
            }
        })
        .collect()
}
pub(crate) fn graph(values: Vec<f32>, cx: &App) -> impl IntoElement {
    let low = values.iter().copied().fold(0.0f32, f32::min);
    let high = values.iter().copied().fold(1.0f32, f32::max);
    let axes = CurveAxes::normalized([low, high]);
    let output_guides = axes.output_guides().to_vec();
    let theme = cx.theme();
    let foreground = theme.primary;
    let grid = theme.border;
    let canvas = canvas(
        move |_, _, _| (values, output_guides),
        move |bounds, (values, output_guides), window, _| {
            let area = bounds.inset(PLOT_INSET);
            if area.size.width <= px(0.) || area.size.height <= px(0.) {
                return;
            }
            let position = |x, value| {
                point(
                    area.left() + area.size.width * x,
                    area.bottom() - area.size.height * output_fraction(value, [low, high]),
                )
            };
            let mut guides = PathBuilder::stroke(px(1.));
            for x in INPUT_TICKS {
                guides.move_to(position(x, low));
                guides.line_to(position(x, high));
            }
            for y in output_guides {
                guides.move_to(position(0., y));
                guides.line_to(position(1., y));
            }
            if let Ok(path) = guides.build() {
                window.paint_path(path, grid);
            }
            let mut path = PathBuilder::stroke(px(2.));
            for (ix, value) in values.iter().enumerate() {
                let position = position(ix as f32 / 64., *value);
                if ix == 0 {
                    path.move_to(position);
                } else {
                    path.line_to(position);
                }
            }
            if let Ok(path) = path.build() {
                window.paint_path(path, foreground);
            }
        },
    )
    .size_full();
    axes.frame(canvas, px(0.), cx)
}

enum Edit {
    Linear,
    Bounds([f32; 2]),
    Nudge(usize, f32),
}
/// Owns only the current filter's curve edit lifecycle and uncommitted fields.
/// Closing or changing the selected filter retires weak queued curve edits.
pub(crate) struct ToneCurveView {
    parameter: PointwiseParameter<ForegroundError>,
    curve: Curve,
    fields: [Entity<InputState>; 2],
    synced: [String; 2],
    selected_endpoint: usize,
    focus: FocusHandle,
    points_focus: FocusHandle,
    window: AnyWindowHandle,
    runtime: Weak<BindingRuntime>,
    active: Arc<AtomicBool>,
    pending: bool,
    error: Option<String>,
}
impl ToneCurveView {
    pub(crate) fn new(
        parameter: PointwiseParameter<ForegroundError>,
        curve: Curve,
        runtime: &Arc<BindingRuntime>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let synced = [curve.minimum().to_string(), curve.maximum().to_string()];
        let fields = std::array::from_fn(|ix| {
            cx.new(|cx| InputState::new(window, cx).default_value(synced[ix].clone()))
        });
        Self {
            parameter,
            curve,
            fields,
            synced,
            selected_endpoint: 0,
            focus: cx.focus_handle().tab_stop(true),
            points_focus: cx.focus_handle().tab_stop(true),
            window: window.window_handle(),
            runtime: Arc::downgrade(runtime),
            active: Arc::new(AtomicBool::new(true)),
            pending: false,
            error: None,
        }
    }
    pub(crate) fn preferred_focus(&self, cx: &App) -> FocusHandle {
        if has_linear_endpoints(&self.curve) {
            self.fields[0].read(cx).focus_handle(cx)
        } else {
            self.focus.clone()
        }
    }
    pub(crate) fn close(&mut self) {
        self.active.store(false, Ordering::Release);
        self.pending = false;
    }
    fn text(&self, cx: &App) -> [String; 2] {
        std::array::from_fn(|ix| self.fields[ix].read(cx).value().to_string())
    }
    fn replace_drafts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.synced = [
            self.curve.minimum().to_string(),
            self.curve.maximum().to_string(),
        ];
        for (field, value) in self.fields.iter().zip(&self.synced) {
            field.update(cx, |state, cx| state.set_value(value.clone(), window, cx));
        }
    }
    fn sync_clean_drafts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending || !self.is_current() || !has_linear_endpoints(&self.curve) {
            return;
        }
        let values = [
            self.curve.minimum().to_string(),
            self.curve.maximum().to_string(),
        ];
        let text = self.text(cx);
        for (ix, value) in values.into_iter().enumerate() {
            if text[ix] == self.synced[ix] && text[ix] != value {
                self.fields[ix].update(cx, |state, cx| state.set_value(value.clone(), window, cx));
            }
            self.synced[ix] = value;
        }
    }
    pub(crate) fn refresh_controls(&mut self, cx: &mut Context<Self>) {
        // Presentation completion has an App context, not an owning Window.
        // Resolve that real window later; retired children and closed windows
        // cannot update fields. Read current bounds at delivery, not a mirror.
        let handle = self.window;
        let active = Arc::downgrade(&self.active);
        cx.spawn(async move |owner, cx| {
            if !active
                .upgrade()
                .is_some_and(|marker| marker.load(Ordering::Acquire))
            {
                return;
            }
            let _ = cx.update_window(handle, |_, window, cx| {
                owner.update(cx, |view, cx| {
                    if !view.active.load(Ordering::Acquire) {
                        return;
                    }
                    view.sync_clean_drafts(window, cx);
                    cx.notify();
                })
            });
        })
        .detach();
        cx.notify();
    }
    fn is_current(&self) -> bool {
        retained_curve(&self.parameter).is_some_and(|curve| curve.same_reference(&self.curve))
    }
    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending {
            return;
        }
        match self.parameter.curve() {
            Ok(curve) => {
                self.curve = curve;
                self.error = None;
                self.replace_drafts(window, cx);
            }
            Err(error) => self.error = Some(error.to_string()),
        }
        cx.notify();
    }
    fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Also reconcile clean fields if Apply arrives before queued refresh.
        self.sync_clean_drafts(window, cx);
        let text = self.text(cx);
        let mut values = [0.0; 2];
        for (slot, text) in values.iter_mut().zip(&text) {
            let Ok(value) = text.parse::<f32>() else {
                self.error = Some("Both endpoint outputs need finite numbers.".into());
                cx.notify();
                return;
            };
            if !value.is_finite() {
                self.error = Some("Both endpoint outputs need finite numbers.".into());
                cx.notify();
                return;
            }
            *slot = value;
        }
        self.submit(Edit::Bounds(values), window, cx);
    }
    fn select_endpoint(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.selected_endpoint = ix;
        window.focus(&self.points_focus, cx);
        cx.notify();
    }
    fn nudge(&mut self, delta: f32, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.points_focus, cx);
        self.submit(Edit::Nudge(self.selected_endpoint, delta), window, cx);
    }
    fn submit(&mut self, edit: Edit, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending || !self.active.load(Ordering::Acquire) {
            return;
        }
        if !self.is_current() {
            self.error = Some("The curve changed. Reload the curve before editing.".into());
            cx.notify();
            return;
        }
        if !matches!(edit, Edit::Linear) && !has_linear_endpoints(&self.curve) {
            return;
        }
        let active = Arc::downgrade(&self.active);
        let runtime = self.runtime.clone();
        let parameter = self.parameter.clone();
        let curve = self.curve.clone();
        let submitted = self.text(cx);
        let creates_linear = matches!(&edit, Edit::Linear);
        let clean: [bool; 2] = std::array::from_fn(|ix| submitted[ix] == self.synced[ix]);
        let nudges_endpoint = matches!(&edit, Edit::Nudge(_, _));
        let handle = window.window_handle();
        self.pending = true;
        cx.notify();
        cx.spawn(async move |owner, cx| {
            let alive = cx
                .update_window(handle, |_, _, cx| owner.read_with(cx, |_, _| ()))
                .is_ok_and(|result| result.is_ok());
            if !alive
                || !active
                    .upgrade()
                    .is_some_and(|marker| marker.load(Ordering::Acquire))
            {
                return;
            }
            // Actual engine operations run without an App, Window or Entity
            // borrow. Source curve/layer identities and direct-write semantics
            // are preserved; no property Changed event is fabricated.
            let result: Result<(), String> = (|| {
                if !retained_curve(&parameter).is_some_and(|current| current.same_reference(&curve))
                {
                    return Err("The curve changed before this edit could be applied.".into());
                }
                match edit {
                    Edit::Linear => {
                        let layer = CurveLayerHandle::new(AnalyticCurveKind::Linear);
                        curve.clear();
                        curve.set_wrapped(false);
                        curve
                            .insert(0, Some(&layer))
                            .map_err(|error| error.to_string())?;
                    }
                    Edit::Bounds([low, high]) => {
                        if !has_linear_endpoints(&curve) {
                            return Err(
                                "The linear curve changed before this edit could be applied."
                                    .into(),
                            );
                        }
                        curve.set_minimum(low);
                        curve.set_maximum(high);
                    }
                    Edit::Nudge(ix, delta) => {
                        if !has_linear_endpoints(&curve) {
                            return Err(
                                "The linear curve changed before this edit could be applied."
                                    .into(),
                            );
                        }
                        let value = if ix == 0 {
                            curve.minimum()
                        } else {
                            curve.maximum()
                        } + delta;
                        if !value.is_finite() {
                            return Err("The endpoint output must stay finite.".into());
                        }
                        if ix == 0 {
                            curve.set_minimum(value);
                        } else {
                            curve.set_maximum(value);
                        }
                    }
                }
                Ok(())
            })();
            if !active
                .upgrade()
                .is_some_and(|marker| marker.load(Ordering::Acquire))
            {
                return;
            }
            let delivered = cx.update_window(handle, |_, window, cx| {
                owner.update(cx, |view, cx| {
                    view.pending = false;
                    view.error = result.err();
                    if view.error.is_none() {
                        if nudges_endpoint {
                            let values = [
                                view.curve.minimum().to_string(),
                                view.curve.maximum().to_string(),
                            ];
                            let current = view.text(cx);
                            for (ix, value) in values.into_iter().enumerate() {
                                if clean[ix] && current[ix] == submitted[ix] {
                                    view.fields[ix].update(cx, |state, cx| {
                                        state.set_value(value.clone(), window, cx)
                                    });
                                }
                                view.synced[ix] = value;
                            }
                        } else if view.text(cx) == submitted {
                            view.replace_drafts(window, cx);
                        }
                    }
                    if creates_linear && view.error.is_none() {
                        window.focus(&view.fields[0].read(cx).focus_handle(cx), cx);
                    }
                    cx.notify();
                })
            });
            if delivered.is_ok_and(|result| result.is_ok())
                && active
                    .upgrade()
                    .is_some_and(|marker| marker.load(Ordering::Acquire))
            {
                if let Some(runtime) = runtime.upgrade() {
                    if let Err(error) = runtime.request_effect_presentation(active.clone()) {
                        let _ = cx.update_window(handle, |_, _, cx| {
                            owner.update(cx, |view, cx| {
                                if view.error.is_none() {
                                    view.error = Some(error.to_string());
                                }
                                cx.notify();
                            })
                        });
                    }
                }
            }
        })
        .detach();
    }
}
impl Drop for ToneCurveView {
    fn drop(&mut self) {
        self.close();
    }
}
impl Render for ToneCurveView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let current = retained_curve(&self.parameter);
        let editable = self.is_current() && has_linear_endpoints(&self.curve);
        let description = current
            .as_ref()
            .map(curve_name)
            .unwrap_or_else(|| "Curve is not initialized".into());
        let sampled = current.as_ref().map(samples);
        let mut body = div().v_flex().gap_3().child(
            div()
                .id("tone-curve-kind")
                .role(gpui_kit::Role::Status)
                .aria_label(description.clone())
                .test_support()
                .text_sm()
                .child(description),
        );
        match sampled {
            Some(Ok(values)) => {
                body = body.child(
                    div()
                        .id("tone-curve-graph")
                        .role(gpui_kit::Role::Image)
                        .aria_label("Tone curve graph · input left to right, output bottom to top")
                        .test_support()
                        .h_40()
                        .w_full()
                        .border_1()
                        .border_color(theme.border)
                        .rounded(theme.radius)
                        .overflow_hidden()
                        .bg(theme.muted)
                        .child(graph(values, cx)),
                )
            }
            Some(Err(error)) => {
                body = body.child(div().text_sm().text_color(theme.danger).child(error))
            }
            None => (),
        }
        if editable {
            body = body
                .child(
                    div()
                        .id("tone-curve-endpoints")
                        .role(gpui_kit::Role::Group)
                        .aria_label("Linear curve endpoints")
                        .test_support()
                        .track_focus(&self.points_focus)
                        .key_context("ToneCurvePoints")
                        .v_flex()
                        .gap_2()
                        .on_action(cx.listener(|view, _: &SelectBlackEndpoint, window, cx| {
                            view.select_endpoint(0, window, cx);
                            cx.stop_propagation();
                        }))
                        .on_action(cx.listener(|view, _: &SelectWhiteEndpoint, window, cx| {
                            view.select_endpoint(1, window, cx);
                            cx.stop_propagation();
                        }))
                        .on_action(cx.listener(|view, _: &RaiseEndpoint, window, cx| {
                            view.nudge(0.01, window, cx);
                            cx.stop_propagation();
                        }))
                        .on_action(cx.listener(|view, _: &LowerEndpoint, window, cx| {
                            view.nudge(-0.01, window, cx);
                            cx.stop_propagation();
                        }))
                        .child(
                            div().h_flex().flex_wrap().gap_2().children(
                                ["Black endpoint", "White endpoint"]
                                    .into_iter()
                                    .enumerate()
                                    .map(|(ix, label)| {
                                        Button::new(if ix == 0 {
                                            "tone-select-black"
                                        } else {
                                            "tone-select-white"
                                        })
                                        .label(label)
                                        .small()
                                        .selected(self.selected_endpoint == ix)
                                        .toggled(self.selected_endpoint == ix)
                                        .on_click(
                                            cx.listener(move |view, _, window, cx| {
                                                view.select_endpoint(ix, window, cx)
                                            }),
                                        )
                                    }),
                            ),
                        )
                        .child(
                            div()
                                .h_flex()
                                .flex_wrap()
                                .gap_2()
                                .child(
                                    Button::new("tone-lower")
                                        .label("Decrease output")
                                        .small()
                                        .disabled(self.pending)
                                        .tooltip(
                                            "Down arrow · decrease the selected endpoint by 0.01",
                                        )
                                        .on_click(cx.listener(|view, _, window, cx| {
                                            view.nudge(-0.01, window, cx)
                                        })),
                                )
                                .child(
                                    Button::new("tone-raise")
                                        .label("Increase output")
                                        .small()
                                        .disabled(self.pending)
                                        .tooltip(
                                            "Up arrow · increase the selected endpoint by 0.01",
                                        )
                                        .on_click(cx.listener(|view, _, window, cx| {
                                            view.nudge(0.01, window, cx)
                                        })),
                                ),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child("Left/Right select an endpoint. Up/Down adjust its output."),
                        ),
                )
                .child(
                    Form::new().children(
                        ["Black output", "White output"]
                            .into_iter()
                            .enumerate()
                            .map(|(ix, label)| {
                                Field::new().label(label).child(
                                    Input::new(&self.fields[ix])
                                        .id(if ix == 0 {
                                            "tone-black-output"
                                        } else {
                                            "tone-white-output"
                                        })
                                        .aria_label(label)
                                        .disabled(self.pending),
                                )
                            }),
                    ),
                )
                .child(
                    div()
                        .h_flex()
                        .flex_wrap()
                        .gap_2()
                        .child(
                            Button::new("tone-apply")
                                .label("Apply endpoints")
                                .primary()
                                .disabled(self.pending)
                                .on_click(
                                    cx.listener(|view, _, window, cx| view.apply(window, cx)),
                                ),
                        )
                        .child(
                            Button::new("tone-revert")
                                .label("Revert edits")
                                .disabled(self.pending)
                                .on_click(
                                    cx.listener(|view, _, window, cx| view.reload(window, cx)),
                                ),
                        ),
                );
        } else {
            body = body.child(div().id("tone-curve-editing-status").role(gpui_kit::Role::Status).aria_label("Endpoint editing is unavailable for this stored curve arrangement.").test_support().text_sm().text_color(theme.muted_foreground).child("Endpoint editing is unavailable for this stored curve arrangement. Use linear curve explicitly replaces its layers."));
        }
        if editable && self.text(cx) != self.synced {
            body = body.child(
                div()
                    .id("tone-curve-draft-status")
                    .role(gpui_kit::Role::Status)
                    .aria_label("Unapplied endpoint edits")
                    .test_support()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child("Unapplied endpoint edits"),
            );
        }
        body = body.child(
            div()
                .h_flex()
                .flex_wrap()
                .gap_2()
                .child(
                    Button::new("tone-use-linear")
                        .label("Use linear curve")
                        .disabled(self.pending || !self.is_current())
                        .on_click(
                            cx.listener(|view, _, window, cx| {
                                view.submit(Edit::Linear, window, cx)
                            }),
                        ),
                )
                .when(!self.is_current(), |row| {
                    row.child(
                        Button::new("tone-reload")
                            .label("Reload curve")
                            .disabled(self.pending)
                            .on_click(cx.listener(|view, _, window, cx| view.reload(window, cx))),
                    )
                }),
        );
        div()
            .id("tone-curve-editor")
            .role(gpui_kit::Role::Group)
            .aria_label("Tone curve")
            .test_support()
            .track_focus(&self.focus)
            .key_context("ToneCurveEditor")
            .v_flex()
            .gap_3()
            .on_action(cx.listener(|view, _: &ApplyToneCurve, window, cx| {
                if has_linear_endpoints(&view.curve) {
                    view.apply(window, cx);
                }
                cx.stop_propagation();
            }))
            .on_action(cx.listener(|view, _: &RevertToneCurve, window, cx| {
                view.reload(window, cx);
                cx.stop_propagation();
            }))
            .child(body)
            .when_some(self.error.clone(), |body, error| {
                body.child(
                    div()
                        .id("tone-curve-error")
                        .role(gpui_kit::Role::Alert)
                        .aria_label(error.clone())
                        .test_support()
                        .text_sm()
                        .text_color(theme.danger)
                        .child(error),
                )
            })
    }
}
