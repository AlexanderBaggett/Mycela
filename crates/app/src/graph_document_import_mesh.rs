use super::preferences::File as PreferenceFile;
use super::*;
use gpui_kit::AsyncApp;
use mycela_engine::ImportedMesh;
use std::io::Read;

impl GraphDocumentView {
    pub(super) fn render_mesh_import(
        &self,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        let node = self.selected_node()?;
        if !node.is_import_3d() {
            return None;
        }
        let count = node.imported_mesh().map(|mesh| mesh.model_count());
        let button = Button::new("graph-import-mesh")
            .small()
            .label(if count.is_some() {
                "Replace model…"
            } else {
                "Import model…"
            })
            .disabled(busy)
            .on_click(cx.listener(|view, _, _, cx| view.choose_mesh(cx)));
        let clear = count.is_some().then(|| {
            Button::new("graph-clear-mesh")
                .small()
                .outline()
                .label("Clear model")
                .disabled(busy)
                .on_click(cx.listener(|view, _, window, cx| {
                    if view.can_change_mesh(cx) {
                        view.commit_mesh(None, window, cx);
                        view.tree.update(cx, |tree, cx| tree.focus(window, cx));
                    }
                }))
        });
        let detail = match count {
            Some(1) => "1 embedded model".into(),
            Some(count) => format!("{count} embedded models"),
            None => "OBJ or 3DS".into(),
        };
        Some(
            div()
                .h_flex()
                .flex_wrap()
                .gap_2()
                .child(button)
                .children(clear)
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(detail),
                ),
        )
    }

    fn can_change_mesh(&mut self, cx: &mut Context<Self>) -> bool {
        if self.is_busy() {
            return false;
        }
        if self.inspector.is_changed(cx) {
            self.error = Some("Correct or reset field text before changing the model.".into());
            cx.notify();
            return false;
        }
        self.selected_node().is_some_and(|node| node.is_import_3d())
    }

    pub(super) fn initial_mesh_folder(&self, cx: &App) -> Option<PathBuf> {
        self.mesh_import_folder
            .clone()
            .filter(|folder| folder.is_dir())
            .or_else(|| {
                PreferenceFile::ModelImportFolder
                    .read_path(cx)
                    .filter(|p| p.is_dir())
            })
            .or_else(|| dirs::document_dir().filter(|folder| folder.is_dir()))
    }

    fn choose_mesh(&mut self, cx: &mut Context<Self>) {
        if !self.can_change_mesh(cx) {
            return;
        }
        let expected = self.selected_node();
        let selection = self.selection.clone();
        let revision = self.revision;
        let window_handle = self.window;
        let background = cx.background_executor().clone();
        let initial_folder = self.initial_mesh_folder(cx);
        let ticket = Ticket::new();
        self.ticket = Some(ticket.clone());
        self.work = Some(Work::ImportingMesh);
        self.error = None;
        self.status = None;
        cx.notify();
        cx.spawn(async move |owner, cx| {
            let result = match prompt_for_mesh(window_handle, initial_folder, cx).await {
                Ok(Some(paths)) if paths.len() == 1 && ticket.is_current() => {
                    let path = paths.into_iter().next().expect("one path");
                    Ok(Some(
                        background
                            .spawn(async move {
                                let mesh = read_mesh(&path);
                                (path, mesh)
                            })
                            .await,
                    ))
                }
                Ok(Some(_)) if ticket.is_current() => Err("Choose one model file.".into()),
                Ok(_) => Ok(None),
                Err(error) => Err(format!("Couldn’t open the file picker. {error}")),
            };
            let _ = cx.update_window(window_handle, |_, window, cx| {
                owner.update(cx, |view, cx| {
                    if !view.owns(&ticket) {
                        return;
                    }
                    view.ticket = None;
                    view.work = None;
                    if view.is_original_selection(&selection, &expected, revision) {
                        match result {
                            Ok(Some((path, mesh))) => {
                                if let Some(folder) = path.parent().filter(|p| p.is_absolute()) {
                                    view.mesh_import_folder = Some(folder.to_owned());
                                    if let Err(error) =
                                        PreferenceFile::ModelImportFolder.write_path(folder, cx)
                                    {
                                        view.status = Some(format!(
                                            "Couldn’t remember the model folder. {error}"
                                        ));
                                    }
                                }
                                match mesh {
                                    Ok(mesh) => view.commit_mesh(Some(mesh), window, cx),
                                    Err(error) => view.error = Some(error),
                                }
                            }
                            Ok(None) => {}
                            Err(error) => view.error = Some(error),
                        }
                    }
                    cx.notify();
                })
            });
        })
        .detach();
    }

    fn commit_mesh(
        &mut self,
        mesh: Option<ImportedMesh>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = self
            .selection
            .as_deref()
            .and_then(|key| self.navigation.target(key))
        else {
            return;
        };
        let Some(node) = target.node() else {
            return;
        };
        if mesh.is_none() && node.imported_mesh().is_none() {
            return;
        }
        let replacement = match mesh {
            Some(mesh) => node.with_imported_mesh(mesh),
            None => node.without_imported_mesh(),
        };
        let replacement = match replacement {
            Ok(node) => node,
            Err(error) => {
                self.error = Some(model::failure(error));
                return;
            }
        };
        let stamp = self.begin_history_edit();
        self.revision += 1;
        self.error = self.edit_target(&target, Some(&replacement)).err();
        self.finish_history_edit(stamp, self.error.is_none());
        self.rebuild(window, cx);
        self.start_preview(cx);
    }
}

