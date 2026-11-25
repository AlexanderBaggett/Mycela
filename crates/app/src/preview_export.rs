//! The quick action copies the applied preview. Export image captures
//! immutable committed frames and renders at the chosen geometry in background.
//! Both use the same temporary/rename writer and actual window/source lifetime.
use super::{
    SolidColorView,
    export_settings::{ExportSettings, checked_dimensions},
};
use crate::{
    LIVE_PREVIEW_SIZE,
    export_render::{ExportRenderJob, ExportSource},
    gradient_source::TextureSourceKind,
};
use gpui_kit::component::{
    WindowExt,
    button::{Button, ButtonVariants},
    dialog::{DialogAction, DialogClose, DialogFooter},
};
use gpui_kit::{App, AppContext, Context, ParentElement, Task, Window};
use image::ImageEncoder;
use mycela_engine::{PreviewBitmap, PreviewBitmapFormat};
use std::{
    fs::{File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

pub(crate) struct PreviewPixels {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}
impl PreviewPixels {
    fn capture(bitmap: &PreviewBitmap) -> Result<Self, String> {
        // The existing quick action keeps its actual applied-preview scope.
        if bitmap.width() != LIVE_PREVIEW_SIZE || bitmap.height() != LIVE_PREVIEW_SIZE {
            return Err("The applied preview has unexpected dimensions.".into());
        }
        Self::capture_rendered(bitmap)
    }
    pub(crate) fn capture_rendered(bitmap: &PreviewBitmap) -> Result<Self, String> {
        let (width, height) = checked_dimensions(bitmap.width(), bitmap.height())?;
        let bpp = match bitmap.format() {
            PreviewBitmapFormat::Rgb24 => 3,
            PreviewBitmapFormat::Argb32 => 4,
            PreviewBitmapFormat::PArgb32 => {
                return Err("Premultiplied preview export is unavailable.".into());
            }
        };
        // rows() copies every byte while holding the bitmap's one storage guard.
        // A retained PreviewBitmap clone alone would NOT be an immutable snapshot.
        let rows = bitmap
            .rows()
            .map_err(|e| format!("Couldn’t capture the preview: {e}"))?;
        let byte_count = (width as usize)
            .checked_mul(height as usize)
            .and_then(|n| n.checked_mul(4))
            .ok_or_else(|| "The PNG byte layout is not addressable.".to_string())?;
        let mut rgba = Vec::new();
        rgba.try_reserve_exact(byte_count)
            .map_err(|e| format!("Couldn’t allocate PNG pixels: {e}"))?;
        for row in rows {
            for pixel in row.chunks_exact(bpp) {
                rgba.extend_from_slice(&[
                    pixel[2],
                    pixel[1],
                    pixel[0],
                    if bpp == 4 { pixel[3] } else { 255 },
                ]);
            }
        }
        Ok(Self {
            width: width as u32,
            height: height as u32,
            rgba,
        })
    }
    fn png(&self) -> Result<Vec<u8>, String> {
        let mut bytes = FalliblePngBuffer(Vec::new());
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(
                &self.rgba,
                self.width,
                self.height,
                image::ExtendedColorType::Rgba8,
            )
            .map_err(|e| format!("Couldn’t encode the preview PNG: {e}"))?;
        Ok(bytes.0)
    }
    pub(crate) fn into_rgba(self) -> Vec<u8> {
        self.rgba
    }
    fn save(&self, path: PathBuf, ticket: &ExportTicket) -> Result<Option<PathBuf>, String> {
        self.save_while(path, &|| ticket.is_current())
    }
    pub(crate) fn save_while(
        &self,
        path: PathBuf,
        is_current: &impl Fn() -> bool,
    ) -> Result<Option<PathBuf>, String> {
        if !is_current() {
            return Ok(None);
        }
        let png = self.png()?;
        if !is_current() {
            return Ok(None);
        }
        let (mut temp, mut file) = TemporaryPng::create(&path).map_err(|e| {
            format!(
                "Couldn’t create the preview PNG: {e}. Destination: {}",
                path.display()
            )
        })?;
        file.write_all(&png)
            .and_then(|()| file.sync_all())
            .map_err(|e| format!("Couldn’t write the preview PNG: {e}"))?;
        drop(file);
        // Cancellation is best effort: check retirement immediately before
        // rename, without claiming an atomic close-versus-commit protocol. A
        // rename already in progress may commit after close/source replacement.
        // No late completion may update the closed or replaced view.
        if !is_current() {
            return Ok(None);
        }
        temp.commit(&path).map_err(|e| {
            format!(
                "Couldn’t replace the preview PNG: {e}. Try another path. Destination: {}",
                path.display()
            )
        })?;
        Ok(Some(path))
    }
}

// Keep this application-owned encoded output buffer fallible instead of
// implicitly growing a Vec through Write; encoder-internal allocations remain
// the encoder's responsibility and are not presented as a product size cap.
struct FalliblePngBuffer(Vec<u8>);
impl Write for FalliblePngBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.try_reserve(bytes.len()).map_err(io::Error::other)?;
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Same-directory create_new avoids clobbering an existing temporary file.
/// rename provides atomic replacement on platforms supporting it; on platforms
/// rejecting replacement it returns an error, preserving the selected old file.
struct TemporaryPng(Option<PathBuf>);
impl TemporaryPng {
    fn create(destination: &Path) -> io::Result<(Self, File)> {
        if destination.file_name().is_none() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Choose a file name",
            ));
        }
        let directory = destination
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        for _ in 0..32 {
            let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
            let path = directory.join(format!(
                ".mycela-preview-{}-{serial}.png.tmp",
                std::process::id()
            ));
            // Even a deliberately chosen temporary-looking name must stay
            // untouched until the final rename, including for relative paths.
            if path.file_name() == destination.file_name() {
                continue;
            }
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => return Ok((Self(Some(path)), file)),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "Preview temporary names are in use",
        ))
    }
    fn commit(&mut self, destination: &Path) -> io::Result<()> {
        let path = self
            .0
            .as_ref()
            .ok_or_else(|| io::Error::other("PNG temporary file was already committed"))?;
        std::fs::rename(path, destination)?;
        self.0 = None;
        Ok(())
    }
}
impl Drop for TemporaryPng {
    fn drop(&mut self) {
        if let Some(path) = &self.0 {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[derive(Clone)]
struct ExportTicket {
    active: Arc<AtomicBool>,
    generation: Arc<AtomicU64>,
    epoch: u64,
}
impl ExportTicket {
    fn is_current(&self) -> bool {
        self.active.load(Ordering::Acquire) && self.generation.load(Ordering::Acquire) == self.epoch
    }
}
#[derive(Clone, Copy)]
enum Phase {
    Choosing,
    Writing,
    ChoosingImage,
    Rendering,
}
enum Feedback {
    Saved(String),
    Error(String),
}
#[derive(Default)]
pub(super) struct PreviewExport {
    phase: Option<Phase>,
    feedback: Option<Feedback>,
    active: Option<Arc<AtomicBool>>,
    task: Option<Task<()>>,
    image_size: Option<(i32, i32)>,
    progress: Option<String>,
}
impl PreviewExport {
    pub(super) fn is_busy(&self) -> bool {
        self.phase.is_some()
    }
    pub(super) fn status(&self) -> Option<&str> {
        match self.phase {
            Some(Phase::Choosing) => Some("Choose where to save the preview"),
            Some(Phase::Writing) if self.image_size.is_none() => Some("Exporting preview…"),
            Some(Phase::ChoosingImage | Phase::Rendering | Phase::Writing) => {
                self.progress.as_deref()
            }
            None => self.feedback.as_ref().map(|v| match v {
                Feedback::Saved(s) | Feedback::Error(s) => s.as_str(),
            }),
        }
    }
    pub(super) fn is_error(&self) -> bool {
        self.phase.is_none() && matches!(self.feedback, Some(Feedback::Error(_)))
    }
    pub(super) fn retire(&mut self) {
        let choosing = matches!(self.phase, Some(Phase::Choosing | Phase::ChoosingImage));
        if let Some(active) = self.active.take() {
            active.store(false, Ordering::Release);
        }
        // A picker that is already open owns modality until it returns. Retire
        // its ticket now, but keep the await that releases modal ownership.
        if !choosing {
            self.task.take();
        }
        self.phase = None;
        self.feedback = None;
        self.image_size = None;
        self.progress = None;
    }
    pub(super) fn cancel(&mut self) {
        if self.image_size.is_none() {
            return;
        }
        let choosing = matches!(self.phase, Some(Phase::ChoosingImage));
        if let Some(active) = self.active.take() {
            active.store(false, Ordering::Release);
        }
        // A native picker owns modal state until it actually returns. Keep its
        // await alive on cancellation so it releases that ownership honestly.
        if !choosing {
            self.task.take();
        }
        self.phase = None;
        self.image_size = None;
        self.progress = None;
        self.feedback = Some(Feedback::Saved("Export cancelled.".into()));
    }
    pub(super) fn can_cancel(&self) -> bool {
        self.is_busy() && self.image_size.is_some()
    }
    fn owns(&self, ticket: &ExportTicket) -> bool {
        ticket.is_current()
            && self
                .active
                .as_ref()
                .is_some_and(|a| Arc::ptr_eq(a, &ticket.active))
    }
    fn finish(&mut self, ticket: &ExportTicket, result: Result<Option<PathBuf>, String>) {
        if !self.owns(ticket) {
            return;
        }
        self.phase = None;
        self.progress = None;
        let image_size = self.image_size.take();
        ticket.active.store(false, Ordering::Release);
        self.active = None;
        match result {
            Ok(Some(path)) => {
                let message = match image_size {
                    Some((w, h)) => format!("Saved click-time {w} × {h} PNG: {}", path.display()),
                    None => format!(
                        "Saved click-time {LIVE_PREVIEW_SIZE} × {LIVE_PREVIEW_SIZE} preview: {}",
                        path.display()
                    ),
                };
                self.feedback = Some(Feedback::Saved(message))
            }
            Ok(None) => {} // Cancelling leaves the previous feedback intact.
            Err(error) => self.feedback = Some(Feedback::Error(error)),
        }
    }
}

impl SolidColorView {
    pub fn is_exporting_preview(&self) -> bool {
        self.preview_export.is_busy()
    }
    pub fn preview_export_status(&self) -> Option<&str> {
        self.preview_export.status()
    }
    pub(super) fn open_export_image(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.opening
            || self.pending
            || self.preview_export.is_busy()
            || self.runtime.is_dialog_visible()
        {
            return;
        }
        let time = self.sample_time(cx);
        let map = self.preview_selection.label().to_string();
        let description = format!(
            "{map} · time {time}. Export uses committed values; unapplied edits stay in the editor."
        );
        let settings = cx.new(|cx| ExportSettings::new(description, window, cx));
        let owner = cx.weak_entity();
        let generation = self.generation.load(Ordering::Acquire);
        let runtime = Arc::clone(&self.runtime);
        self.runtime.set_dialog_visible(true);
        window.open_dialog(cx, move |dialog, _, _| {
            let settings = settings.clone(); let confirm_settings = settings.clone();
            let confirm_owner = owner.clone(); let close_owner = owner.clone(); let close_runtime = Arc::clone(&runtime);
            dialog.title("Export image").child(settings)
                .footer(DialogFooter::new()
                    .child(DialogClose::new().child(Button::new("cancel").label("Cancel")))
                    .child(DialogAction::new().child(Button::new("ok").primary().label("Export PNG…"))))
                .on_ok(move |_, _, cx| {
                    let dimensions = confirm_settings.read(cx).dimensions(cx);
                    let result = dimensions.and_then(|(width, height)| confirm_owner.update(cx, |view, cx| {
                        if view.generation.load(Ordering::Acquire) != generation { return Err("The texture changed. Close this dialog and choose Export image again.".into()); }
                        view.export_image(width, height, cx)
                    }).unwrap_or_else(|_| Err("The texture workspace closed.".into())));
                    match result { Ok(()) => true, Err(error) => { confirm_settings.update(cx, |view, cx| view.set_error(error, cx)); false } }
                })
                .on_close(move |_, _, cx| {
                    // Confirm opens the native picker before this standard dialog
                    // closes. Its modal ownership must survive that handoff.
                    let choosing = close_owner.read_with(cx, |view, _| matches!(view.preview_export.phase, Some(Phase::ChoosingImage))).unwrap_or(false);
                    close_runtime.set_dialog_visible(choosing);
                })
        });
    }
    #[cfg(test)]
    fn capture_export_job(
        &self,
        width: i32,
        height: i32,
        cx: &App,
    ) -> Result<ExportRenderJob, String> {
        let filters = self.filters.read(cx).pipeline();
        let invert = self.invert.as_ref().map(|invert| invert.model());
        // These engine capture APIs read committed fields without callbacks,
        // lazy construction or live cache mutation. The job owns detached frames.
        let result = match self.source {
            TextureSourceKind::SolidColor => ExportRenderJob::capture_at_time(
                ExportSource::FinalSolid(&self.node),
                invert.as_ref(),
                &filters,
                self.preview_selection.clone(),
                width,
                height,
                self.sample_time(cx),
            ),
            TextureSourceKind::Polygon => {
                let polygon = self
                    .active_polygon()
                    .ok_or("The Polygon source is unavailable.")?;
                let node = polygon.read(cx).node();
                ExportRenderJob::capture_at_time(
                    ExportSource::Graph {
                        node: &node,
                        camera: None,
                        time: self.sample_time(cx),
                    },
                    invert.as_ref(),
                    &filters,
                    self.preview_selection.clone(),
                    width,
                    height,
                    self.sample_time(cx),
                )
            }
            source if source.scene_kind().is_some() => {
                let scene = self
                    .active_scene()
                    .ok_or("The scene source is unavailable.")?;
                let node = scene.read(cx).node();
                let camera = self.scene_camera.read(cx).camera();
                ExportRenderJob::capture_at_time(
                    ExportSource::Graph {
                        node: &node,
                        camera: Some(&camera),
                        time: self.sample_time(cx),
                    },
                    invert.as_ref(),
                    &filters,
                    self.preview_selection.clone(),
                    width,
                    height,
                    self.sample_time(cx),
                )
            }
            _ => {
                let generator = self
                    .active_generator()
                    .ok_or("The generator is unavailable.")?;
                let gradient = generator.read(cx);
                let model = gradient.model();
                ExportRenderJob::capture_at_time(
                    ExportSource::Gradient {
                        model: &model,
                        time: gradient.preview_time(),
                    },
                    invert.as_ref(),
                    &filters,
                    self.preview_selection.clone(),
                    width,
                    height,
                    self.sample_time(cx),
                )
            }
        };
        result.map_err(|e| e.to_string())
    }
    pub(super) fn freeze_export_job(
        &self,
        width: i32,
        height: i32,
        cx: &App,
    ) -> Result<crate::export_render::PendingExportRenderJob, String> {
        let filters = self.filters.read(cx).pipeline();
        let invert = self.invert.as_ref().map(|invert| invert.model());
        // These engine capture APIs read committed fields without callbacks,
        // lazy construction or live cache mutation. The job owns detached frames.
        let result = match self.source {
            TextureSourceKind::SolidColor => ExportRenderJob::freeze_at_time(
                ExportSource::FinalSolid(&self.node),
                invert.as_ref(),
                &filters,
                self.preview_selection.clone(),
                width,
                height,
                self.sample_time(cx),
            ),
            TextureSourceKind::Polygon => {
                let polygon = self
                    .active_polygon()
                    .ok_or("The Polygon source is unavailable.")?;
                let node = polygon.read(cx).node();
                ExportRenderJob::freeze_at_time(
                    ExportSource::Graph {
                        node: &node,
                        camera: None,
                        time: self.sample_time(cx),
                    },
                    invert.as_ref(),
                    &filters,
                    self.preview_selection.clone(),
                    width,
                    height,
                    self.sample_time(cx),
                )
            }
            source if source.scene_kind().is_some() => {
                let scene = self
                    .active_scene()
                    .ok_or("The scene source is unavailable.")?;
                let node = scene.read(cx).node();
                let camera = self.scene_camera.read(cx).camera();
                ExportRenderJob::freeze_at_time(
                    ExportSource::Graph {
                        node: &node,
                        camera: Some(&camera),
                        time: self.sample_time(cx),
                    },
                    invert.as_ref(),
                    &filters,
                    self.preview_selection.clone(),
                    width,
                    height,
                    self.sample_time(cx),
                )
            }
            _ => {
                let generator = self
                    .active_generator()
                    .ok_or("The generator is unavailable.")?;
                let gradient = generator.read(cx);
                let model = gradient.model();
                ExportRenderJob::freeze_at_time(
                    ExportSource::Gradient {
                        model: &model,
                        time: gradient.preview_time(),
                    },
                    invert.as_ref(),
                    &filters,
                    self.preview_selection.clone(),
                    width,
                    height,
                    self.sample_time(cx),
                )
            }
        };
        result.map_err(|e| e.to_string())
    }
    fn export_image(
        &mut self,
        width: i32,
        height: i32,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if self.opening || self.preview_export.is_busy() {
            return Err("An export or file operation is already in progress.".into());
        }
        checked_dimensions(width, height)?;
        let job = self.freeze_export_job(width, height, cx)?;
        let directory = self
            .imported_path()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .map(Ok)
            .unwrap_or_else(std::env::current_dir)
            .map_err(|e| format!("Couldn’t choose a starting folder: {e}"))?;
        let ticket = ExportTicket {
            active: Arc::new(AtomicBool::new(true)),
            generation: Arc::clone(&self.generation),
            epoch: self.generation.load(Ordering::Acquire),
        };
        self.preview_export.active = Some(Arc::clone(&ticket.active));
        self.preview_export.phase = Some(Phase::ChoosingImage);
        self.preview_export.image_size = Some((width, height));
        self.preview_export.progress =
            Some(format!("Choose where to save the {width} × {height} PNG"));
        self.runtime.set_dialog_visible(true);
        let prompt = cx.prompt_for_new_path(&directory, Some("mycela-image.png"));
        let runtime = Arc::downgrade(&self.runtime);
        let window_handle = self.dispatcher.window;
        let background = cx.background_executor().clone();
        // Start materialization immediately from the click-time pending frame,
        // concurrently with the picker. Cancelling can only retire owned work.
        let materialize = background.spawn(async move { job.materialize() });
        self.preview_export.task = Some(cx.spawn(async move |owner, cx| {
            let selected = prompt.await;
            if let Some(runtime) = runtime.upgrade() { runtime.set_dialog_visible(false); }
            if !cx.update_window(window_handle, |_, _, cx| owner.read_with(cx, |view, _| view.preview_export.owns(&ticket)))
                .is_ok_and(|r| r.is_ok_and(|current| current)) { return; }
            let result = match selected {
                Ok(Ok(Some(path))) => {
                    let _ = owner.update(cx, |view, cx| {
                        view.preview_export.phase = Some(Phase::Rendering);
                        view.preview_export.progress = Some(format!("Rendering and saving {width} × {height} PNG… Cancel stops between stages.")); cx.notify();
                    });
                    let worker_ticket = ticket.clone();
                    let job = materialize.await.map_err(|e| format!("Couldn’t capture the PNG: {e}"));
                    background.spawn(async move {
                        let job = job?;
                        let bitmap = match job.render(|| !worker_ticket.is_current()) {
                            Ok(bitmap) => bitmap,
                            Err(error) if error.is_cancelled() => return Ok(None),
                            Err(error) => return Err(format!("Couldn’t render the PNG: {error}")),
                        };
                        if !worker_ticket.is_current() { return Ok(None); }
                        PreviewPixels::capture_rendered(&bitmap)?.save(path, &worker_ticket)
                    }).await
                }
                Ok(Ok(None)) => Ok(None),
                Ok(Err(error)) => Err(format!("Couldn’t open the save dialog: {error}")),
                Err(_) => Err("The save dialog closed before returning a result.".into()),
            };
            let _ = cx.update_window(window_handle, |_, _, cx| owner.update(cx, |view, cx| {
                view.preview_export.finish(&ticket, result); cx.notify();
            }));
        }));
        cx.notify();
        Ok(())
    }
    pub(super) fn export_preview(&mut self, cx: &mut Context<Self>) {
        if self.opening || self.preview_export.is_busy() || self.runtime.is_dialog_visible() {
            return;
        }
        let snapshot = self
            .bitmap
            .as_ref()
            .ok_or_else(|| "There is no applied preview to export.".to_string())
            .and_then(PreviewPixels::capture);
        let snapshot = match snapshot {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.preview_export.feedback = Some(Feedback::Error(error));
                cx.notify();
                return;
            }
        };
        let directory = match self.imported_path().and_then(Path::parent) {
            Some(directory) => directory.to_path_buf(),
            None => match std::env::current_dir() {
                Ok(directory) => directory,
                Err(error) => {
                    self.preview_export.feedback = Some(Feedback::Error(format!(
                        "Couldn’t choose a starting folder: {error}"
                    )));
                    cx.notify();
                    return;
                }
            },
        };
        let ticket = ExportTicket {
            active: Arc::new(AtomicBool::new(true)),
            generation: Arc::clone(&self.generation),
            epoch: self.generation.load(Ordering::Acquire),
        };
        self.preview_export.active = Some(Arc::clone(&ticket.active));
        self.preview_export.phase = Some(Phase::Choosing);
        self.runtime.set_dialog_visible(true);
        let prompt = cx.prompt_for_new_path(&directory, Some("mycela-preview.png"));
        let runtime = Arc::downgrade(&self.runtime);
        let window_handle = self.dispatcher.window;
        let background = cx.background_executor().clone();
        self.preview_export.task = Some(cx.spawn(async move |owner, cx| {
            let selected = prompt.await;
            if let Some(runtime) = runtime.upgrade() {
                runtime.set_dialog_visible(false);
            }
            // Check the real window, weak entity and original request before
            // performing any I/O; a retained editor is not a live window.
            if !cx
                .update_window(window_handle, |_, _, cx| {
                    owner.read_with(cx, |view, _| view.preview_export.owns(&ticket))
                })
                .is_ok_and(|r| r.is_ok_and(|current| current))
            {
                return;
            }
            let result = match selected {
                Ok(Ok(Some(path))) => {
                    let _ = owner.update(cx, |view, cx| {
                        view.preview_export.phase = Some(Phase::Writing);
                        cx.notify();
                    });
                    let writer_ticket = ticket.clone();
                    background
                        .spawn(async move { snapshot.save(path, &writer_ticket) })
                        .await
                }
                Ok(Ok(None)) => Ok(None),
                Ok(Err(error)) => Err(format!("Couldn’t open the save dialog: {error}")),
                Err(_) => Err("The save dialog closed before returning a result.".into()),
            };
            // Source replacement and window closure retire the original request.
            let _ = cx.update_window(window_handle, |_, _, cx| {
                owner.update(cx, |view, cx| {
                    view.preview_export.finish(&ticket, result);
                    cx.notify();
                })
            });
        }));
        cx.notify();
    }
}

#[cfg(test)]
#[path = "preview_export_tests.rs"]
mod tests;
