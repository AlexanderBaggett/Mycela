//! Standalone environment files use the selected map value and library.
use super::{
    GraphDocumentView, Ticket, Work, environment::Target, model,
    preferences::File as PreferenceFile,
};
#[path = "graph_document_environment_view.rs"]
mod settings;
use crate::{
    ForegroundError,
    export_render::generator_execution,
    solid_color::{
        map_preview::{PreviewSelection, project_bitmap},
        preview_export::PreviewPixels,
    },
};
use gpui_kit::component::{
    WindowExt,
    button::{Button, ButtonVariants},
    dialog::{DialogAction, DialogClose, DialogFooter},
};
use gpui_kit::{AppContext, Context, ParentElement, Window};
use image::{
    DynamicImage, ImageFormat, Rgb, RgbaImage,
    codecs::{hdr::HdrEncoder, jpeg::JpegEncoder},
};
use mycela_engine::{
    BinaryWriter, EnvironmentPreset, EnvironmentRecord, EnvironmentRenderSettings,
    EnvironmentResourceLibrary, PointwiseError, SharedImageMap,
};
use settings::ExportSettings;
pub(super) use settings::OutputPreferences;
use std::{
    io::{BufWriter, Seek, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Clone, Copy)]
enum FileKind {
    Preset,
    Image {
        render: EnvironmentRenderSettings,
        format: Format,
    },
}

