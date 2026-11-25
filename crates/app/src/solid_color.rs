#[path = "export_settings.rs"]
pub(crate) mod export_settings;
#[path = "generator_preview.rs"]
mod generator_preview;
#[path = "imported_image.rs"]
mod imported_image;
#[path = "map_preview.rs"]
pub(crate) mod map_preview;
#[path = "polygon_source.rs"]
mod polygon_source;
#[path = "preview_export.rs"]
pub(crate) mod preview_export;
#[path = "scene_camera_view.rs"]
mod scene_camera_view;
#[path = "scene_source.rs"]
mod scene_source;
#[path = "solid_animation_editor.rs"]
mod solid_animation_editor;
use crate::LIVE_PREVIEW_SIZE;
use crate::color_binding_effects::{BindingJob, BindingRuntime};
use crate::filter_stack::FilterStackView;
use crate::gradient_source::{GradientSourceView, TextureSourceKind};
use crate::gtx_import::{GtxImport, PreparedGtx};
use crate::live_invert::LiveInvert;
use gpui_kit::base::{Disableable, Selectable, StyledExt, TestSupportExt};
use gpui_kit::component::{
    ActiveTheme, Sizable, WindowExt,
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
    dialog::DialogButtonProps,
    form::{Field, Form},
    input::{Input, InputState},
    menu::{DropdownMenu, PopupMenuItem},
    scroll::ScrollableElement,
    tooltip::Tooltip,
};
use gpui_kit::{
    AnyWindowHandle, App, AppContext, Context, Entity, FocusHandle, Focusable, IntoElement, Render,
    RenderImage, Subscription, Task, WeakEntity, Window, div, img, prelude::*,
};
use map_preview::{PreparedPreview, PreviewSelection};
use mycela_engine::{
    ContextColorValue, PreviewBitmap, SharedColorBinding, SharedColorInput, SharedInvertNode,
    SharedSolidColorNode, SolidColorCloneMode,
};
use polygon_source::PolygonSourceView;
use scene_camera_view::SceneCameraView;
use scene_source::SceneSourceView;
use solid_animation_editor::SolidAnimationEditor;
use std::{
    fmt,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

gpui_kit::actions!(
    mycela,
    [
        ApplyColor,
        RevertColor,
        OpenGtx,
        OpenImage,
        ExportPreviewPng
    ]
);
#[derive(Clone, Debug)]
pub enum ForegroundCommand {
    ApplyDraft,
    RevertDraft,
    SetValues { rgb: [f64; 3], transparency: f64 },
    ApplyValues { rgb: [f64; 3], transparency: f64 },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ForegroundError {
    OwnerClosed,
    WindowClosed,
    TextureReplaced,
    Operation(String),
}
impl fmt::Display for ForegroundError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OwnerClosed => f.write_str("The color editor is closed."),
            Self::WindowClosed => f.write_str("The window is closed."),
            Self::TextureReplaced => {
                f.write_str("The texture changed before this edit could be applied.")
            }
            Self::Operation(e) => f.write_str(e),
        }
    }
}
impl std::error::Error for ForegroundError {}
/// Actual GPUI entity/window route. Queued work never retains a closed view,
/// and missing owners are errors rather than a successful ready-host marker.
#[derive(Clone)]
pub struct ForegroundDispatcher {
    owner: WeakEntity<SolidColorView>,
    window: AnyWindowHandle,
    generation: Arc<AtomicU64>,
}
impl ForegroundDispatcher {
    pub fn enqueue(
        &self,
        command: ForegroundCommand,
        cx: &App,
    ) -> Task<Result<(), ForegroundError>> {
        let command = if matches!(command, ForegroundCommand::ApplyDraft) {
            match self
                .owner
                .read_with(cx, |view, cx| view.requested_values(cx))
            {
                Ok(Ok((rgb, transparency))) => ForegroundCommand::ApplyValues { rgb, transparency },
                Ok(Err(error)) => return Task::ready(Err(error)),
                Err(_) => return Task::ready(Err(ForegroundError::OwnerClosed)),
            }
        } else {
            command
        };
        let generation = self.generation.load(Ordering::Acquire);
        let owner = self.owner.clone();
        let window = self.window;
        cx.spawn(async move |cx| {
            cx.update_window(window, |_, window, cx| {
                owner.update(cx, |view, cx| {
                    if view.generation.load(Ordering::Acquire) != generation {
                        return Err(ForegroundError::TextureReplaced);
                    }
                    view.execute(command, window, cx)
                })
            })
            .map_err(|_| ForegroundError::WindowClosed)?
            .map_err(|_| ForegroundError::OwnerClosed)?
        })
    }
}
/// Retains the actual source authorities and the selected source identity.
/// InputState holds only editable text; bitmap and RenderImage are derived
/// presentation snapshots of the active source and the real effect pipeline.
pub struct SolidColorView {
    node: SharedSolidColorNode<ForegroundError>,
    source: TextureSourceKind,
    sample_time: f64,
    solid_animation_editor: Option<SolidAnimationEditor>,
    solid_animation_document: Option<crate::graph_document::LiveDocument>,
    _filter_time: Subscription,
    gradient: Option<Entity<GradientSourceView>>,
    procedural: std::collections::BTreeMap<TextureSourceKind, Entity<GradientSourceView>>,
    scenes: std::collections::BTreeMap<TextureSourceKind, Entity<SceneSourceView>>,
    polygon: Option<Entity<PolygonSourceView>>,
    polygon_time: Option<Subscription>,
    scene_camera: Entity<SceneCameraView>,
    generator_preview: generator_preview::GeneratorPreview,
    imported: Option<GtxImport>,
    image_import: imported_image::ImageImport,
    opening: bool,
    preview_export: preview_export::PreviewExport,
    generation: Arc<AtomicU64>,
    invert: Option<LiveInvert>,
    filters: Entity<FilterStackView>,
    show_filters: bool,
    binding: SharedColorBinding<ForegroundError>,
    runtime: Arc<BindingRuntime>,
    _binding_task: Task<()>,
    _window_close: Subscription,
    fields: [Entity<InputState>; 4],
    focus: FocusHandle,
    dispatcher: ForegroundDispatcher,
    applied_image: Option<mycela_engine::SharedImageMap>,
    preview_selection: PreviewSelection,
    preview_selection_error: Option<String>,
    pipeline_error: Option<String>,
    bitmap: Option<PreviewBitmap>,
    raster: Option<Arc<RenderImage>>,
    error: Option<String>,
    pending: bool,
    synced_text: [String; 4],
    applied_text: Option<[String; 4]>,
}
impl SolidColorView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let node = SharedSolidColorNode::new_unattached(SolidColorCloneMode::Original)
            .expect("characterized default node");
        let fields =
            std::array::from_fn(|_| cx.new(|cx| InputState::new(window, cx).default_value("0")));
        let (runtime, receiver) = BindingRuntime::new(cx.background_executor().clone());
        let filters = cx.new(|cx| FilterStackView::new(&runtime, cx));
        let scene_camera = cx.new(|cx| SceneCameraView::new(&runtime, window, cx));
        let handle = window.window_handle();
        let _filter_time = cx.subscribe(
            &filters,
            move |view, _, event: &crate::spatial_filter::SampleTime, cx| {
                let time = event.value;
                let activation = event.activation.clone();
                let source = view.source;
                let epoch = view.generation.load(Ordering::Acquire);
                cx.spawn(async move |owner, cx| {
                    let _ = cx.update_window(handle, |_, window, cx| {
                        let _ = owner.update(cx, |view, cx| {
                            if view.source == source
                                && view.generation.load(Ordering::Acquire) == epoch
                                && activation
                                    .upgrade()
                                    .is_some_and(|active| active.load(Ordering::Acquire))
                            {
                                view.set_sample_time(time, window, cx);
                            }
                        });
                    });
                })
                .detach();
            },
        );
        let input = SharedColorInput::new_uncreated();
        let binding =
            SharedColorBinding::new(&input, runtime.endpoint()).expect("live application effects");
        binding
            .set_property(Some(&node.color_property()))
            .expect("actual default Color binding");
        let weak_runtime = Arc::downgrade(&runtime);
        let handle = window.window_handle();
        let close_runtime = Arc::downgrade(&runtime);
        let close_binding = binding.clone();
        let window_id = handle.window_id();
        let close_view = cx.weak_entity();
        let _window_close = cx.on_window_closed(move |cx, id| {
            if id == window_id {
                let _ = close_view.update(cx, |view, cx| {
                    view.close_solid_animation(cx);
                    if let Some(gradient) = &view.gradient {
                        gradient.update(cx, |view, _| view.close());
                    }
                    for generator in view.procedural.values() {
                        generator.update(cx, |view, _| view.close());
                    }
                    for scene in view.scenes.values() {
                        scene.update(cx, |view, _| view.close());
                    }
                    if let Some(polygon) = &view.polygon {
                        polygon.update(cx, |view, _| view.close());
                    }
                    view.scene_camera.update(cx, |view, _| view.close());
                    view.image_import.retire();
                    view.generator_preview.retire();
                    view.filters.update(cx, |filters, cx| filters.close(cx));
                    view.invert.take();
                    view.opening = false;
                    view.preview_export.retire();
                    view.applied_image = None;
                    view.preview_selection = PreviewSelection::BaseColor;
                    view.preview_selection_error = None;
                });
                let result = close_binding.set_property(None);
                if let Some(runtime) = close_runtime.upgrade() {
                    if let Err(error) = result {
                        runtime.retain_failure(error);
                    }
                    runtime.window_closed();
                }
            }
        });
        let _binding_task = cx.spawn(async move |owner, cx| {
            while let Ok(job) = receiver.recv().await {
                if let BindingJob::EffectPresent(active) = &job {
                    if !active
                        .upgrade()
                        .is_some_and(|a| a.load(std::sync::atomic::Ordering::Acquire))
                    {
                        continue;
                    }
                }
                let input_committed = matches!(job, BindingJob::Tick(..));
                // A real window and retained entity must exist before engine
                // work. This acquires no mutable view borrow during callbacks.
                let execution_owner = match cx.update_window(handle, |_, _, _| {
                    owner.upgrade().ok_or(ForegroundError::OwnerClosed)
                }) {
                    Ok(Ok(owner)) => owner,
                    _ => {
                        if let Some(r) = weak_runtime.upgrade() {
                            r.shutdown();
                        }
                        while receiver.try_recv().is_ok() {}
                        break;
                    }
                };
                // An externally retained old RGB callback must not dispatch into
                // a different current source after its normal listener was retired.
                if matches!(job, BindingJob::Refresh(_) | BindingJob::Tick(..))
                    && !cx
                        .update_window(handle, |_, _, cx| {
                            execution_owner.read(cx).source == TextureSourceKind::SolidColor
                        })
                        .unwrap_or(false)
                {
                    continue;
                }
                let result = match job {
                    BindingJob::Refresh(frame) => frame.refresh_current(),
                    BindingJob::Tick(timer, generation, frame) => {
                        if weak_runtime
                            .upgrade()
                            .is_some_and(|r| r.is_current(timer, generation))
                        {
                            frame.timer_fired(timer)
                        } else {
                            continue;
                        }
                    }
                    BindingJob::Present | BindingJob::EffectPresent(_) => Ok(()),
                    BindingJob::Failure(error) => Err(error),
                };
                let delivered = cx.update_window(handle, |_, window, cx| {
                    execution_owner.update(cx, |view, cx| {
                        if let Err(error) = result {
                            view.error = Some(error.to_string());
                        }
                        if view.source == TextureSourceKind::SolidColor {
                            view.sync_unchanged_drafts(input_committed, window, cx);
                        } else if let Some(gradient) = view.active_generator() {
                            gradient.update(cx, |view, cx| view.sync(window, cx));
                        } else if let Some(polygon) = view.active_polygon() {
                            polygon.update(cx, |view, cx| view.sync(window, cx));
                        } else if let Some(scene) = view.active_scene() {
                            scene.update(cx, |view, cx| view.sync(window, cx));
                            view.scene_camera
                                .update(cx, |view, cx| view.sync(window, cx));
                        }
                        view.sync_filter_time(window, cx);
                        let _ = view.refresh_preview(cx);
                        if input_committed {
                            view.sync_solid_animation_values(cx);
                        }
                        cx.notify();
                    })
                });
                if delivered.is_err() {
                    if let Some(r) = weak_runtime.upgrade() {
                        r.shutdown();
                    }
                    while receiver.try_recv().is_ok() {}
                    break;
                }
            }
        });
        // Root's contextual keyboard navigation needs an initial focus path.
        // Set it once at construction, never during render or later refresh.
        let focus = cx.focus_handle().tab_stop(true);
        window.focus(&focus, cx);
        let generation = Arc::new(AtomicU64::new(0));
        let mut this = Self {
            node,
            source: TextureSourceKind::SolidColor,
            sample_time: 0.,
            solid_animation_editor: None,
            solid_animation_document: None,
            _filter_time,
            gradient: None,
            procedural: std::collections::BTreeMap::new(),
            scenes: std::collections::BTreeMap::new(),
            polygon: None,
            polygon_time: None,
            scene_camera,
            generator_preview: generator_preview::GeneratorPreview::default(),
            imported: None,
            image_import: imported_image::ImageImport::default(),
            opening: false,
            preview_export: preview_export::PreviewExport::default(),
            generation: Arc::clone(&generation),
            invert: None,
            filters,
            show_filters: false,
            binding,
            runtime,
            _binding_task,
            _window_close,
            fields,
            focus,
            dispatcher: ForegroundDispatcher {
                owner: cx.weak_entity(),
                window: window.window_handle(),
                generation,
            },
            applied_image: None,
            preview_selection: PreviewSelection::BaseColor,
            preview_selection_error: None,
            pipeline_error: None,
            bitmap: None,
            raster: None,
            error: None,
            pending: false,
            synced_text: std::array::from_fn(|_| "0".into()),
            applied_text: None,
        };
        let _ = this.refresh_preview(cx);
        this
    }
    /// Immutable historical import metadata, never the current editable values.
    pub fn imported_document(&self) -> Option<&mycela_engine::Gtx375WireDocument> {
        self.imported.as_ref().map(GtxImport::document)
    }
    /// Exact historical input bytes. No Save action projects current edits into them.
    pub fn imported_bytes(&self) -> Option<&[u8]> {
        self.imported.as_ref().map(GtxImport::original)
    }
    pub fn imported_path(&self) -> Option<&Path> {
        self.imported.as_ref().map(GtxImport::path)
    }
    pub fn active_source(&self) -> TextureSourceKind {
        self.source
    }
    pub fn generator_source(&self, kind: TextureSourceKind) -> Option<Entity<GradientSourceView>> {
        self.generator_view(kind)
    }
    pub fn gradient_source(&self) -> Option<Entity<GradientSourceView>> {
        self.gradient.clone()
    }
    pub fn filter_stack(&self) -> Entity<FilterStackView> {
        self.filters.clone()
    }
    pub fn dispatcher(&self) -> ForegroundDispatcher {
        self.dispatcher.clone()
    }
    pub fn color_binding(&self) -> SharedColorBinding<ForegroundError> {
        self.binding.clone()
    }
    /// The retained Solid Color source, including while Gradient is active.
    /// `active_source` determines which authority supplies the current preview.
    pub fn node(&self) -> SharedSolidColorNode<ForegroundError> {
        self.node.clone()
    }
    /// The retained effect owns its actual source selection values. There is no
    /// second collection of checked flags in the view or rendered controls.
    pub fn invert(&self) -> Option<SharedInvertNode<ForegroundError>> {
        self.invert.as_ref().map(LiveInvert::model)
    }
    /// The actual applied pipeline result, including its retained named stores.
    /// Preview selection never removes channels or rewrites this authority.
    pub fn applied_image(&self) -> Option<mycela_engine::SharedImageMap> {
        self.applied_image.clone()
    }
    pub fn selected_preview_map(&self) -> Option<&[u16]> {
        self.preview_selection.name()
    }
    pub fn bitmap(&self) -> Option<PreviewBitmap> {
        self.bitmap.clone()
    }
    pub fn error(&self) -> Option<&str> {
        self.pipeline_error.as_deref().or(self.error.as_deref())
    }
    pub fn raster_pixels(&self) -> Option<&[u8]> {
        self.raster.as_ref().and_then(|image| image.as_bytes(0))
    }
    fn open_node_editor(&mut self, cx: &mut Context<Self>) {
        if !self.runtime.is_dialog_visible()
            && let Err(error) = crate::graph_document::open_node_editor_window(cx)
        {
            self.error = Some(error);
            cx.notify();
        }
    }
    fn open_gtx(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.opening || self.pending || self.runtime.is_dialog_visible() {
            return;
        }
        self.cancel_image_import(window, cx);
        self.filters
            .update(cx, |filters, cx| filters.retire_input_work(cx));
        self.opening = true;
        self.runtime.set_dialog_visible(true);
        cx.notify();
        let prompt = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Open GTX".into()),
        });
        let background = cx.background_executor().clone();
        let window_handle = self.dispatcher.window;
        let runtime = Arc::downgrade(&self.runtime);
        cx.spawn(async move |owner, cx| {
            let selected = prompt.await;
            if let Some(runtime) = runtime.upgrade() {
                runtime.set_dialog_visible(false);
            }
            // A late picker response must not read a file or update a retained
            // editor after its real window has physically closed.
            if !cx
                .update_window(window_handle, |_, _, cx| owner.read_with(cx, |_, _| ()))
                .is_ok_and(|value| value.is_ok())
            {
                return;
            }
            let result = match selected {
                Ok(Ok(Some(mut paths))) if paths.len() == 1 => {
                    let path = paths.remove(0);
                    background
                        .spawn(async move { PreparedGtx::read(path) })
                        .await
                        .map(Some)
                }
                Ok(Ok(None)) => Ok(None),
                Ok(Ok(Some(_))) => Err("Choose one GTX file.".into()),
                Ok(Err(error)) => Err(format!("Couldn’t open the file picker: {error}")),
                Err(_) => Err("The file picker closed before returning a result.".into()),
            };
            let _ = cx.update_window(window_handle, |_, window, cx| {
                owner.update(cx, |view, cx| {
                    view.opening = false;
                    match result {
                        Ok(Some(prepared)) => {
                            if let Err(error) = view.install_gtx(prepared, window, cx) {
                                view.error = Some(error.to_string());
                            }
                        }
                        Ok(None) => {} // Cancel preserves the current draft/error/document.
                        Err(error) => view.error = Some(error),
                    }
                    cx.notify();
                })
            });
        })
        .detach();
    }
    fn install_gtx(
        &mut self,
        prepared: PreparedGtx,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), ForegroundError> {
        let PreparedGtx {
            import,
            graph,
            node,
            preview,
        } = prepared;
        let previous =
            (self.source == TextureSourceKind::SolidColor).then(|| self.node.color_property());
        if let Err(error) = self.binding.set_property(Some(&node.color_property())) {
            // Bind only after decoding, eligibility and raster preparation.
            // No current view/model/metadata is published before this succeeds.
            if let Err(rollback) = self.binding.set_property(previous.as_ref()) {
                self.runtime.retain_failure(rollback);
            }
            return Err(error);
        }
        self.image_import.reset();
        self.runtime.retire_pending_input();
        self.preview_export.retire();
        self.generation.fetch_add(1, Ordering::AcqRel);
        if let Some(gradient) = self.gradient.take() {
            gradient.update(cx, |view, _| view.close());
        }
        self.generator_preview.retire();
        for (_, generator) in std::mem::take(&mut self.procedural) {
            generator.update(cx, |view, _| view.close());
        }
        for (_, scene) in std::mem::take(&mut self.scenes) {
            scene.update(cx, |view, _| view.close());
        }
        self.polygon_time.take();
        if let Some(polygon) = self.polygon.take() {
            polygon.update(cx, |view, _| view.close());
        }
        self.scene_camera
            .update(cx, |view, _| view.set_active(false));
        self.source = TextureSourceKind::SolidColor;
        self.sample_time = 0.;
        self.invert.take();
        self.filters.update(cx, |filters, cx| filters.clear(cx));
        self.show_filters = false;
        let name = import.name();
        self.close_solid_animation(cx);
        self.solid_animation_document = Some(graph);
        self.node = node;
        self.imported = Some(import);
        self.install_preview(preview, cx);
        self.pending = false;
        self.error = None;
        self.sync_drafts(window, cx);
        window.set_window_title(&format!("Mycela · {name}"));
        let focus = self.fields[0].read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        Ok(())
    }
    /// One workspace sample time delegates to the active retained source.
    pub fn sample_time(&self, cx: &App) -> f64 {
        if let Some(view) = self.active_generator() {
            view.read(cx).preview_time()
        } else if let Some(view) = self.active_scene() {
            view.read(cx).preview_time()
        } else {
            self.sample_time
        }
    }
    fn set_sample_time(&mut self, time: f64, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(generator) = self.active_generator() {
            generator.update(cx, |v, cx| v.set_preview_time(time, window, cx));
        } else if let Some(scene) = self.active_scene() {
            scene.update(cx, |v, cx| v.set_preview_time(time, window, cx));
        } else if let Some(polygon) = self.active_polygon() {
            self.sample_time = time;
            polygon.update(cx, |v, cx| v.set_preview_time(time, window, cx));
        } else {
            self.sample_time = time;
        }
        if self.source == TextureSourceKind::SolidColor {
            self.sync_solid_animation_time(cx);
        }
        self.filters
            .update(cx, |v, cx| v.set_sample_time(time, window, cx));
        let _ = self.refresh_preview(cx);
        cx.notify();
    }
    fn sync_filter_time(&self, window: &mut Window, cx: &mut App) {
        let time = self.sample_time(cx);
        self.filters
            .update(cx, |v, cx| v.set_sample_time(time, window, cx));
    }
    fn refresh_preview(&mut self, cx: &mut App) -> Result<(), ForegroundError> {
        self.filters
            .update(cx, |filters, cx| filters.refresh_controls(cx));
        // Old source timer presentations must not cancel an explicit source
        // switch. The candidate is owned; its completion uses the current live
        // effect pipeline, while explicit switches/import/close retire it.
        if self.image_import.is_busy() || self.generator_preview.is_switching() {
            return Ok(());
        }
        if self.source.requires_owned_preview() || self.filters.read(cx).uses_spatial() {
            let result = self.request_generator_preview(self.source, false, cx);
            if let Err(error) = &result {
                self.pipeline_error = Some(error.to_string());
            }
            return result;
        }
        self.generator_preview.retire();
        let prepared = (|| {
            let filters = self.filters.read(cx).pipeline();
            let image = match self.source {
                TextureSourceKind::SolidColor => self.solid_preview_image(self.sample_time)?,
                TextureSourceKind::Gradient => {
                    let source = self
                        .gradient
                        .as_ref()
                        .ok_or_else(|| {
                            ForegroundError::Operation("Gradient source is unavailable.".into())
                        })?
                        .read(cx);
                    source
                        .model()
                        .render_unbound(LIVE_PREVIEW_SIZE, LIVE_PREVIEW_SIZE, source.preview_time())
                        .map_err(|e| ForegroundError::Operation(e.to_string()))?
                }
                _ => {
                    return Err(ForegroundError::Operation(
                        "The generator requires an owned render job.".into(),
                    ));
                }
            };
            let preview = map_preview::render(
                image,
                self.invert.as_ref(),
                &filters,
                &self.preview_selection,
                self.sample_time(cx),
            )?;
            Ok::<_, ForegroundError>(preview)
        })();
        match prepared {
            Ok(preview) => {
                self.install_preview(preview, cx);
                Ok(())
            }
            Err(error) => {
                // Preserve the actual applied image/selection/export snapshot.
                // A later success clears only this presentation failure.
                self.pipeline_error = Some(error.to_string());
                Err(error)
            }
        }
    }
    fn install_preview(&mut self, preview: PreparedPreview, cx: &mut App) {
        self.pipeline_error = None;
        self.applied_image = Some(preview.image);
        self.preview_selection = preview.selection;
        self.preview_selection_error = None;
        self.bitmap = Some(preview.bitmap);
        self.raster = Some(preview.raster);
        // Layer Maps refreshes its real choice arrays during pipeline work.
        // Re-render the retained inspector without reloading name/opacity
        // drafts or requesting another render from its presentation runtime.
        self.filters
            .update(cx, |filters, cx| filters.refresh_controls(cx));
    }
    fn select_preview(
        &mut self,
        selection: PreviewSelection,
        shown_image: &mycela_engine::SharedImageMap,
        epoch: u64,
        cx: &mut Context<Self>,
    ) {
        let Some(image) = &self.applied_image else {
            return;
        };
        if self.generation.load(Ordering::Acquire) != epoch || !image.same_reference(shown_image) {
            self.preview_selection_error =
                Some("The applied image changed. Open the map menu again.".into());
            cx.notify();
            return;
        }
        match map_preview::project(image, &selection) {
            Ok((bitmap, raster)) => {
                self.preview_selection = selection;
                self.preview_selection_error = None;
                self.bitmap = Some(bitmap);
                self.raster = Some(raster);
            }
            Err(error) => self.preview_selection_error = Some(error.to_string()),
        }
        cx.notify();
    }
    fn switch_source(
        &mut self,
        source: TextureSourceKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.opening || self.pending || self.runtime.is_dialog_visible() {
            return;
        }
        self.cancel_image_import(window, cx);
        if source == TextureSourceKind::ImportedImage && self.generator_view(source).is_none() {
            self.open_image(cx);
            return;
        }
        if source == self.source {
            if self.generator_preview.is_switching() {
                self.generator_preview.retire();
                let _ = self.refresh_preview(cx);
                cx.notify();
            }
            return;
        }
        // A source switch invalidates queued input-owner work too, even before
        // the owned candidate preview is ready. Retained drafts survive.
        self.filters
            .update(cx, |filters, cx| filters.retire_input_work(cx));
        if source.requires_owned_preview() || self.filters.read(cx).uses_spatial() {
            self.begin_generator_switch(source, window, cx);
            return;
        }
        self.generator_preview.retire();
        let source_image = (|| {
            let image = match source {
                TextureSourceKind::SolidColor => {
                    self.solid_preview_image(self.solid_switch_time(cx))?
                }
                TextureSourceKind::Gradient => {
                    if self.gradient.is_none() {
                        let model = mycela_engine::GradientGenerator::new()
                            .map_err(|e| ForegroundError::Operation(e.to_string()))?;
                        self.gradient =
                            Some(cx.new(|cx| {
                                GradientSourceView::new(model, &self.runtime, window, cx)
                            }));
                    }
                    let gradient = self.gradient.as_ref().expect("prepared source").read(cx);
                    gradient
                        .model()
                        .render_unbound(
                            LIVE_PREVIEW_SIZE,
                            LIVE_PREVIEW_SIZE,
                            gradient.preview_time(),
                        )
                        .map_err(|e| ForegroundError::Operation(e.to_string()))?
                }
                _ => {
                    return Err(ForegroundError::Operation(
                        "The generator requires an owned render job.".into(),
                    ));
                }
            };
            Ok::<_, ForegroundError>(image)
        })();
        let image = match source_image {
            Ok(image) => image,
            Err(error) => {
                self.error = Some(error.to_string());
                cx.notify();
                return;
            }
        };
        let preview = match map_preview::render(
            image,
            self.invert.as_ref(),
            &self.filters.read(cx).pipeline(),
            &PreviewSelection::BaseColor,
            self.generator_view(source).map_or_else(
                || {
                    if self.source.scene_kind().is_some() {
                        self.sample_time(cx)
                    } else {
                        self.sample_time
                    }
                },
                |view| view.read(cx).preview_time(),
            ),
        ) {
            Ok(preview) => preview,
            Err(error) => {
                // Switching a source must not promote a routing/preparation
                // failure into the unrelated source/import error owner.
                self.pipeline_error = Some(error.to_string());
                cx.notify();
                return;
            }
        };
        let target = (source == TextureSourceKind::SolidColor).then(|| self.node.color_property());
        let previous =
            (self.source == TextureSourceKind::SolidColor).then(|| self.node.color_property());
        if let Err(error) = self.binding.set_property(target.as_ref()) {
            if let Err(rollback) = self.binding.set_property(previous.as_ref()) {
                self.runtime.retain_failure(rollback);
            }
            self.error = Some(error.to_string());
            cx.notify();
            return;
        }
        self.runtime.retire_pending_input();
        self.preview_export.retire();
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.applied_text = None;
        if source == TextureSourceKind::SolidColor && self.source.scene_kind().is_some() {
            self.sample_time = self.sample_time(cx);
        }
        self.source = source;
        if source == TextureSourceKind::SolidColor {
            self.sync_solid_animation_time(cx);
        }
        self.install_preview(preview, cx);
        self.activate_generators(window, cx);
        self.sync_filter_time(window, cx);
        if source == TextureSourceKind::SolidColor {
            self.sync_unchanged_drafts(false, window, cx);
        }
        let focus = match source {
            TextureSourceKind::SolidColor => self.fields[0].read(cx).focus_handle(cx),
            _ => self
                .active_generator()
                .expect("prepared source")
                .read(cx)
                .preferred_focus(cx),
        };
        window.focus(&focus, cx);
        let title = if source == TextureSourceKind::Gradient {
            "Gradient workspace".into()
        } else {
            self.imported
                .as_ref()
                .map(GtxImport::name)
                .unwrap_or_else(|| "Texture workspace".into())
        };
        window.set_window_title(&format!("Mycela · {title}"));
        cx.notify();
    }
    fn toggle_invert(&mut self, cx: &mut Context<Self>) {
        self.invert = if self.invert.is_some() {
            None
        } else {
            Some(LiveInvert::new(&self.runtime))
        };
        self.present_effect_change(cx);
    }
    fn set_invert_channel(&mut self, channel: i32, affected: bool, cx: &mut Context<Self>) {
        if let Some(invert) = &self.invert {
            if let Err(error) = invert.set_affected(channel, affected) {
                self.error = Some(error.to_string());
                cx.notify();
                return;
            }
            self.present_effect_change(cx);
        }
    }
    fn present_effect_change(&mut self, cx: &mut Context<Self>) {
        // Effect edits do not validate or commit the color drafts.
        let _ = self.refresh_preview(cx);
        cx.notify();
    }
    fn render_invert(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let options: Option<[Option<bool>; 4]> = self
            .invert
            .as_ref()
            .map(|effect| std::array::from_fn(|channel| effect.affected(channel as i32).ok()));
        div()
            .v_flex()
            .gap_2()
            .pt_3()
            .border_t_1()
            .border_color(cx.theme().border)
            // This command keeps its native identity when its label changes,
            // so Add/Remove never leaves keyboard focus on an unmounted item.
            .child(
                Button::new("toggle-invert")
                    .label(if options.is_some() {
                        "Remove Invert"
                    } else {
                        "Add Invert"
                    })
                    .on_click(cx.listener(|view, _, _, cx| view.toggle_invert(cx))),
            )
            .when_some(options, |this, options| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("Invert channels"),
                )
                .children(
                    [
                        ("invert-red", "Red", 0),
                        ("invert-green", "Green", 1),
                        ("invert-blue", "Blue", 2),
                        ("invert-transparency", "Transparency", 3),
                    ]
                    .into_iter()
                    .map(|(id, label, channel)| {
                        Checkbox::new(id)
                            .label(label)
                            .checked(options[channel as usize].unwrap_or(false))
                            .disabled(options[channel as usize].is_none())
                            .on_change(cx.listener(move |view, checked, _, cx| {
                                view.set_invert_channel(channel, *checked, cx);
                            }))
                    }),
                )
            })
    }
    fn sync_drafts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let values = self.node.snapshot();
        let fields = [
            values.color[0],
            values.color[1],
            values.color[2],
            values.transparency,
        ];
        for (input, value) in self.fields.iter().zip(fields) {
            input.update(cx, |state, cx| {
                state.set_value(value.to_string(), window, cx)
            });
        }
        self.synced_text = self.draft_text(cx);
        self.applied_text = None;
    }
    fn draft_text(&self, cx: &App) -> [String; 4] {
        std::array::from_fn(|ix| self.fields[ix].read(cx).value().to_string())
    }
    fn requested_values(&self, cx: &App) -> Result<([f64; 3], f64), ForegroundError> {
        let labels = ["Red", "Green", "Blue", "Transparency"];
        let mut parsed = [0.; 4];
        for (ix, input) in self.fields.iter().enumerate() {
            parsed[ix] = input.read(cx).value().parse::<f64>().map_err(|_| {
                ForegroundError::Operation(format!(
                    "{} needs a number. Correct the value and apply again.",
                    labels[ix]
                ))
            })?;
        }
        Ok(([parsed[0], parsed[1], parsed[2]], parsed[3]))
    }
    fn sync_unchanged_drafts(
        &mut self,
        input_committed: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = self.draft_text(cx);
        if text == self.synced_text
            || (input_committed && self.applied_text.as_ref() == Some(&text))
        {
            self.sync_drafts(window, cx);
        }
    }
    fn execute(
        &mut self,
        command: ForegroundCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), ForegroundError> {
        if self.image_import.is_busy() {
            return Ok(());
        }
        let result = (|| {
            if self.source != TextureSourceKind::SolidColor {
                return Err(ForegroundError::Operation(
                    "Switch to Solid Color before applying RGB edits.".into(),
                ));
            }
            let (rgb, transparency) = match command {
                ForegroundCommand::RevertDraft => {
                    self.sync_drafts(window, cx);
                    return Ok(());
                }
                ForegroundCommand::SetValues { rgb, transparency } => (rgb, transparency),
                ForegroundCommand::ApplyValues { rgb, transparency } => (rgb, transparency),
                ForegroundCommand::ApplyDraft => unreachable!("draft captured before queue"),
            };
            if matches!(command, ForegroundCommand::ApplyValues { .. }) {
                self.binding
                    .submit_input(&ContextColorValue::unlinked(rgb))?;
            } else {
                self.node
                    .color_property()
                    .set_color(rgb)
                    .map_err(|e| ForegroundError::Operation(e.to_string()))?;
            }
            self.node
                .transparency_property()
                .set_transparency(transparency)
                .map_err(|e| ForegroundError::Operation(e.to_string()))?;
            if !matches!(command, ForegroundCommand::ApplyValues { .. }) {
                self.sync_drafts(window, cx);
            }
            Ok(())
        })();
        self.error = result.as_ref().err().map(ToString::to_string);
        // Even a partial engine failure displays the actual committed node.
        let _ = self.refresh_preview(cx);
        if !matches!(command, ForegroundCommand::RevertDraft) {
            self.sync_solid_animation_values(cx);
        }
        cx.notify();
        result
    }
    fn submit(&mut self, command: ForegroundCommand, cx: &mut Context<Self>) {
        if self.pending || self.opening || self.image_import.is_busy() {
            return;
        }
        let command = if matches!(command, ForegroundCommand::ApplyDraft) {
            match self.requested_values(cx) {
                Ok((rgb, transparency)) => {
                    self.applied_text = Some(self.draft_text(cx));
                    ForegroundCommand::ApplyValues { rgb, transparency }
                }
                Err(error) => {
                    self.error = Some(error.to_string());
                    cx.notify();
                    return;
                }
            }
        } else {
            command
        };
        self.pending = true;
        cx.notify();
        let generation = self.generation.load(Ordering::Acquire);
        let task = self.dispatcher.enqueue(command, cx);
        cx.spawn(async move |owner, cx| {
            let result = task.await;
            let _ = owner.update(cx, |view, cx| {
                if view.generation.load(Ordering::Acquire) != generation {
                    return;
                }
                view.pending = false;
                if let Err(error) = result {
                    view.error = Some(error.to_string());
                }
                cx.notify();
            });
        })
        .detach();
    }
}
impl Render for SolidColorView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let values = self.node.snapshot();
        let inspector = div()
            .v_flex()
            .gap_3()
            .when(self.imported.is_some(), |this| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child("Source values · Original settings retained"),
                )
            })
            .child(
                Form::new()
                    .child(
                        Field::new().label("Red").child(
                            Input::new(&self.fields[0])
                                .id("color-red")
                                .aria_label("Red"),
                        ),
                    )
                    .child(
                        Field::new().label("Green").child(
                            Input::new(&self.fields[1])
                                .id("color-green")
                                .aria_label("Green"),
                        ),
                    )
                    .child(
                        Field::new().label("Blue").child(
                            Input::new(&self.fields[2])
                                .id("color-blue")
                                .aria_label("Blue"),
                        ),
                    )
                    .child(
                        Field::new()
                            .label("Transparency")
                            .description("0 is opaque · 1 is clear")
                            .child(
                                Input::new(&self.fields[3])
                                    .id("color-transparency")
                                    .aria_label("Transparency"),
                            ),
                    ),
            )
            .child(
                div()
                    .h_flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        Button::new("open-color-dialog")
                            .label("Color…")
                            .disabled(self.image_import.is_busy())
                            .on_click(cx.listener(|view, _, window, cx| {
                                let runtime = Arc::clone(&view.runtime);
                                let fields = view.fields.clone();
                                let owner = cx.weak_entity();
                                window.open_dialog(cx, move |dialog, _, _| {
                                    let close = Arc::clone(&runtime);
                                    let commit = owner.clone();
                                    dialog
                                        .title("Color")
                                        .child(
                                            Form::new()
                                                .child(
                                                    Field::new().label("Red").child(
                                                        Input::new(&fields[0])
                                                            .id("dialog-red")
                                                            .aria_label("Red"),
                                                    ),
                                                )
                                                .child(
                                                    Field::new().label("Green").child(
                                                        Input::new(&fields[1])
                                                            .id("dialog-green")
                                                            .aria_label("Green"),
                                                    ),
                                                )
                                                .child(
                                                    Field::new().label("Blue").child(
                                                        Input::new(&fields[2])
                                                            .id("dialog-blue")
                                                            .aria_label("Blue"),
                                                    ),
                                                ),
                                        )
                                        .button_props(DialogButtonProps::default().ok_text("Apply"))
                                        .on_ok(move |_, _, cx| {
                                            let _ = commit.update(cx, |view, cx| {
                                                view.submit(ForegroundCommand::ApplyDraft, cx)
                                            });
                                            true
                                        })
                                        .on_close(move |_, _, _| close.set_dialog_visible(false))
                                });
                                view.runtime.set_dialog_visible(true);
                            })),
                    )
                    .child(
                        Button::new("apply-color")
                            .label("Apply")
                            .primary()
                            .disabled(self.pending || self.opening || self.image_import.is_busy())
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.submit(ForegroundCommand::ApplyDraft, cx)
                            })),
                    )
                    .child(
                        Button::new("revert-color")
                            .label("Revert edits")
                            .disabled(self.pending || self.opening || self.image_import.is_busy())
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.submit(ForegroundCommand::RevertDraft, cx)
                            })),
                    ),
            );
        let inspector = div()
            .v_flex()
            .p_4()
            .gap_3()
            .when(self.source == TextureSourceKind::SolidColor, |this| {
                this.child(inspector)
            })
            .when(
                self.source == TextureSourceKind::ImportedImage || self.image_import.is_busy(),
                |this| this.child(self.render_imported_file(cx)),
            )
            .when(self.source != TextureSourceKind::SolidColor, |this| {
                this.children(self.active_generator())
                    .children(self.active_scene())
                    .children(self.active_polygon())
                    .when(self.source.scene_kind().is_some(), |this| {
                        this.child(self.scene_camera.clone())
                    })
            })
            .child(self.render_invert(cx));
        let inspector = div()
            .v_flex()
            .when(!self.show_filters, |this| this.child(inspector))
            .when(self.show_filters, |this| this.child(self.filters.clone()))
            .when_some(self.error().map(str::to_owned), |this, error| {
                this.child(
                    div()
                        .id("color-error")
                        .test_support()
                        .role(gpui_kit::Role::Alert)
                        .aria_label(error.clone())
                        .px_4()
                        .pb_4()
                        .text_color(theme.danger)
                        .text_sm()
                        .child(error),
                )
            });
        // Navigation is fixed above the one scroll owner. Each page keeps its
        // own scroll identity; changing pages never strands navigation offscreen.
        let content_id = if self.show_filters {
            "filter-inspector-content"
        } else {
            "source-inspector-content"
        };
        let inspector = div()
            .id("color-inspector")
            .role(gpui_kit::Role::Group)
            .aria_label("Properties")
            .test_support()
            .v_flex()
            .w_80()
            .flex_shrink_0()
            .min_h_0()
            .border_r_1()
            .border_color(theme.border)
            .child(
                div()
                    .id("inspector-navigation")
                    .role(gpui_kit::Role::Group)
                    .aria_label("Inspector pages")
                    .test_support()
                    .h_flex()
                    .gap_2()
                    .px_4()
                    .py_3()
                    .flex_shrink_0()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        Button::new("source-tab")
                            .label("Source")
                            .selected(!self.show_filters)
                            .on_click(cx.listener(|view, _, window, cx| {
                                view.show_filters = false;
                                let focus = if view.source == TextureSourceKind::SolidColor {
                                    view.fields[0].read(cx).focus_handle(cx)
                                } else if let Some(polygon) = view.active_polygon() {
                                    polygon.read(cx).preferred_focus(cx)
                                } else if let Some(scene) = view.active_scene() {
                                    scene.read(cx).preferred_focus(cx)
                                } else {
                                    view.active_generator()
                                        .expect("active source")
                                        .read(cx)
                                        .preferred_focus(cx)
                                };
                                window.focus(&focus, cx);
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("filters-tab")
                            .label("Filters")
                            .selected(self.show_filters)
                            .on_click(cx.listener(|view, _, window, cx| {
                                view.show_filters = true;
                                let focus = view.filters.read(cx).preferred_focus(cx);
                                window.focus(&focus, cx);
                                cx.notify();
                            })),
                    )
                    .when(
                        self.source == TextureSourceKind::SolidColor && !self.show_filters,
                        |this| {
                            this.child(
                                Button::new("edit-solid-animation")
                                    .label("Edit animation…")
                                    .disabled(
                                        self.pending || self.opening || self.image_import.is_busy(),
                                    )
                                    .on_click(
                                        cx.listener(|view, _, _, cx| view.open_solid_animation(cx)),
                                    ),
                            )
                        },
                    ),
            )
            .when(!self.show_filters, |this| {
                this.child(
                    div()
                        .id("source-navigation")
                        .role(gpui_kit::Role::Group)
                        .aria_label("Texture source")
                        .test_support()
                        .px_4()
                        .py_3()
                        .flex_shrink_0()
                        .border_b_1()
                        .border_color(theme.border)
                        .child(
                            div()
                                .h_flex()
                                .flex_wrap()
                                .gap_2()
                                .children(
                                    [
                                        (TextureSourceKind::SolidColor, "source-solid"),
                                        (TextureSourceKind::Gradient, "source-gradient"),
                                        (TextureSourceKind::Noise, "source-noise"),
                                        (TextureSourceKind::Cells, "source-cells"),
                                        (TextureSourceKind::WoodRings, "source-wood-rings"),
                                        (TextureSourceKind::BricksBoards, "source-bricks-boards"),
                                        (TextureSourceKind::Polygon, "source-polygon"),
                                        (TextureSourceKind::ImportedImage, "source-imported-image"),
                                    ]
                                    .into_iter()
                                    .map(|(kind, id)| {
                                        Button::new(id)
                                            .small()
                                            .label(
                                                if kind == TextureSourceKind::ImportedImage
                                                    && self.generator_view(kind).is_none()
                                                {
                                                    "Import image…"
                                                } else {
                                                    kind.name()
                                                },
                                            )
                                            .selected(self.source == kind)
                                            .disabled(
                                                self.opening
                                                    || self.pending
                                                    || self.runtime.is_dialog_visible(),
                                            )
                                            .on_click(cx.listener(move |view, _, window, cx| {
                                                view.switch_source(kind, window, cx)
                                            }))
                                    }),
                                )
                                .child({
                                    let available = !self.opening
                                        && !self.pending
                                        && !self.runtime.is_dialog_visible();
                                    let button = Button::new("scene-source-menu")
                                        .label(if self.source.scene_kind().is_some() {
                                            self.source.name()
                                        } else {
                                            "3D primitive…"
                                        })
                                        .selected(self.source.scene_kind().is_some())
                                        .small()
                                        .dropdown_caret(true)
                                        .disabled(!available);
                                    if available {
                                        let owner = cx.entity().downgrade();
                                        button
                                            .dropdown_menu(move |mut menu, _, _| {
                                                for kind in [
                                                    TextureSourceKind::Cone,
                                                    TextureSourceKind::Cube,
                                                    TextureSourceKind::Cylinder,
                                                    TextureSourceKind::Disc,
                                                    TextureSourceKind::Plane,
                                                    TextureSourceKind::Sphere,
                                                    TextureSourceKind::Torus,
                                                ] {
                                                    let owner = owner.clone();
                                                    menu = menu.item(
                                                        PopupMenuItem::new(kind.name()).on_click(
                                                            move |_, window, cx| {
                                                                let _ =
                                                                    owner.update(cx, |view, cx| {
                                                                        view.switch_source(
                                                                            kind, window, cx,
                                                                        )
                                                                    });
                                                            },
                                                        ),
                                                    );
                                                }
                                                menu
                                            })
                                            .into_any_element()
                                    } else {
                                        button.into_any_element()
                                    }
                                }),
                        ),
                )
            })
            .child(
                div()
                    .id(content_id)
                    .role(gpui_kit::Role::Group)
                    .aria_label("Inspector contents")
                    .test_support()
                    .flex_1()
                    .min_h_0()
                    .child(inspector)
                    .overflow_y_scrollbar()
                    .id(content_id),
            );
        let mut pipeline = vec![self.source.name()];
        if self.invert.is_some() {
            pipeline.push("Invert");
        }
        pipeline.extend(self.filters.read(cx).pipeline_names());
        pipeline.push("Preview");
        let pipeline = pipeline.join(" → ");
        let status = if self.image_import.is_busy() {
            self.image_import.status()
        } else if self.opening {
            "Opening GTX…"
        } else if self.error().is_some()
            || self.filters.read(cx).error().is_some()
            || self
                .active_generator()
                .is_some_and(|g| g.read(cx).error().is_some())
            || self
                .active_scene()
                .is_some_and(|g| g.read(cx).error().is_some())
            || self
                .active_polygon()
                .is_some_and(|g| g.read(cx).error().is_some())
        {
            "Check properties"
        } else if self
            .active_generator()
            .is_some_and(|g| g.read(cx).is_pending())
            || self.active_scene().is_some_and(|g| g.read(cx).is_pending())
            || (self.active_scene().is_some() && self.scene_camera.read(cx).is_pending())
        {
            "Applying properties…"
        } else if self.generator_preview.is_busy() {
            "Rendering preview…"
        } else if self.pending
            || (self.source == TextureSourceKind::SolidColor && self.applied_text.is_some())
        {
            "Applying color…"
        } else {
            "Applied values"
        };
        let pipeline_tooltip = pipeline.clone();
        let preview = div()
            .id("preview-panel")
            .role(gpui_kit::Role::Group)
            .aria_label("Preview")
            .test_support()
            .v_flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .p_6()
            .gap_4()
            .child(
                div()
                    .v_flex()
                    .gap_2()
                    .child(
                        div()
                            .h_flex()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_lg()
                                    .font_semibold()
                                    .child("Texture preview"),
                            )
                            .when_some(
                                self.applied_image
                                    .as_ref()
                                    .filter(|image| !image.named_channels().is_empty()),
                                |header, image| {
                                    let choices = map_preview::choices(image);
                                    let selected = self.preview_selection.clone();
                                    let shown_image = image.clone();
                                    let epoch = self.generation.load(Ordering::Acquire);
                                    let owner = cx.weak_entity();
                                    header.child(
                                        Button::new("preview-map-selector")
                                            .label(self.preview_selection.label())
                                            .accessibility_label(format!(
                                                "Preview: {}",
                                                self.preview_selection.label()
                                            ))
                                            .tooltip(self.preview_selection.label())
                                            .max_w_40()
                                            .min_w_0()
                                            .small()
                                            .dropdown_caret(true)
                                            .dropdown_menu(move |mut menu, _, _| {
                                                for selection in &choices {
                                                    let owner = owner.clone();
                                                    let choice = selection.clone();
                                                    let shown_image = shown_image.clone();
                                                    menu = menu.item(
                                                        PopupMenuItem::new(selection.label())
                                                            .checked(*selection == selected)
                                                            .on_click(move |_, _, cx| {
                                                                let _ =
                                                                    owner.update(cx, |view, cx| {
                                                                        view.select_preview(
                                                                            choice.clone(),
                                                                            &shown_image,
                                                                            epoch,
                                                                            cx,
                                                                        )
                                                                    });
                                                            }),
                                                    );
                                                }
                                                menu.scrollable(true)
                                            }),
                                    )
                                },
                            ),
                    )
                    .child(
                        div()
                            .id("texture-pipeline")
                            .role(gpui_kit::Role::Group)
                            .aria_label(pipeline.clone())
                            .test_support()
                            .w_full()
                            .min_w_0()
                            .truncate()
                            .tooltip(move |window, cx| {
                                Tooltip::new(pipeline_tooltip.clone()).build(window, cx)
                            })
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(pipeline),
                    ),
            )
            .when_some(self.preview_selection_error.clone(), |preview, error| {
                preview.child(
                    div()
                        .id("preview-map-error")
                        .role(gpui_kit::Role::Alert)
                        .aria_label(error.clone())
                        .test_support()
                        .text_sm()
                        .text_color(theme.danger)
                        .child(error),
                )
            })
            .child(
                div()
                    .id("preview-surface")
                    .role(gpui_kit::Role::Group)
                    .aria_label("Texture with transparency checkerboard")
                    .test_support()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .border_1()
                    .border_color(theme.border)
                    .rounded(theme.radius)
                    .overflow_hidden()
                    .bg(theme.muted)
                    .child(crate::preview_background::checkerboard())
                    .when_some(self.raster.clone(), |this, image| {
                        this.child(
                            img(image)
                                .id("solid-preview")
                                .absolute()
                                .size_full()
                                .object_fit(gpui_kit::ObjectFit::Contain),
                        )
                    }),
            )
            .child(
                div()
                    .v_flex()
                    .gap_1()
                    .child(
                        div()
                            .id("preview-status")
                            .role(gpui_kit::Role::Status)
                            .aria_label(status)
                            .test_support()
                            .text_sm()
                            .font_medium()
                            .child(status),
                    )
                    .when_some(
                        self.preview_export.status().map(str::to_owned),
                        |this, status| {
                            this.child(
                                div()
                                    .id("preview-export-status")
                                    .role(gpui_kit::Role::Status)
                                    .aria_label(status.clone())
                                    .test_support()
                                    .w_full()
                                    .min_w_0()
                                    .whitespace_normal()
                                    .text_ellipsis()
                                    .line_clamp(2)
                                    .text_sm()
                                    .text_color(if self.preview_export.is_error() {
                                        theme.danger
                                    } else {
                                        theme.muted_foreground
                                    })
                                    .child(status),
                            )
                        },
                    )
                    .child(div().text_sm().text_color(theme.muted_foreground).child(
                        if self.source == TextureSourceKind::SolidColor {
                            format!(
                                "Source RGB {:.3}, {:.3}, {:.3} · Transparency {:.3}",
                                values.color[0],
                                values.color[1],
                                values.color[2],
                                values.transparency
                            )
                        } else if self.source == TextureSourceKind::Polygon
                            || self.source.scene_kind().is_some()
                        {
                            format!("Time {:.3} · {}", self.sample_time(cx), self.source.name())
                        } else {
                            let model = self
                                .active_generator()
                                .expect("active source")
                                .read(cx)
                                .model();
                            let size = model
                                .parameter(mycela_engine::GradientGeneratorParameterKind::Size)
                                .pair()
                                .expect("Size pair");
                            let location = model
                                .parameter(mycela_engine::GradientGeneratorParameterKind::Location)
                                .pair()
                                .expect("Location pair");
                            format!(
                                "Time {:.3} · Size {:.3}, {:.3} · Location {:.3}, {:.3}",
                                self.active_generator()
                                    .expect("active source")
                                    .read(cx)
                                    .preview_time(),
                                size[0],
                                size[1],
                                location[0],
                                location[1]
                            )
                        },
                    )),
            );
        div()
            .id("solid-color-editor")
            .key_context("SolidColor")
            .track_focus(&self.focus)
            .size_full()
            .v_flex()
            .on_action(cx.listener(|view, _: &ApplyColor, _, cx| {
                view.submit(ForegroundCommand::ApplyDraft, cx)
            }))
            .on_action(cx.listener(
                |view, _: &crate::graph_document::OpenGraphDocument, _, cx| {
                    if let Err(error) = crate::graph_document::open_graph_document_window(cx) {
                        view.error = Some(error);
                        cx.notify();
                    }
                },
            ))
            .on_action(
                cx.listener(|view, _: &crate::graph_document::OpenNodeEditor, _, cx| {
                    view.open_node_editor(cx)
                }),
            )
            .on_action(cx.listener(|view, _: &OpenGtx, window, cx| view.open_gtx(window, cx)))
            .on_action(cx.listener(|view, _: &OpenImage, _, cx| view.open_image(cx)))
            .on_action(cx.listener(|view, _: &ExportPreviewPng, _, cx| view.export_preview(cx)))
            .on_action(cx.listener(|view, _: &RevertColor, _, cx| {
                view.submit(ForegroundCommand::RevertDraft, cx)
            }))
            .child(
                div()
                    .h_flex()
                    .justify_between()
                    .gap_4()
                    .px_6()
                    .py_4()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .h_flex()
                            .gap_3()
                            .min_w_0()
                            .flex_1()
                            .child(div().text_lg().font_semibold().child("Mycela"))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_sm()
                                    .text_color(theme.muted_foreground)
                                    .child(if self.source != TextureSourceKind::SolidColor {
                                        format!("{} workspace", self.source.name())
                                    } else {
                                        self.imported
                                            .as_ref()
                                            .map(GtxImport::name)
                                            .unwrap_or_else(|| "Texture workspace".into())
                                    }),
                            ),
                    )
                    .child(if self.preview_export.can_cancel() {
                        Button::new("cancel-image-export")
                            .small()
                            .flex_shrink_0()
                            .label("Cancel export")
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.preview_export.cancel();
                                cx.notify();
                            }))
                    } else {
                        Button::new("export-image")
                            .small()
                            .flex_shrink_0()
                            .label("Export image…")
                            .disabled(
                                self.opening
                                    || self.pending
                                    || self.preview_export.is_busy()
                                    || self.runtime.is_dialog_visible(),
                            )
                            .on_click(
                                cx.listener(|view, _, window, cx| {
                                    view.open_export_image(window, cx)
                                }),
                            )
                    })
                    .child(
                        Button::new("export-preview-png")
                            .small()
                            .flex_shrink_0()
                            .label("Export preview…")
                            .tooltip_with_action(
                                format!("Export {LIVE_PREVIEW_SIZE} × {LIVE_PREVIEW_SIZE} PNG"),
                                &ExportPreviewPng,
                                None,
                            )
                            .disabled(
                                self.opening
                                    || self.preview_export.is_busy()
                                    || self.runtime.is_dialog_visible()
                                    || self.bitmap.is_none(),
                            )
                            .on_click(cx.listener(|view, _, _, cx| view.export_preview(cx))),
                    )
                    .child(
                        Button::new("open-graph-document")
                            .small()
                            .flex_shrink_0()
                            .label("Node editor")
                            .tooltip_with_action(
                                "Open node editor",
                                &crate::graph_document::OpenNodeEditor,
                                None,
                            )
                            .disabled(self.runtime.is_dialog_visible())
                            .on_click(cx.listener(|view, _, _, cx| view.open_node_editor(cx))),
                    )
                    .child(
                        Button::new("open-gtx")
                            .small()
                            .flex_shrink_0()
                            .label(if self.opening {
                                "Opening…"
                            } else {
                                "Open GTX…"
                            })
                            .disabled(
                                self.opening || self.pending || self.runtime.is_dialog_visible(),
                            )
                            .on_click(cx.listener(|view, _, window, cx| view.open_gtx(window, cx))),
                    ),
            )
            .child(
                div()
                    .h_flex()
                    .items_stretch()
                    .flex_1()
                    .min_h_0()
                    .child(inspector)
                    .child(preview),
            )
    }
}
impl Drop for SolidColorView {
    fn drop(&mut self) {
        self.image_import.retire();
        self.generator_preview.retire();
        self.preview_export.retire();
        self.invert.take();
        if let Err(error) = self.binding.set_property(None) {
            self.runtime.retain_failure(error);
        }
        self.runtime.shutdown();
    }
}
#[cfg(feature = "texture-workspace-test")]
pub(crate) fn bind_keys(cx: &mut App) {
    crate::tone_curve::bind_keys(cx);
    cx.bind_keys([
        gpui_kit::KeyBinding::new("secondary-o", OpenGtx, Some("SolidColor")),
        gpui_kit::KeyBinding::new("secondary-shift-o", OpenImage, Some("SolidColor")),
        gpui_kit::KeyBinding::new("secondary-shift-e", ExportPreviewPng, Some("SolidColor")),
        gpui_kit::KeyBinding::new("secondary-enter", ApplyColor, Some("SolidColor")),
        gpui_kit::KeyBinding::new("escape", RevertColor, Some("SolidColor")),
        gpui_kit::KeyBinding::new(
            "secondary-alt-o",
            crate::graph_document::OpenGraphDocument,
            Some("SolidColor"),
        ),
        gpui_kit::KeyBinding::new(
            "secondary-alt-n",
            crate::graph_document::OpenNodeEditor,
            Some("SolidColor"),
        ),
    ]);
}

