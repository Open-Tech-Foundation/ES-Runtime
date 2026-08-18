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

use std::path::{Path, PathBuf};

use rolldown::InputItem;

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

/// Refuses an output directory a library build must not be allowed to replace.
///
/// **A library build owns its output tree, so a stale file in it is a published
/// file.** Delete a module from `src` and without this its `.js` and `.d.ts`
/// would stay in `dist` for ever — and `"files": ["dist"]` puts them in the
/// tarball, where a consumer can still import a module the library no longer
/// has. So the directory is *replaced* rather than written into
/// ([`crate::staging`]), which is the `rm -rf dist` a hand-written build script
/// always grows, moved to the end where a failed build cannot benefit from it.
///
/// Only `--lib` owns its output. An application build's `--out` names a *file*,
/// in a directory that may hold other builds and other people's files; replacing
/// it would be a surprise with no upside, since the one file is overwritten
/// anyway.
///
/// The refusals are the point of the function. `--out` is a path off a command
/// line, and the difference between replacing `dist` and replacing `src` is one
/// keystroke.
fn guard_replacement(out_dir: &Path, source_root: &Path) -> Result<(), String> {
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

    // Replacing the output would replace the input. `--out=src` is the whole
    // library, one keystroke away from `--out=dist`.
    if source.starts_with(&resolved) {
        return Err(format!(
            "--out={} holds the source ({}), and a library build replaces its \
             output directory.\n\n\
             That would delete what it is about to build. Write somewhere the \
             build owns: --out=dist.",
            out_dir.display(),
            source_root.display()
        ));
    }
    // …and replacing the project would take everything else in it too.
    if cwd.starts_with(&resolved) {
        return Err(format!(
            "--out={} holds the working directory, and a library build replaces \
             its output directory.\n\n\
             Name a directory the build owns: --out=dist.",
            out_dir.display()
        ));
    }
    Ok(())
}