// GPUI's file-open options do not carry a starting directory. Use the same
// desktop portal on Linux so Import 3D can restore its own folder.
#[cfg(all(target_os = "linux", not(test)))]
async fn prompt_for_mesh(
    window: AnyWindowHandle,
    folder: Option<PathBuf>,
    cx: &mut AsyncApp,
) -> Result<Option<Vec<PathBuf>>, String> {
    use ashpd::desktop::file_chooser::{FileFilter, OpenFileRequest};
    use raw_window_handle::{HasDisplayHandle, HasWindowHandle};

    let (window_handle, display_handle) = cx
        .update_window(window, |_, window, _| {
            (
                HasWindowHandle::window_handle(window)
                    .ok()
                    .map(|handle| handle.as_raw()),
                HasDisplayHandle::display_handle(window)
                    .ok()
                    .map(|handle| handle.as_raw()),
            )
        })
        .map_err(model::failure)?;
    let identifier = match window_handle {
        Some(handle) => {
            ashpd::WindowIdentifier::from_raw_handle(&handle, display_handle.as_ref()).await
        }
        None => None,
    };
    let request = OpenFileRequest::default()
        .identifier(identifier)
        .modal(true)
        .title("Import model")
        .accept_label("Import")
        .multiple(false)
        .directory(false)
        .filter(
            FileFilter::new("OBJ and 3DS models")
                .glob("*.[oO][bB][jJ]")
                .glob("*.3[dD][sS]"),
        )
        .filter(FileFilter::new("All files").glob("*"))
        .current_folder::<PathBuf>(folder)
        .map_err(model::failure)?
        .send()
        .await
        .map_err(model::failure)?;
    match request.response() {
        Ok(response) => Ok(Some(
            response
                .uris()
                .iter()
                .filter_map(|uri| url::Url::parse(uri.as_str()).ok()?.to_file_path().ok())
                .collect(),
        )),
        Err(ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled)) => Ok(None),
        Err(error) => Err(model::failure(error)),
    }
}

#[cfg(any(not(target_os = "linux"), test))]
async fn prompt_for_mesh(
    _window: AnyWindowHandle,
    _folder: Option<PathBuf>,
    cx: &mut AsyncApp,
) -> Result<Option<Vec<PathBuf>>, String> {
    let prompt = cx.update(|cx| {
        cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Import an OBJ or 3DS model".into()),
        })
    });
    match prompt.await {
        Ok(result) => result.map_err(model::failure),
        Err(_) => Ok(None),
    }
}

fn read_mesh(path: &Path) -> Result<ImportedMesh, String> {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !matches!(extension.as_str(), "obj" | "3ds") {
        return Err("Choose an OBJ or 3DS file.".into());
    }
    let file =
        std::fs::File::open(path).map_err(|error| format!("Couldn’t open the model. {error}"))?;
    if !file.metadata().is_ok_and(|metadata| metadata.is_file()) {
        return Err("Choose a regular model file.".into());
    }
    let mut bytes = Vec::new();
    file.take(i32::MAX as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Couldn’t read the model. {error}"))?;
    if bytes.len() > i32::MAX as usize {
        return Err("The model file exceeds the supported size.".into());
    }
    if extension == "3ds" {
        ImportedMesh::from_3ds(&bytes)
    } else {
        let source = std::str::from_utf8(&bytes)
            .map_err(|_| "The OBJ file contains invalid UTF-8 text.".to_string())?;
        ImportedMesh::from_obj(source)
    }
}
