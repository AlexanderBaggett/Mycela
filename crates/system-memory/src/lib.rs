//! Narrow current-process/physical-memory queries. No process enumeration,
//! CPU refresh, retained sampling cache, or synthesized failure values.
use std::io;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod macos;
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod windows;

/// Current resident/working-set bytes, not virtual or peak process memory.
pub fn working_set_bytes() -> io::Result<u64> {
    #[cfg(target_os = "linux")]
    {
        linux::working_set_bytes()
    }
    #[cfg(target_os = "macos")]
    {
        macos::working_set_bytes()
    }
    #[cfg(target_os = "windows")]
    {
        windows::working_set_bytes()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Current-process resident memory is unavailable on this platform.",
        ))
    }
}

/// Available physical bytes. Linux uses MemAvailable, Windows ullAvailPhys;
/// macOS uses the documented page-accounting estimate in its backend. Zero is
/// a valid observation. A failed native call or malformed record is an error.
pub fn available_physical_bytes() -> io::Result<u64> {
    #[cfg(target_os = "linux")]
    {
        linux::available_physical_bytes()
    }
    #[cfg(target_os = "macos")]
    {
        macos::available_physical_bytes()
    }
    #[cfg(target_os = "windows")]
    {
        windows::available_physical_bytes()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Available physical memory is unavailable on this platform.",
        ))
    }
}

/// Signed low32 boot-millisecond counter for actual default awm construction.
/// Linux procfs precision and Darwin's clock profile differ from Windows; no
/// universal native clock or suspend/timer-resolution equivalence is claimed.
pub fn uptime_milliseconds() -> io::Result<i32> {
    #[cfg(target_os = "linux")]
    {
        linux::uptime_milliseconds()
    }
    #[cfg(target_os = "windows")]
    {
        windows::uptime_milliseconds()
    }
    #[cfg(target_os = "macos")]
    {
        macos::uptime_milliseconds()
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Boot uptime is unavailable on this platform.",
        ))
    }
}