/// The directories a whole-project release build **replaces** rather than
/// writes into.
///
/// # Why a build has to replace anything at all
///
/// Because output filenames are content-hashed and inputs change. `app-1a2b.js`
/// becomes `app-9f8e.js` and the old one would stay, for ever, in the directory
/// that gets deployed — and beside it whatever `esdev start` left, which is
/// *not* hashed and so is a file with an ordinary name that nothing will ever
/// overwrite. What ships is then bigger than the build, and a cache or a
/// hand-edited URL can still reach a version of the app nobody is testing.
///
/// The replacement happens at the end, when the staged build is moved into
/// place ([`crate::staging`]). It used to happen at the start, and that is
/// exactly how a failed build came to destroy the deployment that was working.
///
/// # Only when the build owns everything it is replacing
///
/// Two conditions, and both are about ownership rather than tidiness:
///
/// * **Every target is being built.** `--target=web` writes one target's output
///   into a directory that may be shared with another's, and replacing it would
///   delete a bundle this run is not going to rebuild.
/// * **It is not the dev loop.** `esdev start` rebuilds one target at a time
///   into files with stable names, so nothing accumulates — and a rebuild that
///   replaced the directory would delete the other targets' output with it.
///
/// Only `outdir` directories are owned. `out` names one file in a directory the
/// build does not own ([`crate::config::Output`]); such a file is replaced here
/// only when it happens to sit inside another target's `outdir`, and a
/// whole-project build writes it again on the way past.
///
/// A directory that holds the project root is refused rather than replaced. It
/// is the same keystroke `--lib` guards against, arriving by a different route:
/// `"outdir": "."` is a config away from `"outdir": "dist"`.
fn owned_dirs(
    project: &ProjectBuild,
    selected: &[&crate::config::Target],
) -> Result<Vec<PathBuf>, String> {
    if project.targets.is_some() || project.dev.is_some() {
        return Ok(Vec::new());
    }
    let root = project
        .project
        .dir
        .canonicalize()
        .unwrap_or_else(|_| project.project.dir.clone());

    let mut owned = Vec::new();
    for target in selected {
        let crate::config::Output::Dir(dir) = &target.output else {
            continue;
        };
        let out = project.project.dir.join(dir);
        // A directory that is not there yet is the first build, which has
        // nothing to replace and still owns where it is going.
        if let Ok(resolved) = out.canonicalize()
            && root.starts_with(&resolved)
        {
            return Err(format!(
                "target \"{}\": `outdir` is {}, which holds the project itself.\n\n\
                 A build replaces the directory it owns, and that would delete \
                 everything else here too. Name a directory the build owns: \
                 \"outdir\": \"dist\".",
                target.name,
                out.display()
            ));
        }
        owned.push(PathBuf::from(dir));
    }
    Ok(owned)
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

    // Same reasoning for conditions: `worker` picks which build of a dependency
    // is inlined, and a library inlines none of them.
    //
    // A **browser** target asserts `browser` instead. The two are alternatives,
    // not additions: a package that offers both means them for different
    // places, and asserting `worker` while bundling for a browser hands over a
    // build written for somewhere without a `document`. Conditions match in the
    // order the *package author* wrote them (D40), so the wrong one being
    // present at all is enough to win.
    let target = if config.lib {
        crate::resolve::Target::Library
    } else {
        match config.platform {
            crate::config::Platform::Server => crate::resolve::Target::Server,
            crate::config::Platform::Browser => crate::resolve::Target::Browser,
        }
    };

    let lib = config.lib;
    let options = crate::bundler::Options {
        input: inputs.into_iter().map(|i| (i.name, i.import)).collect(),
        cwd: Some(cwd.clone()),
        platform: target,
        conditions: config.conditions.clone(),
        define,
        minify: config.minify,
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
        treeshake: lib.then_some(false),
        // Only meaningful for `--lib`, where a module that is not an entry is
        // still a file somebody may import.
        preserve_modules: lib.then_some(true),
        preserve_modules_root: preserve_root
            .as_ref()
            .map(|root| root.to_string_lossy().into_owned()),
        // The setting a hand-written config gets wrong. `runtime:fs` is served
        // by the runtime and has no file behind it; inlining it would produce a
        // bundle that dies on its first import.
        //
        // A library externalises everything that is not its own source as well.
        // That is the difference between publishing a package and publishing a
        // private copy of the registry: a consumer can dedupe, override or patch
        // a dependency they still have, and can do none of those to one that was
        // inlined into a file they did not write.
        external: Some(crate::bundler::External::when(move |specifier, _, _| {
            specifier.starts_with("runtime:") || (lib && !is_local(specifier))
        })),
        output: crate::bundler::OutputOptions {
            dir: Some(out_dir.to_string_lossy().into_owned()),
            entry_filenames: Some(filenames.clone()),
            // Left at the same pattern as an entry so the emitted tree mirrors
            // the source tree exactly, with no hashes in it — a hashed filename
            // is unimportable and unpublishable.
            chunk_filenames: lib.then(|| filenames.clone()),
            ..crate::bundler::OutputOptions::default()
        },
        ..crate::bundler::Options::default()
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
    let translated = crate::bundler::translate(&options, options.output.clone(), None)?;
    let mut bundler = rolldown::BundlerBuilder::default()
        .with_options(translated)
        .with_plugins(vec![std::sync::Arc::new(crate::adapter::Adapter::new(
            std::sync::Arc::new(crate::cssmodules::CssModules::new(
                &cwd,
                crate::cssmodules::Collected::new(),
                config.minify,
            )),
        ))])
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
    hmr: Option<String>,
    refresh: bool,
) -> Result<(Vec<(String, String)>, Vec<crate::cssmodules::Sheet>), String> {
    // Hashed for a deployment, stable for the dev loop — the same call `dev`
    // makes everywhere, spelled once here.
    let hash = !dev;
    let mut define: Vec<(String, String)> = vec![(
        "process.env.NODE_ENV".to_string(),
        node_env(dev).to_string(),
    )];
    define.extend(defines);

    let names: Vec<String> = entries.iter().map(|(name, _)| name.clone()).collect();
    // Everything that shapes the build, before any of it is moved into the
    // options. A held bundler whose settings no longer match is the wrong
    // bundler, and reusing it would apply the first run's options to the
    // second's inputs.
    let key = format!(
        "{entries:?}|{}|{}|{minify}|{hash}|{refresh}|{define:?}|{conditions:?}",
        root.display(),
        out_dir.display()
    );
    let options = crate::bundler::Options {
        input: entries
            .into_iter()
            .map(|(name, import)| (Some(name), import))
            .collect(),
        cwd: Some(root.to_path_buf()),
        platform: crate::resolve::Target::Browser,
        conditions,
        define,
        minify,
        hmr_runtime: hmr,
        react_refresh: refresh,
        output: crate::bundler::OutputOptions {
            dir: Some(out_dir.to_string_lossy().into_owned()),
            // Written under the entry's own name and hashed *afterwards*,
            // below. Nothing imports an entry — a chunk is imported by it,
            // never the other way round — so renaming one once it is written
            // breaks no reference, and it keeps this from having to read the
            // bundler's own report of what it called things.
            entry_filenames: Some("[name].js".to_string()),
            // A chunk is always hashed: nothing names one, so there is no
            // filename to keep stable, and a shared chunk that changed without
            // its name changing is a browser running two halves of two builds.
            chunk_filenames: Some("[name]-[hash].js".to_string()),
            ..crate::bundler::OutputOptions::default()
        },
        ..crate::bundler::Options::default()
    };

    // The one plugin: a `.module.css` import becomes its name mapping, and the
    // scoped CSS is pushed into a collector for the caller to write out. See
    // [`crate::cssmodules`].
    //
    // In the dev loop the bundler and its plugin are *held* between rebuilds
    // ([`warm`]); everywhere else they are built for this run and dropped with
    // it. Which is why the collector is the bundler's rather than the caller's:
    // a held plugin keeps the handle it was constructed with, so a caller that
    // made a fresh one each build would be reading an empty one.
    let styles = if dev {
        let held = warm().lock().await;
        build_warm(held, &key, root, out_dir, &options, minify, refresh).await?
    } else {
        let styles = crate::cssmodules::Collected::new();
        let mut bundler = rolldown::BundlerBuilder::default()
            .with_options(crate::bundler::translate(
                &options,
                options.output.clone(),
                None,
            )?)
            .with_plugins(browser_plugins(root, &styles, minify, refresh))
            .build()
            .map_err(reported!())?;
        bundler.write().await.map_err(reported!())?;
        styles.take()
    };

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
    Ok((written, styles))
}

/// The passes a browser build runs, in the order they are declared.
///
/// Fast Refresh's wrapper only exists in a dev loop that asked for it: it makes
/// every component module a hot boundary, which is exactly right when a page is
/// being patched and pointless weight in anything shipped.
fn browser_plugins(
    root: &Path,
    styles: &crate::cssmodules::Collected,
    minify: bool,
    refresh: bool,
) -> Vec<std::sync::Arc<dyn rolldown::plugin::Pluginable>> {
    let mut plugins: Vec<std::sync::Arc<dyn rolldown::plugin::Pluginable>> =
        vec![std::sync::Arc::new(crate::adapter::Adapter::new(
            std::sync::Arc::new(crate::cssmodules::CssModules::new(
                root,
                styles.clone(),
                minify,
            )),
        ))];
    if refresh {
        plugins.push(std::sync::Arc::new(crate::adapter::Adapter::new(
            std::sync::Arc::new(crate::refresh::ReactRefresh::new()),
        )));
    }
    plugins
}

/// The browser bundler, held across the rebuilds of one `esdev start`.
///
/// # Why it is held at all
///
/// Two reasons, and the second is the one that makes it necessary rather than
/// nice. A rebuild re-walks a module graph it has already walked, which is work
/// the bundler's own cache can skip. And **rolldown's HMR refuses to run without
/// it** — `compute_hmr_update_for_file_changes` needs the bundler that produced
/// the bundle the browser is currently running, and errors with *"HMR requires
/// to run at least one bundle before invalidation"* against a fresh one.
///
/// # Why a static rather than an argument
///
/// It belongs to the process, there can only ever be one dev loop in one, and
/// reaching it from `bundle_browser_entries` means threading a handle down
/// through `start`, `run` and `html::build` for a value with exactly one
/// instance. That is D71's reasoning for the test tally, and it holds here for
/// the same reason.
///
/// A `tokio::sync::Mutex` because a `Bundler` is `Send` but not `Sync`, and
/// because it is held across an `await`.
static WARM: std::sync::OnceLock<tokio::sync::Mutex<Option<Warm>>> = std::sync::OnceLock::new();

fn warm() -> &'static tokio::sync::Mutex<Option<Warm>> {
    WARM.get_or_init(|| tokio::sync::Mutex::new(None))
}

