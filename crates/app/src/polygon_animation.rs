//! Animation drafts for the actual Polygon parameter and curve owners.
use super::*;
#[path = "polygon_layer_editor.rs"]
pub(super) mod layer_editor;
use crate::spatial_filter::{SampleTime, animation_axis, set_animation};
use gpui_kit::component::{checkbox::Checkbox, collapsible::Collapsible, input::InputState};
use gpui_kit::{AppContext, Entity};
use mycela_engine::{
    AnalyticCurveKind, CurveLayerHandle, GradientAnimation, PointwiseParameterValue, SharedCurve,
};

type Curve = SharedCurve<ForegroundError>;
type Animation = GradientAnimation<ForegroundError>;

pub(super) struct CurveDraft {
    parameter: usize,
    axis: i32,
    label: String,
    fields: [Entity<InputState>; 2],
    synced: [String; 2],
    curve: Option<Curve>,
}
pub(super) fn drafts(
    node: &PointwiseNode<ForegroundError>,
    window: &mut Window,
    cx: &mut Context<PolygonSourceView>,
) -> Vec<CurveDraft> {
    let mut drafts = Vec::new();
    for (parameter, property) in node
        .filter()
        .expect("Polygon filter")
        .parameters()
        .iter()
        .enumerate()
    {
        if !property.is_animatable() {
            continue;
        }
        let axes = match property.value() {
            PointwiseParameterValue::Pair { .. } => 2,
            PointwiseParameterValue::Number { .. } | PointwiseParameterValue::Choice { .. } => 1,
            _ => continue,
        };
        let name = String::from_utf16_lossy(&property.name());
        for axis in 0..axes {
            let label = if axes == 2 {
                format!("{name} {}", if axis == 0 { "X" } else { "Y" })
            } else {
                name.clone()
            };
            let curve = animation_axis(property, axis).ok().and_then(|v| v.1);
            let synced = curve
                .as_ref()
                .map(|v| [v.minimum().to_string(), v.maximum().to_string()])
                .unwrap_or_default();
            let fields = std::array::from_fn(|i| {
                cx.new(|cx| InputState::new(window, cx).default_value(synced[i].clone()))
            });
            drafts.push(CurveDraft {
                parameter,
                axis,
                label,
                fields,
                synced,
                curve,
            });
        }
    }
    drafts
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
type Layer = CurveLayerHandle<ForegroundError>;
#[derive(Clone)]
enum LayerAction {
    Add(AnalyticCurveKind),
    AddNoise,
    AddCustom,
    Remove(usize),
    Move(usize, usize),
    Blend(usize, i32, i32),
    Edit(usize),
}
fn same_layers(a: &[Option<Layer>], b: &[Option<Layer>]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(a, b)| match (a, b) {
            (None, None) => true,
            (Some(a), Some(b)) => a.same_reference(b),
            _ => false,
        })
}
impl PolygonSourceView {
    pub(crate) fn set_preview_time(
        &mut self,
        time: f64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.time.read(cx).value() == self.time_synced {
            self.time_synced = time.to_string();
            self.time.update(cx, |input, cx| {
                input.set_value(self.time_synced.clone(), window, cx)
            });
        }
        cx.notify();
    }
    pub(super) fn sync_animation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let filter = self.node.filter().expect("Polygon filter");
        for draft in &mut self.curves {
            let current = filter
                .parameter(draft.parameter as i32)
                .ok()
                .and_then(|p| animation_axis(&p, draft.axis).ok())
                .and_then(|v| v.1);
            let clean = draft
                .fields
                .iter()
                .zip(&draft.synced)
                .all(|(field, synced)| field.read(cx).value() == *synced);
            if clean {
                let values = current
                    .as_ref()
                    .map(|v| [v.minimum().to_string(), v.maximum().to_string()])
                    .unwrap_or_default();
                for (field, value) in draft.fields.iter().zip(&values) {
                    field.update(cx, |input, cx| input.set_value(value.clone(), window, cx));
                }
                draft.curve = current;
                draft.synced = values;
            } else if !same_curve(draft.curve.as_ref(), current.as_ref()) {
                self.error = Some(
                    "An animation curve changed. Revert curve edits before continuing.".into(),
                );
            }
        }
    }
    fn notify_animation(&mut self) {
        if let (Some((marker, _)), Some(runtime)) = (&self.presentation, self.runtime.upgrade()) {
            if let Err(error) = runtime.request_effect_presentation(Arc::downgrade(marker)) {
                self.error = Some(error.to_string());
            }
        }
    }
    fn toggle_animation(
        &mut self,
        parameter: usize,
        axis: i32,
        expected: Option<Animation>,
        enabled: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.presentation.is_none() {
            return;
        }
        let result = (|| {
            let filter = self.node.filter().ok_or("Polygon source is unavailable.")?;
            let p = filter
                .parameter(parameter as i32)
                .map_err(|e| e.to_string())?;
            let current = p.retained_animation().map_err(|e| e.to_string())?;
            if !same_animation(current.as_ref(), expected.as_ref()) {
                return Err("The animation changed. Try again.".into());
            }
            set_animation(&p, axis, enabled)
        })();
        self.error = result.err();
        self.sync_animation(window, cx);
        self.notify_animation();
        cx.notify();
    }
    fn apply_curves(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.presentation.is_none() {
            return;
        }
        let result = (|| {
            let filter = self.node.filter().ok_or("Polygon source is unavailable.")?;
            let mut changes = Vec::new();
            for draft in &self.curves {
                let texts: [String; 2] =
                    std::array::from_fn(|i| draft.fields[i].read(cx).value().to_string());
                if texts == draft.synced {
                    continue;
                }
                let expected = draft
                    .curve
                    .as_ref()
                    .ok_or("The animation curve is unavailable.")?;
                let p = filter
                    .parameter(draft.parameter as i32)
                    .map_err(|e| e.to_string())?;
                let actual = animation_axis(&p, draft.axis)?
                    .1
                    .ok_or("The animation curve changed. Revert curve edits before continuing.")?;
                if !actual.same_reference(expected) {
                    return Err(
                        "The animation curve changed. Revert curve edits before continuing.".into(),
                    );
                }
                let mut values = [None; 2];
                for side in 0..2 {
                    if texts[side] != draft.synced[side] {
                        let value = texts[side]
                            .parse::<f32>()
                            .map_err(|_| format!("{} curve output needs a number.", draft.label))?;
                        if !value.is_finite() {
                            return Err(format!(
                                "{} curve output needs a finite number.",
                                draft.label
                            ));
                        }
                        values[side] = Some(value);
                    }
                }
                changes.push((actual, values));
            }
            for (curve, [lower, upper]) in changes {
                if let Some(value) = lower {
                    curve.set_minimum(value);
                }
                if let Some(value) = upper {
                    curve.set_maximum(value);
                }
            }
            Ok::<_, String>(())
        })();
        self.error = result.err();
        if self.error.is_none() {
            for draft in &mut self.curves {
                if let Some(curve) = &draft.curve {
                    draft.synced = [curve.minimum().to_string(), curve.maximum().to_string()];
                    for (field, value) in draft.fields.iter().zip(&draft.synced) {
                        field.update(cx, |input, cx| input.set_value(value.clone(), window, cx));
                    }
                }
            }
        }
        self.notify_animation();
        cx.notify();
    }
    fn revert_curves(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for draft in &mut self.curves {
            draft.synced = std::array::from_fn(|i| draft.fields[i].read(cx).value().to_string());
        }
        self.error = None;
        self.sync_animation(window, cx);
        cx.notify();
    }
    fn layer_action(
        &mut self,
        location: (usize, i32),
        curve: &Curve,
        layers: &[Option<Layer>],
        action: LayerAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (parameter, axis) = location;
        if self.presentation.is_none() {
            return;
        }
        let result = (|| {
            let filter = self.node.filter().ok_or("Polygon source is unavailable.")?;
            let property = filter
                .parameter(parameter as i32)
                .map_err(|e| e.to_string())?;
            let current = animation_axis(&property, axis)?
                .1
                .ok_or("The animation curve changed.")?;
            if !current.same_reference(curve) || !same_layers(&current.layers(), layers) {
                return Err("The animation layers changed. Open the menu again.".into());
            }
            match action {
                LayerAction::Add(kind) => current
                    .insert(0, Some(&Layer::new(kind)))
                    .map_err(|e| e.to_string())?,
                LayerAction::AddNoise => current
                    .insert(0, Some(&Layer::new_noise().map_err(|e| e.to_string())?))
                    .map_err(|e| e.to_string())?,
                LayerAction::AddCustom => current
                    .insert(0, Some(&Layer::new_custom()))
                    .map_err(|e| e.to_string())?,
                LayerAction::Remove(index) => {
                    let selected =
                        layers
                            .get(index)
                            .and_then(Option::as_ref)
                            .is_some_and(|layer| {
                                self.layer_editor
                                    .as_ref()
                                    .is_some_and(|editor| editor.is_layer(&current, layer))
                            });
                    if selected
                        && self
                            .layer_editor
                            .as_ref()
                            .is_some_and(|editor| editor.is_changed(cx))
                    {
                        return Err(
                            "Apply or revert layer edits before removing this layer.".into()
                        );
                    }
                    current.remove(index as i32).map_err(|e| e.to_string())?;
                    if selected {
                        self.layer_editor = None;
                    }
                }
                LayerAction::Move(from, to) => {
                    let layer = layers
                        .get(from)
                        .ok_or("The animation layer changed.")?
                        .clone();
                    current.remove(from as i32).map_err(|e| e.to_string())?;
                    current
                        .insert(to as i32, layer.as_ref())
                        .map_err(|e| e.to_string())?;
                }
                LayerAction::Edit(index) => {
                    let layer = layers
                        .get(index)
                        .and_then(Option::as_ref)
                        .ok_or("The animation layer is unavailable.")?;
                    if self
                        .layer_editor
                        .as_ref()
                        .is_some_and(|editor| editor.is_changed(cx))
                    {
                        return Err(
                            "Apply or revert layer edits before selecting another layer.".into(),
                        );
                    }
                    self.layer_editor = Some(layer_editor::LayerEditor::new(
                        parameter,
                        axis,
                        current.clone(),
                        layer.clone(),
                        window,
                        cx,
                    )?);
                }
                LayerAction::Blend(index, expected, value) => {
                    let layer = layers
                        .get(index)
                        .and_then(Option::as_ref)
                        .ok_or("The animation layer is unavailable.")?;
                    let blend = layer
                        .control(13)
                        .map_err(|e| e.to_string())?
                        .ok_or("The blend control is unavailable.")?;
                    if blend.enumeration().map_err(|e| e.to_string())? != expected {
                        return Err("The layer blend changed. Open the menu again.".into());
                    }
                    blend.set_enumeration(value).map_err(|e| e.to_string())?;
                }
            }
            Ok::<_, String>(())
        })();
        self.error = result.err();
        self.notify_animation();
        cx.notify();
    }
    fn set_curve_wrap(
        &mut self,
        parameter: usize,
        axis: i32,
        curve: &Curve,
        expected: bool,
        value: bool,
        cx: &mut Context<Self>,
    ) {
        if self.presentation.is_none() {
            return;
        }
        let result = (|| {
            let filter = self.node.filter().ok_or("Polygon source is unavailable.")?;
            let property = filter
                .parameter(parameter as i32)
                .map_err(|e| e.to_string())?;
            let current = animation_axis(&property, axis)?
                .1
                .ok_or("The animation curve changed.")?;
            if !current.same_reference(curve) || current.is_wrapped() != expected {
                return Err("The animation curve changed. Try again.".into());
            }
            current.set_wrapped(value);
            Ok::<_, String>(())
        })();
        self.error = result.err();
        self.notify_animation();
        cx.notify();
    }
    fn render_layers(
        &self,
        draft: &CurveDraft,
        curve: &Curve,
        disabled: bool,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let parameter = draft.parameter;
        let axis = draft.axis;
        let id = format!("polygon-animation-{parameter}-{axis}");
        let layers = curve.layers();
        let description = if layers.is_empty() {
            "No layers".to_string()
        } else {
            layers
                .iter()
                .enumerate()
                .map(|(i, layer)| {
                    format!(
                        "{}: {}",
                        i + 1,
                        layer
                            .as_ref()
                            .map(|l| format!("{:?}", l.kind()))
                            .unwrap_or_else(|| "Empty slot".into())
                    )
                })
                .collect::<Vec<_>>()
                .join(" · ")
        };
        let owner = cx.entity().downgrade();
        let activation = self.presentation.as_ref().map(|v| Arc::downgrade(&v.0));
        let make_callback = |action: LayerAction| {
            let owner = owner.clone();
            let activation = activation.clone();
            let curve = curve.clone();
            let layers = layers.clone();
            move |_: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut App| {
                if !activation
                    .as_ref()
                    .and_then(Weak::upgrade)
                    .is_some_and(|v| v.load(Ordering::Acquire))
                {
                    return;
                }
                let _ = owner.update(cx, |view, cx| {
                    view.layer_action(
                        (parameter, axis),
                        &curve,
                        &layers,
                        action.clone(),
                        window,
                        cx,
                    )
                });
            }
        };
        let mut add_callbacks: Vec<_> = [
            ("Sine", AnalyticCurveKind::Sine),
            ("Linear", AnalyticCurveKind::Linear),
            ("Arc", AnalyticCurveKind::Arc),
            ("Bell", AnalyticCurveKind::Bell),
            ("Gain", AnalyticCurveKind::Gain),
            ("Steps", AnalyticCurveKind::Steps),
        ]
        .into_iter()
        .map(|(name, kind)| (name, make_callback(LayerAction::Add(kind))))
        .collect();
        add_callbacks.push(("Noise", make_callback(LayerAction::AddNoise)));
        add_callbacks.push(("Custom", make_callback(LayerAction::AddCustom)));
        let remove_callbacks: Vec<_> = layers
            .iter()
            .enumerate()
            .map(|(index, layer)| {
                (
                    format!(
                        "Remove {}: {}",
                        index + 1,
                        layer
                            .as_ref()
                            .map(|l| format!("{:?}", l.kind()))
                            .unwrap_or_else(|| "Empty slot".into())
                    ),
                    make_callback(LayerAction::Remove(index)),
                )
            })
            .collect();
        let edit_callbacks: Vec<_> = layers
            .iter()
            .enumerate()
            .filter_map(|(index, layer)| {
                layer.as_ref().map(|layer| {
                    (
                        format!("Edit {}: {:?}", index + 1, layer.kind()),
                        make_callback(LayerAction::Edit(index)),
                    )
                })
            })
            .collect();
        let mut move_callbacks = Vec::new();
        for (index, layer) in layers.iter().enumerate() {
            let label = layer
                .as_ref()
                .map(|l| format!("{:?}", l.kind()))
                .unwrap_or_else(|| "Empty slot".into());
            if index > 0 {
                move_callbacks.push((
                    format!("Move {}: {label} up", index + 1),
                    make_callback(LayerAction::Move(index, index - 1)),
                ));
            }
            if index + 1 < layers.len() {
                move_callbacks.push((
                    format!("Move {}: {label} down", index + 1),
                    make_callback(LayerAction::Move(index, index + 1)),
                ));
            }
        }
        let mut blend_callbacks = Vec::new();
        let blend_names = [
            "Replace",
            "Add",
            "Multiply",
            "Scale from 0.5",
            "Scale from 1",
            "Minimum",
            "Maximum",
            "Compose",
        ];
        for (index, layer) in layers.iter().enumerate() {
            if let Some(layer) = layer
                && let Ok(Some(control)) = layer.control(13)
                && let Ok(old) = control.enumeration()
            {
                for (value, name) in blend_names.iter().enumerate() {
                    blend_callbacks.push((
                        format!("{}: {:?} · {name}", index + 1, layer.kind()),
                        old == value as i32,
                        make_callback(LayerAction::Blend(index, old, value as i32)),
                    ));
                }
            }
        }
        let expected_wrap = curve.is_wrapped();
        let wrap_curve = curve.clone();
        let wrap_activation = activation.clone();
        div()
            .v_flex()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(description),
            )
            .child(
                Checkbox::new(format!("{id}-wrap"))
                    .label("Wrap output")
                    .checked(expected_wrap)
                    .disabled(disabled)
                    .on_change(cx.listener(move |view, checked, _, cx| {
                        if !wrap_activation
                            .as_ref()
                            .and_then(Weak::upgrade)
                            .is_some_and(|v| v.load(Ordering::Acquire))
                        {
                            return;
                        }
                        view.set_curve_wrap(
                            parameter,
                            axis,
                            &wrap_curve,
                            expected_wrap,
                            *checked,
                            cx,
                        )
                    })),
            )
            .child(
                div()
                    .h_flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        Button::new(format!("{id}-add-layer"))
                            .label("Add layer")
                            .disabled(disabled)
                            .dropdown_menu(move |mut menu, _, _| {
                                for (name, callback) in &add_callbacks {
                                    menu = menu
                                        .item(PopupMenuItem::new(*name).on_click(callback.clone()));
                                }
                                menu
                            }),
                    )
                    .child(
                        Button::new(format!("{id}-edit-layer"))
                            .label("Edit layer")
                            .disabled(disabled || edit_callbacks.is_empty())
                            .dropdown_menu(move |mut menu, _, _| {
                                for (name, callback) in &edit_callbacks {
                                    menu = menu.item(
                                        PopupMenuItem::new(name.clone()).on_click(callback.clone()),
                                    );
                                }
                                menu.scrollable(true)
                            }),
                    )
                    .child(
                        Button::new(format!("{id}-remove-layer"))
                            .label("Remove layer")
                            .disabled(disabled || layers.is_empty())
                            .dropdown_menu(move |mut menu, _, _| {
                                for (name, callback) in &remove_callbacks {
                                    menu = menu.item(
                                        PopupMenuItem::new(name.clone()).on_click(callback.clone()),
                                    );
                                }
                                menu.scrollable(true)
                            }),
                    )
                    .child(
                        Button::new(format!("{id}-move-layer"))
                            .label("Move layer")
                            .disabled(disabled || layers.len() < 2)
                            .dropdown_menu(move |mut menu, _, _| {
                                for (name, callback) in &move_callbacks {
                                    menu = menu.item(
                                        PopupMenuItem::new(name.clone()).on_click(callback.clone()),
                                    );
                                }
                                menu.scrollable(true)
                            }),
                    )
                    .child(
                        Button::new(format!("{id}-blend-layer"))
                            .label("Layer blend")
                            .disabled(disabled || blend_callbacks.is_empty())
                            .dropdown_menu(move |mut menu, _, _| {
                                for (name, selected, callback) in &blend_callbacks {
                                    menu = menu.item(
                                        PopupMenuItem::new(name.clone())
                                            .checked(*selected)
                                            .on_click(callback.clone()),
                                    );
                                }
                                menu.scrollable(true)
                            }),
                    ),
            )
            .into_any_element()
    }
    fn apply_time(&mut self, cx: &mut Context<Self>) {
        let Some((marker, _)) = &self.presentation else {
            return;
        };
        match self.time.read(cx).value().parse::<f64>() {
            Ok(time) if time.is_finite() && (0.0..=1.0).contains(&time) => {
                self.time_synced = self.time.read(cx).value().to_string();
                self.error = None;
                cx.emit(SampleTime {
                    value: time,
                    activation: Arc::downgrade(marker),
                });
            }
            _ => self.error = Some("Sample time must be a number from 0 to 1.".into()),
        }
        cx.notify();
    }
    pub(super) fn render_animation(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let disabled = self.presentation.is_none();
        let filter = self.node.filter().expect("Polygon filter");
        let mut content = div().v_flex().gap_3().child(
            Field::new().label("Workspace sample time (0–1)").child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(
                        Input::new(&self.time)
                            .id("polygon-time")
                            .aria_label("Workspace sample time")
                            .disabled(disabled),
                    )
                    .child(
                        Button::new("set-polygon-time")
                            .label("Set time")
                            .disabled(disabled)
                            .on_click(cx.listener(|v, _, _, cx| v.apply_time(cx))),
                    ),
            ),
        );
        for draft in &self.curves {
            let parameter = draft.parameter;
            let axis = draft.axis;
            let p = filter
                .parameter(parameter as i32)
                .expect("Polygon parameter");
            let status = animation_axis(&p, axis);
            let enabled = status.as_ref().is_ok_and(|v| v.0);
            let has_curve = status.as_ref().is_ok_and(|v| v.1.is_some());
            let expected = p.retained_animation().ok().flatten();
            let id = format!("polygon-animation-{parameter}-{axis}");
            content = content.child(
                div()
                    .v_flex()
                    .gap_2()
                    .child(
                        Checkbox::new(id.clone())
                            .label(format!("Animate {}", draft.label))
                            .checked(enabled)
                            .disabled(disabled || status.is_err())
                            .on_change(cx.listener(move |view, checked, window, cx| {
                                view.toggle_animation(
                                    parameter,
                                    axis,
                                    expected.clone(),
                                    *checked,
                                    window,
                                    cx,
                                )
                            })),
                    )
                    .child(Form::new().columns(2).children((0..2).map(|side| {
                        let label = if side == 0 { "Lower" } else { "Upper" };
                        Field::new().label(label).child(
                            Input::new(&draft.fields[side])
                                .id(format!("{id}-{}", label.to_lowercase()))
                                .aria_label(format!("{} curve {label}", draft.label))
                                .disabled(disabled || !has_curve),
                        )
                    })))
                    .when_some(status.ok().and_then(|v| v.1), |this, curve| {
                        this.child(self.render_layers(draft, &curve, disabled, cx))
                    })
                    .when(
                        self.layer_editor.as_ref().is_some_and(|editor| {
                            editor.parameter == parameter && editor.axis == axis
                        }),
                        |this| this.child(self.render_layer_editor(cx)),
                    ),
            );
        }
        content = content.child(
            div()
                .h_flex()
                .flex_wrap()
                .gap_2()
                .child(
                    Button::new("apply-polygon-curves")
                        .label("Apply curves")
                        .disabled(disabled)
                        .on_click(cx.listener(|v, _, window, cx| v.apply_curves(window, cx))),
                )
                .child(
                    Button::new("revert-polygon-curves")
                        .label("Revert curve edits")
                        .disabled(disabled)
                        .on_click(cx.listener(|v, _, window, cx| v.revert_curves(window, cx))),
                ),
        );
        Collapsible::new()
            .open(self.show_animation)
            .child(
                Button::new("toggle-polygon-animation")
                    .label(if self.show_animation {
                        "Hide animation"
                    } else {
                        "Show animation"
                    })
                    .on_click(cx.listener(|v, _, _, cx| {
                        v.show_animation = !v.show_animation;
                        cx.notify();
                    })),
            )
            .content(content)
            .into_any_element()
    }
}
