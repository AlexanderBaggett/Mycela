//! Environment preset discovery, thumbnail decoding and user preset naming.
use super::super::{Ticket, environment::read_map, model};
use gpui_kit::RenderImage;
use mycela_engine::{BinaryCursor, EnvironmentPreset, EnvironmentRecord};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
};

const MAX_THUMBNAIL_PREFIX: u64 = 1024 * 1024;

fn preview(path: &Path) -> Result<Arc<RenderImage>, String> {
    let mut prefix = Vec::new();
    File::open(path)
        .and_then(|file| file.take(MAX_THUMBNAIL_PREFIX).read_to_end(&mut prefix))
        .map_err(model::failure)?;
    let mut reader = BinaryCursor::new(&prefix).map_err(model::failure)?;
    let (_, thumbnail) = EnvironmentPreset::read_preview(&mut reader).map_err(model::failure)?;
    model::decode_preset_thumbnail(thumbnail, None)
}

pub(super) struct Preset {
    pub category: String,
    pub name: String,
    pub thumbnail: Arc<RenderImage>,
    pub path: PathBuf,
}

impl Preset {
    pub fn remove_from_disk(&self) -> Result<(), String> {
        let path = &self.path;
        if path
            .parent()
            .and_then(|parent| parent.symlink_metadata().ok())
            .is_some_and(|metadata| metadata.file_type().is_symlink())
        {
            return Err(
                "The preset category moved. Refresh the catalog and choose the preset again."
                    .into(),
            );
        }
        fs::remove_file(path).map_err(|error| format!("Couldn’t delete the preset. {error}"))
    }

    pub fn load(&self) -> Result<EnvironmentRecord, String> {
        let bytes =
            fs::read(&self.path).map_err(|error| format!("Couldn’t read the preset. {error}"))?;
        read_map(&bytes)
    }
}

#[derive(Default)]
pub(super) struct Catalog {
    pub entries: Vec<Arc<Preset>>,
    pub skipped: usize,
    pub warning: Option<String>,
}

pub(super) fn scan(root: Option<&Path>, ticket: &Ticket) -> Catalog {
    let mut sources = BTreeMap::new();
    let mut catalog = Catalog::default();
    if let Some(folder) = root
        && ticket.is_current()
        && let Err(error) = scan_user(folder, &mut sources, ticket)
    {
        catalog.warning = Some(format!(
            "Couldn’t read the preset folder {}. {error}",
            folder.display()
        ));
    }
    for ((category, name), path) in sources {
        if !ticket.is_current() {
            break;
        }
        match preview(&path) {
            Ok(thumbnail) => catalog.entries.push(Arc::new(Preset {
                category,
                name,
                path,
                thumbnail,
            })),
            Err(_) => catalog.skipped += 1,
        }
    }
    catalog
}

fn scan_user(
    path: &Path,
    sources: &mut BTreeMap<(String, String), PathBuf>,
    ticket: &Ticket,
) -> std::io::Result<()> {
    let folders = match fs::read_dir(path) {
        Ok(folders) => folders,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for folder in folders {
        if !ticket.is_current() {
            break;
        }
        let folder = folder?;
        let Some(category) = folder
            .file_name()
            .into_string()
            .ok()
            .filter(|name| name != ".svn")
        else {
            continue;
        };
        if !folder.file_type()?.is_dir() {
            continue;
        }
        for file in category_presets(&folder.path())? {
            if !ticket.is_current() {
                break;
            }
            let (name, path) = file?;
            sources.entry((category.clone(), name)).or_insert(path);
        }
    }
    Ok(())
}

fn category_presets(
    folder: &Path,
) -> std::io::Result<impl Iterator<Item = std::io::Result<(String, PathBuf)>>> {
    let entries = match fs::read_dir(folder) {
        Ok(entries) => Some(entries),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    Ok(entries.into_iter().flatten().filter_map(|entry| {
        let preset = || {
            let entry = entry?;
            let path = entry.path();
            if !entry.file_type()?.is_file()
                || path
                    .extension()
                    .is_none_or(|ext| !ext.eq_ignore_ascii_case("emap"))
            {
                return Ok(None);
            }
            let Some(name) = path.file_stem().and_then(|name| name.to_str()) else {
                return Ok(None);
            };
            Ok(Some((name.to_owned(), path)))
        };
        preset().transpose()
    }))
}

pub(super) fn check_user_name(root: &Path, category: &str, name: &str) -> Result<(), String> {
    let category_folder = root.join(category);
    for preset in category_presets(&category_folder).map_err(model::failure)? {
        if preset.map_err(model::failure)?.0 == name {
            return Err("A preset with this name already exists in the category.".into());
        }
    }
    Ok(())
}

pub(super) fn new_preset_path(root: &Path, category: &str, name: &str) -> Result<PathBuf, String> {
    let portable = |value: &str| {
        let mut parts = Path::new(value).components();
        matches!(parts.next(), Some(std::path::Component::Normal(_)))
            && parts.next().is_none()
            && !value.ends_with('.')
            && !value
                .chars()
                .any(|character| character.is_control() || "/\\:*?\"<>|".contains(character))
    };
    if !portable(category) || !portable(name) {
        return Err("Enter a category and preset name using ordinary file name characters.".into());
    }
    Ok(root.join(category).join(format!("{name}.emap")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_presets_without_a_folder_are_empty() {
        let catalog = scan(None, &Ticket::new());
        assert!(catalog.entries.is_empty());
        assert_eq!(catalog.skipped, 0);
        assert!(catalog.warning.is_none());
    }
}