/// A held bundler, and what it was built for.
struct Warm {
    /// The HMR session: what has been shipped to the page, when each module was
    /// last rendered, and the counter that names the next patch file.
    ///
    /// **One session, not one per page.** rolldown's API takes a list of clients
    /// with a ship map each, because two tabs opened at different times can
    /// legitimately need different patches. esdev keeps one: the pages of a dev
    /// loop have almost always loaded the same bundle, and the one that has not
    /// is caught on the other side — a patch whose factories do not fit the
    /// graph a page holds makes that page reload itself. A registry of clients
    /// is a real thing to want and it is not worth its weight yet.
    hmr: Option<HmrSession>,
    /// Where the bundle is written, and so where a patch has to go: the page
    /// fetches it from the same directory it fetched the bundle from.
    out_dir: PathBuf,
    /// Every build setting that shaped it. A run whose settings differ is a
    /// different build, and reusing a bundler across that would silently apply
    /// the first run's options to the second's inputs.
    key: String,
    bundler: rolldown::Bundler,
    /// The plugin's collector, drained after each build. Held here because the
    /// plugin holds the handle it was constructed with, and the plugin is
    /// inside the bundler.
    styles: crate::cssmodules::Collected,
}

/// How many patch files stay on disk.
///
/// Enough that a page which was told about one can still fetch it after several
/// more saves — a tab on a slow machine, a paused debugger, a second browser —
/// and few enough that a directory served to a browser does not fill with them.
const PATCHES_KEPT: usize = 8;

