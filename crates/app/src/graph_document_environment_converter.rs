//! Local preview converter selection for the private Environment editor.
use super::{
    AppContext, Button, Context, Entity, ForegroundError, Path, PathBuf, PopupMenuItem, Ticket,
    model,
};
use crate::{export_render::generator_execution, solid_color::map_preview};
use gpui_kit::base::Disableable;
use gpui_kit::component::{Sizable, menu::DropdownMenu};
use gpui_kit::{IntoElement, RenderImage};
use mycela_engine::{
    EnvironmentImageProjection, EnvironmentPreviewConverter, EnvironmentRecord,
    EnvironmentResourceLibrary, SharedImageMap,
};
use std::{
    fs,
    io::{Cursor, Read},
    sync::Arc,
};

use super::PreferenceFile;

const PREFERENCE: PreferenceFile = PreferenceFile::EnvironmentPreviewConverter;
const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;

pub(super) struct ProjectedPreview {
    pub(super) image: Arc<RenderImage>,
    pub(super) width: i32,
    pub(super) height: i32,
}

struct Choice {
    path: PathBuf,
    converter: Arc<EnvironmentPreviewConverter>,
}

pub(super) struct CompositePreview {
    map: Arc<EnvironmentRecord>,
    library: EnvironmentResourceLibrary,
    paths: Vec<PathBuf>,
    choice: Option<Choice>,
    native: Option<SharedImageMap>,
    projected: Option<ProjectedPreview>,
    loading: Option<Ticket>,
    rendering: Option<Ticket>,
    picking: bool,
    error: Option<String>,
}

impl CompositePreview {
    pub(super) fn new(
        map: Arc<EnvironmentRecord>,
        library: EnvironmentResourceLibrary,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut preview = Self {
            map,
            library,
            paths: Vec::new(),
            choice: None,
            native: None,
            projected: None,
            loading: None,
            rendering: None,
            picking: false,
            error: None,
        };
        if let Some(path) = PREFERENCE.read_path(cx) {
            preview.select(path, false, cx);
        }
        preview
    }

    pub(super) fn update_map(&mut self, map: &Arc<EnvironmentRecord>, cx: &mut Context<Self>) {
        if !Arc::ptr_eq(&self.map, map) {
            self.map = map.clone();
            self.native = None;
            self.render_projection(cx);
        }
    }

    pub(super) fn projected(&self) -> Option<&ProjectedPreview> {
        self.projected.as_ref()
    }

    pub(super) fn status(&self) -> Option<String> {
        if self.loading.is_some() {
            Some("Loading preview converter…".into())
        } else if self.rendering.is_some() {
            Some("Rendering converted preview…".into())
        } else {
            self.projected().map(|preview| {
                format!("{} × {} · converted preview", preview.width, preview.height)
            })
        }
    }

    pub(super) fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub(super) fn picker(&self, state: &Entity<Self>) -> impl IntoElement + use<> {
        let current = self.choice.as_ref().map(|choice| choice.path.clone());
        let label = current.as_deref().map_or_else(
            || "Preview · Working map".into(),
            |path| format!("Preview · {}", converter_name(path)),
        );
        let state = state.downgrade();
        let paths = self.paths.clone();
        Button::new("environment-preview-converter")
            .small()
            .label(label)
            .disabled(self.picking)
            .dropdown_menu(move |menu, _, _| {
                let reset = state.clone();
                let mut menu = menu.item(
                    PopupMenuItem::new("Working map")
                        .checked(current.is_none())
                        .on_click(move |_, _, cx| {
                            let _ = reset.update(cx, |view, cx| view.reset(cx));
                        }),
                );
                for path in &paths {
                    let selected = current.as_ref() == Some(path);
                    let path = path.clone();
                    let owner = state.clone();
                    menu = menu.item(
                        PopupMenuItem::new(converter_name(&path))
                            .checked(selected)
                            .on_click(move |_, _, cx| {
                                let _ = owner
                                    .update(cx, |view, cx| view.select(path.clone(), true, cx));
                            }),
                    );
                }
                let browse = state.clone();
                menu.item(PopupMenuItem::new("Open preview converter…").on_click(
                    move |_, _, cx| {
                        let _ = browse.update(cx, |view, cx| view.browse(cx));
                    },
                ))
                .scrollable(true)
            })
    }

