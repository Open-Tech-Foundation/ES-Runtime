//! Watching capability checks as they happen (DECISIONS.md D59).
//!
//! Every capability-gated op consults the run's [`CapabilitySet`] before it
//! does anything (ARCHITECTURE.md §4). That check is the only place that knows
//! what a program *actually* reached for, as opposed to what it was granted —
//! which is why `esdev --trace-permissions` observes it, and why the observer
//! has to live down here in `engine` rather than in the binary that wants the
//! answer.
//!
//! [`CapabilitySet`]: es_runtime_common::CapabilitySet
//!
//! **Off unless something asks.** The hook is an `Option` read once per
//! dispatch and `None` in every production run, rather than a compile-time
//! switch: unlike the inspector this observes and cannot act, so there is
//! nothing here to keep out of `esrun` — only a branch to keep cheap. (A Cargo
//! feature would not have kept it out anyway; see `inspector`'s `build.rs` for
//! why.)

use std::sync::Arc;

use es_runtime_common::Capability;

/// Told about every capability check an op makes.
///
/// `Send + Sync` because a run is more than one agent: each worker has its own
/// thread and its own isolate, and a report that omitted what a worker did
/// would be exactly wrong — a worker's grants are set at the spawn, which is
/// where they are hardest to get right.
pub trait CapabilityObserver: Send + Sync {
    /// One check: `op` needs `capability`, and `granted` is whether the run held
    /// it. Called for **every** capability an op names, before the first missing
    /// one is refused — a denial is the most interesting thing a trace can
    /// record, so it must not be the one thing that goes unrecorded.
    ///
    /// `op` is the op's own name (`fs_read`, `process_env`), or `import` for the
    /// module loader, whose check happens above the op boundary.
    fn observed(&self, op: &str, capability: Capability, granted: bool);

    /// The run is over and a report can be made. May be called more than once —
    /// a program can end in several ways and each of them says so — so an
    /// implementation reports at most once.
    fn run_finished(&self) {
        // Nothing to summarise, by default.
    }
}

/// Shared handle to an observer, as the engine and the runtime hold it.
pub type SharedObserver = Arc<dyn CapabilityObserver>;
