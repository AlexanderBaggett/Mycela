//! A reversible Levels session spanning several controls and channels.
use super::*;
use gpui_kit::component::{
    WindowExt,
    dialog::{DialogAction, DialogClose, DialogFooter},
};
use gpui_kit::{EntityId, ObjectFit, RenderImage, img};

pub(super) struct Batch {
    pub(super) draft: LevelsDraft,
    pub(super) preview: Option<gesture::DraftPreview>,
    baseline: Option<Arc<RenderImage>>,
}

impl LevelsEditor {
    pub(super) fn open_batch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_disabled(cx) {
            return;
        }
        let Some(host) = self.host.as_ref().and_then(WeakEntity::upgrade) else {
            return;
        };
        let draft = match self.parameter.levels_draft() {
            Ok(draft) => draft,
            Err(error) => {
                self.error = Some(failure(error));
                cx.notify();
                return;
            }
        };
        let source = cx.entity_id();
        let parameter = self.parameter.clone();
        let channel = self.channel;
        let chart = self.chart.clone();
        let sample_status = self.sample_status.clone();
        let form = cx.new(|cx| {
            let mut editor = Self::new(parameter, channel, draft.points(channel), window, cx);
            editor.host = Some(host.downgrade());
            editor.chart = chart;
            editor.sample_status = sample_status;
            editor.batch = Some(Batch {
                draft,
                preview: None,
                baseline: None,
            });
            editor
        });
        host.update(cx, |view, cx| {
            view.open_levels_batch(source, form, window, cx)
        });
    }

    pub(super) fn edit_batch(
        &mut self,
        edit: LevelsControlEdit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(batch) = &mut self.batch else { return };
        match batch.draft.edit(self.channel, edit) {
            Ok(()) => {
                let draft = batch.draft;
                self.error = self.sync_fields(window, cx).err();
                self.queue_batch_preview(draft, cx);
            }
            Err(_) => {
                self.error = Some(
                    "These Levels points are invalid. Reset this channel to restore its range."
                        .into(),
                );
            }
        }
        cx.notify();
    }

    pub(super) fn queue_batch_preview(&mut self, draft: LevelsDraft, cx: &mut Context<Self>) {
        if self
            .batch
            .as_ref()
            .is_some_and(|batch| batch.preview.is_none())
        {
            match gesture::DraftPreview::start(self, cx) {
                Ok(preview) => self.batch.as_mut().unwrap().preview = preview,
                Err(error) => self.set_draft_error(error, cx),
            }
        }
        if let Some(preview) = self.batch.as_ref().and_then(|batch| batch.preview.as_ref()) {
            preview.queue(draft);
        }
    }

    fn batch_value(&self, cx: &App) -> Result<LevelsDraft, String> {
        let mut draft = self
            .batch
            .as_ref()
            .ok_or("The Levels editor closed.")?
            .draft;
        for (control, field) in PointControl::ALL.into_iter().zip(&self.inputs) {
            let text = field.read(cx).value();
            if text == display(control.value(self.points)) {
                continue;
            }
            let value = text
                .trim()
                .parse::<f32>()
                .ok()
                .filter(|value| value.is_finite())
                .ok_or_else(|| {
                    format!(
                        "Enter a finite value for {}.",
                        control.label().to_lowercase()
                    )
                })?;
            draft
                .edit(self.channel, control.edit(value))
                .map_err(failure)?;
        }
        Ok(draft)
    }

    pub(super) fn stage_batch_fields(&mut self, cx: &mut Context<Self>) -> bool {
        if self.batch.is_none() {
            return true;
        }
        match self.batch_value(cx) {
            Ok(draft) => {
                self.batch.as_mut().unwrap().draft = draft;
                self.queue_batch_preview(draft, cx);
                true
            }
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                false
            }
        }
    }

    pub(super) fn batch_raster(&self) -> Option<Arc<RenderImage>> {
        let batch = self.batch.as_ref()?;
        batch
            .preview
            .as_ref()
            .and_then(gesture::DraftPreview::image)
            .map(|preview| preview.raster.clone())
            .or_else(|| batch.baseline.clone())
    }

    pub(super) fn batch_preview_panel(&self, cx: &App) -> Option<impl IntoElement> {
        self.batch.as_ref()?;
        let raster = self.batch_raster();
        Some(
            div()
                .v_flex()
                .gap_1()
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("Levels preview"),
                )
                .when_some(raster, |panel, raster| {
                    panel.child(
                        div()
                            .id("levels-batch-preview")
                            .test_support()
                            .h(rems(10.))
                            .child(img(raster).size_full().object_fit(ObjectFit::Contain)),
                    )
                }),
        )
    }
}

