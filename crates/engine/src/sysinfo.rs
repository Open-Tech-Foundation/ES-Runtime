//! What this process and this thread have spent (DECISIONS.md D91).
//!
//! Sits beside [`sysmem`](crate::sysmem) and for the same reason: reading the
//! host's own accounting needs platform calls, and this is the crate `unsafe` is
//! permitted in (ARCHITECTURE.md §7). Declaring the handful of externs here beats
//! pinning a dependency to read three integers, which is the trade `sysmem`
//! already made.
//!
//! # Thread, not process
//!
//! [`thread_cpu_ms`] reports the **calling** thread. That is the whole point: a
//! worker is its own OS thread, so an agent asking on its own thread is what
//! makes "which worker is burning the CPU?" answerable at all.
//!
//! # Total, not user versus system
//!
//! Every reading is total CPU. The user/system split is available per *thread*
//! on Linux and Windows and needs Mach on macOS, where getting the struct layout
//! wrong is a memory-safety bug rather than a wrong number — so the split is not
//! reported anywhere rather than on two platforms out of three. `CLOCK_*_CPUTIME_ID`
//! is POSIX, is the same call on both Unixes, and cannot be got wrong.
//!
//! Every reading is best-effort: a failed call answers `0.0`, because a process
//! that cannot introspect itself should still run.

/// CPU milliseconds consumed by the **calling thread**.
pub fn thread_cpu_ms() -> f64 {
    imp::thread_cpu_ms()
}

/// CPU milliseconds consumed by the **whole process**, every thread together.
pub fn process_cpu_ms() -> f64 {
    imp::process_cpu_ms()
}

/// The process's current resident set in bytes, or `0` where it could not be
/// read.
///
/// Resident *now*, not peak: what is in memory is the number a deployment acts
/// on, and the peak is already in whatever supervises the process.
pub fn resident_bytes() -> u64 {
    imp::resident_bytes()
}

// ---------------------------------------------------------------------------
// Unix — both platforms share POSIX's per-thread and per-process CPU clocks
// ---------------------------------------------------------------------------

#[cfg(unix)]
mod imp {
    use std::ffi::c_int;

    #[repr(C)]
    struct Timespec {
        seconds: i64,
        nanoseconds: i64,
    }

    unsafe extern "C" {
        fn clock_gettime(id: c_int, out: *mut Timespec) -> c_int;
    }

    /// `CLOCK_THREAD_CPUTIME_ID` and `CLOCK_PROCESS_CPUTIME_ID`. The numbers
    /// differ between the two Unixes and are part of each one's stable ABI.
    #[cfg(target_os = "linux")]
    const THREAD_CPU: c_int = 3;
    #[cfg(target_os = "linux")]
    const PROCESS_CPU: c_int = 2;
    #[cfg(target_os = "macos")]
    const THREAD_CPU: c_int = 16;
    #[cfg(target_os = "macos")]
    const PROCESS_CPU: c_int = 12;
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    const THREAD_CPU: c_int = 3;
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    const PROCESS_CPU: c_int = 2;

    fn clock_ms(id: c_int) -> f64 {
        let mut out = Timespec {
            seconds: 0,
            nanoseconds: 0,
        };
        // SAFETY: `clock_gettime` writes exactly the `timespec` it is handed and
        // touches nothing else. The struct is initialised first, so a failed call
        // reads back as zero rather than as anything undefined.
        let read = unsafe { clock_gettime(id, &mut out) };
        if read != 0 {
            return 0.0;
        }
        out.seconds as f64 * 1_000.0 + out.nanoseconds as f64 / 1_000_000.0
    }

    pub(super) fn thread_cpu_ms() -> f64 {
        clock_ms(THREAD_CPU)
    }

    pub(super) fn process_cpu_ms() -> f64 {
        clock_ms(PROCESS_CPU)
    }

