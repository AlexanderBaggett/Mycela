//! Rename the document identified by either of its workspace labels.
use super::*;

struct RenameDraft {
    input: Entity<InputState>,
    error: Option<&'static str>,
}

impl GraphDocumentView {
    pub(super) fn document_name(&self) -> String {
        self.path.as_deref().and_then(Path::file_name).map_or_else(
            || self.draft_name.clone(),
            |name| name.to_string_lossy().into_owned(),
        )
    }

    pub(super) fn suggested_document_stem(&self) -> String {
        self.path.as_deref().and_then(Path::file_stem).map_or_else(
            || self.draft_name.clone(),
            |name| name.to_string_lossy().into_owned(),
        )
    }

    pub(super) fn open_rename_document(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.closed || self.is_busy() || self.workspace.is_none() {
            return;
        }
        let current = self.suggested_document_stem();
        let input = cx.new(|cx| InputState::new(window, cx).default_value(current.clone()));
        let draft = cx.new(|_| RenameDraft { input, error: None });
        let source = self.path.clone();
        let owner = cx.weak_entity();
        let focused = Rc::new(Cell::new(false));
        self.modal = true;
        window.open_dialog(cx, move |dialog, _, cx| {
            let editing = draft.clone();
            let draft = draft.read(cx);
            let name = draft.input.clone();
            let focus = name.clone();
            let focused = focused.clone();
            let save = owner.clone();
            let close = owner.clone();
            let source = source.clone();
            let current = current.clone();
            let extension = source
                .as_deref()
                .and_then(Path::extension)
                .and_then(|extension| extension.to_str())
                .unwrap_or(DocumentFormat::Mycela.extension())
                .to_owned();
            dialog
                .title("Rename document")
                .child(
                    Field::new().label("Name").child(
                        div()
                            .relative()
                            .on_prepaint(move |_, window, cx| {
                                if !focused.replace(true) {
                                    let focus = focus.clone();
                                    window.defer(cx, move |window, cx| {
                                        focus.update(cx, |input, cx| {
                                            input.focus(window, cx);
                                            input.select_all(window, cx);
                                        });
                                    });
                                }
                            })
                            .child(Input::new(&name).id("graph-rename-name").aria_label("Name")),
                    ),
                )
                .when_some(draft.error, |dialog, error| {
                    dialog.child(
                        div()
                            .id("graph-rename-error")
                            .test_support()
                            .role(gpui_kit::Role::Alert)
                            .text_color(cx.theme().danger)
                            .child(error),
                    )
                })
                .footer(
                    DialogFooter::new()
                        .child(DialogClose::new().child(Button::new("cancel").label("Cancel")))
                        .child(
                            DialogAction::new().child(
                                Button::new("graph-rename-confirm")
                                    .primary()
                                    .label("Rename"),
                            ),
                        ),
                )
                .on_ok(move |_, _, cx| {
                    let raw = name.read(cx).value();
                    let name = match document_stem(raw.as_ref(), &extension) {
                        Ok(name) => name,
                        Err(error) => {
                            editing.update(cx, |draft, cx| {
                                draft.error = Some(error);
                                cx.notify();
                            });
                            return false;
                        }
                    };
                    let _ = save.update(cx, |view, cx| {
                        if name != current {
                            view.rename_document(name.into(), source.as_deref(), &extension, cx);
                        }
                    });
                    true
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

    fn rename_document(
        &mut self,
        name: String,
        source: Option<&Path>,
        extension: &str,
        cx: &mut Context<Self>,
    ) {
        self.error = None;
        self.status = None;
        let Some(source) = source else {
            self.draft_name = name;
            cx.notify();
            return;
        };
        let source = source.to_owned();
        let destination = source.with_file_name(format!("{name}.{extension}"));
        let open_paths = self.other_open_document_paths(cx);
        let ticket = Ticket::new();
        self.ticket = Some(ticket.clone());
        self.work = Some(Work::Renaming);
        let executor = cx.background_executor().clone();
        cx.spawn(async move |owner, cx| {
            let previous = source.clone();
            let result = executor
                .spawn(async move {
                    Self::validate_save_destination(
                        &destination,
                        DocumentFormat::Mycela,
                        &open_paths,
                        None,
                    )?;
                    rename_file(&source, &destination).map(|()| destination)
                })
                .await;
            let _ = owner.update(cx, |view, cx| {
                if !view.owns(&ticket) {
                    return;
                }
                view.ticket = None;
                view.work = None;
                ticket.retire();
                match result {
                    Ok(path) => {
                        if let Err(error) = recent_paths::RecentPaths::remember_renamed_document(
                            &previous, &path, cx,
                        ) {
                            eprintln!("Could not save recent Mycela paths: {error}");
                        }
                        view.path = Some(path);
                    }
                    Err(error) => view.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

fn document_stem<'a>(input: &'a str, extension: &str) -> Result<&'a str, &'static str> {
    let input = input.trim();
    let name = input.rsplit_once('.').map_or(input, |(stem, suffix)| {
        if suffix.eq_ignore_ascii_case(extension) {
            stem.trim_end()
        } else {
            input
        }
    });
    if name.is_empty() || matches!(name, "." | "..") {
        return Err("Enter a document name.");
    }
    if name
        .chars()
        .any(|c| matches!(c, '/' | '\\') || c.is_control())
    {
        return Err("Enter a name without a folder path.");
    }
    Ok(name)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn rename_file(source: &Path, destination: &Path) -> Result<(), String> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};

    renameat_with(CWD, source, CWD, destination, RenameFlags::NOREPLACE).map_err(|error| {
        if error == rustix::io::Errno::EXIST {
            "A file with this name already exists in this folder.".into()
        } else {
            format!("Couldn’t rename the document: {error}")
        }
    })
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn rename_file(_: &Path, _: &Path) -> Result<(), String> {
    Err("Renaming a saved document is unavailable on this platform.".into())
}
