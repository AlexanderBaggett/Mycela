//! Thumbnails for the visible graph nodes. The inspector preview has its own size and lifetime.
use super::*;
use crate::solid_color::map_preview::{self, PreviewSelection};
use gpui_kit::RenderImage;
use mycela_engine::{
    EnvironmentResourceLibrary, FrozenRenderBatch, ImageGeometry, PendingThumbnailRender,
    PointwiseThumbnailCache, PointwiseThumbnailRender, RenderFreezeSession, RenderedImage,
};
use std::{
    collections::{HashMap, HashSet},
    time::Instant,
};

const THUMBNAIL_SIZE: i32 = 168;
const PREVIEW_FPS: f64 = 7.;
const FIRST_FRAME_BATCH: usize = 4;
const OFFSCREEN_FRAME_BUDGET: usize =
    64 * 1024 * 1024 / (4 * THUMBNAIL_SIZE as usize * THUMBNAIL_SIZE as usize);
const OFFSCREEN_CARD_LIMIT: usize = 256;

#[derive(PartialEq)]
struct Scope {
    revision: u64,
    time: f64,
    library_root: Option<PathBuf>,
    animate: bool,
}

#[derive(Default)]
struct CachedPreview {
    generation: u64,
    last_visible: u64,
    node_revision: Option<u64>,
    frames: Vec<Arc<RenderImage>>,
    fallback_frame: usize,
    complete: bool,
    error: Option<String>,
}

#[derive(Default)]
pub(super) struct NodePreviews {
    scope: Option<Scope>,
    visible_nodes: Vec<Node>,
    visible_ids: HashSet<u64>,
    viewport: u64,
    generation: u64,
    local_gradient_edits: Option<(u64, u64)>,
    images: HashMap<u64, CachedPreview>,
    raster_cache: Option<PointwiseThumbnailCache>,
    ticket: Option<Ticket>,
    job: Option<Task<()>>,
    playback: Option<(Instant, Task<()>)>,
    paused: bool,
}

type Thumbnail = PointwiseThumbnailRender<ForegroundError>;

struct FirstFrame {
    id: u64,
    animated: bool,
    node_revision: u64,
    pending: PendingThumbnailRender<ForegroundError>,
}

struct CaptureBatch {
    nodes: Vec<FirstFrame>,
    frozen: FrozenRenderBatch,
}

fn capture_first_frames(nodes: &[(u64, bool, Node)], time: f64) -> Result<CaptureBatch, String> {
    let mut session = RenderFreezeSession::new();
    let nodes = nodes
        .iter()
        .map(|(id, animated, node)| {
            Ok(FirstFrame {
                id: *id,
                animated: *animated,
                node_revision: node.render_revision(),
                pending: session
                    .freeze_thumbnail(node, time)
                    .map_err(model::failure)?,
            })
        })
        .collect::<Result<_, String>>()?;
    Ok(CaptureBatch {
        nodes,
        frozen: session.finish().map_err(model::failure)?,
    })
}

enum Work {
    FirstFrames {
        batches: Vec<CaptureBatch>,
        time: f64,
    },
    Animation {
        id: u64,
        index: usize,
        pending: Option<Result<CaptureBatch, String>>,
    },
}

struct ThumbnailWorker {
    cache: PointwiseThumbnailCache,
    devices: crate::scene_backend::PreviewDevices,
    animation: Option<(u64, PointwiseThumbnailRender<ForegroundError>)>,
}

impl ThumbnailWorker {
    fn new(cache: Option<PointwiseThumbnailCache>) -> Self {
        Self {
            cache: cache.unwrap_or_else(|| {
                PointwiseThumbnailCache::new(
                    ImageGeometry::new(THUMBNAIL_SIZE, THUMBNAIL_SIZE),
                    crate::scene_backend::GRAPH_PNG_OPTIONS,
                )
            }),
            devices: Default::default(),
            animation: None,
        }
    }

