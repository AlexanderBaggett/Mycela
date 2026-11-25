//! View-local rendering of the Texture Synthesis editor's preview scopes.
use super::*;
use crate::solid_color::map_preview::PreviewSelection;
use mycela_engine::{ImageGeometry, StyleEdit, SynthesisRecord};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::graph_document::view) enum PreviewMode {
    Whole,
    Current,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::graph_document::view) enum TextureContext {
    Standalone,
    Embedded,
    Lab,
}

enum Owner {
    Standalone(u64),
    Embedded {
        node: u64,
        canvas_layer: Option<usize>,
        style: StyleEdit,
        pass: usize,
    },
    Lab(SynthesisAdjuster),
}

impl Owner {
    fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Standalone(a), Self::Standalone(b)) => a == b,
            (Self::Lab(a), Self::Lab(b)) => a.same_reference(b),
            (
                Self::Embedded {
                    node,
                    canvas_layer,
                    style,
                    pass,
                },
                Self::Embedded {
                    node: old_node,
                    canvas_layer: old_layer,
                    style: old_style,
                    pass: old_pass,
                },
            ) => {
                node == old_node
                    && canvas_layer == old_layer
                    && style.corresponding_pass(old_style, *old_pass) == Some(*pass)
            }
            _ => false,
        }
    }

    fn context(&self) -> TextureContext {
        match self {
            Self::Standalone(_) => TextureContext::Standalone,
            Self::Embedded { .. } => TextureContext::Embedded,
            Self::Lab(_) => TextureContext::Lab,
        }
    }
}

#[derive(PartialEq, Eq)]
struct Request {
    revision: u64,
    time: u64,
    library: Option<PathBuf>,
    layer: Option<usize>,
}

struct Render {
    request: Request,
    ticket: Ticket,
    result: Option<Result<model::Preview, String>>,
}

impl Drop for Render {
    fn drop(&mut self) {
        self.ticket.retire();
    }
}

#[derive(Default)]
pub(in crate::graph_document::view) struct PreviewState {
    owner: Option<Owner>,
    mode: Option<PreviewMode>,
    render: Option<Render>,
}

struct Job {
    texture: Option<SynthesisRecord>,
    upstream: Option<model::PendingPreview>,
    layer: Option<usize>,
}

impl Job {
    fn run(
        self,
        ticket: &Ticket,
        library: &mycela_engine::EnvironmentResourceLibrary,
    ) -> Result<model::Preview, String> {
        let snapshot = self
            .upstream
            .map(model::PendingPreview::materialize)
            .transpose()?;
        render_synthesis_preview_frame(
            snapshot.as_ref(),
            self.texture.as_ref(),
            self.layer,
            ticket,
            library,
        )
    }
}

pub(in crate::graph_document::view) fn render_synthesis_preview_frame(
    snapshot: Option<&mycela_engine::PointwiseNodeRenderSnapshot<ForegroundError>>,
    texture: Option<&SynthesisRecord>,
    layer: Option<usize>,
    ticket: &Ticket,
    library: &mycela_engine::EnvironmentResourceLibrary,
) -> Result<model::Preview, String> {
    let image = crate::scene_backend::render_synthesis_preview(
        snapshot,
        texture,
        layer,
        ImageGeometry::new(LIVE_PREVIEW_SIZE, LIVE_PREVIEW_SIZE),
        &|| !ticket.is_current(),
        library,
    )
    .map_err(model::failure)?;
    let result = model::Preview::project(&image, &PreviewSelection::BaseColor);
    image.dispose();
    result
}

pub(in crate::graph_document::view) struct SynthesisDockTarget {
    pub(in crate::graph_document::view) node: Node,
    pub(in crate::graph_document::view) layer: Option<usize>,
    pub(in crate::graph_document::view) paint: Option<SynthesisDockPaint>,
}

pub(in crate::graph_document::view) struct SynthesisDockPaint {
    pub(in crate::graph_document::view) canvas_layer: Option<usize>,
    pub(in crate::graph_document::view) pass: usize,
    pub(in crate::graph_document::view) input: Option<Node>,
}

