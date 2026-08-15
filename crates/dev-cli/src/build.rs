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
//! unbundled:  esrun --allow-imports --allow-listen=8080 app.js
//! bundled:    esrun --allow-listen=8080 dist/app.js
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
    BundlerOptions, InputItem, IsExternal, OutputFormat, Platform, RawMinifyOptions,
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
    /// A **directory** for an application build — a target's `outdir`, where
    /// `out` names one file.
    ///
    /// The distinction is not cosmetic: a dynamic `import()` emits a chunk
    /// beside its entry ([`config::Output::Dir`](crate::config::Output::Dir)),
    /// and a build whose whole output is one named file has nowhere to put a
    /// second one.
    pub out_dir: Option<String>,
    /// Which environment the output runs in. Decides the conditions a
    /// dependency's `exports` map is read under.
    pub platform: crate::config::Platform,
    /// Files and directories copied into the output directory verbatim.
    pub assets: Vec<String>,
    /// The directory the paths above are relative to, and the bundler's working
    /// directory. `None` is the process's own — which is every command line,
    /// since a path typed into a shell is relative to where it was typed.
    pub root: Option<PathBuf>,
    /// Whether this is a build for the dev loop rather than for deploying.
    ///
    /// Two things follow, and only two: `NODE_ENV` is `"development"` (so React
    /// and everything like it hands over the build with the warnings in it),
    /// and nothing is content-hashed. A stable filename is what keeps a reload
    /// cheap and a stack trace readable, and it is exactly wrong for a
    /// deployment, where the unchanged name is what serves last week's bundle
    /// to half your users.
    ///
    /// Everything else is identical to a release build, deliberately. Dev and
    /// prod differing on how a module *resolves* is the failure this whole
    /// toolchain is arranged to prevent.
    pub dev: bool,
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
    /// The entry whose declarations are linked into one file, from
    /// `--dts-bundle[=<entry>]`. `None` leaves a `.d.ts` beside each module.
    pub dts_bundle: Option<String>,
}

