//! How much memory this process may actually use.
//!
//! Read once, to size an isolate's heap when the embedder asked for the
//! machine's answer rather than a fixed one
//! ([`Limits::heap_limit_bytes`](es_runtime_common::Limits::heap_limit_bytes)
//! set to `None`).
//!
//! The number that matters is the **cgroup** limit, not physical RAM. A server
//! runtime spends most of its life in a container, where the two are different
//! and only one of them gets the process killed: a 2 GiB container on a 64 GiB
//! host still has 64 GiB of physical memory, so sizing from that hands V8 a
//! ceiling it can never reach and turns what should have been a garbage
//! collection into an OOM kill. Node and Deno both read physical memory here,
//! which is precisely why deploying either one means hardcoding
//! `--max-old-space-size` in a Dockerfile.

/// Bytes this process may use: the cgroup memory limit when there is one,
/// otherwise the host's physical memory.
///
/// Zero when neither could be read — the caller treats that as "no answer" and
/// falls back to a fixed ceiling, since a wrong number here is worse than none.
pub(crate) fn available_bytes() -> u64 {
    cgroup_limit().unwrap_or_else(|| physical_memory().unwrap_or(0))
}

/// The container's memory ceiling, from cgroup v2 then v1.
///
/// Both spell "unlimited" as a value near the top of the address space rather
/// than as an absent file — v2 writes the literal `max`, v1 a huge number — so
/// an implausibly large answer is read as no limit at all.
fn cgroup_limit() -> Option<u64> {
    const IMPLAUSIBLE: u64 = 1 << 50; // 1 PiB: no container has this.
    for path in [
        "/sys/fs/cgroup/memory.max",                   // v2
        "/sys/fs/cgroup/memory/memory.limit_in_bytes", // v1
    ] {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let Ok(bytes) = text.trim().parse::<u64>() else {
            continue; // "max"
        };
        if bytes > 0 && bytes < IMPLAUSIBLE {
            return Some(bytes);
        }
    }
    None
}

/// Physical memory, three ways, because there is no portable answer.
///
/// This used to be `/proc/meminfo` and nothing else, which is Linux and only
/// Linux — so on macOS and Windows the documented fallback did not exist and
/// `available_bytes` returned zero, handing V8 no number and letting it size
/// from its own default instead of the machine. That is safe (a wrong number is
/// worse than none) and it is not what this module says it does.
///
/// The two foreign calls are declared here rather than depended on. `libc` has
/// nothing for the Windows one, and `windows-sys` is already in this lockfile at
/// four different versions — pinning a fifth to read a single integer is a worse
/// trade than eleven lines of `extern`. Both are stable ABI and neither is going
/// to change. This is the crate `unsafe` is permitted in (ARCHITECTURE.md §7).
#[cfg(target_os = "linux")]
fn physical_memory() -> Option<u64> {
    // `MemTotal` is kibibytes.
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
    let total = meminfo
        .lines()
        .find_map(|line| line.strip_prefix("MemTotal:"))?;
    let kib: u64 = total.split_whitespace().next()?.parse().ok()?;
    Some(kib * 1024)
}

/// macOS and the rest of Apple's platforms: `hw.memsize`, an `int64_t`.
#[cfg(target_vendor = "apple")]
fn physical_memory() -> Option<u64> {
    use std::ffi::{c_char, c_int, c_void};

    unsafe extern "C" {
        fn sysctlbyname(
            name: *const c_char,
            oldp: *mut c_void,
            oldlenp: *mut usize,
            newp: *mut c_void,
            newlen: usize,
        ) -> c_int;
    }

    let mut bytes: u64 = 0;
    let mut len = size_of::<u64>();
    // SAFETY: `name` is a NUL-terminated literal. `oldp` is a live `u64` and
    // `oldlenp` says how many bytes may be written there, which is the width
    // `hw.memsize` reports; the call writes no more than it is told it may.
    // `newp`/`newlen` are the documented spelling of "reading, not setting".
    // `bytes` is read only when the call reports success.
    let read = unsafe {
        sysctlbyname(
            c"hw.memsize".as_ptr(),
            (&raw mut bytes).cast::<c_void>(),
            &raw mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    (read == 0 && bytes > 0).then_some(bytes)
}

/// Windows: `GlobalMemoryStatusEx`, which fills a struct it is told the size of.
#[cfg(windows)]
fn physical_memory() -> Option<u64> {
    /// `MEMORYSTATUSEX`. Only `total_phys` is read; the rest is there because
    /// the call fills all of it and a short buffer would be written past.
    #[repr(C)]
    struct MemoryStatusEx {
        length: u32,
        memory_load: u32,
        total_phys: u64,
        avail_phys: u64,
        total_page_file: u64,
        avail_page_file: u64,
        total_virtual: u64,
        avail_virtual: u64,
        avail_extended_virtual: u64,
    }

    unsafe extern "system" {
        fn GlobalMemoryStatusEx(buffer: *mut MemoryStatusEx) -> i32;
    }

    let mut status = MemoryStatusEx {
        // The call refuses a struct that does not declare its own size, which
        // is how it tells the caller's vintage from its own.
        length: u32::try_from(size_of::<MemoryStatusEx>()).ok()?,
        memory_load: 0,
        total_phys: 0,
        avail_phys: 0,
        total_page_file: 0,
        avail_page_file: 0,
        total_virtual: 0,
        avail_virtual: 0,
        avail_extended_virtual: 0,
    };
    // SAFETY: the pointer is to a live, fully initialised `MemoryStatusEx`
    // whose `length` field states its own size, which is the contract the call
    // documents. Nothing beyond that struct is written, and `total_phys` is
    // read only when the call reports success.
    let read = unsafe { GlobalMemoryStatusEx(&raw mut status) };
    (read != 0 && status.total_phys > 0).then_some(status.total_phys)
}

/// Anywhere else: no answer rather than a guess, which `available_bytes`
/// already knows how to report.
#[cfg(not(any(target_os = "linux", target_vendor = "apple", windows)))]
fn physical_memory() -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whatever this machine is, the answer has to be a usable number: the
    /// isolate's ceiling is derived from it.
    ///
    /// Asserted on the three platforms this ships to — where a zero is a bug,
    /// as it was on two of them until the platform arms above existed. Anywhere
    /// else there is deliberately no implementation, and the contract is that
    /// the caller is told nothing rather than told wrong.
    #[test]
    fn this_host_reports_something_plausible() {
        let bytes = available_bytes();
        if !cfg!(any(target_os = "linux", target_vendor = "apple", windows)) {
            return;
        }
        assert!(
            bytes >= 64 * 1024 * 1024,
            "implausibly little memory: {bytes}"
        );
    }
}
