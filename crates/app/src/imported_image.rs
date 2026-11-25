//! Encoded file candidates belong to this operation until the owned preview
//! succeeds. The current source and its InputState drafts remain untouched.
use super::*;
use gpui_kit::component::spinner::Spinner;
use mycela_engine::{ByteBuffer, ImageGenerator, ImageGeneratorKind, ImageGeneratorParameterKind};
use std::{fs::File, io::Read, path::PathBuf};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum ImportPhase {
    #[default]
    Idle,
    Choosing,
    Reading,
    Rendering,
}
struct FileInfo {
    path: PathBuf,
    dimensions: [i32; 2],
    byte_count: usize,
}
struct Candidate {
    model: ImageGenerator<ForegroundError>,
    info: FileInfo,
    time: f64,
}
#[derive(Default)]
pub(super) struct ImageImport {
    serial: Arc<AtomicU64>,
    phase: ImportPhase,
    candidate: Option<Candidate>,
    loaded: Option<FileInfo>,
}
impl ImageImport {
    pub(super) fn retire(&mut self) {
        self.serial.fetch_add(1, Ordering::AcqRel);
        self.phase = ImportPhase::Idle;
        self.candidate.take();
    }
    pub(super) fn reset(&mut self) {
        self.retire();
        self.loaded.take();
    }
    pub(super) fn is_busy(&self) -> bool {
        self.phase != ImportPhase::Idle
    }
    pub(super) fn status(&self) -> &'static str {
        match self.phase {
            ImportPhase::Choosing => "Choosing image…",
            ImportPhase::Reading => "Reading image…",
            ImportPhase::Rendering => "Rendering image…",
            ImportPhase::Idle => "Image loaded",
        }
    }
}
impl Drop for ImageImport {
    fn drop(&mut self) {
        self.retire();
    }
}
#[derive(Clone, Copy)]
struct Settings {
    size: [f64; 2],
    location: [f64; 2],
    square: i32,
    smoothing: i32,
}
impl Settings {
    fn capture(model: Option<ImageGenerator<ForegroundError>>) -> Result<Option<Self>, String> {
        match model {
            Some(model) => Ok(Some(Self {
                size: model
                    .parameter(ImageGeneratorParameterKind::Size)
                    .pair()
                    .map_err(|e| e.to_string())?,
                location: model
                    .parameter(ImageGeneratorParameterKind::Location)
                    .pair()
                    .map_err(|e| e.to_string())?,
                square: model
                    .parameter(ImageGeneratorParameterKind::ForceSquareRatio)
                    .index()
                    .map_err(|e| e.to_string())?,
                smoothing: model
                    .parameter(ImageGeneratorParameterKind::UpsizeSmoothing)
                    .index()
                    .map_err(|e| e.to_string())?,
            })),
            None => Ok(None),
        }
    }
}
fn read_candidate(
    path: PathBuf,
    settings: Option<Settings>,
    time: f64,
    is_current: impl Fn() -> bool,
) -> Result<Option<Candidate>, String> {
    if !is_current() {
        return Ok(None);
    }
    let file = File::open(&path).map_err(|e| format!("Couldn’t open the image: {e}"))?;
    let metadata = file
        .metadata()
        .map_err(|e| format!("Couldn’t inspect the image: {e}"))?;
    if !metadata.is_file() {
        return Err("Choose a regular image file.".into());
    }
    // Source arrays are Int32-addressed. This is the actual encoded owner seam,
    // not a fixture-size cap, and all allocation/IO stays on the worker.
    let length = usize::try_from(metadata.len())
        .ok()
        .filter(|n| *n <= i32::MAX as usize)
        .ok_or_else(|| {
            "The encoded image exceeds the engine’s addressable byte array.".to_owned()
        })?;
    let mut encoded = Vec::new();
    encoded
        .try_reserve_exact(length)
        .map_err(|e| format!("Couldn’t allocate image bytes: {e}"))?;
    file.take(i32::MAX as u64 + 1)
        .read_to_end(&mut encoded)
        .map_err(|e| format!("Couldn’t read the image: {e}"))?;
    if encoded.len() > i32::MAX as usize {
        return Err("The image grew beyond the engine’s addressable byte array.".into());
    }
    if !is_current() {
        return Ok(None);
    }
    let byte_count = encoded.len();
    let model =
        ImageGenerator::with_kind(ImageGeneratorKind::ImportedImage).map_err(|e| e.to_string())?;
    let custom = model
        .parameter(ImageGeneratorParameterKind::Image)
        .custom_value()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "The imported image property has no source.".to_owned())?;
    let payload = custom.imported_image().map_err(|e| e.to_string())?;
    payload
        .set_bytes(Some(ByteBuffer::new(encoded)))
        .map_err(|e| format!("Couldn’t retain image bytes: {e}"))?;
    // Genuine mn lazy decoding validates only this fresh candidate. No source
    // Map, cache setter, or file metadata supplies target pixel state.
    let image = payload
        .image()
        .map_err(|e| format!("Couldn’t decode the image: {e}"))?
        .ok_or_else(|| "The image contains no decodable pixels.".to_owned())?;
    let geometry = image.geometry();
    let dimensions = [geometry.width(), geometry.height()];
    if let Some(settings) = settings {
        model
            .parameter(ImageGeneratorParameterKind::Size)
            .set_pair(settings.size)
            .map_err(|e| e.to_string())?;
        model
            .parameter(ImageGeneratorParameterKind::Location)
            .set_pair(settings.location)
            .map_err(|e| e.to_string())?;
        model
            .parameter(ImageGeneratorParameterKind::ForceSquareRatio)
            .set_index(settings.square)
            .map_err(|e| e.to_string())?;
        model
            .parameter(ImageGeneratorParameterKind::UpsizeSmoothing)
            .set_index(settings.smoothing)
            .map_err(|e| e.to_string())?;
    }
    if !is_current() {
        return Ok(None);
    }
    Ok(Some(Candidate {
        model,
        info: FileInfo {
            path,
            dimensions,
            byte_count,
        },
        time,
    }))
}
impl SolidColorView {
    pub fn imported_image_path(&self) -> Option<&Path> {
        self.image_import
            .loaded
            .as_ref()
            .map(|info| info.path.as_path())
    }
    pub fn imported_image_dimensions(&self) -> Option<[i32; 2]> {
        self.image_import
            .loaded
            .as_ref()
            .map(|info| info.dimensions)
    }
    pub(super) fn import_candidate_model(&self) -> Option<(ImageGenerator<ForegroundError>, f64)> {
        self.image_import
            .candidate
            .as_ref()
            .map(|candidate| (candidate.model.clone(), candidate.time))
    }
    pub(super) fn cancel_image_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.image_import.is_busy() {
            self.image_import.retire();
            self.generator_preview.retire();
            self.activate_generators(window, cx);
        }
    }
    pub(super) fn fail_image_import(
        &mut self,
        error: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.image_import.retire();
        self.pipeline_error = None;
        self.error = Some(error);
        self.activate_generators(window, cx);
    }
    /// Called only after the candidate's owned preview and binding preparation
    /// succeed. Reusing the view preserves dirty text without sharing old models.
    pub(super) fn commit_image_candidate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(candidate) = self.image_import.candidate.take() else {
            return;
        };
        if let Some(editor) = self.generator_view(TextureSourceKind::ImportedImage) {
            editor.update(cx, |view, cx| {
                view.replace_imported_model(candidate.model, window, cx)
            });
        } else {
            let editor =
                cx.new(|cx| GradientSourceView::new(candidate.model, &self.runtime, window, cx));
            self.procedural
                .insert(TextureSourceKind::ImportedImage, editor);
        }
        if let Some(editor) = self.generator_view(TextureSourceKind::ImportedImage) {
            editor.update(cx, |view, cx| {
                view.set_preview_time(candidate.time, window, cx)
            });
        }
        self.image_import.loaded = Some(candidate.info);
        self.image_import.phase = ImportPhase::Idle;
        self.error = None;
    }
    pub(super) fn open_image(&mut self, cx: &mut Context<Self>) {
        if self.opening
            || self.pending
            || self.image_import.is_busy()
            || self.runtime.is_dialog_visible()
        {
            return;
        }
        let settings = Settings::capture(
            self.generator_view(TextureSourceKind::ImportedImage)
                .map(|view| view.read(cx).model()),
        );
        let settings = match settings {
            Ok(settings) => settings,
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        self.image_import.retire();
        self.generator_preview.retire();
        self.preview_export.retire();
        self.runtime.retire_pending_input();
        self.filters
            .update(cx, |view, cx| view.retire_input_work(cx));
        if let Some(generator) = self.active_generator() {
            generator.update(cx, |view, cx| {
                view.close();
                cx.notify();
            });
        }
        // Queued former-source edits/exports cannot mutate or publish this import.
        self.generation.fetch_add(1, Ordering::AcqRel);
        let epoch = self.generation.load(Ordering::Acquire);
        let serial = self.image_import.serial.clone();
        let request = serial.load(Ordering::Acquire);
        let time = self.sample_time(cx);
        self.image_import.phase = ImportPhase::Choosing;
        self.runtime.set_dialog_visible(true);
        let prompt = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Import image".into()),
        });
        let background = cx.background_executor().clone();
        let window_handle = self.dispatcher.window;
        let runtime = Arc::downgrade(&self.runtime);
        cx.notify();
        cx.spawn(async move |owner, cx| {
            let selected = prompt.await;
            if let Some(runtime) = runtime.upgrade() {
                runtime.set_dialog_visible(false);
            }
            if serial.load(Ordering::Acquire) != request {
                return;
            }
            let alive = cx
                .update_window(window_handle, |_, _, cx| {
                    owner.read_with(cx, |view, _| {
                        view.generation.load(Ordering::Acquire) == epoch
                    })
                })
                .is_ok_and(|value| value.is_ok_and(|current| current));
            if !alive {
                return;
            }
            let path = match selected {
                Ok(Ok(Some(mut paths))) if paths.len() == 1 => Ok(paths.pop()),
                Ok(Ok(None)) => Ok(None),
                Ok(Ok(Some(_))) => Err("Choose one image file.".to_owned()),
                Ok(Err(error)) => Err(format!("Couldn’t open the file picker: {error}")),
                Err(_) => Err("The file picker closed before returning a result.".into()),
            };
            let result = match path {
                Ok(Some(path)) => {
                    let _ = cx.update_window(window_handle, |_, _, cx| {
                        owner.update(cx, |view, cx| {
                            if serial.load(Ordering::Acquire) == request {
                                view.image_import.phase = ImportPhase::Reading;
                                cx.notify();
                            }
                        })
                    });
                    let worker_serial = serial.clone();
                    background
                        .spawn(async move {
                            read_candidate(path, settings, time, || {
                                worker_serial.load(Ordering::Acquire) == request
                            })
                        })
                        .await
                }
                Ok(None) => Ok(None),
                Err(error) => Err(error),
            };
            if serial.load(Ordering::Acquire) != request {
                return;
            }
            let _ = cx.update_window(window_handle, |_, window, cx| {
                owner.update(cx, |view, cx| {
                    if serial.load(Ordering::Acquire) != request
                        || view.generation.load(Ordering::Acquire) != epoch
                    {
                        return;
                    }
                    match result {
                        Ok(Some(candidate)) => {
                            view.image_import.candidate = Some(candidate);
                            view.image_import.phase = ImportPhase::Rendering;
                            // This path must use real detached RenderCaptureSession
                            // support. Its explicit capability error is never bypassed.
                            if let Err(error) = view.request_generator_preview(
                                TextureSourceKind::ImportedImage,
                                true,
                                cx,
                            ) {
                                view.fail_image_import(error.to_string(), window, cx);
                            }
                        }
                        Ok(None) => {
                            view.image_import.retire();
                            view.activate_generators(window, cx);
                        }
                        Err(error) => view.fail_image_import(error, window, cx),
                    }
                    cx.notify();
                })
            });
        })
        .detach();
    }
    pub(super) fn render_imported_file(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let info = self.image_import.loaded.as_ref();
        let name = info
            .and_then(|info| info.path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Choose an image to begin".into());
        div()
            .v_flex()
            .gap_2()
            .child(div().text_sm().font_medium().child("Image file"))
            .child(
                div()
                    .id("imported-image-file")
                    .test_support()
                    .aria_label(name.clone())
                    .min_w_0()
                    .truncate()
                    .text_sm()
                    .child(name),
            )
            .when_some(info, |view, info| {
                view.child(
                    div()
                        .id("imported-image-metadata")
                        .test_support()
                        .aria_label(format!(
                            "{} × {} pixels · {} bytes",
                            info.dimensions[0], info.dimensions[1], info.byte_count
                        ))
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!(
                            "{} × {} pixels · {} bytes",
                            info.dimensions[0], info.dimensions[1], info.byte_count
                        )),
                )
            })
            .child(
                Button::new("replace-imported-image")
                    .label(if info.is_some() {
                        "Replace image…"
                    } else {
                        "Import image…"
                    })
                    .disabled(
                        self.image_import.is_busy()
                            || self.opening
                            || self.pending
                            || self.runtime.is_dialog_visible(),
                    )
                    .on_click(cx.listener(|view, _, _, cx| view.open_image(cx))),
            )
            .when(self.image_import.is_busy(), |view| {
                view.child(
                    div().h_flex().gap_2().child(Spinner::new().small()).child(
                        div()
                            .id("imported-image-progress")
                            .test_support()
                            .role(gpui_kit::Role::Status)
                            .aria_label(self.image_import.status())
                            .text_sm()
                            .child(self.image_import.status()),
                    ),
                )
            })
    }
}
