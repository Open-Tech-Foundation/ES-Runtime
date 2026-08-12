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
//!
//! # `--lib`: the same command, for something that is not deployed
//!
//! All four are right for an application and wrong for a **library**, because a
//! library is not the end of the line — it is an input to somebody else's build,
//! and every one of those settings is a decision that belongs to *them*:
//!
//! * **The unit is a source directory, not an entry.** Which modules a consumer
//!   may import is decided by the package's `exports` map, long after this build
//!   ran — so there is no root to start from and nothing is unreachable. Every
//!   module under the directory is built, and built as an *entry*, which is what
//!   keeps an export no current caller uses (see [`source_entries`]).
//! * **Dependencies stay external.** Inlining `hono` into a published package
//!   ships a private copy of it that the consumer cannot dedupe, override or
//!   patch. In `--lib` every bare specifier is left alone, so a dependency
//!   stays a dependency.
//! * **Module structure is preserved**, file for file. That is what makes a
//!   subpath `exports` map possible (`./pool` has to *be* a file), what keeps a
//!   stack trace pointing at a module rather than an offset into a bundle, and
//!   what lets a test import one internal module without the package exporting
//!   it.
//! * **Nothing is defined and no condition is asserted**, because
//!   `NODE_ENV=production` and `worker` are the consumer's build's call. Baking
//!   them in freezes their environment into your package.
//! * **`.d.ts` travels with the `.js`** ([`crate::declarations`]). A library is
//!   a typed contract; a build that emitted only JavaScript would leave every
//!   author reaching for a second tool.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rolldown::{
    Bundler, BundlerOptions, InputItem, IsExternal, OutputFormat, Platform, RawMinifyOptions,
    ResolveOptions, TreeshakeOptions,
};

/// What `esdev build` was asked to do.
pub struct BuildConfig {
    /// What to build, as written on the command line: the **entry module** for
    /// an application, the **source directory** for `--lib`.
    ///
    /// The asymmetry is the difference between the two artifacts. A bundle has
    /// one root — that is what makes it one file. A library has no root at all:
    /// which of its modules a consumer imports is decided by the package's
    /// `exports` map, long after this build ran, so the unit is the tree.
    pub source: String,
    /// Where the output goes: a **file** for an application build (default
    /// `dist/<entry stem>.js`), a **directory** for `--lib` (default `dist`).
    pub out: Option<String>,
    /// Whether to minify.
    pub minify: bool,
    /// Extra `exports` conditions, from `--conditions`. These **add** to the
    /// defaults rather than replacing them.
    pub conditions: Vec<String>,
    /// Extra compile-time replacements, from `--define=<name>=<value>`.
    pub defines: Vec<(String, String)>,
    /// Build a library rather than a deployable application.
    pub lib: bool,
    /// Whether a `--lib` build emits `.d.ts` files. On unless `--no-types`.
    pub types: bool,
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

/// The extensions a library's source tree is built from.
const SOURCE_EXTENSIONS: &[&str] = &["ts", "tsx", "mts", "cts", "js", "mjs", "jsx"];

/// Every source module under `root`, as an entry: its path, and the name the
/// output takes (the path relative to `root`, without the extension).
///
/// **Every module is an entry, and that is the whole reason a library builds
/// from a directory rather than from a file.** A module reached only as an
/// import keeps just the exports its importer used — which is correct for an
/// application, where nothing else will ever run, and wrong for a library,
/// whose callers have not been written yet. Making each one an entry says what
/// is true: every module here is somebody's starting point.
///
/// The layout that falls out is `tsc`'s — `src/**` becomes `dist/**`, one file
/// each — which is the layout a package's `exports` map is already written
/// against.
///
/// Skipped: `*.test.*` (esdev's own name for a test, and not something to
/// publish), `.d.ts` files (already declarations), and dot-directories.
fn source_entries(root: &Path) -> Vec<(String, String)> {
    let mut entries = Vec::new();
    collect_sources(root, root, &mut entries);
    entries.sort();
    entries
}

fn collect_sources(root: &Path, dir: &Path, entries: &mut Vec<(String, String)>) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for item in read.flatten() {
        let path = item.path();
        let name = item.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || name == "node_modules" {
            continue;
        }
        if path.is_dir() {
            collect_sources(root, &path, entries);
            continue;
        }
        let is_source = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| SOURCE_EXTENSIONS.contains(&e));
        if !is_source || name.ends_with(".d.ts") || name.contains(".test.") {
            continue;
        }
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        entries.push((
            relative.with_extension("").to_string_lossy().into_owned(),
            path.to_string_lossy().into_owned(),
        ));
    }
}

/// Whether a specifier names a file in the project rather than a package.
///
/// The distinction `--lib` turns on: a relative or absolute specifier is part
/// of the library and is emitted, anything else is a dependency and is left for
/// the consumer's resolver to find.
fn is_local(specifier: &str) -> bool {
    specifier.starts_with('.') || Path::new(specifier).is_absolute()
}

