#[path = "graph_document_sequence_output.rs"]
mod output;
#[path = "graph_document_sequence_settings.rs"]
mod settings;
#[path = "graph_document_video.rs"]
mod video;

use super::{GraphDocumentView, Ticket, Work, model, results::RenderTarget};

use gpui_kit::component::{
    WindowExt,
    button::{Button, ButtonVariants},
    dialog::{DialogAction, DialogClose, DialogFooter},
};
use gpui_kit::{AppContext, Context, Global, ParentElement, Window};
use output::SequenceSettings;
use settings::Settings;
use std::path::{Path, PathBuf};
use video::VideoSettings;

struct LastSettings(SequenceSettings);
impl Global for LastSettings {}

struct LastVideoSettings(VideoSettings);
impl Global for LastVideoSettings {}

#[derive(Clone, Copy)]
enum ExportRequest {
    Sequence(SequenceSettings),
    Video(VideoSettings),
}

impl ExportRequest {
    fn length(self) -> u32 {
        match self {
            Self::Sequence(settings) => settings.length(),
            Self::Video(settings) => settings.frames.length(),
        }
    }

    fn default_filename(self) -> String {
        let extension = match self {
            Self::Sequence(settings) => settings.format.extension(),
            Self::Video(settings) => settings.codec.extension(),
        };
        format!("animation.{extension}")
    }

    fn initial_progress(self) -> SequenceUpdate {
        match self {
            Self::Sequence(_) => SequenceUpdate::Sequence(0),
            Self::Video(_) => SequenceUpdate::VideoFrames(0),
        }
    }
}

enum ExportReport {
    Sequence(output::SequenceReport),
    Video(Result<Option<PathBuf>, String>),
}

#[derive(Clone, Copy)]
enum SequenceUpdate {
    Sequence(u32),
    VideoFrames(u32),
    VideoEncoding(u32),
}

pub(super) struct SequenceProgress {
    update: SequenceUpdate,
    total: u32,
    stop: Ticket,
}

impl SequenceProgress {
    pub(super) fn percentage(&self) -> f32 {
        let progress = |frames| frames as f32 / self.total as f32;
        match self.update {
            SequenceUpdate::Sequence(completed) => 100. * progress(completed),
            SequenceUpdate::VideoFrames(completed) => 50. * progress(completed),
            SequenceUpdate::VideoEncoding(completed) => 50. + 50. * progress(completed),
        }
    }

    pub(super) fn status(&self) -> String {
        let total = self.total;
        match (self.running(), self.update) {
            (true, SequenceUpdate::Sequence(n)) => {
                format!("Rendering image sequence · {n} of {total} saved")
            }
            (false, SequenceUpdate::Sequence(n)) => {
                format!("Stopping image sequence · {n} of {total} saved")
            }
            (true, SequenceUpdate::VideoFrames(n)) => {
                format!("1 of 2 · Rendering frames · {n} of {total}")
            }
            (true, SequenceUpdate::VideoEncoding(n)) => {
                format!("2 of 2 · Encoding video with FFmpeg · {n} of {total}")
            }
            (false, _) => "Stopping video export…".into(),
        }
    }

    pub(super) fn running(&self) -> bool {
        self.stop.is_current()
    }
}

impl GraphDocumentView {
    pub(super) fn open_sequence(
        &mut self,
        target: RenderTarget,
        video: bool,
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
        let scope = match target {
            RenderTarget::Texture => "texture",
            RenderTarget::Selection if self.preview_lab_section => "Lab section",
            RenderTarget::Selection => "selected node",
        };
        let kind = if video { "video" } else { "animation" };
        let title = format!("Render {scope} {kind}");
        let source = self.result_source(target, &node);
        let previous = if video {
            cx.try_global::<LastVideoSettings>()
                .map(|settings| ExportRequest::Video(settings.0))
        } else {
            cx.try_global::<LastSettings>()
                .map(|settings| ExportRequest::Sequence(settings.0))
        };
        let settings = cx.new(|cx| Settings::new(source, video, previous, window, cx));
        let owner = cx.weak_entity();
        self.pause_preview();
        self.modal = true;
        window.open_dialog(cx, move |dialog, window, _| {
            let settings = settings.clone();
            let confirm = settings.clone();
            let owner = owner.clone();
            let close = owner.clone();
            dialog
                .title(title.clone())
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
        request: ExportRequest,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if self.closed || self.work.is_some() {
            return Err("Another operation is in progress.".into());
        }
        let pending = model::freeze(node, 0.)?;
        match request {
            ExportRequest::Sequence(settings) => cx.set_global(LastSettings(settings)),
            ExportRequest::Video(settings) => cx.set_global(LastVideoSettings(settings)),
        }
        let directory = self
            .path
            .as_deref()
            .and_then(Path::parent)
            .unwrap_or(Path::new("."));
        let name = request.default_filename();
        let prompt = cx.prompt_for_new_path(directory, Some(&name));
        let library = self.environment_library.clone();
        let active = Ticket::new();
        let stop = Ticket::new();
        self.ticket = Some(active.clone());
        self.work = Some(match request {
            ExportRequest::Sequence(_) => Work::ChoosingSequence,
            ExportRequest::Video(_) => Work::ChoosingVideo,
        });
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
                        update: request.initial_progress(),
                        total: request.length(),
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
                let progress = |update| {
                    let _ = send.force_send(update);
                };
                match request {
                    ExportRequest::Sequence(settings) => ExportReport::Sequence(output::render(
                        pending,
                        settings,
                        &work_path,
                        &work_active,
                        &work_stop,
                        &library,
                        |completed| progress(SequenceUpdate::Sequence(completed)),
                    )),
                    ExportRequest::Video(settings) => ExportReport::Video(video::render(
                        pending,
                        settings,
                        work_path,
                        &work_active,
                        &work_stop,
                        &library,
                        progress,
                    )),
                }
            });
            while let Ok(update) = receive.recv().await {
                let _ = owner.update(cx, |view, cx| {
                    if view.owns(&active)
                        && let Some(progress) = &mut view.sequence_progress
                    {
                        progress.update = update;
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
                match report {
                    ExportReport::Video(Ok(Some(path))) => {
                        view.status = Some(format!("Saved video to {}", path.display()));
                    }
                    ExportReport::Video(Ok(None)) => {
                        view.status = Some("Video export stopped.".into());
                    }
                    ExportReport::Video(Err(error)) => view.error = Some(error),
                    ExportReport::Sequence(report) => {
                        let completed = report.completed;
                        let total = request.length();
                        let folder = path
                            .parent()
                            .filter(|path| !path.as_os_str().is_empty())
                            .unwrap_or(Path::new("."));
                        if let Some(error) = report.error {
                            view.error = Some(if completed == 0 {
                                error
                            } else {
                                format!("Saved {completed} of {total} frames. {error}")
                            });
                        } else if completed == total {
                            let frames = if completed == 1 { "frame" } else { "frames" };
                            view.status = Some(format!(
                                "Saved {completed} {frames} to {}",
                                folder.display()
                            ));
                        } else {
                            view.status = Some(format!(
                                "Stopped after saving {completed} of {total} frames to {}",
                                folder.display(),
                            ));
                        }
                    }
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
