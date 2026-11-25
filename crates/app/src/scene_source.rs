//! Scene primitive inspector. Draft text is presentation state; committed values
//! remain in the retained modern properties on the actual pointwise node.
use crate::{ForegroundError, color_binding_effects::BindingRuntime};
use gpui_kit::base::{Disableable, StyledExt, TestSupportExt};
use gpui_kit::component::{
    ActiveTheme,
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
    form::{Field, Form},
    input::{Input, InputState},
};
use gpui_kit::{
    App, AppContext, Context, Entity, Focusable, IntoElement, Render, Window, div, prelude::*,
};
use mycela_engine::{
    ModernNodeProperty, ModernNodePropertyHandler, ModernNodeValue, PointwiseNode,
    ScenePrimitiveNode,
};
use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, Ordering},
};

gpui_kit::actions!(scene_source, [ApplyScene, RevertScene]);

#[path = "scene_animation.rs"]
mod animation;
use animation::CurveDraft;

struct Draft {
    property: usize,
    axis: Option<usize>,
    label: String,
    field: Entity<InputState>,
    synced: String,
}
struct Presentation {
    active: Arc<AtomicBool>,
    listener: ModernNodePropertyHandler<ForegroundError>,
}

pub(crate) struct SceneSourceView {
    model: ScenePrimitiveNode<ForegroundError>,
    fields: Vec<Draft>,
    curves: Vec<CurveDraft>,
    time_input: Entity<InputState>,
    time_synced: String,
    preview_time: f64,
    show_animation: bool,
    runtime: Weak<BindingRuntime>,
    presentation: Option<Presentation>,
    pending: bool,
    error: Option<String>,
}
impl SceneSourceView {
    pub(crate) fn new(
        model: ScenePrimitiveNode<ForegroundError>,
        runtime: &Arc<BindingRuntime>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.bind_keys([
            gpui_kit::KeyBinding::new("secondary-enter", ApplyScene, Some("SceneSource")),
            gpui_kit::KeyBinding::new("escape", RevertScene, Some("SceneSource")),
        ]);
        let mut fields = Vec::new();
        for (property, p) in model.properties().iter().enumerate() {
            let name = p
                .name()
                .as_deref()
                .map(String::from_utf16_lossy)
                .unwrap_or_else(|| "Property".into());
            let values: Vec<(Option<usize>, String, String)> = match p.value() {
                ModernNodeValue::Int32(v) => vec![(None, name, v.to_string())],
                ModernNodeValue::Double(v) => vec![(None, name, v.to_string())],
                ModernNodeValue::Vector3D(v) => {
                    let labels = p.component_labels();
                    (0..3)
                        .map(|axis| {
                            let suffix = labels
                                .as_ref()
                                .map(|l| String::from_utf16_lossy(&l[axis]))
                                .unwrap_or_else(|| ["X", "Y", "Z"][axis].into());
                            (Some(axis), format!("{name} {suffix}"), v[axis].to_string())
                        })
                        .collect()
                }
                ModernNodeValue::Boolean(_) => Vec::new(),
            };
            for (axis, label, synced) in values {
                let field = cx.new(|cx| InputState::new(window, cx).default_value(synced.clone()));
                fields.push(Draft {
                    property,
                    axis,
                    label,
                    field,
                    synced,
                });
            }
        }
        let curves = fields.iter().map(|_| CurveDraft::new(window, cx)).collect();
        let time_input = cx.new(|cx| InputState::new(window, cx).default_value("0"));
        Self {
            model,
            fields,
            curves,
            time_input,
            time_synced: "0".into(),
            preview_time: 0.,
            show_animation: false,
            runtime: Arc::downgrade(runtime),
            presentation: None,
            pending: false,
            error: None,
        }
    }
    pub(crate) fn node(&self) -> PointwiseNode<ForegroundError> {
        self.model.node()
    }
    pub(crate) fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
    pub(crate) fn is_pending(&self) -> bool {
        self.pending
    }
    pub(crate) fn preferred_focus(&self, cx: &App) -> gpui_kit::FocusHandle {
        self.fields[0].field.read(cx).focus_handle(cx)
    }
    fn actual_text(p: &ModernNodeProperty<ForegroundError>, axis: Option<usize>) -> String {
        match p.value() {
            ModernNodeValue::Int32(v) => v.to_string(),
            ModernNodeValue::Double(v) => v.to_string(),
            ModernNodeValue::Vector3D(v) => v[axis.expect("vector axis")].to_string(),
            ModernNodeValue::Boolean(_) => unreachable!("boolean has no numeric draft"),
        }
    }
    pub(crate) fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let properties = self.model.properties();
        for draft in &mut self.fields {
            if draft.field.read(cx).value() == draft.synced {
                draft.synced = Self::actual_text(&properties[draft.property], draft.axis);
                draft
                    .field
                    .update(cx, |v, cx| v.set_value(draft.synced.clone(), window, cx));
            }
        }
        self.sync_animation(window, cx);
        cx.notify();
    }
    pub(crate) fn set_active(&mut self, active: bool) {
        self.close();
        if !active {
            return;
        }
        let marker = Arc::new(AtomicBool::new(true));
        let weak = Arc::downgrade(&marker);
        let runtime = self.runtime.clone();
        let listener = ModernNodePropertyHandler::new(move |_, _| {
            if !weak.upgrade().is_some_and(|m| m.load(Ordering::Acquire)) {
                return Ok(());
            }
            runtime
                .upgrade()
                .ok_or(ForegroundError::OwnerClosed)?
                .request_effect_presentation(weak.clone())
        });
        for p in self.model.properties() {
            p.add_change_handler(&listener);
        }
        self.presentation = Some(Presentation {
            active: marker,
            listener,
        });
    }
    pub(crate) fn close(&mut self) {
        self.pending = false;
        if let Some(old) = self.presentation.take() {
            old.active.store(false, Ordering::Release);
            for p in self.model.properties() {
                if let Err(error) = p.remove_change_handler(&old.listener) {
                    if let Some(runtime) = self.runtime.upgrade() {
                        runtime.retain_failure(ForegroundError::Operation(error.to_string()));
                    }
                }
            }
        }
    }
    fn submit(
        &mut self,
        edits: Vec<(usize, ModernNodeValue)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.pending {
            return;
        }
        let Some(presentation) = &self.presentation else {
            return;
        };
        let active = Arc::downgrade(&presentation.active);
        let model = self.model.clone();
        let submitted: Vec<String> = self
            .fields
            .iter()
            .map(|d| d.field.read(cx).value().to_string())
            .collect();
        let applies_numeric = edits
            .iter()
            .any(|(_, value)| !matches!(value, ModernNodeValue::Boolean(_)));
        let handle = window.window_handle();
        self.pending = true;
        cx.notify();
        cx.spawn(async move |owner, cx| {
            let alive = cx
                .update_window(handle, |_, _, cx| owner.read_with(cx, |_, _| ()))
                .is_ok_and(|r| r.is_ok());
            if !alive || !active.upgrade().is_some_and(|m| m.load(Ordering::Acquire)) {
                return;
            }
            let result = (|| {
                for (index, value) in edits {
                    if !active.upgrade().is_some_and(|m| m.load(Ordering::Acquire)) {
                        return Ok(());
                    }
                    let p = model.property(index as i32)?;
                    match value {
                        ModernNodeValue::Int32(v) => p.set_int32(v)?,
                        ModernNodeValue::Double(v) => p.set_double(v)?,
                        ModernNodeValue::Boolean(v) => p.set_boolean(v)?,
                        ModernNodeValue::Vector3D(v) => p.set_vector3d(v)?,
                    }
                }
                Ok::<_, mycela_engine::PointwiseError<ForegroundError>>(())
            })();
            if !active.upgrade().is_some_and(|m| m.load(Ordering::Acquire)) {
                return;
            }
            let _ = cx.update_window(handle, |_, window, cx| {
                owner.update(cx, |view, cx| {
                    view.pending = false;
                    view.error = result.err().map(|e| e.to_string());
                    if view.error.is_none() {
                        let properties = view.model.properties();
                        for (draft, submitted) in view.fields.iter_mut().zip(submitted) {
                            if draft.field.read(cx).value() == submitted
                                && (applies_numeric || draft.synced == submitted)
                            {
                                draft.synced =
                                    Self::actual_text(&properties[draft.property], draft.axis);
                                draft.field.update(cx, |v, cx| {
                                    v.set_value(draft.synced.clone(), window, cx)
                                });
                            }
                        }
                    }
                    view.sync(window, cx);
                })
            });
        })
        .detach();
    }
    fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let properties = self.model.properties();
        let mut values: Vec<ModernNodeValue> = properties.iter().map(|p| p.value()).collect();
        for draft in &self.fields {
            let text = draft.field.read(cx).value();
            let parsed = match &mut values[draft.property] {
                ModernNodeValue::Int32(v) => text.parse::<i32>().map(|n| *v = n).is_ok(),
                ModernNodeValue::Double(v) => text
                    .parse::<f64>()
                    .ok()
                    .filter(|n| n.is_finite())
                    .map(|n| *v = n)
                    .is_some(),
                ModernNodeValue::Vector3D(v) => text
                    .parse::<f64>()
                    .ok()
                    .filter(|n| n.is_finite())
                    .map(|n| v[draft.axis.expect("vector axis")] = n)
                    .is_some(),
                ModernNodeValue::Boolean(_) => unreachable!(),
            };
            if !parsed {
                self.error = Some(format!("{} needs a valid number.", draft.label));
                cx.notify();
                return;
            }
        }
        let edits = values
            .into_iter()
            .enumerate()
            .filter(|(_, v)| !matches!(v, ModernNodeValue::Boolean(_)))
            .collect();
        self.submit(edits, window, cx);
    }
    fn revert(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending {
            return;
        }
        let properties = self.model.properties();
        for draft in &mut self.fields {
            draft.synced = Self::actual_text(&properties[draft.property], draft.axis);
            draft
                .field
                .update(cx, |v, cx| v.set_value(draft.synced.clone(), window, cx));
        }
        self.revert_animation(window, cx);
        self.error = None;
        cx.notify();
    }
}
impl Drop for SceneSourceView {
    fn drop(&mut self) {
        self.close();
    }
}
impl Render for SceneSourceView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let disabled = self.pending || self.presentation.is_none();
        let theme = cx.theme().clone();
        let properties = self.model.properties();
        div()
            .id("scene-source")
            .key_context("SceneSource")
            .v_flex()
            .gap_3()
            .on_action(cx.listener(|view, _: &ApplyScene, window, cx| {
                view.apply(window, cx);
                cx.stop_propagation();
            }))
            .on_action(cx.listener(|view, _: &RevertScene, window, cx| {
                view.revert(window, cx);
                cx.stop_propagation();
            }))
            .child(
                Form::new()
                    .columns(2)
                    .children(self.fields.iter().map(|draft| {
                        let axis = draft.axis.map_or("value".to_string(), |v| v.to_string());
                        Field::new().label(draft.label.clone()).child(
                            Input::new(&draft.field)
                                .id(format!("scene-property-{}-{axis}", draft.property))
                                .aria_label(draft.label.clone())
                                .disabled(disabled),
                        )
                    })),
            )
            .children(properties.iter().enumerate().filter_map(|(index, p)| {
                let ModernNodeValue::Boolean(value) = p.value() else {
                    return None;
                };
                let label = p
                    .name()
                    .as_deref()
                    .map(String::from_utf16_lossy)
                    .unwrap_or_else(|| "Option".into());
                Some(
                    Checkbox::new(format!("scene-property-{index}-boolean"))
                        .label(label)
                        .checked(value)
                        .disabled(disabled)
                        .on_change(cx.listener(move |view, checked, window, cx| {
                            view.submit(
                                vec![(index, ModernNodeValue::Boolean(*checked))],
                                window,
                                cx,
                            )
                        })),
                )
            }))
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(
                        Button::new("apply-scene")
                            .label("Apply")
                            .primary()
                            .disabled(disabled)
                            .on_click(cx.listener(|view, _, window, cx| view.apply(window, cx))),
                    )
                    .child(
                        Button::new("revert-scene")
                            .label("Revert edits")
                            .disabled(disabled)
                            .on_click(cx.listener(|view, _, window, cx| view.revert(window, cx))),
                    ),
            )
            .child(self.render_animation(cx))
            .when_some(self.error.clone(), |this, error| {
                this.child(
                    div()
                        .id("scene-error")
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

#[cfg(test)]
#[path = "scene_source_tests.rs"]
mod tests;
