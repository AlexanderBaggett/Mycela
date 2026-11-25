//! Export the selected retained graph as a click-time PNG frame.
use super::{GraphDocumentView, Ticket, Work};
use crate::{
    export_render::{ExportRenderJob, ExportSource},
    solid_color::{
        export_settings::{ExportSettings, checked_dimensions},
        map_preview::PreviewSelection,
        preview_export::PreviewPixels,
    },
};
use gpui_kit::component::{
    WindowExt,
    button::{Button, ButtonVariants},
    dialog::{DialogAction, DialogClose, DialogFooter},
};
use gpui_kit::{AppContext, Context, ParentElement, Window};
use std::path::Path;

impl GraphDocumentView {
    pub(super) fn open_export_image(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.closed || self.is_busy() {
            return;
        }
        let Some(target) = self.preview_target() else {
            return;
        };
        let target_id = target.occurrence_id();
        let selection = self.selection.clone();
        let node = self.inspector.owner.clone();
        let revision = self.revision;
        let texture_output = self.preview_uses_texture_output();
        if !texture_output && !self.is_original_selection(&selection, &node, revision) {
            self.error =
                Some("The selected node changed. Select it again before exporting.".into());
            cx.notify();
            return;
        }
        let map = match self.export_preview_map(cx) {
            Ok(map) => map,
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        let time = self.sample_time;
        self.pause_preview();
        let label = if texture_output {
            "Texture output"
        } else if self.preview_lab_section {
            self.lab_sections.selected_label().unwrap_or("Lab section")
        } else {
            "Selected node"
        };
        let title = if texture_output {
            "Export texture output"
        } else if self.preview_lab_section {
            "Export Lab section"
        } else {
            "Export selected node"
        };
        let description = format!(
            "{label} · {} · time {time}. Export uses committed values.",
            map.label()
        );
        let settings = cx.new(|cx| ExportSettings::new(description, window, cx));
        let owner = cx.weak_entity();
        self.modal = true;
        window.open_dialog(cx, move |dialog, _, _| {
            let settings = settings.clone(); let confirm = settings.clone();
            let owner = owner.clone(); let close = owner.clone();
            let selection = selection.clone(); let node = node.clone(); let map = map.clone();
            dialog.title(title).child(settings)
                .footer(DialogFooter::new()
                    .child(DialogClose::new().child(Button::new("cancel").label("Cancel")))
                    .child(DialogAction::new().child(Button::new("ok").primary().label("Export PNG…"))))
                .on_ok(move |_, _, cx| {
                    let result = confirm.read(cx).dimensions(cx).and_then(|(width, height)| {
                        owner.update(cx, |view, cx| {
                            if view.closed || view.revision != revision
                                || (!texture_output && !view.is_original_selection(&selection, &node, revision))
                                || view.preview_target().is_none_or(|target| target.occurrence_id() != target_id)
                                || view.sample_time != time
                                || view.preview_uses_texture_output() != texture_output
                                || view.export_preview_map(cx)? != map
                            { return Err("The render target changed. Close this dialog and select it again.".into()); }
                            view.begin_graph_export(width, height, time, map.clone(), cx)
                        }).unwrap_or_else(|_| Err("The graph window closed.".into()))
                    });
                    match result { Ok(()) => true, Err(error) => { confirm.update(cx, |settings, cx| settings.set_error(error, cx)); false } }
                })
                .on_close(move |_, _, cx| { let _ = close.update(cx, |view, cx| {view.modal=false;cx.notify();}); })
        });
        cx.notify();
    }
    fn begin_graph_export(
        &mut self,
        width: i32,
        height: i32,
        time: f64,
        map: PreviewSelection,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if self.closed || self.work.is_some() {
            return Err("Another file operation is in progress.".into());
        }
        checked_dimensions(width, height)?;
        let node = self
            .preview_target()
            .ok_or("Choose a graph node to export.")?;
        let pending = ExportRenderJob::freeze_at_time(
            ExportSource::Graph {
                node: &node,
                camera: None,
                time,
            },
            None,
            &[],
            map,
            width,
            height,
            time,
        )
        .map_err(|e| e.to_string())?
        .with_graph_cache(self.export_cache.clone())
        .with_environment_library(self.environment_library.clone());
        let directory = self
            .path
            .as_ref()
            .and_then(|p| p.parent())
            .unwrap_or(Path::new("."))
            .to_path_buf();
        let ticket = Ticket::new();
        self.ticket = Some(ticket.clone());
        self.work = Some(Work::ChoosingExport);
        self.error = None;
        self.status = None;
        let prompt = cx.prompt_for_new_path(&directory, Some("mycela-graph.png"));
        let background = cx.background_executor().clone();
        let handle = self.window;
        let materialize = background.spawn(async move { pending.materialize() });
        cx.spawn(async move |owner, cx| {
            let selected = prompt.await;
            if !cx
                .update_window(handle, |_, _, cx| {
                    owner.read_with(cx, |view, _| view.owns(&ticket))
                })
                .is_ok_and(|r| r.is_ok_and(|v| v))
            {
                return;
            }
            let result = match selected {
                Ok(Ok(Some(path))) => {
                    let _ = owner.update(cx, |view, cx| {
                        view.work = Some(Work::Exporting);
                        cx.notify();
                    });
                    let job = materialize
                        .await
                        .map_err(|e| format!("Couldn’t capture the image: {e}"));
                    let work = ticket.clone();
                    background
                        .spawn(async move {
                            let job = job?;
                            let bitmap = match job.render(|| !work.is_current()) {
                                Ok(bitmap) => bitmap,
                                Err(error) if error.is_cancelled() => return Ok(None),
                                Err(error) => {
                                    return Err(format!("Couldn’t render the image: {error}"));
                                }
                            };
                            if !work.is_current() {
                                return Ok(None);
                            }
                            PreviewPixels::capture_rendered(&bitmap)?
                                .save_while(path, &|| work.is_current())
                        })
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
                        Ok(Some(path)) => {
                            view.remember_export_folder(&path, cx);
                            view.status =
                                Some(format!("Saved {width} × {height} PNG: {}", path.display()))
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
        Ok(())
    }
    pub(super) fn cancel_export(&mut self, cx: &mut Context<Self>) {
        if !self.work.is_some_and(Work::can_cancel_export) {
            return;
        }
        if let Some(ticket) = self.ticket.take() {
            ticket.retire();
        }
        self.work = None;
        self.status = Some("Export cancelled.".into());
        cx.notify();
    }
}
