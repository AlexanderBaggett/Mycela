//! File operations for the selected Synthesis library.
use super::super::super::preset_files;
use super::super::export::PresetExport;
use super::{Choice, Ticket, model};
pub(super) use preset_files::{FileStamp, valid_name};
use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

pub(super) fn save_new(
    export: PresetExport,
    root: &Path,
    factory: Option<&Path>,
    category: &str,
    name: &str,
    ticket: &Ticket,
) -> Result<Option<PathBuf>, String> {
    let Some(path) =
        preset_files::new_path(root, factory, category, &format!("{name}.tsyn"), ticket)?
    else {
        return Ok(None);
    };
    let Some(binary) = export.encode(ticket)? else {
        return Ok(None);
    };
    preset_files::verify_parent(&path)?;
    model::save_new_bytes(binary.bytes(), path, ticket)
}

pub(super) fn delete(preset: &Choice, ticket: &Ticket) -> Result<(), String> {
    if preset.factory {
        return Err("Factory presets are read-only.".into());
    }
    let stamp = preset
        .stamp
        .as_ref()
        .ok_or("Refresh the library and choose the preset again.")?;
    preset_files::delete(&preset.path, stamp, ticket)
}

#[derive(Clone)]
pub(super) struct Category {
    path: PathBuf,
    stamp: FileStamp,
}

impl Category {
    pub(super) fn contains(&self, preset: &Choice) -> bool {
        preset.path.parent() == Some(&self.path)
    }

    fn verify(&self, library: &Library) -> Result<(), String> {
        library.verify_category_path(&self.path)?;
        if !fs::symlink_metadata(&self.path)
            .is_ok_and(|metadata| metadata.is_dir() && FileStamp::read(&metadata) == self.stamp)
        {
            return Err("The category changed. Refresh the library and choose it again.".into());
        }
        Ok(())
    }
}

pub(super) type Categories = BTreeMap<String, Category>;

pub(super) fn categories(
    root: &Path,
    factory: Option<&Path>,
    ticket: &Ticket,
) -> Result<Categories, String> {
    let root = root.canonicalize().map_err(model::failure)?;
    let library = Library { root, factory };
    let mut categories = BTreeMap::new();
    for entry in fs::read_dir(&library.root).map_err(model::failure)? {
        if !ticket.is_current() {
            break;
        }
        let entry = entry.map_err(model::failure)?;
        let Some(name) = entry
            .file_name()
            .into_string()
            .ok()
            .filter(|name| valid_name(name))
        else {
            continue;
        };
        let path = entry.path();
        if library.verify_category_path(&path).is_err() {
            continue;
        }
        let metadata = entry.metadata().map_err(model::failure)?;
        if metadata.is_dir() {
            categories.insert(
                name,
                Category {
                    path,
                    stamp: FileStamp::read(&metadata),
                },
            );
        }
    }
    Ok(categories)
}

pub(super) const ORGANIZE_SUPPORTED: bool = cfg!(any(target_os = "linux", target_os = "macos"));

pub(super) enum Change {
    RenamePreset(Choice, String),
    MovePreset(Choice, Category),
    RenameCategory(Category, String),
    DeleteCategory(Category),
}

pub(super) enum Outcome {
    Relocated { from: PathBuf, to: PathBuf },
    DeletedCategory,
}

impl Outcome {
    pub(super) fn update_selection(&self, selected: Option<&Path>) -> Option<PathBuf> {
        let selected = selected?;
        let Self::Relocated { from, to } = self else {
            return Some(selected.to_path_buf());
        };
        if selected == from {
            Some(to.clone())
        } else if let Ok(tail) = selected.strip_prefix(from) {
            Some(to.join(tail))
        } else {
            Some(selected.to_path_buf())
        }
    }
}

struct Library<'a> {
    root: PathBuf,
    factory: Option<&'a Path>,
}

impl Library<'_> {
    fn verify_category_path(&self, path: &Path) -> Result<(), String> {
        if path.parent() != Some(&self.root) {
            return Err("Choose a category in the user preset folder.".into());
        }
        if self
            .factory
            .is_some_and(|factory| path.starts_with(factory) || factory.starts_with(path))
        {
            return Err("Factory categories are read-only.".into());
        }
        Ok(())
    }

    fn verify_preset<'a>(&self, preset: &'a Choice) -> Result<&'a Path, String> {
        let path = preset.path.as_path();
        if preset.factory
            || self
                .factory
                .is_some_and(|factory| path.starts_with(factory))
        {
            return Err("Factory presets are read-only.".into());
        }
        let parent = path.parent().ok_or("Choose a user preset.")?;
        if parent != self.root && parent.parent() != Some(&self.root) {
            return Err("Choose a preset in the user folder.".into());
        }
        let stamp = preset
            .stamp
            .as_ref()
            .ok_or("Refresh the library and choose the preset again.")?;
        preset_files::verify_file(path, stamp)?;
        Ok(parent)
    }

    fn verify_factory_collision(&self, destination: &Path, category: bool) -> Result<(), String> {
        let Some(factory) = self.factory else {
            return Ok(());
        };
        let relative = destination
            .strip_prefix(&self.root)
            .map_err(model::failure)?;
        let message = if category {
            "A factory category with this name already exists."
        } else {
            "A factory preset with this name already exists in the category."
        };
        check_collision(&factory.join(relative), None, message)
    }
}