/// What one dev loop has told the browser so far.
struct HmrSession {
    /// Module stable id → the rebuild stamp of the copy the page holds.
    shipped: rustc_hash::FxHashMap<arcstr::ArcStr, u32>,
    /// When each module was last rendered, which is how rolldown decides what a
    /// patch has to carry.
    stamps: rolldown_common::HmrStampTable,
    /// Names the next `hmr_patch_N.js`. Shared with rolldown, which increments
    /// it as it builds one.
    next_patch: std::sync::Arc<std::sync::atomic::AtomicU32>,
    /// The patches still on disk, oldest first.
    ///
    /// They are never overwritten — each one is a new number — so without this
    /// a long afternoon leaves hundreds of them in the directory the browser is
    /// served from. Deleting the newest few is not safe: a page told about a
    /// patch fetches it a moment later, and a slow tab, a paused debugger or a
    /// second browser can still be reaching for one. So a few are kept and the
    /// rest go.
    written: std::collections::VecDeque<String>,
}

/// A hot update, ready to hand to the page.
pub struct Hot {
    /// The patch file, relative to the output directory.
    pub filename: String,
    /// The modules that changed. The page walks its own graph from these.
    pub changed_ids: Vec<String>,
}

/// Forgets what the browser has been sent, so the next patch carries all of it.
///
/// Called when a page connects. The ship map records what *has been delivered*,
/// and a patch is trimmed against it — so a tab that arrives later, having
/// loaded a bundle rather than the patches before it, is missing exactly the
/// factories the next patch assumes it already has. It cannot apply that patch
/// and reloads itself, which is safe and costs it its state.
///
/// Forgetting is the cheap half of the fix: the next patch is computed as though
/// nothing had been delivered, so it carries what the newest page needs, and the
/// pages that already had those modules are handed a superset — which is what
/// rolldown ships anyway, and what the client's own graph walk is built to
/// filter. One slightly larger patch per page opened, against a page that would
/// otherwise have been reloaded.
///
/// The thorough half is a ship map per client, which this deliberately is not.
pub async fn forget_shipped() {
    if let Some(warm) = warm().lock().await.as_mut()
        && let Some(session) = warm.hmr.as_mut()
    {
        session.shipped.clear();
    }
}

