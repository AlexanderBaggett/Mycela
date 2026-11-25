//! The unadjusted image under a Texture Synthesis source guide.
use super::*;
use crate::solid_color::map_preview::{self, PreviewSelection};
use gpui_kit::RenderImage;
use mycela_engine::{
    EnvironmentResourceLibrary, FrozenRenderValues, ImageGeometry, PointwiseRenderEffects,
    SynthesisRecord,
};

const FULL_SIZE_ASSET_WIDTH: i32 = 4000;

#[derive(Clone, PartialEq, Eq)]
enum RequestKey {
    Asset(Option<PathBuf>),
    Graph {
        node: u64,
        values: FrozenRenderValues,
        time: u64,
        library: Option<PathBuf>,
    },
}

pub(super) struct Preview {
    key: Option<RequestKey>,
    pub(super) state: State,
}

impl Preview {
    pub(super) fn geometry(&self) -> Option<ImageGeometry> {
        match &self.state {
            State::Ready(raster) => Some(raster.geometry),
            _ => None,
        }
    }
}

pub(super) enum State {
    Loading(WorkTicket),
    Ready(Raster),
    Failed(String),
}

impl State {
    pub(super) fn loading(ticket: Ticket) -> Self {
        Self::Loading(WorkTicket(ticket))
    }
}

#[derive(Clone)]
pub(super) struct Raster {
    pub(super) image: Arc<RenderImage>,
    pub(super) aspect: f32,
    pub(super) geometry: ImageGeometry,
    fitted_image: Option<Arc<RenderImage>>,
}

impl Raster {
    /// Try the fitted image if the device cannot upload the full source.
    pub(super) fn images(&self) -> impl Iterator<Item = Arc<RenderImage>> + '_ {
        std::iter::once(&self.image)
            .chain(&self.fitted_image)
            .cloned()
    }
}

pub(super) struct WorkTicket(Ticket);

impl Drop for WorkTicket {
    fn drop(&mut self) {
        self.0.retire();
    }
}

pub(super) enum Job {
    Asset(SynthesisEdit, usize),
    FullSizeAsset(SynthesisEdit, usize, Option<Raster>),
    Graph(model::PendingPreview),
}

pub(super) struct ManualPreviewJob {
    texture: SynthesisRecord,
    input: Option<model::PendingPreview>,
    layer: usize,
    seed: Option<i32>,
}

impl ManualPreviewJob {
    pub(super) fn render(
        self,
        library: &EnvironmentResourceLibrary,
        ticket: &Ticket,
    ) -> Result<(Raster, Option<ImageGeometry>), String> {
        let cancelled = || !ticket.is_current();
        if cancelled() {
            return Err("Manual placement preview cancelled.".into());
        }
        let input = self
            .input
            .map(|pending| Job::graph_image(pending, library, ticket))
            .transpose()?;
        let mut capacity = crate::memory_capacity::query;
        let mut preflight = crate::memory_capacity::preflight;
        let mut effects =
            PointwiseRenderEffects::<ForegroundError>::new(&mut capacity, &mut preflight)
                .with_cancel_check(&cancelled)
                .with_environment_library(Some(library));
        let rendered = self.texture.render_manual_placement_preview_with_geometry(
            self.layer,
            input.as_ref(),
            LIVE_PREVIEW_SIZE,
            self.seed,
            crate::export_render::generator_execution(),
            &mut effects,
        );
        if cancelled() {
            return Err("Manual placement preview cancelled.".into());
        }
        let (image, geometry) = rendered.map_err(model::failure)?;
        let raster = Job::raster(&image);
        image.dispose();
        raster.map(|raster| (raster, geometry))
    }
}

impl Job {
    fn graph_image(
        pending: model::PendingPreview,
        library: &EnvironmentResourceLibrary,
        ticket: &Ticket,
    ) -> Result<mycela_engine::SharedImageMap, String> {
        let cancelled = || !ticket.is_current();
        if cancelled() {
            return Err("Source preview cancelled.".into());
        }
        let side = LIVE_PREVIEW_SIZE;
        let image = crate::scene_backend::render_export(
            &pending.materialize()?,
            ImageGeometry::new(side, side),
            &cancelled,
            crate::scene_backend::GRAPH_PNG_OPTIONS,
            None,
            None,
            Some(library),
        )
        .map_err(model::failure)?;
        if cancelled() {
            return Err("Source preview cancelled.".into());
        }
        Ok(image)
    }

    fn fitted_asset(
        edit: &SynthesisEdit,
        index: usize,
        library: &EnvironmentResourceLibrary,
        ticket: &Ticket,
    ) -> Result<Raster, String> {
        edit.layer_asset_preview::<ForegroundError>(
            index,
            LIVE_PREVIEW_SIZE,
            Some(library),
            crate::export_render::generator_execution(),
            &|| !ticket.is_current(),
        )
        .map_err(model::failure)
        .and_then(|image| Self::raster(&image))
    }

    pub(super) fn render(
        self,
        library: &EnvironmentResourceLibrary,
        ticket: &Ticket,
    ) -> Result<Raster, String> {
        match self {
            Self::Asset(edit, index) => Self::fitted_asset(&edit, index, library, ticket),
            Self::Graph(pending) => Self::raster(&Self::graph_image(pending, library, ticket)?),
            Self::FullSizeAsset(edit, index, cached) => {
                let fallback = || match cached {
                    Some(raster) => Ok(raster),
                    None => Self::fitted_asset(&edit, index, library, ticket),
                };
                let full = edit
                    .layer_asset_editor_preview::<ForegroundError>(
                        index,
                        FULL_SIZE_ASSET_WIDTH,
                        Some(library),
                        crate::export_render::generator_execution(),
                        &|| !ticket.is_current(),
                    )
                    .map_err(model::failure)
                    .and_then(|image| Self::raster(&image));
                if !ticket.is_current() {
                    return Err("Source preview cancelled.".into());
                }
                let mut raster = match full {
                    Ok(raster) => raster,
                    Err(error) => return fallback().or(Err(error)),
                };
                // The photo's height, or a library tier, can exceed the
                // portable 4K atlas size. Fit only for a failed GPU upload.
                if raster.geometry.width().max(raster.geometry.height()) > FULL_SIZE_ASSET_WIDTH {
                    raster.fitted_image = fallback().ok().map(|raster| raster.image);
                }
                Ok(raster)
            }
        }
    }

    fn raster(image: &mycela_engine::SharedImageMap) -> Result<Raster, String> {
        let geometry = image.geometry();
        let (_, raster) =
            map_preview::project(image, &PreviewSelection::BaseColor).map_err(model::failure)?;
        Ok(Raster {
            image: raster,
            aspect: geometry.width() as f32 / geometry.height() as f32,
            geometry,
            fitted_image: None,
        })
    }
}

pub(super) enum GuidePreparation {
    Available(Option<State>),
    Pending(Box<Job>),
}

struct PatchInputRequest {
    node: u64,
    key: Option<RequestKey>,
    cached: Option<ImageGeometry>,
    pending: Option<model::PendingPreview>,
}

pub(in crate::graph_document::view) struct PatchInputRequests(Vec<PatchInputRequest>);

#[derive(Clone)]
struct PatchInputSize {
    node: u64,
    key: Option<RequestKey>,
    geometry: Option<ImageGeometry>,
}

#[derive(Clone)]
pub(in crate::graph_document::view) struct PatchInputSizes(Vec<PatchInputSize>);

impl PatchInputRequests {
    pub(in crate::graph_document::view) fn is_ready(&self) -> bool {
        self.0.iter().all(|request| request.pending.is_none())
    }

    pub(in crate::graph_document::view) fn render(
        self,
        library: &EnvironmentResourceLibrary,
        ticket: &Ticket,
    ) -> PatchInputSizes {
        PatchInputSizes(
            self.0
                .into_iter()
                .map(|input| {
                    let geometry = input.cached.or_else(|| {
                        Job::graph_image(input.pending?, library, ticket)
                            .ok()
                            .map(|raster| raster.geometry())
                    });
                    PatchInputSize {
                        node: input.node,
                        key: input.key,
                        geometry,
                    }
                })
                .collect(),
        )
    }

    pub(in crate::graph_document::view) fn matches(&self, prior: &PatchInputSizes) -> bool {
        self.0.len() == prior.0.len()
            && self
                .0
                .iter()
                .zip(&prior.0)
                .all(|(current, prior)| current.node == prior.node && current.key == prior.key)
    }
}

impl PatchInputSizes {
    pub(in crate::graph_document::view) fn dimensions(&self) -> Vec<(u64, ImageGeometry)> {
        self.0
            .iter()
            .filter_map(|input| input.geometry.map(|size| (input.node, size)))
            .collect()
    }
}

impl Panel {
    pub(super) fn source_guide_layer(&self) -> Option<usize> {
        match self.geometry {
            Some(_) => self.layer,
            None => self
                .layer
                .filter(|&layer| self.edit.patch_counts(layer).is_some()),
        }
    }
}

impl GraphDocumentView {
    pub(in crate::graph_document::view) fn capture_synthesis_patch_inputs(
        &self,
        mut inputs: Vec<Node>,
    ) -> PatchInputRequests {
        inputs.sort_by_key(Node::occurrence_id);
        let guide = self
            .synthesis
            .as_ref()
            .and_then(|panel| panel.guide_source.as_ref());
        PatchInputRequests(
            inputs
                .into_iter()
                .map(|input| {
                    let node = input.occurrence_id();
                    match self.freeze_synthesis_guide_node(&input) {
                        Ok((key, pending)) => {
                            let cached = guide.and_then(|guide| match &guide.state {
                                State::Ready(raster) if guide.key.as_ref() == Some(&key) => {
                                    Some(raster.geometry)
                                }
                                _ => None,
                            });
                            PatchInputRequest {
                                node,
                                key: Some(key),
                                cached,
                                pending: cached.is_none().then_some(pending),
                            }
                        }
                        Err(_) => PatchInputRequest {
                            node,
                            key: None,
                            cached: None,
                            pending: None,
                        },
                    }
                })
                .collect(),
        )
    }

    fn freeze_synthesis_guide_node(
        &self,
        input: &Node,
    ) -> Result<(RequestKey, model::PendingPreview), String> {
        self.freeze_preview(input, self.sample_time).map(|pending| {
            (
                RequestKey::Graph {
                    node: input.occurrence_id(),
                    values: pending.values(),
                    time: self.sample_time.to_bits(),
                    library: self.environment_library.root().map(Path::to_path_buf),
                },
                pending,
            )
        })
    }

    pub(super) fn synthesis_guide_input(&self, panel: &Panel) -> Option<Node> {
        if panel.lab_material.is_some() {
            return None;
        }
        if let Some(style) = &self.style {
            match style.canvas_layer() {
                Some(layer) => layer.style_input_node(),
                None => panel.node.slot(2).ok()?.node(),
            }
        } else {
            panel.node.slot(0).ok()?.node()
        }
    }

    fn freeze_synthesis_guide_input(
        &self,
        panel: &Panel,
    ) -> Option<Result<(RequestKey, model::PendingPreview), String>> {
        let input = self.synthesis_guide_input(panel)?;
        Some(self.freeze_synthesis_guide_node(&input))
    }

    /// Capture the current owner and a Manual tile request, including a
    /// working geometry draft when the full-size editor is open.
    pub(super) fn prepare_synthesis_manual_preview(
        &self,
        target: &Target,
        edit: &SynthesisEdit,
    ) -> Result<Option<ManualPreviewJob>, String> {
        if self.closed || !target.current(self) {
            return Ok(None);
        }
        let Some((panel, layer)) = self.synthesis.as_ref().zip(target.layer) else {
            return Ok(None);
        };
        if !matches!(edit.layer(layer), Some((_, SynthesisLayerKind::Patches)))
            || panel.edit.layer(layer) != edit.layer(layer)
        {
            return Ok(None);
        }
        let input = if edit.layer_uses_input(layer) {
            self.synthesis_guide_input(panel)
                .map(|node| self.freeze_preview(&node, self.sample_time))
                .transpose()?
        } else {
            None
        };
        let seed = if panel.lab_material.is_none()
            && panel.node.kind() == Some(mycela_engine::PointwiseKind::Synthesis)
        {
            Some(
                panel
                    .node
                    .filter()
                    .ok_or("The Texture Synthesis node changed.")?
                    .synthesis_manual_preview_seed_at(self.sample_time)
                    .map_err(model::failure)?,
            )
        } else {
            None
        };
        Ok(Some(ManualPreviewJob {
            texture: edit.record(),
            input,
            layer,
            seed,
        }))
    }