pub(super) fn organize(
    change: Change,
    root: &Path,
    factory: Option<&Path>,
    ticket: &Ticket,
) -> Result<Option<Outcome>, String> {
    if !ticket.is_current() {
        return Ok(None);
    }
    let root = root.canonicalize().map_err(model::failure)?;
    let factory = factory.and_then(|folder| folder.canonicalize().ok());
    if factory
        .as_ref()
        .is_some_and(|factory| root.starts_with(factory))
    {
        return Err("Choose a user folder outside the factory library.".into());
    }
    let library = Library {
        root,
        factory: factory.as_deref(),
    };
    let (from, to, category) = match change {
        Change::RenamePreset(preset, name) => {
            validate_name(&name)?;
            let parent = library.verify_preset(&preset)?;
            let to = parent.join(format!("{name}.tsyn"));
            (preset.path.as_ref().clone(), to, false)
        }
        Change::MovePreset(preset, category) => {
            library.verify_preset(&preset)?;
            category.verify(&library)?;
            let file_name = preset.path.file_name().ok_or("Choose a user preset.")?;
            (
                preset.path.as_ref().clone(),
                category.path.join(file_name),
                false,
            )
        }
        Change::RenameCategory(category, name) => {
            validate_name(&name)?;
            category.verify(&library)?;
            let to = library.root.join(name);
            library.verify_category_path(&to)?;
            (category.path, to, true)
        }
        Change::DeleteCategory(category) => {
            category.verify(&library)?;
            preset_files::verify_parent(&category.path)?;
            if !ticket.is_current() {
                return Ok(None);
            }
            // Directory symlinks inside the category are unlinked, never traversed.
            fs::remove_dir_all(&category.path)
                .map_err(|error| format!("Couldn’t delete the category. {error}"))?;
            return Ok(Some(Outcome::DeletedCategory));
        }
    };
    if from == to {
        return Ok(Some(Outcome::Relocated { from, to }));
    }
    let collision = if category {
        "A category with this name already exists."
    } else {
        "A preset with this name already exists in the category."
    };
    library.verify_factory_collision(&to, category)?;
    check_collision(&to, Some(&from), collision)?;
    preset_files::verify_parent(&to)?;
    if !ticket.is_current() {
        return Ok(None);
    }
    rename_noreplace(&from, &to).map_err(|error| {
        if error.kind() == ErrorKind::AlreadyExists {
            collision.into()
        } else {
            format!("Couldn’t update the user preset folder. {error}")
        }
    })?;
    Ok(Some(Outcome::Relocated { from, to }))
}

fn validate_name(name: &str) -> Result<(), String> {
    if valid_name(name) {
        Ok(())
    } else {
        Err("Enter a name using ordinary file name characters.".into())
    }
}

fn filename(path: &Path) -> &OsStr {
    path.file_name().unwrap_or_default()
}

fn check_collision(path: &Path, except: Option<&Path>, message: &str) -> Result<(), String> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    let entries = match fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(model::failure(error)),
    };
    for entry in entries {
        let entry = entry.map_err(model::failure)?;
        if entry.file_name().eq_ignore_ascii_case(filename(path))
            && except != Some(entry.path().as_path())
        {
            return Err(message.into());
        }
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn rename_noreplace(from: &Path, to: &Path) -> std::io::Result<()> {
    use rustix::fs::{Mode, OFlags, RenameFlags, open, renameat_with};
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let missing_parent = || std::io::Error::new(ErrorKind::InvalidInput, "Missing preset folder");
    let from_parent = open(
        from.parent().ok_or_else(missing_parent)?,
        flags,
        Mode::empty(),
    )?;
    let to_parent = open(
        to.parent().ok_or_else(missing_parent)?,
        flags,
        Mode::empty(),
    )?;
    renameat_with(
        from_parent,
        filename(from),
        to_parent,
        filename(to),
        RenameFlags::NOREPLACE,
    )?;
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn rename_noreplace(_: &Path, _: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        ErrorKind::Unsupported,
        "Organizing presets is unavailable on this platform.",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn synthesis_library_preserves_a_file_changed_since_selection() {
        let dir =
            std::env::temp_dir().join(format!("mycela-synthesis-stale-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.canonicalize().unwrap().join("User preset.tsyn");
        fs::write(&path, b"original file").unwrap();
        let preset = Choice {
            path: path.clone().into(),
            category: "Test".into(),
            name: "User preset".into(),
            thumbnail: None,
            factory: false,
            stamp: Some(FileStamp::read(&fs::metadata(&path).unwrap())),
        };
        fs::write(&path, b"replacement with different contents").unwrap();
        let error = delete(&preset, &Ticket::new()).unwrap_err();
        assert!(error.contains("file changed"));
        assert_eq!(
            fs::read(&path).unwrap(),
            b"replacement with different contents"
        );
        fs::remove_dir_all(dir).unwrap();
    }
}
