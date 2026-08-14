//! What `esdev` adds to the `runtime:` namespace, and `esrun` does not have.
//!
//! Two modules, both of them development machinery that has no business in a
//! production binary:
//!
//! * **`runtime:build`** — the bundler, callable from guest JS. rolldown is
//!   already linked into `esdev` for the `build` subcommand; this makes it
//!   reachable from a program, which is what a framework's dev server needs in
//!   order to be a program on this runtime rather than a Node script.
//! * **`runtime:watch`** — file-change events, for that same dev server: it has
//!   to stay up through a save and invalidate what changed, which the
//!   restart-the-child watcher behind `esdev --watch` cannot do.
//!
//! Both go through the ordinary machinery — the same module pipeline, the same
//! capability gates on the ops behind them — via the
//! [`HostExtension`](es_runtime_cli_common::HostExtension) seam. Nothing here
//! is a second runtime; it is the same one with two more modules in front of
//! it, on a binary that can honour them.

pub mod build;
pub mod watch;

use es_runtime_cli_common::HostExtension;

/// Every extension `esdev` installs on a run.
pub fn extensions() -> Vec<Box<dyn HostExtension>> {
    vec![
        Box::new(build::BuildExtension),
        Box::new(watch::WatchExtension),
    ]
}
