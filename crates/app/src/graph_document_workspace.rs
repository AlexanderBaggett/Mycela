use super::*;
use gpui_kit::{RenderImage, WeakEntity};

#[cfg(test)]
#[path = "graph_document_workspace_tests.rs"]
mod tests;

struct Document {
    view: Entity<GraphDocumentView>,
    label: String,
    path: Option<PathBuf>,
    root: Option<u64>,
    thumbnail: Option<Arc<RenderImage>>,
    _changes: Subscription,
}

impl Document {
    fn refresh(&mut self, view: &GraphDocumentView) -> bool {
        let path = view.path.as_ref().or(view.imported_from.as_ref()).cloned();
        let name = view.document_name();
        let label = if view.is_dirty() {
            format!("{name} *")
        } else {
            name
        };
        let output = guard(&view.document).branch(0).flatten();
        let root = output.as_ref().map(Node::occurrence_id);
        let thumbnail = (!view.preview_pending && view.preview_error.is_none())
            .then(|| {
                let output = output.as_ref()?;
                let target = view.preview_target()?;
                let preview = view.preview.as_ref()?;
                (output.same_reference(&target) && preview.selection.name().is_none())
                    .then(|| preview.raster.clone())
            })
            .flatten();
        let path_changed = self.path != path;
        let mut changed = self.label != label || path_changed;
        self.label = label;
        self.path = path;
        if self.root != root && thumbnail.is_none() {
            changed |= self.thumbnail.is_some();
            self.thumbnail = None;
        }
        self.root = root;
        if let Some(thumbnail) = thumbnail
            && !self
                .thumbnail
                .as_ref()
                .is_some_and(|old| Arc::ptr_eq(old, &thumbnail))
        {
            self.thumbnail = Some(thumbnail);
            changed = true;
        }
        changed
    }
}

pub(super) struct DocumentWorkspace {
    documents: Vec<Document>,
    active: usize,
    confirming_close: bool,
    close_after_save: Option<CloseScope>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CloseScope {
    Current,
    All,
}

impl DocumentWorkspace {
    pub(super) fn new(choose_file: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut workspace = Self {
            documents: Vec::new(),
            active: 0,
            confirming_close: false,
            close_after_save: None,
        };
        workspace.add_empty(window, cx);
        if choose_file {
            workspace
                .active()
                .update(cx, |view, cx| view.choose_open(DocumentFormat::Mycela, cx));
        }
        let weak = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| {
            weak.update(cx, |workspace, cx| {
                workspace.confirm_window_close(window, cx)
            })
            .unwrap_or(true)
        });
        workspace
    }

    pub(super) fn active(&self) -> Entity<GraphDocumentView> {
        self.documents[self.active].view.clone()
    }

    fn add_empty(&mut self, window: &mut Window, cx: &mut Context<Self>) -> usize {
        let owner = cx.weak_entity();
        let view = cx.new(|cx| {
            let mut view = GraphDocumentView::new_in_workspace(Some(owner), window, cx);
            view.canvas.visible = true;
            view.ensure_history_binding();
            view
        });
        self.add_document(view, window, cx)
    }

