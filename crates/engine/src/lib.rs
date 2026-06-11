//! V8 embedding for ES-Runtime — the **only** crate that uses the `v8` crate
//! (DECISIONS.md D2, D3; ARCHITECTURE.md §3).
//!
//! Everything V8-specific is confined here. The public surface is expressed in
//! plain Rust and [`es_runtime_common`] types so that crates above (`runtime`,
//! Phase 4+) never name a V8 type — the test of the engine boundary is that a
//! second engine could be slotted in behind it without editing `runtime`.
//!
//! Phase 1 establishes the lifecycle and proves it end-to-end:
//!
//! - [`Engine`] — owns a V8 isolate + a persistent context; [`Engine::eval`]
//!   compiles and runs source and marshals the result to a [`Value`].
//! - [`snapshot`] — startup-snapshot build/load scaffolding (DECISIONS.md D8),
//!   with [`Engine::with_snapshot`] restoring a captured context.
//!
//! Later phases grow this into the full abstraction in ARCHITECTURE.md §3 (op
//! registration, module instantiation, execution control). The op system,
//! driven loop, and a formal engine *trait* arrive in Phase 2; see the D3a leak
//! list in DECISIONS.md for boundary caveats recorded as they appear.
//!
//! ## Safety
//!
//! `unsafe` is permitted in this crate alone (ARCHITECTURE.md §7), under
//! `#![forbid(unsafe_op_in_unsafe_fn)]` (workspace lint, DECISIONS.md D1). Phase
//! 1 requires **no** `unsafe`: the `v8` crate's scope-based API is entirely safe
//! for what we do here. The forbid lint stands guard for when later phases need
//! it; any `unsafe` block added must carry a `// SAFETY:` invariant note.

mod convert;
mod engine;
pub mod error;
pub mod op;
pub mod snapshot;
mod value;

pub use engine::{Engine, V8Engine};
pub use error::{Error, Result};
pub use op::{AsyncOp, OpDecl, OpError, OpHandler, OpResult, TimerId};
pub use value::Value;

use std::sync::Once;

/// Guards one-time, process-global V8 platform initialization.
static V8_INIT: Once = Once::new();

/// Initializes the V8 platform exactly once per process.
///
/// V8 requires its platform be set up before any isolate is created, and that
/// this happens a single time for the life of the process. Every entry point
/// that touches V8 ([`V8Engine::new`], [`snapshot::build`], …) calls this first,
/// so callers never have to sequence it themselves. Idempotent and thread-safe
/// via [`Once`].
/// Serializes V8-touching unit tests within this test binary.
///
/// V8 forbids snapshot creation from running concurrently with other isolate
/// creation (the `v8` crate serializes its own snapshot tests the same way).
/// Cargo's harness runs a binary's tests in parallel, so every test that builds
/// an [`Engine`] or a snapshot acquires this guard for its whole body. This is a
/// test concern only; in production the embedder owns thread placement (the
/// constraint is documented on [`snapshot::build`]).
#[cfg(test)]
pub(crate) fn v8_test_guard() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    // The guard only serializes; a poisoned lock carries no invalid state.
    LOCK.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub(crate) fn ensure_v8_initialized() {
    V8_INIT.call_once(|| {
        // A default platform with V8's own thread-pool sizing (0 = auto) and no
        // idle-task support — the embedder drives the loop (ARCHITECTURE.md §5),
        // so V8 owns no background idle work here.
        let platform = v8::new_default_platform(0, false).make_shared();
        v8::V8::initialize_platform(platform);
        v8::V8::initialize();
        tracing::debug!("V8 platform initialized");
    });
}
