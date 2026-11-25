//! Thumbnails for the visible graph nodes. The inspector preview has its own size and lifetime.
use super::*;
use crate::solid_color::map_preview::{self, PreviewSelection};
use gpui_kit::RenderImage;
use mycela_engine::{ImageGeometry, RenderedImage};
use std::{
    collections::{HashMap, HashSet},
    time::Instant,
};

const THUMBNAIL_SIZE: i32 = 168;
const PREVIEW_FPS: f64 = 7.;

#[derive(Debug, PartialEq)]
struct Scope {
    revision: u64,
    time: f64,
    library_root: Option<PathBuf>,
    animate: bool,
    nodes: Vec<u64>,
}

#[derive(Default)]
pub(super) struct NodePreviews {
    scope: Option<Scope>,
    visible_nodes: Vec<Node>,
    generation: u64,
    images: HashMap<u64, Result<Vec<Arc<RenderImage>>, String>>,
    ticket: Option<Ticket>,
    job: Option<Task<()>>,
    playback: Option<(Instant, Task<()>)>,
    paused: bool,
}

impl NodePreviews {
    pub(super) fn visible_nodes(&self) -> &[Node] {
        &self.visible_nodes
    }

    #[cfg(test)]
    pub(super) fn admitted_for_test(&self, node: &Node) -> bool {
        self.scope
            .as_ref()
            .is_some_and(|scope| scope.nodes.contains(&node.occurrence_id()))
    }

    #[cfg(test)]
    pub(super) fn frames(&self, node: &Node) -> Option<&[Arc<RenderImage>]> {
        self.images
            .get(&node.occurrence_id())
            .and_then(|result| result.as_ref().ok())
            .map(Vec::as_slice)
    }

    #[cfg(test)]
    pub(super) fn diagnostic(&self, node: &Node) -> String {
        format!(
            "scope {:?}, node {}, error {:?}",
            self.scope,
            node.occurrence_id(),
            self.images
                .get(&node.occurrence_id())
                .and_then(|image| image.as_ref().err())
        )
    }

    fn cancel(&mut self) {
        if let Some(ticket) = self.ticket.take() {
            ticket.retire();
        }
        self.job.take();
    }

    pub(super) fn clear(&mut self) {
        if self.scope.take().is_some() {
            self.generation += 1;
            self.cancel();
            self.images.clear();
            self.visible_nodes.clear();
            self.playback.take();
        }
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
            nodes: nodes.iter().map(Node::occurrence_id).collect(),
        };
        let previews = &mut self.node_previews;
        if previews.scope.as_ref() == Some(&scope) {
            return;
        }
        previews.cancel();
        if previews.scope.as_ref().is_some_and(|old| {
            old.revision == scope.revision
                && old.time == scope.time
                && old.library_root == scope.library_root
                && old.animate == scope.animate
        }) {
            previews.images.retain(|id, _| ids.contains(id));
        } else {
            previews.images.clear();
        }
        if !previews
            .images
            .values()
            .any(|result| result.as_ref().is_ok_and(|frames| frames.len() > 1))
        {
            previews.playback.take();
        }
        previews.visible_nodes.clone_from(&nodes);
        let missing = nodes
            .into_iter()
            .filter(|node| !previews.images.contains_key(&node.occurrence_id()))
            .collect::<Vec<_>>();
        previews.scope = Some(scope);
        previews.generation += 1;
        if !missing.is_empty() {
            let generation = previews.generation;
            let owner = cx.weak_entity();
            cx.defer(move |cx| {
                let _ = owner.update(cx, |view, cx| {
                    view.render_node_previews(generation, missing, cx);
                });
            });
        }
    }

    fn render_node_previews(&mut self, generation: u64, nodes: Vec<Node>, cx: &mut Context<Self>) {
        let previews = &mut self.node_previews;
        if self.closed || !self.canvas.visible || previews.generation != generation {
            return;
        }
        let Some(scope) = previews.scope.as_ref() else {
            return;
        };
        let (time, animate) = (scope.time, scope.animate);
        let ticket = Ticket::new();
        previews.ticket = Some(ticket.clone());
        let background = cx.background_executor().clone();
        let library = self.environment_library.clone();
        previews.job = Some(cx.spawn(async move |owner, cx| {
            for node in nodes {
                if !ticket.is_current() {
                    return;
                }
                let id = node.occurrence_id();
                let Ok(pending) = owner.update(cx, |_, _| model::freeze(&node, time)) else {
                    return;
                };
                let animate = animate && node.has_animated_preview().unwrap_or(false);
                let job_ticket = ticket.clone();
                let library = library.clone();
                let image = background
                    .spawn(async move {
                        let frame = pending?.materialize()?;
                        make_thumbnail(&frame, animate, &job_ticket, &library)
                    })
                    .await;
                let Ok(()) = owner.update(cx, |view, cx| {
                    if ticket.is_current()
                        && !view.closed
                        && view.node_previews.generation == generation
                    {
                        view.node_previews.images.insert(id, image);
                        view.start_node_preview_playback(cx);
                        cx.notify();
                    }
                }) else {
                    return;
                };
            }
        }));
    }

    fn start_node_preview_playback(&mut self, cx: &mut Context<Self>) {
        let previews = &mut self.node_previews;
        if previews.paused
            || previews.playback.is_some()
            || !previews
                .images
                .values()
                .any(|result| result.as_ref().is_ok_and(|frames| frames.len() > 1))
        {
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
        let frames = self
            .node_previews
            .images
            .get(&node.occurrence_id())?
            .as_ref()
            .ok()?;
        let tick = self
            .node_previews
            .playback
            .as_ref()
            .map_or(0, |(start, _)| {
                (start.elapsed().as_secs_f64() * PREVIEW_FPS) as usize
            });
        frames.get(tick.checked_rem(frames.len())?).cloned()
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
                .is_some_and(Result::is_err)
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