impl GraphDocumentView {
    pub(in crate::graph_document::view) fn synthesis_dock_target(
        &self,
    ) -> Option<SynthesisDockTarget> {
        self.synthesis_preview_mode()?;
        let panel = self.synthesis.as_ref()?;
        let layer = self.synthesis_preview_layer().map(|(index, _, _)| index);
        let paint = match self.synthesis_preview_context()? {
            TextureContext::Standalone => None,
            TextureContext::Embedded => {
                let (_, pass) = self.style.as_ref()?.synthesis_style_draft()?;
                let canvas_layer = self
                    .style
                    .as_ref()?
                    .canvas_layer()
                    .map(|layer| layer.identity());
                let needs_input = match layer {
                    Some(index) => panel.edit.layer_uses_input(index),
                    None => panel.edit.record().uses_input(),
                };
                Some(SynthesisDockPaint {
                    pass,
                    canvas_layer,
                    input: needs_input
                        .then(|| self.embedded_synthesis_input(panel))
                        .flatten(),
                })
            }
            TextureContext::Lab => return None,
        };
        Some(SynthesisDockTarget {
            node: panel.node.clone(),
            layer,
            paint,
        })
    }

    fn synthesis_preview_owner(&self) -> Option<Owner> {
        let panel = self.synthesis.as_ref()?;
        if panel.lab_material.is_some() {
            return panel.editing_material.clone().map(Owner::Lab);
        }
        let node = panel.node.occurrence_id();
        if panel.node.kind() == Some(mycela_engine::PointwiseKind::Synthesis) {
            return Some(Owner::Standalone(node));
        }
        let (style, pass) = self.style.as_ref()?.synthesis_style_draft()?;
        let canvas_layer = self
            .style
            .as_ref()?
            .canvas_layer()
            .map(|layer| layer.identity());
        Some(Owner::Embedded {
            node,
            canvas_layer,
            style: style.clone(),
            pass,
        })
    }

    pub(in crate::graph_document::view) fn synthesis_preview_context(
        &self,
    ) -> Option<TextureContext> {
        if !self.supports_preview_maps() {
            return None;
        }
        let context = self.synthesis_preview.owner.as_ref()?.context();
        (!self.preview_lab_section || context == TextureContext::Lab).then_some(context)
    }

    pub(in crate::graph_document::view) fn synthesis_preview_mode(&self) -> Option<PreviewMode> {
        self.synthesis_preview_context()?;
        self.synthesis_preview.mode
    }

    pub(in crate::graph_document::view) fn synthesis_whole_is_selection(&self) -> bool {
        self.synthesis_preview_context() == Some(TextureContext::Standalone)
            && self
                .synthesis
                .as_ref()
                .zip(self.selection_preview_target().as_ref())
                .is_some_and(|(panel, selection)| panel.node.same_reference(selection))
    }

    pub(in crate::graph_document::view) fn synthesis_preview_layer(
        &self,
    ) -> Option<(usize, usize, SynthesisLayerKind)> {
        if self.synthesis_preview_mode() != Some(PreviewMode::Current) {
            return None;
        }
        let panel = self.synthesis.as_ref()?;
        let index = panel.layer.unwrap_or(0);
        let (id, kind) = panel.edit.layer(index)?;
        Some((index, id, kind))
    }

    pub(in crate::graph_document::view) fn synthesis_preview_status(&self) -> Option<String> {
        let mode = self.synthesis_preview_mode()?;
        if self
            .synthesis_preview
            .render
            .as_ref()
            .is_none_or(|render| render.result.is_none())
        {
            return Some("Rendering texture…".into());
        }
        let label = match (mode, self.synthesis_preview_layer()) {
            (PreviewMode::Current, Some((index, _, kind))) => {
                format!("Layer {} · {}", index + 1, kind.label())
            }
            _ => "Whole texture".into(),
        };
        Some(format!(
            "{LIVE_PREVIEW_SIZE} × {LIVE_PREVIEW_SIZE} · {label}"
        ))
    }

    pub(in crate::graph_document::view) fn synthesis_preview_frame(
        &self,
    ) -> Option<&model::Preview> {
        self.synthesis_preview_mode()?;
        self.synthesis_preview
            .render
            .as_ref()?
            .result
            .as_ref()?
            .as_ref()
            .ok()
    }