    fn add_document(
        &mut self,
        view: Entity<GraphDocumentView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> usize {
        let changes = cx.observe_in(&view, window, |workspace, view, window, cx| {
            if let Some(document) = workspace.documents.iter_mut().find(|doc| doc.view == view)
                && document.refresh(view.read(cx))
            {
                cx.notify();
            }
            workspace.finish_close_save(&view, window, cx);
        });
        let mut document = Document {
            view: view.clone(),
            label: String::new(),
            path: None,
            root: None,
            thumbnail: None,
            _changes: changes,
        };
        document.refresh(view.read(cx));
        self.documents.push(document);
        self.documents.len() - 1
    }

    pub(super) fn open_prepared(
        &mut self,
        prepared: Prepared,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(index) = self.documents.iter().position(|document| {
            let view = document.view.read(cx);
            let open_path = match prepared.format {
                DocumentFormat::Mycela => view.path.as_deref(),
                DocumentFormat::Gtx => view.imported_from.as_deref(),
            };
            open_path == Some(prepared.path.as_path())
        }) {
            self.switch(index, window, cx);
            return;
        }
        let current = self.active();
        let current = current.read(cx);
        let replace_untitled = self.documents.len() == 1
            && current.path.is_none()
            && current.imported_from.is_none()
            && current.draft_name == "Untitled"
            && !current.is_dirty()
            && !current.has_field_drafts(cx);
        let index = if replace_untitled {
            self.active
        } else {
            self.add_empty(window, cx)
        };
        self.documents[index].view.update(cx, |view, cx| {
            view.show_nodes_workspace(cx);
            view.install(prepared, window, cx);
            cx.notify();
        });
        self.switch(index, window, cx);
        cx.notify();
    }

    fn switch(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index == self.active
            || index >= self.documents.len()
            || self.confirming_close
            || self.active().read(cx).is_busy()
        {
            return;
        }
        let results = self.active().update(cx, |view, _| {
            view.pause_preview();
            view.node_previews.clear();
            let stored = std::mem::take(&mut view.results);
            view.results.set_visible(stored.is_visible());
            stored
        });
        self.active = index;
        self.active().update(cx, |view, cx| {
            view.results = results.with_visibility(view.results.is_visible());
            view.focus.focus(window, cx);
            cx.notify();
        });
        cx.notify();
    }

    pub(super) fn new_document(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.confirming_close || self.active().read(cx).is_busy() {
            return;
        }
        let index = self.add_empty(window, cx);
        self.switch(index, window, cx);
    }

    pub(super) fn duplicate_document(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let source = self.active();
        let current = source.read(cx);
        if self.confirming_close || current.is_busy() {
            return;
        }
        if current.has_field_drafts(cx) {
            source.update(cx, |view, cx| {
                view.error = Some("Correct or reset field text before duplicating.".into());
                cx.notify();
            });
            return;
        }
        let document = current.document.clone();
        let time = current.sample_time;
        let collapsed = current.canvas.collapsed_rows(&current.navigation);
        let selected_row = current
            .navigation
            .rows
            .iter()
            .position(|row| Some(&row.key) == current.selection.as_ref());
        source.update(cx, |view, cx| {
            view.work = Some(Work::Duplicating);
            view.status = None;
            view.error = None;
            cx.notify();
        });
        let background = cx.background_executor().clone();
        cx.spawn_in(window, async move |owner, cx| {
            let result = background
                .spawn(async move { model::duplicate(&document) })
                .await;
            let _ = owner.update_in(cx, |workspace, window, cx| {
                source.update(cx, |view, cx| {
                    view.work = None;
                    view.error = result.as_ref().err().cloned();
                    cx.notify();
                });
                let Ok(document) = result else {
                    return;
                };
                let owner = cx.weak_entity();
                let name = (1..=workspace.documents.len() + 1)
                    .map(|number| format!("Untitled {number}"))
                    .find(|name| {
                        workspace
                            .documents
                            .iter()
                            .all(|doc| doc.view.read(cx).draft_name != *name)
                    })
                    .expect("one available untitled name");
                let copy = cx.new(|cx| {
                    let mut view = GraphDocumentView::with_document_in_workspace(
                        document,
                        None,
                        time,
                        false,
                        Some(owner),
                        window,
                        cx,
                    );
                    view.canvas.visible = true;
                    view.ensure_history_binding();
                    view.draft_name = name;
                    view.dirty = true;
                    // The native copy retains the row order but assigns fresh occurrence IDs.
                    view.canvas
                        .restore_collapsed_rows(&view.navigation, &collapsed);
                    if let Some(row) =
                        selected_row.and_then(|index| view.navigation.rows.get(index))
                    {
                        let key = row.key.clone();
                        view.canvas
                            .follow_selection(&view.navigation, &row.key, &row.target);
                        view.select(Some(key), window, cx);
                        view.start_preview(cx);
                    }
                    view
                });
                let index = workspace.add_document(copy, window, cx);
                workspace.switch(index, window, cx);
            });
        })
        .detach();
    }

    pub(super) fn other_document_paths(&self, owner: gpui_kit::EntityId, cx: &App) -> Vec<PathBuf> {
        self.documents
            .iter()
            .filter(|document| document.view.entity_id() != owner)
            .flat_map(|document| {
                let view = document.view.read(cx);
                view.path.iter().chain(&view.imported_from).cloned()
            })
            .collect()
    }

    pub(super) fn close_document(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.confirming_close || self.active().read(cx).is_busy() {
            return;
        }
        self.advance_close(CloseScope::Current, window, cx);
    }

    pub(super) fn close_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.confirming_close || self.documents.iter().any(|doc| doc.view.read(cx).is_busy()) {
            return;
        }
        self.advance_close(CloseScope::All, window, cx);
    }