/// Bundles `config` and reports what was written.
pub async fn build(config: BuildConfig) -> Result<String, String> {
    if !Path::new(&config.source).exists() {
        return Err(format!("cannot read {}", config.source));
    }

    let cwd = std::env::current_dir().map_err(|e| format!("cannot read working directory: {e}"))?;

    // An application build names one file; a library build names a source
    // directory and mirrors it. Everything below that differs between the two
    // follows from that one difference — a library has an output *tree*,
    // because its module structure is part of what it publishes.
    let (out_dir, filenames, preserve_root, inputs) = if config.lib {
        let root = PathBuf::from(&config.source);
        let entries = source_entries(&root);
        if entries.is_empty() {
            return Err(format!(
                "no source modules under {}\n\n\
                 --lib builds a source directory the way tsc builds a rootDir: \
                 every module in it becomes a file in the output.",
                root.display()
            ));
        }
        let out = config.out.clone().unwrap_or_else(|| "dist".to_string());
        (
            PathBuf::from(out),
            "[name].js".to_string(),
            Some(root),
            entries
                .into_iter()
                .map(|(name, import)| InputItem {
                    // Named explicitly, so `[name].js` reproduces the source
                    // tree. Left to default it would be the file *stem*, and
                    // `a/config.ts` and `b/config.ts` would collide on one
                    // output file — silently, since a collision is just a
                    // second write.
                    name: Some(name),
                    import,
                })
                .collect(),
        )
    } else {
        let out = config
            .out
            .as_ref()
            .map_or_else(|| default_out(&config.source), PathBuf::from);
        let dir = out
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        let name = out
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| format!("--out={} does not name a file", out.display()))?
            .to_string();
        (
            dir,
            name,
            None,
            vec![InputItem {
                name: None,
                import: config.source.clone(),
            }],
        )
    };

    // `process.env.NODE_ENV` first, so an explicit --define of the same name
    // overrides it rather than fighting it. A library defines nothing by
    // default: a replacement made here is one the consumer's build can no
    // longer make, and which environment their code runs in is theirs to say.
    let mut define: Vec<(String, String)> = if config.lib {
        Vec::new()
    } else {
        vec![(
            "process.env.NODE_ENV".to_string(),
            "\"production\"".to_string(),
        )]
    };
    define.extend(config.defines);
    let define: HashMap<String, String> = define.into_iter().collect();

    // Same reasoning for conditions: `worker` picks which build of a dependency
    // is inlined, and a library inlines none of them.
    let mut conditions: Vec<String> = if config.lib {
        Vec::new()
    } else {
        DEFAULT_CONDITIONS
            .iter()
            .map(|c| (*c).to_string())
            .collect()
    };
    conditions.extend(config.conditions);

    let lib = config.lib;
    let options = BundlerOptions {
        input: Some(inputs),
        cwd: Some(cwd),
        dir: Some(out_dir.to_string_lossy().into_owned()),
        entry_filenames: Some(filenames.clone().into()),
        // Only meaningful for `--lib`, where a module that is not an entry is
        // still a file somebody may import. Left at the same pattern so the
        // emitted tree mirrors the source tree exactly, with no hashes in it —
        // a hashed filename is unimportable and unpublishable.
        chunk_filenames: lib.then(|| filenames.clone().into()),
        preserve_modules: lib.then_some(true),
        preserve_modules_root: preserve_root
            .as_ref()
            .map(|root| root.to_string_lossy().into_owned()),
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
        //
        // A library externalises everything that is not its own source as well.
        // That is the difference between publishing a package and publishing a
        // private copy of the registry: a consumer can dedupe, override or patch
        // a dependency they still have, and can do none of those to one that was
        // inlined into a file they did not write.
        external: Some(IsExternal::Fn(Some(std::sync::Arc::new(
            move |specifier: &str, _importer: Option<&str>, _resolved: bool| {
                let is_external =
                    specifier.starts_with("runtime:") || (lib && !is_local(specifier));
                Box::pin(async move { Ok(is_external) })
            },
        )))),
        define: Some(define.into_iter().collect()),
        minify: config.minify.then_some(RawMinifyOptions::Bool(true)),
        // **A library keeps every export it wrote.** Tree-shaking asks "what
        // does the entry use?", and for an application that is the whole
        // question — nothing else will ever run. For a library it is the wrong
        // question: the code that will use these modules has not been written
        // yet, so an export no *current* caller reaches is not dead, it is the
        // API.
        //
        // Found by building this repository's own Redis driver with it: shaking
        // removed `BLOCKING_COMMANDS` from `protocol/blocking.js` because only a
        // test imported it, and the failure was a SyntaxError at import time in
        // the consumer rather than anything the build said. Whatever really is
        // dead here, the consumer's own build removes — it can only shake what
        // it was given.
        treeshake: if lib {
            TreeshakeOptions::Boolean(false)
        } else {
            TreeshakeOptions::default()
        },
        ..BundlerOptions::default()
    };

    let mut bundler = Bundler::new(options).map_err(|e| format!("{e:?}"))?;
    bundler.write().await.map_err(|e| format!("{e:?}"))?;

    if !config.lib {
        let written = out_dir.join(&filenames);
        let size = std::fs::metadata(&written).map(|m| m.len()).unwrap_or(0);
        return Ok(format!(
            "{} ({:.1} KB)",
            written.display(),
            size as f64 / 1024.0
        ));
    }

    let root = preserve_root.unwrap_or_else(|| PathBuf::from("."));
    let modules = crate::declarations::emitted_modules(&out_dir);
    let mut counted = format!(
        "{} module{}",
        modules.len(),
        if modules.len() == 1 { "" } else { "s" }
    );
    if config.types {
        let declared = crate::declarations::emit(&modules, &out_dir, &root)?;
        if declared > 0 {
            counted.push_str(&format!(
                ", {declared} declaration{}",
                if declared == 1 { "" } else { "s" }
            ));
        }
    }
    Ok(format!("{}/ ({counted})", out_dir.display()))
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
