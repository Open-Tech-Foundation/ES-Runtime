//! Reference implementations of the [provider traits](es_runtime_providers),
//! plus a standalone [`Driver`] (ARCHITECTURE.md §2/§5, DECISIONS.md D5).
//!
//! This is the **only** crate that owns a real loop, a real clock, and real OS
//! entropy. It exists for standalone use and tests; an embedder (or Layer B)
//! supplies its own providers instead. Two families live here:
//!
//! - **Production** ([`SystemClock`], [`OsEntropy`], [`TokioTimers`],
//!   [`TokioTaskSpawner`]) — backed by the OS and tokio.
//! - **Deterministic** ([`testing`]) — seeded/manual providers that make runs
//!   reproducible (DECISIONS.md D5), for tests only.
//!
//! [`Driver`] ties a [`Runtime`](es_runtime_runtime::Runtime) to a clock and a
//! timer source and advances it to quiescence on tokio — the concrete loop the
//! `runtime` crate deliberately does not own (D4).

// `unsafe` is confined to `engine`; the default providers use none.
#![forbid(unsafe_code)]

mod clock;
mod driver;
mod entropy;
mod task;
mod timers;

pub mod testing;

pub use clock::SystemClock;
pub use driver::Driver;
pub use entropy::OsEntropy;
pub use task::TokioTaskSpawner;
pub use timers::TokioTimers;
