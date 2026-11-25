//! Temporary graph and dock images for an open editor dialog.
use super::*;
use crate::solid_color::map_preview::PreviewSelection;
use gpui_kit::{EntityId, RenderImage};
use mycela_engine::{
    EnvironmentResourceLibrary, FrozenRenderBatch, MaterializedRenderBatch, PendingNodeRender,
    RenderFreezeSession, SynthesisRecord,
};
use std::collections::{HashMap, HashSet};

pub(super) struct DraftPreview {
    editor: EntityId,
    _subscription: Subscription,
    ticket: Option<Ticket>,
    dock_pending: bool,
    frame: Option<model::Preview>,
    thumbnails: HashMap<u64, Arc<RenderImage>>,
    error: Option<String>,
}

impl DraftPreview {
    pub(super) fn new(editor: EntityId, subscription: Subscription) -> Self {
        Self {
            editor,
            _subscription: subscription,
            ticket: None,
            dock_pending: false,
            frame: None,
            thumbnails: HashMap::new(),
            error: None,
        }
    }

    fn retire(&mut self) {
        if let Some(ticket) = self.ticket.take() {
            ticket.retire();
        }
    }
}

impl Drop for DraftPreview {
    fn drop(&mut self) {
        self.retire();
    }
}

type PendingDraftNode = PendingNodeRender<ForegroundError>;

pub(super) struct DraftPreviewJob {
    dock: Option<DraftDock>,
    thumbnails: Vec<(u64, PendingDraftNode)>,
    batch: FrozenRenderBatch,
}

enum DraftDock {
    Graph(PendingDraftNode, bool, PreviewSelection),
    Synthesis {
        owner: PendingDraftNode,
        layer: Option<usize>,
        paint: Option<DraftPaint>,
    },
    Material(SynthesisRecord, Option<usize>),
}

struct DraftPaint {
    canvas_layer: Option<usize>,
    pass: usize,
    input: Option<PendingDraftNode>,
}

impl DraftDock {
    fn render(
        self,
        batch: &mut MaterializedRenderBatch<ForegroundError>,
        ticket: &Ticket,
        library: &EnvironmentResourceLibrary,
    ) -> Result<Option<model::Preview>, String> {
        match self {
            Self::Graph(pending, canvas, map) => {
                let frame = pending.complete(batch).map_err(model::failure)?;
                if canvas {
                    model::render_canvas_tile(frame, ticket, Some(library))
                } else {
                    model::render_map(frame, ticket, Some(library), &map)
                }
            }
            Self::Synthesis {
                owner,
                layer,
                paint,
            } => {
                let owner = owner.complete(batch).map_err(model::failure)?;
                if let Some(paint) = paint {
                    let texture = owner
                        .synthesis_editor_record(paint.canvas_layer, paint.pass)
                        .ok_or_else(|| {
                            "This Texture Synthesis pass is no longer available.".to_owned()
                        })?;
                    let input = paint
                        .input
                        .map(|input| input.complete(batch))
                        .transpose()
                        .map_err(model::failure)?;
                    synthesis::render_synthesis_preview_frame(
                        input.as_ref(),
                        Some(&texture),
                        layer,
                        ticket,
                        library,
                    )
                    .map(Some)
                } else {
                    synthesis::render_synthesis_preview_frame(
                        Some(&owner),
                        None,
                        layer,
                        ticket,
                        library,
                    )
                    .map(Some)
                }
            }
            Self::Material(texture, layer) => synthesis::render_synthesis_preview_frame(
                None,
                Some(&texture),
                layer,
                ticket,
                library,
            )
            .map(Some),
        }
    }
}

impl GraphDocumentView {
    pub(super) fn draft_preview_frame(&self) -> Option<&model::Preview> {
        self.draft_preview.as_ref()?.frame.as_ref()
    }

    pub(super) fn draft_preview_thumbnail(&self, id: u64) -> Option<&Arc<RenderImage>> {
        self.draft_preview.as_ref()?.thumbnails.get(&id)
    }

    pub(super) fn draft_preview_error(&self) -> Option<&str> {
        self.draft_preview.as_ref()?.error.as_deref()
    }