    fn render(
        &mut self,
        renderer: &PointwiseThumbnailRender<ForegroundError>,
        time: f64,
        animated: bool,
        ticket: &Ticket,
        library: &EnvironmentResourceLibrary,
    ) -> Result<Vec<Arc<RenderImage>>, String> {
        let images = crate::scene_backend::render_thumbnail_sample(
            renderer,
            time,
            animated,
            &mut self.cache,
            &mut self.devices,
            &|| !ticket.is_current(),
            library,
        )
        .map_err(model::failure)?;
        project_thumbnails(images)
    }

    fn first_frame(
        &mut self,
        thumbnail: Result<Thumbnail, String>,
        time: f64,
        animated: bool,
        ticket: &Ticket,
        library: &EnvironmentResourceLibrary,
    ) -> Result<Vec<Arc<RenderImage>>, String> {
        self.render(&thumbnail?, time, animated, ticket, library)
    }

    fn animation_frame(
        &mut self,
        id: u64,
        index: usize,
        pending: Option<Result<CaptureBatch, String>>,
        ticket: &Ticket,
        library: &EnvironmentResourceLibrary,
    ) -> Result<Vec<Arc<RenderImage>>, String> {
        let renderer = if let Some(pending) = pending {
            let mut batch = pending?;
            let ready = batch.frozen.materialize().map_err(model::failure)?;
            batch
                .nodes
                .pop()
                .expect("one pending animation")
                .pending
                .complete(&ready)
                .map_err(model::failure)?
        } else {
            self.animation
                .take()
                .filter(|(owner, _)| *owner == id)
                .map(|(_, renderer)| renderer)
                .ok_or("Animation render state was retired.")?
        };
        let time = index as f64 / RenderedImage::PREVIEW_FRAME_COUNT as f64;
        let image = self.render(&renderer, time, true, ticket, library);
        self.animation = Some((id, renderer));
        image
    }
}

impl NodePreviews {
    pub(super) fn visible_nodes(&self) -> &[Node] {
        &self.visible_nodes
    }

    #[cfg(test)]
    pub(super) fn admitted_for_test(&self, node: &Node) -> bool {
        self.visible_ids.contains(&node.occurrence_id())
    }

    #[cfg(test)]
    pub(super) fn frames(&self, node: &Node) -> Option<&[Arc<RenderImage>]> {
        let cached = self.images.get(&node.occurrence_id())?;
        (!cached.frames.is_empty()).then_some(cached.frames.as_slice())
    }

    #[cfg(test)]
    pub(super) fn diagnostic(&self, node: &Node) -> String {
        format!(
            "visible {}, node {}, error {:?}",
            self.visible_ids.contains(&node.occurrence_id()),
            node.occurrence_id(),
            self.images
                .get(&node.occurrence_id())
                .and_then(|image| image.error.as_ref())
        )
    }

    fn cancel(&mut self) {
        if let Some(ticket) = self.ticket.take() {
            ticket.retire();
        }
        self.job.take();
    }

    pub(super) fn suspend(&mut self) {
        self.cancel();
        self.visible_nodes.clear();
        self.visible_ids.clear();
        self.playback.take();
        self.prune();
    }

    pub(super) fn clear(&mut self) {
        self.suspend();
        self.scope = None;
        self.local_gradient_edits = None;
        self.images.clear();
        self.raster_cache = None;
    }

    // Direct, unbound Gradient controls follow the graph's invalidation
    // notifications. All other revisions refresh the visible set.
    pub(super) fn report_local_gradient_edit(&mut self, before: u64) {
        let start = self
            .local_gradient_edits
            .filter(|(_, end)| *end == before)
            .map_or(before, |(start, _)| start);
        self.local_gradient_edits = Some((start, before + 1));
    }

    fn keep_unchanged_gradient_neighbors(&mut self, scope: &Scope, nodes: &[Node]) {
        let edits = self.local_gradient_edits.take();
        let valid = edits
            .zip(self.scope.as_ref())
            .is_some_and(|((start, end), old)| {
                end == scope.revision
                    && (start..end).contains(&old.revision)
                    && old.time == scope.time
                    && old.library_root == scope.library_root
                    && old.animate == scope.animate
            });
        if valid {
            for node in nodes {
                if let Some(image) = self.images.get_mut(&node.occurrence_id())
                    && image.generation == self.generation
                    && image.node_revision == Some(node.render_revision())
                {
                    image.generation += 1;
                }
            }
        }
    }

