//! Main workspace presentation for the retained Polygon graph node.
use crate::{
    ForegroundError, color_binding_effects::BindingRuntime, graph_document::inspector::Inspector,
};
use gpui_kit::base::{Disableable, StyledExt, TestSupportExt};
use gpui_kit::component::{
    ActiveTheme,
    button::{Button, ButtonVariants},
    form::{Field, Form},
    input::Input,
    menu::{DropdownMenu, PopupMenuItem},
};
use gpui_kit::{App, Context, Focusable, IntoElement, Render, Window, div, prelude::*};
use mycela_engine::{PointwiseNode, PointwisePropertyHandler};
use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, Ordering},
};

gpui_kit::actions!(polygon_source, [ApplyPolygon, RevertPolygon]);
#[path = "polygon_animation.rs"]
mod animation;
use animation::{CurveDraft, layer_editor::LayerEditor};
impl gpui_kit::EventEmitter<crate::spatial_filter::SampleTime> for PolygonSourceView {}

pub(super) struct PolygonSourceView {
    node: PointwiseNode<ForegroundError>,
    inspector: Inspector,
    runtime: Weak<BindingRuntime>,
    presentation: Option<(Arc<AtomicBool>, PointwisePropertyHandler<ForegroundError>)>,
    error: Option<String>,
    curves: Vec<CurveDraft>,
    layer_editor: Option<LayerEditor>,
    time: gpui_kit::Entity<gpui_kit::component::input::InputState>,
    time_synced: String,
    show_animation: bool,
}
impl PolygonSourceView {
    pub(super) fn new(
        node: PointwiseNode<ForegroundError>,
        inspector: Inspector,
        runtime: &Arc<BindingRuntime>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.bind_keys([
            gpui_kit::KeyBinding::new("secondary-enter", ApplyPolygon, Some("PolygonSource")),
            gpui_kit::KeyBinding::new("escape", RevertPolygon, Some("PolygonSource")),
        ]);
        let curves = animation::drafts(&node, window, cx);
        let time =
            cx.new(|cx| gpui_kit::component::input::InputState::new(window, cx).default_value("0"));
        Self {
            curves,
            layer_editor: None,
            time,
            time_synced: "0".into(),
            show_animation: false,
            node,
            inspector,
            runtime: Arc::downgrade(runtime),
            presentation: None,
            error: None,
        }
    }
    pub(super) fn node(&self) -> PointwiseNode<ForegroundError> {
        self.node.clone()
    }
    pub(super) fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
    pub(super) fn preferred_focus(&self, cx: &App) -> gpui_kit::FocusHandle {
        self.inspector.drafts[0].input.read(cx).focus_handle(cx)
    }
    pub(super) fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Err(error) = self.inspector.sync_unchanged(window, cx) {
            self.error = Some(error);
        }
        self.sync_animation(window, cx);
        self.sync_layer_editor(window, cx);
        cx.notify();
    }
    pub(super) fn set_active(&mut self, active: bool) {
        self.close();
        if !active {
            return;
        }
        let marker = Arc::new(AtomicBool::new(true));
        let weak = Arc::downgrade(&marker);
        let runtime = self.runtime.clone();
        let handler = PointwisePropertyHandler::new(move |_, _| {
            if !weak.upgrade().is_some_and(|m| m.load(Ordering::Acquire)) {
                return Ok(());
            }
            runtime
                .upgrade()
                .ok_or(ForegroundError::OwnerClosed)?
                .request_effect_presentation(weak.clone())
        });
        for p in self.node.filter().expect("Polygon filter").parameters() {
            p.add_change_handler(handler.clone());
        }
        self.presentation = Some((marker, handler));
    }
    pub(super) fn close(&mut self) {
        if let Some((marker, handler)) = self.presentation.take() {
            marker.store(false, Ordering::Release);
            for p in self.node.filter().expect("Polygon filter").parameters() {
                p.remove_change_handler(&handler);
            }
        }
    }
    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match Inspector::load(&self.node, "polygon", window, cx) {
            Ok(inspector) => {
                self.inspector = inspector;
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
        cx.notify();
    }
    fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.presentation.is_none() {
            return;
        }
        for draft in &self.inspector.drafts {
            if let Err(error) = draft.commit(cx) {
                self.error = Some(error);
                cx.notify();
                return;
            }
        }
        self.reload(window, cx);
    }
    fn choose(&mut self, key: &str, index: i32, window: &mut Window, cx: &mut Context<Self>) {
        if self.presentation.is_none() {
            return;
        }
        if self.inspector.is_changed(cx) {
            self.error = Some("Apply or revert field edits before changing a choice.".into());
            cx.notify();
            return;
        }
        if let Some(choice) = self.inspector.choices.iter().find(|v| v.id == key) {
            if let Err(error) = choice.binding.set(index) {
                self.error = Some(error);
                cx.notify();
                return;
            }
            self.reload(window, cx);
        }
    }
}
impl Drop for PolygonSourceView {
    fn drop(&mut self) {
        self.close();
    }
}
impl Render for PolygonSourceView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let disabled = self.presentation.is_none();
        let owner = cx.entity().downgrade();
        let mut form = Form::new().columns(2);
        for draft in &self.inspector.drafts {
            form = form.child(
                Field::new().label(draft.label.clone()).child(
                    Input::new(&draft.input)
                        .id(draft.id.clone())
                        .aria_label(draft.label.clone())
                        .disabled(disabled),
                ),
            );
        }
        for choice in &self.inspector.choices {
            let options = choice.binding.options().ok();
            let selected = choice.binding.index().ok();
            let label = options
                .as_ref()
                .and_then(|v| selected.and_then(|i| v.entries.get(i as usize)))
                .and_then(|v| v.as_deref())
                .map(String::from_utf16_lossy)
                .unwrap_or_else(|| "Unavailable".into());
            let entries = options
                .as_ref()
                .map(|v| v.entries.clone())
                .unwrap_or_default();
            let key = choice.id.clone();
            let owner = owner.clone();
            let marker = self.presentation.as_ref().map(|(m, _)| Arc::downgrade(m));
            form = form.child(
                Field::new().label(choice.label.clone()).child(
                    Button::new(choice.id.clone())
                        .label(label)
                        .disabled(disabled)
                        .dropdown_menu(move |mut menu, _, _| {
                            for (index, entry) in entries.iter().enumerate() {
                                let label = entry
                                    .as_deref()
                                    .map(String::from_utf16_lossy)
                                    .unwrap_or_else(|| "Unnamed option".into());
                                let owner = owner.clone();
                                let key = key.clone();
                                let expected = options.clone();
                                let marker = marker.clone();
                                menu = menu.item(
                                    PopupMenuItem::new(label)
                                        .checked(selected == Some(index as i32))
                                        .on_click(move |_, window, cx| {
                                            let _ = owner.update(cx, |view, cx| {
                                                let active = marker
                                                    .as_ref()
                                                    .and_then(Weak::upgrade)
                                                    .is_some_and(|v| v.load(Ordering::Acquire));
                                                let unchanged = view
                                                    .inspector
                                                    .choices
                                                    .iter()
                                                    .find(|v| v.id == key)
                                                    .and_then(|v| v.binding.options().ok())
                                                    .is_some_and(|current| {
                                                        expected.as_ref().is_some_and(|old| {
                                                            current.same_snapshot(old)
                                                        })
                                                    });
                                                if active && unchanged {
                                                    view.choose(&key, index as i32, window, cx);
                                                }
                                            });
                                        }),
                                );
                            }
                            menu.scrollable(true)
                        }),
                ),
            );
        }
        let theme = cx.theme().clone();
        div()
            .id("polygon-source")
            .key_context("PolygonSource")
            .v_flex()
            .gap_3()
            .on_action(cx.listener(|v, _: &ApplyPolygon, window, cx| {
                v.apply(window, cx);
                cx.stop_propagation();
            }))
            .on_action(cx.listener(|v, _: &RevertPolygon, window, cx| {
                v.reload(window, cx);
                cx.stop_propagation();
            }))
            .child(form)
            .child(self.render_animation(cx))
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(
                        Button::new("apply-polygon")
                            .label("Apply")
                            .primary()
                            .disabled(disabled)
                            .on_click(cx.listener(|v, _, window, cx| v.apply(window, cx))),
                    )
                    .child(
                        Button::new("revert-polygon")
                            .label("Revert edits")
                            .disabled(disabled)
                            .on_click(cx.listener(|v, _, window, cx| v.reload(window, cx))),
                    ),
            )
            .when_some(self.error.clone(), |this, error| {
                this.child(
                    div()
                        .id("polygon-error")
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
