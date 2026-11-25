//! Per-user settings for the graph editor.
use super::{Ticket, model};
use gpui_kit::App;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy)]
pub(super) enum File {
    ResourceLibrary,
    PresetFolder,
    EnvironmentSquareProjection,
    EnvironmentPreviewConverter,
    Export,
    ModelImportFolder,
    RecentPaths,
    OverviewPresetFolder,
}

impl File {
    fn path(self, cx: &App) -> Option<PathBuf> {
        Some(directory(cx)?.join(match self {
            Self::ResourceLibrary => "environment-resource-library.path",
            Self::PresetFolder => "environment-presets.path",
            Self::EnvironmentSquareProjection => "environment-square-projection.conf",
            Self::EnvironmentPreviewConverter => "environment-preview-converter.path",
            Self::Export => "environment-export.conf",
            Self::ModelImportFolder => "model-import.path",
            Self::RecentPaths => "recent-paths.bin",
            Self::OverviewPresetFolder => "overview-presets.path",
        }))
    }

    pub(super) fn read(self, cx: &App) -> Option<Vec<u8>> {
        std::fs::read(self.path(cx)?).ok()
    }

    pub(super) fn write(self, bytes: &[u8], cx: &App) -> Result<(), String> {
        let Some(path) = self.path(cx) else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(model::failure)?;
        }
        model::save_bytes(bytes, path, &Ticket::new()).map(|_| ())
    }

    pub(super) fn read_path(self, cx: &App) -> Option<PathBuf> {
        let path = decode_path(self.read(cx)?)?;
        path.is_absolute().then_some(path)
    }

    pub(super) fn write_path(self, path: &Path, cx: &App) -> Result<(), String> {
        self.write(&encode_path(path), cx)
    }
}

#[cfg(not(test))]
fn directory(cx: &App) -> Option<PathBuf> {
    // Integration tests link the app as a library and use GPUI's test scheduler.
    if cx.is_test() {
        return None;
    }
    Some(dirs::config_dir()?.join("mycela"))
}

#[cfg(test)]
pub(super) struct TestDirectory(pub(super) PathBuf);

#[cfg(test)]
impl gpui_kit::Global for TestDirectory {}

#[cfg(test)]
fn directory(cx: &App) -> Option<PathBuf> {
    cx.try_global::<TestDirectory>()
        .map(|value| value.0.clone())
}

#[cfg(unix)]
pub(super) fn encode_path(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().to_vec()
}

#[cfg(unix)]
pub(super) fn decode_path(bytes: Vec<u8>) -> Option<PathBuf> {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    (!bytes.contains(&0)).then(|| PathBuf::from(OsString::from_vec(bytes)))
}

#[cfg(windows)]
pub(super) fn encode_path(path: &Path) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect()
}

#[cfg(windows)]
pub(super) fn decode_path(bytes: Vec<u8>) -> Option<PathBuf> {
    use std::{ffi::OsString, os::windows::ffi::OsStringExt};
    let pairs = bytes.chunks_exact(2);
    if !pairs.remainder().is_empty() {
        return None;
    }
    let path: Vec<_> = pairs
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    (!path.contains(&0)).then(|| PathBuf::from(OsString::from_wide(&path)))
}

#[cfg(not(any(unix, windows)))]
pub(super) fn encode_path(path: &Path) -> Vec<u8> {
    path.to_string_lossy().into_owned().into_bytes()
}

#[cfg(not(any(unix, windows)))]
pub(super) fn decode_path(bytes: Vec<u8>) -> Option<PathBuf> {
    (!bytes.contains(&0)).then_some(())?;
    String::from_utf8(bytes).ok().map(PathBuf::from)
}

#[cfg(all(test, unix))]
#[test]
fn saved_path_preserves_non_utf8_names_and_line_breaks() {
    let original = b"/media/map\n\xffs".to_vec();
    let path = decode_path(original.clone()).unwrap();
    assert_eq!(encode_path(&path), original);
    assert!(decode_path(b"/media/invalid\0path".to_vec()).is_none());
}
