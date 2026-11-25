//! Recent paths shared by the document workspaces in this application.
use super::{
    DocumentFormat, model,
    preferences::{File, decode_path, encode_path},
};
use gpui_kit::{AnyWindowHandle, App, AsyncApp, Global};
use std::path::{Path, PathBuf};

const VERSION: &[u8] = b"MRP1";
const MAX_FILES: usize = 15;
const MAX_FOLDERS: usize = 10;

pub(super) async fn missing(path: &Path, directory: bool, cx: &AsyncApp) -> bool {
    let path = path.to_owned();
    cx.background_executor()
        .spawn(async move {
            match std::fs::metadata(path) {
                Ok(metadata) => metadata.is_dir() != directory,
                Err(error) => error.kind() == std::io::ErrorKind::NotFound,
            }
        })
        .await
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RecentFile {
    path: PathBuf,
    format: DocumentFormat,
}

impl RecentFile {
    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    pub(super) fn format(&self) -> DocumentFormat {
        self.format
    }

    pub(super) fn label(&self) -> String {
        self.path
            .file_name()
            .unwrap_or(self.path.as_os_str())
            .to_string_lossy()
            .into_owned()
    }
}

#[derive(Default)]
pub(super) struct RecentPaths {
    files: Vec<RecentFile>,
    folders: Vec<PathBuf>,
}

impl Global for RecentPaths {}

impl RecentPaths {
    pub(super) fn initialize(cx: &mut App) {
        if cx.try_global::<Self>().is_none() {
            let state = File::RecentPaths
                .read(cx)
                .and_then(|bytes| Self::decode(&bytes))
                .unwrap_or_default();
            cx.set_global(state);
        }
    }

    pub(super) fn files(cx: &App) -> &[RecentFile] {
        cx.try_global::<Self>()
            .map_or(&[], |state| state.files.as_slice())
    }

    pub(super) fn folders(cx: &App) -> &[PathBuf] {
        cx.try_global::<Self>()
            .map_or(&[], |state| state.folders.as_slice())
    }

    pub(super) fn remember_document(
        path: &Path,
        format: DocumentFormat,
        cx: &mut App,
    ) -> Result<(), String> {
        let parent = path
            .parent()
            .filter(|parent| parent.is_absolute() && path.file_name().is_some())
            .ok_or_else(|| "A recent document needs an absolute file path.".to_owned())?;
        Self::change(cx, |state| {
            state.files.retain(|file| file.path != path);
            state.files.insert(
                0,
                RecentFile {
                    path: path.to_owned(),
                    format,
                },
            );
            state.files.truncate(MAX_FILES);
            state.promote_folder(parent);
        })
    }

    pub(super) fn remember_folder(path: &Path, cx: &mut App) -> Result<(), String> {
        if !path.is_absolute() {
            return Err("A recent folder needs an absolute path.".into());
        }
        Self::change(cx, |state| state.promote_folder(path))
    }

    pub(super) fn forget_file(path: &Path, cx: &mut App) -> Result<(), String> {
        Self::change(cx, |state| state.files.retain(|file| file.path != path))
    }

    pub(super) fn forget_folder(path: &Path, cx: &mut App) -> Result<(), String> {
        Self::change(cx, |state| state.folders.retain(|folder| folder != path))
    }

    fn promote_folder(&mut self, path: &Path) {
        self.folders.retain(|folder| folder != path);
        self.folders.insert(0, path.to_owned());
        self.folders.truncate(MAX_FOLDERS);
    }

    fn change(cx: &mut App, modify: impl FnOnce(&mut Self)) -> Result<(), String> {
        Self::initialize(cx);
        let state = cx.global_mut::<Self>();
        modify(state);
        let data = state.encode()?;
        File::RecentPaths.write(&data, cx)
    }

    fn encode(&self) -> Result<Vec<u8>, String> {
        let mut data = VERSION.to_vec();
        data.push(self.files.len() as u8);
        for file in &self.files {
            data.push(match file.format {
                DocumentFormat::Mycela => 0,
                DocumentFormat::Gtx => 1,
            });
            write_path(&mut data, &file.path)?;
        }
        data.push(self.folders.len() as u8);
        for folder in &self.folders {
            write_path(&mut data, folder)?;
        }
        Ok(data)
    }

    fn decode(data: &[u8]) -> Option<Self> {
        let mut data = data.strip_prefix(VERSION)?;
        let mut state = Self::default();
        for _ in 0..read_count(&mut data, MAX_FILES)? {
            let (&kind, rest) = data.split_first()?;
            let format = match kind {
                0 => DocumentFormat::Mycela,
                1 => DocumentFormat::Gtx,
                _ => return None,
            };
            data = rest;
            let path = read_path(&mut data)?;
            path.file_name()?;
            if !state.files.iter().any(|file| file.path == path) {
                state.files.push(RecentFile { path, format });
            }
        }
        for _ in 0..read_count(&mut data, MAX_FOLDERS)? {
            let path = read_path(&mut data)?;
            if !state.folders.contains(&path) {
                state.folders.push(path);
            }
        }
        data.is_empty().then_some(state)
    }
}

fn write_path(data: &mut Vec<u8>, path: &Path) -> Result<(), String> {
    let path = encode_path(path);
    let size =
        u32::try_from(path.len()).map_err(|_| "The recent path is too long to save.".to_owned())?;
    data.extend(size.to_le_bytes());
    data.extend(path);
    Ok(())
}

fn read_path(data: &mut &[u8]) -> Option<PathBuf> {
    let (size, remaining) = data.split_at_checked(4)?;
    let size = u32::from_le_bytes(size.try_into().ok()?) as usize;
    let (bytes, remaining) = remaining.split_at_checked(size)?;
    *data = remaining;
    let path = decode_path(bytes.to_vec())?;
    path.is_absolute().then_some(path)
}

fn read_count(data: &mut &[u8], limit: usize) -> Option<usize> {
    let (&count, remaining) = data.split_first()?;
    *data = remaining;
    let count = usize::from(count);
    (count <= limit).then_some(count)
}

fn open_title(format: DocumentFormat) -> &'static str {
    match format {
        DocumentFormat::Mycela => "Open Mycela project",
        DocumentFormat::Gtx => "Import Genetica GTX",
    }
}

