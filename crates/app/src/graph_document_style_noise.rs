//! The Style pass owns a static Lab record. The editor works on a private Lab.
use super::*;
use mycela_engine::{ImageGeometry, SharedImageMap};

#[derive(Clone)]
pub(in crate::graph_document::view) struct Host {
    owner: WeakEntity<GraphDocumentView>,
    window: AnyWindowHandle,
    target: Target,
    scope: StyleNoiseScope,
    temporary_input: bool,
}

impl GraphDocumentView {
    pub(super) fn open_style_noise_lab(
        &mut self,
        target: Target,
        scope: StyleNoiseScope,
        cx: &mut Context<Self>,
    ) {
        if self.has_field_drafts(cx) {
            self.error = Some("Correct or reset field text before opening the Noise Lab.".into());
            cx.notify();
            return;
        }
        let Some((panel, pass)) = self.style.as_ref().zip(target.pass) else {
            return;
        };
        let lab = match panel
            .edit
            .open_noise_lab(pass, scope)
            .map_err(model::failure)
        {
            Ok(lab) => lab,
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        let source = match &target.canvas_layer {
            Some(layer) => layer.style_input_node(),
            None => target.node.slot(2).ok().and_then(|slot| slot.node()),
        };
        let host = Host {
            owner: cx.weak_entity(),
            window: self.window,
            target,
            scope,
            temporary_input: false,
        };
        self.error = None;
        let Some(source) = source else {
            self.show_style_noise_lab(host, lab, None, cx);
            return;
        };
        let pending = match model::freeze(&source, self.sample_time) {
            Ok(pending) => pending,
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        self.pause_preview();
        self.work = Some(Work::LoadingNoiseLab);
        let ticket = Ticket::new();
        self.ticket = Some(ticket.clone());
        cx.notify();
        let background = cx.background_executor().clone();
        let library = self.environment_library.clone();
        let handle = self.window;
        cx.spawn(async move |owner, cx| {
            let work = ticket.clone();
            let result = background
                .spawn(async move {
                    let source = pending.materialize()?;
                    let size = LIVE_PREVIEW_SIZE;
                    crate::scene_backend::render_export(
                        &source,
                        ImageGeometry::new(size, size),
                        &|| !work.is_current(),
                        crate::scene_backend::GRAPH_PNG_OPTIONS,
                        None,
                        None,
                        Some(&library),
                    )
                    .map_err(model::failure)
                })
                .await;
            let _ = cx.update_window(handle, |_, _, cx| {
                owner.update(cx, |view, cx| {
                    if !view.owns(&ticket) {
                        return;
                    }
                    view.ticket = None;
                    view.work = None;
                    if host.target.current(view) {
                        match result {
                            Ok(image) => view.show_style_noise_lab(host, lab, Some(image), cx),
                            Err(error) => {
                                view.error = Some(error);
                                cx.notify();
                            }
                        }
                    } else {
                        cx.notify();
                    }
                })
            });
        })
        .detach();
    }

    fn show_style_noise_lab(
        &mut self,
        mut host: Host,
        lab: Node,
        source: Option<SharedImageMap>,
        cx: &mut Context<Self>,
    ) {
        let prepared = (|| {
            if let Some(source) = source {
                let slot = lab.slot(0).map_err(model::failure)?;
                let input = match slot.node() {
                    Some(node) => node,
                    None => {
                        let node = Node::input();
                        slot.set_node_quiet(Some(&node)).map_err(model::failure)?;
                        host.temporary_input = true;
                        node
                    }
                };
                input
                    .set_input_image(Some(source))
                    .map_err(model::failure)?;
            }
            let document = model::Document::new(vec![Some(lab)]).map_err(model::failure)?;
            Ok::<_, String>(Arc::new(Mutex::new(document)))
        })();
        let document = match prepared {
            Ok(document) => document,
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        let title = format!("Edit {}", host.scope.label());
        let opened = gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(gpui_kit::WindowBounds::Windowed(
                    gpui_kit::Bounds::centered(
                        None,
                        gpui_kit::size(gpui_kit::px(880.), gpui_kit::px(640.)),
                        cx,
                    ),
                )),
                ..Default::default()
            },
            cx,
            move |window, cx| {
                window.set_window_title(&title);
                cx.new(|cx| {
                    let mut view =
                        GraphDocumentView::with_document(document, None, 0., false, window, cx);
                    view.embedded_noise = Some(host);
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

    pub(in crate::graph_document::view) fn apply_embedded_noise(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(host) = self.embedded_noise.clone().filter(|_| !self.is_busy()) else {
            return;
        };
        if self.has_field_drafts(cx) {
            self.commit_staged_fields(window, cx);
            if self.error.is_some() || self.has_field_drafts(cx) {
                return;
            }
        }
        let result = (|| {
            let lab = guard(&self.document)
                .branch(0)
                .flatten()
                .ok_or("The Noise Lab is unavailable.")?;
            let slot = lab.slot(0).map_err(model::failure)?;
            let temporary = slot.node().filter(|_| host.temporary_input);
            if temporary.is_some() {
                slot.set_node_quiet(None).map_err(model::failure)?;
            }
            let committed = host
                .window
                .update(cx, |_, window, cx| {
                    host.owner
                        .update(cx, |view, cx| {
                            view.commit_embedded_noise(&host.target, host.scope, &lab, window, cx)
                        })
                        .map_err(model::failure)?
                })
                .map_err(model::failure);
            if let Some(input) = temporary.as_ref() {
                slot.set_node_quiet(Some(input)).map_err(model::failure)?;
            }
            committed?
        })();
        match result {
            Ok(()) => window.remove_window(),
            Err(error) => {
                self.error = Some(error);
                cx.notify();
            }
        }
    }

    fn commit_embedded_noise(
        &mut self,
        target: &Target,
        scope: StyleNoiseScope,
        lab: &Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if self.closed || !target.current(self) {
            return Err("The selected Style pass changed. Open its Noise Lab again.".into());
        }
        let (panel, pass) = self
            .style
            .as_ref()
            .zip(target.pass)
            .ok_or("The Style pass is unavailable.")?;
        let mut edit = panel.edit.clone();
        if edit
            .set_noise_lab(pass, scope, lab)
            .map_err(model::failure)?
        {
            self.commit_style(target, edit, "Edit Noise Lab", None, window, cx);
            if let Some(error) = &self.error {
                return Err(error.clone());
            }
        }
        Ok(())
    }
}