impl GraphDocumentView {
    fn open_levels_batch(
        &mut self,
        source: EntityId,
        form: Entity<LevelsEditor>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.closed || self.is_busy() || !self.has_levels_editor(source) {
            return;
        }
        self.pause_preview();
        self.modal = true;
        let baseline = self.preview.as_ref().map(|preview| preview.raster.clone());
        let needs_preview = baseline.is_none();
        let time = self.sample_time;
        let library = self.environment_library.clone();
        form.update(cx, |editor, cx| {
            editor.batch.as_mut().unwrap().baseline = baseline;
            if editor.chart.is_none() {
                let input = editor
                    .parameter
                    .owner()
                    .node()
                    .slot(0)
                    .ok()
                    .and_then(|slot| slot.node());
                editor.refresh_input(input, time, library, cx);
            }
        });
        if needs_preview {
            let initial = form.downgrade();
            cx.spawn(async move |_, cx| {
                let _ = initial.update(cx, |editor, cx| {
                    if let Some(batch) = &editor.batch {
                        editor.queue_batch_preview(batch.draft, cx);
                    }
                });
            })
            .detach();
        }
        let owner = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let submit = form.clone();
            let release = form.clone();
            let edit = owner.clone();
            let close = owner.clone();
            dialog
                .title("Edit Levels channels")
                .child(form.clone())
                .footer(
                    DialogFooter::new()
                        .child(DialogClose::new().child(Button::new("cancel").label("Cancel")))
                        .child(
                            DialogAction::new().child(Button::new("ok").primary().label("Apply")),
                        ),
                )
                .on_ok(move |_, window, cx| {
                    let form = submit.read(cx);
                    let parameter = form.parameter.clone();
                    let result = form.batch_value(cx).and_then(|draft| {
                        edit.update(cx, |view, cx| {
                            if view.closed || !view.modal || !view.has_levels_editor(source) {
                                return Err("The selected Levels node changed.".into());
                            }
                            view.record_levels_edit(|| parameter.commit_levels_draft(&draft), cx)?;
                            if let Some(editor) = &view.inspector.levels {
                                editor
                                    .update(cx, |editor, cx| editor.did_commit(Ok(()), window, cx));
                            }
                            Ok(())
                        })
                        .unwrap_or_else(|_| Err("The graph window closed.".into()))
                    });
                    if let Err(error) = result {
                        submit.update(cx, |editor, cx| {
                            editor.error = Some(error);
                            cx.notify();
                        });
                        return false;
                    }
                    true
                })
                .on_close(move |_, _, cx| {
                    release.update(cx, |editor, _| {
                        editor.drag = None;
                        editor.batch = None;
                    });
                    let _ = close.update(cx, |view, cx| {
                        view.modal = false;
                        cx.notify();
                    });
                })
        });
        cx.notify();
    }

    pub(super) fn has_levels_editor(&self, source: EntityId) -> bool {
        self.inspector
            .levels
            .as_ref()
            .is_some_and(|editor| editor.entity_id() == source)
    }

    pub(super) fn record_levels_edit(
        &mut self,
        operation: impl FnOnce() -> Result<bool, mycela_engine::PointwiseError<ForegroundError>>,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        self.ensure_history_binding();
        let was_dirty = self.dirty;
        let stamp = self.begin_history_edit();
        let result = operation().map_err(failure);
        if matches!(&result, Ok(true)) {
            self.finish_history_edit(stamp, true);
            self.start_preview_job(false, cx);
        } else {
            self.dirty = was_dirty;
        }
        result.map(|_| ())
    }
}