// The desktop portal takes the starting directory as a native byte path.
// GPUI's Open dialog options do not have a starting directory field.
#[cfg(all(target_os = "linux", not(test)))]
async fn open_linux_in_folder(
    window: AnyWindowHandle,
    folder: PathBuf,
    format: DocumentFormat,
    cx: &mut AsyncApp,
) -> Result<Option<PathBuf>, String> {
    use ashpd::desktop::file_chooser::{FileFilter, OpenFileRequest};
    use gpui_kit::AppContext;
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
    let (filter, accept) = match format {
        DocumentFormat::Mycela => (
            FileFilter::new("Mycela projects").glob("*.[mM][yY][cC][eE][lL][aA]"),
            "Open",
        ),
        DocumentFormat::Gtx => (
            FileFilter::new("Genetica textures").glob("*.[gG][tT][xX]"),
            "Import",
        ),
    };
    let request = OpenFileRequest::default()
        .identifier(identifier)
        .modal(true)
        .title(open_title(format))
        .accept_label(accept)
        .multiple(false)
        .directory(false)
        .current_filter(filter.clone())
        .filters([filter, FileFilter::new("All files").glob("*")])
        .current_folder(folder)
        .map_err(model::failure)?
        .send()
        .await
        .map_err(model::failure)?;
    match request.response() {
        Ok(response) => {
            let [uri] = response.uris() else {
                return Err("Choose one document.".into());
            };
            let path = url::Url::parse(uri.as_str())
                .ok()
                .and_then(|uri| uri.to_file_path().ok())
                .ok_or_else(|| "Choose a local document file.".to_owned())?;
            Ok(Some(path))
        }
        Err(ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled)) => Ok(None),
        Err(error) => Err(model::failure(error)),
    }
}

