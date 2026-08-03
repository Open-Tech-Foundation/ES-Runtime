//! `tracing` setup (ARCHITECTURE.md §8).
//!
//! Observability is structured `tracing`, never `println!`. Library crates only
//! *emit* spans and events; installing a subscriber is a process-global action
//! that belongs to a binary or a test. This module provides one idempotent
//! helper to do that, so `runtime-cli` and tests share a consistent setup
//! without each re-deriving it.

use std::io::IsTerminal;

use tracing_subscriber::EnvFilter;

/// The filter used when `RUST_LOG` says nothing.
///
/// `warn`, not `info`: the only `warn!` sites in the tree are the three accept
/// loops reporting that they could not accept and are backing off — a failure
/// of the listening socket, which is the operator's problem and not something a
/// peer can provoke. Nothing else fires in normal operation, so a healthy
/// process stays silent. `info` would be a global level, so hyper, rustls and
/// reqwest could log at it too, and dependency chatter in the default output of
/// every run is how a log people are supposed to read becomes one they filter
/// out.
const DEFAULT_FILTER: &str = "warn";

/// Installs a process-global `tracing` subscriber that formats events to stderr,
/// with the filter taken from `RUST_LOG` (falling back to
/// [`DEFAULT_FILTER`]).
///
/// `RUST_LOG` rather than a name of our own: it is what `tracing-subscriber`
/// already reads, it is not runtime-branded, and an embedder who installs their
/// own subscriber gets the identical target names with nothing to translate.
///
/// Idempotent and safe to call from multiple tests: if a global subscriber is
/// already installed, this is a no-op and returns `false`. Returns `true` when
/// it installed the subscriber.
pub fn init_tracing() -> bool {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        // Colour only when a human is reading. Piped to a file or a log
        // collector, escape sequences are not decoration, they are corruption
        // of the field they wrap — `peer=1.2.3.4` stops being greppable. The
        // same test the CLI applies to its own diagnostics.
        .with_ansi(std::io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none())
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
