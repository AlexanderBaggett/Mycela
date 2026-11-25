//! Kernel procfs units and meanings:
//! https://www.kernel.org/doc/html/latest/filesystems/proc.html
use std::{fs, io};

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}
fn kibibyte_field(text: &str, key: &str) -> io::Result<u64> {
    let mut found = None;
    for line in text.lines() {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name != key {
            continue;
        }
        if found.is_some() {
            return Err(invalid(format!("Duplicate {key} in procfs memory record.")));
        }
        let mut parts = value.split_ascii_whitespace();
        let number = parts
            .next()
            .ok_or_else(|| invalid(format!("Missing {key} value.")))?;
        if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
            return Err(invalid(format!("Invalid {key} value.")));
        }
        if parts.next() != Some("kB") || parts.next().is_some() {
            return Err(invalid(format!("Unexpected {key} unit/layout.")));
        }
        let bytes = number
            .parse::<u64>()
            .ok()
            .and_then(|v| v.checked_mul(1024))
            .ok_or_else(|| invalid(format!("{key} does not fit the byte counter.")))?;
        found = Some(bytes);
    }
    found.ok_or_else(|| {
        invalid(format!(
            "{key} is unavailable in this procfs memory record."
        ))
    })
}
pub(super) fn working_set_bytes() -> io::Result<u64> {
    kibibyte_field(&fs::read_to_string("/proc/self/status")?, "VmRSS")
}
pub(super) fn available_physical_bytes() -> io::Result<u64> {
    // Missing MemAvailable (for example, older kernels) is explicit failure;
    // the engine owns the source's fallback policy, not this OS adapter.
    kibibyte_field(&fs::read_to_string("/proc/meminfo")?, "MemAvailable")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_field_units_zero_and_unrelated_fields() {
        let record = "Name:\tmycela\nVmSize: 99999 kB\nVmRSS:\t1234 kB\n";
        assert_eq!(kibibyte_field(record, "VmRSS").unwrap(), 1234 * 1024);
        assert_eq!(
            kibibyte_field("MemAvailable: 0 kB\n", "MemAvailable").unwrap(),
            0
        );
        assert_eq!(
            kibibyte_field("MemFree: 22 kB\nMemAvailable: 42 kB\n", "MemAvailable").unwrap(),
            42 * 1024
        );
    }
    #[test]
    fn missing_duplicate_malformed_units_and_overflow_are_errors() {
        for record in [
            "MemFree: 12 kB",
            "MemAvailable:",
            "MemAvailable: -1 kB",
            "MemAvailable: +1 kB",
            "MemAvailable: 1 MB",
            "MemAvailable: 1 kB junk",
            "MemAvailable: 18446744073709551615 kB",
            "MemAvailable: 1 kB\nMemAvailable: 2 kB",
        ] {
            assert_eq!(
                kibibyte_field(record, "MemAvailable").unwrap_err().kind(),
                io::ErrorKind::InvalidData,
                "{record}"
            );
        }
    }
}

/// procfs uptime is boot uptime in seconds, normally printed to centiseconds.
/// Truncate finer fractions to milliseconds; never substitute wall/process time.
pub(super) fn uptime_milliseconds() -> io::Result<i32> {
    uptime_record_milliseconds(&fs::read_to_string("/proc/uptime")?)
}
fn uptime_record_milliseconds(text: &str) -> io::Result<i32> {
    let token = text
        .split_ascii_whitespace()
        .next()
        .ok_or_else(|| invalid("Missing boot uptime."))?;
    let (whole, fraction) = token.split_once('.').unwrap_or((token, ""));
    if whole.is_empty()
        || (token.contains('.') && fraction.is_empty())
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || !fraction.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(invalid("Malformed boot uptime."));
    }
    let whole = whole
        .parse::<u64>()
        .map_err(|_| invalid("Boot uptime exceeds UInt64 seconds."))?;
    let mut millis = 0u64;
    for i in 0..3 {
        millis = millis * 10
            + fraction
                .as_bytes()
                .get(i)
                .map_or(0, |b| u64::from(*b - b'0'));
    }
    // The source publishes signed low32 bits of boot milliseconds. Reduce the
    // seconds before multiplication so a large valid uptime does not overflow.
    Ok(((whole as u32)
        .wrapping_mul(1000)
        .wrapping_add(millis as u32)) as i32)
}
#[cfg(test)]
mod uptime_laws {
    use super::*;
    #[test]
    fn uptime_decimal_precision_and_signed_wrap_are_input_derived() {
        assert_eq!(uptime_record_milliseconds("0.00 1.00").unwrap(), 0);
        assert_eq!(uptime_record_milliseconds("1.2 0").unwrap(), 1200);
        assert_eq!(
            uptime_record_milliseconds("2147483.648 0").unwrap(),
            i32::MIN
        );
        assert_eq!(uptime_record_milliseconds("4294967.296 0").unwrap(), 0);
        assert_eq!(uptime_record_milliseconds("1.234999 0").unwrap(), 1234);
        for text in ["", "-1.0 0", "1.x 0", ".1 0", "1.2.3 0"] {
            assert!(uptime_record_milliseconds(text).is_err());
        }
    }
}
