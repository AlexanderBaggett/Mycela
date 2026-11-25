//! Export the texture and a thumbnail without changing its graph.
use super::*;
use crate::export_render::generator_execution;
use image::{ExtendedColorType, codecs::jpeg::JpegEncoder};
use mycela_engine::{
    BinaryWriter, EnvironmentResourceLibrary, ImageGeometry, PointwiseRenderEffects, SharedImageMap,
};

pub(super) struct PresetExport {
    texture: SynthesisEdit,
    input: Option<model::PendingPreview>,
    library: EnvironmentResourceLibrary,
}

impl GraphDocumentView {
    pub(super) fn capture_synthesis_preset(
        &self,
        target: &Target,
        cx: &App,
    ) -> Result<PresetExport, String> {
        if self.closed || !target.current(self) || self.has_field_drafts(cx) {
            return Err("Correct or reset field text and select a texture to save.".into());
        }
        let panel = self.synthesis.as_ref().ok_or("Select a texture to save.")?;
        let standalone = target.style.is_none() && !target.lab_material;
        let texture = if standalone {
            target
                .node
                .filter()
                .ok_or("Select a Synthesis node to save.")?
                .synthesis_preset_snapshot()
                .map_err(model::failure)?
        } else {
            panel.edit.clone()
        };
        let input = if standalone && texture.uses_input() {
            target
                .node
                .slot(0)
                .ok()
                .and_then(|slot| slot.node())
                .and_then(|node| model::freeze(&node, self.sample_time).ok())
        } else {
            None
        };
        Ok(PresetExport {
            texture,
            input,
            library: self.environment_library.clone(),
        })
    }

    pub(super) fn save_synthesis_preset(&mut self, target: &Target, cx: &mut Context<Self>) {
        let export = match self.capture_synthesis_preset(target, cx) {
            Ok(export) => export,
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        let directory = self
            .path
            .as_deref()
            .or(self.imported_from.as_deref())
            .and_then(Path::parent)
            .unwrap_or(Path::new("."));
        let selected = cx.prompt_for_new_path(directory, Some("texture.tsyn"));
        let ticket = Ticket::new();
        self.ticket = Some(ticket.clone());
        self.work = Some(Work::ChoosingSynthesisPreset);
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
                                view.work = Some(Work::WritingSynthesisPreset);
                                cx.notify();
                            }
                        })
                    });
                    let worker = ticket.clone();
                    executor
                        .spawn(async move { export.write(path, &worker) })
                        .await
                }
                Ok(Ok(None)) => Ok(None),
                Ok(Err(error)) => Err(format!("Couldn’t open the save dialog. {error}")),
                Err(_) => Err("The save dialog closed before returning a result.".into()),
            };
            let _ = cx.update_window(handle, |_, _, cx| {
                owner.update(cx, |view, cx| {
                    if !view.owns(&ticket) {
                        return;
                    }
                    view.ticket = None;
                    view.work = None;
                    ticket.retire();
                    match result {
                        Ok(Some(path)) => {
                            view.status =
                                Some(format!("Saved Synthesis preset to {}", path.display()))
                        }
                        Ok(None) => {}
                        Err(error) => view.error = Some(error),
                    }
                    cx.notify();
                })
            });
        })
        .detach();
        cx.notify();
    }
}

impl PresetExport {
    fn write(self, mut path: PathBuf, ticket: &Ticket) -> Result<Option<PathBuf>, String> {
        let add_extension = path.extension().is_none();
        if add_extension {
            path.set_extension("tsyn");
        }
        if !path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("tsyn"))
        {
            return Err("Use a .tsyn file name for a Synthesis preset.".into());
        }
        if add_extension && path.try_exists().map_err(model::failure)? {
            return Err(format!(
                "Choose the full file name to replace {}.",
                path.display()
            ));
        }
        let Some(writer) = self.encode(ticket)? else {
            return Ok(None);
        };
        if add_extension {
            model::save_new_bytes(writer.bytes(), path, ticket)
        } else {
            model::save_bytes(writer.bytes(), path, ticket)
        }
    }

    pub(super) fn encode(self, ticket: &Ticket) -> Result<Option<BinaryWriter>, String> {
        let cancelled = || !ticket.is_current();
        let size = SynthesisEdit::PRESET_THUMBNAIL_SIZE;
        let input = self
            .input
            .and_then(|input| input.materialize().ok())
            .and_then(|input| {
                crate::scene_backend::render_export(
                    &input,
                    ImageGeometry::new(size, size),
                    &cancelled,
                    crate::scene_backend::GRAPH_PNG_OPTIONS,
                    None,
                    None,
                    Some(&self.library),
                )
                .ok()
            });
        if cancelled() {
            return Ok(None);
        }
        let mut capacity = |_| Ok(true);
        let mut preflight = |_, _| Ok(());
        let mut effects =
            PointwiseRenderEffects::<ForegroundError>::new(&mut capacity, &mut preflight)
                .with_cancel_check(&cancelled)
                .with_environment_library(Some(&self.library));
        let image = match self.texture.render_preset_thumbnail(
            input.as_ref(),
            generator_execution(),
            &mut effects,
        ) {
            Ok(image) => image,
            Err(mycela_engine::PointwiseError::Cancelled) => return Ok(None),
            Err(error) => return Err(format!("Couldn’t render the Synthesis thumbnail. {error}")),
        };
        let thumbnail = jpeg_thumbnail(&image)?;
        let mut writer = BinaryWriter::default();
        self.texture
            .save_preset(&thumbnail, &mut writer)
            .map_err(model::failure)?;
        Ok(Some(writer))
    }
}

fn jpeg_thumbnail(image: &SharedImageMap) -> Result<Vec<u8>, String> {
    let size = SynthesisEdit::PRESET_THUMBNAIL_SIZE as usize;
    let [red, green, blue, transparency] =
        [0, 1, 2, 3].map(|channel| image.materialize(channel).map_err(model::failure));
    let (red, green, blue, transparency) = (red?, green?, blue?, transparency?);
    let mut pixels = Vec::with_capacity(size * size * 3);
    for (index, &transparency) in transparency.values().iter().enumerate() {
        let cell = (index % size / 8 + index / size / 8) % 2;
        let background = [192., 240.][cell];
        let transparency = transparency.clamp(0., 1.);
        for plane in [&red, &green, &blue] {
            let color = plane.values()[index].clamp(0., 1.) * 255.;
            pixels.push((color * (1. - transparency) + background * transparency).round() as u8);
        }
    }
    let mut bytes = Vec::new();
    JpegEncoder::new_with_quality(&mut bytes, 95)
        .encode(&pixels, size as u32, size as u32, ExtendedColorType::Rgb8)
        .map_err(model::failure)?;
    Ok(bytes)
}