    fn prune(&mut self) {
        let mut offscreen = self
            .images
            .iter()
            .filter(|(id, _)| !self.visible_ids.contains(id))
            .map(|(id, image)| (image.last_visible, *id, image.frames.len()))
            .collect::<Vec<_>>();
        let mut cards = offscreen.len();
        let mut frames = offscreen.iter().map(|(_, _, count)| count).sum::<usize>();
        if cards <= OFFSCREEN_CARD_LIMIT && frames <= OFFSCREEN_FRAME_BUDGET {
            return;
        }
        offscreen.sort_unstable();
        for (_, id, count) in offscreen {
            self.images.remove(&id);
            cards -= 1;
            frames -= count;
            if cards <= OFFSCREEN_CARD_LIMIT && frames <= OFFSCREEN_FRAME_BUDGET {
                break;
            }
        }
    }

    fn playback_tick(&self) -> usize {
        self.playback.as_ref().map_or(0, |(start, _)| {
            (start.elapsed().as_secs_f64() * PREVIEW_FPS) as usize
        })
    }

    fn has_visible_animation(&self) -> bool {
        self.visible_ids.iter().any(|id| {
            self.images.get(id).is_some_and(|image| {
                image.generation == self.generation && image.complete && image.frames.len() > 1
            })
        })
    }

    fn next_work(&mut self, animation_owner: Option<u64>) -> Option<Work> {
        let scope = self.scope.as_ref()?;
        let mut first = self
            .visible_nodes
            .iter()
            .filter(|node| {
                !self
                    .images
                    .get(&node.occurrence_id())
                    .is_some_and(|image| image.generation == self.generation)
            })
            .take(FIRST_FRAME_BATCH)
            .map(|node| {
                let animated = scope.animate && node.has_animated_preview().unwrap_or(false);
                (node.occurrence_id(), animated, node.clone())
            })
            .collect::<Vec<_>>();
        if !first.is_empty() {
            let time = scope.time;
            let batches = match capture_first_frames(&first, time) {
                Ok(batch) => vec![batch],
                // A failed capture retires its session. Fall back only for
                // this set so other cards still get their own first frame.
                Err(_) => first
                    .drain(..)
                    .filter_map(|card| {
                        match capture_first_frames(std::slice::from_ref(&card), time) {
                            Ok(batch) => Some(batch),
                            Err(error) => {
                                self.publish(card.0, 0, false, None, Err(error));
                                None
                            }
                        }
                    })
                    .collect(),
            };
            return Some(Work::FirstFrames { batches, time });
        }
        self.visible_nodes.iter().find_map(|node| {
            let id = node.occurrence_id();
            let image = self.images.get(&id)?;
            (!image.complete).then(|| Work::Animation {
                id,
                index: image.frames.len(),
                pending: (animation_owner != Some(id))
                    .then(|| capture_first_frames(&[(id, true, node.clone())], scope.time)),
            })
        })
    }

    fn publish(
        &mut self,
        id: u64,
        index: usize,
        animated: bool,
        node_revision: Option<u64>,
        result: Result<Vec<Arc<RenderImage>>, String>,
    ) {
        if index != 0
            && !self.images.get(&id).is_some_and(|image| {
                image.generation == self.generation && image.frames.len() == index
            })
        {
            return;
        }
        let image = self.images.entry(id).or_default();
        if index == 0 {
            image.generation = self.generation;
            image.node_revision = node_revision.filter(|_| result.is_ok());
            image.frames.clear();
            image.fallback_frame = 0;
            image.error = None;
        }
        if self.visible_ids.contains(&id) {
            image.last_visible = self.viewport;
        }
        match result {
            Ok(frames) if frames.len() > 1 => image.frames = frames,
            Ok(frames) => image.frames.extend(frames),
            Err(error) => image.error = Some(error),
        }
        image.complete = !animated
            || image.error.is_some()
            || image.frames.len() >= RenderedImage::PREVIEW_FRAME_COUNT;
        self.prune();
    }
}

