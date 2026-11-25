//! Shared filename and filesystem rules for managed preset libraries.
use super::{Ticket, model};
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::{
    fs,
    path::{Component, Path, PathBuf},
    time::SystemTime,
};

#[derive(Clone, Eq, PartialEq)]
pub(super) struct FileStamp {
    len: u64,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

impl FileStamp {
    pub(super) fn read(metadata: &fs::Metadata) -> Self {
        Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
        }
    }
}

pub(super) fn valid_name(value: &str) -> bool {
    let mut parts = Path::new(value).components();
    matches!(parts.next(), Some(Component::Normal(_)))
        && parts.next().is_none()
        && !value.starts_with('.')
        && !value.ends_with('.')
        && !value
            .chars()
            .any(|character| character.is_control() || "/\\:*?\"<>|".contains(character))
}

pub(super) fn new_path(
    root: &Path,
    factory: Option<&Path>,
    category: &str,
    file_name: &str,
    ticket: &Ticket,
) -> Result<Option<PathBuf>, String> {
    match destination(root, factory, category, file_name, ticket)? {
        Some(Destination::Vacant(path)) => Ok(Some(path)),
        Some(Destination::Existing(_)) => {
            Err("A preset with this name already exists in the category.".into())
        }
        None => Ok(None),
    }
}

pub(super) enum Destination {
    Vacant(PathBuf),
    Existing(PathBuf),
}

pub(super) fn destination(
    root: &Path,
    factory: Option<&Path>,
    category: &str,
    file_name: &str,
    ticket: &Ticket,
) -> Result<Option<Destination>, String> {
    let root = root.canonicalize().map_err(model::failure)?;
    let folder = root.join(category);
    verify_user_path(&folder, factory)?;
    if !ticket.is_current() {
        return Ok(None);
    }
    match fs::create_dir(&folder) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(model::failure(error)),
    }
    if !fs::symlink_metadata(&folder).is_ok_and(|metadata| metadata.file_type().is_dir()) {
        return Err("Choose a category stored directly in the preset folder.".into());
    }
    let mut existing = None;
    for entry in fs::read_dir(&folder).map_err(model::failure)? {
        let entry = entry.map_err(model::failure)?;
        if entry.file_name().eq_ignore_ascii_case(file_name)
            && existing.replace(entry.path()).is_some()
        {
            return Err("More than one preset matches that name in the category.".into());
        }
    }
    Ok(Some(existing.map_or_else(
        || Destination::Vacant(folder.join(file_name)),
        Destination::Existing,
    )))
}

pub(super) fn verify_user_path(path: &Path, factory: Option<&Path>) -> Result<(), String> {
    if factory
        .and_then(|factory| factory.canonicalize().ok())
        .is_some_and(|factory| path.starts_with(factory))
    {
        return Err("Choose a user folder outside the factory library.".into());
    }
    Ok(())
}

pub(super) fn verify_parent(path: &Path) -> Result<(), String> {
    if !path
        .parent()
        .is_some_and(|parent| parent.canonicalize().is_ok_and(|current| current == parent))
    {
        return Err("The category moved. Choose a folder and try again.".into());
    }
    Ok(())
}

pub(super) fn verify_file(path: &Path, stamp: &FileStamp) -> Result<(), String> {
    let unchanged = verify_parent(path).is_ok()
        && fs::symlink_metadata(path)
            .is_ok_and(|metadata| metadata.is_file() && *stamp == FileStamp::read(&metadata));
    if !unchanged {
        return Err("The preset file changed. Refresh the library and choose it again.".into());
    }
    Ok(())
}

pub(super) fn delete(path: &Path, stamp: &FileStamp, ticket: &Ticket) -> Result<(), String> {
    verify_file(path, stamp)?;
    if ticket.is_current() {
        fs::remove_file(path).map_err(|error| format!("Couldn’t delete the preset. {error}"))?;
    }
    Ok(())
}