/// Computes a hot update for `changed`, writing the patch beside the bundle.
///
/// `Ok(None)` means there is nothing to hot-apply and the caller should fall
/// back to a reload — either rolldown said so (a change no patch can represent,
/// like a tsconfig that re-transforms every module) or there is no held bundler
/// to compute against, which is the first build.
///
/// The patch is *written*, not returned: rolldown hands back the code and the
/// name it should have, and leaves the writing to whoever is serving it — which
/// is us, out of the same directory the bundle is served from.
pub async fn hot_update(changed: &[PathBuf]) -> Option<Hot> {
    let mut held = warm().lock().await;
    let warm = held.as_mut()?;
    let out_dir = warm.out_dir.clone();
    let session = warm.hmr.get_or_insert_with(|| HmrSession {
        shipped: rustc_hash::FxHashMap::default(),
        stamps: rolldown_common::HmrStampTable::default(),
        next_patch: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0)),
        written: std::collections::VecDeque::new(),
    });

    let mut files: rolldown_utils::indexmap::FxIndexMap<
        String,
        rolldown_common::WatcherChangeKind,
    > = rolldown_utils::indexmap::FxIndexMap::default();
    for path in changed {
        files.insert(
            path.to_string_lossy().into_owned(),
            rolldown_common::WatcherChangeKind::Update,
        );
    }

    let clients = [rolldown_common::ClientHmrInput {
        client_id: "esdev",
        shipped: &session.shipped,
    }];
    let updates = warm
        .bundler
        .compute_hmr_update_for_file_changes(
            &files,
            &clients,
            &mut session.stamps,
            std::sync::Arc::clone(&session.next_patch),
            false,
        )
        .await;
    let updates = match updates {
        Ok(updates) => updates,
        // Reported rather than swallowed. A patch that cannot be computed is a
        // page that reloads, which looks exactly like a dev loop with no hot
        // updates at all — so the one thing that must not happen is this failing
        // quietly.
        Err(errors) => {
            eprintln!("esdev: no hot update ({errors:?})");
            return None;
        }
    };

    let update = updates.into_iter().next()?;
    match update.update {
        rolldown_common::HmrUpdate::Patch(patch) => {
            std::fs::write(out_dir.join(&patch.filename), &patch.code).ok()?;
            // Only once the patch is on disk: the ship map records what the page
            // *can* have, and recording a delivery that failed would leave the
            // next patch assuming a module the page never got.
            for (id, stamp) in patch.carried {
                session.shipped.insert(id, stamp);
            }
            session.written.push_back(patch.filename.clone());
            while session.written.len() > PATCHES_KEPT {
                if let Some(old) = session.written.pop_front() {
                    let _ = std::fs::remove_file(out_dir.join(old));
                }
            }
            Some(Hot {
                filename: patch.filename,
                changed_ids: patch.changed_ids,
            })
        }
        // Something no patch can express. Its reason is rolldown's own words,
        // and it is the sort of thing a developer wants to see rather than a
        // page that reloads for no stated cause.
        rolldown_common::HmrUpdate::FullReload { reason } => {
            eprintln!("esdev: reloading — {reason}");
            None
        }
        rolldown_common::HmrUpdate::Noop => None,
    }
}

