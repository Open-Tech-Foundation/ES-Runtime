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

/// Physical memory, from `MemTotal` in `/proc/meminfo` (kibibytes).
fn physical_memory() -> Option<u64> {
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
    let total = meminfo
        .lines()
        .find_map(|line| line.strip_prefix("MemTotal:"))?;
    let kib: u64 = total.split_whitespace().next()?.parse().ok()?;
    Some(kib * 1024)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whatever this machine is, the answer has to be a usable number: the
    /// isolate's ceiling is derived from it.
    #[test]
    fn this_host_reports_something_plausible() {
        let bytes = available_bytes();
        assert!(
            bytes >= 64 * 1024 * 1024,
            "implausibly little memory: {bytes}"
        );
    }
}
