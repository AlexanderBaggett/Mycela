use crate::{PointwiseError, VariableFileService};
use std::marker::PhantomData;

// Only first-party templates may be bundled. Preserve the missing-template
// diagnostic for an import that references a template outside its document.
const ASSETS: &[(&str, &[u8])] = &[];

pub(super) fn read(path: &str) -> Option<(&'static str, &'static [u8])> {
    let path = path.replace('\\', "/");
    ASSETS
        .iter()
        .copied()
        .find(|(name, _)| name.eq_ignore_ascii_case(&path))
}

pub(super) struct LabFiles<E>(PhantomData<fn() -> E>);

impl<E> Default for LabFiles<E> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

impl<E> VariableFileService for LabFiles<E> {
    type Error = PointwiseError<E>;

    fn files(&mut self, directory: &[u16]) -> Result<Vec<Vec<u16>>, Self::Error> {
        let directory = String::from_utf16(directory)
            .map_err(|_| PointwiseError::Argument)?
            .replace('\\', "/");
        let directory = directory.trim_end_matches('/');
        let files: Vec<_> = ASSETS
            .iter()
            .filter_map(|(path, _)| {
                let (parent, _) = path.rsplit_once('/')?;
                parent
                    .eq_ignore_ascii_case(directory)
                    .then(|| path.encode_utf16().collect())
            })
            .collect();
        if files.is_empty() {
            return Err(PointwiseError::UnsupportedCapability(
                "Wizard Reference template directory",
            ));
        }
        Ok(files)
    }
}
