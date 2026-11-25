//! libc 0.2.189 supplies proc_taskinfo/vm_statistics64 and the public Darwin
//! constants. Mach declarations absent from that crate use the published MIG
//! mach_host.defs/mach_port.defs signatures, with libc's own scalar ABI types.
use std::{
    io,
    mem::{offset_of, size_of},
};

const _: () = {
    assert!(size_of::<libc::proc_taskinfo>() == 96);
    assert!(offset_of!(libc::proc_taskinfo, pti_resident_size) == 8);
    assert!(size_of::<libc::vm_statistics64>() % size_of::<libc::integer_t>() == 0);
};

unsafe extern "C" {
    fn host_page_size(host: libc::host_t, size: *mut libc::vm_size_t) -> libc::kern_return_t;
    fn mach_port_deallocate(
        task: libc::mach_port_t,
        name: libc::mach_port_t,
    ) -> libc::kern_return_t;
}
fn mach_error(operation: &str, code: libc::kern_return_t) -> io::Error {
    io::Error::other(format!("{operation} failed with Mach status {code}."))
}
struct HostPort(libc::mach_port_t);
impl HostPort {
    fn new() -> io::Result<Self> {
        // SAFETY: mach_host_self has no arguments and returns this task's send
        // right. This owner releases exactly that acquired right, never a task
        // or foreign process port.
        let port = unsafe { libc::mach_host_self() };
        if port == libc::MACH_PORT_NULL as libc::mach_port_t {
            return Err(io::Error::other(
                "mach_host_self returned a null host port.",
            ));
        }
        Ok(Self(port))
    }
    fn release(&mut self) -> io::Result<()> {
        let port = self.0;
        if port == libc::MACH_PORT_NULL as libc::mach_port_t {
            return Ok(());
        }
        // SAFETY: mach_task_self is a borrowed current-task port from libc's
        // platform binding. port is the one owned send right from mach_host_self,
        // cleared from this owner only after successful release. On failure it
        // remains owned so Drop can make one bounded cleanup attempt.
        #[allow(deprecated)]
        let code = unsafe { mach_port_deallocate(libc::mach_task_self(), port) };
        if code == libc::KERN_SUCCESS {
            self.0 = libc::MACH_PORT_NULL as libc::mach_port_t;
            Ok(())
        } else {
            Err(mach_error("mach_port_deallocate", code))
        }
    }
}
impl Drop for HostPort {
    fn drop(&mut self) {
        let _ = self.release();
    } // Unwind safety; normal errors are returned explicitly.
}
pub(super) fn working_set_bytes() -> io::Result<u64> {
    // SAFETY: proc_taskinfo is libc's POD integer-only native output struct;
    // zero is valid for every field, including output left untouched on failure.
    let mut info: libc::proc_taskinfo = unsafe { std::mem::zeroed() };
    let size = i32::try_from(size_of::<libc::proc_taskinfo>())
        .map_err(|_| io::Error::other("proc_taskinfo layout is too large."))?;
    // SAFETY: getpid targets this process only. proc_pidinfo receives its exact
    // libc struct/size, writes within the owned buffer, and retains no pointer.
    let received = unsafe {
        libc::proc_pidinfo(
            libc::getpid(),
            libc::PROC_PIDTASKINFO,
            0,
            (&mut info as *mut libc::proc_taskinfo).cast(),
            size,
        )
    };
    if received <= 0 {
        return Err(io::Error::other(format!(
            "proc_pidinfo returned {received}: {}",
            io::Error::last_os_error()
        )));
    }
    if received != size {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "proc_pidinfo returned a partial task record.",
        ));
    }
    Ok(info.pti_resident_size)
}
pub(super) fn available_physical_bytes() -> io::Result<u64> {
    let mut host = HostPort::new()?;
    let port = host.0;
    let result = (|| {
        let mut page_size: libc::vm_size_t = 0;
        // SAFETY: port is a live owned current-host send right; page_size is a
        // writable output of the documented vm_size_t type. No pointer retained.
        let code = unsafe { host_page_size(port, &mut page_size) };
        if code != libc::KERN_SUCCESS {
            return Err(mach_error("host_page_size", code));
        }
        if page_size == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "host_page_size returned zero.",
            ));
        }
        // SAFETY: libc's vm_statistics64 is a POD integer-only output record;
        // all-zero initialization is valid even when older kernels fill a prefix.
        let mut stats: libc::vm_statistics64 = unsafe { std::mem::zeroed() };
        let mut count = libc::HOST_VM_INFO64_COUNT;
        // SAFETY: The flavor and count come from the same cached libc definition
        // as the owned output buffer. The pointer is aligned for integer_t and
        // the count bounds the full allocation. Only a returned valid prefix is read.
        let code = unsafe {
            libc::host_statistics64(
                port,
                libc::HOST_VM_INFO64,
                (&mut stats as *mut libc::vm_statistics64).cast(),
                &mut count,
            )
        };
        if code != libc::KERN_SUCCESS {
            return Err(mach_error("host_statistics64", code));
        }
        let minimum = (offset_of!(libc::vm_statistics64, compressor_page_count)
            + size_of::<libc::natural_t>())
            / size_of::<libc::integer_t>();
        if usize::try_from(count).ok().is_none_or(|n| {
            n < minimum || n > size_of::<libc::vm_statistics64>() / size_of::<libc::integer_t>()
        }) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "host_statistics64 returned an incomplete/oversized memory record.",
            ));
        }
        // Portable macOS estimate, matching the reviewed page-accounting seam:
        // free + inactive + purgeable minus compressor pages, floored at zero.
        // This is not a claim of a native Windows ullAvailPhys equivalent.
        let pages = u64::from(stats.free_count)
            .checked_add(u64::from(stats.inactive_count))
            .and_then(|n| n.checked_add(u64::from(stats.purgeable_count)))
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "Physical page count overflow.")
            })?
            .saturating_sub(u64::from(stats.compressor_page_count));
        pages.checked_mul(page_size as u64).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "Physical byte count overflow.")
        })
    })();
    let released = host.release();
    match (result, released) {
        (Ok(bytes), Ok(())) => Ok(bytes),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(cleanup)) => Err(io::Error::other(format!(
            "{error} Host-port cleanup also failed: {cleanup}"
        ))),
    }
}

pub(super) fn uptime_milliseconds() -> io::Result<i32> {
    let mut value = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: libc declares Darwin's clock_gettime ABI and timespec layout.
    // CLOCK_MONOTONIC is an actual monotonic system uptime clock, not wall or
    // process time. The owned initialized buffer remains live and is not retained.
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut value) } != 0 {
        return Err(io::Error::last_os_error());
    }
    if value.tv_sec < 0 || !(0..1_000_000_000).contains(&value.tv_nsec) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid native uptime timespec.",
        ));
    }
    let seconds = value.tv_sec as u32;
    let millis = (value.tv_nsec / 1_000_000) as u32;
    Ok(seconds.wrapping_mul(1000).wrapping_add(millis) as i32)
}