    fn browse(&mut self, cx: &mut Context<Self>) {
        self.picking = true;
        self.error = None;
        let prompt = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Open preview converter".into()),
        });
        cx.spawn(async move |owner, cx| {
            let result = prompt.await;
            let _ = owner.update(cx, |view, cx| {
                view.picking = false;
                match result {
                    Ok(Ok(Some(paths))) => match paths.as_slice() {
                        [path] => view.select(path.clone(), true, cx),
                        _ => view.error = Some("Choose a single preview converter.".into()),
                    },
                    Ok(Ok(None)) => {}
                    Ok(Err(error)) => {
                        view.error = Some(format!("Couldn’t open the file picker. {error}"))
                    }
                    Err(_) => {
                        view.error = Some("The file picker closed without a selection.".into())
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn select(&mut self, path: PathBuf, save: bool, cx: &mut Context<Self>) {
        retire(&mut self.loading);
        self.error = None;
        let ticket = Ticket::new();
        self.loading = Some(ticket.clone());
        let task =
            cx.background_spawn(async move { load(&path).map(|converter| (path, converter)) });
        cx.spawn(async move |owner, cx| {
            let result = task.await;
            let _ = owner.update(cx, |view, cx| {
                if !ticket.is_current() {
                    return;
                }
                view.loading = None;
                match result {
                    Ok((path, (converter, paths))) => {
                        view.paths = paths;
                        if save {
                            view.error = PREFERENCE.write_path(&path, cx).err().map(|error| {
                                format!("Couldn’t remember the preview converter. {error}")
                            });
                        }
                        view.choice = Some(Choice {
                            path,
                            converter: Arc::new(converter),
                        });
                        view.render_projection(cx);
                    }
                    Err(error) => {
                        view.error = Some(format!("Couldn’t open the preview converter. {error}"))
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn reset(&mut self, cx: &mut Context<Self>) {
        retire(&mut self.loading);
        retire(&mut self.rendering);
        self.choice = None;
        self.projected = None;
        self.error = PREFERENCE
            .write(b"", cx)
            .err()
            .map(|error| format!("Couldn’t remember the preview selection. {error}"));
        cx.notify();
    }

    fn render_projection(&mut self, cx: &mut Context<Self>) {
        retire(&mut self.rendering);
        self.projected = None;
        let Some(choice) = &self.choice else {
            return;
        };
        let converter = choice.converter.clone();
        let map = self.map.clone();
        let library = self.library.clone();
        let cached = self.native.clone();
        let ticket = Ticket::new();
        self.rendering = Some(ticket.clone());
        let work = ticket.clone();
        let task = cx.background_spawn(async move {
            let cancelled = || !work.is_current();
            let native = match cached {
                Some(map) => map,
                None => map
                    .render_projected::<ForegroundError>(
                        336,
                        EnvironmentImageProjection::LatitudeLongitude,
                        generator_execution(),
                        Some(&library),
                        &cancelled,
                    )
                    .map_err(model::failure)?,
            };
            let pixels = converter
                .project::<ForegroundError>(&native, &cancelled)
                .map_err(model::failure)?;
            let (_, image) =
                map_preview::project(&pixels, &map_preview::PreviewSelection::BaseColor)
                    .map_err(model::failure)?;
            let geometry = pixels.geometry();
            Ok::<_, String>((
                native,
                ProjectedPreview {
                    image,
                    width: geometry.width(),
                    height: geometry.height(),
                },
            ))
        });
        cx.spawn(async move |owner, cx| {
            let result = task.await;
            let _ = owner.update(cx, |view, cx| {
                if !ticket.is_current() {
                    return;
                }
                view.rendering = None;
                match result {
                    Ok((map, projected)) => {
                        view.native = Some(map);
                        view.projected = Some(projected);
                    }
                    Err(error) => {
                        view.error = Some(format!("Couldn’t render the converted preview. {error}"))
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

impl Drop for CompositePreview {
    fn drop(&mut self) {
        retire(&mut self.loading);
        retire(&mut self.rendering);
    }
}

fn retire(ticket: &mut Option<Ticket>) {
    if let Some(ticket) = ticket.take() {
        ticket.retire();
    }
}

fn converter_name(path: &Path) -> String {
    path.file_stem().map_or_else(
        || "Preview converter".into(),
        |stem| stem.to_string_lossy().into_owned(),
    )
}

fn is_converter(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("pconv"))
}

fn load(path: &Path) -> Result<(EnvironmentPreviewConverter, Vec<PathBuf>), String> {
    if !is_converter(path) || !path.is_file() {
        return Err("Choose a .pconv file.".into());
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(model::failure)?
        .take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(model::failure)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err("The preview converter exceeds 16 MiB.".into());
    }
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(model::failure)?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(1024);
    limits.max_image_height = Some(1024);
    limits.max_alloc = Some(MAX_FILE_BYTES);
    reader.limits(limits);
    let image = reader.decode().map_err(model::failure)?.into_rgb8();
    let converter = EnvironmentPreviewConverter::from_rgb8(
        image.width() as i32,
        image.height() as i32,
        image.as_raw(),
    )
    .map_err(model::failure)?;
    let mut paths = path
        .parent()
        .and_then(|folder| fs::read_dir(folder).ok())
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| is_converter(path) && path.is_file())
        .collect::<Vec<_>>();
    if !paths.iter().any(|entry| entry == path) {
        paths.push(path.to_owned());
    }
    paths.sort();
    Ok((converter, paths))
}

#[cfg(test)]
#[test]
fn environment_preview_loads_all_supplied_converter_bitmaps() {
    use mycela_engine::ImageGeometry;
    let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../genetica/Preview");
    let source = SharedImageMap::new(ImageGeometry::new(2, 2));
    source.set_constant(0, 0.75).unwrap();
    for name in [
        "Ball", "Cells", "Panorama", "Pattern", "Skull", "Star", "Text",
    ] {
        let (converter, catalogue) = load(&folder.join(format!("{name}.pconv"))).unwrap();
        assert_eq!(catalogue.len(), 7);
        let image = converter.project::<()>(&source, &|| false).unwrap();
        assert_eq!(image.geometry(), ImageGeometry::new(168, 168));
        assert_eq!(image.channel(0, true).unwrap().read(0).unwrap(), 0.75);
    }
}