impl Drop for NodePreviews {
    fn drop(&mut self) {
        self.cancel();
    }
}

impl GraphDocumentView {
    pub(super) fn update_node_preview_scope(
        &mut self,
        mut nodes: Vec<Node>,
        cx: &mut Context<Self>,
    ) {
        let mut ids = HashSet::new();
        nodes.retain(|node| ids.insert(node.occurrence_id()));
        let scope = Scope {
            revision: self.revision,
            time: self.sample_time,
            library_root: self.environment_library.root().map(Path::to_path_buf),
            animate: !cx.reduce_motion(),
        };
        let previews = &mut self.node_previews;
        let changed = previews.scope.as_ref() != Some(&scope);
        if changed {
            let tick = previews.playback_tick();
            for image in previews.images.values_mut() {
                if image.generation == previews.generation && image.complete {
                    image.fallback_frame = tick.checked_rem(image.frames.len()).unwrap_or(0);
                }
            }
            previews.keep_unchanged_gradient_neighbors(&scope, &nodes);
            previews.cancel();
            previews.raster_cache = None;
            previews.generation += 1;
            previews.scope = Some(scope);
        }
        let viewport_changed = !previews
            .visible_nodes
            .iter()
            .map(Node::occurrence_id)
            .eq(nodes.iter().map(Node::occurrence_id));
        if !changed && !viewport_changed {
            return;
        }
        if viewport_changed {
            previews.viewport += 1;
            for id in &ids {
                if let Some(image) = previews.images.get_mut(id) {
                    image.last_visible = previews.viewport;
                }
            }
            previews.visible_nodes = nodes;
            previews.visible_ids = ids;
            previews.prune();
        }
        if !previews.has_visible_animation() {
            previews.playback.take();
        }
        self.start_node_preview_playback(cx);
        let previews = &self.node_previews;
        if previews.job.is_some() || previews.visible_nodes.is_empty() {
            return;
        }
        let generation = previews.generation;
        let owner = cx.weak_entity();
        cx.defer(move |cx| {
            let _ = owner.update(cx, |view, cx| view.render_node_previews(generation, cx));
        });
    }

    fn thumbnail_job_current(&self, ticket: &Ticket) -> bool {
        ticket.is_current()
            && !self.closed
            && self.node_previews.scope.as_ref().is_some_and(|scope| {
                scope.revision == self.revision
                    && scope.time == self.sample_time
                    && scope.library_root.as_deref() == self.environment_library.root()
            })
    }

