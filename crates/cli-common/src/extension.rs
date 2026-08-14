//! Host extensions: `runtime:` modules and ops a **binary** adds to a run.
//!
//! The `runtime:` namespace is fixed on purpose — a program's imports have to
//! mean the same thing under every embedding, and a module that exists in one
//! place and not another is exactly the kind of divergence this runtime is
//! arranged to prevent. This seam is for the one case where that cannot hold:
//! **development machinery that production does not contain.**
//!
//! `esdev` adds two. `runtime:build` is the bundler — rolldown, already linked
//! into `esdev` and until now reachable only from its own `build` subcommand —
//! and `runtime:watch` is file-change events. Neither can be in `esrun`: a
//! production binary that could bundle would have to carry a bundler, and one
//! that could watch would be admitting there is source on the box to watch.
//! Both are how a framework's dev server stops being a Node program.
//!
//! What crosses the seam is deliberately small: a module's source, and the ops
//! it calls. Everything else — the capability check, the module pipeline, the
//! drive loop — is the same machinery the baked modules go through, because an
//! extension that had its own would be a second runtime.

use std::path::Path;
use std::sync::Arc;

use es_runtime::OpDecl;
use es_runtime_providers::FileSystem;

/// One `runtime:` module an extension serves: the specifier a program imports,
/// and the ES module source behind it.
pub struct HostModule {
    /// The specifier, which must be in the `runtime:` scheme and must not be
    /// one of the baked modules' names.
    pub specifier: &'static str,
    /// The module's source. Baked into the binary like every other `runtime:`
    /// module — there is no file on disk for it, and nothing reads one.
    pub source: &'static str,
}

/// What the run has already built by the time an extension's ops are made.
///
/// An extension gets the **run's own** filesystem view rather than making one,
/// which is the only way its ops can be scoped by the same
/// `--allow-read`/`--allow-write` lists as `runtime:fs`. A watcher that resolved
/// paths itself would be a second policy, and the second policy is always the
/// one with the hole in it.
pub struct ExtensionContext<'a> {
    /// The jailed, allowlist-checked filesystem view (D25/D38). Resolving a
    /// guest path through [`FileSystem::real_path`] is how an extension inherits
    /// both without reimplementing either.
    pub file_system: Arc<dyn FileSystem>,
    /// The entry module's directory — what a relative path in guest code means,
    /// the same base `runtime:fs` uses.
    pub base_dir: &'a Path,
}

/// A binary's addition to the `runtime:` namespace.
///
/// Registered on the **main agent only**. A worker gets the standard namespace
/// and nothing else: its capabilities are granted at the spawn rather than on
/// the command line, and handing background threads a bundler is not a power
/// anyone asked for.
pub trait HostExtension {
    /// The modules this extension serves.
    fn modules(&self) -> &[HostModule];

    /// The ops those modules call, built against the run's providers.
    ///
    /// Each op names the capabilities it needs ([`OpDecl::requires`]) exactly
    /// like a built-in one: the gate is the op, never the import.
    fn ops(&self, ctx: &ExtensionContext<'_>) -> Vec<OpDecl>;
}
