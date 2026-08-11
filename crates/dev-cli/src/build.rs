//! `esdev build` — a server entry and its dependencies, as one ES module.
//!
//! This is the increment that makes the npm ecosystem reachable **without
//! weakening anything about `esrun`**. The runtime loads ES modules only (D22),
//! and a large share of the registry — React among it — still ships CommonJS.
//! Rather than teach the runtime `require`, the conversion happens here, at
//! build time, on the developer's machine. What `esrun` receives is ordinary
//! ESM, and the non-goal holds completely.
//!
//! It also **narrows what production needs to be granted.** An unbundled
//! program needs `--allow-imports`, because the loader must walk `node_modules`
//! at runtime; a bundle has no imports left to resolve, so that grant can go:
//!
//! ```text
//! unbundled:  esrun --deny-all --allow-imports --allow-listen=8080 app.js
//! bundled:    esrun --deny-all --allow-listen=8080 dist/app.js
//! ```
//!
//! Four settings are what make this a command rather than a note in the README
//! telling people to run a bundler with the right flags. Getting any of them
//! wrong is silent:
//!
//! * **`runtime:*` stays external.** It is served by the runtime itself and
//!   there is nothing on disk to inline; bundling it produces an artifact that
//!   fails at the first import. This is the one a hand-written config gets
//!   wrong.
//! * **The output is ESM.** The runtime has no other module system.
//! * **`process.env.NODE_ENV` is defined**, because packages branch on it
//!   before doing anything else and nothing defines it here — there is no
//!   `process` global on this runtime.
//! * **The `worker` condition is asserted**, which is how a package with an
//!   `exports` map hands over its Web-API build rather than its `node:`-based
//!   one.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rolldown::{
    Bundler, BundlerOptions, InputItem, IsExternal, OutputFormat, Platform, RawMinifyOptions,
    ResolveOptions,
};

/// What `esdev build` was asked to do.
pub struct BuildConfig {
    /// The entry module, as written on the command line.
    pub entry: String,
    /// Where the bundle is written. Defaults to `dist/<entry stem>.js`.
    pub out: Option<String>,
    /// Whether to minify.
    pub minify: bool,
    /// Extra `exports` conditions, from `--conditions`. These **add** to the
    /// defaults rather than replacing them.
    pub conditions: Vec<String>,
    /// Extra compile-time replacements, from `--define=<name>=<value>`.
    pub defines: Vec<(String, String)>,
}

/// The `exports` conditions a build asserts before the user adds any.
///
/// `import` and `default` are rolldown's own and always present. `worker` is
/// ours, and it is the one that matters: it is the key a Web-API-targeting
/// package uses for the build that does not reach for `node:` modules — React's
/// `react-dom/server` resolves to its Web Streams implementation under it, and
/// to a `node:stream` one without it.
///
/// This is deliberately *not* the runtime's condition set. D40 keeps that
/// standards-only (`import`/`default`) and that stays true; a condition changes
/// which code runs, so the place to choose one is a build the developer ran on
/// purpose, not a server resolving imports under load.
const DEFAULT_CONDITIONS: &[&str] = &["worker"];

/// Where a bundle goes when `--out` did not say.
fn default_out(entry: &str) -> PathBuf {
    let stem = Path::new(entry)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("bundle");
    PathBuf::from("dist").join(format!("{stem}.js"))
}

/// Bundles `config` and reports where the result was written.
pub async fn build(config: BuildConfig) -> Result<String, String> {
    let entry = Path::new(&config.entry);
    if !entry.exists() {
        return Err(format!("cannot read {}", config.entry));
    }
    let out = config
        .out
        .as_ref()
        .map_or_else(|| default_out(&config.entry), PathBuf::from);
    let out_dir = out
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let out_name = out
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| format!("--out={} does not name a file", out.display()))?
        .to_string();

    let cwd = std::env::current_dir().map_err(|e| format!("cannot read working directory: {e}"))?;

    // `process.env.NODE_ENV` first, so an explicit --define of the same name
    // overrides it rather than fighting it.
    let mut define: Vec<(String, String)> = vec![(
        "process.env.NODE_ENV".to_string(),
        "\"production\"".to_string(),
    )];
    define.extend(config.defines);
    let define: HashMap<String, String> = define.into_iter().collect();

    let mut conditions: Vec<String> = DEFAULT_CONDITIONS
        .iter()
        .map(|c| (*c).to_string())
        .collect();
    conditions.extend(config.conditions);

    let options = BundlerOptions {
        input: Some(vec![InputItem {
            name: None,
            import: config.entry.clone(),
        }]),
        cwd: Some(cwd),
        dir: Some(out_dir.to_string_lossy().into_owned()),
        entry_filenames: Some(out_name.clone().into()),
        format: Some(OutputFormat::Esm),
        // Not a browser and not Node: this runtime is neither, and saying either
        // would pull in that platform's `main` fields and aliases. The
        // conditions above are how a package's Web-API build is selected, which
        // is the part `platform` would otherwise be doing by implication.
        platform: Some(Platform::Neutral),
        // `Platform::Neutral` leaves these empty, which breaks any package old
        // enough to have no `exports` map. ESM first, then the CommonJS entry —
        // which is fine, because converting it is the point.
        resolve: Some(ResolveOptions {
            condition_names: Some(conditions),
            main_fields: Some(vec!["module".to_string(), "main".to_string()]),
            ..ResolveOptions::default()
        }),
        // The setting a hand-written config gets wrong. `runtime:fs` is served
        // by the runtime and has no file behind it; inlining it would produce a
        // bundle that dies on its first import.
        external: Some(IsExternal::Fn(Some(std::sync::Arc::new(
            |specifier: &str, _importer: Option<&str>, _resolved: bool| {
                let is_builtin = specifier.starts_with("runtime:");
                Box::pin(async move { Ok(is_builtin) })
            },
        )))),
        define: Some(define.into_iter().collect()),
        minify: config.minify.then_some(RawMinifyOptions::Bool(true)),
        ..BundlerOptions::default()
    };

    let mut bundler = Bundler::new(options).map_err(|e| format!("{e:?}"))?;
    bundler.write().await.map_err(|e| format!("{e:?}"))?;

    let written = out_dir.join(&out_name);
    let size = std::fs::metadata(&written).map(|m| m.len()).unwrap_or(0);
    Ok(format!(
        "{} ({:.1} KB)",
        written.display(),
        size as f64 / 1024.0
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_output_defaults_to_dist_beside_the_entry_name() {
        assert_eq!(default_out("server.mjs"), PathBuf::from("dist/server.js"));
        assert_eq!(default_out("src/app.ts"), PathBuf::from("dist/app.js"));
    }

    /// The condition that decides whether a package hands over its Web-API build
    /// or its `node:` one. Losing it would not fail the build — it would produce
    /// a bundle that imports `node:stream` and dies at runtime.
    #[test]
    fn the_worker_condition_is_asserted_by_default() {
        assert!(DEFAULT_CONDITIONS.contains(&"worker"));
    }
}
