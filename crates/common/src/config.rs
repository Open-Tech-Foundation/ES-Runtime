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
    /// Maximum V8 heap size in bytes. On approach the engine terminates the
    /// isolate gracefully rather than letting the host OOM (ARCHITECTURE.md §7).
    pub heap_limit_bytes: usize,

    /// Maximum synchronous JS call-stack depth before a guard trips.
    pub max_stack_depth: u32,

    /// Maximum number of in-flight async ops per isolate (bounded concurrency).
    pub max_pending_ops: u32,
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
        self.heap_limit_bytes = bytes;
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

    /// Validates the limits, rejecting values that would defeat enforcement
    /// (e.g. a zero heap cap). Returns [`Error::Config`] on the first problem.
    pub fn validate(&self) -> Result<()> {
        if self.heap_limit_bytes == 0 {
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
            heap_limit_bytes: Limits::DEFAULT_HEAP_LIMIT_BYTES,
            max_stack_depth: Limits::DEFAULT_MAX_STACK_DEPTH,
            max_pending_ops: Limits::DEFAULT_MAX_PENDING_OPS,
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
            heap_limit_bytes: 0,
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
    fn zero_pending_ops_is_rejected() {
        let limits = Limits {
            max_pending_ops: 0,
            ..Limits::default()
        };
        assert!(limits.validate().is_err());
    }
}
