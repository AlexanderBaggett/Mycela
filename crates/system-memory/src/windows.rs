//! Native contracts: Microsoft Learn MEMORYSTATUSEX, PROCESS_MEMORY_COUNTERS,
//! GlobalMemoryStatusEx and GetProcessMemoryInfo. The borrowed current-process
//! pseudo handle is never closed. These calls do not enumerate processes.
use std::{
    ffi::c_void,
    io,
    mem::{offset_of, size_of},
};

#[repr(C)]
#[derive(Default)]
struct MemoryStatusEx {
    length: u32,
    load: u32,
    total_phys: u64,
    avail_phys: u64,
    total_page_file: u64,
    avail_page_file: u64,
    total_virtual: u64,
    avail_virtual: u64,
    avail_extended_virtual: u64,
}
#[repr(C)]
#[derive(Default)]
struct ProcessMemoryCounters {
    cb: u32,
    page_fault_count: u32,
    peak_working_set_size: usize,
    working_set_size: usize,
    quota_peak_paged_pool_usage: usize,
    quota_paged_pool_usage: usize,
    quota_peak_non_paged_pool_usage: usize,
    quota_non_paged_pool_usage: usize,
    pagefile_usage: usize,
    peak_pagefile_usage: usize,
}
const _: () = {
    assert!(size_of::<MemoryStatusEx>() == 64);
    assert!(offset_of!(MemoryStatusEx, avail_phys) == 16);
    assert!(size_of::<ProcessMemoryCounters>() == 8 + 8 * size_of::<usize>());
    assert!(offset_of!(ProcessMemoryCounters, working_set_size) == 8 + size_of::<usize>());
};
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetTickCount() -> u32;
    fn GetCurrentProcess() -> *mut c_void;
    fn GlobalMemoryStatusEx(buffer: *mut MemoryStatusEx) -> i32;
}
#[link(name = "psapi")]
unsafe extern "system" {
    fn GetProcessMemoryInfo(
        process: *mut c_void,
        counters: *mut ProcessMemoryCounters,
        cb: u32,
    ) -> i32;
}
pub(super) fn working_set_bytes() -> io::Result<u64> {
    let mut counters = ProcessMemoryCounters {
        cb: size_of::<ProcessMemoryCounters>() as u32,
        ..Default::default()
    };
    let cb = counters.cb;
    // SAFETY: The ABI/layout is fixed by the documented C structs and asserted
    // above. The borrowed pseudo handle refers only to this process. The native
    // function receives a live writable buffer of exactly cb bytes; it retains
    // neither the pointer nor handle. Read the output only on nonzero success.
    let succeeded = unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, cb) };
    if succeeded == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(counters.working_set_size as u64)
}
pub(super) fn available_physical_bytes() -> io::Result<u64> {
    let mut status = MemoryStatusEx {
        length: size_of::<MemoryStatusEx>() as u32,
        ..Default::default()
    };
    // SAFETY: status is an owned writable MEMORYSTATUSEX with the required
    // initialized length and asserted layout. The OS retains no pointer. A
    // failed call is reported before any output is consumed.
    if unsafe { GlobalMemoryStatusEx(&mut status) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(status.avail_phys)
}

pub(super) fn uptime_milliseconds() -> io::Result<i32> {
    // SAFETY: GetTickCount has no parameters, retains no pointers, cannot fail,
    // and returns the documented wrapping DWORD boot-millisecond counter.
    Ok(unsafe { GetTickCount() } as i32)
}