#[cfg(test)]
mod document_tests {
    use super::*;

    #[gpui_kit::test]
    async fn source_switch_rejects_already_queued_rgb_edit(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        let (handle, root) = cx.update(crate::open_main_window).unwrap();
        let editor = cx.update(|cx| root.read(cx).color_view());
        let task = cx
            .update_window(handle, |_, window, cx| {
                editor.update(cx, |view, cx| {
                    let task = view.dispatcher().enqueue(
                        ForegroundCommand::SetValues {
                            rgb: [0.9; 3],
                            transparency: 0.8,
                        },
                        cx,
                    );
                    view.switch_source(TextureSourceKind::Gradient, window, cx);
                    task
                })
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(task.await, Err(ForegroundError::TextureReplaced));
        cx.update(|cx| {
            let view = editor.read(cx);
            assert_eq!(view.active_source(), TextureSourceKind::Gradient);
            assert_eq!(view.node().snapshot().color, [0.; 3]);
        });
    }

    /// Owner/queue regression: replacement and enqueue happen in one foreground
    /// borrow so the queued old edit cannot run before the replacement.
    #[gpui_kit::test]
    async fn replacement_invalidates_already_queued_foreground_edits(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let (handle, root) = cx.update(crate::open_main_window).unwrap();
        let editor = cx.update(|cx| root.read(cx).color_view());
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../genetica/Presets/Texture/Pattern/Solid Color.gtx");
        let prepared = PreparedGtx::read(path).unwrap();
        let task = cx
            .update_window(handle, |_, window, cx| {
                editor.update(cx, |view, cx| {
                    let task = view.dispatcher().enqueue(
                        ForegroundCommand::SetValues {
                            rgb: [0.1, 0.2, 0.3],
                            transparency: 0.8,
                        },
                        cx,
                    );
                    view.install_gtx(prepared, window, cx).unwrap();
                    task
                })
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(task.await, Err(ForegroundError::TextureReplaced));
        cx.update(|cx| assert_eq!(editor.read(cx).node().snapshot().color, [1.; 3]));
    }
}

#[cfg(test)]
#[path = "solid_color_scene_tests.rs"]
mod scene_tests;