    fn advance_close(&mut self, scope: CloseScope, window: &mut Window, cx: &mut Context<Self>) {
        loop {
            if scope == CloseScope::All {
                self.switch(0, window, cx);
            }
            let active = self.active();
            let view = active.read(cx);
            let has_drafts = view.has_field_drafts(cx);
            if view.is_dirty() || has_drafts {
                self.confirm_close(scope, active, has_drafts, window, cx);
                return;
            }
            let last = self.documents.len() == 1;
            self.remove_active(window, cx);
            if last || scope == CloseScope::Current {
                return;
            }
        }
    }

    fn confirm_close(
        &mut self,
        scope: CloseScope,
        active: Entity<GraphDocumentView>,
        has_drafts: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.confirming_close = true;
        let name = self.documents[self.active]
            .label
            .trim_end_matches(" *")
            .to_owned();
        let owner = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let owner = owner.clone();
            let restore = owner.clone();
            let discard = owner.clone();
            let discard_target = active.clone();
            let active = active.clone();
            dialog
                .title("Save changes before closing?")
                .child(
                    div()
                        .id("graph-close-document-name")
                        .test_support()
                        .aria_label(name.clone())
                        .child(name.clone()),
                )
                .when(has_drafts, |dialog| {
                    dialog.child("Correct or reset field text before saving.")
                })
                .footer(
                    DialogFooter::new()
                        .child(DialogClose::new().child(Button::new("cancel").label("Cancel")))
                        .child(
                            Button::new("graph-close-discard")
                                .danger()
                                .label("Discard changes")
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    let _ = discard.update(cx, |host, cx| {
                                        host.confirming_close = false;
                                        if host.active() == discard_target {
                                            host.close_and_continue(scope, window, cx);
                                        }
                                    });
                                }),
                        )
                        .child(
                            DialogAction::new().child(
                                Button::new("graph-close-save")
                                    .primary()
                                    .label("Save")
                                    .disabled(has_drafts),
                            ),
                        ),
                )
                .on_ok(move |_, window, cx| {
                    if has_drafts {
                        return false;
                    }
                    let owner = owner.clone();
                    let active = active.clone();
                    window.defer(cx, move |_, cx| {
                        let Some(workspace) = owner.upgrade() else {
                            return;
                        };
                        if workspace.read(cx).active() != active {
                            return;
                        }
                        let started = active.update(cx, |view, cx| {
                            view.save(false, cx);
                            view.is_busy()
                        });
                        if started {
                            workspace.update(cx, |workspace, _| {
                                workspace.close_after_save = Some(scope);
                            });
                        }
                    });
                    true
                })
                .on_close(move |_, _, cx| {
                    let _ = restore.update(cx, |host, cx| {
                        host.confirming_close = false;
                        cx.notify();
                    });
                })
        });
        cx.notify();
    }

    fn finish_close_save(
        &mut self,
        saved: &Entity<GraphDocumentView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(scope) = self.close_after_save else {
            return;
        };
        let view = saved.read(cx);
        if self.active() != *saved || view.is_busy() {
            return;
        }
        self.close_after_save = None;
        if !view.is_dirty() && !view.has_field_drafts(cx) {
            self.close_and_continue(scope, window, cx);
        }
    }

    fn close_and_continue(
        &mut self,
        scope: CloseScope,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let last = self.documents.len() == 1;
        self.remove_active(window, cx);
        if !last && scope == CloseScope::All {
            self.advance_close(scope, window, cx);
        }
    }

    fn remove_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.active().read(cx).is_busy() {
            return;
        }
        let remove = self.active;
        let next = if self.documents.len() == 1 {
            self.add_empty(window, cx)
        } else if remove > 0 {
            remove - 1
        } else {
            1
        };
        self.switch(next, window, cx);
        self.documents
            .remove(remove)
            .view
            .update(cx, |view, _| view.retire());
        if remove < self.active {
            self.active -= 1;
        }
        cx.notify();
    }

    fn confirm_window_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.confirming_close
            || self
                .documents
                .iter()
                .any(|document| document.view.read(cx).is_busy())
        {
            return false;
        }
        let dirty = self.documents.iter().any(|document| {
            let view = document.view.read(cx);
            view.is_dirty() || view.has_field_drafts(cx)
        });
        if !dirty {
            return true;
        }
        self.confirming_close = true;
        let owner = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let owner = owner.clone();
            discard_dialog(
                dialog,
                "Close window with unsaved documents?",
                "Save any open documents whose changes you want to keep.",
            )
            .on_ok(|_, window, _| {
                window.remove_window();
                true
            })
            .on_close(move |_, _, cx| {
                let _ = owner.update(cx, |host, cx| {
                    host.confirming_close = false;
                    cx.notify();
                });
            })
        });
        false
    }

    pub(super) fn render_strip(
        &self,
        owner: WeakEntity<Self>,
        disabled: bool,
        cx: &App,
    ) -> impl IntoElement + use<> {
        let theme = cx.theme();
        div()
            .id("graph-open-documents")
            .test_support()
            .role(gpui_kit::Role::TabList)
            .aria_label("Open documents")
            .v_flex()
            .w(rems(7.))
            .h_full()
            .min_h_0()
            .flex_shrink_0()
            .border_r_1()
            .border_color(theme.border)
            .child(
                div()
                    .id("graph-open-documents-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .child(div().v_flex().gap_2().p_2().children(
                        self.documents.iter().enumerate().map(|(index, document)| {
                            let owner = owner.clone();
                            Button::new(("graph-open-document", document.view.entity_id()))
                                .ghost()
                                .w_full()
                                .h(rems(5.75))
                                .tab_index(0)
                                .selected(index == self.active)
                                .disabled(disabled || self.confirming_close)
                                .accessibility_label(document.label.clone())
                                .tooltip(document.path.as_ref().map_or_else(
                                    || document.label.clone(),
                                    |path| path.to_string_lossy().into_owned(),
                                ))
                                .child(
                                    div()
                                        .v_flex()
                                        .w_full()
                                        .min_w_0()
                                        .gap_1()
                                        .child(
                                            div()
                                                .w_full()
                                                .h(rems(3.75))
                                                .relative()
                                                .overflow_hidden()
                                                .rounded_sm()
                                                .bg(theme.muted)
                                                .children(document.thumbnail.as_ref().map(
                                                    |raster| {
                                                        img(raster.clone())
                                                            .absolute()
                                                            .size_full()
                                                            .object_fit(
                                                                gpui_kit::ObjectFit::Contain,
                                                            )
                                                    },
                                                )),
                                        )
                                        .child(
                                            div()
                                                .w_full()
                                                .truncate()
                                                .text_xs()
                                                .child(document.label.clone()),
                                        ),
                                )
                                .on_click(move |event, window, cx| {
                                    let _ = owner.update(cx, |workspace, cx| {
                                        workspace.switch(index, window, cx);
                                        if event.click_count() == 2 && workspace.active == index {
                                            workspace.active().update(cx, |view, cx| {
                                                view.open_rename_document(window, cx);
                                            });
                                        }
                                    });
                                })
                        }),
                    )),
            )
    }
}

impl Render for DocumentWorkspace {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.active())
    }
}

fn discard_dialog(
    dialog: gpui_kit::component::dialog::Dialog,
    title: &'static str,
    message: &'static str,
) -> gpui_kit::component::dialog::Dialog {
    dialog.title(title).child(message).footer(
        DialogFooter::new()
            .child(DialogClose::new().child(Button::new("cancel").label("Cancel")))
            .child(DialogAction::new().child(Button::new("ok").danger().label("Discard changes"))),
    )
}