/// Builds the browser entries on the held bundler, making one if there is none
/// or if the settings have changed.
async fn build_warm(
    mut held: tokio::sync::MutexGuard<'_, Option<Warm>>,
    key: &str,
    root: &Path,
    out_dir: &Path,
    options: &crate::bundler::Options,
    minify: bool,
    refresh: bool,
) -> Result<Vec<crate::cssmodules::Sheet>, String> {
    if held.as_ref().is_none_or(|warm| warm.key != key) {
        let styles = crate::cssmodules::Collected::new();
        let bundler = rolldown::BundlerBuilder::default()
            .with_options(crate::bundler::translate(
                options,
                options.output.clone(),
                None,
            )?)
            .with_plugins(browser_plugins(root, &styles, minify, refresh))
            .build()
            .map_err(reported!())?;
        *held = Some(Warm {
            key: key.to_string(),
            bundler,
            styles,
            out_dir: out_dir.to_path_buf(),
            // A new bundler is a new graph, so whatever the page was told about
            // the old one is worthless. Starting the session empty is what stops
            // a patch being computed against a ship map for a build that no
            // longer exists.
            hmr: None,
        });
    }

    let warm = held.as_mut().expect("just built");
    // A failed build must not leave the next one reading half a graph, and
    // rolldown keeps its own cache coherent across a failure — so the handle
    // stays held either way and the error is simply reported.
    warm.bundler.write().await.map_err(reported!())?;
    // Drained, not read: the same collector serves every rebuild, and sheets
    // left in it would be emitted again next time.
    Ok(warm.styles.take())
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
    /// Whether to build the browser bundle in rolldown's dev mode — modules
    /// registered with a runtime instead of scope-hoisted into one another, and
    /// `import.meta.hot` on each.
    ///
    /// On by default, and `--no-hot` turns it off. It is not free — rolldown's
    /// dev mode forces treeshaking off, so the react template's dev bundle goes
    /// from 870 KB to 1.45 MB and a rebuild costs about 20 ms more — but what it
    /// buys is the state in the page surviving a save, which is the difference
    /// between editing a form and refilling it. Both numbers are development
    /// only; nothing shipped is affected either way.
    pub hot: bool,
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
            let written = build_single(*config).await?;
            let paint = crate::style::Palette::stdout();
            println!("{} {} {written}", paint.green(verb), paint.dim("→"));
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

    // Nothing is written where it is deployed until every target — and every
    // step that runs after them — has succeeded. See [`crate::staging`].
    let mut staging = crate::staging::Staging::new(&project.project.dir, project.dev.is_none())?;
    for dir in owned_dirs(&project, &selected)? {
        staging.own(dir);
    }

    match build_targets(&project, &selected, &staging).await {
        // Everything worked, so everything moves. Until this line the output on
        // disk is the last build that worked.
        Ok(()) => staging.commit(),
        // …and this one drops the staging directory with it. The note is worth
        // the two lines: the paths in whatever the failing step printed name a
        // directory that no longer exists, and a developer looking at an
        // untouched `dist` should know it is untouched rather than stale.
        Err(err) => Err(format!(
            "{err}\n\n\
             Nothing was written: a build stages its output and moves it into \
             place only once every target and every step has succeeded, so what \
             is deployed is still the last build that worked."
        )),
    }
}

/// Builds every selected target into `staging`, then runs the steps that run
/// after a build — the order the whole file is about.
async fn build_targets(
    project: &ProjectBuild,
    selected: &[&crate::config::Target],
    staging: &crate::staging::Staging,
) -> Result<(), String> {
    for target in selected {
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
            let out_dir = staging.path(dir);
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
            let paint = crate::style::Palette::stdout();
            println!(
                "{} {} {}",
                paint.green("built"),
                paint.dim("→"),
                staging.reveal(&written)
            );
            continue;
        }

        let (out, out_dir) = match &target.output {
            crate::config::Output::File(out) => {
                (Some(staging.path(out).to_string_lossy().into_owned()), None)
            }
            crate::config::Output::Dir(dir) => {
                (None, Some(staging.path(dir).to_string_lossy().into_owned()))
            }
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
        .map_err(|e| format!("target \"{}\": {}", target.name, staging.reveal(&e)))?;
        let paint = crate::style::Palette::stdout();
        println!(
            "{} {} {}",
            paint.green("bundled"),
            paint.dim("→"),
            staging.reveal(&written)
        );
    }

    // Every bundle exists before any of them runs. A prerender step renders the
    // pages of a site that the *browser* bundle is referenced from, so ordering
    // it after the whole build is the difference between generating HTML that
    // points at a file and HTML that points at one that does not exist yet.
    for target in selected.iter().filter(|target| target.run_after_build) {
        let output = staging.path(output_path(target));
        run_output(&project.project.dir, &output)
            .await
            .map_err(|e| format!("target \"{}\": {}", target.name, staging.reveal(&e)))?;
        let paint = crate::style::Palette::stdout();
        println!(
            "{} {} {}",
            paint.green("ran"),
            paint.dim("→"),
            paint.cyan(staging.reveal(&output.to_string_lossy()))
        );
    }
    Ok(())
}

