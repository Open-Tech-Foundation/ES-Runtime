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
//! [`Driver`] ties a [`Runtime`](es_runtime::Runtime) to a clock and a
//! timer source and advances it to quiescence on tokio — the concrete loop the
//! `runtime` crate deliberately does not own (D4).

// `unsafe` is confined to `engine`; the default providers use none.
#![forbid(unsafe_code)]

mod accept_backoff;
mod clock;
mod console;
mod driver;
mod entropy;
mod esm_resolve;
mod host_allowlist;
mod import_policy;
mod modules;
mod net;
mod node_modules;
mod path_allowlist;
mod process;
mod signals;
mod system_command;
mod system_fs;
mod system_http;
mod system_net;
mod system_sync_fs;
mod system_websocket;
mod task;
mod timers;
mod tls;

pub mod path;
pub mod testing;

pub use clock::SystemClock;
pub use console::{NullConsole, TracingConsole};
pub use driver::Driver;
pub use entropy::OsEntropy;
pub use host_allowlist::HostAllowlist;
pub use import_policy::ImportPolicy;
pub use modules::{DenyModuleLoader, FsModuleLoader};
pub use net::ReqwestTransport;
pub use node_modules::NodeModuleLoader;
pub use path_allowlist::PathAllowlist;
pub use process::SystemProcess;
pub use signals::{ManualSignals, SystemSignals};
pub use system_command::SystemCommands;
pub use system_fs::SystemFileSystem;
pub use system_http::SystemHttpServer;
pub use system_net::SystemNet;
pub use system_sync_fs::SystemSyncFileSystem;
pub use system_websocket::SystemWebSocket;
pub use task::TokioTaskSpawner;
pub use timers::TokioTimers;