    pub(super) fn synthesis_patch_input_geometry(&self, panel: &Panel) -> Option<ImageGeometry> {
        if !matches!(panel.geometry, Some(SynthesisGeometry::Patch { .. }))
            || !panel.edit.layer_uses_input(panel.layer?)
        {
            return None;
        }
        let Preview {
            key,
            state: State::Ready(raster),
        } = panel.guide_source.as_ref()?
        else {
            return None;
        };
        let (current, _) = self.freeze_synthesis_guide_input(panel)?.ok()?;
        (key.as_ref() == Some(&current)).then_some(raster.geometry)
    }

    fn synthesis_source_request(
        &self,
        panel: &Panel,
        index: usize,
    ) -> Option<Result<(RequestKey, Job), String>> {
        if self.closed || !panel.edit.layer_has_source(index) {
            return None;
        }
        if !panel.edit.layer_uses_input(index) {
            let library = self.environment_library.root().map(Path::to_path_buf);
            return Some(Ok((
                RequestKey::Asset(library),
                Job::Asset(panel.edit.clone(), index),
            )));
        }
        self.freeze_synthesis_guide_input(panel)
            .map(|request| request.map(|(key, pending)| (key, Job::Graph(pending))))
    }

    pub(super) fn prepare_full_size_synthesis_source(
        &self,
        panel: &Panel,
        index: usize,
    ) -> GuidePreparation {
        match self.synthesis_source_request(panel, index) {
            None => GuidePreparation::Available(None),
            Some(Err(error)) => GuidePreparation::Available(Some(State::Failed(error))),
            Some(Ok((key, job))) => {
                let cached = if let Some(Preview {
                    key: Some(current),
                    state: State::Ready(raster),
                }) = &panel.guide_source
                    && current == &key
                {
                    Some(raster.clone())
                } else {
                    None
                };
                match (job, cached) {
                    (Job::Asset(edit, index), cached) => {
                        GuidePreparation::Pending(Box::new(Job::FullSizeAsset(edit, index, cached)))
                    }
                    (_, Some(raster)) => GuidePreparation::Available(Some(State::Ready(raster))),
                    (job, None) => GuidePreparation::Pending(Box::new(job)),
                }
            }
        }
    }

    pub(super) fn install_synthesis_panel(
        &mut self,
        mut next: Option<Panel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some((old, next)) = self.synthesis.as_mut().zip(next.as_mut())
            && old.node.same_reference(&next.node)
            && old.lab_material.is_some() == next.lab_material.is_some()
        {
            if let Some((old_layer, next_layer)) = old.layer.zip(next.layer) {
                let same_layer = old.edit.layer(old_layer) == next.edit.layer(next_layer);
                next.layer_color_open = old.layer_color_open && same_layer;
                if same_layer
                    && next.edit.patch_counts(next_layer).is_some()
                    && !matches!(next.geometry, Some(SynthesisGeometry::Patch { .. }))
                    && let Some(manual) = old.manual.take()
                {
                    next.geometry = manual.sync(&next.edit, next_layer, self.revision, window, cx);
                    next.manual = Some(manual);
                }
                if same_layer
                    && let (Some((old_count, _)), Some((next_count, _))) = (
                        old.edit.patch_counts(old_layer),
                        next.edit.patch_counts(next_layer),
                    )
                {
                    next.patch_list = old.patch_list.clone();
                    next.patch_transform_mode = old.patch_transform_mode;
                    next.patch_numbers = old.patch_numbers.take();
                    next.sync_patch_numbers(window, cx);
                    next.source_focus = old.source_focus.clone();
                    let selection = PatchList::selected(next.geometry);
                    if old_count != next_count || PatchList::selected(old.geometry) != selection {
                        next.patch_list.reveal(selection);
                    }
                }
                if next.geometry == Some(SynthesisGeometry::Grid)
                    && matches!(
                        old.geometry,
                        Some(SynthesisGeometry::Grid | SynthesisGeometry::GridPoint(_))
                    )
                    && old.edit.layer(old_layer) == next.edit.layer(next_layer)
                    && old
                        .edit
                        .repeat_grid(old_layer)
                        .map(|(dimensions, _)| dimensions)
                        == next
                            .edit
                            .repeat_grid(next_layer)
                            .map(|(dimensions, _)| dimensions)
                {
                    next.grid_selection.copy_points_from(&old.grid_selection);
                }
            }
            if let Some((old_layer, next_layer)) =
                old.source_guide_layer().zip(next.source_guide_layer())
                && old
                    .edit
                    .same_layer_source(old_layer, &next.edit, next_layer)
            {
                if old.edit.layer(old_layer) == next.edit.layer(next_layer) {
                    next.source_navigation
                        .copy_view_from(&old.source_navigation);
                }
                next.guide_source = old.guide_source.take();
            }
        }
        self.synthesis = next;
        if let Some(panel) = self.synthesis.as_ref()
            && panel.manual.is_none()
            && let Some(SynthesisGeometry::Placement(index)) = panel.geometry
        {
            self.install_inline_synthesis_manual(Some(index), window, cx);
        }
    }

    pub(in crate::graph_document::view) fn refresh_synthesis_guide_source(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        if self
            .synthesis
            .as_ref()
            .is_some_and(|panel| panel.manual.is_some())
        {
            self.refresh_inline_synthesis_manual(cx);
            return;
        }
        let Some(panel) = self.synthesis.as_ref() else {
            return;
        };
        let request = panel
            .source_guide_layer()
            .and_then(|index| self.synthesis_source_request(panel, index));
        let Some(panel) = self.synthesis.as_mut() else {
            return;
        };
        let (key, job) = match request {
            None => {
                if panel.guide_source.take().is_some() {
                    cx.notify();
                }
                return;
            }
            Some(Err(error)) => {
                panel.guide_source = Some(Preview {
                    key: None,
                    state: State::Failed(error),
                });
                cx.notify();
                return;
            }
            Some(Ok(request)) => request,
        };
        if panel
            .guide_source
            .as_ref()
            .is_some_and(|source| source.key.as_ref() == Some(&key))
        {
            return;
        }
        let ticket = Ticket::new();
        panel.guide_source = Some(Preview {
            key: Some(key),
            state: State::Loading(WorkTicket(ticket.clone())),
        });
        let library = self.environment_library.clone();
        let worker = ticket.clone();
        cx.spawn(async move |owner, cx| {
            let result = cx
                .background_spawn(async move { job.render(&library, &worker) })
                .await;
            let _ = owner.update(cx, |view, cx| {
                if view.closed || !ticket.is_current() {
                    return;
                }
                let Some(preview) = view
                    .synthesis
                    .as_mut()
                    .and_then(|p| p.guide_source.as_mut())
                else {
                    return;
                };
                if !matches!(&preview.state, State::Loading(current)
                    if Arc::ptr_eq(&current.0.active, &ticket.active))
                {
                    return;
                }
                preview.state = match result {
                    Ok(raster) => State::Ready(raster),
                    Err(error) => State::Failed(error),
                };
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{
        Bounds, InputEvent as _, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent,
        MouseUpEvent, Pixels, Point, ScrollDelta, ScrollWheelEvent, TestAppContext, WindowBounds,
        point, px, size, test::TestWindowExt,
    };
    use image::ImageEncoder;
    use mycela_engine::{CanvasPrimitive, StyleEdit, StylePaintKind};

    fn open(cx: &mut TestAppContext, node: Node) -> (AnyWindowHandle, Entity<GraphDocumentView>) {
        open_document(cx, Gtx375GraphDocument::new(vec![Some(node)]).unwrap())
    }

    fn open_document(
        cx: &mut TestAppContext,
        document: Gtx375GraphDocument<ForegroundError>,
    ) -> (AnyWindowHandle, Entity<GraphDocumentView>) {
        open_document_sized(cx, document, size(px(1200.), px(1800.)))
    }

    fn open_document_sized(
        cx: &mut TestAppContext,
        document: Gtx375GraphDocument<ForegroundError>,
        window_size: gpui_kit::Size<gpui_kit::Pixels>,
    ) -> (AnyWindowHandle, Entity<GraphDocumentView>) {
        let document = Arc::new(Mutex::new(document));
        let (window, view) = cx
            .update(|cx| {
                gpui_kit::open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                            Default::default(),
                            window_size,
                        ))),
                        ..Default::default()
                    },
                    cx,
                    |window, cx| {
                        cx.new(|cx| {
                            GraphDocumentView::with_document(document, None, 0., false, window, cx)
                        })
                    },
                )
            })
            .unwrap();
        cx.run_until_parked();
        (window, view)
    }

