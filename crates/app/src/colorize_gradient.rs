//! Retained Colorize stops are the model. Inputs and stable occurrence IDs are
//! presentation state; edits never replace the Gradient or resample its lists.
use crate::{ForegroundError, color_binding_effects::BindingRuntime};
use gpui_kit::base::{Disableable, Selectable, StyledExt, TestSupportExt};
use gpui_kit::component::{
    ActiveTheme, Sizable,
    button::{Button, ButtonVariants},
    form::{Field, Form},
    input::{Input, InputState},
    menu::{DropdownMenu, PopupMenuItem},
};
use gpui_kit::{
    AnyWindowHandle, App, AppContext, Context, Entity, FocusHandle, Focusable, IntoElement, Render,
    Window, div, prelude::*,
};
use mycela_engine::{
    AlphaMixingPoint, ColorMixingPoint, GradientEntry, PointwiseParameter, PointwiseParameterValue,
    RgbColor, SharedGradient,
};
use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, Ordering},
};

gpui_kit::actions!(
    colorize_gradient,
    [
        ApplyColorizeStop,
        RevertColorizeStop,
        PreviousColorizeStop,
        NextColorizeStop,
        FirstColorizeStop,
        LastColorizeStop,
        RemoveColorizeStop
    ]
);
pub(crate) fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        gpui_kit::KeyBinding::new(
            "secondary-enter",
            ApplyColorizeStop,
            Some("ColorizeGradient"),
        ),
        gpui_kit::KeyBinding::new("escape", RevertColorizeStop, Some("ColorizeGradient")),
        gpui_kit::KeyBinding::new("left", PreviousColorizeStop, Some("ColorizeStops")),
        gpui_kit::KeyBinding::new("right", NextColorizeStop, Some("ColorizeStops")),
        gpui_kit::KeyBinding::new("home", FirstColorizeStop, Some("ColorizeStops")),
        gpui_kit::KeyBinding::new("end", LastColorizeStop, Some("ColorizeStops")),
        gpui_kit::KeyBinding::new("delete", RemoveColorizeStop, Some("ColorizeStops")),
    ]);
}
#[derive(Clone)]
enum Stop {
    Color(ColorMixingPoint),
    Opacity(AlphaMixingPoint),
}
impl Stop {
    fn entry(&self) -> GradientEntry {
        match self {
            Self::Color(p) => GradientEntry::Color(p.clone()),
            Self::Opacity(p) => GradientEntry::Alpha(p.clone()),
        }
    }
    fn same(&self, other: &Self) -> bool {
        self.entry().same_reference(&other.entry())
    }
    fn is_opacity(&self) -> bool {
        matches!(self, Self::Opacity(_))
    }
    fn position(&self) -> i32 {
        match self {
            Self::Color(p) => p.coordinate(),
            Self::Opacity(p) => p.coordinate(),
        }
    }
    fn values(&self) -> Vec<String> {
        let mut values = vec![self.position().to_string()];
        match self {
            Self::Color(p) => values.extend(p.color().argb()[1..].iter().map(ToString::to_string)),
            Self::Opacity(p) => values.push(p.value().to_string()),
        }
        values
    }
}
fn retained(parameter: &PointwiseParameter<ForegroundError>) -> Option<SharedGradient> {
    match parameter.value() {
        PointwiseParameterValue::Gradient(g) => g,
        _ => None,
    }
}
fn actual_stops(g: &SharedGradient) -> Result<Vec<Stop>, String> {
    let mut stops = Vec::new();
    for (opacity, list) in [(false, g.color_points()), (true, g.alpha_points())] {
        let list = list.ok_or("A stored stop list is null. Its contents have been preserved.")?;
        for ix in 0..list.len() {
            let entry = list.entry(ix as i32).map_err(|e| e.to_string())?;
            stops.push(match (opacity, entry) {
                (false, GradientEntry::Color(p)) => Stop::Color(p),
                (true, GradientEntry::Alpha(p)) => Stop::Opacity(p),
                _ => {
                    return Err(format!(
                        "Stored {} stop {} is unsupported. Its contents have been preserved.",
                        if opacity { "opacity" } else { "color" },
                        ix + 1
                    ));
                }
            });
        }
    }
    Ok(stops)
}
struct StopDraft {
    id: u64,
    stop: Stop,
    fields: Vec<Entity<InputState>>,
    synced: Vec<String>,
}
impl StopDraft {
    fn text(&self, cx: &App) -> Vec<String> {
        self.fields
            .iter()
            .map(|f| f.read(cx).value().to_string())
            .collect()
    }
    fn sync(&mut self, reset: bool, window: &mut Window, cx: &mut App) {
        let values = self.stop.values();
        for (ix, value) in values.iter().enumerate() {
            if (reset || self.fields[ix].read(cx).value().as_ref() == self.synced[ix].as_str())
                && self.fields[ix].read(cx).value().as_ref() != value.as_str()
            {
                self.fields[ix].update(cx, |s, cx| s.set_value(value.clone(), window, cx));
            }
        }
        self.synced = values;
    }
}
enum Edit {
    Reload,
    Apply(Stop, Option<i32>, [Option<f64>; 3]),
    Add(bool, i32),
    Remove(Stop),
}
pub(crate) struct ColorizeGradientView {
    parameter: PointwiseParameter<ForegroundError>,
    gradient: Option<SharedGradient>,
    stops: Vec<StopDraft>,
    selected: Option<u64>,
    next_id: u64,
    opacity: bool,
    new_position: Entity<InputState>,
    focus: FocusHandle,
    stop_focus: FocusHandle,
    window: AnyWindowHandle,
    runtime: Weak<BindingRuntime>,
    active: Arc<AtomicBool>,
    pending: bool,
    error: Option<String>,
    projection_error: Option<String>,
}
impl ColorizeGradientView {
    pub(crate) fn new(
        parameter: PointwiseParameter<ForegroundError>,
        runtime: &Arc<BindingRuntime>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let gradient = retained(&parameter);
        let middle = gradient.as_ref().map_or(500, |g| g.width() / 2).to_string();
        let mut view = Self {
            parameter,
            gradient,
            stops: Vec::new(),
            selected: None,
            next_id: 1,
            opacity: false,
            new_position: cx.new(|cx| InputState::new(window, cx).default_value(middle)),
            focus: cx.focus_handle().tab_stop(true),
            stop_focus: cx.focus_handle().tab_stop(true),
            window: window.window_handle(),
            runtime: Arc::downgrade(runtime),
            active: Arc::new(AtomicBool::new(true)),
            pending: false,
            error: None,
            projection_error: None,
        };
        view.refresh_stops(None, None, window, cx);
        view
    }
    pub(crate) fn initialize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.submit(Edit::Reload, window, cx);
    }
    pub(crate) fn close(&mut self) {
        self.active.store(false, Ordering::Release);
    }
    pub(crate) fn preferred_focus(&self, cx: &App) -> FocusHandle {
        self.current().map_or_else(
            || self.focus.clone(),
            |d| d.fields[0].read(cx).focus_handle(cx),
        )
    }
    fn current(&self) -> Option<&StopDraft> {
        self.stops.iter().find(|d| Some(d.id) == self.selected)
    }
    fn is_current(&self) -> bool {
        match (&self.gradient, retained(&self.parameter)) {
            (Some(a), Some(b)) => a.same_reference(&b),
            _ => false,
        }
    }
    fn editable(&self) -> bool {
        self.is_current() && self.projection_error.is_none()
    }
    fn refresh_stops(
        &mut self,
        reset: Option<u64>,
        prefer: Option<Stop>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(g) = &self.gradient else {
            return;
        };
        let actual = match actual_stops(g) {
            Ok(stops) => stops,
            Err(e) => {
                self.projection_error = Some(e);
                return;
            }
        };
        self.projection_error = None;
        let mut old = std::mem::take(&mut self.stops);
        for stop in actual {
            // Match retained handle occurrences, not mutable positions or list
            // indices. Repeated aliases get distinct stable presentation IDs.
            let draft = if let Some(ix) = old.iter().position(|d| d.stop.same(&stop)) {
                let mut draft = old.remove(ix);
                draft.sync(reset == Some(draft.id), window, cx);
                draft
            } else {
                let synced = stop.values();
                let id = self.next_id;
                self.next_id += 1;
                let fields = synced
                    .iter()
                    .map(|value| {
                        cx.new(|cx| InputState::new(window, cx).default_value(value.clone()))
                    })
                    .collect();
                StopDraft {
                    id,
                    stop,
                    fields,
                    synced,
                }
            };
            self.stops.push(draft);
        }
        if let Some(point) = prefer {
            // Existing occurrence selection wins over another occurrence of the
            // same aliased point. A new insertion has no surviving selected ID.
            self.selected = self
                .stops
                .iter()
                .find(|d| Some(d.id) == self.selected && d.stop.same(&point))
                .or_else(|| self.stops.iter().find(|d| d.stop.same(&point)))
                .map(|d| d.id);
            self.opacity = point.is_opacity();
        }
        if !self
            .stops
            .iter()
            .any(|d| Some(d.id) == self.selected && d.stop.is_opacity() == self.opacity)
        {
            self.selected = self
                .stops
                .iter()
                .find(|d| d.stop.is_opacity() == self.opacity)
                .map(|d| d.id);
        }
    }
    pub(crate) fn refresh_controls(&mut self, cx: &mut Context<Self>) {
        let handle = self.window;
        let active = Arc::downgrade(&self.active);
        cx.spawn(async move |owner, cx| {
            let prepared = cx
                .update_window(handle, |_, _, cx| {
                    owner.read_with(cx, |view, _| {
                        if !view.pending && view.is_current() {
                            view.gradient.clone()
                        } else {
                            None
                        }
                    })
                })
                .ok()
                .and_then(Result::ok)
                .flatten();
            if !active.upgrade().is_some_and(|m| m.load(Ordering::Acquire)) {
                return;
            }
            // Actual cache preparation belongs to presentation completion, not
            // Render. It also reflects external edits without replacing roots.
            let result = prepared.map(|g| g.rebuild()).transpose();
            let _ = cx.update_window(handle, |_, window, cx| {
                owner.update(cx, |view, cx| {
                    if !view.active.load(Ordering::Acquire) {
                        return;
                    }
                    if let Err(error) = result {
                        view.error = Some(error.to_string());
                    }
                    if !view.pending && view.is_current() {
                        view.refresh_stops(None, None, window, cx);
                    }
                    cx.notify();
                })
            });
        })
        .detach();
        cx.notify();
    }
    fn select(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending || !self.active.load(Ordering::Acquire) {
            return;
        }
        if let Some(d) = self.stops.iter().find(|d| d.id == id) {
            self.selected = Some(id);
            self.opacity = d.stop.is_opacity();
            window.focus(&d.fields[0].read(cx).focus_handle(cx), cx);
        }
        cx.notify();
    }
    fn kind(&mut self, opacity: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending {
            return;
        }
        self.opacity = opacity;
        self.selected = self
            .stops
            .iter()
            .find(|d| d.stop.is_opacity() == opacity)
            .map(|d| d.id);
        window.focus(&self.stop_focus, cx);
        cx.notify();
    }
    fn navigate(&mut self, direction: i32, window: &mut Window, cx: &mut Context<Self>) {
        let ids: Vec<_> = self
            .stops
            .iter()
            .filter(|d| d.stop.is_opacity() == self.opacity)
            .map(|d| d.id)
            .collect();
        if ids.is_empty() {
            return;
        }
        let current = ids
            .iter()
            .position(|id| Some(*id) == self.selected)
            .unwrap_or(0);
        let ix = match direction {
            i32::MIN => 0,
            i32::MAX => ids.len() - 1,
            n if n < 0 => current.saturating_sub(1),
            _ => (current + 1).min(ids.len() - 1),
        };
        self.select(ids[ix], window, cx);
        window.focus(&self.stop_focus, cx);
    }
    fn revert(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending {
            return;
        }
        self.error = None;
        self.refresh_stops(self.selected, None, window, cx);
        cx.notify();
    }
    fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.editable() || self.pending {
            return;
        }
        let Some(activated) = self.current().map(|d| (d.id, d.stop.clone())) else {
            return;
        };
        self.refresh_stops(None, None, window, cx);
        let Some(d) = self
            .current()
            .filter(|d| d.id == activated.0 && d.stop.same(&activated.1))
        else {
            self.error =
                Some("The selected stop was removed. Choose a stop before applying.".into());
            cx.notify();
            return;
        };
        let text = d.text(cx);
        let parsed = (|| -> Result<(Option<i32>, [Option<f64>; 3]), String> {
            let position = if text[0] == d.synced[0] {
                None
            } else {
                Some(
                    text[0]
                        .parse()
                        .map_err(|_| "Position needs a whole number.".to_owned())?,
                )
            };
            let mut values = [None; 3];
            for ix in 1..text.len() {
                if text[ix] != d.synced[ix] {
                    let value: f64 = text[ix]
                        .parse()
                        .map_err(|_| "Stop values need finite numbers.".to_owned())?;
                    if !value.is_finite() {
                        return Err("Stop values need finite numbers.".into());
                    }
                    values[ix - 1] = Some(value);
                }
            }
            Ok((position, values))
        })();
        match parsed {
            Ok((position, values)) => {
                self.submit(Edit::Apply(d.stop.clone(), position, values), window, cx)
            }
            Err(e) => {
                self.error = Some(e);
                cx.notify();
            }
        }
    }
    fn add(&mut self, opacity: bool, window: &mut Window, cx: &mut Context<Self>) {
        match self.new_position.read(cx).value().parse::<i32>() {
            Ok(position) => self.submit(Edit::Add(opacity, position), window, cx),
            Err(_) => {
                self.error = Some("New stop position needs a whole number.".into());
                cx.notify();
            }
        }
    }
    fn remove(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(d) = self.current() {
            self.submit(Edit::Remove(d.stop.clone()), window, cx);
        }
    }
    fn submit(&mut self, edit: Edit, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending || !self.active.load(Ordering::Acquire) {
            return;
        }
        if !matches!(edit, Edit::Reload) && !self.editable() {
            return;
        }
        let active = Arc::downgrade(&self.active);
        let runtime = self.runtime.clone();
        let parameter = self.parameter.clone();
        let gradient = self.gradient.clone();
        let reset = if matches!(edit, Edit::Apply(..)) {
            self.selected
        } else {
            None
        };
        let handle = window.window_handle();
        self.pending = true;
        cx.notify();
        cx.spawn(async move |owner, cx| {
            let alive = cx
                .update_window(handle, |_, _, cx| owner.read_with(cx, |_, _| ()))
                .is_ok_and(|result| result.is_ok());
            if !alive || !active.upgrade().is_some_and(|m| m.load(Ordering::Acquire)) {
                return;
            }
            // Domain writes/rebuilds run outside App, Window and Entity borrows.
            // Gradient/point releases are callback-free; direct stop edits emit no fake events.
            let result = (|| -> Result<(SharedGradient, Option<Stop>), String> {
                if matches!(edit, Edit::Reload) {
                    let g = parameter.gradient().map_err(|e| e.to_string())?;
                    g.rebuild().map_err(|e| e.to_string())?;
                    return Ok((g, None));
                }
                let g = gradient.ok_or("The gradient is unavailable.")?;
                if !retained(&parameter).is_some_and(|current| current.same_reference(&g)) {
                    return Err("The gradient changed. Reload it before editing.".into());
                }
                let preferred = match edit {
                    Edit::Apply(point, position, values) => {
                        if g.point_reference_index(&point.entry())
                            .map_err(|e| e.to_string())?
                            < 0
                        {
                            return Err("The selected stop was removed.".into());
                        }
                        match &point {
                            Stop::Color(p) => {
                                if let Some(x) = position {
                                    p.set_coordinate(x);
                                }
                                let old = p.color();
                                let mut argb = old.argb();
                                for (ix, value) in values.iter().enumerate() {
                                    if let Some(value) = value {
                                        argb[ix + 1] = *value;
                                    }
                                }
                                p.set_color(
                                    RgbColor::from_argb(argb).grayscale(old.is_grayscale()),
                                );
                            }
                            Stop::Opacity(p) => {
                                if let Some(x) = position {
                                    p.set_coordinate(x);
                                }
                                if let Some(v) = values[0] {
                                    p.set_value(v);
                                }
                            }
                        }
                        Some(point)
                    }
                    Edit::Add(opacity, x) => Some(if opacity {
                        Stop::Opacity(g.insert_sampled_alpha(x).map_err(|e| e.to_string())?)
                    } else {
                        Stop::Color(g.insert_sampled_color(x).map_err(|e| e.to_string())?)
                    }),
                    Edit::Remove(point) => {
                        if g.point_reference_index(&point.entry())
                            .map_err(|e| e.to_string())?
                            < 0
                        {
                            return Err("The selected stop was removed.".into());
                        }
                        match point {
                            Stop::Color(p) => g.remove_color(Some(p)),
                            Stop::Opacity(p) => g.remove_alpha(Some(p)),
                        }
                        .map_err(|e| e.to_string())?;
                        None
                    }
                    Edit::Reload => unreachable!(),
                };
                g.rebuild().map_err(|e| e.to_string())?;
                Ok((g, preferred))
            })();
            if !active.upgrade().is_some_and(|m| m.load(Ordering::Acquire)) {
                return;
            }
            let delivered = cx.update_window(handle, |_, window, cx| {
                owner.update(cx, |view, cx| {
                    view.pending = false;
                    match result {
                        Ok((g, point)) => {
                            if !view
                                .gradient
                                .as_ref()
                                .is_some_and(|old| old.same_reference(&g))
                            {
                                view.stops.clear();
                                view.selected = None;
                            }
                            view.gradient = Some(g);
                            view.error = None;
                            view.refresh_stops(reset, point, window, cx);
                            if view.focus.is_focused(window) {
                                window.focus(&view.preferred_focus(cx), cx);
                            }
                        }
                        Err(e) => {
                            view.error = Some(e);
                            view.refresh_stops(None, None, window, cx);
                        }
                    }
                    cx.notify();
                })
            });
            if delivered.is_ok_and(|result| result.is_ok())
                && active.upgrade().is_some_and(|m| m.load(Ordering::Acquire))
            {
                if let Some(runtime) = runtime.upgrade() {
                    if let Err(e) = runtime.request_effect_presentation(active) {
                        let _ = owner.update(cx, |view, cx| {
                            view.error.get_or_insert_with(|| e.to_string());
                            cx.notify();
                        });
                    }
                }
            }
        })
        .detach();
    }
}
impl Drop for ColorizeGradientView {
    fn drop(&mut self) {
        self.close();
    }
}
impl Render for ColorizeGradientView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let editable = self.editable() && !self.pending;
        let mut body = div()
            .v_flex()
            .gap_3()
            .child(div().font_semibold().child("Gradient stops"));
        if let Some(g) = &self.gradient {
            let samples: Result<Vec<_>, _> =
                (0..32).map(|i| g.sample(f64::from(i) / 31.)).collect();
            if let Ok(samples) = samples {
                body = body.child(
                    div()
                        .id("colorize-gradient-strip")
                        .role(gpui_kit::Role::Image)
                        .aria_label("Applied gradient colors and opacity")
                        .test_support()
                        .h_8()
                        .w_full()
                        .flex()
                        .overflow_hidden()
                        .rounded(theme.radius)
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.muted)
                        .children(samples.into_iter().map(|color| {
                            // Audited raster/data colors, not interface palette values.
                            let [a, r, g, b] = color.to_argb_bytes();
                            div()
                                .flex_1()
                                .h_full()
                                .bg(gpui_kit::rgba(u32::from_be_bytes([r, g, b, a])))
                        })),
                );
            }
            body = body.child(
                div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(format!(
                        "{} color stops · {} opacity stops · width {}",
                        self.stops.iter().filter(|d| !d.stop.is_opacity()).count(),
                        self.stops.iter().filter(|d| d.stop.is_opacity()).count(),
                        g.width()
                    )),
            );
        }
        let selected = self.current().map(|d| d.id);
        let choices: Vec<_> = self
            .stops
            .iter()
            .filter(|d| d.stop.is_opacity() == self.opacity)
            .enumerate()
            .map(|(ix, d)| {
                (
                    d.id,
                    format!(
                        "{} stop {} · {}",
                        if self.opacity { "Opacity" } else { "Color" },
                        ix + 1,
                        d.stop.position()
                    ),
                )
            })
            .collect();
        let label = choices
            .iter()
            .find(|(id, _)| Some(*id) == selected)
            .map_or("No stops".to_owned(), |(_, label)| label.clone());
        let owner = cx.entity().downgrade();
        body = body
            .child(
                div()
                    .h_flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        Button::new("colorize-colors")
                            .label("Colors")
                            .small()
                            .selected(!self.opacity)
                            .disabled(self.pending)
                            .on_click(
                                cx.listener(|view, _, window, cx| view.kind(false, window, cx)),
                            ),
                    )
                    .child(
                        Button::new("colorize-opacity")
                            .label("Opacity")
                            .small()
                            .selected(self.opacity)
                            .disabled(self.pending)
                            .on_click(
                                cx.listener(|view, _, window, cx| view.kind(true, window, cx)),
                            ),
                    ),
            )
            .child(
                div()
                    .id("colorize-stop-navigation")
                    .role(gpui_kit::Role::Group)
                    .aria_label("Gradient stop selection")
                    .test_support()
                    .track_focus(&self.stop_focus)
                    .key_context("ColorizeStops")
                    .v_flex()
                    .gap_2()
                    .on_action(cx.listener(|view, _: &PreviousColorizeStop, window, cx| {
                        view.navigate(-1, window, cx);
                        cx.stop_propagation();
                    }))
                    .on_action(cx.listener(|view, _: &NextColorizeStop, window, cx| {
                        view.navigate(1, window, cx);
                        cx.stop_propagation();
                    }))
                    .on_action(cx.listener(|view, _: &FirstColorizeStop, window, cx| {
                        view.navigate(i32::MIN, window, cx);
                        cx.stop_propagation();
                    }))
                    .on_action(cx.listener(|view, _: &LastColorizeStop, window, cx| {
                        view.navigate(i32::MAX, window, cx);
                        cx.stop_propagation();
                    }))
                    .on_action(cx.listener(|view, _: &RemoveColorizeStop, window, cx| {
                        view.remove(window, cx);
                        cx.stop_propagation();
                    }))
                    .child(
                        Button::new("colorize-stop-menu")
                            .label(label)
                            .w_full()
                            .small()
                            .dropdown_caret(true)
                            .disabled(self.pending || choices.is_empty())
                            .dropdown_menu(move |mut menu, _, _| {
                                for (id, label) in &choices {
                                    let owner = owner.clone();
                                    let id = *id;
                                    menu = menu.item(
                                        PopupMenuItem::new(label.clone())
                                            .checked(Some(id) == selected)
                                            .on_click(move |_, window, cx| {
                                                let _ = owner.update(cx, |view, cx| {
                                                    view.select(id, window, cx)
                                                });
                                            }),
                                    );
                                }
                                menu.scrollable(true)
                            }),
                    )
                    .child(
                        div()
                            .h_flex()
                            .gap_2()
                            .child(
                                Button::new("colorize-previous")
                                    .label("Previous stop")
                                    .small()
                                    .disabled(self.pending)
                                    .on_click(cx.listener(|view, _, window, cx| {
                                        view.navigate(-1, window, cx)
                                    })),
                            )
                            .child(
                                Button::new("colorize-next")
                                    .label("Next stop")
                                    .small()
                                    .disabled(self.pending)
                                    .on_click(cx.listener(|view, _, window, cx| {
                                        view.navigate(1, window, cx)
                                    })),
                            ),
                    ),
            );
        if let Some(d) = self.current() {
            let labels: &[&str] = if d.stop.is_opacity() {
                &["Position", "Opacity"]
            } else {
                &["Position", "Red", "Green", "Blue"]
            };
            body = body
                .child(
                    Form::new().children(d.fields.iter().zip(labels).enumerate().map(
                        |(ix, (field, label))| {
                            Field::new().label(*label).child(
                                Input::new(field)
                                    .id(format!("colorize-stop-value-{ix}"))
                                    .aria_label(*label)
                                    .disabled(!editable),
                            )
                        },
                    )),
                )
                .child(
                    div()
                        .h_flex()
                        .flex_wrap()
                        .gap_2()
                        .child(
                            Button::new("colorize-stop-apply")
                                .label("Apply stop")
                                .primary()
                                .disabled(!editable)
                                .on_click(
                                    cx.listener(|view, _, window, cx| view.apply(window, cx)),
                                ),
                        )
                        .child(
                            Button::new("colorize-stop-revert")
                                .label("Revert edits")
                                .disabled(self.pending)
                                .on_click(
                                    cx.listener(|view, _, window, cx| view.revert(window, cx)),
                                ),
                        )
                        .child(
                            Button::new("colorize-stop-remove")
                                .label("Remove stop")
                                .disabled(!editable)
                                .on_click(
                                    cx.listener(|view, _, window, cx| view.remove(window, cx)),
                                ),
                        ),
                );
            if d.text(cx) != d.synced {
                body = body.child(
                    div()
                        .id("colorize-stop-dirty")
                        .role(gpui_kit::Role::Status)
                        .aria_label("Unapplied stop edits")
                        .test_support()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child("Unapplied stop edits"),
                );
            }
        } else {
            body = body.child(
                div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child("No stops in this list. Add a stop to edit it."),
            );
        }
        body = body
            .child(
                Field::new().label("New stop position").child(
                    Input::new(&self.new_position)
                        .id("colorize-new-position")
                        .aria_label("New stop position")
                        .disabled(!editable),
                ),
            )
            .child(
                div()
                    .h_flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        Button::new("colorize-add-color")
                            .label("Add color stop")
                            .disabled(!editable)
                            .on_click(
                                cx.listener(|view, _, window, cx| view.add(false, window, cx)),
                            ),
                    )
                    .child(
                        Button::new("colorize-add-opacity")
                            .label("Add opacity stop")
                            .disabled(!editable)
                            .on_click(
                                cx.listener(|view, _, window, cx| view.add(true, window, cx)),
                            ),
                    ),
            )
            .child(div().text_sm().text_color(theme.muted_foreground).child(
                "Opacity 0 is transparent; 1 is opaque. New stops sample the current gradient.",
            ));
        if !self.is_current() {
            body = body.child(
                Button::new("colorize-reload")
                    .label("Reload gradient")
                    .disabled(self.pending)
                    .on_click(
                        cx.listener(|view, _, window, cx| view.submit(Edit::Reload, window, cx)),
                    ),
            );
        }
        if self.pending {
            body = body.child(
                div()
                    .id("colorize-gradient-pending")
                    .role(gpui_kit::Role::Status)
                    .text_sm()
                    .child("Updating gradient…"),
            );
        }
        if let Some(error) = self.error.as_ref().or(self.projection_error.as_ref()) {
            body = body.child(
                div()
                    .id("colorize-gradient-error")
                    .role(gpui_kit::Role::Status)
                    .aria_label(error.clone())
                    .test_support()
                    .text_sm()
                    .text_color(theme.danger)
                    .child(error.clone()),
            );
        }
        div()
            .id("colorize-gradient-editor")
            .role(gpui_kit::Role::Group)
            .aria_label("Colorize gradient")
            .test_support()
            .track_focus(&self.focus)
            .key_context("ColorizeGradient")
            .v_flex()
            .gap_3()
            .on_action(cx.listener(|view, _: &ApplyColorizeStop, window, cx| {
                view.apply(window, cx);
                cx.stop_propagation();
            }))
            .on_action(cx.listener(|view, _: &RevertColorizeStop, window, cx| {
                view.revert(window, cx);
                cx.stop_propagation();
            }))
            .child(body)
    }
}
