//! Temporary Levels display frames and histogram range extension during a drag.
use super::*;
use crate::{
    LIVE_PREVIEW_SIZE,
    scene_backend::{self, GRAPH_PNG_OPTIONS},
    solid_color::map_preview::{self, PreviewSelection},
};
use mycela_engine::SharedImageMap;
use std::time::Duration;

const EDGE_INTERVAL: Duration = Duration::from_millis(10);
const EDGE_ZONE: f32 = 1.0 / 16.0;
const RANGE_LIMIT: f32 = 10.0;

pub(super) struct DraftPreview {
    ticket: Ticket,
    updates: async_channel::Sender<LevelsDraft>,
    _task: Task<()>,
    context: PreviewContext,
    image: Option<model::Preview>,
}

impl Drop for DraftPreview {
    fn drop(&mut self) {
        self.ticket.retire();
    }
}

#[derive(Clone, Copy)]
struct PreviewContext {
    node: u64,
    revision: u64,
    time: f64,
    dimensions: [i32; 2],
    modal: bool,
}

impl PreviewContext {
    fn current(self, view: &GraphDocumentView) -> bool {
        view.revision == self.revision
            && view.sample_time == self.time
            && !view.closed
            && view.modal == self.modal
            && view.work.is_none()
            && view.drawing.is_none()
            && !view.preview_animation_active()
            && view
                .preview_target()
                .is_some_and(|node| node.occurrence_id() == self.node)
    }
}

impl DraftPreview {
    pub(super) fn start(
        editor: &LevelsEditor,
        cx: &mut Context<LevelsEditor>,
    ) -> Result<Option<Self>, String> {
        let Some(host) = editor.host.as_ref().and_then(WeakEntity::upgrade) else {
            return Ok(None);
        };
        let view = host.read(cx);
        let node = editor.parameter.owner().node();
        let context = PreviewContext {
            node: node.occurrence_id(),
            revision: view.revision,
            time: view.sample_time,
            modal: editor.batch.is_some(),
            dimensions: view
                .preview
                .as_ref()
                .map_or([LIVE_PREVIEW_SIZE; 2], |preview| {
                    [preview.bitmap.width(), preview.bitmap.height()]
                }),
        };
        if !context.current(view) {
            return Ok(None);
        }

        let input = node
            .slot(0)
            .map_err(failure)?
            .node()
            .filter(|node| !node.is_default_input())
            .map(|node| view.freeze_preview(&node, context.time))
            .transpose()?;
        let library = view.environment_library.clone();
        let (updates, receiver) = async_channel::bounded(1);
        let ticket = Ticket::new();
        let worker = ticket.clone();
        let publication = ticket.clone();
        let [width, height] = context.dimensions;
        let geometry = ImageGeometry::new(width, height);
        let task = cx.spawn(async move |owner, cx| {
            let cancellation = worker.clone();
            let image = cx
                .background_spawn(async move {
                    match input {
                        Some(input) => scene_backend::render_export(
                            &input.materialize()?,
                            geometry,
                            &|| !cancellation.is_current(),
                            GRAPH_PNG_OPTIONS,
                            None,
                            None,
                            Some(&library),
                        )
                        .map_err(failure),
                        None => Ok(SharedImageMap::new(geometry)),
                    }
                })
                .await;
            let image = match image {
                Ok(image) => image,
                Err(error) => {
                    if publication.is_current() {
                        let _ = owner.update(cx, |editor, cx| editor.set_draft_error(error, cx));
                    }
                    return;
                }
            };
            while let Ok(draft) = receiver.recv().await {
                if !publication.is_current() {
                    break;
                }
                let source = image.clone();
                let result = cx
                    .background_spawn(async move { render_draft(&source, geometry, draft) })
                    .await;
                if publication.is_current() {
                    let _ = owner.update(cx, |editor, cx| {
                        editor.publish_draft(&publication, result, cx);
                    });
                }
            }
        });
        Ok(Some(Self {
            ticket,
            updates,
            _task: task,
            context,
            image: None,
        }))
    }

    pub(super) fn queue(&self, draft: LevelsDraft) {
        let _ = self.updates.force_send(draft);
    }

    pub(super) fn image(&self) -> Option<&model::Preview> {
        self.image.as_ref()
    }
}

fn render_draft(
    source: &SharedImageMap,
    size: ImageGeometry,
    draft: LevelsDraft,
) -> Result<model::Preview, String> {
    let filter = draft.preview_filter::<ForegroundError>().map_err(failure)?;
    let result = filter
        .render_supplied_with_options(
            &[Some(source.clone())],
            size.width(),
            size.height(),
            GRAPH_PNG_OPTIONS,
        )
        .map_err(failure)?
        .ok_or_else(|| "The Levels draft returned no image.".to_owned())?;
    let projection = map_preview::project(&result, &PreviewSelection::BaseColor).map_err(failure);
    result.dispose();
    let (bitmap, raster) = projection?;
    Ok(model::Preview::base_color(bitmap, raster))
}

