#[path = "graph_document_sequence_output.rs"]
mod output;
#[path = "graph_document_sequence_settings.rs"]
mod settings;

use super::{GraphDocumentView, Ticket, Work, model, results::RenderTarget};

use gpui_kit::component::{
    WindowExt,
    button::{Button, ButtonVariants},
    dialog::{DialogAction, DialogClose, DialogFooter},
};
use gpui_kit::{AppContext, Context, Global, ParentElement, Window};
use output::SequenceSettings;
use settings::Settings;
use std::path::Path;

struct LastSettings(SequenceSettings);
impl Global for LastSettings {}

pub(super) struct SequenceProgress {
    completed: u32,
    total: u32,
    stop: Ticket,
}

impl SequenceProgress {
    pub(super) fn percentage(&self) -> f32 {
        self.completed as f32 * 100. / self.total as f32
    }

    pub(super) fn status(&self) -> String {
        let verb = if self.stop.is_current() {
            "Rendering"
        } else {
            "Stopping"
        };
        format!(
            "{verb} image sequence · {} of {} saved",
            self.completed, self.total
        )
    }

    pub(super) fn running(&self) -> bool {
        self.stop.is_current()
    }
}

impl GraphDocumentView {
    pub(super) fn open_sequence(
        &mut self,
        target: RenderTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.closed || self.is_busy() {
            return;
        }
        let Some(node) = self.result_target(target) else {
            return;
        };
        let occurrence = node.occurrence_id();
        let revision = self.revision;
        let title = match target {
            RenderTarget::Texture => "Render texture animation",
            RenderTarget::Selection if self.preview_lab_section => "Render Lab section animation",
            RenderTarget::Selection => "Render selected node animation",
        };
        let source = self.result_source(target, &node);
        let previous = cx.try_global::<LastSettings>().map(|settings| settings.0);
        let settings = cx.new(|cx| Settings::new(source, previous, window, cx));
        let owner = cx.weak_entity();
        self.pause_preview();
        self.modal = true;
        window.open_dialog(cx, move |dialog, window, _| {
            let settings = settings.clone();
            let confirm = settings.clone();
            let owner = owner.clone();
            let close = owner.clone();
            dialog
                .title(title)
                .width(window.rem_size() * 36.)
                .child(settings)
                .footer(
                    DialogFooter::new()
                        .child(
                            DialogClose::new()
                                .child(Button::new("sequence-dismiss").label("Cancel")),
                        )
                        .child(
                            DialogAction::new().child(
                                Button::new("sequence-choose-path")
                                    .primary()
                                    .label("Choose output…"),
                            ),
                        ),
                )
                .on_ok(move |_, _, cx| {
                    let request = confirm.read(cx).request(cx).and_then(|settings| {
                        owner
                            .update(cx, |view, cx| {
                                let node = view
                                    .result_target(target)
                                    .filter(|node| {
                                        node.occurrence_id() == occurrence
                                            && view.revision == revision
                                    })
                                    .ok_or("The render target changed. Choose it again.")?;
                                view.begin_sequence(&node, settings, cx)
                            })
                            .unwrap_or_else(|_| Err("The graph window closed.".into()))
                    });
                    match request {
                        Ok(()) => true,
                        Err(error) => {
                            confirm.update(cx, |settings, cx| settings.show_error(error, cx));
                            false
                        }
                    }
                })
                .on_close(move |_, _, cx| {
                    let _ = close.update(cx, |view, cx| {
                        view.modal = false;
                        cx.notify();
                    });
                })
        });
        cx.notify();
    }

    fn begin_sequence(
        &mut self,
        node: &model::Node,
        settings: SequenceSettings,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if self.closed || self.work.is_some() {
            return Err("Another operation is in progress.".into());
        }
        let pending = model::freeze(node, 0.)?;
        cx.set_global(LastSettings(settings));
        let directory = self
            .path
            .as_deref()
            .and_then(Path::parent)
            .unwrap_or(Path::new("."));
        let name = format!("animation.{}", settings.format.extension());
        let prompt = cx.prompt_for_new_path(directory, Some(&name));
        let library = self.environment_library.clone();
        let active = Ticket::new();
        let stop = Ticket::new();
        self.ticket = Some(active.clone());
        self.work = Some(Work::ChoosingSequence);
        self.error = None;
        self.status = None;
        let background = cx.background_executor().clone();
        cx.spawn(async move |owner, cx| {
            let selected = prompt.await;
            let path = match selected {
                Ok(Ok(Some(path))) => path,
                other => {
                    let _ = owner.update(cx, |view, cx| {
                        if !view.owns(&active) {
                            return;
                        }
                        view.finish_sequence();
                        view.error = match other {
                            Ok(Ok(None)) => None,
                            Ok(Err(error)) => {
                                Some(format!("Couldn’t open the save dialog ({error})."))
                            }
                            Err(_) => {
                                Some("The save dialog closed before returning a result.".into())
                            }
                            Ok(Ok(Some(_))) => unreachable!(),
                        };
                        cx.notify();
                    });
                    return;
                }
            };
            let started = owner
                .update(cx, |view, cx| {
                    if !view.owns(&active) {
                        return false;
                    }
                    view.remember_export_folder(&path, cx);
                    view.work = Some(Work::RenderingSequence);
                    view.sequence_progress = Some(SequenceProgress {
                        completed: 0,
                        total: settings.length(),
                        stop: stop.clone(),
                    });
                    cx.notify();
                    true
                })
                .unwrap_or(false);
            if !started {
                return;
            }
            let (send, receive) = async_channel::bounded(1);
            let work_active = active.clone();
            let work_stop = stop.clone();
            let work_path = path.clone();
            let job = background.spawn(async move {
                output::render(
                    pending,
                    settings,
                    &work_path,
                    &work_active,
                    &work_stop,
                    &library,
                    |completed| {
                        let _ = send.try_send(completed);
                    },
                )
            });
            while let Ok(completed) = receive.recv().await {
                let _ = owner.update(cx, |view, cx| {
                    if view.owns(&active)
                        && let Some(progress) = &mut view.sequence_progress
                    {
                        progress.completed = completed;
                        cx.notify();
                    }
                });
            }
            let report = job.await;
            let _ = owner.update(cx, |view, cx| {
                if !view.owns(&active) {
                    return;
                }
                view.finish_sequence();
                let completed = report.completed;
                let folder = path
                    .parent()
                    .filter(|path| !path.as_os_str().is_empty())
                    .unwrap_or(Path::new("."));
                if let Some(error) = report.error {
                    view.error = Some(if completed == 0 {
                        error
                    } else {
                        format!("Saved {completed} of {} frames. {error}", settings.length())
                    });
                } else if completed == settings.length() {
                    let frames = if completed == 1 { "frame" } else { "frames" };
                    view.status = Some(format!(
                        "Saved {completed} {frames} to {}",
                        folder.display()
                    ));
                } else {
                    view.status = Some(format!(
                        "Stopped after saving {completed} of {} frames to {}",
                        settings.length(),
                        folder.display(),
                    ));
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
        Ok(())
    }

    pub(super) fn stop_sequence(&mut self, cx: &mut Context<Self>) {
        if let Some(progress) = &self.sequence_progress {
            progress.stop.retire();
            cx.notify();
        }
    }

    fn finish_sequence(&mut self) {
        if let Some(ticket) = self.ticket.take() {
            ticket.retire();
        }
        self.work = None;
        self.sequence_progress = None;
    }
}
