//! Connects the focused animation window to the workspace's retained source.
use super::{ForegroundError, SolidColorView, TextureSourceKind};
use crate::graph_document::{GraphDocumentView, SourceAnimationFrame, open_solid_animation_window};
use gpui_kit::{AnyWindowHandle, App, AppContext, Context, Entity, Subscription, Window};
use mycela_engine::{Gtx375GraphDocument, PointwiseNode, SharedImageMap};
use std::sync::{Arc, Mutex};

pub(super) struct SolidAnimationEditor {
    window: AnyWindowHandle,
    view: Entity<GraphDocumentView>,
    _frames: Subscription,
    _closed: Subscription,
}

impl SolidColorView {
    pub(super) fn solid_preview_image(&self, time: f64) -> Result<SharedImageMap, ForegroundError> {
        let frame = self
            .node
            .capture_preview_at(time)
            .map_err(|error| ForegroundError::Operation(error.to_string()))?;
        frame
            .render(crate::LIVE_PREVIEW_SIZE, crate::LIVE_PREVIEW_SIZE)
            .map(SharedImageMap::from_image)
            .map_err(|error| ForegroundError::Operation(error.to_string()))
    }

    pub(super) fn solid_switch_time(&self, cx: &App) -> f64 {
        if self.source.scene_kind().is_some() {
            self.sample_time(cx)
        } else {
            self.sample_time
        }
    }

