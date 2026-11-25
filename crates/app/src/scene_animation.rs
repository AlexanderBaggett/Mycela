//! Scene inspector animation uses the actual retained property animation and
//! curve roots. Text drafts and the expanded state belong only to the view.
use super::*;
use gpui_kit::component::collapsible::Collapsible;
use mycela_engine::{AnalyticCurveKind, CurveLayerHandle, ModernNodeAnimation, SharedCurve};

pub(super) struct CurveDraft {
    fields: [Entity<InputState>; 2],
    synced: [String; 2],
    curve: Option<Curve>,
}
impl CurveDraft {
    pub(super) fn new(window: &mut Window, cx: &mut Context<SceneSourceView>) -> Self {
        Self {
            fields: std::array::from_fn(|_| cx.new(|cx| InputState::new(window, cx))),
            synced: std::array::from_fn(|_| String::new()),
            curve: None,
        }
    }
}

type Curve = SharedCurve<ForegroundError>;
type Animation = ModernNodeAnimation<ForegroundError>;

enum Edit {
    Toggle {
        property: usize,
        axis: i32,
        expected: Option<Animation>,
        enabled: bool,
    },
    Bounds(Vec<(usize, i32, Curve, [f32; 2])>),
    Time(f64),
}

fn animation_axis(
    property: &ModernNodeProperty<ForegroundError>,
    axis: i32,
) -> Result<(bool, Option<Curve>), String> {
    let Some(animation) = property.retained_animation() else {
        return Ok((false, None));
    };
    let flags = animation
        .flags()
        .ok_or_else(|| "Animation flags are unavailable.".to_owned())?;
    let curves = animation
        .curves()
        .ok_or_else(|| "Animation curves are unavailable.".to_owned())?;
    Ok((
        flags.value(axis).map_err(|e| e.to_string())?,
        curves.value(axis).map_err(|e| e.to_string())?,
    ))
}

fn same_animation(a: Option<&Animation>, b: Option<&Animation>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.same_reference(b),
        _ => false,
    }
}

fn same_curve(a: Option<&Curve>, b: Option<&Curve>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.same_reference(b),
        _ => false,
    }
}

fn toggle(
    property: &ModernNodeProperty<ForegroundError>,
    axis: i32,
    expected: Option<&Animation>,
    enabled: bool,
) -> Result<(), String> {
    let current = property.retained_animation();
    if !same_animation(current.as_ref(), expected) {
        return Err("The animation changed before the edit was applied.".into());
    }
    let animation = if enabled {
        property
            .animation()
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "This property cannot be animated.".to_owned())?
    } else if let Some(current) = current {
        current
    } else {
        return Ok(());
    };
    if enabled {
        let curves = animation
            .curves()
            .ok_or_else(|| "Animation curves are unavailable.".to_owned())?;
        let is_new = curves.value(axis).map_err(|e| e.to_string())?.is_none();
        animation.ensure_curve(axis).map_err(|e| e.to_string())?;
        // An existing curve, even an intentionally empty one, is never replaced.
        if is_new {
            let curve = curves
                .value(axis)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "The animation curve is unavailable.".to_owned())?;
            let linear = CurveLayerHandle::new(AnalyticCurveKind::Linear);
            curve.insert(0, Some(&linear)).map_err(|e| e.to_string())?;
        }
    }
    animation
        .flags()
        .ok_or_else(|| "Animation flags are unavailable.".to_owned())?
        .set(axis, enabled)
        .map_err(|e| e.to_string())
}

impl SceneSourceView {
    pub(crate) fn preview_time(&self) -> f64 {
        self.preview_time
    }

    /// The workspace sets the same sample time on its active source and filters.
    /// Preserve a dirty time draft when a different owner changes the workspace.
    pub(crate) fn set_preview_time(
        &mut self,
        time: f64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.preview_time = time;
        if self.time_input.read(cx).value() == self.time_synced {
            self.time_synced = time.to_string();
            self.time_input.update(cx, |field, cx| {
                field.set_value(self.time_synced.clone(), window, cx)
            });
        }
        cx.notify();
    }

