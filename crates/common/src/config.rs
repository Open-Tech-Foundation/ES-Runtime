//! Resource-limit primitives (ARCHITECTURE.md §7, SPEC.md §4).
//!
//! These are plain configuration values the runtime enforces against hostile
//! input; the enforcement lives in `engine`/`runtime`, but the *shape* of the
//! limits is shared here so every layer agrees on it. Phase 1 wires
//! [`Limits::heap_limit_bytes`] into isolate creation; the remaining fields are
//! enforced as their phases land (op concurrency, stack guard).

use crate::error::{Error, Result};

/// Per-isolate resource ceilings.
///
/// All fields are hard caps the host relies on to stay safe regardless of what
/// the executed JS does. [`Limits::default`] is a conservative, embeddable
/// baseline; embedders tune it per workload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Limits {
    /// Maximum V8 heap size in bytes, or `None` to size it from the machine.
    ///
    /// On approach the engine terminates the isolate gracefully rather than
    /// letting the host OOM (ARCHITECTURE.md §7) — `None` moves that ceiling,
    /// it does not remove it.
    ///
    /// `Some` is the embeddable default: a library that is one part of somebody
    /// else's process must not decide how much of it to take. `None` is for a
    /// runtime that *is* the process — `esrun` — where a fixed 256 MiB on a
    /// 64 GiB host is an arbitrary ceiling nobody asked for, and where Node and
    /// Deno both size the heap from the machine.
    pub heap_limit_bytes: Option<usize>,

    /// Maximum synchronous JS call-stack depth before a guard trips.
    pub max_stack_depth: u32,

    /// Maximum number of in-flight async ops per isolate (bounded concurrency).
    pub max_pending_ops: u32,

    /// Whether this agent may block its own thread — the ECMAScript agent
    /// record's `[[CanBlock]]`. When `false`, `Atomics.wait` throws a
    /// `TypeError` instead of parking the thread.
    ///
    /// `false` on the agent that drives the loop, because a blocked driver
    /// stops timers, async-op settlement and interrupts alike: the process
    /// hangs until the execution watchdog terminates it, and with no
    /// `--timeout` it hangs forever. HTML makes the same choice, setting
    /// `[[CanBlock]]` false on the window agent and true on worker agents, so
    /// the spec-mandated `TypeError` and the safe behaviour are the same thing.
    ///
    /// A worker agent sets this `true`: blocking there stops only its own
    /// thread, which is what `Atomics.wait` is for.
    pub can_block: bool,
}

impl Limits {
    /// Default heap limit: 256 MiB. Large enough for real work, small enough to
    /// keep a single isolate well clear of host memory pressure.
    pub const DEFAULT_HEAP_LIMIT_BYTES: usize = 256 * 1024 * 1024;
    /// Default stack-depth guard.
    pub const DEFAULT_MAX_STACK_DEPTH: u32 = 1024;
    /// Default bound on concurrent pending async ops.
    pub const DEFAULT_MAX_PENDING_OPS: u32 = 1024;

    /// Returns these limits with the heap ceiling replaced. Builder-style so the
    /// `#[non_exhaustive]` struct stays constructible from downstream crates.
    #[must_use]
    pub fn with_heap_limit_bytes(mut self, bytes: usize) -> Self {
        self.heap_limit_bytes = Some(bytes);
        self
    }

    /// Returns these limits with the heap ceiling sized from the host instead of
    /// fixed — see [`heap_limit_bytes`](Self::heap_limit_bytes).
    #[must_use]
    pub fn with_system_heap_limit(mut self) -> Self {
        self.heap_limit_bytes = None;
        self
    }

    /// Returns these limits with the stack-depth guard replaced.
    #[must_use]
    pub fn with_max_stack_depth(mut self, depth: u32) -> Self {
        self.max_stack_depth = depth;
        self
    }

    /// Returns these limits with the pending-op bound replaced.
    #[must_use]
    pub fn with_max_pending_ops(mut self, ops: u32) -> Self {
        self.max_pending_ops = ops;
        self
    }

    /// Returns these limits with the agent's `[[CanBlock]]` replaced. Set
    /// `true` only for an agent that owns its thread — a worker, never the
    /// driver. See [`can_block`](Self::can_block).
    #[must_use]
    pub fn with_can_block(mut self, can_block: bool) -> Self {
        self.can_block = can_block;
        self
    }

    /// Validates the limits, rejecting values that would defeat enforcement
    /// (e.g. a zero heap cap). Returns [`Error::Config`] on the first problem.
    pub fn validate(&self) -> Result<()> {
        if self.heap_limit_bytes == Some(0) {
            return Err(Error::Config("heap_limit_bytes must be non-zero".into()));
        }
        if self.max_stack_depth == 0 {
            return Err(Error::Config("max_stack_depth must be non-zero".into()));
        }
        if self.max_pending_ops == 0 {
            return Err(Error::Config("max_pending_ops must be non-zero".into()));
        }
        Ok(())
    }
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            heap_limit_bytes: Some(Limits::DEFAULT_HEAP_LIMIT_BYTES),
            max_stack_depth: Limits::DEFAULT_MAX_STACK_DEPTH,
            max_pending_ops: Limits::DEFAULT_MAX_PENDING_OPS,
            // The default agent is the one driving the loop. See `can_block`.
            can_block: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_valid() {
        assert!(Limits::default().validate().is_ok());
    }

    #[test]
    fn zero_heap_limit_is_rejected() {
        let limits = Limits {
            heap_limit_bytes: Some(0),
            ..Limits::default()
        };
        let err = limits.validate().unwrap_err();
        assert!(matches!(err, Error::Config(_)));
    }

    #[test]
    fn zero_stack_depth_is_rejected() {
        let limits = Limits {
            max_stack_depth: 0,
            ..Limits::default()
        };
        assert!(limits.validate().is_err());
    }

    #[test]
    fn the_default_agent_cannot_block() {
        // The default agent drives the loop, so `Atomics.wait` must throw
        // rather than park the only thread that can make progress.
        assert!(!Limits::default().can_block);
        assert!(Limits::default().with_can_block(true).can_block);
    }

    #[test]
    fn zero_pending_ops_is_rejected() {
        let limits = Limits {
            max_pending_ops: 0,
            ..Limits::default()
        };
        assert!(limits.validate().is_err());
    }
}