    pub(super) fn open_solid_animation(&mut self, cx: &mut Context<Self>) {
        if self.source != TextureSourceKind::SolidColor {
            return;
        }
        if let Some(editor) = &self.solid_animation_editor
            && editor
                .window
                .update(cx, |_, window, _| window.activate_window())
                .is_ok()
        {
            return;
        }
        self.solid_animation_editor = None;
        let document = self
            .solid_animation_document
            .get_or_insert_with(|| {
                let node = PointwiseNode::from_solid_color(self.node.clone());
                let document = Gtx375GraphDocument::new(vec![Some(node)]).expect("one real branch");
                Arc::new(Mutex::new(document))
            })
            .clone();
        let path = self.imported_path().map(|path| path.to_path_buf());
        match open_solid_animation_window(document, path, self.sample_time, cx) {
            Ok((window, editor)) => {
                let workspace = self.dispatcher.window;
                let frames =
                    cx.subscribe(&editor, move |_, _, frame: &SourceAnimationFrame, cx| {
                        let time = frame.0;
                        cx.spawn(async move |owner, cx| {
                            let _ = cx.update_window(workspace, |_, workspace, cx| {
                                let _ = owner.update(cx, |view, cx| {
                                    if view.source != TextureSourceKind::SolidColor
                                        || view
                                            .solid_animation_editor
                                            .as_ref()
                                            .is_none_or(|editor| editor.window != window)
                                    {
                                        return;
                                    }
                                    view.show_solid_animation_frame(time, workspace, cx);
                                });
                            });
                        })
                        .detach();
                    });
                let owner = cx.weak_entity();
                let closed = cx.on_window_closed(move |cx, id| {
                    if id != window.window_id() {
                        return;
                    }
                    let _ = workspace.update(cx, |_, workspace, cx| {
                        let _ = owner.update(cx, |view, cx| {
                            let Some(editor) = view
                                .solid_animation_editor
                                .as_ref()
                                .filter(|editor| editor.window == window)
                            else {
                                return;
                            };
                            let time = editor.view.read(cx).source_time();
                            view.solid_animation_editor = None;
                            view.show_solid_animation_frame(time, workspace, cx);
                        });
                    });
                });
                self.solid_animation_editor = Some(SolidAnimationEditor {
                    window,
                    view: editor,
                    _frames: frames,
                    _closed: closed,
                });
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
        cx.notify();
    }

    fn show_solid_animation_frame(
        &mut self,
        time: f64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.source == TextureSourceKind::SolidColor {
            self.sample_time = time;
            self.sync_filter_time(window, cx);
            let _ = self.refresh_preview(cx);
            cx.notify();
        }
    }

    pub(super) fn sync_solid_animation_time(&self, cx: &mut App) {
        if let Some(editor) = &self.solid_animation_editor {
            let _ = editor.window.update(cx, |_, window, cx| {
                editor.view.update(cx, |view, cx| {
                    view.sync_source_time(self.sample_time, window, cx)
                });
            });
        }
    }

    pub(super) fn sync_solid_animation_values(&self, cx: &mut App) {
        if let Some(editor) = &self.solid_animation_editor {
            editor
                .view
                .update(cx, |view, cx| view.sync_source_values(cx));
        }
    }

    pub(super) fn close_solid_animation(&mut self, cx: &mut App) {
        if let Some(editor) = self.solid_animation_editor.take() {
            let _ = editor
                .window
                .update(cx, |_, window, _| window.remove_window());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{ScrollDelta, TestAppContext, point, px, test::TestWindowExt};

    #[gpui_kit::test]
    async fn solid_animation_window_shares_preview_time_and_freezes_the_final_frame(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let (workspace, root) = cx.update(crate::open_main_window).unwrap();
        let view = cx.update(|cx| root.read(cx).color_view());
        let open_editor = |cx: &mut TestAppContext| {
            cx.update_window(workspace, |_, window, cx| {
                window.render_frame(cx);
                window.click("edit-solid-animation", cx);
            })
            .unwrap();
            cx.run_until_parked();
            cx.update(|cx| {
                view.read(cx)
                    .solid_animation_editor
                    .as_ref()
                    .unwrap()
                    .window
            })
        };
        let editor = open_editor(cx);
        assert_eq!(open_editor(cx), editor);
        cx.update_window(editor, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("graph-open").is_none());
            window.click("branch-1:solid-animation:color", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(editor, |_, window, cx| {
            window.render_frame(cx);
            window.click("branch-1:solid-animation:color:1", cx);
            window.press("secondary-a", cx);
            window.input("100", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(editor, |_, window, cx| {
            window.render_frame(cx);
            window.scroll(
                "branch-1:solid-animation:color",
                ScrollDelta::Pixels(point(px(0.), px(-240.))),
                cx,
            );
            window.click("graph-gradient-color-0-1", cx);
            window.press("secondary-a", cx);
            window.input("0.5", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(editor, |_, window, cx| {
            window.render_frame(cx);
            window.within("solid-animation-properties").click(1, cx);
            window.click("branch-1:solid-animation:transparency", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(editor, |_, window, cx| {
            window.render_frame(cx);
            for (id, value) in [
                ("branch-1:solid-animation:transparency:0", "0"),
                ("branch-1:solid-animation:transparency:1", "0.5"),
            ] {
                window.click(id, cx);
                window.press("secondary-a", cx);
                window.input(value, cx);
            }
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(editor, |_, window, cx| {
            window.render_frame(cx);
            window.click("graph-sample-time", cx);
            window.press("secondary-a", cx);
            window.input("0.75", cx);
            window.click("graph-set-sample-time", cx);
        })
        .unwrap();
        cx.run_until_parked();
        let pixel =
            |image: &SharedImageMap, channel| image.materialize(channel).unwrap().read(0).unwrap();
        let (frozen, expected_red) = cx.update(|cx| {
            let view = view.read(cx);
            assert_eq!(view.sample_time(cx), 0.75);
            let image = view.applied_image().unwrap();
            assert_eq!(pixel(&image, 3), 0.375);
            assert!(pixel(&image, 0) > pixel(&image, 1));
            (view.freeze_export_job(7, 3, cx).unwrap(), pixel(&image, 0))
        });
        cx.update_window(editor, |_, window, cx| {
            window.render_frame(cx);
            window.click("branch-1:solid-animation:transparency", cx);
            window.click("solid-animation-close", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(
            cx.update(|cx| pixel(&view.read(cx).applied_image().unwrap(), 3)),
            0.0
        );
        let exported = frozen
            .materialize()
            .unwrap()
            .render_scene(|| false, false)
            .unwrap()
            .image;
        assert_eq!(pixel(&exported, 0), expected_red);
        assert_eq!(pixel(&exported, 3), 0.375);
        let reopened = open_editor(cx);
        assert_ne!(reopened, editor);
        cx.update_window(reopened, |_, window, cx| {
            window.render_frame(cx);
            window.within("solid-animation-properties").click(1, cx);
            window.click("branch-1:solid-animation:transparency", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(
            cx.update(|cx| pixel(&view.read(cx).applied_image().unwrap(), 3)),
            0.375
        );
    }
}
