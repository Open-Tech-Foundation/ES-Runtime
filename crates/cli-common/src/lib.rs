//! Shared plumbing for the ES-Runtime binaries.
//!
//! Two binaries ship from this workspace — `esrun`, the production server
//! runtime, and `esdev`, the local development tool — and a program must behave
//! identically under both. Everything that decides *how a run behaves* therefore
//! lives here once: the baked prelude snapshot, the permission-flag grammar
//! (D38), the provider wiring, the drive loop, graceful shutdown, and the error
//! block. Each binary owns only its own command line and its own subcommands.
//!
//! This crate sits above `default-providers` in the dependency order fixed by
//! ARCHITECTURE.md §2 / DECISIONS D11, and below the two binary crates. It is
//! internal: it is published only because the binaries depend on it, and carries
//! no stability promise of its own.

// These crates' whole job is to talk to the terminal.
#![allow(clippy::print_stdout, clippy::print_stderr)]

pub mod args;
pub mod console;
pub mod diagnostics;
pub mod dotenv;
pub mod extension;
pub mod permissions;
pub mod run;
pub mod shutdown;
pub mod upgrade;

pub use extension::{ExtensionContext, HostExtension, HostModule};
pub use run::{Config, Inspector, Source, run};

/// What writing a host extension's ops takes, re-exported so a binary reaches
/// for this crate rather than for the engine and the provider crates directly.
/// The same reasoning as the two re-exports above: a binary's business is its
/// command line, and it should need one dependency to state what its ops are.
pub use es_runtime::{AsyncOp, FileSystem, OpDecl, OpError, OpResult, Value};

/// The debugger transport, re-exported so a binary that implements one does not
/// have to name the engine crate to do it (`esdev`'s `--inspect` server is the
/// only implementation there is).
pub use es_runtime::InspectorTransport;

/// The capability vocabulary and the hook that watches it, re-exported for the
/// same reason: `esdev --trace-permissions` implements the observer and writes
/// its report in these names, and should reach for one crate to do it.
pub use es_runtime::{CapabilityObserver, SharedObserver};
pub use es_runtime_common::Capability;

/// The V8 startup snapshot, baked at build time by `build.rs`.
///
/// Both binaries restore this same blob rather than compiling and evaluating the
/// prelude (D8), and both share one build of it — the snapshot is expensive to
/// produce and identical for either, so it is made once here.
pub static SNAPSHOT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/prelude.snapshot.bin"));