    pub(in crate::graph_document::view) fn synthesis_preview_error(&self) -> Option<&str> {
        self.synthesis_preview_mode()?;
        self.synthesis_preview
            .render
            .as_ref()?
            .result
            .as_ref()?
            .as_ref()
            .err()
            .map(String::as_str)
    }

    pub(in crate::graph_document::view) fn set_synthesis_preview_mode(
        &mut self,
        mode: Option<PreviewMode>,
        cx: &mut Context<Self>,
    ) {
        if self.closed || self.is_busy() || self.synthesis_preview_context().is_none() {
            return;
        }
        if self.synthesis_preview.mode != mode {
            self.synthesis_preview.mode = mode;
            self.pause_preview();
            self.refresh_synthesis_preview(cx);
            cx.notify();
        }
    }

    fn preview_job(&self, layer: Option<usize>) -> Result<Job, String> {
        let panel = self.synthesis.as_ref().expect("active texture editor");
        let context = self
            .synthesis_preview_context()
            .expect("active texture preview");
        let (texture, input) = match context {
            TextureContext::Standalone => (None, Some(panel.node.clone())),
            TextureContext::Lab => (Some(panel.edit.record()), None),
            TextureContext::Embedded => (
                Some(panel.edit.record()),
                self.embedded_synthesis_input(panel),
            ),
        };
        let needs_input = texture.as_ref().is_none_or(|texture| match layer {
            Some(layer) => panel.edit.layer_uses_input(layer),
            None => texture.uses_input(),
        });
        let upstream = input
            .filter(|_| needs_input)
            .map(|input| self.freeze_preview(&input, self.sample_time))
            .transpose()?;
        Ok(Job {
            texture,
            upstream,
            layer,
        })
    }

    fn embedded_synthesis_input(&self, panel: &Panel) -> Option<Node> {
        if let Some(canvas_layer) = self.style.as_ref().and_then(style::Panel::canvas_layer) {
            canvas_layer.style_input_node()
        } else {
            panel.node.slot(2).ok().and_then(|slot| slot.node())
        }
    }

    pub(in crate::graph_document::view) fn refresh_synthesis_preview(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let owner = self.synthesis_preview_owner();
        if !self
            .synthesis_preview
            .owner
            .as_ref()
            .zip(owner.as_ref())
            .is_some_and(|(old, new)| new.same(old))
        {
            self.synthesis_preview = PreviewState {
                mode: owner
                    .as_ref()
                    .filter(|owner| owner.context() == TextureContext::Lab)
                    .map(|_| PreviewMode::Whole),
                owner,
                render: None,
            };
        }
        if self.closed || self.synthesis_preview_mode().is_none() {
            self.synthesis_preview.render = None;
            return;
        }
        let layer = self.synthesis_preview_layer();
        let request = Request {
            revision: self.revision,
            time: self.sample_time.to_bits(),
            library: self.environment_library.root().map(Path::to_path_buf),
            layer: layer.map(|(_, id, _)| id),
        };
        if self
            .synthesis_preview
            .render
            .as_ref()
            .is_some_and(|render| render.request == request)
        {
            return;
        }
        let ticket = Ticket::new();
        let job = self.preview_job(layer.map(|(index, _, _)| index));
        let (job, result) = match job {
            Ok(job) => (Some(job), None),
            Err(error) => (None, Some(Err(error))),
        };
        self.synthesis_preview.render = Some(Render {
            request,
            ticket: ticket.clone(),
            result,
        });
        if let Some(job) = job {
            let library = self.environment_library.clone();
            cx.spawn(async move |owner, cx| {
                let worker = ticket.clone();
                let result = cx
                    .background_spawn(async move { job.run(&worker, &library) })
                    .await;
                let _ = owner.update(cx, |view, cx| {
                    if ticket.is_current()
                        && !view.closed
                        && let Some(render) = &mut view.synthesis_preview.render
                    {
                        render.result = Some(result);
                        cx.notify();
                    }
                });
            })
            .detach();
        }
        cx.notify();
    }
}