    pub(super) fn draft_preview_pending(&self) -> bool {
        self.draft_preview
            .as_ref()
            .is_some_and(|preview| preview.dock_pending)
    }

    pub(super) fn close_draft_preview(&mut self, editor: EntityId) -> bool {
        if self.owns_draft_preview(editor) {
            self.draft_preview = None;
            return true;
        }
        false
    }

    pub(super) fn retire_draft_render(
        &mut self,
        editor: EntityId,
        clear: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(preview) = self
            .draft_preview
            .as_mut()
            .filter(|preview| preview.editor == editor)
        else {
            return;
        };
        preview.retire();
        let had_error = preview.error.take().is_some();
        let was_pending = std::mem::take(&mut preview.dock_pending);
        if clear {
            preview.frame = None;
            preview.thumbnails.clear();
        }
        if clear || had_error || was_pending {
            cx.notify();
        }
    }

    fn owns_draft_preview(&self, editor: EntityId) -> bool {
        self.draft_preview
            .as_ref()
            .is_some_and(|preview| preview.editor == editor)
    }

    pub(super) fn freeze_draft_preview(
        &self,
        mut session: RenderFreezeSession<ForegroundError>,
        material: Option<SynthesisRecord>,
        affected_dock_only: bool,
    ) -> Result<DraftPreviewJob, String> {
        let time = self.sample_time;
        let dock = if let Some(texture) = material
            && self.synthesis_preview_mode().is_some()
        {
            let layer = self.synthesis_preview_layer().map(|(index, _, _)| index);
            Some(DraftDock::Material(texture, layer))
        } else if let Some(target) = self.synthesis_dock_target() {
            let owner = session
                .freeze_node(&target.node, time)
                .map_err(model::failure)?;
            let paint = target
                .paint
                .map(|paint| {
                    let input = paint
                        .input
                        .map(|input| session.freeze_node(&input, time))
                        .transpose()
                        .map_err(model::failure)?;
                    Ok::<_, String>(DraftPaint {
                        canvas_layer: paint.canvas_layer,
                        pass: paint.pass,
                        input,
                    })
                })
                .transpose()?;
            Some(DraftDock::Synthesis {
                owner,
                layer: target.layer,
                paint,
            })
        } else if self.synthesis_preview_mode().is_none()
            && let Some(target) = self.preview_target()
            && (!affected_dock_only || session.affects_draft_preview(&target, time))
        {
            let frame = session.freeze_node(&target, time).map_err(model::failure)?;
            let map = if self.supports_preview_maps() {
                self.preview_maps.requested_for(target.occurrence_id())
            } else {
                PreviewSelection::BaseColor
            };
            Some(DraftDock::Graph(frame, self.drawing.is_some(), map))
        } else {
            None
        };
        let section = self
            .lab_sections
            .selected()
            .filter(|_| self.lab_thumbnail.is_some());
        let mut seen = HashSet::new();
        let mut thumbnails = Vec::new();
        for target in section
            .into_iter()
            .chain(self.node_previews.visible_nodes())
        {
            let id = target.occurrence_id();
            if seen.insert(id) && session.affects_draft_preview(target, time) {
                let frame = session.freeze_node(target, time).map_err(model::failure)?;
                thumbnails.push((id, frame));
            }
        }
        Ok(DraftPreviewJob {
            dock,
            thumbnails,
            batch: session.finish().map_err(model::failure)?,
        })
    }

