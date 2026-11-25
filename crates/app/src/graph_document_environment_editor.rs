//! A private Environment document with a single commit back to its owner.
use super::converter::CompositePreview;
use super::{Target, *};
use crate::{export_render::generator_execution, solid_color::map_preview};
use gpui_kit::{ObjectFit, RenderImage, WeakEntity, rems};
use mycela_engine::{EnvironmentLayerPreview, PointwiseKind};

pub(in crate::graph_document::view) struct Host {
    owner: WeakEntity<GraphDocumentView>,
    window: AnyWindowHandle,
    target: Target,
    layer_preview: Entity<LayerPreview>,
    composite_preview: Entity<CompositePreview>,
    _composite_observer: Subscription,
}

#[cfg(test)]
impl Host {
    pub(in crate::graph_document::view) fn layer_preview_pixels<'a>(
        &self,
        cx: &'a App,
    ) -> Option<&'a [u8]> {
        self.layer_preview
            .read(cx)
            .result
            .as_ref()?
            .as_ref()
            .ok()?
            .as_bytes(0)
    }
}

#[derive(Default)]
struct LayerPreview {
    layer: Option<EnvironmentLayerPreview>,
    label: Option<String>,
    ticket: Option<Ticket>,
    result: Option<Result<Arc<RenderImage>, String>>,
}

impl LayerPreview {
    fn retire(&mut self) {
        if let Some(ticket) = self.ticket.take() {
            ticket.retire();
        }
    }

