//! The `runtime:` built-in module namespace (DECISIONS D24).
//!
//! `runtime:<name>` specifiers are served by the runtime itself — not the
//! injected [`ModuleLoader`](es_runtime_providers::ModuleLoader) — so the
//! standard library exists regardless of which loader (or none) an embedder
//! installs, and never touches the filesystem. Each entry is a baked ES module
//! source (like the prelude, but imported as a module) that calls
//! capability-gated ops. Loading and dedup go through the normal module
//! pipeline; the capability check lives in the ops, not here.

/// The baked source for a `runtime:` built-in module, or `None` if the
/// specifier is not a known built-in.
pub(crate) fn source(specifier: &str) -> Option<&'static str> {
    match specifier {
        "runtime:process" => Some(include_str!("runtime_modules/process.js")),
        "runtime:path" => Some(include_str!("runtime_modules/path.js")),
        "runtime:fs" => Some(include_str!("runtime_modules/fs.js")),
        "runtime:net" => Some(include_str!("runtime_modules/net.js")),
        "runtime:http" => Some(include_str!("runtime_modules/http.js")),
        _ => None,
    }
}

/// Whether `specifier` names the `runtime:` built-in scheme.
pub(crate) fn is_builtin_scheme(specifier: &str) -> bool {
    specifier.starts_with("runtime:")
}