    /// `VmRSS` from `/proc/self/status` — a text read, so no call and no struct
    /// layout to get wrong.
    ///
    /// Not `statm`, which counts pages and would need the page size; not
    /// `ru_maxrss`, which is the *peak* and so can never show memory released.
    #[cfg(target_os = "linux")]
    pub(super) fn resident_bytes() -> u64 {
        let Ok(status) = std::fs::read_to_string("/proc/self/status") else {
            return 0;
        };
        for line in status.lines() {
            let Some(rest) = line.strip_prefix("VmRSS:") else {
                continue;
            };
            // "VmRSS:    12345 kB"
            let Some(kib) = rest.split_whitespace().next() else {
                return 0;
            };
            return kib.parse::<u64>().unwrap_or(0).saturating_mul(1024);
        }
        0
    }

    /// `task_info(MACH_TASK_BASIC_INFO)`, the only place macOS keeps a live
    /// resident size.
    #[cfg(target_os = "macos")]
    pub(super) fn resident_bytes() -> u64 {
        use std::ffi::c_uint;

        /// `mach_task_basic_info`, in declaration order. Only `resident_size` is
        /// read; the rest is here so the struct is the size Mach writes.
        #[repr(C)]
        #[derive(Default)]
        struct MachTaskBasicInfo {
            virtual_size: u64,
            resident_size: u64,
            resident_size_max: u64,
            user_time: [i32; 2],
            system_time: [i32; 2],
            policy: i32,
            suspend_count: i32,
        }

        const MACH_TASK_BASIC_INFO: c_uint = 20;
        const KERN_SUCCESS: c_int = 0;

        unsafe extern "C" {
            /// `mach_task_self()` is a macro over this global in the real header.
            static mach_task_self_: c_uint;
            fn task_info(
                target: c_uint,
                flavor: c_uint,
                out: *mut i32,
                count: *mut c_uint,
            ) -> c_int;
        }

        let mut info = MachTaskBasicInfo::default();
        // Mach counts the output buffer in 32-bit words, not bytes.
        let mut count = (size_of::<MachTaskBasicInfo>() / size_of::<i32>()) as c_uint;
        // SAFETY: `task_info` writes at most `count` 32-bit words into the
        // buffer, and `count` is derived from the buffer's own size. The task
        // port is a global the kernel owns; it is not a reference to release.
        let status = unsafe {
            task_info(
                mach_task_self_,
                MACH_TASK_BASIC_INFO,
                (&raw mut info).cast(),
                &mut count,
            )
        };
        if status != KERN_SUCCESS {
            return 0;
        }
        info.resident_size
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    pub(super) fn resident_bytes() -> u64 {
        0
    }
}

// ---------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod imp {
    use std::ffi::{c_int, c_void};

    /// A `FILETIME`: 100-nanosecond intervals, split across two 32-bit halves.
    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct FileTime {
        low: u32,
        high: u32,
    }

    impl FileTime {
        fn ms(self) -> f64 {
            let ticks = (u64::from(self.high) << 32) | u64::from(self.low);
            ticks as f64 / 10_000.0
        }
    }

    /// `PROCESS_MEMORY_COUNTERS`. Only `working_set` is read; the rest is here so
    /// the struct is the size the call is told it is.
    #[repr(C)]
    #[derive(Default)]
    struct ProcessMemoryCounters {
        cb: u32,
        page_fault_count: u32,
        peak_working_set: usize,
        working_set: usize,
        quota_peak_paged_pool: usize,
        quota_paged_pool: usize,
        quota_peak_non_paged_pool: usize,
        quota_non_paged_pool: usize,
        pagefile: usize,
        peak_pagefile: usize,
    }

    unsafe extern "system" {
        fn GetCurrentThread() -> *mut c_void;
        fn GetCurrentProcess() -> *mut c_void;
        fn GetThreadTimes(
            thread: *mut c_void,
            creation: *mut FileTime,
            exit: *mut FileTime,
            kernel: *mut FileTime,
            user: *mut FileTime,
        ) -> c_int;
        fn GetProcessTimes(
            process: *mut c_void,
            creation: *mut FileTime,
            exit: *mut FileTime,
            kernel: *mut FileTime,
            user: *mut FileTime,
        ) -> c_int;
        /// The kernel32 forwarder for `GetProcessMemoryInfo`, which avoids
        /// linking psapi separately.
        fn K32GetProcessMemoryInfo(
            process: *mut c_void,
            counters: *mut ProcessMemoryCounters,
            size: u32,
        ) -> c_int;
    }

