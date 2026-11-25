//! Workspace preview camera controls. The retained SceneCamera is the owner;
//! fields contain only uncommitted presentation drafts.
use crate::{ForegroundError, color_binding_effects::BindingRuntime};
use gpui_kit::base::{Disableable, StyledExt, TestSupportExt};
use gpui_kit::component::{
    ActiveTheme,
    button::Button,
    form::{Field, Form},
    input::{Input, InputState},
};
use gpui_kit::{AppContext, Context, Entity, IntoElement, Render, Window, div, prelude::*};
use mycela_engine::{CameraDelegate, CameraHandler, CameraOrbitComponent, SceneCamera};
use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, Ordering},
};

struct Presentation {
    active: Arc<AtomicBool>,
    delegate: CameraDelegate<ForegroundError>,
}

pub(crate) struct SceneCameraView {
    camera: SceneCamera<ForegroundError>,
    fields: [Entity<InputState>; 3],
    synced: [String; 3],
    runtime: Weak<BindingRuntime>,
    presentation: Option<Presentation>,
    pending: bool,
    error: Option<String>,
}

impl SceneCameraView {
    pub(crate) fn new(
        runtime: &Arc<BindingRuntime>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let camera = SceneCamera::source_preview();
        let synced = camera
            .orbit_values()
            .expect("preview orbit")
            .map(|v| v.to_string());
        let fields = std::array::from_fn(|index| {
            cx.new(|cx| InputState::new(window, cx).default_value(synced[index].clone()))
        });
        Self {
            camera,
            fields,
            synced,
            runtime: Arc::downgrade(runtime),
            presentation: None,
            pending: false,
            error: None,
        }
    }
    pub(crate) fn camera(&self) -> SceneCamera<ForegroundError> {
        self.camera.clone()
    }
    pub(crate) fn is_pending(&self) -> bool {
        self.pending
    }
    pub(crate) fn set_active(&mut self, active: bool) {
        self.close();
        if !active {
            return;
        }
        let marker = Arc::new(AtomicBool::new(true));
        let weak = Arc::downgrade(&marker);
        let runtime = self.runtime.clone();
        let handler = CameraHandler::new(move |_, _| {
            if !weak.upgrade().is_some_and(|a| a.load(Ordering::Acquire)) {
                return Ok(());
            }
            runtime
                .upgrade()
                .ok_or(ForegroundError::OwnerClosed)?
                .request_effect_presentation(weak.clone())
        });
        let delegate = CameraDelegate::from_handlers(&[handler]);
        self.camera.add_changed(&delegate);
        self.presentation = Some(Presentation {
            active: marker,
            delegate,
        });
    }
    pub(crate) fn close(&mut self) {
        self.pending = false;
        if let Some(presentation) = self.presentation.take() {
            presentation.active.store(false, Ordering::Release);
            self.camera.remove_changed(&presentation.delegate);
        }
    }
    pub(crate) fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(values) = self.camera.orbit_values() {
            for (index, value) in values.iter().enumerate() {
                if self.fields[index].read(cx).value() == self.synced[index] {
                    self.synced[index] = value.to_string();
                    let value = self.synced[index].clone();
                    self.fields[index].update(cx, |v, cx| v.set_value(value, window, cx));
                }
            }
        }
        cx.notify();
    }
    fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending {
            return;
        }
        let Some(presentation) = &self.presentation else {
            return;
        };
        let mut values = [0.; 3];
        let submitted =
            std::array::from_fn::<_, 3, _>(|i| self.fields[i].read(cx).value().to_string());
        for index in 0..3 {
            let Some(value) = submitted[index]
                .parse::<f64>()
                .ok()
                .filter(|v| v.is_finite())
            else {
                self.error = Some(format!(
                    "{} needs a finite number.",
                    ["Elevation", "Azimuth", "Distance"][index]
                ));
                cx.notify();
                return;
            };
            values[index] = value;
        }
        if values[2] <= 0. {
            self.error = Some("Distance must be greater than zero.".into());
            cx.notify();
            return;
        }
        let active = Arc::downgrade(&presentation.active);
        let camera = self.camera.clone();
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
                for (component, value) in [
                    CameraOrbitComponent::Elevation,
                    CameraOrbitComponent::Azimuth,
                    CameraOrbitComponent::Distance,
                ]
                .into_iter()
                .zip(values)
                {
                    if !active.upgrade().is_some_and(|m| m.load(Ordering::Acquire)) {
                        return Ok(());
                    }
                    camera.set_orbit_component(component, value)?;
                }
                Ok::<_, mycela_engine::CameraError<ForegroundError>>(())
            })();
            if !active.upgrade().is_some_and(|m| m.load(Ordering::Acquire)) {
                return;
            }
            let _ = cx.update_window(handle, |_, window, cx| {
                owner.update(cx, |view, cx| {
                    view.pending = false;
                    view.error = result.err().map(|e| e.to_string());
                    if view.error.is_none() {
                        let actual = view.camera.orbit_values().expect("preview orbit");
                        for index in 0..3 {
                            if view.fields[index].read(cx).value() == submitted[index] {
                                view.synced[index] = actual[index].to_string();
                                let value = view.synced[index].clone();
                                view.fields[index]
                                    .update(cx, |v, cx| v.set_value(value, window, cx));
                            }
                        }
                    }
                    view.sync(window, cx);
                })
            });
        })
        .detach();
    }
    fn revert(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending {
            return;
        }
        if let Some(values) = self.camera.orbit_values() {
            for (index, value) in values.iter().enumerate() {
                self.synced[index] = value.to_string();
                let value = self.synced[index].clone();
                self.fields[index].update(cx, |v, cx| v.set_value(value, window, cx));
            }
        }
        self.error = None;
        cx.notify();
    }
}
impl Drop for SceneCameraView {
    fn drop(&mut self) {
        self.close();
    }
}
impl Render for SceneCameraView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let disabled = self.pending || self.presentation.is_none();
        let theme = cx.theme().clone();
        div()
            .id("scene-camera")
            .v_flex()
            .gap_3()
            .child(div().text_sm().font_semibold().child("Preview camera"))
            .child(
                Form::new().columns(2).children(
                    ["Elevation (°)", "Azimuth (°)", "Distance"]
                        .into_iter()
                        .enumerate()
                        .map(|(index, label)| {
                            Field::new().label(label).child(
                                Input::new(&self.fields[index])
                                    .id(format!("camera-{index}"))
                                    .aria_label(label)
                                    .disabled(disabled),
                            )
                        }),
                ),
            )
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(
                        Button::new("apply-camera")
                            .label("Apply camera")
                            .disabled(disabled)
                            .on_click(cx.listener(|view, _, window, cx| view.apply(window, cx))),
                    )
                    .child(
                        Button::new("revert-camera")
                            .label("Revert camera")
                            .disabled(disabled)
                            .on_click(cx.listener(|view, _, window, cx| view.revert(window, cx))),
                    ),
            )
            .when_some(self.error.clone(), |this, error| {
                this.child(
                    div()
                        .id("camera-error")
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
#[path = "scene_camera_view_tests.rs"]
mod tests;
