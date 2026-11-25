//! Actual portable disk/cache capability; no GPUI/model handle survives here.
use mycela_engine::{
    EncodedByteStore, EncodedStorageEffects, EncodedStorageEnvironment, EncodedStorageNotice,
    ImportedImageError,
};
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};
struct ActualStorageEffects;
impl EncodedStorageEffects for ActualStorageEffects {
    fn tick_count(&self) -> Result<i32, ImportedImageError> {
        mycela_system_memory::uptime_milliseconds().map_err(ImportedImageError::from)
    }
    fn capacity_available(&self, width: i32, quality: i32) -> Result<bool, ImportedImageError> {
        crate::memory_capacity::query_quality(width, quality)
            .map_err(|_| ImportedImageError::CapacityQuery)
    }
    fn available_disk_bytes(&self, directory: &Path) -> Result<u64, ImportedImageError> {
        let directory = directory.canonicalize()?;
        let mut disks = sysinfo::Disks::new_with_refreshed_list();
        let disk = disks
            .list_mut()
            .iter_mut()
            .filter(|d| directory.starts_with(d.mount_point()))
            .max_by_key(|d| d.mount_point().components().count())
            .ok_or(ImportedImageError::Io(std::io::ErrorKind::NotFound))?;
        // refresh's bool distinguishes failed observation from valid zero.
        if !disk.refresh() {
            return Err(ImportedImageError::Io(std::io::ErrorKind::Other));
        }
        Ok(disk.available_space())
    }
    fn report(&self, notice: EncodedStorageNotice) -> Result<(), ImportedImageError> {
        report_to(&mut std::io::stderr().lock(), notice)
    }
}
fn cache_root() -> Result<PathBuf, ImportedImageError> {
    let absolute_env = |name: &str| {
        std::env::var_os(name)
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
    };
    let home = || {
        absolute_env("HOME").ok_or(ImportedImageError::UnsupportedCapability(
            "absolute user home cache directory",
        ))
    };
    let root = if cfg!(target_os = "windows") {
        absolute_env("LOCALAPPDATA").ok_or(ImportedImageError::UnsupportedCapability(
            "LOCALAPPDATA cache directory",
        ))?
    } else if cfg!(target_os = "macos") {
        home()?.join("Library/Caches")
    } else {
        match absolute_env("XDG_CACHE_HOME") {
            Some(p) => p,
            None => home()?.join(".cache"),
        }
    };
    Ok(root.join("Mycela/Encoded Images"))
}
pub(crate) fn install() -> Result<(), ImportedImageError> {
    // Installation does not create directories, scan shared temporary files,
    // observe capacity, or perform any store operation on the foreground.
    static INSTALLED: OnceLock<Result<(), ImportedImageError>> = OnceLock::new();
    *INSTALLED.get_or_init(|| {
        EncodedByteStore::install_environment(EncodedStorageEnvironment::new(
            cache_root()?,
            Arc::new(ActualStorageEffects),
        )?);
        Ok(())
    })
}

// mw.a(Exception) delegates to the best-effort string logger. A diagnostic
// transport error must not prevent the distinct first warning, stored reset or
// process sweep. Warning effects retain their independent fallible boundary.
fn report_to(
    writer: &mut impl Write,
    notice: EncodedStorageNotice,
) -> Result<(), ImportedImageError> {
    let result = writeln!(writer, "Encoded image storage: {notice:?}");
    if matches!(notice, EncodedStorageNotice::WriteDiagnostic(_)) {
        Ok(())
    } else {
        result.map_err(ImportedImageError::from)
    }
}
#[cfg(test)]
mod diagnostic_laws {
    use super::*;
    #[test]
    fn closed_diagnostic_transport_does_not_replace_recovery_but_warnings_still_fail() {
        struct Closed(usize);
        impl Write for Closed {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                self.0 += 1;
                Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe))
            }
        }
        let mut writer = Closed(0);
        assert_eq!(
            report_to(
                &mut writer,
                EncodedStorageNotice::WriteDiagnostic(ImportedImageError::Io(
                    std::io::ErrorKind::PermissionDenied
                ))
            ),
            Ok(())
        );
        assert_eq!(
            writer.0, 1,
            "one attempted diagnostic write, with no retry or replacement failure"
        );
        assert_eq!(
            report_to(
                &mut writer,
                EncodedStorageNotice::WriteFailed(ImportedImageError::Io(
                    std::io::ErrorKind::PermissionDenied
                ))
            ),
            Err(ImportedImageError::Io(std::io::ErrorKind::BrokenPipe))
        );
        assert_eq!(writer.0, 2);
        assert_eq!(
            report_to(
                &mut writer,
                EncodedStorageNotice::InsufficientDiskSpace {
                    available_mebibytes: 12
                }
            ),
            Err(ImportedImageError::Io(std::io::ErrorKind::BrokenPipe))
        );
        assert_eq!(writer.0, 3);
    }
}
