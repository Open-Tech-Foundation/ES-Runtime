//! Cross-cutting primitives shared by every ES-Runtime crate.
//!
//! This is the base of the dependency graph (ARCHITECTURE.md §2): every other
//! crate may depend on `common`, and `common` depends on nothing internal. It
//! owns no I/O and names no engine type. What lives here is the shared
//! vocabulary the layers above agree on:
//!
//! - [`error`] — the JS exception-class taxonomy and the [`IntoException`] trait
//!   that each layer's error enum implements (DECISIONS.md D12).
//! - [`capability`] — deny-by-default [`CapabilitySet`] tokens threaded from the
//!   embedder (ARCHITECTURE.md §7; DECISIONS.md D7).
//! - [`config`] — resource-[`Limits`] primitives the runtime enforces.
//! - [`telemetry`] — `tracing` setup for binaries and tests (ARCHITECTURE.md §8).

// `unsafe` is confined to `engine` (ARCHITECTURE.md §7). `common` carries none.
#![forbid(unsafe_code)]

pub mod capability;
pub mod config;
pub mod error;
pub mod telemetry;

pub use capability::{Capability, CapabilitySet};
pub use config::Limits;
pub use error::{Error, ErrorCode, ExceptionClass, IntoException, Result};