    fn axis(&self, ix: usize) -> (usize, i32) {
        let field = &self.fields[ix];
        (field.property, field.axis.unwrap_or(0) as i32)
    }
    fn curve_text(&self, ix: usize, cx: &App) -> [String; 2] {
        std::array::from_fn(|axis| self.curves[ix].fields[axis].read(cx).value().to_string())
    }
    fn replace_curve_draft(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let (property, axis) = self.axis(ix);
        let curve = self
            .model
            .property(property as i32)
            .ok()
            .and_then(|p| animation_axis(&p, axis).ok())
            .and_then(|(_, curve)| curve);
        let bounds = curve
            .as_ref()
            .map(|curve| [curve.minimum().to_string(), curve.maximum().to_string()])
            .unwrap_or_default();
        self.curves[ix].curve = curve;
        self.curves[ix].synced = bounds.clone();
        for (field, text) in self.curves[ix].fields.iter().zip(bounds) {
            field.update(cx, |state, cx| state.set_value(text, window, cx));
        }
    }
    pub(super) fn sync_animation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for ix in 0..self.curves.len() {
            if self.curve_text(ix, cx) == self.curves[ix].synced {
                self.replace_curve_draft(ix, window, cx);
            }
        }
        if self.time_input.read(cx).value() == self.time_synced {
            self.time_synced = self.preview_time.to_string();
            self.time_input.update(cx, |state, cx| {
                state.set_value(self.time_synced.clone(), window, cx)
            });
        }
    }
    pub(super) fn revert_animation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for ix in 0..self.curves.len() {
            self.replace_curve_draft(ix, window, cx);
        }
        self.time_synced = self.preview_time.to_string();
        self.time_input.update(cx, |state, cx| {
            state.set_value(self.time_synced.clone(), window, cx)
        });
    }
    fn submit_animation(&mut self, edit: Edit, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending {
            return;
        }
        let Some(presentation) = &self.presentation else {
            return;
        };
        let active = Arc::downgrade(&presentation.active);
        let runtime = self.runtime.clone();
        let model = self.model.clone();
        let handle = window.window_handle();
        let submitted: Vec<_> = (0..self.curves.len())
            .map(|ix| self.curve_text(ix, cx))
            .collect();
        let time_submitted = self.time_input.read(cx).value().to_string();
        let time = if let Edit::Time(time) = &edit {
            Some(*time)
        } else {
            None
        };
        let bounds_indices: Vec<usize> = if let Edit::Bounds(values) = &edit {
            values
                .iter()
                .filter_map(|(property, axis, _, _)| {
                    self.fields.iter().position(|f| {
                        f.property == *property && f.axis.unwrap_or(0) as i32 == *axis
                    })
                })
                .collect()
        } else {
            Vec::new()
        };
        self.pending = true;
        cx.notify();
        cx.spawn(async move |owner, cx| {
            let alive = cx
                .update_window(handle, |_, _, cx| owner.read_with(cx, |_, _| ()))
                .is_ok_and(|r| r.is_ok());
            if !alive || !active.upgrade().is_some_and(|m| m.load(Ordering::Acquire)) {
                return;
            }
            let result = match edit {
                Edit::Toggle {
                    property,
                    axis,
                    expected,
                    enabled,
                } => model
                    .property(property as i32)
                    .map_err(|e| e.to_string())
                    .and_then(|p| toggle(&p, axis, expected.as_ref(), enabled)),
                Edit::Bounds(values) => (|| {
                    for (property, axis, expected, [lower, upper]) in values {
                        if !active.upgrade().is_some_and(|m| m.load(Ordering::Acquire)) {
                            return Ok(());
                        }
                        let p = model.property(property as i32).map_err(|e| e.to_string())?;
                        let actual = animation_axis(&p, axis)?.1.ok_or_else(|| {
                            "The animation curve changed before the edit was applied.".to_owned()
                        })?;
                        if !actual.same_reference(&expected) {
                            return Err("The animation curve changed before the edit was applied."
                                .to_owned());
                        }
                        actual.set_minimum(lower);
                        actual.set_maximum(upper);
                    }
                    Ok(())
                })(),
                Edit::Time(_) => Ok(()),
            };
            if !active.upgrade().is_some_and(|m| m.load(Ordering::Acquire)) {
                return;
            }
            let delivered = cx.update_window(handle, |_, window, cx| {
                owner.update(cx, |view, cx| {
                    view.pending = false;
                    view.error = result.err();
                    if view.error.is_none() {
                        for ix in bounds_indices {
                            if view.curve_text(ix, cx) == submitted[ix] {
                                view.replace_curve_draft(ix, window, cx);
                            }
                        }
                        if let Some(time) = time {
                            view.preview_time = time;
                            if view.time_input.read(cx).value() == time_submitted {
                                view.time_synced = time.to_string();
                                view.time_input.update(cx, |field, cx| {
                                    field.set_value(view.time_synced.clone(), window, cx)
                                });
                            }
                        }
                    }
                    view.sync(window, cx);
                })
            });
            // Flags and curves are silent source writes; explicitly request the
            // existing owner presentation after all successful or partial writes.
            if delivered.is_ok_and(|r| r.is_ok())
                && active.upgrade().is_some_and(|m| m.load(Ordering::Acquire))
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
    fn apply_time(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.time_input.read(cx).value().parse::<f64>() {
            Ok(time) if time.is_finite() && (0.0..=1.0).contains(&time) => {
                self.submit_animation(Edit::Time(time), window, cx)
            }
            _ => {
                self.error = Some("Preview time needs a number from 0 to 1.".into());
                cx.notify();
            }
        }
    }
    fn apply_curve_bounds(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending || self.presentation.is_none() {
            return;
        }
        let mut edits = Vec::new();
        for ix in 0..self.curves.len() {
            let text = self.curve_text(ix, cx);
            // A clean draft is not an instruction to overwrite a newer silent
            // change to that curve when another row is being applied.
            if text == self.curves[ix].synced {
                continue;
            }
            let (property, axis) = self.axis(ix);
            let p = match self.model.property(property as i32) {
                Ok(p) => p,
                Err(e) => {
                    self.error = Some(e.to_string());
                    cx.notify();
                    return;
                }
            };
            let current = match animation_axis(&p, axis) {
                Ok((_, curve)) => curve,
                Err(error) => {
                    self.error = Some(error);
                    cx.notify();
                    return;
                }
            };
            if !same_curve(current.as_ref(), self.curves[ix].curve.as_ref()) {
                self.error = Some(format!(
                    "{} curve changed. Revert edits before continuing.",
                    self.fields[ix].label
                ));
                cx.notify();
                return;
            }
            let Some(curve) = current else {
                continue;
            };
            let mut bounds = [0.; 2];
            for side in 0..2 {
                match text[side].parse::<f32>() {
                    Ok(value) if value.is_finite() => bounds[side] = value,
                    _ => {
                        self.error = Some(format!(
                            "{} curve {} needs a valid number.",
                            self.fields[ix].label,
                            if side == 0 { "lower" } else { "upper" }
                        ));
                        cx.notify();
                        return;
                    }
                }
            }
            edits.push((property, axis, curve, bounds));
        }
        self.submit_animation(Edit::Bounds(edits), window, cx);
    }
    pub(super) fn render_animation(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let disabled = self.pending || self.presentation.is_none();
        let properties = self.model.properties();
        let theme = cx.theme().clone();
        let content = div()
            .v_flex()
            .gap_3()
            .child(
                Field::new().label("Preview time").child(
                    div()
                        .h_flex()
                        .gap_2()
                        .min_w_0()
                        .child(
                            Input::new(&self.time_input)
                                .id("scene-preview-time")
                                .aria_label("Preview time")
                                .disabled(disabled)
                                .flex_1()
                                .min_w_0(),
                        )
                        .child(
                            Button::new("set-scene-time")
                                .label("Set time")
                                .disabled(disabled)
                                .on_click(
                                    cx.listener(|view, _, window, cx| view.apply_time(window, cx)),
                                ),
                        ),
                ),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child("From 0 to 1"),
            )
            .children(self.fields.iter().enumerate().filter_map(|(ix, field)| {
                let p = &properties[field.property];
                if !p.backing_snapshot().animatable {
                    return None;
                }
                let axis = field.axis.unwrap_or(0) as i32;
                let status = animation_axis(p, axis);
                let enabled = status.as_ref().is_ok_and(|v| v.0);
                let has_curve = status.as_ref().is_ok_and(|v| v.1.is_some());
                let expected = p.retained_animation();
                let property = field.property;
                let id = format!("scene-animation-{property}-{axis}");
                Some(
                    div()
                        .v_flex()
                        .gap_2()
                        .child(
                            Checkbox::new(id.clone())
                                .label(format!("Animate {}", field.label))
                                .checked(enabled)
                                .disabled(disabled || status.is_err())
                                .on_change(cx.listener(move |view, checked, window, cx| {
                                    view.submit_animation(
                                        Edit::Toggle {
                                            property,
                                            axis,
                                            expected: expected.clone(),
                                            enabled: *checked,
                                        },
                                        window,
                                        cx,
                                    )
                                })),
                        )
                        .child(Form::new().columns(2).children((0..2).map(|side| {
                            let bound = if side == 0 { "Lower" } else { "Upper" };
                            Field::new().label(bound).child(
                                Input::new(&self.curves[ix].fields[side])
                                    .id(format!("{id}-{}", bound.to_lowercase()))
                                    .aria_label(format!(
                                        "{} curve {}",
                                        field.label,
                                        bound.to_lowercase()
                                    ))
                                    .disabled(disabled || !has_curve),
                            )
                        })))
                        .when_some(status.err(), |this, error| {
                            this.child(div().text_sm().text_color(theme.danger).child(error))
                        }),
                )
            }))
            .child(
                Button::new("apply-scene-curves")
                    .label("Apply curves")
                    .disabled(
                        disabled
                            || !self.fields.iter().any(|field| {
                                animation_axis(
                                    &properties[field.property],
                                    field.axis.unwrap_or(0) as i32,
                                )
                                .is_ok_and(|(_, curve)| curve.is_some())
                            }),
                    )
                    .on_click(
                        cx.listener(|view, _, window, cx| view.apply_curve_bounds(window, cx)),
                    ),
            );
        Collapsible::new()
            .open(self.show_animation)
            .child(
                Button::new("toggle-scene-animation")
                    .label(if self.show_animation {
                        "Hide animation"
                    } else {
                        "Show animation"
                    })
                    .on_click(cx.listener(|view, _, _, cx| {
                        view.show_animation = !view.show_animation;
                        cx.notify();
                    })),
            )
            .content(content)
            .into_any_element()
    }
}
