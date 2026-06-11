//! `tracing` setup (ARCHITECTURE.md §8).
//!
//! Observability is structured `tracing`, never `println!`. Library crates only
//! *emit* spans and events; installing a subscriber is a process-global action
//! that belongs to a binary or a test. This module provides one idempotent
//! helper to do that, so `runtime-cli` and tests share a consistent setup
//! without each re-deriving it.

use tracing_subscriber::EnvFilter;

/// Installs a process-global `tracing` subscriber that formats events to stderr,
/// with the filter taken from `RUST_LOG` (falling back to `info`).
///
/// Idempotent and safe to call from multiple tests: if a global subscriber is
/// already installed, this is a no-op and returns `false`. Returns `true` when
/// it installed the subscriber.
pub fn init_tracing() -> bool {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init()
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_is_idempotent() {
        // First call may or may not win the global slot depending on test
        // ordering within the process; either way a second call must not panic
        // and must report that no fresh install happened.
        let _ = init_tracing();
        assert!(
            !init_tracing(),
            "a subscriber is already installed after the first call"
        );
    }
}
