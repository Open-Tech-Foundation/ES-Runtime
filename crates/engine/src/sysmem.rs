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

/// The memory ceiling on this process's cgroup, from cgroup v2 or v1.
fn cgroup_limit() -> Option<u64> {
    let own = std::fs::read_to_string("/proc/self/cgroup").unwrap_or_default();
    limit_under(std::path::Path::new("/sys/fs/cgroup"), &own)
}

/// The tightest memory limit on the cgroup `own` names (the text of
/// `/proc/self/cgroup`) and on each of its ancestors, read from the tree at
/// `root`.
///
/// The process's own cgroup, not the root of the tree. The two are the same
/// only inside a container with a private cgroup namespace, Docker's default.
/// Under systemd a service sits at `system.slice/app.service` and the root has
/// no `memory.max` at all, so a unit's `MemoryMax=` went unseen and the heap
/// was sized from the host — 1.9 GiB under a 256 MiB limit. Ancestors too,
/// because a limit on a parent slice binds every service in it.
///
/// Both versions spell "unlimited" as a value near the top of the address space
/// rather than as an absent file — v2 writes the literal `max`, v1 a huge
/// number — so an implausibly large answer is read as no limit at all.
fn limit_under(root: &std::path::Path, own: &str) -> Option<u64> {
    const IMPLAUSIBLE: u64 = 1 << 50; // 1 PiB: no container has this.
    let mut files = Vec::new();
    for line in own.lines() {
        // `hierarchy-id:controllers:path`; v2 is the line with no controllers.
        let mut parts = line.splitn(3, ':');
        let (Some(_), Some(controllers), Some(path)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let (base, file) = if controllers.is_empty() {
            (root.to_path_buf(), "memory.max")
        } else if controllers.split(',').any(|c| c == "memory") {
            (root.join("memory"), "memory.limit_in_bytes")
        } else {
            continue;
        };
        let mut dir = base.join(path.trim_start_matches('/'));
        loop {
            files.push(dir.join(file));
            if dir == base || !dir.pop() {
                break;
            }
        }
    }
    files
        .iter()
        .filter_map(|file| std::fs::read_to_string(file).ok())
        .filter_map(|text| text.trim().parse::<u64>().ok()) // "max" is no limit
        .filter(|&bytes| bytes > 0 && bytes < IMPLAUSIBLE)
        .min()
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

    /// A cgroup tree under a fresh directory: `(path, file, contents)` triples.
    fn tree(name: &str, files: &[(&str, &str, &str)]) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("sysmem-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (dir, file, contents) in files {
            let dir = root.join(dir);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(file), contents).unwrap();
        }
        root
    }

    #[test]
    fn a_systemd_unit_s_limit_is_found_in_its_own_cgroup() {
        // The root has no memory.max under systemd; the service's cgroup does.
        let root = tree(
            "unit",
            &[
                ("system.slice", "memory.max", "max\n"),
                ("system.slice/app.service", "memory.max", "268435456\n"),
            ],
        );
        let own = "0::/system.slice/app.service\n";
        assert_eq!(limit_under(&root, own), Some(268_435_456));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_tightest_limit_on_the_way_up_wins() {
        let root = tree(
            "slice",
            &[
                ("work.slice", "memory.max", "134217728\n"),
                ("work.slice/app.service", "memory.max", "max\n"),
            ],
        );
        assert_eq!(
            limit_under(&root, "0::/work.slice/app.service\n"),
            Some(134_217_728)
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_namespaced_container_reads_the_root_and_v1_reads_its_hierarchy() {
        // Docker's default: the container's cgroup is the root of what it sees.
        let v2 = tree("ns", &[("", "memory.max", "536870912\n")]);
        assert_eq!(limit_under(&v2, "0::/\n"), Some(536_870_912));
        std::fs::remove_dir_all(v2).unwrap();

        let v1 = tree(
            "v1",
            &[("memory/docker/abc", "memory.limit_in_bytes", "1073741824\n")],
        );
        let own = "12:cpu,cpuacct:/docker/abc\n7:memory:/docker/abc\n0::/\n";
        assert_eq!(limit_under(&v1, own), Some(1_073_741_824));
        // v1's spelling of "unlimited" is a huge number, not a limit.
        std::fs::write(
            v1.join("memory/docker/abc/memory.limit_in_bytes"),
            "9223372036854771712\n",
        )
        .unwrap();
        assert_eq!(limit_under(&v1, own), None);
        std::fs::remove_dir_all(v1).unwrap();
    }

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