    pub(super) fn queue_draft_preview(
        &mut self,
        editor: EntityId,
        freeze: impl FnOnce(&Self) -> Result<Option<DraftPreviewJob>, String> + 'static,
        cx: &mut Context<Self>,
    ) {
        let Some(preview) = self
            .draft_preview
            .as_mut()
            .filter(|preview| preview.editor == editor)
        else {
            return;
        };
        preview.retire();
        if preview.error.take().is_some() {
            cx.notify();
        }
        let ticket = Ticket::new();
        preview.ticket = Some(ticket.clone());
        let executor = cx.background_executor().clone();
        let library = self.environment_library.clone();
        let revision = self.revision;
        let time = self.sample_time;
        cx.spawn(async move |owner, cx| {
            let current = |view: &GraphDocumentView| {
                ticket.is_current()
                    && !view.closed
                    && view.revision == revision
                    && view.sample_time == time
                    && view.owns_draft_preview(editor)
            };
            let report_error =
                |view: &mut GraphDocumentView, cx: &mut Context<Self>, error: String| {
                    if current(view)
                        && let Some(preview) = &mut view.draft_preview
                    {
                        preview.dock_pending = false;
                        preview.error = Some(format!("Couldn’t update the draft preview. {error}"));
                        cx.notify();
                    }
                };
            executor.timer(Duration::from_millis(90)).await;
            if !ticket.is_current() {
                return;
            }
            let job = owner.update(cx, |view, cx| {
                if !current(view) {
                    return Ok(None);
                }
                let job = freeze(view)?;
                let pending = job.as_ref().is_some_and(|job| job.dock.is_some());
                if let Some(preview) = &mut view.draft_preview
                    && preview.dock_pending != pending
                {
                    preview.dock_pending = pending;
                    cx.notify();
                }
                Ok(job)
            });
            let DraftPreviewJob {
                dock,
                thumbnails,
                batch,
            } = match job {
                Ok(Ok(Some(job))) => job,
                Ok(Ok(None)) => {
                    let _ = owner.update(cx, |view, cx| {
                        if current(view) {
                            view.retire_draft_render(editor, true, cx);
                        }
                    });
                    return;
                }
                Ok(Err(error)) => {
                    let _ = owner.update(cx, |view, cx| report_error(view, cx, error));
                    return;
                }
                Err(_) => return,
            };
            let work = ticket.clone();
            let dock_library = library.clone();
            let result = executor
                .spawn(async move {
                    if !work.is_current() {
                        return Ok(None);
                    }
                    let mut batch = batch.materialize().map_err(model::failure)?;
                    if !work.is_current() {
                        return Ok(None);
                    }
                    let frame = dock
                        .map(|dock| dock.render(&mut batch, &work, &dock_library))
                        .transpose()
                        .map(Option::flatten);
                    Ok(Some((batch, frame)))
                })
                .await;
            let (mut batch, frame) = match result {
                Ok(Some(previews)) => previews,
                Ok(None) => return,
                Err(error) => {
                    let _ = owner.update(cx, |view, cx| report_error(view, cx, error));
                    return;
                }
            };
            match frame {
                Ok(frame) => {
                    let _ = owner.update(cx, |view, cx| {
                        if current(view)
                            && let Some(preview) = &mut view.draft_preview
                        {
                            preview.dock_pending = false;
                            preview.frame = frame;
                            cx.notify();
                        }
                    });
                }
                Err(error) => {
                    let _ = owner.update(cx, |view, cx| report_error(view, cx, error));
                }
            }
            for (id, pending) in thumbnails {
                if !ticket.is_current() {
                    return;
                }
                let work = ticket.clone();
                let library = library.clone();
                let (reused_batch, image) = executor
                    .spawn(async move {
                        let image = pending
                            .complete(&mut batch)
                            .map_err(model::failure)
                            .and_then(|frame| {
                                node_previews::make_thumbnail(&frame, false, &work, &library)
                            });
                        (batch, image.map(|images| images.into_iter().next()))
                    })
                    .await;
                batch = reused_batch;
                let Ok(()) = owner.update(cx, |view, cx| {
                    if current(view)
                        && let Some(preview) = &mut view.draft_preview
                    {
                        match image {
                            Ok(Some(image)) => {
                                preview.thumbnails.insert(id, image);
                            }
                            Ok(None) => {
                                preview.thumbnails.remove(&id);
                            }
                            Err(error) => {
                                preview.error.get_or_insert_with(|| {
                                    format!("Couldn’t update a node’s draft preview. {error}")
                                });
                            }
                        }
                        cx.notify();
                    }
                }) else {
                    return;
                };
            }
        })
        .detach();
    }
}