    fn show(
        &mut self,
        selected: Option<(EnvironmentLayerPreview, String)>,
        library: EnvironmentResourceLibrary,
        cx: &mut Context<Self>,
    ) {
        let Some((layer, label)) = selected else {
            if self.layer.take().is_some() {
                self.retire();
                self.label = None;
                self.result = None;
                cx.notify();
            }
            return;
        };
        let same_layer = self
            .layer
            .as_ref()
            .is_some_and(|current| current.same_layer(&layer));
        if self.label.as_ref() != Some(&label) {
            self.label = Some(label);
            cx.notify();
        }
        if same_layer {
            return;
        }
        self.retire();
        self.layer = Some(layer.clone());
        self.result = None;
        let ticket = Ticket::new();
        self.ticket = Some(ticket.clone());
        let work = ticket.clone();
        let task = cx.background_spawn(async move {
            let sphere = layer
                .render_sphere::<ForegroundError>(generator_execution(), &library, &|| {
                    !work.is_current()
                })
                .map_err(model::failure)?;
            map_preview::project(&sphere, &map_preview::PreviewSelection::BaseColor)
                .map(|(_, raster)| raster)
                .map_err(model::failure)
        });
        cx.spawn(async move |owner, cx| {
            let result = task.await;
            let _ = owner.update(cx, |preview, cx| {
                if ticket.is_current() {
                    preview.ticket = None;
                    preview.result = Some(result);
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }
}

impl Drop for LayerPreview {
    fn drop(&mut self) {
        self.retire();
    }
}

impl Render for LayerPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (raster, error) = match &self.result {
            Some(Ok(raster)) => (Some(raster.clone()), None),
            Some(Err(error)) => (
                None,
                Some(format!("Couldn’t render layer preview. {error}")),
            ),
            None => (None, None),
        };
        let label = self
            .label
            .clone()
            .unwrap_or_else(|| "No layer selected".into());
        div()
            .h_flex()
            .flex_shrink_0()
            .items_center()
            .gap_3()
            .pt_2()
            .border_t_1()
            .border_color(theme.border)
            .child(
                div()
                    .relative()
                    .flex_shrink_0()
                    .size(rems(7.5))
                    .rounded_full()
                    .bg(theme.muted)
                    .when_some(raster, |tile, raster| {
                        tile.child(
                            img(raster)
                                .id("environment-layer-preview")
                                .absolute()
                                .size_full()
                                .rounded_full()
                                .object_fit(ObjectFit::Contain)
                                .role(gpui_kit::Role::Image)
                                .aria_label("Selected layer on a sphere")
                                .test_support(),
                        )
                    }),
            )
            .child(
                div()
                    .v_flex()
                    .min_w_0()
                    .gap_1()
                    .child(div().font_semibold().child("Layer preview"))
                    .child(
                        div()
                            .id("environment-layer-preview-label")
                            .test_support()
                            .aria_label(label.clone())
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(label),
                    )
                    .when(self.ticket.is_some(), |content| {
                        content.child(
                            div()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child("Rendering layer…"),
                        )
                    })
                    .when_some(error, |content, error| {
                        content.child(div().text_sm().text_color(theme.danger).child(error))
                    }),
            )
    }
}

impl GraphDocumentView {
    pub(in crate::graph_document::view) fn environment_converter_picker(
        &self,
        cx: &App,
    ) -> Option<impl IntoElement + use<>> {
        let state = &self.embedded_environment.as_ref()?.composite_preview;
        Some(state.read(cx).picker(state))
    }

    pub(in crate::graph_document::view) fn environment_converter_raster(
        &self,
        cx: &App,
    ) -> Option<(Arc<RenderImage>, f32)> {
        let preview = self
            .embedded_environment
            .as_ref()?
            .composite_preview
            .read(cx)
            .projected()?;
        Some((
            preview.image.clone(),
            preview.width as f32 / preview.height as f32,
        ))
    }

    pub(in crate::graph_document::view) fn environment_converter_status(
        &self,
        cx: &App,
    ) -> Option<String> {
        self.embedded_environment
            .as_ref()?
            .composite_preview
            .read(cx)
            .status()
    }

    pub(in crate::graph_document::view) fn environment_converter_error(
        &self,
        cx: &App,
    ) -> Option<String> {
        self.embedded_environment
            .as_ref()?
            .composite_preview
            .read(cx)
            .error()
            .map(str::to_owned)
    }

    pub(in crate::graph_document::view) fn refresh_environment_layer_preview(
        &self,
        cx: &mut Context<Self>,
    ) {
        let Some(host) = &self.embedded_environment else {
            return;
        };
        let selected = self.inspector.environment.as_ref().and_then(|panel| {
            let index = panel.selected()?;
            let map = panel.value();
            let layer = map.layer_preview(index)?;
            let label = map
                .layer_kind(index)
                .map_or("Layer", EnvironmentLayerKind::label);
            Some((layer, format!("{} · {label}", index + 1)))
        });
        host.layer_preview.update(cx, |preview, cx| {
            preview.show(selected, self.environment_library.clone(), cx);
        });
    }

    pub(in crate::graph_document::view) fn render_environment_layer_preview(
        &self,
    ) -> Option<impl IntoElement + use<>> {
        self.embedded_environment
            .as_ref()
            .map(|host| host.layer_preview.clone())
    }

    pub(in crate::graph_document::view) fn publish_working_environment(
        &self,
        cx: &mut Context<Self>,
    ) {
        let Some((host, panel)) = self
            .embedded_environment
            .as_ref()
            .zip(self.inspector.environment.as_ref())
        else {
            return;
        };
        let map = panel.value();
        host.composite_preview
            .update(cx, |preview, cx| preview.update_map(map, cx));
        let _ = host.owner.update(cx, |parent, cx| {
            if !host.target.current(parent) || host.target.panel.validate().is_err() {
                return;
            }
            if Arc::ptr_eq(map, host.target.panel.value()) {
                if parent.environment_preview.take().is_some() {
                    parent.start_preview(cx);
                }
            } else if parent
                .environment_preview
                .as_ref()
                .is_none_or(|previous| !Arc::ptr_eq(map, previous.map()))
            {
                let edit = host
                    .target
                    .panel
                    .preview_override(map.clone(), &guard(&parent.document));
                parent.environment_preview = Some(edit);
                parent.start_preview(cx);
            }
        });
    }

    pub(super) fn open_environment_editor(&mut self, target: &Target, cx: &mut Context<Self>) {
        if self.embedded_environment.is_some() || !self.can_edit_environment(target, cx) {
            return;
        }
        let document = match environment_document(target) {
            Ok(document) => document,
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        let owner = cx.weak_entity();
        let parent_window = self.window;
        let target = target.clone();
        let library = self.environment_library.clone();
        let opened = gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(gpui_kit::WindowBounds::Windowed(
                    gpui_kit::Bounds::centered(
                        None,
                        gpui_kit::size(gpui_kit::px(1040.), gpui_kit::px(760.)),
                        cx,
                    ),
                )),
                window_min_size: Some(gpui_kit::size(gpui_kit::px(640.), gpui_kit::px(480.))),
                ..Default::default()
            },
            cx,
            move |window, cx| {
                window.set_window_title("Mycela · Edit environment map");
                cx.new(|cx| {
                    let mut view = Self::with_document(document, None, 0., false, window, cx);
                    view.error = view
                        .inspector
                        .select_environment_layer(target.panel.selected().unwrap_or(0), window, cx)
                        .err();
                    let library_changed = view.environment_library.root() != library.root();
                    view.environment_library = library;
                    if library_changed {
                        view.start_preview(cx);
                    }
                    let composite_preview = cx.new(|cx| {
                        CompositePreview::new(
                            target.panel.value().clone(),
                            view.environment_library.clone(),
                            cx,
                        )
                    });
                    let composite_observer = cx.observe(&composite_preview, |_, _, cx| cx.notify());
                    view.embedded_environment = Some(Host {
                        owner,
                        window: parent_window,
                        target,
                        layer_preview: cx.new(|_| LayerPreview::default()),
                        composite_preview,
                        _composite_observer: composite_observer,
                    });
                    view.refresh_environment_layer_preview(cx);
                    view.observe_drafts(cx);
                    view
                })
            },
        );
        match opened {
            Ok((child, _)) => self.watch_child_editor(child.window_id(), cx),
            Err(error) => self.error = Some(error.to_string()),
        }
        cx.notify();
    }

    pub(in crate::graph_document::view) fn apply_embedded_environment(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(host) = self
            .embedded_environment
            .as_ref()
            .filter(|_| !self.is_busy())
        else {
            return;
        };
        let owner = host.owner.clone();
        let parent_window = host.window;
        let target = host.target.clone();
        if self.has_field_drafts(cx) {
            self.commit_staged_fields(window, cx);
            if self.error.is_some() || self.has_field_drafts(cx) {
                return;
            }
        }
        let result = (|| {
            let panel = self
                .inspector
                .environment
                .as_ref()
                .ok_or("The working map is unavailable.")?;
            panel.validate()?;
            let map = panel.value();
            if Arc::ptr_eq(map, target.panel.value()) {
                return Ok(());
            }
            let selected = panel.selected().unwrap_or(0);
            parent_window
                .update(cx, |_, window, cx| {
                    owner
                        .update(cx, |view, cx| {
                            if view.closed || !target.current(view) {
                                return Err(
                                    "The selected Environment changed. Open its editor again."
                                        .into(),
                                );
                            }
                            target.panel.validate()?;
                            view.environment_preview = None;
                            view.apply_environment_change(
                                &target.panel,
                                map.as_ref().clone(),
                                selected,
                                window,
                                cx,
                            );
                            view.error.clone().map_or(Ok(()), Err)
                        })
                        .map_err(model::failure)?
                })
                .map_err(model::failure)?
        })();
        match result {
            Ok(()) => window.remove_window(),
            Err(error) => {
                self.error = Some(error);
                cx.notify();
            }
        }
    }
}

fn environment_document(target: &Target) -> Result<LiveDocument, String> {
    let filter = PointwiseFilter::new(PointwiseKind::MakeEnvironment).map_err(model::failure)?;
    filter
        .parameter(0)
        .and_then(|parameter| parameter.set_environment(target.panel.value().clone()))
        .map_err(model::failure)?;
    if let Some(source) = target
        .node
        .as_ref()
        .and_then(Node::filter)
        .filter(|source| source.kind() == PointwiseKind::MakeEnvironment)
    {
        let limit = source.parameter(1).map_err(model::failure)?.index();
        filter
            .parameter(1)
            .and_then(|parameter| parameter.set_index(limit))
            .map_err(model::failure)?;
    }
    let document = model::Document::new(vec![Some(filter.node())]).map_err(model::failure)?;
    Ok(Arc::new(Mutex::new(document)))
}