    /// Both time calls have the same shape, so the sum is written once.
    fn times(process: bool) -> f64 {
        let mut creation = FileTime::default();
        let mut exit = FileTime::default();
        let mut kernel = FileTime::default();
        let mut user = FileTime::default();
        // SAFETY: the two `GetCurrent*` handles are pseudo-handles that need no
        // release, and each call writes only the four `FILETIME`s it is given,
        // all of which are initialised.
        let ok = unsafe {
            if process {
                GetProcessTimes(
                    GetCurrentProcess(),
                    &mut creation,
                    &mut exit,
                    &mut kernel,
                    &mut user,
                )
            } else {
                GetThreadTimes(
                    GetCurrentThread(),
                    &mut creation,
                    &mut exit,
                    &mut kernel,
                    &mut user,
                )
            }
        };
        if ok == 0 {
            return 0.0;
        }
        // Total, to match the Unix clocks: kernel time is what they call system.
        user.ms() + kernel.ms()
    }

    pub(super) fn thread_cpu_ms() -> f64 {
        times(false)
    }

    pub(super) fn process_cpu_ms() -> f64 {
        times(true)
    }

    pub(super) fn resident_bytes() -> u64 {
        let mut counters = ProcessMemoryCounters {
            cb: size_of::<ProcessMemoryCounters>() as u32,
            ..ProcessMemoryCounters::default()
        };
        // SAFETY: the struct is initialised and is told its own size, which is
        // the contract the call documents.
        let ok =
            unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) };
        if ok == 0 {
            return 0;
        }
        // The working set is what Task Manager calls "memory", and is the nearest
        // thing Windows has to a resident set.
        counters.working_set as u64
    }
}

#[cfg(not(any(unix, windows)))]
mod imp {
    pub(super) fn thread_cpu_ms() -> f64 {
        0.0
    }
    pub(super) fn process_cpu_ms() -> f64 {
        0.0
    }
    pub(super) fn resident_bytes() -> u64 {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn burn(ms: u64) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(ms);
        let mut sink = 0u64;
        while std::time::Instant::now() < deadline {
            sink = sink.wrapping_add(1);
        }
        assert!(sink > 0);
    }

    #[test]
    fn a_busy_thread_accrues_cpu() {
        let before = thread_cpu_ms();
        burn(60);
        let after = thread_cpu_ms();
        assert!(
            after > before,
            "thread CPU did not advance: {before} -> {after}"
        );
    }

    #[test]
    fn another_thread_is_accounted_separately() {
        // The property this exists for: a worker burning CPU must not show up in
        // the CPU of the agent that started it.
        let before = thread_cpu_ms();
        std::thread::spawn(|| {
            let start = thread_cpu_ms();
            burn(80);
            assert!(
                thread_cpu_ms() > start,
                "the busy thread reported no CPU of its own"
            );
        })
        .join()
        .expect("thread");
        let after = thread_cpu_ms();
        assert!(
            after - before < 40.0,
            "another thread's CPU landed on this one: {before} -> {after}"
        );
    }

    #[test]
    fn the_process_total_covers_every_thread() {
        burn(20);
        let thread = thread_cpu_ms();
        let process = process_cpu_ms();
        // A slack of a millisecond, because the two are not read at one instant.
        assert!(
            process + 1.0 >= thread,
            "process CPU {process} is below this thread's {thread}"
        );
    }

    #[test]
    fn the_resident_set_is_plausible() {
        let bytes = resident_bytes();
        // A V8-linked test binary is never under a megabyte, and never a terabyte.
        assert!(
            bytes > 1024 * 1024,
            "resident set implausibly small: {bytes}"
        );
        assert!(bytes < 1 << 40, "resident set implausibly large: {bytes}");
    }
}