    fn render_node_previews(&mut self, generation: u64, cx: &mut Context<Self>) {
        let previews = &mut self.node_previews;
        if self.closed
            || !self.canvas.visible
            || previews.generation != generation
            || previews.job.is_some()
            || previews.visible_nodes.is_empty()
        {
            return;
        }
        let ticket = Ticket::new();
        previews.ticket = Some(ticket.clone());
        let background = cx.background_executor().clone();
        let library = self.environment_library.clone();
        let raster_cache = previews.raster_cache.take();
        previews.job = Some(cx.spawn(async move |owner, cx| {
            let mut worker = ThumbnailWorker::new(raster_cache);
            loop {
                let Ok(work) = owner.update(cx, |view, _| {
                    if !view.thumbnail_job_current(&ticket) {
                        return None;
                    }
                    view.node_previews
                        .next_work(worker.animation.as_ref().map(|(id, _)| *id))
                }) else {
                    break;
                };
                let Some(work) = work else { break };
                match work {
                    Work::FirstFrames { batches, time } => {
                        for batch in batches {
                            // The detached batch shares upstream owners and
                            // materializes any backing files once.
                            let snapshots = background
                                .spawn(async move {
                                    let prepared =
                                        batch.frozen.materialize().map_err(model::failure);
                                    batch
                                        .nodes
                                        .into_iter()
                                        .map(|card| {
                                            let frame = match &prepared {
                                                Ok(batch) => card
                                                    .pending
                                                    .complete(batch)
                                                    .map_err(model::failure),
                                                Err(error) => Err(error.clone()),
                                            };
                                            (card.id, card.animated, card.node_revision, frame)
                                        })
                                        .collect::<Vec<_>>()
                                })
                                .await;
                            for (id, animated, node_revision, frame) in snapshots {
                                let Ok(current) = owner.update(cx, |view, _| {
                                    view.thumbnail_job_current(&ticket)
                                        && view.node_previews.visible_ids.contains(&id)
                                }) else {
                                    return;
                                };
                                if !current {
                                    continue;
                                }
                                let cancelled = ticket.clone();
                                let library = library.clone();
                                let result;
                                (worker, result) = background
                                    .spawn(async move {
                                        let result = worker.first_frame(
                                            frame,
                                            if animated { 0. } else { time },
                                            animated,
                                            &cancelled,
                                            &library,
                                        );
                                        (worker, result)
                                    })
                                    .await;
                                if owner
                                    .update(cx, |view, cx| {
                                        view.publish_thumbnail(
                                            &ticket,
                                            id,
                                            0,
                                            Some((animated, node_revision)),
                                            result,
                                            cx,
                                        )
                                    })
                                    .is_err()
                                {
                                    return;
                                }
                            }
                        }
                    }
                    Work::Animation { id, index, pending } => {
                        let cancelled = ticket.clone();
                        let library = library.clone();
                        let result;
                        (worker, result) = background
                            .spawn(async move {
                                let result = worker
                                    .animation_frame(id, index, pending, &cancelled, &library);
                                (worker, result)
                            })
                            .await;
                        if owner
                            .update(cx, |view, cx| {
                                view.publish_thumbnail(&ticket, id, index, None, result, cx)
                            })
                            .is_err()
                        {
                            return;
                        }
                    }
                }
            }
            let ThumbnailWorker {
                cache,
                devices,
                animation,
            } = worker;
            let _ = owner.update(cx, move |view, cx| {
                if view.thumbnail_job_current(&ticket) {
                    view.node_previews.raster_cache = Some(cache);
                    view.node_previews.ticket.take();
                    view.node_previews.job.take();
                    cx.notify();
                }
            });
            background
                .spawn(async move { drop((devices, animation)) })
                .detach();
        }));
    }

    fn publish_thumbnail(
        &mut self,
        ticket: &Ticket,
        id: u64,
        index: usize,
        first: Option<(bool, u64)>,
        image: Result<Vec<Arc<RenderImage>>, String>,
        cx: &mut Context<Self>,
    ) {
        if !self.thumbnail_job_current(ticket) {
            return;
        }
        let animated = first.is_none_or(|(animated, _)| animated);
        self.node_previews.publish(
            id,
            index,
            animated,
            first.map(|(_, revision)| revision),
            image,
        );
        if self.node_previews.visible_ids.contains(&id) {
            self.start_node_preview_playback(cx);
            let complete = self.node_previews.images[&id].complete;
            if index == 0 || complete {
                cx.notify();
            }
        }
    }