/// A bundler failure, in the shape a person reads.
///
/// Rolldown's own `Display` is the message alone — "Unexpected token" — which
/// in a project of twenty files is most of a question rather than an answer.
/// The module id is what turns it into one, and it is the first thing a dev
/// loop's user needs, because they are looking at the editor rather than the
/// terminal when it happens.
fn diagnostics<D: std::fmt::Display>(reported: Vec<(Option<String>, D)>) -> String {
    reported
        .into_iter()
        .map(|(id, diagnostic)| match id {
            Some(id) => format!("{id}: {diagnostic}"),
            None => diagnostic.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The bundler's failures, paired with the module each came from.
///
/// Spelled at the call sites rather than as a typed helper because the
/// diagnostic type belongs to a crate this one does not depend on directly —
/// it arrives through rolldown, and naming it would mean declaring a
/// dependency to write one signature.
macro_rules! reported {
    () => {
        |error| {
            $crate::build::diagnostics(
                error
                    .into_vec()
                    .into_iter()
                    .map(|diagnostic| (diagnostic.id(), diagnostic))
                    .collect(),
            )
        }
    };
}

/// The `process.env.NODE_ENV` a build defines before any `--define` overrides
/// it.
fn node_env(dev: bool) -> &'static str {
    if dev {
        "\"development\""
    } else {
        "\"production\""
    }
}

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

/// Empties the output directory before a library build writes into it.
///
/// **A library build owns its output tree, so a stale file in it is a published
/// file.** Delete a module from `src` and without this its `.js` and `.d.ts`
/// stay in `dist` for ever — and `"files": ["dist"]` puts them in the tarball,
/// where a consumer can still import a module the library no longer has. This is
/// the `rm -rf dist` a hand-written build script always ends up growing, and the
/// reason it grows it.
///
/// Only `--lib` cleans. An application build's `--out` names a *file*, in a
/// directory that may hold other builds and other people's files; emptying it
/// would be a surprise with no upside, since the one file is overwritten anyway.
///
/// The refusals are the point of the function. `--out` is a path off a command
/// line, and the difference between emptying `dist` and emptying `src` is one
/// keystroke.
fn clean_output(out_dir: &Path, source_root: &Path) -> Result<(), String> {
    if !out_dir.exists() {
        return Ok(());
    }
    if !out_dir.is_dir() {
        return Err(format!(
            "--out={} is a file, and --lib writes a directory of them.",
            out_dir.display()
        ));
    }
    // Resolved rather than compared as written: `dist`, `./dist` and an absolute
    // path to it are one directory, and a symlink is whatever it points at.
    // Every question below is about that real directory.
    let resolved = out_dir
        .canonicalize()
        .map_err(|e| format!("cannot read {}: {e}", out_dir.display()))?;
    let source = source_root
        .canonicalize()
        .map_err(|e| format!("cannot read {}: {e}", source_root.display()))?;
    let cwd = std::env::current_dir().map_err(|e| format!("cannot read working directory: {e}"))?;

    // Emptying the output would empty the input. `--out=src` is the whole
    // library, one keystroke away from `--out=dist`.
    if source.starts_with(&resolved) {
        return Err(format!(
            "--out={} holds the source ({}), and a library build empties its \
             output directory first.\n\n\
             That would delete what it is about to build. Write somewhere the \
             build owns: --out=dist.",
            out_dir.display(),
            source_root.display()
        ));
    }
    // …and emptying the project would take everything else in it too.
    if cwd.starts_with(&resolved) {
        return Err(format!(
            "--out={} holds the working directory, and a library build empties \
             its output directory first.\n\n\
             Name a directory the build owns: --out=dist.",
            out_dir.display()
        ));
    }
    std::fs::remove_dir_all(&resolved)
        .map_err(|e| format!("cannot clear {}: {e}", out_dir.display()))
}

/// Bundles `config` and reports what was written.
pub async fn build(config: BuildConfig) -> Result<String, String> {
    let cwd = match &config.root {
        Some(root) => root.clone(),
        None => {
            std::env::current_dir().map_err(|e| format!("cannot read working directory: {e}"))?
        }
    };
    if !cwd.join(&config.source).exists() {
        return Err(format!("cannot read {}", config.source));
    }

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
    } else if let Some(dir) = &config.out_dir {
        // A directory output. The entry keeps its own name — `[name].js` with
        // the stem given explicitly — so a rebuild overwrites the same file
        // and the HTML that points at it does not have to be rewritten to
        // follow. Chunks land beside it under rolldown's own hashed names.
        let stem = Path::new(&config.source)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("bundle")
            .to_string();
        (
            PathBuf::from(dir),
            "[name].js".to_string(),
            None,
            vec![InputItem {
                name: Some(stem),
                import: config.source.clone(),
            }],
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

    // Before anything is written, and only for `--lib`. It is also what makes
    // the count below honest: the emitted modules are read back off disk, so a
    // stale file left in place would be reported as one of them.
    if let Some(root) = &preserve_root {
        clean_output(&out_dir, root)?;
    }

    // `process.env.NODE_ENV` first, so an explicit --define of the same name
    // overrides it rather than fighting it. A library defines nothing by
    // default: a replacement made here is one the consumer's build can no
    // longer make, and which environment their code runs in is theirs to say.
    let mut define: Vec<(String, String)> = if config.lib {
        Vec::new()
    } else {
        vec![(
            "process.env.NODE_ENV".to_string(),
            node_env(config.dev).to_string(),
        )]
    };
    define.extend(config.defines);
    let define: HashMap<String, String> = define.into_iter().collect();

    // Same reasoning for conditions: `worker` picks which build of a dependency
    // is inlined, and a library inlines none of them.
    //
    // A **browser** target asserts `browser` instead. The two are alternatives,
    // not additions: a package that offers both means them for different
    // places, and asserting `worker` while bundling for a browser hands over a
    // build written for somewhere without a `document`. Conditions match in the
    // order the *package author* wrote them (D40), so the wrong one being
    // present at all is enough to win.
    let target = match config.platform {
        crate::config::Platform::Server => crate::resolve::Target::Server,
        crate::config::Platform::Browser => crate::resolve::Target::Browser,
    };
    // A library externalises everything that is not its own source, so there is
    // nothing left for a condition to pick between — and baking one in would
    // publish a package that had already chosen for its consumer.
    let conditions = if config.lib {
        config.conditions.clone()
    } else {
        crate::resolve::conditions(target, config.conditions.clone())
    };

    let lib = config.lib;
    let options = BundlerOptions {
        input: Some(inputs),
        cwd: Some(cwd.clone()),
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
        //
        // A browser target is the exception, because there it is simply true —
        // and the aliases that come with it (a package's `browser` field, which
        // predates `exports` and is still how a good deal of the registry
        // redirects away from `node:` builtins) are the point of saying so.
        platform: Some(match config.platform {
            crate::config::Platform::Server => Platform::Neutral,
            crate::config::Platform::Browser => Platform::Browser,
        }),
        resolve: Some(ResolveOptions {
            condition_names: Some(conditions),
            main_fields: crate::resolve::main_fields(target),
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

    // The same CSS Modules plugin the browser build runs, and for the same
    // reason the *server* needs it: a component importing `./x.module.css`
    // renders `className={styles.button}`, so the server has to resolve that to
    // the identical scoped name or the markup it sends will not match the
    // stylesheet the browser fetched.
    //
    // Its CSS output is discarded here. The name is derived from the file's
    // path relative to the project root, so both builds arrive at it
    // independently and neither has to tell the other. What the browser build
    // writes is the one copy.
    let mut bundler = rolldown::BundlerBuilder::default()
        .with_options(options)
        .with_plugins(vec![std::sync::Arc::new(
            crate::cssmodules::CssModules::new(
                &cwd,
                crate::cssmodules::Collected::new(),
                config.minify,
            ),
        )])
        .build()
        .map_err(reported!())?;
    bundler.write().await.map_err(reported!())?;

    if !config.lib {
        // `[name].js` is a pattern, not a filename, so the directory case is
        // reported by the name the entry was given rather than by the literal
        // it was written with.
        let written = match &config.out_dir {
            Some(dir) => {
                let stem = Path::new(&config.source)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("bundle");
                PathBuf::from(dir).join(format!("{stem}.js"))
            }
            None => out_dir.join(&filenames),
        };
        let size = std::fs::metadata(cwd.join(&written))
            .map(|m| m.len())
            .unwrap_or(0);
        let copied = copy_assets(&config.assets, &cwd, &cwd.join(&out_dir))?;
        return Ok(format!(
            "{} ({:.1} KB{copied})",
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
        counted.push_str(&match &config.dts_bundle {
            Some(entry) => {
                let written = bundle_declarations(entry, &modules, &out_dir, &root)?;
                format!(", 1 declaration ({written})")
            }
            None => {
                let declared = crate::declarations::emit(&modules, &out_dir, &root)?;
                format!(
                    ", {declared} declaration{}",
                    if declared == 1 { "" } else { "s" }
                )
            }
        });
    }
    Ok(format!("{}/ ({counted})", out_dir.display()))
}

/// Bundles the module scripts an HTML file names, and reports what each of them
/// was written as.
///
/// Separate from [`build`] because the unit is different: that builds *one*
/// entry to a name the caller chose, and this builds **every entry one document
/// referenced** to names the bundler chooses — content-hashed, so the HTML has
/// to be told what they turned out to be. Building them in one bundler run
/// rather than one each is what lets two scripts on a page share a chunk instead
/// of carrying two copies of their common imports.
///
/// **`runtime:*` is not external here, and that is deliberate.** In a server
/// bundle it is left for the runtime to serve; in a browser there is nothing to
/// serve it, so leaving it external would emit a bundle whose first import fails
/// in somebody's browser. Unresolved, it fails here instead, naming the module
/// and the file that imported it.
#[allow(
    clippy::too_many_arguments,
    reason = "every one is a distinct build setting; a struct would only move the list"
)]
pub async fn bundle_browser_entries(
    entries: Vec<(String, String)>,
    root: &Path,
    out_dir: &Path,
    dev: bool,
    minify: bool,
    defines: Vec<(String, String)>,
    conditions: Vec<String>,
    styles: &crate::cssmodules::Collected,
) -> Result<Vec<(String, String)>, String> {
    // Hashed for a deployment, stable for the dev loop — the same call `dev`
    // makes everywhere, spelled once here.
    let hash = !dev;
    let mut define: Vec<(String, String)> = vec![(
        "process.env.NODE_ENV".to_string(),
        node_env(dev).to_string(),
    )];
    define.extend(defines);
    let condition_names = crate::resolve::conditions(crate::resolve::Target::Browser, conditions);

    let names: Vec<String> = entries.iter().map(|(name, _)| name.clone()).collect();
    let options = BundlerOptions {
        input: Some(
            entries
                .into_iter()
                .map(|(name, import)| InputItem {
                    name: Some(name),
                    import,
                })
                .collect(),
        ),
        cwd: Some(root.to_path_buf()),
        dir: Some(out_dir.to_string_lossy().into_owned()),
        // Written under the entry's own name and hashed *afterwards*, below.
        // Nothing imports an entry — a chunk is imported by it, never the other
        // way round — so renaming one once it is written breaks no reference,
        // and it keeps this from having to read the bundler's own report of
        // what it called things.
        entry_filenames: Some("[name].js".to_string().into()),
        // A chunk is always hashed: nothing names one, so there is no filename
        // to keep stable, and a shared chunk that changed without its name
        // changing is a browser running two halves of two builds.
        chunk_filenames: Some("[name]-[hash].js".to_string().into()),
        format: Some(OutputFormat::Esm),
        platform: Some(Platform::Browser),
        resolve: Some(ResolveOptions {
            condition_names: Some(condition_names),
            main_fields: crate::resolve::main_fields(crate::resolve::Target::Browser),
            ..ResolveOptions::default()
        }),
        define: Some(
            define
                .into_iter()
                .collect::<HashMap<_, _>>()
                .into_iter()
                .collect(),
        ),
        minify: minify.then_some(RawMinifyOptions::Bool(true)),
        ..BundlerOptions::default()
    };

    // The one plugin: a `.module.css` import becomes its name mapping, and the
    // scoped CSS is pushed into `styles` for the caller to write out. See
    // [`crate::cssmodules`].
    let mut bundler = rolldown::BundlerBuilder::default()
        .with_options(options)
        .with_plugins(vec![std::sync::Arc::new(
            crate::cssmodules::CssModules::new(root, styles.clone(), minify),
        )])
        .build()
        .map_err(reported!())?;
    bundler.write().await.map_err(reported!())?;

    let mut written = Vec::new();
    for name in names {
        let path = out_dir.join(format!("{name}.js"));
        let bytes =
            std::fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        if !hash {
            written.push((name.clone(), format!("{name}.js")));
            continue;
        }
        let filename = crate::html::hashed_name(&path, &bytes);
        std::fs::rename(&path, out_dir.join(&filename))
            .map_err(|e| format!("cannot name {filename}: {e}"))?;
        written.push((name, filename));
    }
    Ok(written)
}

/// Copies a target's `assets` into its output directory, and reports how many.
///
/// **A file is copied by name; a directory is copied by its contents.** So
/// `"assets": ["index.html", "public"]` puts `index.html` and everything under
/// `public/` at the root of the output — which is what makes `/styles.css` the
/// URL of `public/styles.css` without anything having to rewrite a path. A
/// directory copied *as* a directory would put it at `/public/styles.css`, and
/// every href in the project would have to know that.
///
/// This is also what makes a deployment one directory. The runtime resolves a
/// relative path against the **entry module's** directory, so a server bundle in
/// `dist/` reading `index.html` reads `dist/index.html` — not the one in the
/// source tree, which is not shipped and, on the machine that runs it, is not
/// there at all.
fn copy_assets(assets: &[String], root: &Path, into: &Path) -> Result<String, String> {
    if assets.is_empty() {
        return Ok(String::new());
    }
    let mut copied = 0usize;
    for asset in assets {
        let from = root.join(asset);
        if !from.exists() {
            return Err(format!(
                "cannot read {asset}, listed in this target's assets"
            ));
        }
        if from.is_dir() {
            copied += copy_tree(&from, into)?;
        } else {
            let name = from
                .file_name()
                .ok_or_else(|| format!("{asset} does not name a file"))?;
            std::fs::create_dir_all(into)
                .map_err(|e| format!("cannot create {}: {e}", into.display()))?;
            std::fs::copy(&from, into.join(name))
                .map_err(|e| format!("cannot copy {asset}: {e}"))?;
            copied += 1;
        }
    }
    Ok(format!(
        ", {copied} asset{}",
        if copied == 1 { "" } else { "s" }
    ))
}

/// Copies the contents of `from` into `into`, recursively; reports the file
/// count.
fn copy_tree(from: &Path, into: &Path) -> Result<usize, String> {
    std::fs::create_dir_all(into).map_err(|e| format!("cannot create {}: {e}", into.display()))?;
    let mut copied = 0usize;
    let entries =
        std::fs::read_dir(from).map_err(|e| format!("cannot read {}: {e}", from.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        let target = into.join(entry.file_name());
        if path.is_dir() {
            copied += copy_tree(&path, &target)?;
        } else {
            std::fs::copy(&path, &target)
                .map_err(|e| format!("cannot copy {}: {e}", path.display()))?;
            copied += 1;
        }
    }
    Ok(copied)
}

/// What `esdev build` was asked to build: one entry named on the command line,
/// or the targets a project describes.
pub enum BuildRequest {
    /// `esdev build <entry>` — one bundle, configured entirely by flags.
    Single(Box<BuildConfig>),
    /// `esdev build` — every target in `esdev.json`, or the one `--target` named.
    Project(Box<ProjectBuild>),
}

/// A build of the targets in a project's config.
pub struct ProjectBuild {
    /// The parsed config.
    ///
    /// Shared rather than owned because `esdev start` builds the same project
    /// once a keystroke, and re-reading the file each time would mean a build
    /// that quietly changed shape under an edit the developer had not finished.
    pub project: std::sync::Arc<crate::config::Project>,
    /// The targets to build; `None` is all of them.
    pub targets: Option<Vec<String>>,
    /// `--minify`, which turns it on for every target built.
    pub minify: bool,
    /// `--define`s, added to every target's own.
    pub defines: Vec<(String, String)>,
    /// `--conditions`, added to every target's own.
    pub conditions: Vec<String>,
    /// Set when this build is feeding the dev loop rather than a deployment.
    pub dev: Option<Dev>,
}

/// What a dev-loop build needs to know that a release build does not.
pub struct Dev {
    /// The port esdev's own endpoint is on, so a document can be given the few
    /// lines that reload it.
    pub reload_port: u16,
}

/// Where a target's bundle lands.
pub fn output_path(target: &crate::config::Target) -> PathBuf {
    match &target.output {
        crate::config::Output::File(out) => PathBuf::from(out),
        crate::config::Output::Dir(dir) => {
            let stem = Path::new(&target.entry)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("bundle");
            PathBuf::from(dir).join(format!("{stem}.js"))
        }
    }
}

/// Builds what was asked for, reporting each artifact as it lands.
///
/// A project build prints per target rather than returning one summary: a build
/// of four entries takes long enough that silence until the end is silence
/// about which of them is slow, and the first failure should name the target it
/// happened in while the others are still visibly pending.
pub async fn run(request: BuildRequest) -> Result<(), String> {
    let project = match request {
        BuildRequest::Single(config) => {
            let verb = if config.lib { "built" } else { "bundled" };
            let written = build(*config).await?;
            println!("{verb} → {written}");
            return Ok(());
        }
        BuildRequest::Project(project) => project,
    };

    let names: Vec<&str> = project
        .project
        .targets
        .iter()
        .map(|t| t.name.as_str())
        .collect();
    let selected: Vec<&crate::config::Target> = match &project.targets {
        None => project.project.targets.iter().collect(),
        Some(wanted) => wanted
            .iter()
            .map(|name| {
                project
                    .project
                    .targets
                    .iter()
                    .find(|target| &target.name == name)
                    .ok_or_else(|| {
                        format!(
                            "--target={name} is not a target in this project.\n\n\
                             Targets: {}.",
                            names.join(", ")
                        )
                    })
            })
            .collect::<Result<Vec<_>, _>>()?,
    };

    for target in &selected {
        let mut defines = target.define.clone();
        defines.extend(project.defines.iter().cloned());
        let mut conditions = target.conditions.clone();
        conditions.extend(project.conditions.iter().cloned());

        // A document is its own kind of build: what it references is what gets
        // built, and what is written is the document pointing at the results.
        if target.is_html() {
            let crate::config::Output::Dir(dir) = &target.output else {
                return Err(format!(
                    "target \"{}\": an HTML target writes a directory",
                    target.name
                ));
            };
            let out_dir = project.project.dir.join(dir);
            let written = crate::html::build(
                target,
                &project.project.dir,
                &out_dir,
                project.dev.as_ref(),
                target.minify || project.minify,
                defines,
                conditions,
            )
            .await
            .map_err(|e| format!("target \"{}\": {e}", target.name))?;
            copy_assets(&target.assets, &project.project.dir, &out_dir)
                .map_err(|e| format!("target \"{}\": {e}", target.name))?;
            println!("built → {written}");
            continue;
        }

        let (out, out_dir) = match &target.output {
            crate::config::Output::File(out) => (Some(out.clone()), None),
            crate::config::Output::Dir(dir) => (None, Some(dir.clone())),
        };
        let written = build(BuildConfig {
            source: target.entry.clone(),
            out,
            out_dir,
            platform: target.platform,
            assets: target.assets.clone(),
            root: Some(project.project.dir.clone()),
            dev: project.dev.is_some(),
            // A flag beats the file: `--minify` on a config that does not ask
            // for it is how a release build is taken of a project whose day to
            // day is unminified.
            minify: target.minify || project.minify,
            conditions,
            defines,
            lib: false,
            types: false,
            dts_bundle: None,
        })
        .await
        .map_err(|e| format!("target \"{}\": {e}", target.name))?;
        println!("bundled → {written}");
    }

    // Every bundle exists before any of them runs. A prerender step renders the
    // pages of a site that the *browser* bundle is referenced from, so ordering
    // it after the whole build is the difference between generating HTML that
    // points at a file and HTML that points at one that does not exist yet.
    for target in selected.iter().filter(|t| t.run_after_build) {
        let output = output_path(target);
        run_output(&project.project.dir, &output)
            .await
            .map_err(|e| format!("target \"{}\": {e}", target.name))?;
        println!("ran → {}", output.display());
    }
    Ok(())
}

/// Runs a built artifact as the next step of the build.
///
/// **In a child process, not in this one.** The same call `esdev test` makes and
/// for the same reason: the step is somebody's program, and a program that
/// wedges, exhausts its heap or calls `exit()` should take only itself down —
/// not the build, and not the three artifacts already written. It runs under
/// `esdev` with the ordinary developer-machine grant, because writing a
/// directory of HTML is the entire point of the step.
async fn run_output(root: &Path, output: &Path) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot find the esdev binary: {e}"))?;
    let status = tokio::process::Command::new(exe)
        .arg(output)
        .current_dir(root)
        .status()
        .await
        .map_err(|e| format!("cannot run {}: {e}", output.display()))?;
    if status.success() {
        return Ok(());
    }
    Err(format!(
        "{} exited {}",
        output.display(),
        status
            .code()
            .map_or_else(|| "on a signal".to_string(), |code| code.to_string())
    ))
}

/// Links every declaration reachable from `entry` into one file, and reports
/// where it went.
///
/// The output is named after the entry, so `src/index.ts` becomes
/// `dist/index.d.ts` — the path a package's `types` field already points at,
/// and the same one the per-module build would have written for that module.
fn bundle_declarations(
    entry: &str,
    modules: &[PathBuf],
    out_dir: &Path,
    root: &Path,
) -> Result<String, String> {
    let entry_path = Path::new(entry);
    if !entry_path.is_file() {
        return Err(format!(
            "--dts-bundle={entry} does not name a file.\n\n\
             One declaration file is built from one entry — the module a consumer \
             importing your package arrives at."
        ));
    }
    let generated = crate::declarations::generate(modules, root)?;
    let text = crate::dts::bundle(entry_path, &generated)?;

    let stem = entry_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("index");
    let target = out_dir.join(format!("{stem}.d.ts"));
    std::fs::write(&target, text).map_err(|e| format!("cannot write {}: {e}", target.display()))?;
    Ok(target.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_output_defaults_to_dist_beside_the_entry_name() {
        assert_eq!(default_out("server.mjs"), PathBuf::from("dist/server.js"));
        assert_eq!(default_out("src/app.ts"), PathBuf::from("dist/app.js"));
    }

    /// A directory of its own per test: these delete things.
    fn clean_fixture(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("esdev_clean_{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).expect("create src");
        std::fs::create_dir_all(dir.join("dist")).expect("create dist");
        std::fs::write(dir.join("src/index.ts"), "export const x: number = 1;").expect("write");
        dir
    }

    #[test]
    fn cleaning_empties_the_output_and_leaves_the_source() {
        let dir = clean_fixture("ok");
        std::fs::write(dir.join("dist/stale.js"), "gone").expect("write");

        clean_output(&dir.join("dist"), &dir.join("src")).expect("clean");
        assert!(!dir.join("dist/stale.js").exists());
        assert!(dir.join("src/index.ts").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Nothing to clean is not an error — the first build of a project has no
    /// output directory yet.
    #[test]
    fn cleaning_a_directory_that_is_not_there_is_fine() {
        let dir = clean_fixture("absent");
        clean_output(&dir.join("nowhere"), &dir.join("src")).expect("clean");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The refusal that matters: `--out=src` differs from `--out=dist` by one
    /// keystroke, and would delete the library rather than build it.
    #[test]
    fn cleaning_refuses_to_empty_anything_holding_the_source() {
        let dir = clean_fixture("source");

        let onto_itself = clean_output(&dir.join("src"), &dir.join("src")).expect_err("refused");
        assert!(onto_itself.contains("holds the source"), "{onto_itself}");

        // …and the parent of the source, which takes it with everything else.
        let onto_parent = clean_output(&dir, &dir.join("src")).expect_err("refused");
        assert!(onto_parent.contains("holds the source"), "{onto_parent}");

        assert!(dir.join("src/index.ts").exists(), "the source was deleted");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cleaning_refuses_a_path_that_is_a_file() {
        let dir = clean_fixture("file");
        std::fs::write(dir.join("bundle.js"), "x").expect("write");

        let err = clean_output(&dir.join("bundle.js"), &dir.join("src")).expect_err("refused");
        assert!(err.contains("is a file"), "{err}");
        assert!(dir.join("bundle.js").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