    fn click(cx: &mut TestAppContext, window: AnyWindowHandle, target: &'static str) {
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            window.click(target, cx);
        })
        .unwrap();
        cx.run_until_parked();
    }

    #[gpui_kit::test]
    fn manual_preview_freezes_current_graph_input_and_seed_with_a_detached_placement_draft(
        cx: &mut TestAppContext,
    ) {
        use mycela_engine::SynthesisPlacementTransform;

        cx.update(gpui_kit::init);
        let node = new_graph_node("Synthesis").unwrap();
        let upstream = Node::solid_color().unwrap();
        upstream.set_solid_color([1., 0., 0.]).unwrap();
        node.slot(0).unwrap().set_node(Some(&upstream)).unwrap();
        let filter = node.filter().unwrap();
        filter.parameter(1).unwrap().set_number(17.).unwrap();
        let mut edit = filter.synthesis_edit().unwrap();
        edit.add_layer(SynthesisLayerKind::Patches);
        edit.set_input_source(0);
        edit.set_automatic_placement(0, false);
        assert_eq!(edit.add_placement(0, 0), Some(0));
        assert!(
            filter
                .edit_synthesis(edit, "Setup Manual placement")
                .unwrap()
        );
        let (_window, view) = open(cx, node);
        let library = EnvironmentResourceLibrary::default();
        let prepare = |cx: &mut TestAppContext, move_draft| {
            cx.update(|cx| {
                let view = view.read(cx);
                let panel = view.synthesis.as_ref().unwrap();
                let target = Target::new(view, panel);
                let mut edit = panel.edit.clone();
                if move_draft {
                    assert!(edit.transform_placement(
                        0,
                        0,
                        SynthesisPlacementTransform::Move {
                            delta: [0.3, 0.],
                            wrap: false,
                        },
                        ImageGeometry::new(LIVE_PREVIEW_SIZE, LIVE_PREVIEW_SIZE),
                    ));
                }
                view.prepare_synthesis_manual_preview(&target, &edit)
                    .unwrap()
                    .unwrap()
            })
        };
        let original = prepare(cx, false);
        let detached = prepare(cx, true);
        assert_eq!(detached.seed, Some(17));
        assert!(detached.input.is_some());
        upstream.set_solid_color([0., 0., 1.]).unwrap();
        filter.parameter(1).unwrap().set_number(29.).unwrap();

        let ticket = Ticket::new();
        let (original, source_geometry) = original.render(&library, &ticket).unwrap();
        let (detached, _) = detached.render(&library, &ticket).unwrap();
        let (current, _) = prepare(cx, true).render(&library, &ticket).unwrap();
        assert_eq!(source_geometry, Some(original.geometry));
        let pixel = |raster: &Raster, x: usize| {
            let i = ((LIVE_PREVIEW_SIZE * LIVE_PREVIEW_SIZE / 2) as usize + x) * 4;
            let pixel: [u8; 4] = raster.image.as_bytes(0).unwrap()[i..i + 4]
                .try_into()
                .unwrap();
            pixel
        };
        let center = LIVE_PREVIEW_SIZE as usize / 2;
        let moved = 4 * LIVE_PREVIEW_SIZE as usize / 5;
        assert_eq!(
            original.geometry,
            ImageGeometry::new(LIVE_PREVIEW_SIZE, LIVE_PREVIEW_SIZE)
        );
        assert_eq!(pixel(&original, center), [0, 0, 255, 255]);
        assert_eq!(pixel(&detached, center)[3], 0);
        assert_eq!(pixel(&detached, moved), [0, 0, 255, 255]);
        assert_eq!(pixel(&current, moved), [255, 0, 0, 255]);
        assert_eq!(
            filter
                .synthesis_edit()
                .unwrap()
                .placement_geometry(0, 0)
                .unwrap()
                .center(),
            [0.5, 0.5],
        );
        ticket.retire();
        assert!(prepare(cx, true).render(&library, &ticket).is_err());
    }

    fn ready(cx: &mut TestAppContext, view: &Entity<GraphDocumentView>) -> Raster {
        cx.update(|cx| {
            let source = view
                .read(cx)
                .synthesis
                .as_ref()
                .unwrap()
                .guide_source
                .as_ref()
                .expect("the guide source was requested");
            match &source.state {
                State::Ready(raster) => raster.clone(),
                State::Loading(_) => panic!("the source did not finish"),
                State::Failed(error) => panic!("{error}"),
            }
        })
    }

    fn red_photo() -> EnvironmentImage {
        red_photo_at(4, 2)
    }

    fn red_photo_at(width: u32, height: u32) -> EnvironmentImage {
        photo(
            width,
            height,
            &[255, 0, 0, 255].repeat((width * height) as usize),
        )
    }

    fn photo(width: u32, height: u32, pixels: &[u8]) -> EnvironmentImage {
        let mut bytes = Vec::new();
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(pixels, width, height, image::ExtendedColorType::Rgba8)
            .unwrap();
        EnvironmentImage::from_file_bytes(bytes, "png").unwrap()
    }

    #[gpui_kit::test]
    fn full_size_synthesis_asset_keeps_the_photo_dimensions_without_reusing_the_inline_raster(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let node = new_graph_node("Synthesis").unwrap();
        let filter = node.filter().unwrap();
        let mut edit = filter.synthesis_edit().unwrap();
        edit.add_layer(SynthesisLayerKind::Repeating);
        assert!(edit.set_image_source(0, red_photo_at(600, 900)));
        filter
            .edit_synthesis(edit, "Set up a portrait source")
            .unwrap();
        let (window, view) = open(cx, node);
        click(cx, window, "synthesis-grid-toggle");
        let fitted = ready(cx, &view);
        assert!(fitted.geometry.height() <= LIVE_PREVIEW_SIZE);

        let preparation = cx.update(|cx| {
            let view = view.read(cx);
            view.prepare_full_size_synthesis_source(view.synthesis.as_ref().unwrap(), 0)
        });
        let GuidePreparation::Pending(source) = preparation else {
            panic!("the full-size editor must request its own asset quality");
        };
        let full = source
            .render(&EnvironmentResourceLibrary::default(), &Ticket::new())
            .unwrap();
        assert_eq!(full.geometry, ImageGeometry::new(600, 900));
        assert_ne!(full.image.id, fitted.image.id);
        assert_eq!(ready(cx, &view).image.id, fitted.image.id);
    }

    #[test]
    fn full_size_synthesis_embedded_asset_caps_the_width_and_keeps_proportional_height() {
        let node = new_graph_node("Synthesis").unwrap();
        let mut edit = node.filter().unwrap().synthesis_edit().unwrap();
        edit.add_layer(SynthesisLayerKind::Repeating);
        assert!(edit.set_image_source(0, red_photo_at(5000, 11)));
        let raster = Job::FullSizeAsset(edit, 0, None)
            .render(&EnvironmentResourceLibrary::default(), &Ticket::new())
            .unwrap();
        assert_eq!(raster.geometry, ImageGeometry::new(4000, 9));
    }

    fn graph_color() -> Node {
        let input = Node::solid_color().unwrap();
        input.set_solid_color([0., 1., 0.]).unwrap();
        input
    }

    fn edit_full_size_source(
        cx: &mut TestAppContext,
        window: AnyWindowHandle,
        from: [f32; 2],
        to: [f32; 2],
    ) {
        let inline = cx
            .update_window(window, |_, window, cx| {
                window.render_frame(cx);
                assert!(window.find("synthesis-full-size").visible());
                window
                    .try_find("synthesis-geometry-surface")
                    .map(|guide| guide.bounds().size)
            })
            .unwrap();
        click(cx, window, "synthesis-full-size");
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            let bounds = window.find("synthesis-full-size-surface").bounds();
            if let Some(inline) = inline {
                assert!(bounds.size.width > inline.width * 2.);
                assert!(bounds.size.height > inline.height * 2.);
            }
            let margin = window.rem_size();
            let height = (bounds.size.height - margin).min((bounds.size.width - margin) / 2.);
            let position = |[x, y]: [f32; 2]| {
                bounds.center() + gpui_kit::point((x - 0.5) * 2. * height, (y - 0.5) * height)
            };
            window.drag(position(from), position(to), cx);
        })
        .unwrap();
        cx.run_until_parked();
    }

    fn close_full_size_source(
        cx: &mut TestAppContext,
        window: AnyWindowHandle,
        button: &'static str,
    ) {
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            window.within("dialog").click(button, cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("synthesis-full-size-surface").is_none());
        })
        .unwrap();
    }

    fn pan_source(window: &mut Window, cx: &mut App, from: Point<Pixels>, to: Point<Pixels>) {
        window.dispatch_event(
            MouseDownEvent {
                button: MouseButton::Right,
                position: from,
                click_count: 1,
                ..Default::default()
            }
            .to_platform_input(),
            cx,
        );
        window.dispatch_event(
            MouseMoveEvent {
                position: to,
                pressed_button: Some(MouseButton::Right),
                ..Default::default()
            }
            .to_platform_input(),
            cx,
        );
        window.dispatch_event(
            MouseUpEvent {
                button: MouseButton::Right,
                position: to,
                click_count: 1,
                ..Default::default()
            }
            .to_platform_input(),
            cx,
        );
        window.render_frame(cx);
    }

    fn shift_click_source_point(window: &mut Window, cx: &mut App, position: Point<Pixels>) {
        let modifiers = Modifiers {
            shift: true,
            ..Default::default()
        };
        window.dispatch_event(
            MouseDownEvent {
                button: MouseButton::Left,
                position,
                modifiers,
                click_count: 1,
                ..Default::default()
            }
            .to_platform_input(),
            cx,
        );
        window.dispatch_event(
            MouseUpEvent {
                button: MouseButton::Left,
                position,
                modifiers,
                click_count: 1,
            }
            .to_platform_input(),
            cx,
        );
        window.render_frame(cx);
    }

    fn project_fitted_source(
        surface: Bounds<Pixels>,
        margin: Pixels,
        [x, y]: [f32; 2],
    ) -> Point<Pixels> {
        let height = (surface.size.height - margin).min((surface.size.width - margin) / 2.);
        surface.center() + point((x - 0.5) * 2. * height, (y - 0.5) * height)
    }

    fn assert_source_numeric_fields(window: &Window, expected: [f32; 2]) {
        for (index, expected) in expected.into_iter().enumerate() {
            let field = window.find(("synthesis-full-size-field", index));
            let actual: f32 = field.value().unwrap().parse().unwrap();
            assert!(
                (actual - expected).abs() < 0.002,
                "field {index} was {actual}"
            );
        }
    }

    #[gpui_kit::test]
    fn full_size_source_zooms_at_the_pointer_pans_and_preserves_edit_coordinates(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_reduce_motion(true);
            bind_keys(cx);
        });
        let node = new_graph_node("Synthesis").unwrap();
        let filter = node.filter().unwrap();
        let mut edit = filter.synthesis_edit().unwrap();
        edit.add_layer(SynthesisLayerKind::Repeating);
        assert!(edit.set_image_source(0, red_photo()));
        let initial = [0.32, 0.37];
        assert!(edit.move_geometry_point(0, SynthesisGeometry::GridPoint(0), initial));
        filter
            .edit_synthesis(edit, "Set up source navigation")
            .unwrap();
        let grid = || {
            filter
                .synthesis_edit()
                .unwrap()
                .repeat_grid(0)
                .unwrap()
                .1
                .to_vec()
        };
        let original = grid();
        let next = [0.375, 0.325];
        let doc = Gtx375GraphDocument::new(vec![Some(node)]).unwrap();
        let (window, view) = open_document_sized(cx, doc, size(px(1400.), px(807.)));
        let check_unchanged = |cx: &mut TestAppContext| {
            assert_eq!(grid(), original);
            cx.update(|cx| assert!(!view.read(cx).is_dirty()));
        };

        let edit_after_navigation = |cx: &mut TestAppContext, check_fit: bool| {
            click(cx, window, "synthesis-full-size");
            cx.update_window(window, |_, window, cx| {
                const SURFACE: &str = "synthesis-full-size-surface";
                const ZOOM: &str = "synthesis-source-zoom-value";
                window.render_frame(cx);
                assert_eq!(window.find(ZOOM).label(), Some("100%"));
                if check_fit {
                    window.click("synthesis-source-zoom-in", cx);
                    assert_eq!(window.find(ZOOM).label(), Some("200%"));
                    window.click("synthesis-source-zoom-out", cx);
                    assert_eq!(window.find(ZOOM).label(), Some("100%"));
                }

                let surface = window.find(SURFACE).bounds();
                let margin = window.rem_size();
                let project = |position| project_fitted_source(surface, margin, position);
                let anchor = project(initial);
                let wheel = |window: &mut Window, cx: &mut App, lines| {
                    window.dispatch_event(
                        ScrollWheelEvent {
                            position: anchor,
                            delta: ScrollDelta::Lines(point(0., lines)),
                            ..Default::default()
                        }
                        .to_platform_input(),
                        cx,
                    );
                };
                wheel(window, cx, 1.);
                if check_fit {
                    wheel(window, cx, 1.);
                    window.render_frame(cx);
                    assert_eq!(window.find(ZOOM).label(), Some("400%"));
                    wheel(window, cx, -1.);
                }
                window.render_frame(cx);
                assert_eq!(window.find(ZOOM).label(), Some("200%"));
                window.click_at(SURFACE, anchor - surface.origin, cx);
                assert_source_numeric_fields(window, initial);

                let panned = anchor + point(px(32.), px(22.));
                pan_source(window, cx, anchor, panned);
                assert_source_numeric_fields(window, initial);
                let movement = (project(next) - project(initial)) * 2.;
                window.drag(panned, panned + movement, cx);
                assert_source_numeric_fields(window, next);

                if check_fit {
                    window.click("synthesis-source-fit", cx);
                    assert_eq!(window.find(ZOOM).label(), Some("100%"));
                    assert_source_numeric_fields(window, next);
                    window.click_at(SURFACE, project(original[1]) - surface.origin, cx);
                    assert_source_numeric_fields(window, original[1]);
                }
            })
            .unwrap();
            cx.run_until_parked();
            check_unchanged(cx);
        };

        edit_after_navigation(cx, true);
        close_full_size_source(cx, window, "cancel");
        check_unchanged(cx);

        edit_after_navigation(cx, false);
        close_full_size_source(cx, window, "ok");
        let applied = grid();
        for (actual, expected) in applied[0].into_iter().zip(next) {
            assert!((actual - expected).abs() < 0.002);
        }
        assert_eq!(applied[1..], original[1..]);
        cx.update_window(window, |_, window, cx| window.press("secondary-z", cx))
            .unwrap();
        cx.run_until_parked();
        check_unchanged(cx);
    }

    #[gpui_kit::test]
    fn inline_repeat_source_keeps_its_navigation_while_editing_and_resets_with_fit(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_reduce_motion(true);
            bind_keys(cx);
        });
        let node = new_graph_node("Synthesis").unwrap();
        let filter = node.filter().unwrap();
        let mut edit = filter.synthesis_edit().unwrap();
        edit.add_layer(SynthesisLayerKind::Repeating);
        assert!(edit.set_image_source(0, red_photo_at(160, 80)));
        let initial = [0.36, 0.4];
        assert!(edit.move_geometry_point(0, SynthesisGeometry::GridPoint(0), initial));
        filter
            .edit_synthesis(edit, "Set up inline source navigation")
            .unwrap();
        let grid = || {
            filter
                .synthesis_edit()
                .unwrap()
                .repeat_grid(0)
                .unwrap()
                .1
                .to_vec()
        };
        let original = grid();
        let next = [0.42, 0.365];
        let doc = Gtx375GraphDocument::new(vec![Some(node)]).unwrap();
        let (window, view) = open_document_sized(cx, doc, size(px(1400.), px(807.)));
        click(cx, window, "synthesis-grid-toggle");
        assert_eq!(ready(cx, &view).geometry, ImageGeometry::new(160, 80));
        cx.update_window(window, |_, window, cx| {
            const SURFACE: &str = "synthesis-geometry-surface";
            const ZOOM: &str = "synthesis-inline-source-zoom-value";
            window.render_frame(cx);
            // The mock platform does not deliver the native frame callbacks
            // used to measure the new source row and then reveal it.
            for _ in 0..2 {
                window.simulate_next_frame(cx);
                window.render_frame(cx);
            }
            let inspector = window.find("graph-inspector-scroll").bounds();
            for id in [
                "synthesis-grid-toggle",
                SURFACE,
                ZOOM,
                "synthesis-inline-source-zoom-in",
                "synthesis-inline-source-zoom-out",
                "synthesis-inline-source-fit",
            ] {
                let target = window.find(id);
                let bounds = target.bounds();
                assert!(
                    target.visible()
                        && bounds.top() >= inspector.top()
                        && bounds.bottom() <= inspector.bottom(),
                    "{id} is outside the inspector after opening the grid"
                );
            }
            assert_eq!(window.find(ZOOM).label(), Some("100%"));
            window.click("synthesis-inline-source-zoom-in", cx);
            assert_eq!(window.find(ZOOM).label(), Some("200%"));
            window.click("synthesis-inline-source-zoom-out", cx);
            assert_eq!(window.find(ZOOM).label(), Some("100%"));

            let surface = window.find(SURFACE).bounds();
            let margin = window.rem_size();
            let project = |position| project_fitted_source(surface, margin, position);
            let anchor = project(initial);
            window.dispatch_event(
                ScrollWheelEvent {
                    position: anchor,
                    delta: ScrollDelta::Lines(point(0., 1.)),
                    ..Default::default()
                }
                .to_platform_input(),
                cx,
            );
            window.render_frame(cx);
            assert_eq!(window.find(ZOOM).label(), Some("200%"));
            window.click_at(SURFACE, anchor - surface.origin, cx);
            let selected = |cx: &App| view.read(cx).synthesis.as_ref().unwrap().geometry;
            assert_eq!(selected(cx), Some(SynthesisGeometry::GridPoint(0)));
            assert_eq!(window.find(SURFACE).bounds(), surface);

            let panned = anchor + point(px(22.), px(7.));
            pan_source(window, cx, anchor, panned);
            assert_eq!(grid(), original);
            assert!(!view.read(cx).is_dirty());
            let movement = (project(next) - project(initial)) * 2.;
            window.drag(panned, panned + movement, cx);
            assert_eq!(window.find(ZOOM).label(), Some("200%"));
            let applied = grid();
            for (actual, expected) in applied[0].into_iter().zip(next) {
                assert!((actual - expected).abs() < 0.002);
            }
            assert_eq!(applied[1..], original[1..]);

            window.click("synthesis-inline-source-fit", cx);
            assert_eq!(window.find(ZOOM).label(), Some("100%"));
            window.click_at(SURFACE, project(original[1]) - surface.origin, cx);
            assert_eq!(selected(cx), Some(SynthesisGeometry::GridPoint(1)));
            assert_eq!(grid(), applied);
        })
        .unwrap();
        cx.update_window(window, |_, window, cx| window.press("secondary-z", cx))
            .unwrap();
        cx.run_until_parked();
        assert_eq!(grid(), original);
        cx.update(|cx| assert!(!view.read(cx).is_dirty()));
    }

    #[gpui_kit::test]
    fn full_size_repeat_shift_and_marquee_selection_move_only_the_selected_points(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_reduce_motion(true);
            bind_keys(cx);
        });
        let node = new_graph_node("Synthesis").unwrap();
        let filter = node.filter().unwrap();
        let mut edit = filter.synthesis_edit().unwrap();
        edit.add_layer(SynthesisLayerKind::Repeating);
        assert!(edit.set_image_source(0, red_photo_at(160, 80)));
        filter
            .edit_synthesis(edit, "Set up repeat selection")
            .unwrap();
        let grid = || {
            filter
                .synthesis_edit()
                .unwrap()
                .repeat_grid(0)
                .unwrap()
                .1
                .to_vec()
        };
        let original = grid();
        let top = [0.03, 0.04];
        let bottom = [-0.03, -0.04];
        let moved = |[x, y]: [f32; 2], [dx, dy]: [f32; 2]| [x + dx, y + dy];

        let doc = Gtx375GraphDocument::new(vec![Some(node)]).unwrap();
        let (window, _) = open_document_sized(cx, doc, size(px(1400.), px(807.)));
        click(cx, window, "synthesis-full-size");
        cx.update_window(window, |_, window, cx| {
            const SURFACE: &str = "synthesis-full-size-surface";
            window.render_frame(cx);
            let surface = window.find(SURFACE).bounds();
            let margin = window.rem_size();
            let at = |position| project_fitted_source(surface, margin, position);

            window.click_at(SURFACE, at(original[0]) - surface.origin, cx);
            assert_source_numeric_fields(window, original[0]);
            shift_click_source_point(window, cx, at(original[1]));
            assert_source_numeric_fields(window, [2., 2.]);
            shift_click_source_point(window, cx, at(original[1]));
            assert_source_numeric_fields(window, original[0]);
            shift_click_source_point(window, cx, at(original[1]));
            window.drag(at(original[0]), at(moved(original[0], top)), cx);
            assert_source_numeric_fields(window, [2., 2.]);

            window.drag(
                at(moved(original[7], [-0.07, -0.07])),
                at(moved(original[8], [0.07, 0.07])),
                cx,
            );
            window.drag(at(original[8]), at(moved(original[8], bottom)), cx);
            assert_source_numeric_fields(window, [2., 2.]);
        })
        .unwrap();
        assert_eq!(grid(), original);
        close_full_size_source(cx, window, "ok");
        let applied = grid();
        assert_eq!(applied.len(), original.len());
        for (index, actual) in applied.into_iter().enumerate() {
            let expected = match index {
                0 | 1 => moved(original[index], top),
                7 | 8 => moved(original[index], bottom),
                _ => original[index],
            };
            assert!(
                actual
                    .into_iter()
                    .zip(expected)
                    .all(|(actual, expected)| (actual - expected).abs() < 0.002),
                "point {index} was {actual:?}"
            );
        }
        cx.update_window(window, |_, window, cx| window.press("secondary-z", cx))
            .unwrap();
        cx.run_until_parked();
        assert_eq!(grid(), original);
    }

    #[gpui_kit::test]
    fn patches_source_list_selects_contours_without_history_and_scrolls_independently(
        cx: &mut TestAppContext,
    ) {
        use mycela_engine::{LabNumericLink, LabNumericValue};

        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_reduce_motion(true);
            bind_keys(cx);
        });
        let node = new_graph_node("Synthesis").unwrap();
        let filter = node.filter().unwrap();
        let mut edit = filter.synthesis_edit().unwrap();
        edit.add_layer(SynthesisLayerKind::Patches);
        assert!(edit.set_image_source(0, red_photo_at(160, 80)));
        for index in 1..10 {
            assert_eq!(edit.add_patch(0), Some(index));
        }
        let second = SynthesisGeometry::Patch {
            selection: 1,
            knot: 0,
        };
        let knot_position = [0.4, 0.4];
        assert!(edit.move_geometry_point(
            0,
            SynthesisGeometry::Patch {
                selection: 1,
                knot: 1,
            },
            knot_position
        ));
        let frequency = SynthesisNumberField::PatchFrequency(1);
        let weights = edit
            .geometry_fields(0, second)
            .into_iter()
            .map(|field| {
                if field.number_field() == Some(frequency) {
                    53.
                } else {
                    field.value()
                }
            })
            .collect::<Vec<_>>();
        edit.set_geometry_fields(0, second, &weights).unwrap();
        filter.edit_synthesis(edit, "Set up Patches list").unwrap();
        let patch_count = || filter.synthesis_edit().unwrap().patch_counts(0).unwrap().0;
        let mut doc = Gtx375GraphDocument::new(vec![Some(node.clone())]).unwrap();
        let draft = || filter.synthesis_edit().unwrap();
        assert!(
            doc.set_synthesis_number_adjuster(
                &node,
                &draft(),
                Some(0),
                frequency,
                LabNumericLink::Create("Patch frequency".into()),
            )
            .unwrap()
        );
        let control = doc
            .synthesis_number_adjuster(&node, &draft(), Some(0), frequency)
            .unwrap();
        assert!(
            doc.edit_numeric_lab_adjuster(None, &control, LabNumericValue::Number(63.))
                .unwrap()
        );
        assert_eq!(draft().number_value(Some(0), frequency), Some(63.));
        let (window, view) = open_document_sized(cx, doc, size(px(1700.), px(980.)));
        let selected = |cx: &App| view.read(cx).synthesis.as_ref().unwrap().geometry;
        click(cx, window, "synthesis-edit-patches");
        assert_eq!(ready(cx, &view).geometry, ImageGeometry::new(160, 80));
        cx.update_window(window, |_, window, cx| {
            const LIST: &str = "synthesis-patch-list";
            const ROW: &str = "synthesis-patch-row";
            const SURFACE: &str = "synthesis-geometry-surface";
            const ZOOM: &str = "synthesis-inline-source-zoom-value";
            window.render_frame(cx);
            for _ in 0..2 {
                window.simulate_next_frame(cx);
                window.render_frame(cx);
            }
            let inspector = window.find("graph-inspector-scroll").bounds();
            let first = window.find((ROW, 0_usize));
            assert_eq!(first.selected(), Some(true));
            assert!(first.visible() && first.bounds().top() >= inspector.top());
            assert!(first.bounds().bottom() <= inspector.bottom());
            let guide = window.find(SURFACE).bounds();
            assert!(first.bounds().left() >= guide.right());
            assert!(first.bounds().top() >= guide.top());
            assert!(first.bounds().bottom() <= guide.bottom());

            window.click("synthesis-inline-source-zoom-in", cx);
            let surface = window.find(SURFACE).bounds();
            let margin = window.rem_size();
            let pan = point(px(12.), px(3.));
            pan_source(window, cx, surface.center(), surface.center() + pan);
            let second_row = window.find((ROW, 1_usize));
            assert_eq!(second_row.label(), Some("Patch 2, Frequency 63%"));
            let rows = window.find("synthesis-patch-list-rows").bounds();
            let second_bounds = second_row.bounds();
            assert!((second_bounds.right() - rows.right()).abs() <= px(1.));
            window.click_at(
                (ROW, 1_usize),
                point(
                    second_bounds.size.width - px(24.),
                    second_bounds.size.height / 2.,
                ),
                cx,
            );
            assert_eq!(selected(cx), Some(second));
            assert_eq!(window.find((ROW, 1_usize)).selected(), Some(true));
            assert_eq!(window.find(LIST).focused(), Some(true));
            assert_eq!(window.find(ZOOM).label(), Some("200%"));
            assert_eq!(window.find(SURFACE).bounds(), surface);

            let knot = project_fitted_source(surface, margin, knot_position);
            let projected = surface.center() + (knot - surface.center()) * 2. + pan;
            window.click_at(SURFACE, projected - surface.origin, cx);
            assert_eq!(
                selected(cx),
                Some(SynthesisGeometry::Patch {
                    selection: 1,
                    knot: 1,
                })
            );

            let wheel_target = window.find((ROW, 1_usize)).bounds().center();
            assert!(inspector.contains(&wheel_target));
            let offset = view.read(cx).inspector_scroll.offset();
            window.dispatch_event(
                ScrollWheelEvent {
                    position: wheel_target,
                    delta: ScrollDelta::Pixels(point(px(0.), px(-56.))),
                    ..Default::default()
                }
                .to_platform_input(),
                cx,
            );
            window.render_frame(cx);
            assert_eq!(window.find(SURFACE).bounds(), surface);
            assert_eq!(view.read(cx).inspector_scroll.offset(), offset);
            assert_eq!(window.find(ZOOM).label(), Some("200%"));
            assert!(window.find((ROW, 3_usize)).visible());

            window.click((ROW, 3_usize), cx);
            window.press("end", cx);
            assert_eq!(
                selected(cx),
                Some(SynthesisGeometry::Patch {
                    selection: 9,
                    knot: 0,
                })
            );
            assert!(window.find((ROW, 9_usize)).visible());
            assert_eq!(window.find((ROW, 9_usize)).selected(), Some(true));
            assert!(!view.read(cx).is_dirty());
        })
        .unwrap();
        assert_eq!(patch_count(), 10);

        click(cx, window, "synthesis-create-rectangle");
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            let added = window.find(("synthesis-patch-row", 10_usize));
            assert_eq!(added.selected(), Some(true));
            assert!(added.visible());
            assert_eq!(window.find("synthesis-patch-list").focused(), Some(true));
            window.press("secondary-z", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(patch_count(), 10);
        cx.update(|cx| assert!(!view.read(cx).is_dirty()));
    }

    #[gpui_kit::test]
    fn compact_patches_picker_keeps_the_full_width_source_and_selects_without_history(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_reduce_motion(true);
            bind_keys(cx);
        });
        let node = new_graph_node("Synthesis").unwrap();
        let filter = node.filter().unwrap();
        let mut edit = filter.synthesis_edit().unwrap();
        edit.add_layer(SynthesisLayerKind::Patches);
        assert!(edit.set_image_source(0, red_photo_at(160, 80)));
        for index in 1..10 {
            assert_eq!(edit.add_patch(0), Some(index));
        }
        let weights = [0, 9].map(|index| {
            edit.number_value(Some(0), SynthesisNumberField::PatchFrequency(index))
                .unwrap()
        });
        filter
            .edit_synthesis(edit, "Set up compact patch choices")
            .unwrap();
        let document = Gtx375GraphDocument::new(vec![Some(node)]).unwrap();
        let (window, view) = open_document_sized(cx, document, size(px(1180.), px(840.)));
        click(cx, window, "synthesis-edit-patches");
        cx.update_window(window, |_, window, cx| {
            const GUIDE: &str = "synthesis-geometry-surface";
            const PICKER: &str = "synthesis-patch-picker";
            const ZOOM: &str = "synthesis-inline-source-zoom-value";
            window.render_frame(cx);
            for _ in 0..2 {
                window.simulate_next_frame(cx);
                window.render_frame(cx);
            }
            let guide = window.find(GUIDE);
            let before = guide.bounds();
            assert!(guide.visible() && before.size.width >= px(240.));
            assert!(window.try_find("synthesis-patch-list").is_none());
            let picker = window.find(PICKER);
            let picker_before = picker.bounds();
            assert!(picker.visible());
            window.click("synthesis-inline-source-zoom-in", cx);
            window.click(PICKER, cx);
            let first = format!("Patch 1 · {:.0}% (selected)", weights[0]);
            let last = format!("Patch 10 · {:.0}%", weights[1]);
            let mut popup = window.within("popup-menu");
            assert_eq!(popup.find(0).label(), Some(first.as_str()));
            assert_eq!(popup.find(9).label(), Some(last.as_str()));
            popup.click(9, cx);
            assert_eq!(
                view.read(cx).synthesis.as_ref().unwrap().geometry,
                Some(SynthesisGeometry::Patch {
                    selection: 9,
                    knot: 0
                }),
            );
            let selected = format!("Select patch. Patch 10, Frequency {:.0}%", weights[1]);
            let picker = window.find(PICKER);
            assert_eq!(picker.label(), Some(selected.as_str()));
            assert_eq!(picker.bounds(), picker_before);
            assert_eq!(window.find(GUIDE).bounds(), before);
            assert_eq!(window.find(ZOOM).label(), Some("200%"));
            assert!(!view.read(cx).is_dirty());
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn full_size_patches_list_selects_locally_and_reveals_the_nearest_row_after_removal(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_reduce_motion(true);
            bind_keys(cx);
        });
        let node = new_graph_node("Synthesis").unwrap();
        let filter = node.filter().unwrap();
        let mut edit = filter.synthesis_edit().unwrap();
        edit.add_layer(SynthesisLayerKind::Patches);
        assert!(edit.set_image_source(0, red_photo_at(160, 80)));
        assert_eq!(edit.add_patch(0), Some(1));
        assert_eq!(edit.add_patch(0), Some(2));
        filter
            .edit_synthesis(edit, "Set up full-size Patches list")
            .unwrap();
        let patch_count = || filter.synthesis_edit().unwrap().patch_counts(0).unwrap().0;
        let selected = |cx: &App, view: &Entity<GraphDocumentView>| {
            view.read(cx).synthesis.as_ref().unwrap().geometry
        };
        let doc = Gtx375GraphDocument::new(vec![Some(node)]).unwrap();
        let (window, view) = open_document_sized(cx, doc, size(px(1180.), px(840.)));
        click(cx, window, "synthesis-full-size");
        cx.update_window(window, |_, window, cx| {
            const ROW: &str = "synthesis-full-patch-row";
            window.render_frame(cx);
            assert!(window.find((ROW, 0_usize)).visible());
            assert_eq!(window.find((ROW, 0_usize)).selected(), Some(true));
            window.click((ROW, 2_usize), cx);
            assert_eq!(window.find((ROW, 2_usize)).selected(), Some(true));
            assert!(selected(cx, &view).is_none());
        })
        .unwrap();
        close_full_size_source(cx, window, "cancel");
        assert_eq!(patch_count(), 3);
        cx.update(|cx| assert!(selected(cx, &view).is_none()));

        click(cx, window, "synthesis-full-size");
        cx.update_window(window, |_, window, cx| {
            const ROW: &str = "synthesis-full-patch-row";
            window.render_frame(cx);
            window.click((ROW, 2_usize), cx);
            window.click("synthesis-full-remove-patch", cx);
            assert_eq!(window.find((ROW, 1_usize)).selected(), Some(true));
            assert_eq!(
                window.find("synthesis-full-patch-list").focused(),
                Some(true)
            );
        })
        .unwrap();
        assert_eq!(patch_count(), 3);
        close_full_size_source(cx, window, "ok");
        assert_eq!(patch_count(), 2);
        cx.update(|cx| {
            assert_eq!(
                selected(cx, &view),
                Some(SynthesisGeometry::Patch {
                    selection: 1,
                    knot: 0,
                })
            )
        });
        cx.update_window(window, |_, window, cx| window.press("secondary-z", cx))
            .unwrap();
        cx.run_until_parked();
        assert_eq!(patch_count(), 3);
        cx.update(|cx| assert!(!view.read(cx).is_dirty()));
    }

    #[gpui_kit::test]
    fn patches_source_transforms_preview_cancel_and_commit_once(cx: &mut TestAppContext) {
        use mycela_engine::SynthesisPatchTransformField::{CenterX, Height, Width};
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_reduce_motion(true);
            bind_keys(cx);
        });
        let node = new_graph_node("Synthesis").unwrap();
        let filter = node.filter().unwrap();
        let mut edit = filter.synthesis_edit().unwrap();
        edit.add_layer(SynthesisLayerKind::Patches);
        assert!(edit.set_image_source(0, red_photo_at(160, 80)));
        let fitted = Some(ImageGeometry::new(160, 80));
        assert!(edit.set_patch_transform_field(0, 0, Width, 17., fitted));
        assert!(edit.set_patch_transform_field(0, 0, Height, 20., fitted));
        filter
            .edit_synthesis(edit, "Set up patch transform")
            .unwrap();
        let bounds = || {
            filter
                .synthesis_edit()
                .unwrap()
                .patch_transform_bounds(0, 0)
                .unwrap()
        };
        let original = bounds();
        let document = Gtx375GraphDocument::new(vec![Some(node)]).unwrap();
        let (window, view) = open_document_sized(cx, document, size(px(1180.), px(840.)));
        click(cx, window, "synthesis-edit-patches");
        ready(cx, &view);
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            for _ in 0..2 {
                window.simulate_next_frame(cx);
                window.render_frame(cx);
            }
        })
        .unwrap();
        click(cx, window, "synthesis-patch-tool");
        cx.update_window(window, |_, window, cx| {
            let value = |window: &Window| {
                window
                    .find(("synthesis-patch-transform-field", 0_usize))
                    .value()
                    .unwrap()
                    .parse::<f64>()
                    .unwrap()
            };
            window.render_frame(cx);
            assert_eq!(value(window), original.field_percent(CenterX));
            let surface = window.find("synthesis-geometry-surface").bounds();
            assert!(surface.size.width > px(185.));
            let margin = window.rem_size();
            let start = project_fitted_source(surface, margin, [0.5, 0.5]);
            let end = project_fitted_source(surface, margin, [0.6, 0.55]);
            window.dispatch_event(
                MouseDownEvent {
                    button: MouseButton::Left,
                    position: start,
                    click_count: 1,
                    ..Default::default()
                }
                .to_platform_input(),
                cx,
            );
            window.dispatch_event(
                MouseMoveEvent {
                    position: end,
                    pressed_button: Some(MouseButton::Left),
                    ..Default::default()
                }
                .to_platform_input(),
                cx,
            );
            window.render_frame(cx);
            assert!((value(window) - 60.).abs() < 0.2);
            assert_eq!(bounds(), original);
            window.press("escape", cx);
            window.dispatch_event(
                MouseUpEvent {
                    button: MouseButton::Left,
                    position: end,
                    click_count: 1,
                    ..Default::default()
                }
                .to_platform_input(),
                cx,
            );
            window.render_frame(cx);
            assert_eq!(value(window), original.field_percent(CenterX));
            assert_eq!(bounds(), original);
            assert!(!view.read(cx).is_dirty());

            window.drag(start, end, cx);
            assert!((bounds().field_percent(CenterX) - 60.).abs() < 0.2);
            window.press("secondary-z", cx);
            assert_eq!(bounds(), original);
        })
        .unwrap();

        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            let opener = window.find("synthesis-full-size");
            if !opener.visible() {
                let inspector = window.find("graph-inspector-scroll").bounds();
                let scroll = view.read(cx).inspector_scroll.clone();
                let mut offset = scroll.offset();
                offset.y += inspector.center().y - opener.bounds().center().y;
                scroll.set_offset(offset);
                window.refresh();
                window.render_frame(cx);
            }
            window.click("synthesis-full-size", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            let width = ("synthesis-full-patch-transform-field", 2_usize);
            window.click(width, cx);
            window.press("secondary-a", cx);
            window.press("2", cx);
            window.press("5", cx);
            window.press("enter", cx);
            window.render_frame(cx);
            assert!(window.find("synthesis-full-size-surface").visible());
            assert_eq!(window.find(width).value(), Some("25.00"));
            assert_eq!(bounds(), original);
        })
        .unwrap();
        close_full_size_source(cx, window, "ok");
        assert_eq!(bounds().field_percent(Width), 25.);
        click(cx, window, "graph-undo");
        assert_eq!(bounds(), original);
    }

    #[gpui_kit::test]
    fn patches_source_create_appends_primitives_inline_and_in_a_full_size_draft(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_reduce_motion(true);
            bind_keys(cx);
        });
        let node = new_graph_node("Synthesis").unwrap();
        let filter = node.filter().unwrap();
        let mut edit = filter.synthesis_edit().unwrap();
        edit.add_layer(SynthesisLayerKind::Patches);
        assert!(edit.set_image_source(0, red_photo_at(160, 80)));
        filter
            .edit_synthesis(edit, "Set up patch creation")
            .unwrap();
        let retained = || filter.synthesis_edit().unwrap();
        let original = retained().patch_knots(0, 0).unwrap().collect::<Vec<_>>();
        let counts = || retained().patch_counts(0).unwrap();
        assert_eq!(counts(), (1, 0));
        let (window, view) = open(cx, node);
        let selected = |cx: &mut TestAppContext| {
            cx.update(|cx| view.read(cx).synthesis.as_ref().unwrap().geometry)
        };
        let assert_shape = |index, knots, mode| {
            let edit = retained();
            assert_eq!(edit.patch_knots(0, index).unwrap().count(), knots);
            for knot in 0..knots {
                assert_eq!(edit.patch_knot_mode(0, index, knot), Some(mode));
            }
            assert_eq!(
                edit.patch_knots(0, 0).unwrap().collect::<Vec<_>>(),
                original
            );
        };

        assert_eq!(selected(cx), None);
        assert_eq!(ready(cx, &view).geometry, ImageGeometry::new(160, 80));
        click(cx, window, "synthesis-create-rectangle");
        assert_eq!(counts(), (2, 0));
        assert_shape(1, 4, 0);
        assert_eq!(
            retained().patch_knots(0, 1).unwrap().collect::<Vec<_>>(),
            [[0., 1.], [0., 0.], [1., 0.], [1., 1.]]
        );
        assert_eq!(
            selected(cx),
            Some(SynthesisGeometry::Patch {
                selection: 1,
                knot: 0
            })
        );
        click(cx, window, "graph-undo");
        assert_eq!(counts(), (1, 0));

        click(cx, window, "synthesis-full-size");
        click(cx, window, "synthesis-full-create-triangle");
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            let point = |window: &Window| {
                [0_usize, 1].map(|index| {
                    window
                        .find(("synthesis-full-size-field", index))
                        .value()
                        .unwrap()
                        .parse::<f32>()
                        .unwrap()
                })
            };
            let triangle_first = point(window);
            assert_ne!(triangle_first, original[0]);
            window.click("synthesis-full-prev-patch", cx);
            assert_source_numeric_fields(window, original[0]);
            window.click("synthesis-full-next-patch", cx);
            assert_source_numeric_fields(window, triangle_first);
            window.click("synthesis-full-next-knot", cx);
            window.click("synthesis-full-next-knot", cx);
            let triangle_last = point(window);
            assert_ne!(triangle_last, triangle_first);
            window.click("synthesis-full-next-knot", cx);
            assert_source_numeric_fields(window, triangle_last);
        })
        .unwrap();
        assert_eq!(counts(), (1, 0));
        close_full_size_source(cx, window, "cancel");
        assert_eq!(counts(), (1, 0));
        cx.update(|cx| assert!(!view.read(cx).is_dirty()));

        click(cx, window, "synthesis-full-size");
        click(cx, window, "synthesis-full-create-circle");
        click(cx, window, "synthesis-full-create-triangle");
        assert_eq!(counts(), (1, 0));
        close_full_size_source(cx, window, "ok");
        assert_eq!(counts(), (3, 0));
        assert_shape(1, 4, 1);
        assert_shape(2, 3, 0);
        assert_eq!(
            selected(cx),
            Some(SynthesisGeometry::Patch {
                selection: 2,
                knot: 0
            })
        );
        cx.update_window(window, |_, window, cx| window.press("secondary-z", cx))
            .unwrap();
        cx.run_until_parked();
        assert_eq!(counts(), (1, 0));
        assert_eq!(
            retained().patch_knots(0, 0).unwrap().collect::<Vec<_>>(),
            original
        );
        cx.update(|cx| assert!(!view.read(cx).is_dirty()));
    }

    #[gpui_kit::test]
    fn full_size_patches_uses_the_rectangular_source_and_stages_one_undo(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_reduce_motion(true);
            bind_keys(cx);
        });
        let node = new_graph_node("Synthesis").unwrap();
        let filter = node.filter().unwrap();
        let mut edit = filter.synthesis_edit().unwrap();
        edit.add_layer(SynthesisLayerKind::Patches);
        assert!(edit.set_image_source(0, red_photo()));
        filter
            .edit_synthesis(edit, "Set up full-size Patches")
            .unwrap();
        let point = || {
            filter
                .synthesis_edit()
                .unwrap()
                .patch_knots(0, 0)
                .unwrap()
                .next()
                .unwrap()
        };
        let before = point();
        let after = [0.18, 0.24];
        let doc = Gtx375GraphDocument::new(vec![Some(node)]).unwrap();
        let (window, view) = open_document_sized(cx, doc, size(px(1400.), px(807.)));

        edit_full_size_source(cx, window, before, after);
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            let last = window.find(("synthesis-full-size-field", 11_usize));
            assert!(last.visible());
            assert!(last.bounds().bottom() < window.within("dialog").find("ok").bounds().top());
        })
        .unwrap();
        assert_eq!(point(), before);
        close_full_size_source(cx, window, "cancel");
        assert_eq!(point(), before);
        cx.update(|cx| assert!(view.read(cx).synthesis.as_ref().unwrap().geometry.is_none()));
        edit_full_size_source(cx, window, before, after);
        close_full_size_source(cx, window, "ok");
        assert!(
            point()
                .into_iter()
                .zip(after)
                .all(|(actual, expected)| (actual - expected).abs() < 0.002)
        );
        cx.update_window(window, |_, window, cx| window.press("secondary-z", cx))
            .unwrap();
        cx.run_until_parked();
        assert_eq!(point(), before);
    }

    #[gpui_kit::test]
    fn full_size_repeat_preserves_the_same_grid_on_cancel_and_updates_the_inline_guide_on_apply(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_reduce_motion(true);
        });
        let node = new_graph_node("Synthesis").unwrap();
        let filter = node.filter().unwrap();
        let mut edit = filter.synthesis_edit().unwrap();
        edit.add_layer(SynthesisLayerKind::Repeating);
        assert!(edit.set_image_source(0, red_photo()));
        filter
            .edit_synthesis(edit, "Set up full-size Repeat")
            .unwrap();
        let point = || filter.synthesis_edit().unwrap().repeat_grid(0).unwrap().1[0];
        let before = point();
        let after = [0.25, 0.22];
        let (window, view) = open(cx, node);
        click(cx, window, "synthesis-grid-toggle");
        assert_eq!(ready(cx, &view).aspect, 2.);

        edit_full_size_source(cx, window, before, after);
        assert_eq!(point(), before);
        close_full_size_source(cx, window, "cancel");
        assert_eq!(point(), before);
        edit_full_size_source(cx, window, before, after);
        close_full_size_source(cx, window, "ok");
        assert!(
            point()
                .into_iter()
                .zip(after)
                .all(|(actual, expected)| (actual - expected).abs() < 0.002)
        );
        cx.update(|cx| {
            assert_eq!(
                view.read(cx).synthesis.as_ref().unwrap().geometry,
                Some(SynthesisGeometry::GridPoint(0))
            );
        });
        click(cx, window, "graph-undo");
        assert_eq!(point(), before);
    }

    #[gpui_kit::test]
    fn full_size_blend_crop_stages_pointer_edits_and_undo_restores_the_preview(
        cx: &mut TestAppContext,
    ) {
        use super::super::adjuster_ui_tests::choose;

        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_reduce_motion(true);
            bind_keys(cx);
        });
        let node = new_graph_node("Synthesis").unwrap();
        let filter = node.filter().unwrap();
        let mut edit = filter.synthesis_edit().unwrap();
        edit.add_layer(SynthesisLayerKind::Blend);
        let row: Vec<_> = (0..160_u8)
            .flat_map(|value| [value, 255 - value, value + value / 2, 255])
            .collect();
        assert!(edit.set_image_source(0, photo(160, 80, &row.repeat(80))));
        assert!(edit.set_crop_rectangle(0, [0.2, 0.2, 0.5, 0.5]));
        filter
            .edit_synthesis(edit, "Set up the Blend crop")
            .unwrap();
        let rectangle = || filter.synthesis_edit().unwrap().crop_rectangle(0).unwrap();
        let before = rectangle();
        let downstream = Node::invert();
        downstream.slot(0).unwrap().set_node(Some(&node)).unwrap();
        let (window, view) = open(cx, downstream.clone());
        click(cx, window, "graph-view-canvas");
        let id = cx.update(|cx| view.read(cx).canvas.card_id_for_test(&node).unwrap());
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            window.click(format!("graph-node-{id}"), cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| view.update(cx, |view, _| view.ensure_history_binding()));
        let preview = |cx: &mut TestAppContext| {
            cx.background_executor
                .advance_clock(std::time::Duration::from_millis(200));
            cx.run_until_parked();
            cx.update(|cx| {
                let view = view.read(cx);
                assert!(view.preview_error().is_none(), "{:?}", view.preview_error());
                view.raster_pixels().unwrap().to_vec()
            })
        };
        let history = |cx: &mut TestAppContext| {
            cx.update(|cx| view.read(cx).history().unwrap().edit_sequence())
        };
        let original = preview(cx);
        let card_pixels = |cx: &mut TestAppContext| {
            cx.update(|cx| {
                view.read(cx)
                    .preview_frame(&downstream)
                    .unwrap()
                    .as_bytes(0)
                    .unwrap()
                    .to_vec()
            })
        };
        let original_card = card_pixels(cx);
        let sequence = history(cx);
        assert_eq!(ready(cx, &view).geometry, ImageGeometry::new(160, 80));

        let from = [0.45, 0.45];
        let to = [0.6, 0.4];
        edit_full_size_source(cx, window, from, to);
        assert_eq!(rectangle(), before);
        let dragged = preview(cx);
        assert_ne!(dragged, original);
        assert_ne!(card_pixels(cx), original_card);
        assert_eq!(history(cx), sequence);
        cx.update(|cx| {
            let view = view.read(cx);
            assert!(
                view.draft_preview_thumbnail(downstream.occurrence_id())
                    .is_some()
            );
            assert!(!view.is_dirty());
        });
        let set_center_x = |cx: &mut TestAppContext, value: &str| {
            cx.update_window(window, |_, window, cx| {
                window.click(("synthesis-full-size-field", 0_usize), cx);
                window.press("secondary-a", cx);
                window.input(value, cx);
                window.press("enter", cx);
            })
            .unwrap();
            cx.run_until_parked();
        };
        set_center_x(cx, "70");
        assert_ne!(preview(cx), dragged);
        set_center_x(cx, "65");
        close_full_size_source(cx, window, "cancel");
        assert_eq!(rectangle(), before);
        assert_eq!(preview(cx), original);
        assert_eq!(card_pixels(cx), original_card);
        assert_eq!(history(cx), sequence);
        cx.update(|cx| {
            assert!(
                view.read(cx)
                    .draft_preview_thumbnail(downstream.occurrence_id())
                    .is_none()
            )
        });

        edit_full_size_source(cx, window, from, to);
        close_full_size_source(cx, window, "ok");
        assert!(
            rectangle()
                .into_iter()
                .zip([0.35, 0.15, 0.5, 0.5])
                .all(|(actual, expected)| (actual - expected).abs() < 0.002)
        );
        assert_eq!(preview(cx), dragged);
        cx.update_window(window, |_, window, cx| window.press("secondary-z", cx))
            .unwrap();
        assert_eq!(preview(cx), original);
        assert_eq!(rectangle(), before);

        choose(cx, window, "graph-preview-target", "Texture output");
        let pinned = preview(cx);
        assert_ne!(pinned, original);
        let before_pinned_edit = history(cx);
        edit_full_size_source(cx, window, from, to);
        let pinned_draft = preview(cx);
        assert_ne!(pinned_draft, pinned);
        assert_ne!(pinned_draft, dragged);
        close_full_size_source(cx, window, "cancel");
        assert_eq!(preview(cx), pinned);
        assert_eq!(history(cx), before_pinned_edit);

        click(cx, window, "synthesis-full-size");
        cx.update_window(window, |_, window, cx| {
            let width = ("synthesis-full-size-field", 2_usize);
            let height = ("synthesis-full-size-field", 3_usize);
            window.click(width, cx);
            window.press("secondary-a", cx);
            window.input("-10", cx);
            window.press("enter", cx);
            let committed = window.find(width);
            let value: f32 = committed.value().unwrap().parse().unwrap();
            assert!((value - 0.01).abs() < 0.00001);
            assert_eq!(committed.focused(), Some(true));
            window.press("tab", cx);
            assert_eq!(window.find(height).focused(), Some(true));
        })
        .unwrap();
        assert_eq!(rectangle(), before);
        close_full_size_source(cx, window, "ok");
        let [x, _, width, _] = rectangle();
        assert!((width - 0.0001).abs() < 0.000001);
        assert!((x + width / 2. - 0.45).abs() < 0.00001);
    }

    #[gpui_kit::test]
    fn patch_guide_reuses_the_embedded_source_and_updates_for_the_graph(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let synthesis = new_graph_node("Synthesis").unwrap();
        let input = graph_color();
        synthesis.slot(0).unwrap().set_node(Some(&input)).unwrap();
        let filter = synthesis.filter().unwrap();
        let mut edit = filter.synthesis_edit().unwrap();
        edit.add_layer(SynthesisLayerKind::Patches);
        assert!(edit.set_image_source(0, red_photo()));
        filter.edit_synthesis(edit, "Create fixture").unwrap();

        let (window, view) = open(cx, synthesis);
        click(cx, window, "synthesis-edit-patches");
        let original = ready(cx, &view);
        assert_eq!(original.aspect, 2.);
        assert_eq!(&original.image.as_bytes(0).unwrap()[..4], [0, 0, 255, 255]);
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            let area = window.find("synthesis-geometry-surface").bounds();
            let margin = window.rem_size();
            let height = (area.size.height - margin).min((area.size.width - margin) / 2.);
            let center = area.center();
            let at = |[x, y]: [f32; 2]| {
                center + gpui_kit::point((x - 0.5) * 2. * height, (y - 0.5) * height)
            };
            window.drag(at([0.3, 0.3]), at([0.45, 0.4]), cx);
        })
        .unwrap();
        cx.run_until_parked();
        let [x, y] = cx.update(|cx| {
            view.read(cx)
                .synthesis
                .as_ref()
                .unwrap()
                .edit
                .patch_knots(0, 0)
                .unwrap()
                .next()
                .unwrap()
        });
        assert!((x - 0.45).abs() < 0.01 && (y - 0.4).abs() < 0.01);
        click(cx, window, "synthesis-next-patch-point");
        assert!(Arc::ptr_eq(&ready(cx, &view).image, &original.image));
        click(cx, window, "synthesis-input-source");
        let graph = ready(cx, &view);
        assert_eq!(graph.aspect, 1.);
        assert_eq!(&graph.image.as_bytes(0).unwrap()[..4], [0, 255, 0, 255]);
        cx.update(|cx| {
            let view = view.read(cx);
            let GuidePreparation::Available(Some(State::Ready(full))) =
                view.prepare_full_size_synthesis_source(view.synthesis.as_ref().unwrap(), 0)
            else {
                panic!("a current graph raster should also supply its full-size guide");
            };
            assert_eq!(full.geometry, graph.geometry);
            assert!(Arc::ptr_eq(&full.image, &graph.image));
        });
        click(cx, window, "graph-undo");
        assert_eq!(
            &ready(cx, &view).image.as_bytes(0).unwrap()[..4],
            [0, 0, 255, 255]
        );
        click(cx, window, "graph-redo");
        assert_eq!(
            &ready(cx, &view).image.as_bytes(0).unwrap()[..4],
            [0, 255, 0, 255]
        );

        input.set_solid_color([0., 0., 1.]).unwrap();
        cx.update(|cx| view.update(cx, |view, cx| view.start_preview(cx)));
        cx.run_until_parked();
        let updated = ready(cx, &view);
        assert_eq!(&updated.image.as_bytes(0).unwrap()[..4], [255, 0, 0, 255]);
        click(cx, window, "synthesis-hide-geometry");
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("synthesis-geometry-surface").is_none());
        })
        .unwrap();
        assert!(Arc::ptr_eq(&ready(cx, &view).image, &updated.image));
        click(cx, window, "synthesis-edit-patches");
        assert!(Arc::ptr_eq(&ready(cx, &view).image, &updated.image));

        input.set_solid_color([1., 0., 0.]).unwrap();
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.start_preview(cx);
                let state = &view
                    .synthesis
                    .as_ref()
                    .unwrap()
                    .guide_source
                    .as_ref()
                    .unwrap()
                    .state;
                let State::Loading(request) = state else {
                    panic!("an upstream revision starts a new guide request");
                };
                let ticket = request.0.clone();
                view.retire();
                assert!(!ticket.is_current());
                assert!(view.synthesis.is_none());
            });
        });
        cx.run_until_parked();
    }

    #[gpui_kit::test]
    fn patch_input_authoring_uses_raw_guide_geometry_only_for_the_current_source(
        cx: &mut TestAppContext,
    ) {
        use mycela_engine::SynthesisNumberField::PatchFeather;

        cx.update(gpui_kit::init);
        let node = new_graph_node("Synthesis").unwrap();
        let input = graph_color();
        node.slot(0).unwrap().set_node(Some(&input)).unwrap();
        let filter = node.filter().unwrap();
        let mut edit = filter.synthesis_edit().unwrap();
        edit.add_layer(SynthesisLayerKind::Patches);
        let patch = SynthesisGeometry::Patch {
            selection: 0,
            knot: 0,
        };
        assert!(edit.move_geometry_point(0, patch, [0.05, 0.25]));
        let fields = edit.geometry_fields(0, patch);
        let feather = fields
            .iter()
            .position(|field| field.number_field() == Some(PatchFeather(0)))
            .unwrap();
        let mut requested: Vec<_> = fields.into_iter().map(|field| field.value()).collect();
        requested[feather] = 100.;
        let expected = |geometry| {
            let mut candidate = edit.clone();
            candidate
                .set_geometry_fields_with_input_geometry(0, patch, &requested, Some(geometry))
                .unwrap();
            candidate.number_value(Some(0), PatchFeather(0)).unwrap()
        };
        let raw = ImageGeometry::new(43, 17);
        let display = ImageGeometry::new(LIVE_PREVIEW_SIZE, LIVE_PREVIEW_SIZE);
        let expected_value = expected(raw);
        assert_ne!(expected_value, expected(display));
        assert_ne!(expected_value, expected(ImageGeometry::new(17, 43)));
        assert!(expected_value > 0. && expected_value < 100.);
        filter
            .edit_synthesis(edit, "Create connected patch fixture")
            .unwrap();

        let (window, view) = open(cx, node.clone());
        click(cx, window, "synthesis-edit-patches");
        assert_eq!(ready(cx, &view).geometry, display);
        cx.update(|cx| {
            view.update(cx, |view, _| {
                let panel = view.synthesis.as_mut().unwrap();
                let State::Ready(raster) = &mut panel.guide_source.as_mut().unwrap().state else {
                    panic!("The guide render should be ready");
                };
                // Exercise an upstream raster whose geometry differs from the fitted bitmap.
                raster.geometry = raw;
                raster.aspect = raw.width() as f32 / raw.height() as f32;
                let panel = view.synthesis.as_ref().unwrap();
                assert_eq!(view.synthesis_patch_input_geometry(panel), Some(raw));
            })
        });

        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            window.click(("synthesis-geometry-control", feather), cx);
            window.press("secondary-a", cx);
            window.input("100", cx);
        })
        .unwrap();
        cx.run_until_parked();
        let committed = || {
            filter
                .synthesis_edit()
                .unwrap()
                .number_value(Some(0), PatchFeather(0))
        };
        assert_eq!(committed(), Some(expected_value));

        input.set_solid_color([0., 0., 1.]).unwrap();
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                assert_eq!(
                    view.synthesis_patch_input_geometry(view.synthesis.as_ref().unwrap()),
                    None
                );
                view.start_preview(cx);
            })
        });
        cx.run_until_parked();
        assert_eq!(ready(cx, &view).geometry, display);
        assert_eq!(committed(), Some(expected_value));

        let frame = model::freeze(&node, 0.).unwrap().materialize().unwrap();
        for size in [ImageGeometry::new(96, 64), ImageGeometry::new(64, 96)] {
            let rendered = crate::scene_backend::render_export(
                &frame,
                size,
                &|| false,
                crate::scene_backend::GRAPH_PNG_OPTIONS,
                None,
                None,
                None,
            )
            .unwrap();
            assert_eq!(rendered.geometry(), size);
            assert_eq!(committed(), Some(expected_value));
        }
    }

    #[gpui_kit::test]
    fn shared_patch_lab_edit_uses_a_current_guide_and_an_unselected_source_raster(
        cx: &mut TestAppContext,
    ) {
        use super::super::adjuster_ui_tests::choose;
        use mycela_engine::{LabNumericLink, SynthesisNumberField::PatchFeather};

        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_reduce_motion(true);
        });
        let patch = SynthesisGeometry::Patch {
            selection: 0,
            knot: 0,
        };
        let field = PatchFeather(0);
        let source_sizes = [
            ImageGeometry::new(43, 17),
            ImageGeometry::new(LIVE_PREVIEW_SIZE, LIVE_PREVIEW_SIZE),
        ];
        let create = |source_geometry| {
            let node = new_graph_node("Synthesis").unwrap();
            node.slot(0)
                .unwrap()
                .set_node(Some(&graph_color()))
                .unwrap();
            let filter = node.filter().unwrap();
            let mut edit = filter.synthesis_edit().unwrap();
            edit.add_layer(SynthesisLayerKind::Patches);
            assert!(edit.move_geometry_point(0, patch, [0.05, 0.25]));
            filter
                .edit_synthesis(edit.clone(), "Create linked Patches fixture")
                .unwrap();
            let requested = edit
                .geometry_fields(0, patch)
                .into_iter()
                .map(|item| {
                    if item.number_field() == Some(field) {
                        30.
                    } else {
                        item.value()
                    }
                })
                .collect::<Vec<_>>();
            edit.set_geometry_fields_with_input_geometry(
                0,
                patch,
                &requested,
                Some(source_geometry),
            )
            .unwrap();
            (node, filter, edit.number_value(Some(0), field).unwrap())
        };
        let [
            (first, first_filter, first_expected),
            (second, second_filter, second_expected),
        ] = source_sizes.map(create);
        assert_ne!(first_expected, second_expected);
        let mut document =
            Gtx375GraphDocument::new(vec![Some(first.clone()), Some(second.clone())]).unwrap();
        assert!(
            document
                .set_synthesis_number_adjuster(
                    &first,
                    &first_filter.synthesis_edit().unwrap(),
                    Some(0),
                    field,
                    LabNumericLink::Create("Shared feather".into())
                )
                .unwrap()
        );
        let control = document.numeric_lab_adjusters(None).remove(0);
        assert!(
            document
                .set_synthesis_number_adjuster(
                    &second,
                    &second_filter.synthesis_edit().unwrap(),
                    Some(0),
                    field,
                    LabNumericLink::Existing(control)
                )
                .unwrap()
        );

        let (window, view) = open_document(cx, document);
        click(cx, window, "synthesis-edit-patches");
        assert_eq!(ready(cx, &view).geometry, source_sizes[1]);
        cx.update(|cx| {
            view.update(cx, |view, _| {
                let panel = view.synthesis.as_mut().unwrap();
                let State::Ready(raster) = &mut panel.guide_source.as_mut().unwrap().state else {
                    panic!("the connected guide should be ready");
                };
                raster.geometry = source_sizes[0];
            })
        });

        choose(cx, window, "graph-lab-adjusters", "Shared feather…");
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("lab-adjuster-patch-input-status").is_none());
            let mut dialog = window.within("dialog");
            dialog.click("lab-adjuster-number", cx);
            assert_eq!(dialog.find("lab-adjuster-number").focused(), Some(true));
            dialog.press("secondary-a", cx);
            dialog.input("100", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("lab-adjuster-number").value(), Some("100"));
            let mut apply = window.within("dialog-action");
            assert!(apply.find("ok").visible());
            apply.click("ok", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("lab-adjuster-number").is_none());
        })
        .unwrap();
        let read = || {
            [&first_filter, &second_filter].map(|filter| {
                filter
                    .synthesis_edit()
                    .unwrap()
                    .number_value(Some(0), field)
                    .unwrap()
            })
        };
        assert_eq!(read(), [first_expected, second_expected]);
        click(cx, window, "graph-undo");
        assert_eq!(read(), [0.; 2]);
    }

    #[gpui_kit::test]
    fn repeating_pattern_grid_tracks_the_fitted_source(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let node = new_graph_node("Synthesis").unwrap();
        node.slot(0)
            .unwrap()
            .set_node(Some(&graph_color()))
            .unwrap();
        let filter = node.filter().unwrap();
        let mut edit = filter.synthesis_edit().unwrap();
        edit.add_layer(SynthesisLayerKind::Repeating);
        assert!(edit.set_image_source(0, red_photo()));
        filter.edit_synthesis(edit, "Create fixture").unwrap();

        let (window, view) = open(cx, node);
        click(cx, window, "synthesis-grid-toggle");
        let embedded = ready(cx, &view);
        assert_eq!(embedded.aspect, 2.);
        assert_eq!(&embedded.image.as_bytes(0).unwrap()[..4], [0, 0, 255, 255]);
        cx.update_window(window, |_, window, cx| {
            const SURFACE: &str = "synthesis-geometry-surface";
            window.render_frame(cx);
            let bounds = window.find(SURFACE).bounds();
            let margin = window.rem_size();
            let height = (bounds.size.height - margin).min((bounds.size.width - margin) / 2.);
            let at = |[x, y]: [f32; 2]| {
                bounds.center() + gpui_kit::point((x - 0.5) * 2. * height, (y - 0.5) * height)
            };
            let first = at([0.15, 0.15]);
            let neighbor = at([0.5, 0.15]);
            let geometry = |cx: &App| view.read(cx).synthesis.as_ref().unwrap().geometry;
            window.click_at(SURFACE, first - bounds.origin, cx);
            assert_eq!(geometry(cx), Some(SynthesisGeometry::GridPoint(0)));
            assert_eq!(window.find(SURFACE).bounds(), bounds);
            shift_click_source_point(window, cx, neighbor);
            assert_eq!(geometry(cx), Some(SynthesisGeometry::Grid));
            assert_eq!(window.find(SURFACE).bounds(), bounds);
            shift_click_source_point(window, cx, neighbor);
            assert_eq!(geometry(cx), Some(SynthesisGeometry::GridPoint(0)));
            window.drag(first, at([0.25, 0.3]), cx);
        })
        .unwrap();
        cx.run_until_parked();

        let [x, y] = cx.update(|cx| {
            view.read(cx)
                .synthesis
                .as_ref()
                .unwrap()
                .edit
                .repeat_grid(0)
                .unwrap()
                .1[0]
        });
        assert!((x - 0.25).abs() < 0.01 && (y - 0.3).abs() < 0.01);
        assert!(Arc::ptr_eq(&ready(cx, &view).image, &embedded.image));
        click(cx, window, "synthesis-input-source");
        let input = ready(cx, &view);
        assert_eq!(input.aspect, 1.);
        assert_eq!(&input.image.as_bytes(0).unwrap()[..4], [0, 255, 0, 255]);
        click(cx, window, "graph-undo");
        assert_eq!(
            &ready(cx, &view).image.as_bytes(0).unwrap()[..4],
            [0, 0, 255, 255]
        );
        click(cx, window, "synthesis-grid-toggle");
        cx.update(|cx| {
            assert!(
                view.read(cx)
                    .synthesis
                    .as_ref()
                    .unwrap()
                    .guide_source
                    .is_none()
            )
        });
    }

    fn attach_synthesis(mut style: StyleEdit) -> StyleEdit {
        assert!(style.set_paint(0, StylePaintKind::Synthesis).unwrap());
        let mut edit = style.synthesis_edit(0).unwrap();
        edit.add_layer(SynthesisLayerKind::Patches);
        assert!(style.set_synthesis(0, edit).unwrap());
        style
    }

    #[gpui_kit::test]
    fn embedded_style_repeat_source_previews_draft_without_recording_history(
        cx: &mut TestAppContext,
    ) {
        use super::super::adjuster_ui_tests::choose;

        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_reduce_motion(true);
            bind_keys(cx);
        });
        let node = new_graph_node("Style").unwrap();
        let filter = node.filter().unwrap();
        let mut style = filter.style_edit().unwrap();
        assert!(style.set_paint(0, StylePaintKind::Synthesis).unwrap());
        let mut texture = style.synthesis_edit(0).unwrap();
        texture.add_layer(SynthesisLayerKind::Repeating);
        let pixels: Vec<_> = (0..80_u8)
            .flat_map(|y| (0..160_u8).flat_map(move |x| [x, y * 3, x + y, 255]))
            .collect();
        assert!(texture.set_image_source(0, photo(160, 80, &pixels)));
        assert!(style.set_synthesis(0, texture).unwrap());
        filter.edit_style(style, "Set up Style Repeat").unwrap();

        let retained_grid = || {
            filter
                .style_edit()
                .unwrap()
                .synthesis_edit(0)
                .unwrap()
                .repeat_grid(0)
                .unwrap()
                .1
                .to_vec()
        };
        let before = retained_grid();
        let to = [0.29, 0.25];
        let (window, view) = open(cx, node);
        cx.update(|cx| view.update(cx, |view, _| view.ensure_history_binding()));
        choose(cx, window, "graph-preview-target", "Current Layer");
        let preview = |cx: &mut TestAppContext| {
            cx.background_executor
                .advance_clock(std::time::Duration::from_millis(200));
            cx.run_until_parked();
            cx.update(|cx| {
                let view = view.read(cx);
                assert_eq!(
                    view.synthesis_preview_mode(),
                    Some(super::super::PreviewMode::Current)
                );
                assert!(view.preview_error().is_none(), "{:?}", view.preview_error());
                view.raster_pixels().unwrap().to_vec()
            })
        };
        let history = |cx: &mut TestAppContext| {
            cx.update(|cx| {
                let view = view.read(cx);
                let history = view.history().unwrap();
                (history.cursor(), history.edit_sequence(), view.is_dirty())
            })
        };
        let committed = preview(cx);
        let saved = history(cx);
        assert!(!saved.2);

        edit_full_size_source(cx, window, before[0], to);
        let live = preview(cx);
        assert_ne!(
            live, committed,
            "The open Repeat draft must update the dock"
        );
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            let preview = window.find("synthesis-full-texture-preview");
            assert!(preview.visible());
            assert_eq!(preview.label(), Some("Texture preview · Current Layer"));
        })
        .unwrap();
        assert_eq!(retained_grid(), before);
        assert_eq!(history(cx), saved);
        close_full_size_source(cx, window, "cancel");
        assert_eq!(preview(cx), committed);
        assert_eq!(retained_grid(), before);
        assert_eq!(history(cx), saved);

        edit_full_size_source(cx, window, before[0], to);
        assert_eq!(preview(cx), live);
        assert_eq!(history(cx), saved);
        close_full_size_source(cx, window, "ok");
        let after = retained_grid();
        assert!(
            after[0]
                .into_iter()
                .zip(to)
                .all(|(actual, expected)| { (actual - expected).abs() < 0.002 })
        );
        assert_eq!(after[1..], before[1..]);
        assert_eq!(preview(cx), live);
        let (cursor, _, dirty) = history(cx);
        assert_eq!(cursor, saved.0 + 1);
        assert!(dirty);

        cx.update_window(window, |_, window, cx| window.press("secondary-z", cx))
            .unwrap();
        assert_eq!(preview(cx), committed);
        assert_eq!(retained_grid(), before);
        let (cursor, _, dirty) = history(cx);
        assert_eq!(cursor, saved.0);
        assert!(!dirty);
    }

    #[gpui_kit::test]
    fn nested_lab_material_owns_the_dock_until_done_then_restores_the_section_preview(
        cx: &mut TestAppContext,
    ) {
        use super::super::adjuster_ui_tests::choose;
        use mycela_engine::{
            SynthesisAdjusterLink, SynthesisColorTarget, VARIABLE_NUMERIC, VariableSpec,
        };

        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_reduce_motion(true);
            bind_keys(cx);
        });
        let lab = new_graph_node("Wizard Group").unwrap();
        let section = new_graph_node("Wizard Group").unwrap();
        let mut variable = VariableSpec::new(VARIABLE_NUMERIC);
        variable.set_name(Some(&"Amount".encode_utf16().collect::<Vec<_>>()));
        let section_filter = section.filter().unwrap();
        section_filter
            .parameter(3)
            .unwrap()
            .set_variable(variable)
            .unwrap();
        let section_color = Node::solid_color().unwrap();
        section_color.set_solid_color([0.1, 0.4, 0.8]).unwrap();
        section
            .content(0)
            .unwrap()
            .set_node(Some(&section_color))
            .unwrap();

        let style = new_graph_node("Style").unwrap();
        let filter = style.filter().unwrap();
        let mut paint = filter.style_edit().unwrap();
        assert!(paint.set_paint(0, StylePaintKind::Synthesis).unwrap());
        let mut texture = paint.synthesis_edit(0).unwrap();
        texture.add_layer(SynthesisLayerKind::Solid);
        assert!(texture.set_color_value(SynthesisColorTarget::Solid(0), [0.8, 0.2, 0.1]));
        assert!(paint.set_synthesis(0, texture).unwrap());
        filter.edit_style(paint, "Set up nested Material").unwrap();
        section_filter
            .append_wizard_content([20., 20.])
            .unwrap()
            .set_node(Some(&style))
            .unwrap();
        lab.content(0).unwrap().set_node(Some(&section)).unwrap();

        let mut document = Gtx375GraphDocument::new(vec![Some(lab)]).unwrap();
        assert!(
            document
                .set_style_synthesis_adjuster(
                    &style,
                    None,
                    &filter.style_edit().unwrap(),
                    0,
                    SynthesisAdjusterLink::Create("Nested paint".into()),
                )
                .unwrap()
        );
        let material = document
            .lab_adjuster_metadata(None)
            .into_iter()
            .find(|entry| entry.is_material())
            .unwrap();
        let (window, view) = open_document(cx, document);
        cx.update(|cx| view.update(cx, |view, _| view.ensure_history_binding()));
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            window.click(("graph-lab-section-row", section.occurrence_id()), cx);
        })
        .unwrap();
        click(cx, window, "graph-preview-lab-section");

        let read = |cx: &mut TestAppContext| {
            cx.run_until_parked();
            cx.update(|cx| {
                let view = view.read(cx);
                assert!(view.preview_lab_section);
                assert!(
                    view.lab_sections
                        .selected()
                        .unwrap()
                        .same_reference(&section)
                );
                assert!(view.preview_error().is_none(), "{:?}", view.preview_error());
                assert!(!view.is_dirty());
                (
                    view.raster_pixels().unwrap().to_vec(),
                    view.history().unwrap().cursor(),
                )
            })
        };
        let section_preview = read(cx);
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("graph-preview-target").is_none());
            let action = ("graph-lab-edit-material", material.identity());
            assert_eq!(window.find(action).label(), Some("Edit Nested paint…"));
            window.click(action, cx);
        })
        .unwrap();
        let whole = read(cx);
        assert_ne!(whole.0, section_preview.0);
        assert_eq!(whole.1, section_preview.1);
        choose(cx, window, "graph-preview-target", "Current Layer");
        cx.update(|cx| {
            assert_eq!(
                view.read(cx).synthesis_preview_mode(),
                Some(super::super::PreviewMode::Current)
            )
        });
        let current = read(cx);
        assert_ne!(current.0, section_preview.0);
        assert_eq!(current.1, section_preview.1);
        choose(cx, window, "graph-preview-target", "Whole Texture");
        assert_eq!(read(cx), whole);

        click(cx, window, "synthesis-done-material");
        assert_eq!(read(cx), section_preview);
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("graph-preview-target").is_none());
            assert_eq!(
                window.find("graph-preview-lab-section").label(),
                Some("Show whole branch")
            );
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn nested_patch_guides_read_the_style_and_canvas_inputs(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        for is_canvas in [false, true] {
            let node = new_graph_node(if is_canvas { "Canvas" } else { "Style" }).unwrap();
            let filter = node.filter().unwrap();
            let input = graph_color();
            if is_canvas {
                filter.parameter(0).unwrap().set_number(1.).unwrap();
                node.slot(0).unwrap().set_node(Some(&input)).unwrap();
                filter
                    .edit_add_canvas_shape(CanvasPrimitive::Rectangle, Some(0))
                    .unwrap();
                let layer = filter.canvas_layer_editors().remove(0);
                layer
                    .edit_style(
                        attach_synthesis(layer.style_edit().unwrap()),
                        "Create fixture",
                    )
                    .unwrap();
            } else {
                node.slot(2).unwrap().set_node(Some(&input)).unwrap();
                filter
                    .edit_style(
                        attach_synthesis(filter.style_edit().unwrap()),
                        "Create fixture",
                    )
                    .unwrap();
            }
            let (window, view) = open(cx, node);
            if is_canvas {
                click(cx, window, "canvas-style-expanded");
            }
            click(cx, window, "synthesis-edit-patches");
            assert_eq!(
                &ready(cx, &view).image.as_bytes(0).unwrap()[..4],
                [0, 255, 0, 255],
                "Canvas: {is_canvas}",
            );
        }
    }
}