impl LevelsEditor {
    pub(super) fn update_drag(&mut self, pointer: Pixels, cx: &mut Context<Self>) {
        let Some(drag) = &mut self.drag else {
            return;
        };
        drag.pointer = pointer;
        if drag.plot.extended(drag.pointer).is_some() {
            if drag.range_task.is_none() {
                drag.range_task = Some(cx.spawn(async move |owner, cx| {
                    loop {
                        cx.background_executor().timer(EDGE_INTERVAL).await;
                        let keep_extending = owner.update(cx, |editor, cx| editor.extend_drag(cx));
                        if !matches!(keep_extending, Ok(true)) {
                            break;
                        }
                    }
                }));
            }
        } else {
            drag.range_task = None;
        }
        self.update_draft(cx);
    }

    fn extend_drag(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(drag) = &mut self.drag else {
            return false;
        };
        if let Some(next) = drag.plot.extended(drag.pointer) {
            drag.plot = next;
            self.update_draft(cx);
            true
        } else {
            drag.range_task = None;
            false
        }
    }

    fn update_draft(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = &mut self.drag else {
            return;
        };
        let edit = drag.control.drag_edit(drag.plot.position(drag.pointer));
        let request = drag
            .start
            .edited(edit)
            .ok()
            .filter(|points| *points != drag.start)
            .map(|points| (edit, points));
        if drag.request != request {
            drag.request = request;
            if let Some((edit, _)) = request {
                self.render_drag_request(edit, cx);
            } else if let Some(batch) = &self.batch {
                self.queue_batch_preview(batch.draft, cx);
            } else if let Some(preview) = &mut drag.preview {
                preview.image = None;
                cx.emit(LevelsEditorEvent::DraftChanged);
            }
        }
        cx.notify();
    }

    fn render_drag_request(&mut self, edit: LevelsControlEdit, cx: &mut Context<Self>) {
        let draft = match self.draft_for_control(edit) {
            Ok(draft) => draft,
            Err(error) => return self.set_draft_error(error, cx),
        };
        if self.batch.is_some() {
            self.queue_batch_preview(draft, cx);
            return;
        }
        if self
            .drag
            .as_ref()
            .is_some_and(|drag| drag.preview.is_none())
        {
            match DraftPreview::start(self, cx) {
                Ok(next) => self.drag.as_mut().unwrap().preview = next,
                Err(error) => self.set_draft_error(error, cx),
            }
        }
        if let Some(preview) = self.drag.as_ref().and_then(|drag| drag.preview.as_ref()) {
            preview.queue(draft);
        }
    }

    fn draft_for_control(&self, edit: LevelsControlEdit) -> Result<LevelsDraft, String> {
        let mut draft = self.batch.as_ref().map_or_else(
            || self.parameter.levels_draft().map_err(failure),
            |batch| Ok(batch.draft),
        )?;
        draft.edit(self.channel, edit).map_err(failure)?;
        Ok(draft)
    }

    fn publish_draft(
        &mut self,
        ticket: &Ticket,
        result: Result<model::Preview, String>,
        cx: &mut Context<Self>,
    ) {
        if let Some(batch) = &mut self.batch {
            if let Some(preview) = &mut batch.preview
                && Arc::ptr_eq(&preview.ticket.active, &ticket.active)
            {
                match result {
                    Ok(image) => preview.image = Some(image),
                    Err(error) => self.set_draft_error(error, cx),
                }
                cx.notify();
            }
            return;
        }
        let Some(drag) = &mut self.drag else {
            return;
        };
        if drag.request.is_none() {
            return;
        }
        let Some(preview) = &mut drag.preview else {
            return;
        };
        if !Arc::ptr_eq(&preview.ticket.active, &ticket.active) {
            return;
        }
        match result {
            Ok(image) => {
                preview.image = Some(image);
                cx.emit(LevelsEditorEvent::DraftChanged);
            }
            Err(error) => self.set_draft_error(error, cx),
        }
    }

    pub(super) fn set_draft_error(&mut self, error: String, cx: &mut Context<Self>) {
        self.error = Some(format!("Couldn’t preview the Levels draft. {error}"));
        cx.notify();
    }
}

impl Plot {
    fn extended(self, pointer: Pixels) -> Option<Self> {
        let ratio = ((pointer - self.bounds.left()) / self.bounds.size.width).clamp(0., 1.);
        let [mut low, mut high] = self.extent;
        if ratio < EDGE_ZONE && low > -RANGE_LIMIT {
            let strength = 1.0 - ratio / EDGE_ZONE;
            low = (low - edge_step(strength)).max(-RANGE_LIMIT);
        } else if ratio > 1.0 - EDGE_ZONE && high < RANGE_LIMIT {
            let strength = 1.0 - (1.0 - ratio) / EDGE_ZONE;
            high = (high + edge_step(strength)).min(RANGE_LIMIT);
        } else {
            return None;
        }
        Some(Self {
            extent: [low, high],
            ..self
        })
    }
}

fn edge_step(strength: f32) -> f32 {
    0.002 + strength * 0.028
}

impl GraphDocumentView {
    pub(in crate::graph_document::view) fn levels_draft_preview<'a>(
        &self,
        cx: &'a App,
    ) -> Option<&'a model::Preview> {
        if self.preview_uses_texture_output() {
            return None;
        }
        let editor = self.inspector.levels.as_ref()?.read(cx);
        let preview = editor.drag.as_ref()?.preview.as_ref()?;
        preview
            .context
            .current(self)
            .then_some(preview.image.as_ref())
            .flatten()
    }
}