pub(super) async fn open_in_folder(
    window: AnyWindowHandle,
    folder: PathBuf,
    format: DocumentFormat,
    cx: &mut AsyncApp,
) -> Result<Option<PathBuf>, String> {
    #[cfg(all(target_os = "linux", not(test)))]
    if !cx.is_test() {
        return open_linux_in_folder(window, folder, format, cx).await;
    }
    #[cfg(any(not(target_os = "linux"), test))]
    let _ = (window, folder);

    let prompt = cx.update(|cx| {
        cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(open_title(format).into()),
        })
    });
    match prompt.await {
        Ok(Ok(Some(mut paths))) if paths.len() == 1 => Ok(paths.pop()),
        Ok(Ok(None)) => Ok(None),
        Ok(Ok(Some(_))) => Err("Choose one document.".into()),
        Ok(Err(error)) => Err(model::failure(error)),
        Err(_) => Err("The file picker closed before returning a result.".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph_document::view::preferences::TestDirectory;
    use gpui_kit::TestAppContext;

    #[gpui_kit::test]
    fn recent_paths_reopen_with_order_limits_and_exact_file_identity(cx: &mut TestAppContext) {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../port/.scratch/mycela-recent-paths-tests")
            .join(std::process::id().to_string());
        cx.update(|cx| {
            cx.set_global(TestDirectory(path.clone()));
            let files: Vec<_> = (0..MAX_FILES + 2)
                .map(|index| path.join(format!("project-{index}/Texture.mycela")))
                .collect();
            for file in &files {
                RecentPaths::remember_document(file, DocumentFormat::Mycela, cx).unwrap();
            }
            let stored = RecentPaths::files(cx);
            assert_eq!(stored.len(), MAX_FILES);
            assert_eq!(stored.last().unwrap().path(), files[2]);
            assert_eq!(stored.first().unwrap().path(), files.last().unwrap());
            assert!(stored.iter().all(|file| file.label() == "Texture.mycela"));
            assert_eq!(RecentPaths::folders(cx).len(), MAX_FOLDERS);

            let promoted = &files[8];
            RecentPaths::remember_document(promoted, DocumentFormat::Gtx, cx).unwrap();
            let stored = RecentPaths::files(cx);
            assert_eq!(stored.len(), MAX_FILES);
            assert_eq!(stored[0].path(), promoted);
            assert_eq!(stored[0].format(), DocumentFormat::Gtx);
            assert_eq!(RecentPaths::folders(cx)[0], promoted.parent().unwrap());
            assert_eq!(
                RecentPaths::folders(cx)
                    .iter()
                    .filter(|folder| folder.as_path() == promoted.parent().unwrap())
                    .count(),
                1
            );

            let export = path.join("Exports é");
            RecentPaths::remember_folder(&export, cx).unwrap();
            assert_eq!(RecentPaths::files(cx)[0].path(), promoted);
            assert_eq!(RecentPaths::folders(cx)[0], export);

            let alternate = path.join("Independent 雪").join(legacy_file_name());
            RecentPaths::remember_document(&alternate, DocumentFormat::Gtx, cx).unwrap();
            assert_eq!(RecentPaths::files(cx)[0].path(), alternate);
            assert!(!alternate.exists());
            RecentPaths::forget_file(promoted, cx).unwrap();
            RecentPaths::forget_folder(&export, cx).unwrap();
            let files = RecentPaths::files(cx).to_vec();
            let folders = RecentPaths::folders(cx).to_vec();
            assert!(files.iter().all(|entry| entry.path() != promoted));
            assert!(!folders.contains(&export));

            cx.remove_global::<RecentPaths>();
            RecentPaths::initialize(cx);
            assert_eq!(RecentPaths::files(cx), files);
            assert_eq!(RecentPaths::folders(cx), folders);
            assert_eq!(RecentPaths::files(cx)[0].path(), alternate);
        });
        std::fs::remove_dir_all(path).unwrap();
    }

    fn legacy_file_name() -> std::ffi::OsString {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            std::ffi::OsString::from_vec(b"material\n\xff.gtx".to_vec())
        }
        #[cfg(not(unix))]
        {
            "material-é.gtx".into()
        }
    }
}
