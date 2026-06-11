//! I/O provider traits — the integration seam (ARCHITECTURE.md §6, DECISIONS.md
//! D5).
//!
//! The runtime owns no I/O and carries no ambient authority: time, entropy,
//! timers, and offloaded work all arrive through the traits defined here. This
//! crate holds **only the trait definitions** — concrete implementations live in
//! `default-providers` (tokio-backed, for standalone use) or, later, in Layer B.
//!
//! Because clock and entropy are providers, a run is **fully reproducible** under
//! a deterministic provider set (DECISIONS.md D5): the same inputs and the same
//! providers yield the same outputs.
//!
//! Phase 3 defines [`Clock`], [`Entropy`], [`Timers`], and [`TaskSpawner`].
//! `NetTransport` and `FileSystem` arrive with their consuming APIs (fetch, FS).

// Providers are pure trait definitions; no `unsafe` (ARCHITECTURE.md §7).
#![forbid(unsafe_code)]

use std::future::Future;
use std::pin::Pin;

use es_runtime_common::{ExceptionClass, IntoException};

/// A heap-allocated, `Send` future returned by async provider methods.
///
/// Providers must be usable from a driver that may move work across threads, so
/// the future is `Send`. `'static` because provider futures outlive the call.
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// An error raised by a provider.
///
/// Provider calls return typed errors (ARCHITECTURE.md §6); this is the shared
/// shape. It maps to a JS exception via [`IntoException`] so the runtime can
/// surface it uniformly (DECISIONS.md D12).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ProviderError {
    /// The entropy source failed to produce randomness.
    #[error("entropy source failed: {0}")]
    Entropy(String),

    /// The operation was cancelled before completing.
    #[error("provider operation cancelled")]
    Cancelled,

    /// Any other provider failure.
    #[error("provider error: {0}")]
    Other(String),
}

impl IntoException for ProviderError {
    fn exception_class(&self) -> ExceptionClass {
        match self {
            // A failed CSPRNG is an environment/operation failure, not a type
            // error; surface as a generic Error (Web crypto would use
            // OperationError, a DOMException — added with the prelude).
            ProviderError::Entropy(_) => ExceptionClass::Error,
            ProviderError::Cancelled => ExceptionClass::NOT_ALLOWED,
            ProviderError::Other(_) => ExceptionClass::Error,
        }
    }
}

/// A source of time (ARCHITECTURE.md §6).
///
/// Backs `performance.now`, timers, and wall-clock reads. Splitting monotonic
/// from wall time keeps timer math immune to wall-clock jumps. A deterministic
/// `Clock` makes timer-driven runs reproducible.
pub trait Clock: Send + Sync {
    /// Milliseconds from an arbitrary fixed epoch, never decreasing. Used for
    /// timer deadlines and elapsed-time measurement.
    fn monotonic_ms(&self) -> u64;

    /// Milliseconds since the Unix epoch (UTC) — the basis for `Date.now`.
    fn wall_ms(&self) -> u64;
}

/// A cryptographically secure source of randomness (ARCHITECTURE.md §6).
///
/// Backs `crypto.getRandomValues` and `crypto.randomUUID`. A deterministic
/// (seeded, non-secure) implementation is permitted **only** for reproducible
/// tests, never for production.
pub trait Entropy: Send + Sync {
    /// Fills `dest` entirely with random bytes, or returns
    /// [`ProviderError::Entropy`] if the source failed.
    fn fill(&self, dest: &mut [u8]) -> Result<(), ProviderError>;
}

/// A scheduler of delayed wakeups (ARCHITECTURE.md §6).
///
/// Backs the `setTimeout`/`setInterval` family and lets the driver park until
/// the next timer is due instead of busy-polling.
pub trait Timers: Send + Sync {
    /// A future that completes no sooner than `delay_ms` from now.
    fn sleep(&self, delay_ms: u64) -> BoxFuture<()>;
}

/// An offloader of blocking work (ARCHITECTURE.md §6).
///
/// Lets an op run a blocking closure off the driving thread at the provider's
/// discretion. Results flow through state the closure captures (e.g. a channel);
/// the returned future completes when the work has run.
pub trait TaskSpawner: Send + Sync {
    /// Runs `work` off the calling thread; the future resolves once it finishes.
    fn spawn_blocking(&self, work: Box<dyn FnOnce() + Send + 'static>) -> BoxFuture<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    // A trivial in-test implementation proves the traits are object-safe and
    // usable through `dyn`, which is how the runtime/driver consume them.
    struct FixedClock;
    impl Clock for FixedClock {
        fn monotonic_ms(&self) -> u64 {
            42
        }
        fn wall_ms(&self) -> u64 {
            1_000
        }
    }

    #[test]
    fn clock_is_object_safe() {
        let clock: &dyn Clock = &FixedClock;
        assert_eq!(clock.monotonic_ms(), 42);
        assert_eq!(clock.wall_ms(), 1_000);
    }

    #[test]
    fn provider_error_maps_to_exception() {
        let err = ProviderError::Entropy("no /dev/urandom".into());
        assert_eq!(err.exception_class(), ExceptionClass::Error);
        assert_eq!(
            ProviderError::Cancelled.exception_class(),
            ExceptionClass::NOT_ALLOWED
        );
    }
}