/// Builds one entry — or one library — named on the command line.
///
/// Staged like a project build, for the same reason and with one difference:
/// what it writes is one file (plus whatever chunks that file's dynamic imports
/// produce) in a directory the build does not own, so the staged output is moved
/// *into* that directory rather than replacing it. `--lib` is the exception —
/// a library's output tree is the build's, and a stale module left in it is a
/// module the package publishes.
async fn build_single(mut config: BuildConfig) -> Result<String, String> {
    let root = match &config.root {
        Some(root) => root.clone(),
        None => {
            std::env::current_dir().map_err(|e| format!("cannot read working directory: {e}"))?
        }
    };
    let mut staging = crate::staging::Staging::new(&root, true)?;

    if config.lib {
        let out = PathBuf::from(config.out.clone().unwrap_or_else(|| "dist".to_string()));
        // Against the real directory: what the refusals are about is the
        // directory that is going to be replaced, not the one being written now.
        guard_replacement(&root.join(&out), &root.join(&config.source))?;
        staging.own(out.clone());
        config.out = Some(staging.path(&out).to_string_lossy().into_owned());
    } else if let Some(dir) = config.out_dir.clone() {
        config.out_dir = Some(staging.path(dir).to_string_lossy().into_owned());
    } else {
        let out = config
            .out
            .as_ref()
            .map_or_else(|| default_out(&config.source), PathBuf::from);
        config.out = Some(staging.path(out).to_string_lossy().into_owned());
    }

    let written = build(config)
        .await
        .map_err(|e| staging.reveal(&e))
        .map(|report| staging.reveal(&report))?;
    staging.commit()?;
    Ok(written)
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

    /// A directory of its own per test: these write and delete things.
    fn clean_fixture(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("esdev_clean_{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).expect("create src");
        std::fs::create_dir_all(dir.join("dist")).expect("create dist");
        std::fs::write(dir.join("src/index.ts"), "export const x: number = 1;").expect("write");
        dir
    }

    /// An ordinary output directory is allowed, and — the half that changed
    /// when staging arrived — is still there when the guard returns. Nothing is
    /// deleted until the staged build is ready to take its place.
    #[test]
    fn guarding_an_output_directory_deletes_nothing() {
        let dir = clean_fixture("ok");
        std::fs::write(dir.join("dist/stale.js"), "still here for now").expect("write");

        guard_replacement(&dir.join("dist"), &dir.join("src")).expect("guard");
        assert!(dir.join("dist/stale.js").exists(), "the guard deleted it");
        assert!(dir.join("src/index.ts").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Nothing there is not an error — the first build of a project has no
    /// output directory yet.
    #[test]
    fn guarding_a_directory_that_is_not_there_is_fine() {
        let dir = clean_fixture("absent");
        guard_replacement(&dir.join("nowhere"), &dir.join("src")).expect("guard");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The refusal that matters: `--out=src` differs from `--out=dist` by one
    /// keystroke, and would replace the library rather than build it.
    #[test]
    fn guarding_refuses_anything_holding_the_source() {
        let dir = clean_fixture("source");

        let onto_itself =
            guard_replacement(&dir.join("src"), &dir.join("src")).expect_err("refused");
        assert!(onto_itself.contains("holds the source"), "{onto_itself}");

        // …and the parent of the source, which takes it with everything else.
        let onto_parent = guard_replacement(&dir, &dir.join("src")).expect_err("refused");
        assert!(onto_parent.contains("holds the source"), "{onto_parent}");

        assert!(dir.join("src/index.ts").exists(), "the source was deleted");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn guarding_refuses_a_path_that_is_a_file() {
        let dir = clean_fixture("file");
        std::fs::write(dir.join("bundle.js"), "x").expect("write");

        let err = guard_replacement(&dir.join("bundle.js"), &dir.join("src")).expect_err("refused");
        assert!(err.contains("is a file"), "{err}");
        assert!(dir.join("bundle.js").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
