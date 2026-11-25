//! One process-wide source memory policy. Invoke only from the actual lazy
//! Combine capacity or generator/filter preflight in a background render; no GPUI/model reference is
//! retained here, and the mutex is released before any image kernel runs.
use crate::ForegroundError;
use chrono::{Datelike, NaiveDateTime, Timelike};
use mycela_engine::{
    MemoryAddressSpace, MemoryCapacityEffects, MemoryCapacityPolicy, PointwiseError,
};
use std::{
    io::{self, Write},
    mem::size_of,
    sync::{Mutex, OnceLock},
};

static POLICY: OnceLock<Mutex<MemoryCapacityPolicy>> = OnceLock::new();

fn address_space() -> MemoryAddressSpace {
    if cfg!(target_os = "macos") {
        MemoryAddressSpace::MacOs
    } else if cfg!(any(target_os = "windows", target_os = "linux")) {
        // Linux is an explicit portability mapping to the same pointer-width
        // address budget. It is not a claim about the original Windows binary
        // having run on Linux, nor an image-output or license limit.
        MemoryAddressSpace::Windows {
            pointer_bytes: size_of::<usize>() as u8,
        }
    } else {
        MemoryAddressSpace::Other
    }
}
struct ActualMemoryEffects;
impl MemoryCapacityEffects for ActualMemoryEffects {
    type Error = io::Error;
    fn working_set_bytes(&mut self) -> io::Result<i64> {
        i64::try_from(mycela_system_memory::working_set_bytes()?).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "Current-process resident memory exceeds the signed source counter.",
            )
        })
    }
    fn available_physical_bytes(&mut self) -> io::Result<u64> {
        mycela_system_memory::available_physical_bytes()
    }
    fn now_local_ticks(&mut self) -> io::Result<i64> {
        // Each policy request performs its own Now read. Do not combine the
        // expiry comparison and later cache timestamp into one clock sample.
        local_ticks(chrono::Local::now().naive_local())
    }
    fn report_available_physical_error(&mut self, error: io::Error) -> io::Result<()> {
        // mw.a(string) swallows ordinary diagnostic I/O failures. Preserve the
        // source policy's retained previous-available fallback (initially 800 MiB),
        // even if stderr is closed. No App callback or modal is introduced.
        report_available_error(&mut io::stderr().lock(), &error);
        Ok(())
    }
}
fn report_available_error(writer: &mut impl Write, error: &io::Error) {
    let _ = writeln!(writer, "Available physical memory query failed: {error}");
}
fn local_ticks(now: NaiveDateTime) -> io::Result<i64> {
    if !(1..=9999).contains(&now.year()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "The local clock is outside the source DateTime range.",
        ));
    }
    i64::from(now.num_days_from_ce() - 1)
        .checked_mul(864_000_000_000)
        .and_then(|ticks| {
            ticks.checked_add(i64::from(now.num_seconds_from_midnight()) * 10_000_000)
        })
        .and_then(|ticks| ticks.checked_add(i64::from(now.nanosecond() / 100)))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "The local DateTime tick counter overflowed.",
            )
        })
}
pub(crate) fn query(width: i32) -> Result<bool, PointwiseError<ForegroundError>> {
    query_quality(width, 4)
}
pub(crate) fn preflight(width: i32, quality: i32) -> Result<(), PointwiseError<ForegroundError>> {
    crate::encoded_storage::install().map_err(|error| {
        PointwiseError::ImportedImage(mycela_engine::ImportedImageDecodeError::Storage(error))
    })?;
    mycela_engine::EncodedByteStore::preflight(width, quality).map_err(|error| {
        PointwiseError::ImportedImage(mycela_engine::ImportedImageDecodeError::Storage(error))
    })
}
// Compatibility name for the existing procedural source path. Both routes use
// the same real policy, queue dispatch and portable owned-memory sweep.
pub(crate) fn noise_preflight(
    width: i32,
    quality: i32,
) -> Result<(), PointwiseError<ForegroundError>> {
    preflight(width, quality)
}
pub(crate) fn query_quality(
    width: i32,
    quality: i32,
) -> Result<bool, PointwiseError<ForegroundError>> {
    let policy = POLICY.get_or_init(|| Mutex::new(MemoryCapacityPolicy::new(address_space())));
    let mut policy = policy.lock().map_err(|_| {
        PointwiseError::CapacityQuery("The process memory policy lock is poisoned.".into())
    })?;
    policy
        .capacity_available(width, quality, &mut ActualMemoryEffects)
        .map_err(|error| PointwiseError::CapacityQuery(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    #[test]
    fn local_clock_uses_source_epoch_and_one_hundred_nanosecond_ticks() {
        let epoch = NaiveDate::from_ymd_opt(1, 1, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();
        assert_eq!(local_ticks(epoch).unwrap(), 0);
        let unix = NaiveDate::from_ymd_opt(1970, 1, 1)
            .unwrap()
            .and_hms_nano_opt(0, 0, 1, 123_456_789)
            .unwrap();
        assert_eq!(local_ticks(unix).unwrap(), 621_355_968_011_234_567);
        let invalid = NaiveDate::from_ymd_opt(0, 12, 31)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();
        assert_eq!(
            local_ticks(invalid).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }
    #[test]
    fn ordinary_diagnostic_write_failure_is_best_effort() {
        struct Closed(usize);
        impl Write for Closed {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                self.0 += 1;
                Err(io::Error::from(io::ErrorKind::BrokenPipe))
            }
            fn flush(&mut self) -> io::Result<()> {
                Err(io::Error::from(io::ErrorKind::BrokenPipe))
            }
        }
        let mut closed = Closed(0);
        report_available_error(
            &mut closed,
            &io::Error::from(io::ErrorKind::PermissionDenied),
        );
        assert_eq!(
            closed.0, 1,
            "one failed write is ignored without a retry or a replacement policy error"
        );
    }
    #[test]
    fn platform_profile_is_explicit_and_service_state_is_thread_safe() {
        let expected = if cfg!(target_os = "macos") {
            MemoryAddressSpace::MacOs
        } else if cfg!(any(target_os = "windows", target_os = "linux")) {
            MemoryAddressSpace::Windows {
                pointer_bytes: size_of::<usize>() as u8,
            }
        } else {
            MemoryAddressSpace::Other
        };
        assert_eq!(address_space(), expected);
        fn send_sync<T: Send + Sync>() {}
        send_sync::<ActualMemoryEffects>();
        send_sync::<Mutex<MemoryCapacityPolicy>>();
    }
}