    fn start_node_preview_playback(&mut self, cx: &mut Context<Self>) {
        let previews = &mut self.node_previews;
        if previews.paused || previews.playback.is_some() || !previews.has_visible_animation() {
            return;
        }
        let timer = cx.background_executor().clone();
        let task = cx.spawn(async move |owner, cx| {
            loop {
                timer.timer(Duration::from_secs_f64(1. / PREVIEW_FPS)).await;
                if owner.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        });
        previews.playback = Some((Instant::now(), task));
    }

    pub(super) fn node_preview_animation_paused(&self) -> bool {
        self.node_previews.paused
    }

    pub(super) fn toggle_node_preview_animation(&mut self, cx: &mut Context<Self>) {
        self.node_previews.paused = !self.node_previews.paused;
        if self.node_previews.paused {
            self.node_previews.playback.take();
        } else {
            self.start_node_preview_playback(cx);
        }
        cx.notify();
    }

    pub(super) fn preview_frame(&self, node: &Node) -> Option<Arc<RenderImage>> {
        if let Some(frame) = self.draft_preview_thumbnail(node.occurrence_id()) {
            return Some(frame.clone());
        }
        let cached = self.node_previews.images.get(&node.occurrence_id())?;
        let tick = if cached.generation != self.node_previews.generation {
            cached.fallback_frame
        } else if cached.complete {
            self.node_previews.playback_tick()
        } else {
            0
        };
        cached
            .frames
            .get(tick.checked_rem(cached.frames.len())?)
            .cloned()
    }

    pub(super) fn render_group_input_preview(
        &self,
        node: &Node,
        index: usize,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        div()
            .id(("group-input-preview", index))
            .size_12()
            .flex_none()
            .rounded_sm()
            .overflow_hidden()
            .bg(cx.theme().background)
            .when_some(self.preview_frame(node), |surface, frame| {
                surface.child(preview_image(
                    format!("graph-group-input-preview-{}", index + 1),
                    rems(3.).into(),
                    frame,
                ))
            })
            .when(self.preview_failed(node), |surface| {
                surface
                    .items_center()
                    .justify_center()
                    .flex()
                    .aria_label("Input preview unavailable")
                    .text_color(cx.theme().muted_foreground)
                    .child("—")
            })
    }

    fn preview_failed(&self, node: &Node) -> bool {
        self.draft_preview_thumbnail(node.occurrence_id()).is_none()
            && self
                .node_previews
                .images
                .get(&node.occurrence_id())
                .is_some_and(|image| {
                    image.generation == self.node_previews.generation
                        && image.frames.is_empty()
                        && image.error.is_some()
                })
    }

    pub(super) fn render_node_preview(
        &self,
        node: &Node,
        title: &str,
        size: gpui_kit::Pixels,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let id = node.occurrence_id();
        div()
            .id(("node-preview", id))
            .relative()
            .size(size)
            .flex_none()
            .overflow_hidden()
            .rounded(theme.radius)
            .bg(theme.muted)
            .h_flex()
            .items_center()
            .justify_center()
            .aria_label(format!("{title} output preview"))
            .test_support()
            .when_some(self.preview_frame(node), |surface, frame| {
                surface.role(gpui_kit::Role::Image).child(preview_image(
                    ("node-preview-image", id),
                    size.into(),
                    frame,
                ))
            })
            .when(self.preview_failed(node), |surface| {
                surface
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child("Preview unavailable")
            })
    }
}

fn preview_image(
    id: impl Into<gpui_kit::ElementId>,
    size: gpui_kit::Length,
    frame: Arc<RenderImage>,
) -> impl IntoElement {
    div()
        .id(id)
        .test_support()
        .relative()
        .size(size)
        .child(crate::preview_background::checkerboard())
        .child(img(frame).absolute().size_full())
}

pub(super) fn make_thumbnail(
    frame: &mycela_engine::PointwiseNodeRenderSnapshot<ForegroundError>,
    animate: bool,
    ticket: &Ticket,
    library: &mycela_engine::EnvironmentResourceLibrary,
) -> Result<Vec<Arc<RenderImage>>, String> {
    use crate::scene_backend;
    let geometry = ImageGeometry::new(THUMBNAIL_SIZE, THUMBNAIL_SIZE);
    let cancelled = || !ticket.is_current();
    let images = if animate {
        scene_backend::render_graph_playback(frame, geometry, &cancelled, Some(library))
    } else {
        scene_backend::render_export(
            frame,
            geometry,
            &cancelled,
            scene_backend::GRAPH_PNG_OPTIONS,
            None,
            None,
            Some(library),
        )
        .map(RenderedImage::Still)
    }
    .map_err(model::failure)?;
    project_thumbnails(images)
}

fn project_thumbnails(images: RenderedImage) -> Result<Vec<Arc<RenderImage>>, String> {
    let project = |image| {
        map_preview::project(image, &PreviewSelection::BaseColor)
            .map(|(_, raster)| raster)
            .map_err(model::failure)
    };
    match images {
        RenderedImage::Still(image) => Ok(vec![project(&image)?]),
        RenderedImage::Animated(images) => images.iter().map(project).collect(),
    }
}