impl FileKind {
    fn extension(self) -> &'static str {
        match self {
            Self::Preset => "emap",
            Self::Image { format, .. } => format.extension(),
        }
    }

    fn destination(self, mut path: PathBuf) -> Result<PathBuf, String> {
        let extension = self.extension();
        if path.extension().is_none() {
            path.set_extension(extension);
            if path.try_exists().map_err(model::failure)? {
                return Err(format!(
                    "Choose the full file name to replace {}.",
                    path.display()
                ));
            }
        }
        let matches = path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| {
                ext.eq_ignore_ascii_case(extension)
                    || extension == "jpg" && ext.eq_ignore_ascii_case("jpeg")
                    || extension == "tiff" && ext.eq_ignore_ascii_case("tif")
            });
        if matches {
            Ok(path)
        } else {
            Err(format!("Use a .{extension} file name for this format."))
        }
    }

    fn success(self, path: &Path) -> String {
        match self {
            Self::Preset => format!("Saved environment map to {}", path.display()),
            Self::Image {
                render:
                    EnvironmentRenderSettings {
                        width, projection, ..
                    },
                format,
            } => {
                let height = projection
                    .output_geometry(width)
                    .expect("validated export dimensions")
                    .height();
                format!(
                    "Saved {width} × {height} {} to {}",
                    format.label(),
                    path.display()
                )
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Format {
    Png,
    Jpeg,
    Tga,
    Tiff,
    Bmp,
    Pfm,
    Hdr,
}

impl Format {
    const ALL: [Self; 7] = [
        Self::Png,
        Self::Jpeg,
        Self::Tga,
        Self::Tiff,
        Self::Bmp,
        Self::Pfm,
        Self::Hdr,
    ];

    fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Tga => "tga",
            Self::Tiff => "tiff",
            Self::Bmp => "bmp",
            Self::Pfm => "pfm",
            Self::Hdr => "hdr",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Png => "PNG",
            Self::Jpeg => "JPEG",
            Self::Tga => "Targa",
            Self::Tiff => "TIFF",
            Self::Bmp => "BMP",
            Self::Pfm => "PFM",
            Self::Hdr => "Radiance HDR",
        }
    }
}

impl GraphDocumentView {
    pub(super) fn save_environment_map(&mut self, target: &Target, cx: &mut Context<Self>) {
        if self.is_busy() {
            return;
        }
        if let Err(error) = self.begin_environment_write(target, FileKind::Preset, cx) {
            self.error = Some(error);
            cx.notify();
        }
    }

    pub(super) fn open_environment_export(
        &mut self,
        target: &Target,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_busy() || !target.current(self) {
            return;
        }
        if let Err(error) = target.panel.validate() {
            self.error = Some(error);
            cx.notify();
            return;
        }
        self.pause_preview();
        self.modal = true;
        let preferences = OutputPreferences::current(cx);
        let settings = cx.new(|cx| {
            ExportSettings::new(
                target.panel.value().clone(),
                self.environment_library.clone(),
                preferences,
                window,
                cx,
            )
        });
        settings.update(cx, |view, cx| view.refresh_preview(false, cx));
        let owner = cx.weak_entity();
        let target = target.clone();
        window.open_dialog(cx, move |dialog, window, _| {
            let confirm = settings.clone();
            let close_settings = settings.clone();
            let close = owner.clone();
            let owner = owner.clone();
            let target = target.clone();
            dialog
                .width(window.rem_size() * 48.)
                .title("Export environment map")
                .child(settings.clone())
                .footer(
                    DialogFooter::new()
                        .child(DialogClose::new().child(Button::new("cancel").label("Cancel")))
                        .child(
                            DialogAction::new().child(Button::new("ok").primary().label("Export…")),
                        ),
                )
                .on_ok(move |_, _, cx| {
                    let Ok(request) = confirm.read(cx).request(cx) else {
                        return false;
                    };
                    let result = owner
                        .update(cx, |view, cx| {
                            view.begin_environment_write(&target, request, cx)
                        })
                        .unwrap_or_else(|_| Err("The graph window closed.".into()));
                    match result {
                        Ok(()) => true,
                        Err(error) => {
                            confirm.update(cx, |settings, cx| settings.set_error(error, cx));
                            false
                        }
                    }
                })
                .on_close(move |_, _, cx| {
                    let preferences = close_settings.update(cx, |settings, cx| {
                        settings.retire_preview();
                        settings.preferences(cx)
                    });
                    let preference_error = preferences.and_then(|preferences| {
                        cx.set_global(preferences);
                        preferences.save(cx).err()
                    });
                    let _ = close.update(cx, |view, cx| {
                        view.modal = false;
                        if let Some(error) = preference_error {
                            view.error = Some(format!(
                                "Couldn’t remember environment export settings. {error}"
                            ));
                        }
                        cx.notify();
                    });
                })
        });
        cx.notify();
    }

    fn begin_environment_write(
        &mut self,
        target: &Target,
        request: FileKind,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if self.closed || self.work.is_some() || !target.current(self) {
            return Err("The selected map changed. Choose the map again.".into());
        }
        if self.inspector.is_changed(cx) {
            return Err("Correct or reset field text before saving the map.".into());
        }
        target.panel.validate()?;
        let map = target.panel.value().clone();
        let library = self.environment_library.clone();
        let directory = self
            .path
            .as_deref()
            .and_then(Path::parent)
            .unwrap_or(Path::new("."));
        let name = format!("environment.{}", request.extension());
        let selected = cx.prompt_for_new_path(directory, Some(&name));
        let ticket = Ticket::new();
        self.ticket = Some(ticket.clone());
        self.work = Some(Work::ChoosingEnvironmentOutput);
        self.status = None;
        self.error = None;
        let handle = self.window;
        let executor = cx.background_executor().clone();
        cx.spawn(async move |owner, cx| {
            let selected = selected.await;
            if !ticket.is_current() {
                return;
            }
            let result = match selected {
                Ok(Ok(Some(path))) => {
                    let _ = cx.update_window(handle, |_, _, cx| {
                        owner.update(cx, |view, cx| {
                            if view.owns(&ticket) {
                                view.work = Some(Work::WritingEnvironmentOutput);
                                cx.notify();
                            }
                        })
                    });
                    let worker = ticket.clone();
                    executor
                        .spawn(
                            async move { write_environment(map, library, request, path, &worker) },
                        )
                        .await
                }
                Ok(Ok(None)) => Ok(None),
                Ok(Err(error)) => Err(format!("Couldn’t open the save dialog: {error}")),
                Err(_) => Err("The save dialog closed before returning a result.".into()),
            };
            let _ = cx.update_window(handle, |_, _, cx| {
                owner.update(cx, |view, cx| {
                    if !view.owns(&ticket) {
                        return;
                    }
                    view.work = None;
                    view.ticket = None;
                    ticket.retire();
                    match result {
                        Ok(Some(path)) => view.status = Some(request.success(&path)),
                        Ok(None) => {}
                        Err(error) => view.error = Some(error),
                    }
                    cx.notify();
                })
            });
        })
        .detach();
        cx.notify();
        Ok(())
    }
}

fn write_environment(
    map: Arc<EnvironmentRecord>,
    library: EnvironmentResourceLibrary,
    request: FileKind,
    path: PathBuf,
    ticket: &Ticket,
) -> Result<Option<PathBuf>, String> {
    let path = request.destination(path)?;
    let FileKind::Image { render, format } = request else {
        return match encode_preset(&map, &library, ticket)? {
            Some(binary) => model::save_bytes(binary.bytes(), path, ticket),
            None => Ok(None),
        };
    };
    let cancelled = || !ticket.is_current();
    let rendered = match map.render_projected_with::<ForegroundError>(
        render,
        generator_execution(),
        Some(&library),
        &cancelled,
    ) {
        Ok(rendered) => rendered,
        Err(PointwiseError::Cancelled) => return Ok(None),
        Err(error) => return Err(format!("Couldn’t render the environment map: {error}")),
    };
    model::write_file(path, ticket, |file| {
        let mut writer = BufWriter::new(file);
        match format {
            Format::Pfm | Format::Hdr => write_hdr(&rendered, format, &mut writer, &cancelled)?,
            _ => write_standard(&rendered, format, &mut writer)?,
        }
        writer.flush().map_err(model::failure)
    })
}

pub(super) fn encode_preset(
    map: &EnvironmentRecord,
    library: &EnvironmentResourceLibrary,
    ticket: &Ticket,
) -> Result<Option<BinaryWriter>, String> {
    let rendered = match map.render_preset_thumbnail::<ForegroundError>(
        generator_execution(),
        Some(library),
        &|| !ticket.is_current(),
    ) {
        Ok(rendered) => rendered,
        Err(PointwiseError::Cancelled) => return Ok(None),
        Err(error) => {
            return Err(format!(
                "Couldn’t render the environment thumbnail: {error}"
            ));
        }
    };
    if !ticket.is_current() {
        return Ok(None);
    }
    let mut thumbnail = Vec::new();
    write_standard(
        &rendered,
        Format::Jpeg,
        &mut std::io::Cursor::new(&mut thumbnail),
    )?;
    let mut preset = EnvironmentPreset::new(map.clone(), thumbnail);
    let mut binary = BinaryWriter::default();
    preset.save(&mut binary).map_err(model::failure)?;
    Ok(Some(binary))
}

fn write_standard(
    map: &SharedImageMap,
    format: Format,
    writer: &mut (impl Write + Seek),
) -> Result<(), String> {
    let bitmap = project_bitmap(map, &PreviewSelection::BaseColor).map_err(model::failure)?;
    let pixels = PreviewPixels::capture_rendered(&bitmap)?.into_rgba();
    let image = DynamicImage::ImageRgba8(
        RgbaImage::from_raw(bitmap.width() as u32, bitmap.height() as u32, pixels)
            .ok_or("The environment raster dimensions are invalid.")?,
    );
    let codec = match format {
        Format::Png => ImageFormat::Png,
        Format::Jpeg => {
            return JpegEncoder::new_with_quality(writer, 95)
                .encode_image(&image.into_rgb8())
                .map_err(model::failure);
        }
        Format::Tga => ImageFormat::Tga,
        Format::Tiff => ImageFormat::Tiff,
        Format::Bmp => ImageFormat::Bmp,
        Format::Pfm | Format::Hdr => unreachable!("floating-point writer"),
    };
    image.write_to(writer, codec).map_err(model::failure)
}

fn write_hdr(
    map: &SharedImageMap,
    format: Format,
    writer: &mut impl Write,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), String> {
    let geometry = map.geometry();
    let (width, height) = (geometry.width() as usize, geometry.height() as usize);
    let channels =
        [0, 1, 2].map(|channel| map.channel(channel, true).map(|buffer| buffer.values()));
    let [red, green, blue] = channels;
    let planes = [
        red.map_err(model::failure)?,
        green.map_err(model::failure)?,
        blue.map_err(model::failure)?,
    ];
    let rgb = |index| {
        planes
            .each_ref()
            .map(|plane| plane[if plane.len() == 1 { 0 } else { index }])
    };
    if matches!(format, Format::Pfm) {
        write!(writer, "PF\n{width} {height}\n-1.000000\n").map_err(model::failure)?;
        for y in (0..height).rev() {
            if cancelled() {
                return Ok(());
            }
            for x in 0..width {
                for channel in rgb(y * width + x) {
                    writer
                        .write_all(&channel.to_le_bytes())
                        .map_err(model::failure)?;
                }
            }
        }
        return Ok(());
    }
    let mut pixels = Vec::new();
    pixels
        .try_reserve_exact(width * height)
        .map_err(model::failure)?;
    for y in 0..height {
        if cancelled() {
            return Ok(());
        }
        pixels.extend((0..width).map(|x| Rgb(rgb(y * width + x).map(|value| value.max(0.)))));
    }
    HdrEncoder::new(writer)
        .encode(&pixels, width, height)
        .map_err(model::failure)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mycela_engine::{
        BinaryCursor, EnvironmentDisplayTone, EnvironmentField, EnvironmentImageProjection,
        EnvironmentLayerKind, EnvironmentLight, ProceduralExecution,
    };

    #[test]
    fn standalone_environment_view_rotation_and_tone_use_separate_stages() {
        let mut map = EnvironmentRecord::default();
        map.add_layer(EnvironmentLayerKind::Light(EnvironmentLight::Hard))
            .unwrap();
        let execution = ProceduralExecution::CallingThread;
        let settings = EnvironmentRenderSettings {
            width: 96,
            projection: EnvironmentImageProjection::LatitudeLongitude,
            source_width_limit: Some(64),
            rotation: [90., 0., 0.],
        };
        let render = |settings| {
            map.render_projected_with::<()>(settings, execution, None, &|| false)
                .unwrap()
        };
        let projected = render(settings);
        let unrotated = render(EnvironmentRenderSettings {
            rotation: [0.; 3],
            ..settings
        });
        assert_eq!(
            (projected.geometry().width(), projected.geometry().height()),
            (96, 48)
        );
        let plane = |image: &SharedImageMap| image.channel(0, true).unwrap().values();
        let original = plane(&projected);
        assert_ne!(original, plane(&unrotated));
        let display = EnvironmentDisplayTone {
            exposure: 1.,
            gamma: 1.,
        }
        .apply::<()>(&projected, &|| false)
        .unwrap();
        assert!(
            original
                .iter()
                .zip(plane(&display))
                .all(|(&color, shown)| shown == color * 2.)
        );
        assert_eq!(plane(&projected), original);
    }

    #[test]
    fn standalone_environment_formats_preserve_geometry_and_range() {
        let mut map = EnvironmentRecord::default();
        map.add_layer(EnvironmentLayerKind::Color).unwrap();
        map.set_field(EnvironmentField::Red(0), 0.75).unwrap();
        map.set_field(EnvironmentField::Green(0), 0.25).unwrap();
        map.set_exposure(1.);
        let execution = ProceduralExecution::CallingThread;
        for (projection, width, height) in [
            (EnvironmentImageProjection::LatitudeLongitude, 63, 31),
            (EnvironmentImageProjection::LightProbe, 64, 64),
            (EnvironmentImageProjection::MirroredBall, 64, 64),
            (EnvironmentImageProjection::HorizontalCross, 64, 48),
            (EnvironmentImageProjection::VerticalCross, 63, 84),
        ] {
            let image = map
                .render_projected::<()>(width, projection, execution, None, &|| false)
                .unwrap();
            assert_eq!(
                (image.geometry().width(), image.geometry().height()),
                (width, height)
            );
        }
        let image = map
            .render_projected::<()>(
                32,
                EnvironmentImageProjection::LatitudeLongitude,
                execution,
                None,
                &|| false,
            )
            .unwrap();
        for format in [
            Format::Png,
            Format::Jpeg,
            Format::Tga,
            Format::Tiff,
            Format::Bmp,
        ] {
            let mut file = std::io::Cursor::new(Vec::new());
            write_standard(&image, format, &mut file).unwrap();
            let codec = ImageFormat::from_extension(format.extension()).unwrap();
            let decoded = image::load_from_memory_with_format(file.get_ref(), codec)
                .unwrap()
                .into_rgb8();
            assert_eq!(decoded.dimensions(), (32, 16));
            let [red, green, _] = decoded.get_pixel(15, 7).0;
            assert!(red > 250);
            assert!((125..=129).contains(&green));
        }
        for format in [Format::Pfm, Format::Hdr] {
            let mut file = Vec::new();
            write_hdr(&image, format, &mut file, &|| false).unwrap();
            let mut reader = BinaryCursor::new(&file).unwrap();
            let mut decoded = match format {
                Format::Pfm => mycela_engine::decode_pfm(&mut reader).unwrap(),
                Format::Hdr => mycela_engine::decode_hdr(&mut reader).unwrap(),
                _ => unreachable!(),
            };
            assert_eq!(decoded.geometry(), image.geometry());
            let red = decoded.channel(0, true).unwrap().read(0).unwrap();
            assert!((red - 1.5).abs() < 0.015);
        }
    }
}
