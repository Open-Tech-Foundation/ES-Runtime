//! `esdev.json` — what a project builds, in a file rather than on a command
//! line.
//!
//! Every knob in this tool has been a flag until now, and for a run that is
//! right: a flag is typed by a person, in view, once. A **build** is not that.
//! An application that renders on the server and hydrates in the browser is two
//! bundles from two entries with two different output shapes, and the moment
//! that has to be spelled out in `package.json` scripts it is spelled out
//! twice — once for the dev loop and once for the release — where the two
//! quietly drift apart. What a project builds is a property *of the project*,
//! so it belongs in the project.
//!
//! # Why JSON, and not `esdev.config.ts`
//!
//! Vite and Next both take an executable config, and both are right to: their
//! configs carry **plugins**, and a plugin is a function, which JSON cannot
//! hold. esdev has no plugin API, no resolver hooks and no transform pipeline
//! to configure, so an executable config here would be a program whose entire
//! content is data.
//!
//! There is also an ordering problem specific to this project. This file
//! carries `permissions`, and executing a config to learn what a run may do
//! means running guest code *before* that has been decided. Vite has no
//! capability model, so the question never arises for them; here it would be a
//! hole in the one property the runtime is built around. The day esdev grows a
//! hook that takes a function, this becomes a real question again — and the key
//! names below are chosen so a future `esdev.config.ts` can export the same
//! shape and leave every existing `esdev.json` valid.
//!
//! # `esrun` never reads this file
//!
//! Deliberately, and it is the line this design holds. A production binary that
//! picks up a checked-in file granting itself capabilities is precisely what the
//! capability model exists to prevent — the grant a service runs under must be
//! visible on the command that deployed it, not in a file that travelled with
//! the source. `permissions` here shapes the child that `esdev start` runs on a
//! developer's machine, which is how you develop *under* production's grants
//! without being able to ship them by accident.

use std::path::{Path, PathBuf};

use es_runtime_cli_common::args::try_permission_flag;
use es_runtime_cli_common::permissions::{Baseline, Permissions};
use serde_json::{Map, Value};

/// The file looked for when `--config` did not name one.
pub const FILE_NAME: &str = "esdev.json";
/// Where the dev loop writes when `start` does not name a directory.
///
/// A hidden sibling of the deploy outputs rather than one of them, so a save
/// never overwrites a deployment: `dist/server.js` is built for development
/// as `.dev/dist/server.js`. The same name the OTF toolchain uses for its own
/// dev-server working dir, so the two never collide.
pub const DEFAULT_DEV_DIR: &str = ".dev";

/// A parsed `esdev.json`.
#[derive(Debug)]
pub struct Project {
    /// The directory the file was found in.
    ///
    /// Every path in the file is relative to *this*, not to the working
    /// directory — so a config describes its own project the same way whether
    /// esdev was run from the project root or pointed at it from elsewhere.
    pub dir: PathBuf,
    /// How JSX compiles, for every command that compiles any.
    pub jsx: crate::transform::JsxSettings,
    /// The build targets, in name order.
    ///
    /// Sorted rather than left in the order they were written, because a JSON
    /// object has no order worth relying on. Where sequence actually matters —
    /// a target that runs after the build — it is expressed by the target
    /// itself rather than by its position in the file.
    pub targets: Vec<Target>,
    /// What `esdev start` does, if the file says.
    pub start: Start,
    /// The permission flags the dev loop's child runs under, **as flags**.
    ///
    /// Kept in the spelling a person would type rather than as a resolved
    /// capability set, because that is what they are: `esdev start` hands them
    /// to a child process, and what it hands over should be readable in `ps`
    /// and pasteable into a terminal. The translation happens once, here, and
    /// is checked by `esrun`'s own parser on the way through.
    pub permissions: Vec<String>,
    /// Specifier rewrites applied to every target's build: `find` → what it is
    /// replaced with, longest prefix first.
    ///
    /// **A bundling rule, and only a bundling rule.** `@/db` resolves because
    /// the bundler was told what `@` is; a module run *unbundled* — `esdev
    /// src/thing.ts`, or a file `esdev test` runs — resolves the way `esrun`
    /// does and knows nothing about it. That boundary is why the paths are
    /// resolved here, against the project rather than the working directory: an
    /// alias that means something different depending on where the build was
    /// started from would be worse than not having one.
    pub alias: Vec<(String, String)>,
    /// What `esdev test` does here, if the file says.
    pub test: TestSettings,
    /// Every plugin this project loads, in the order they are loaded — the
    /// top-level ones first, then each target's own.
    ///
    /// One flat list rather than a list per target because they are loaded
    /// **once**, into one isolate that lives for the run
    /// ([`crate::plugins`]); a target names the ones that apply to it by
    /// index. A dev loop rebuilds forty times a minute and a plugin is a
    /// module with a module's initialisation, so paying for it per build would
    /// be paying for it forty times.
    pub plugins: Vec<PluginSpec>,
}

/// One plugin, as the file names it.
#[derive(Clone, Debug, PartialEq)]
pub struct PluginSpec {
    /// The module to import — a path relative to the project, or a package.
    pub module: String,
    /// Which export the plugin is. `None` is the default export.
    pub export: Option<String>,
    /// What to call it with, when the export is a **factory**.
    ///
    /// A plugin that takes options is a function you call, and JSON cannot
    /// hold the call — so the file holds the argument instead and esdev makes
    /// the call. An export that is not a function is the plugin itself, and
    /// naming options for one is refused rather than ignored.
    pub options: Option<Value>,
}

/// What `esdev start` runs and watches.
#[derive(Debug, Default)]
pub struct Start {
    /// The target whose output is *the server* — run as a child process, and
    /// restarted when a rebuild finishes. Absent for a stack with no server of
    /// its own, where esdev serves the output directory itself.
    pub run: Option<String>,
    /// The targets to rebuild on a change. Empty means all of them, which is
    /// the useful default: a rebuild costs milliseconds, and a list that has
    /// fallen out of date is a save that appears to do nothing.
    pub watch: Vec<String>,
    /// The directory to serve when there is no `run` target. Defaults to the
    /// output of the one HTML target, since that is what a frontend-only stack
    /// has.
    pub serve: Option<String>,
    /// **The port you open.** For a project with a `run` target that is the
    /// application's own port; for a frontend project, where esdev serves the
    /// output itself, it is esdev's listener. Either way it is the address a
    /// developer types, which is why it has the plain name.
    pub port: Option<u16>,
    /// Where the dev loop writes its builds, as a directory in the project.
    /// `None` is [`DEFAULT_DEV_DIR`].
    ///
    /// Every target output is mirrored underneath it (`dist/server.js` becomes
    /// `.dev/dist/server.js`), so what a save rebuilds never touches what
    /// `esdev build` deploys. A directory of the loop's own, not a second
    /// spelling of an output: it may not overlap any target's `out` or
    /// `outdir`, and it stays inside the project.
    pub devdir: Option<String>,
}

impl Start {
    /// Where the dev loop writes: `devdir`, or [`DEFAULT_DEV_DIR`] when the
    /// file is silent.
    pub fn devdir(&self) -> &str {
        self.devdir.as_deref().unwrap_or(DEFAULT_DEV_DIR)
    }
}

/// What `esdev test` is configured to do in this project.
///
/// A section rather than flags-only for the reason the build has one: a
/// project's setup files and its per-test budget are properties *of the
/// project*, and a flag that has to be repeated in every `package.json` script
/// is one that will be repeated differently in two of them.
#[derive(Debug, Default)]
pub struct TestSettings {
    /// Modules imported before each test file runs — a polyfill, a global stub,
    /// a fixture registry. In the order written.
    pub setup: Vec<String>,
    /// Modules run once, in a process of their own, before any test file and
    /// torn down after the last. In the order written.
    pub global_setup: Vec<String>,
    /// How long a single file may take, in milliseconds. `None` is no limit.
    pub timeout: Option<u64>,
    /// How many files run at once. `None` is the machine's parallelism.
    pub jobs: Option<usize>,
    /// Whether files get their usual process boundary, or share one runtime.
    pub isolation: Option<TestIsolation>,
    /// `"human"` (the default) or `"json"`.
    pub reporter: Option<String>,
    /// Run the files in a real browser rather than this runtime, and which.
    pub browser: Option<crate::browser::Choice>,
    /// What `--coverage` measures and writes, and whether it is on without
    /// the flag.
    pub coverage: Option<CoverageSection>,
    /// How many concurrent cases in a file may run at once.
    pub max_concurrency: Option<u64>,
    /// The tags tests may carry, and the options each gives them.
    pub tags: Vec<TagDefinition>,
    /// Whether a test naming a tag not defined here is an error. `None` is
    /// on: a misspelt tag is otherwise a test no filter ever selects.
    pub strict_tags: Option<bool>,
}

/// One entry of `test.tags`.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct TagDefinition {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repeats: Option<u64>,
    /// Which tag's options win when two set the same one: the lower number.
    /// Tags without one give way to those with.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<f64>,
}

/// `test.coverage`.
#[derive(Debug, Clone, PartialEq)]
pub struct CoverageSection {
    /// `"enabled": true` collects coverage on every run, flag or not.
    pub enabled: bool,
    pub settings: crate::coverage::Settings,
}

/// The boundary between files selected by `esdev test`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TestIsolation {
    /// One child process — and therefore one V8 isolate — per file.
    Process,
    /// All selected files share the runner process and its module cache.
    None,
}

/// One thing a project builds.
#[derive(Debug)]
pub struct Target {
    /// The key this target was written under, and the name `--target=` selects
    /// it by.
    pub name: String,
    /// The module the bundle is rooted at, or the HTML file that names it.
    pub entry: String,
    /// Where the output goes.
    pub output: Output,
    /// Which environment the output runs in.
    pub platform: Platform,
    /// Files and directories copied into the output directory verbatim.
    pub assets: Vec<String>,
    /// Whether to minify this target.
    pub minify: bool,
    /// Whether this target's build writes source maps, and in which shape:
    /// `"external"` (a `.map` beside the output), `"inline"` (a data URL in the
    /// file itself) or `"hidden"` (written, and not pointed at).
    ///
    /// `None` is "the build decides", which for a release build is none and for
    /// the dev loop is inline ([`crate::build::sourcemap_for`]).
    pub sourcemap: Option<String>,
    /// `"refresh": "<scheme>"` — the hot-reload scheme this target's modules
    /// should be prepared for, applied in the dev loop only.
    ///
    /// A name rather than a boolean because the schemes are not one thing:
    /// React's registers components and matches hook signatures, and another
    /// framework's does something else entirely. **esdev implements none of
    /// them.** It provides the generic half — `import.meta.hot`, the update
    /// channel, and the compiler's component registrations on request — and the
    /// name is what a plugin reads (as `ctx.refresh`) to know which scheme it is
    /// installing, and that this build is the hot one.
    pub refresh: Option<String>,
    /// `"lib": true` — this target publishes a library rather than deploying an
    /// application, exactly as `esdev build --lib` does.
    ///
    /// It is a key on a target and not only a flag because everything else
    /// about a build lives in this file, and a library that had to be described
    /// on a command line could describe only *part* of itself: `assets` — the
    /// README and LICENSE a package ships — is a target key, so a library built
    /// from flags alone could not copy them.
    pub lib: bool,
    /// `"format"` — the module systems a `--lib` target writes: `"esm"`
    /// (the default), `"cjs"`, or both. A string or a list.
    pub formats: Vec<String>,
    /// `"types": false` — skip the declarations, as `--no-types` does.
    /// Meaningless off a library, and refused there.
    pub types: bool,
    /// `"dts-bundle"` — link every declaration into one file, as
    /// `--dts-bundle` does. `true` takes this target's `index.ts`; a string
    /// names the entry.
    ///
    /// Resolved to a path at parse time, so a `true` with no `index.ts` beside
    /// it is a config error naming what was looked for rather than a build that
    /// fails later.
    pub dts_bundle: Option<String>,
    /// Compile-time replacements, as `--define` makes them.
    pub define: Vec<(String, String)>,
    /// Extra `exports` conditions, as `--conditions` adds them.
    pub conditions: Vec<String>,
    /// The plugins that apply to this target, as indices into
    /// [`Project::plugins`] — the project's own first, then this target's.
    ///
    /// Indices rather than the specs themselves because a plugin is *loaded*
    /// once and used by however many targets name it: two targets that both
    /// take the project's `plugins` share the one instance, and the module
    /// they came from is evaluated once.
    pub plugins: Vec<usize>,
    /// Whether the built output is *executed* once the build finishes.
    ///
    /// This is how a static site gets generated without esdev knowing what a
    /// static site is: the bundle runs, and what it writes is the build's real
    /// output. Bundling and prerendering are the same step to everything
    /// downstream, which is what keeps `esdev build` a single command for a
    /// stack whose deliverable is a directory of HTML.
    pub run_after_build: bool,
}

impl Target {
    /// Whether this target's entry is a document rather than a module.
    ///
    /// A server bundle starts at a module, because the runtime does. The
    /// browser starts at a **document** — the module is something the document
    /// references — so an HTML entry is a different kind of build, not a
    /// different setting on the same one ([`crate::html`]).
    pub fn is_html(&self) -> bool {
        is_html_entry(&self.entry)
    }
}

/// Whether an entry names a document rather than a module.
pub(crate) fn is_html_entry(entry: &str) -> bool {
    Path::new(entry)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("html") || e.eq_ignore_ascii_case("htm"))
}

/// What a target's output looks like on disk.
#[derive(Debug)]
pub enum Output {
    /// `out` — one file. The directory it lands in may hold other things and is
    /// never cleaned, because the build does not own it.
    File(String),
    /// `outdir` — a directory this target writes into.
    ///
    /// What a browser target needs: a dynamic `import()` emits a hashed chunk
    /// beside its entry, and a build whose output is one named file has nowhere
    /// to put a second one.
    Dir(String),
}

/// Which environment a target's output runs in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    /// This runtime — the default, and what every target was before there was
    /// a browser one.
    Server,
    /// A browser. Changes which build of a dependency is inlined: the `browser`
    /// condition rather than `worker`.
    Browser,
}

/// The keys a target may carry.
const TARGET_KEYS: &[&str] = &[
    "entry",
    "plugins",
    "out",
    "outdir",
    "platform",
    "assets",
    "minify",
    "sourcemap",
    "define",
    "conditions",
    "then",
    "refresh",
    "lib",
    "format",
    "types",
    "dts-bundle",
];

/// The keys the file may carry at the top level.
const TOP_LEVEL_KEYS: &[&str] = &[
    "$schema",
    "targets",
    "start",
    "permissions",
    "plugins",
    "alias",
    "test",
    "jsx",
];

/// The keys `start` may carry.
///
/// Read in full here, and *consumed* by `esdev start`. Validating a key the
/// command that uses it has not been written yet is deliberate: a typo in
/// `start` should be reported by the build that read the file, not held until
/// the day somebody runs the other command.
const START_KEYS: &[&str] = &["run", "watch", "serve", "port", "devdir"];

/// The keys `test` may carry.
const TEST_KEYS: &[&str] = &[
    "setup",
    "globalSetup",
    "timeout",
    "jobs",
    "isolation",
    "reporter",
    "browser",
    "coverage",
    "tags",
    "strictTags",
    "maxConcurrency",
];

/// The keys a `test.tags` entry may carry.
const TAG_KEYS: &[&str] = &[
    "name",
    "description",
    "timeout",
    "retry",
    "repeats",
    "priority",
];

/// The keys `test.coverage` may carry.
const COVERAGE_KEYS: &[&str] = &[
    "enabled",
    "include",
    "exclude",
    "reporter",
    "reportsDirectory",
    "thresholds",
];

/// The keys `test.coverage.thresholds` may carry.
const THRESHOLD_KEYS: &[&str] = &["lines", "functions", "branches", "statements"];

/// The keys `jsx` may carry.
const JSX_KEYS: &[&str] = &["importSource", "factory", "fragment", "development"];

/// Loads the project config: the one `--config` named, or `./esdev.json`.
///
/// `Ok(None)` means there is no config and none was asked for — the ordinary
/// state of a project that names its entry on the command line. A `--config`
/// that names a file which is not there is an error, never a silent fallback:
/// building something other than what was pointed at is worse than not
/// building.
pub fn load(named: Option<&str>) -> Result<Option<Project>, String> {
    let path = match named {
        Some(path) => PathBuf::from(path),
        None => {
            let default = PathBuf::from(FILE_NAME);
            if !default.is_file() {
                return Ok(None);
            }
            default
        }
    };
    if !path.is_file() {
        return Err(format!(
            "cannot read {}\n\n\
             --config names the file to read; drop it to use ./{FILE_NAME}.",
            path.display()
        ));
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    // Absolute from here on. Every path in the file is resolved against this
    // directory, including the bundler's own working directory, and a relative
    // one would be resolved a second time against wherever the process happens
    // to be — which is the same place only by coincidence. Stripped of the
    // Windows verbatim prefix: this directory becomes bundler inputs and
    // module URLs, which `\\?\` breaks.
    let dir =
        dunce::canonicalize(&dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
    parse(&text, dir, &path.display().to_string())
}

/// Why a directory looks like an OTF Web project: one the OTF toolchain
/// (`otfw`) builds, not esdev.
///
/// `esdev create` scaffolds these alongside its own templates, and nothing
/// else about them is esdev's — no `esdev.json`, no targets. `esdev build`
/// and `esdev start` refuse with where they belong rather than the
/// missing-file errors, which read as if something were misconfigured rather
/// than a different toolchain in use. A malformed or absent `package.json`
/// is not evidence either way, so this says nothing then.
pub(crate) fn otfw_reason(dir: &Path) -> Option<String> {
    if dir.join("otfw.config.js").is_file() {
        return Some("it carries an `otfw.config.js`".to_string());
    }
    let package = std::fs::read_to_string(dir.join("package.json")).ok()?;
    let package: Value = serde_json::from_str(&package).ok()?;
    let scripts = package.get("scripts")?.as_object()?;
    let mut names: Vec<&str> = scripts
        .iter()
        .filter(|(_, cmd)| {
            cmd.as_str()
                .is_some_and(|cmd| cmd == "otfw" || cmd.starts_with("otfw "))
        })
        .map(|(name, _)| name.as_str())
        .collect();
    if names.is_empty() {
        return None;
    }
    names.sort();
    let scripts = match &names[..] {
        [one] => format!("its \"{one}\" script calls `otfw`"),
        [head @ .., last] => {
            let head: Vec<String> = head.iter().map(|n| format!("\"{n}\"")).collect();
            format!("its {} and \"{last}\" scripts call `otfw`", head.join(", "))
        }
        [] => unreachable!("names is not empty"),
    };
    Some(scripts)
}

/// Parses the text of an `esdev.json`.
///
/// Split from [`load`] so the whole grammar is testable without a filesystem,
/// which is what keeps the error messages under test rather than under review.
pub fn parse(text: &str, dir: PathBuf, name: &str) -> Result<Option<Project>, String> {
    let root: Value = serde_json::from_str(text).map_err(|e| {
        format!(
            "{name} is not valid JSON: {e}\n\n\
             It is read as data — there are no comments, no trailing commas and \
             nothing is executed."
        )
    })?;
    let root = object(&root, name, "the file")?;
    known_keys(root, name, "", TOP_LEVEL_KEYS)?;

    // `targets` is optional, because a config is no longer only about building:
    // a project that is tested and never bundled still has a `test` section and
    // a `jsx` one, and making it invent a target to say so would be a worse file
    // than no file. What is refused is a config that says *nothing*.
    // The sections that mean something without a build: how the project is
    // tested, how its JSX compiles, what its specifiers resolve to.
    const BUILDLESS_KEYS: &[&str] = &["test", "jsx", "alias", "plugins", "permissions"];
    let targets = match root.get("targets") {
        Some(targets) => object(targets, name, "`targets`")?.clone(),
        None => {
            // `start` builds and serves the targets, so a file that names one
            // without them is still incomplete.
            if !root.contains_key("start")
                && root
                    .keys()
                    .any(|key| BUILDLESS_KEYS.contains(&key.as_str()))
            {
                serde_json::Map::new()
            } else {
                return Err(format!(
                    "{name} has no `targets`.\n\n\
                     A target is one thing the project builds — an entry, and where its \
                     output goes:\n\n  \
                     \"targets\": {{ \"server\": {{ \"entry\": \"src/server.ts\", \"out\": \"dist/server.js\" }} }}\n\n\
                     A config that only says how the project is tested or how its JSX \
                     compiles — `test`, `jsx`, `alias`, `plugins` — needs no targets."
                ));
            }
        }
    };
    if root.contains_key("targets") && targets.is_empty() {
        return Err(format!(
            "{name} has no targets in `targets`.\n\n\
             An empty object builds nothing; remove the key, or name what it builds."
        ));
    }
    let targets = &targets;
    // The project's own plugins load first, and every target gets them. A
    // target's list adds to that rather than replacing it: a project that
    // compiles `.mdx` compiles it for the server bundle and the browser one,
    // and a config where naming one extra plugin silently dropped the shared
    // ones would be a build that differs between targets for no stated reason.
    let mut plugins = plugin_specs(root.get("plugins"), name, "`plugins`")?;
    let shared: Vec<usize> = (0..plugins.len()).collect();

    let mut targets = targets
        .iter()
        .map(|(target_name, value)| target(target_name, value, name, &dir, &shared, &mut plugins))
        .collect::<Result<Vec<_>, _>>()?;
    // Sorted here rather than taken as they came. `serde_json` keeps insertion
    // order only when a feature enables it, and that feature is currently on
    // because *something else* in this workspace asked for it — an order that
    // would change under a dependency edit nobody connected to this file.
    targets.sort_by(|a, b| a.name.cmp(&b.name));

    let start = match root.get("start") {
        Some(start) => read_start(start, &targets, name)?,
        None => Start::default(),
    };
    let permissions = match root.get("permissions") {
        Some(permissions) => permission_flags(permissions, name)?,
        None => Vec::new(),
    };
    let alias = aliases(root.get("alias"), name, &dir)?;
    let test = read_test(root.get("test"), name)?;
    let jsx = read_jsx(root.get("jsx"), name)?;
    Ok(Some(Project {
        dir,
        targets,
        start,
        permissions,
        plugins,
        alias,
        test,
        jsx,
    }))
}

/// Parses the `jsx` section.
///
/// There is no mode to name. `importSource` means the compiler writes the
/// import; `factory` means it calls what the module imports itself. The shape of
/// the section is the answer, so nothing here is spelled in another framework's
/// vocabulary — and naming both is a question with two answers.
fn read_jsx(value: Option<&Value>, file: &str) -> Result<crate::transform::JsxSettings, String> {
    let Some(value) = value else {
        return Ok(crate::transform::JsxSettings::default());
    };
    let map = object(value, file, "`jsx`")?;
    // `runtime` is what every React-descended toolchain calls this, so it is
    // worth a sentence rather than a list of the keys that do exist.
    if map.contains_key("runtime") {
        return Err(format!(
            "{file}: `jsx.runtime` is not a key here.\n\n\
             Name an `importSource` and the compiler writes the import, or a \
             `factory` and it calls what the module imports itself:\n\n  \
             \"jsx\": {{ \"importSource\": \"preact\" }}\n  \
             \"jsx\": {{ \"factory\": \"h\", \"fragment\": \"Fragment\" }}"
        ));
    }
    known_keys(map, file, "`jsx`", JSX_KEYS)?;

    let text = |key: &str| -> Result<Option<String>, String> {
        match map.get(key) {
            None => Ok(None),
            Some(found) => {
                let value = found
                    .as_str()
                    .ok_or_else(|| format!("{file}: `jsx.{key}` must be a string."))?;
                if value.is_empty() {
                    return Err(format!("{file}: `jsx.{key}` cannot be empty."));
                }
                Ok(Some(value.to_string()))
            }
        }
    };
    let import_source = text("importSource")?;
    let factory = text("factory")?;
    let fragment = text("fragment")?;
    let development = match map.get("development") {
        None => false,
        Some(found) => found
            .as_bool()
            .ok_or_else(|| format!("{file}: `jsx.development` must be true or false."))?,
    };

    let function = match (import_source, factory) {
        (Some(source), None) => {
            if fragment.is_some() {
                return Err(format!(
                    "{file}: `jsx.fragment` goes with `jsx.factory`.\n\n\
                     An imported runtime brings its own fragment, so naming one \
                     here would name something the compiler never calls."
                ));
            }
            Some(crate::transform::JsxFunction::Imported { source })
        }
        (None, Some(factory)) => Some(crate::transform::JsxFunction::InScope { factory, fragment }),
        (Some(_), Some(_)) => {
            return Err(format!(
                "{file}: `jsx` names an `importSource` and a `factory`, and those are \
                 two ways to reach the same function.\n\n\
                 Keep one: an `importSource` has the compiler write the import, a \
                 `factory` has it call what the module imports itself."
            ));
        }
        (None, None) => {
            if fragment.is_some() {
                return Err(format!("{file}: `jsx.fragment` goes with `jsx.factory`."));
            }
            return Err(format!(
                "{file}: `jsx` says nothing.\n\n\
                 Name an `importSource` and the compiler writes the import, or a \
                 `factory` and it calls what the module imports itself:\n\n  \
                 \"jsx\": {{ \"importSource\": \"preact\" }}\n  \
                 \"jsx\": {{ \"factory\": \"h\", \"fragment\": \"Fragment\" }}"
            ));
        }
    };
    Ok(crate::transform::JsxSettings {
        function,
        development,
    })
}

/// Parses the `test` section.
///
/// Every key here has a flag of the same name on `esdev test`, and the flag
/// wins — the same rule the build uses, so a project whose day to day is four
/// jobs can still be run one at a time to watch a hang.
fn read_test(value: Option<&Value>, file: &str) -> Result<TestSettings, String> {
    let Some(value) = value else {
        return Ok(TestSettings::default());
    };
    let map = object(value, file, "`test`")?;
    known_keys(map, file, "`test`", TEST_KEYS)?;

    let setup = match map.get("setup") {
        // A single setup file is the common case and reads better as one.
        Some(Value::String(one)) => vec![one.clone()],
        other => string_array(other, file, "`test`'s `setup`")?,
    };
    let global_setup = match map.get("globalSetup") {
        Some(Value::String(one)) => vec![one.clone()],
        other => string_array(other, file, "`test`'s `globalSetup`")?,
    };
    let timeout = match map.get("timeout") {
        None => None,
        Some(Value::Number(ms)) if ms.as_u64().is_some_and(|ms| ms > 0) => ms.as_u64(),
        Some(other) => {
            return Err(format!(
                "{file}: `test`'s `timeout` is {}, and it is how many milliseconds one \
                 file may take.\n\n\
                 A whole number above zero: \"timeout\": 5000.",
                kind(other)
            ));
        }
    };
    let jobs = match map.get("jobs") {
        None => None,
        Some(Value::Number(n)) if n.as_u64().is_some_and(|n| n > 0) => {
            n.as_u64().and_then(|n| usize::try_from(n).ok())
        }
        Some(other) => {
            return Err(format!(
                "{file}: `test`'s `jobs` is {}, and it is how many files run at \
                 once.\n\n\
                 One or more; 1 runs them one at a time and lets each write straight \
                 to the terminal.",
                kind(other)
            ));
        }
    };
    let isolation = match map.get("isolation") {
        None => None,
        Some(Value::String(name)) if name == "process" => Some(TestIsolation::Process),
        Some(Value::String(name)) if name == "none" => Some(TestIsolation::None),
        Some(other) => {
            return Err(format!(
                "{file}: `test`'s `isolation` is {}, and it says whether files share a runtime.\n\n  \
                 \"process\"  — one process per file, the default\n  \
                 \"none\"     — all files share one process and module cache",
                kind(other)
            ));
        }
    };
    let reporter = match map.get("reporter") {
        None => None,
        Some(Value::String(name))
            if crate::report::REPORTERS
                .iter()
                .any(|(known, _)| *known == name.as_str()) =>
        {
            Some(name.clone())
        }
        Some(other) => {
            let known: String = crate::report::REPORTERS
                .iter()
                .map(|(known, what)| {
                    format!(
                        "\n  \"{known}\"{:<pad$} — {what}",
                        "",
                        pad = 6 - known.len()
                    )
                })
                .collect();
            return Err(format!(
                "{file}: `test`'s `reporter` is {}, and it says how the run reports \
                 itself.\n{known}",
                kind(other)
            ));
        }
    };
    let browser = match map.get("browser") {
        None => None,
        Some(Value::String(name)) => Some(
            crate::browser::Choice::parse(name)
                .map_err(|err| format!("{file}: `test`'s `browser`: {err}"))?,
        ),
        // Several: every file runs in each.
        Some(Value::Array(names)) if names.iter().all(Value::is_string) => Some(
            crate::browser::Choice::list(names.iter().filter_map(Value::as_str))
                .map_err(|err| format!("{file}: `test`'s `browser`: {err}"))?,
        ),
        Some(other) => {
            return Err(format!(
                "{file}: `test`'s `browser` is {}, and it names the browser the test \
                 files run in.\n\n\
                 \"auto\" for the first available, or one of \"chrome\", \"chromium\", \
                 \"firefox\", \"edge\": \"browser\": \"auto\".",
                kind(other)
            ));
        }
    };
    let coverage = map
        .get("coverage")
        .map(|value| read_coverage(value, file))
        .transpose()?;
    let tags = match map.get("tags") {
        None => Vec::new(),
        Some(Value::Array(entries)) => entries
            .iter()
            .map(|entry| read_tag(entry, file))
            .collect::<Result<_, _>>()?,
        Some(other) => {
            return Err(format!(
                "{file}: `test`'s `tags` is {}, and it lists the tags tests may carry.\n\n\
                 An array: \"tags\": [{{ \"name\": \"db\", \"timeout\": 60000 }}].",
                kind(other)
            ));
        }
    };
    let strict_tags = match map.get("strictTags") {
        None => None,
        Some(Value::Bool(on)) => Some(*on),
        Some(other) => {
            return Err(format!(
                "{file}: `test`'s `strictTags` is {}, and it says whether an undefined tag is an error.\n\n\
                 true (the default) or false.",
                kind(other)
            ));
        }
    };
    let max_concurrency = match map.get("maxConcurrency") {
        None => None,
        Some(Value::Number(n)) if n.as_u64().is_some_and(|n| n > 0) => n.as_u64(),
        Some(other) => {
            return Err(format!(
                "{file}: `test`'s `maxConcurrency` is {}, and it is how many concurrent tests \
                 in a file run at once.\n\n\
                 A whole number above zero: \"maxConcurrency\": 10.",
                kind(other)
            ));
        }
    };
    Ok(TestSettings {
        max_concurrency,
        tags,
        strict_tags,
        coverage,
        setup,
        global_setup,
        timeout,
        jobs,
        isolation,
        reporter,
        browser,
    })
}

/// Parses one entry of `test.tags`.
fn read_tag(value: &Value, file: &str) -> Result<TagDefinition, String> {
    let map = object(value, file, "a `tags` entry")?;
    known_keys(map, file, "a `tags` entry", TAG_KEYS)?;
    let name = match map.get("name") {
        Some(Value::String(name)) => name.clone(),
        _ => return Err(format!("{file}: a `tags` entry needs a \"name\".")),
    };
    crate::tags::check_name(&name).map_err(|err| format!("{file}: {err}."))?;
    let count = |key: &str| -> Result<Option<u64>, String> {
        match map.get(key) {
            None => Ok(None),
            Some(Value::Number(n)) if n.as_u64().is_some() => Ok(n.as_u64()),
            Some(other) => Err(format!(
                "{file}: tag `{name}`'s `{key}` is {}, and should be a whole number.",
                kind(other)
            )),
        }
    };
    let timeout = count("timeout")?;
    let retry = count("retry")?;
    let repeats = count("repeats")?;
    if timeout == Some(0) {
        return Err(format!(
            "{file}: tag `{name}`'s `timeout` is milliseconds above zero."
        ));
    }
    let description = match map.get("description") {
        None => None,
        Some(Value::String(text)) => Some(text.clone()),
        Some(other) => {
            return Err(format!(
                "{file}: tag `{name}`'s `description` is {}, and should be a string.",
                kind(other)
            ));
        }
    };
    let priority = match map.get("priority") {
        None => None,
        Some(Value::Number(n)) => n.as_f64(),
        Some(other) => {
            return Err(format!(
                "{file}: tag `{name}`'s `priority` is {}, and should be a number.",
                kind(other)
            ));
        }
    };
    Ok(TagDefinition {
        name,
        description,
        timeout,
        retry,
        repeats,
        priority,
    })
}

/// Parses `test.coverage`.
fn read_coverage(value: &Value, file: &str) -> Result<CoverageSection, String> {
    let map = object(value, file, "`test`'s `coverage`")?;
    known_keys(map, file, "`test`'s `coverage`", COVERAGE_KEYS)?;
    let mut settings = crate::coverage::Settings::default();
    let enabled = match map.get("enabled") {
        None => false,
        Some(Value::Bool(on)) => *on,
        Some(other) => {
            return Err(format!(
                "{file}: `coverage`'s `enabled` is {}, and it says whether every run collects coverage.\n\n\
                 true or false.",
                kind(other)
            ));
        }
    };
    settings.include = string_array(map.get("include"), file, "`coverage`'s `include`")?;
    settings.exclude = string_array(map.get("exclude"), file, "`coverage`'s `exclude`")?;
    if let Some(reporters) = map.get("reporter") {
        settings.reporters = match reporters {
            Value::String(one) => vec![one.clone()],
            other => string_array(Some(other), file, "`coverage`'s `reporter`")?,
        };
        if let Some(unknown) = settings
            .reporters
            .iter()
            .find(|name| !crate::coverage::REPORTERS.contains(&name.as_str()))
        {
            return Err(format!(
                "{file}: `{unknown}` is not a coverage reporter.\n\n\
                 One of: {}.",
                crate::coverage::REPORTERS.join(", ")
            ));
        }
    }
    match map.get("reportsDirectory") {
        None => {}
        Some(Value::String(dir)) if !dir.is_empty() => settings.directory.clone_from(dir),
        Some(other) => {
            return Err(format!(
                "{file}: `coverage`'s `reportsDirectory` is {}, and it names where the reports go.\n\n\
                 A path in the project: \"reportsDirectory\": \"coverage\".",
                kind(other)
            ));
        }
    }
    if let Some(thresholds) = map.get("thresholds") {
        let limits = object(thresholds, file, "`coverage`'s `thresholds`")?;
        known_keys(limits, file, "`coverage`'s `thresholds`", THRESHOLD_KEYS)?;
        let read = |name: &str| -> Result<Option<f64>, String> {
            match limits.get(name) {
                None => Ok(None),
                Some(Value::Number(n)) if n.as_f64().is_some_and(|n| n <= 100.0) => Ok(n.as_f64()),
                Some(other) => Err(format!(
                    "{file}: `thresholds`' `{name}` is {}, and it is the least coverage a run may have.\n\n\
                     A percentage up to 100, or a negative number for how many may be uncovered: \
                     \"{name}\": 80, or \"{name}\": -10.",
                    kind(other)
                )),
            }
        };
        settings.thresholds = crate::coverage::Thresholds {
            lines: read("lines")?,
            functions: read("functions")?,
            branches: read("branches")?,
            statements: read("statements")?,
        };
    }
    Ok(CoverageSection { enabled, settings })
}

/// Parses one entry of `targets`.
fn target(
    name: &str,
    value: &Value,
    file: &str,
    dir: &Path,
    shared: &[usize],
    plugins: &mut Vec<PluginSpec>,
) -> Result<Target, String> {
    let at = format!("target \"{name}\"");
    if name.trim().is_empty() || name.chars().any(char::is_whitespace) {
        return Err(format!(
            "{file}: \"{name}\" is not a usable target name.\n\n\
             A name is what `esdev build --target=<name>` selects, so it cannot be \
             blank or carry spaces."
        ));
    }
    let map = object(value, file, &at)?;
    known_keys(map, file, &at, TARGET_KEYS)?;

    let entry = match map.get("entry") {
        Some(entry) => string(entry, file, &format!("{at}'s `entry`"))?.to_string(),
        None => {
            return Err(format!(
                "{file}: {at} has no `entry`.\n\n\
                 Every target is rooted at one: \"entry\": \"src/server.ts\"."
            ));
        }
    };

    let output = match (map.get("out"), map.get("outdir")) {
        (Some(_), Some(_)) => {
            return Err(format!(
                "{file}: {at} sets both `out` and `outdir`, which name different \
                 shapes of output.\n\n\
                 `out` is one file; `outdir` is a directory the target writes into. \
                 A browser target wants `outdir` — a dynamic import emits a chunk \
                 beside its entry, and one named file has nowhere to put it."
            ));
        }
        (Some(out), None) => {
            let out = string(out, file, &format!("{at}'s `out`"))?;
            if Path::new(out).extension().is_none() {
                return Err(format!(
                    "{file}: {at} has \"out\": \"{out}\", which names a directory.\n\n\
                     `out` is one file (\"dist/server.js\"). For a directory, write \
                     \"outdir\": \"{out}\"."
                ));
            }
            Output::File(out.to_string())
        }
        (None, Some(dir)) => {
            let dir = string(dir, file, &format!("{at}'s `outdir`"))?;
            if Path::new(dir).extension().is_some() {
                return Err(format!(
                    "{file}: {at} has \"outdir\": \"{dir}\", which names a file.\n\n\
                     `outdir` is a directory the target writes into. For one file, \
                     write \"out\": \"{dir}\"."
                ));
            }
            Output::Dir(dir.to_string())
        }
        // A document's output is a directory whichever way you look at it —
        // the file itself, the bundles its scripts became, the chunks those
        // split into and the stylesheets beside them.
        (None, None) if is_html_entry(&entry) => Output::Dir("dist".to_string()),
        (None, None) => Output::File(default_out(&entry)),
    };

    let platform = match map.get("platform") {
        None => Platform::Server,
        Some(value) => match string(value, file, &format!("{at}'s `platform`"))? {
            "server" => Platform::Server,
            "browser" => Platform::Browser,
            other => {
                return Err(format!(
                    "{file}: {at} has \"platform\": \"{other}\".\n\n\
                     It is \"server\" (this runtime, the default) or \"browser\" — \
                     which decides whether a dependency hands over its `worker` build \
                     or its `browser` one."
                ));
            }
        },
    };

    let run_after_build = match map.get("then") {
        None => false,
        Some(value) => match string(value, file, &format!("{at}'s `then`"))? {
            "run" => true,
            other => {
                return Err(format!(
                    "{file}: {at} has \"then\": \"{other}\".\n\n\
                     The only thing a build can do next is \"run\" the output it just \
                     wrote — which is how a prerender step emits a directory of HTML."
                ));
            }
        },
    };
    if run_after_build && platform == Platform::Browser {
        return Err(format!(
            "{file}: {at} is a browser target with \"then\": \"run\".\n\n\
             A browser bundle is served, not executed here — it expects a `document` \
             this runtime does not have. A prerender step is a server target that \
             *writes* the HTML."
        ));
    }

    // Resolved before the target is built, because two of its fields depend on
    // it: which plugins apply, and whether a `refresh` scheme esdev does not
    // implement has anything that could.
    let mut mine = shared.to_vec();
    for spec in plugin_specs(map.get("plugins"), file, &format!("{at}'s `plugins`"))? {
        mine.push(plugins.len());
        plugins.push(spec);
    }

    let lib = flag(map.get("lib"), file, &format!("{at}'s `lib`"))?;
    let formats = library_formats(map.get("format"), file, &at)?;
    let types = match map.get("types") {
        None => true,
        Some(value) => flag(Some(value), file, &format!("{at}'s `types`"))?,
    };

    // The refusals come **before** `dts-bundle` is resolved. Resolving it means
    // looking for an index under the entry, and on a target whose entry is a
    // module rather than a directory that reports a missing
    // `src/app.ts/index.ts` — an answer to a question the reader never asked.
    // What they need to hear is that the key needs `"lib": true`.
    //
    // The same refusals `esdev build` makes for the same flags, in the same
    // words. A key that means nothing here is a belief about the build that is
    // wrong, and the two surfaces disagreeing about which is which is how a
    // project ends up built one way from the file and another from a flag.
    if !lib {
        if !formats.is_empty() {
            return Err(format!(
                "{file}: {at} has `format` without \"lib\": true.\n\n\
                 An application build's output is loaded by esrun, which loads ES \
                 modules and nothing else (D22). A library is an input to somebody \
                 else's build, and that build may still be a CommonJS one."
            ));
        }
        if map.contains_key("types") {
            return Err(format!(
                "{file}: {at} has `types` without \"lib\": true.\n\n\
                 An application build emits no declarations to skip: a bundle is \
                 deployed and run, not imported and type-checked."
            ));
        }
        if map.contains_key("dts-bundle") {
            return Err(format!(
                "{file}: {at} has `dts-bundle` without \"lib\": true.\n\n\
                 An application build emits no declarations to link."
            ));
        }
    }
    let dts_bundle = dts_bundle(map.get("dts-bundle"), file, &at, dir, &entry)?;
    if lib {
        if !types && dts_bundle.is_some() {
            return Err(format!(
                "{file}: {at} sets `dts-bundle` and \"types\": false, which ask for \
                 opposite things.\n\n\
                 One links every declaration into a file; the other emits none."
            ));
        }
        if is_html_entry(&entry) {
            return Err(format!(
                "{file}: {at} is a library whose `entry` is a document.\n\n\
                 A library's entry is the **source directory** its modules are \
                 under — \"entry\": \"src\" — because a published tree has no one \
                 root: which module a consumer imports is its `exports` map's \
                 decision, not this build's."
            ));
        }
        if let Output::File(out) = &output {
            return Err(format!(
                "{file}: {at} is a library with \"out\": \"{out}\", which names one \
                 file.\n\n\
                 A library keeps its module structure, so its output is a \
                 directory: \"outdir\": \"dist\"."
            ));
        }
        if run_after_build {
            return Err(format!(
                "{file}: {at} is a library with \"then\": \"run\".\n\n\
                 A library is imported by somebody else's build, not executed by \
                 this one."
            ));
        }
    }

    let built = Target {
        name: name.to_string(),
        entry,
        output,
        platform,
        assets: string_array(map.get("assets"), file, &format!("{at}'s `assets`"))?,
        minify: flag(map.get("minify"), file, &format!("{at}'s `minify`"))?,
        sourcemap: sourcemap(map.get("sourcemap"), file, &at)?,
        define: defines(map.get("define"), file, &at)?,
        conditions: string_array(map.get("conditions"), file, &format!("{at}'s `conditions`"))?,
        refresh: refresh(map.get("refresh"), file, &at, !mine.is_empty())?,
        plugins: mine,
        run_after_build,
        lib,
        formats,
        types,
        dts_bundle,
    };

    // An HTML target's shape is decided by the document, so the keys that would
    // decide it here are refused rather than quietly ignored. Each of these is
    // a reasonable thing to write and a wrong thing to believe.
    if built.is_html() {
        if map.contains_key("out") {
            return Err(format!(
                "{file}: {at} builds an HTML file, and `out` names one output.\n\n\
                 A document is a bundle, its chunks, its stylesheets and itself — \
                 write \"outdir\": \"dist\"."
            ));
        }
        if map.contains_key("platform") {
            return Err(format!(
                "{file}: {at} builds an HTML file, and sets `platform`.\n\n\
                 What a document's scripts are built for is not in question: they \
                 run in a browser."
            ));
        }
        if built.run_after_build {
            return Err(format!(
                "{file}: {at} builds an HTML file with \"then\": \"run\".\n\n\
                 There is nothing to execute — the output is a document and the \
                 files it references."
            ));
        }
    }
    Ok(built)
}

/// Where a target's output goes when it did not say — the same default the
/// command line has always had, so a config that omits `out` and a command line
/// that omits `--out` write the same file.
fn default_out(entry: &str) -> String {
    let stem = Path::new(entry)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("bundle");
    format!("dist/{stem}.js")
}

/// Reads `start`, checking the target names it refers to.
fn read_start(value: &Value, targets: &[Target], file: &str) -> Result<Start, String> {
    let map = object(value, file, "`start`")?;
    known_keys(map, file, "`start`", START_KEYS)?;
    let names: Vec<&str> = targets.iter().map(|t| t.name.as_str()).collect();

    let run = match map.get("run") {
        None => None,
        Some(run) => {
            let run = string(run, file, "`start`'s `run`")?;
            if !names.contains(&run) {
                return Err(unknown_target(file, "`start`'s `run`", run, &names));
            }
            Some(run.to_string())
        }
    };
    let watch = string_array(map.get("watch"), file, "`start`'s `watch`")?;
    for name in &watch {
        if !names.contains(&name.as_str()) {
            return Err(unknown_target(file, "`start`'s `watch`", name, &names));
        }
    }
    let serve = match map.get("serve") {
        None => None,
        Some(serve) => Some(string(serve, file, "`start`'s `serve`")?.to_string()),
    };
    let port = read_port(map, "port", file)?;
    let devdir = read_devdir(map, file, targets)?;
    Ok(Start {
        run,
        watch,
        serve,
        port,
        devdir,
    })
}

/// `start`'s `devdir`: the directory the dev loop's builds go into.
///
/// Validated against the targets it is defined to stay clear of: a dev
/// directory that *is* a deploy output, contains one, or sits inside one
/// recreates the overwrite the separation exists to prevent — silently, one
/// save at a time.
fn read_devdir(
    map: &Map<String, Value>,
    file: &str,
    targets: &[Target],
) -> Result<Option<String>, String> {
    let Some(value) = map.get("devdir") else {
        return Ok(None);
    };
    let devdir = string(value, file, "`start`'s `devdir`")?;
    if devdir.is_empty() {
        return Err(format!(
            "{file}: `start`'s `devdir` is empty — name the directory the dev loop writes into (\"{DEFAULT_DEV_DIR}\")."
        ));
    }
    let path = Path::new(devdir);
    if path.is_absolute() {
        return Err(format!(
            "{file}: `start`'s `devdir` is absolute, and the dev loop writes inside the project — write \"{DEFAULT_DEV_DIR}\", not \"{devdir}\"."
        ));
    }
    let flat = flatten(path);
    if flat.as_os_str().is_empty() {
        return Err(format!(
            "{file}: `start`'s `devdir` is the project root, which is where `esdev build` deploys — \
             the dev loop needs a directory of its own (\"{DEFAULT_DEV_DIR}\"), or every save overwrites the deployment."
        ));
    }
    if flat
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(format!(
            "{file}: `start`'s `devdir` escapes the project, and the dev loop writes inside it — keep it under the project (\"{DEFAULT_DEV_DIR}\")."
        ));
    }
    for target in targets {
        let out = match &target.output {
            Output::File(out) | Output::Dir(out) => out,
        };
        if overlaps(&flat, &flatten(Path::new(out))) {
            return Err(format!(
                "{file}: `start`'s `devdir` (\"{devdir}\") overlaps target \"{}\"'s output (\"{out}\") — \
                 the dev loop would write into what `esdev build` deploys. Give it a directory of its own.",
                target.name
            ));
        }
    }
    Ok(Some(devdir.to_string()))
}

/// A path with its `.` components removed, so `dist/` and `dist` compare as
/// the same directory and `./` flattens to the project root.
fn flatten(path: &Path) -> PathBuf {
    path.components()
        .filter(|c| !matches!(c, std::path::Component::CurDir))
        .collect()
}

/// Whether two project-relative paths overlap: equal, or one inside the other.
fn overlaps(a: &Path, b: &Path) -> bool {
    a == b || a.starts_with(b) || b.starts_with(a)
}

/// One of `start`'s port numbers, checked for being one.
fn read_port(map: &Map<String, Value>, key: &str, file: &str) -> Result<Option<u16>, String> {
    match map.get(key) {
        None => Ok(None),
        Some(port) => Ok(Some(
            port.as_u64()
                .filter(|p| *p > 0 && *p <= u64::from(u16::MAX))
                .and_then(|p| u16::try_from(p).ok())
                .ok_or_else(|| {
                    format!("{file}: `start`'s `{key}` is a number from 1 to 65535, not {port}.")
                })?,
        )),
    }
}

/// Reads a `plugins` list.
///
/// # Why a config file can carry a plugin at all
///
/// The header of this file argues that an executable config would be "a program
/// whose entire content is data", and that argument turned on esdev having no
/// plugin API. It has one — `runtime:build`'s, the same contract this
/// toolchain's own passes implement — and without a way to say so here, a
/// project that compiles `.jsx` or `.mdx` could only be built by a *program*
/// that called `build()` itself. `esdev build` and `esdev start` could not
/// build it at all.
///
/// So the file names the module and esdev imports it. What JSON cannot hold is
/// the **call**: a plugin that takes options is a factory, and `mdx({ …})` is a
/// function application. The file holds the argument instead —
/// `{ "module": "…", "options": { … } }` — and esdev makes the call. That is
/// the whole of the difference from an executable config, and it keeps
/// `permissions` decidable without running anything: this file is still read as
/// data, and the plugins load *after* what the run may do has been settled.
///
/// Two spellings, because most plugins need no options:
///
/// ```json
/// "plugins": ["./plugins/mdx.js", { "module": "@otfw/compiler", "options": { "jsx": "automatic" } }]
/// ```
fn plugin_specs(value: Option<&Value>, file: &str, at: &str) -> Result<Vec<PluginSpec>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let items = value.as_array().ok_or_else(|| {
        format!(
            "{file}: {at} is {}, and should be a list of plugins.\n\n\
             Each is a module to import — a path in this project, or a package: \
             \"plugins\": [\"./plugins/mdx.js\"]",
            kind(value)
        )
    })?;
    items
        .iter()
        .map(|item| plugin_spec(item, file, at))
        .collect()
}

fn plugin_spec(value: &Value, file: &str, at: &str) -> Result<PluginSpec, String> {
    if let Value::String(_) = value {
        return Ok(PluginSpec {
            module: string(value, file, at)?.to_string(),
            export: None,
            options: None,
        });
    }
    let map = object(value, file, at).map_err(|_| {
        format!(
            "{file}: {at} has {}, and a plugin is a module to import.\n\n\
             Write the module — \"./plugins/mdx.js\" — or an object naming it with \
             what to call it with: {{ \"module\": \"./plugins/mdx.js\", \"options\": {{ … }} }}",
            kind(value)
        )
    })?;
    known_keys(map, file, at, &["module", "export", "options"])?;
    let module = match map.get("module") {
        Some(module) => string(module, file, &format!("{at}'s `module`"))?.to_string(),
        None => {
            return Err(format!(
                "{file}: {at} has no `module`.\n\n\
                 A plugin is a module to import: {{ \"module\": \"./plugins/mdx.js\" }}"
            ));
        }
    };
    let export = match map.get("export") {
        None => None,
        Some(export) => Some(string(export, file, &format!("{at}'s `export`"))?.to_string()),
    };
    Ok(PluginSpec {
        module,
        export,
        options: map.get("options").cloned(),
    })
}

/// `refresh`, checked against what could actually implement it.
///
/// A name is refused rather than ignored, because a name that is quietly
/// dropped is a project whose components stop keeping their state one day, with
/// the reason sitting unread in a config file.
///
/// **esdev implements no scheme, and knows the name of none.** It used to
/// implement React's, and `"react"` was for a while the only name this would
/// accept — which meant every other framework took a full page reload on each
/// edit, not because the mechanism was missing but because the config would not
/// let a target say it had a scheme. Both halves of that are gone: the React
/// pass moved out into a plugin the template loads, and what is left here is
/// generic. A scheme is a plugin, so a target that names one and has no plugins
/// has named something nothing can implement — which is the one case worth
/// refusing, and the only check left.
fn refresh(
    value: Option<&Value>,
    file: &str,
    at: &str,
    has_plugins: bool,
) -> Result<Option<String>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    let name = string(value, file, &format!("{at}'s `refresh`"))?;
    if !has_plugins {
        return Err(format!(
            "{file}: {at}'s `refresh` is \"{name}\", and this target has no plugins \
             that could implement it.\n\n\
             It names the hot-reload convention this target's modules are prepared \
             for — registering components, matching hook signatures, whatever the \
             framework's scheme is. esdev provides the generic half (`import.meta.hot` \
             and the update channel); the scheme itself is a `plugins` entry, which \
             reads this name as `ctx.refresh`."
        ));
    }
    Ok(Some(name.to_string()))
}

/// The error for a `start` key naming a target that is not there.
fn unknown_target(file: &str, at: &str, named: &str, names: &[&str]) -> String {
    let suggestion = nearest(named, names)
        .map(|near| format!(" Did you mean \"{near}\"?"))
        .unwrap_or_default();
    format!(
        "{file}: {at} names \"{named}\", which is not a target.{suggestion}\n\n\
         Targets in this file: {}.",
        names.join(", ")
    )
}

/// Validates `permissions` by translating it into the flags it stands for and
/// handing them to the parser `esrun` uses.
///
/// **The translation is the point.** A second dialect of what `read` means would
/// be a second thing to keep true, and the one that drifted would be the one
/// granting capabilities. Here `{"allow": {"read": ["./data"]}}` becomes
/// `--allow-read=./data` and is checked by exactly the code that checks the
/// flag — so an unknown capability, a scope on a capability that takes none, and
/// a grant that moves the wrong way all fail here with the message they have
/// always had.
///
/// Checked against [`Baseline::Nothing`], because this block states the grant
/// the *deployed* program runs under — an `esrun` line — even though `esdev
/// start` is what spawns it. The returned list is therefore pinned to its mode
/// with an explicit `--deny-all`/`--allow-all` (D65), so it means the same thing
/// whichever binary is handed it and a developer's `esdev start` child runs
/// under exactly the production grant.
fn permission_flags(value: &Value, file: &str) -> Result<Vec<String>, String> {
    let map = object(value, file, "`permissions`")?;
    known_keys(map, file, "`permissions`", &["deny", "allow"])?;
    let mut permissions = Permissions::new(Baseline::Nothing);
    let mut flags = Vec::new();

    for name in string_array(map.get("deny"), file, "`permissions`'s `deny`")? {
        let flag = format!("--deny-{name}");
        try_permission_flag(&mut permissions, &flag, None)
            .map_err(|e| format!("{file}: `permissions`: {e}"))?;
        flags.push(flag);
    }
    if let Some(allow) = map.get("allow") {
        let allow = object(allow, file, "`permissions`'s `allow`")?;
        for (name, scopes) in allow {
            let flag = format!("--allow-{name}");
            let at = format!("`permissions`'s `allow.{name}`");
            // `true` is the unnarrowed grant, the shape `--allow-net` has. A
            // list narrows it. Both spellings exist because both flags do.
            let value = match scopes {
                Value::Bool(true) => None,
                Value::Array(_) => Some(string_array(Some(scopes), file, &at)?.join(",")),
                other => {
                    return Err(format!(
                        "{file}: {at} is {other}, which is neither a grant nor a \
                         narrowing.\n\n\
                         Write `true` to grant it outright, or a list to narrow it: \
                         \"read\": [\"./data\"]."
                    ));
                }
            };
            try_permission_flag(&mut permissions, &flag, value.as_deref())
                .map_err(|e| format!("{file}: `permissions`: {e}"))?;
            flags.push(match &value {
                Some(scopes) => format!("{flag}={scopes}"),
                None => flag,
            });
        }
    }
    // Resolving is what rejects a grant that contradicts the denials around it,
    // and it is cheap; doing it here means the file is wrong when it is read
    // rather than when a run is finally attempted with it.
    permissions
        .resolve()
        .map_err(|e| format!("{file}: `permissions`: {e}"))?;
    permissions
        .scopes()
        .map_err(|e| format!("{file}: `permissions`: {e}"))?;
    // Pin the mode. A file that already says `"deny": ["all"]` or
    // `"allow": {"all": true}` has said it; anything else was checked against
    // "nothing granted" and has to carry that with it, or `esdev start` — whose
    // own baseline is everything — would read the same list the other way round.
    if !flags
        .iter()
        .any(|f| f == "--deny-all" || f == "--allow-all")
    {
        flags.insert(0, "--deny-all".to_string());
    }
    Ok(flags)
}

/// Reads a JSON object, or says what was found instead.
fn object<'a>(value: &'a Value, file: &str, at: &str) -> Result<&'a Map<String, Value>, String> {
    value
        .as_object()
        .ok_or_else(|| format!("{file}: {at} is {}, and should be an object.", kind(value)))
}

/// Reads a JSON string.
fn string<'a>(value: &'a Value, file: &str, at: &str) -> Result<&'a str, String> {
    let text = value
        .as_str()
        .ok_or_else(|| format!("{file}: {at} is {}, and should be a string.", kind(value)))?;
    if text.trim().is_empty() {
        return Err(format!("{file}: {at} is empty."));
    }
    Ok(text)
}

/// Reads a JSON array of strings; absent is an empty list.
fn string_array(value: Option<&Value>, file: &str, at: &str) -> Result<Vec<String>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let items = value
        .as_array()
        .ok_or_else(|| format!("{file}: {at} is {}, and should be a list.", kind(value)))?;
    items
        .iter()
        .map(|item| string(item, file, at).map(str::to_string))
        .collect()
}

/// Reads a JSON boolean; absent is `false`.
fn flag(value: Option<&Value>, file: &str, at: &str) -> Result<bool, String> {
    match value {
        None => Ok(false),
        Some(value) => value.as_bool().ok_or_else(|| {
            format!(
                "{file}: {at} is {}, and should be true or false.",
                kind(value)
            )
        }),
    }
}

/// Reads a `define` object into the pairs `--define=<name>=<value>` makes.
///
/// The values are JSON, and what reaches the bundler is their JSON text — so
/// `"port": 8080` replaces the name with the number `8080` and `"mode": "dev"`
/// replaces it with the *string* `"dev"`, quotes included. That is the part a
/// hand-written `--define` gets wrong: on a command line the quotes have to
/// survive the shell, and here the type is simply what you wrote.
fn defines(value: Option<&Value>, file: &str, at: &str) -> Result<Vec<(String, String)>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let map = object(value, file, &format!("{at}'s `define`"))?;
    map.iter()
        .map(|(name, replacement)| match replacement {
            Value::Object(_) | Value::Array(_) => Err(format!(
                "{file}: {at}'s `define.{name}` is {}, and a replacement is a single \
                 value.\n\n\
                 What lands in the bundle is the JSON text of it, so a string, a \
                 number or a boolean.",
                kind(replacement)
            )),
            other => Ok((name.clone(), other.to_string())),
        })
        .collect()
}

/// Parses a target's `sourcemap`: `true`, or which shape of one.
///
/// A boolean as well as the three names, because "yes" is what most projects
/// mean and `true` is how a JSON file says it. `false` is "none", which is also
/// what leaving the key out means for a release build.
fn sourcemap(value: Option<&Value>, file: &str, at: &str) -> Result<Option<String>, String> {
    match value {
        None => Ok(None),
        Some(Value::Bool(true)) => Ok(Some("external".to_string())),
        Some(Value::Bool(false)) => Ok(Some("none".to_string())),
        Some(Value::String(kind))
            if matches!(kind.as_str(), "external" | "inline" | "hidden" | "none") =>
        {
            Ok(Some(kind.clone()))
        }
        Some(other) => Err(format!(
            "{file}: {at}'s `sourcemap` is {}, and it says whether a build writes \
             them.\n\n  \
             true, or \"external\" — a .map beside the output\n  \
             \"inline\"        — a data URL in the file itself\n  \
             \"hidden\"        — written, with nothing pointing at it\n  \
             false, or \"none\"  — none",
            kind(other)
        )),
    }
}

/// Parses a library target's `format`: which module systems it writes.
///
/// A string or a list, so the common case reads as one — `"format": "cjs"` —
/// and both is a list. The names are `--format`'s, because they are the same
/// answer to the same question.
fn library_formats(value: Option<&Value>, file: &str, at: &str) -> Result<Vec<String>, String> {
    let named = match value {
        None => return Ok(Vec::new()),
        Some(Value::String(one)) => vec![one.clone()],
        Some(Value::Array(many)) => many
            .iter()
            .map(|entry| match entry {
                Value::String(name) => Ok(name.clone()),
                other => Err(format!(
                    "{file}: {at}'s `format` holds {}, and each entry is a module \
                     system's name.",
                    kind(other)
                )),
            })
            .collect::<Result<Vec<_>, _>>()?,
        Some(other) => {
            return Err(format!(
                "{file}: {at}'s `format` is {}, and it names the module systems this \
                 library writes.\n\n  \
                 \"esm\"            — ES modules, the default\n  \
                 \"cjs\"            — CommonJS, for a require() consumer\n  \
                 [\"esm\", \"cjs\"]   — both, into one directory",
                kind(other)
            ));
        }
    };
    let mut formats: Vec<String> = Vec::new();
    for name in named {
        if !matches!(name.as_str(), "esm" | "cjs") {
            return Err(format!(
                "{file}: {at}'s `format` names \"{name}\".\n\n\
                 It is \"esm\" (ES modules) or \"cjs\" (CommonJS)."
            ));
        }
        if formats.contains(&name) {
            return Err(format!("{file}: {at}'s `format` names \"{name}\" twice."));
        }
        formats.push(name);
    }
    Ok(formats)
}

/// Parses `dts-bundle`, resolving `true` to the entry it would take.
///
/// Resolved **here** rather than in the build, for the reason the flag resolves
/// it in the argument parser: a `true` with no `index.ts` under the source
/// directory should say so while naming what was looked for and how to name a
/// different one — not fail later, from inside a build, about a file the config
/// never mentioned.
fn dts_bundle(
    value: Option<&Value>,
    file: &str,
    at: &str,
    dir: &Path,
    entry: &str,
) -> Result<Option<String>, String> {
    match value {
        None | Some(Value::Bool(false)) => Ok(None),
        Some(Value::String(named)) => Ok(Some(named.clone())),
        Some(Value::Bool(true)) => {
            let found = ["ts", "tsx", "mts", "cts"]
                .iter()
                .map(|extension| Path::new(entry).join(format!("index.{extension}")))
                .find(|candidate| dir.join(candidate).is_file());
            match found {
                Some(found) => Ok(Some(found.to_string_lossy().into_owned())),
                None => Err(format!(
                    "{file}: {at} has \"dts-bundle\": true, and there is no index.ts \
                     in {entry}.\n\n\
                     One declaration file is built from one entry. Name it: \
                     \"dts-bundle\": \"{entry}/main.ts\"."
                )),
            }
        }
        Some(other) => Err(format!(
            "{file}: {at}'s `dts-bundle` is {}, and it says whether every \
             declaration is linked into one file.\n\n  \
             true             — from this target's index.ts\n  \
             \"src/main.ts\"    — from the entry you name\n  \
             false            — one .d.ts per module, the default",
            kind(other)
        )),
    }
}

/// Parses `alias`: what a specifier is rewritten to before it is resolved.
///
/// A **path** replacement is resolved against the project directory and kept
/// absolute, because that is the only spelling that means the same thing from
/// every working directory a build might be started from. Anything else is left
/// as written — `"react": "preact/compat"` names a package, and where that lives
/// is the resolver's question, not this file's.
fn aliases(value: Option<&Value>, file: &str, dir: &Path) -> Result<Vec<(String, String)>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let map = object(value, file, "`alias`")?;
    let mut found = Vec::new();
    for (find, replacement) in map {
        if find.trim().is_empty() {
            return Err(format!(
                "{file}: `alias` has an empty name.\n\n\
                 An alias rewrites the start of a specifier: \
                 \"alias\": {{ \"@\": \"./src\" }}."
            ));
        }
        let Value::String(replacement) = replacement else {
            return Err(format!(
                "{file}: `alias.{find}` is {}, and an alias is replaced by one \
                 path or package name.\n\n  \
                 \"alias\": {{ \"@\": \"./src\", \"react\": \"preact/compat\" }}",
                kind(replacement)
            ));
        };
        let is_path = replacement.starts_with("./")
            || replacement.starts_with("../")
            || Path::new(replacement).is_absolute();
        let to = if is_path {
            dir.join(replacement).to_string_lossy().into_owned()
        } else {
            replacement.clone()
        };
        found.push((find.clone(), to));
    }
    // Longest first, so `@/ui` wins over `@` — a resolver takes the first match,
    // and the first match written in a JSON object is whichever one the map
    // happened to yield.
    found.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then_with(|| a.0.cmp(&b.0)));
    Ok(found)
}

/// Rejects a key that is not in `allowed`, naming the nearest one when the
/// spelling is close — a mistyped key is otherwise a setting that silently does
/// nothing, which for `minify` is a slow bundle and for `platform` is the wrong
/// build of a dependency.
fn known_keys(
    map: &Map<String, Value>,
    file: &str,
    at: &str,
    allowed: &[&str],
) -> Result<(), String> {
    for key in map.keys() {
        if allowed.contains(&key.as_str()) {
            continue;
        }
        let where_ = if at.is_empty() {
            String::new()
        } else {
            format!(" in {at}")
        };
        let suggestion = nearest(key, allowed)
            .map(|near| format!(" Did you mean `{near}`?"))
            .unwrap_or_default();
        return Err(format!(
            "{file}: unknown key `{key}`{where_}.{suggestion}\n\n\
             Known here: {}.",
            allowed.join(", ")
        ));
    }
    Ok(())
}

/// The closest candidate to `word`, if one is close enough to be a typo of it.
fn nearest<'a>(word: &str, candidates: &[&'a str]) -> Option<&'a str> {
    candidates
        .iter()
        .map(|candidate| (distance(word, candidate), *candidate))
        // Two edits on a short key is the line between a typo and a different
        // word: `outDir` reaches `outdir`, `output` does not reach `out`.
        .filter(|(distance, _)| *distance <= 2)
        .min_by_key(|(distance, _)| *distance)
        .map(|(_, candidate)| candidate)
}

/// Levenshtein distance, case-insensitively — `outDir` is a typo of `outdir`
/// and the message should say so rather than list the keys and leave it there.
fn distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.to_lowercase().chars().collect();
    let b: Vec<char> = b.to_lowercase().chars().collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0; b.len() + 1];
    for (i, x) in a.iter().enumerate() {
        current[0] = i + 1;
        for (j, y) in b.iter().enumerate() {
            let cost = usize::from(x != y);
            current[j + 1] = (previous[j] + cost)
                .min(previous[j + 1] + 1)
                .min(current[j] + 1);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[b.len()]
}

/// What a JSON value is, for a message that has to say what was found.
fn kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "a list",
        Value::Object(_) => "an object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parses `text` as a config, or returns the error message.
    fn read(text: &str) -> Result<Project, String> {
        parse(text, PathBuf::from("."), "esdev.json").map(|p| p.expect("a config"))
    }

    /// A directory holding a fixture project, removed when the test ends.
    /// `otfw_reason` reads the filesystem, so unlike `parse` it cannot be
    /// tested without one.
    fn fixture(name: &str, files: &[(&str, &str)]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("esdev-otfw-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the fixture");
        for (name, contents) in files {
            std::fs::write(dir.join(name), contents).expect("write the fixture");
        }
        dir
    }

    /// An OTF Web project is recognised by what builds it: an `otfw.config.js`,
    /// or scripts calling `otfw`. Anything else — an esdev project, an empty
    /// directory, a broken `package.json` — is not evidence either way.
    #[test]
    fn an_otfw_project_is_recognised_by_what_builds_it() {
        let scripts = fixture(
            "scripts",
            &[(
                "package.json",
                r#"{ "scripts": { "dev": "otfw dev", "build": "otfw build" } }"#,
            )],
        );
        let reason = otfw_reason(&scripts).expect("recognised");
        assert!(reason.contains("\"dev\""), "{reason}");
        assert!(reason.contains("\"build\""), "{reason}");

        let config = fixture("config", &[("otfw.config.js", "export default {};\n")]);
        let reason = otfw_reason(&config).expect("recognised");
        assert!(reason.contains("otfw.config.js"), "{reason}");

        let esdev = fixture(
            "esdev",
            &[(
                "package.json",
                r#"{ "scripts": { "dev": "esdev start", "build": "esdev build" } }"#,
            )],
        );
        assert!(otfw_reason(&esdev).is_none());

        let empty = fixture("empty", &[]);
        assert!(otfw_reason(&empty).is_none());

        let broken = fixture("broken", &[("package.json", "{ not json")]);
        assert!(otfw_reason(&broken).is_none());

        for dir in [&scripts, &config, &esdev, &empty, &broken] {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn a_target_is_an_entry_and_where_its_output_goes() {
        let project = read(
            r#"{ "targets": { "server": { "entry": "src/server.ts", "out": "dist/server.js" } } }"#,
        )
        .expect("parsed");
        let target = &project.targets[0];
        assert_eq!(target.name, "server");
        assert_eq!(target.entry, "src/server.ts");
        assert!(matches!(&target.output, Output::File(out) if out == "dist/server.js"));
        assert_eq!(target.platform, Platform::Server);
        assert!(!target.run_after_build);
    }

    /// A library target says the same things `--lib` says, in the same words.
    #[test]
    fn a_library_target_carries_what_the_flags_carry() {
        let project = read(
            r#"{ "targets": { "std": {
                 "entry": "src", "lib": true, "outdir": "dist",
                 "format": ["esm", "cjs"], "minify": true,
                 "assets": ["README.md", "LICENSE"] } } }"#,
        )
        .expect("parsed");
        let target = &project.targets[0];
        assert!(target.lib);
        assert_eq!(target.formats, ["esm", "cjs"]);
        assert!(target.minify);
        // The default, and the one that matters: a library emits declarations
        // unless it says otherwise.
        assert!(target.types);
        assert_eq!(target.dts_bundle, None);
        assert_eq!(target.assets, ["README.md", "LICENSE"]);
        assert!(matches!(&target.output, Output::Dir(dir) if dir == "dist"));
    }

    /// One format is a string, because the common case should read as one.
    #[test]
    fn a_single_format_needs_no_list() {
        let project = read(
            r#"{ "targets": { "std": { "entry": "src", "lib": true, "outdir": "dist",
                 "format": "cjs" } } }"#,
        )
        .expect("parsed");
        assert_eq!(project.targets[0].formats, ["cjs"]);
    }

    /// The keys that mean nothing off a library are refused rather than
    /// ignored — the same answer `esdev build` gives the same flags, because a
    /// file and a command line disagreeing about that is how a project gets
    /// built one way from each.
    #[test]
    fn the_library_keys_are_refused_on_an_application() {
        for (key, value) in [
            ("format", "\"cjs\""),
            ("types", "false"),
            ("dts-bundle", "true"),
        ] {
            let err = read(&format!(
                r#"{{ "targets": {{ "app": {{ "entry": "src/app.ts",
                     "out": "dist/app.js", "{key}": {value} }} }} }}"#
            ))
            .expect_err("refused");
            assert!(err.contains(key), "{err}");
            assert!(err.contains("lib"), "{err}");
        }
    }

    /// A library keeps its module structure, so its output is a directory.
    #[test]
    fn a_library_writing_one_file_is_refused() {
        let err = read(
            r#"{ "targets": { "std": { "entry": "src", "lib": true,
                 "out": "dist/std.js" } } }"#,
        )
        .expect_err("refused");
        assert!(err.contains("outdir"), "{err}");
    }

    /// A document is not a library's entry: a published tree has no one root.
    #[test]
    fn a_library_rooted_at_a_document_is_refused() {
        let err = read(
            r#"{ "targets": { "std": { "entry": "index.html", "lib": true,
                 "outdir": "dist" } } }"#,
        )
        .expect_err("refused");
        assert!(err.contains("source directory"), "{err}");
    }

    /// `"then": "run"` executes what was built, and a library is imported by
    /// somebody else's build rather than run by this one.
    #[test]
    fn a_library_that_runs_afterwards_is_refused() {
        let err = read(
            r#"{ "targets": { "std": { "entry": "src", "lib": true,
                 "outdir": "dist", "then": "run" } } }"#,
        )
        .expect_err("refused");
        assert!(err.contains("imported"), "{err}");
    }

    /// The module systems are named, so a typo is an error rather than a
    /// silently missing half of the package.
    #[test]
    fn an_unknown_format_is_named() {
        let err = read(
            r#"{ "targets": { "std": { "entry": "src", "lib": true, "outdir": "dist",
                 "format": ["esm", "umd"] } } }"#,
        )
        .expect_err("refused");
        assert!(err.contains("umd"), "{err}");
        assert!(err.contains("\"cjs\""), "{err}");

        let twice = read(
            r#"{ "targets": { "std": { "entry": "src", "lib": true, "outdir": "dist",
                 "format": ["esm", "esm"] } } }"#,
        )
        .expect_err("refused");
        assert!(twice.contains("twice"), "{twice}");
    }

    /// `"dts-bundle": true` needs an index to bundle from, and says so while
    /// naming what it looked for — resolved when the file is read rather than
    /// from inside a build, which is where the flag resolves it too.
    #[test]
    fn dts_bundle_true_with_no_index_says_where_to_look() {
        let err = read(
            r#"{ "targets": { "std": { "entry": "nowhere", "lib": true,
                 "outdir": "dist", "dts-bundle": true } } }"#,
        )
        .expect_err("refused");
        assert!(err.contains("index.ts"), "{err}");
        assert!(err.contains("nowhere/main.ts"), "{err}");
    }

    /// …and a named entry is taken as written.
    #[test]
    fn dts_bundle_takes_the_entry_it_is_given() {
        let project = read(
            r#"{ "targets": { "std": { "entry": "src", "lib": true, "outdir": "dist",
                 "dts-bundle": "src/public.ts" } } }"#,
        )
        .expect("parsed");
        assert_eq!(
            project.targets[0].dts_bundle.as_deref(),
            Some("src/public.ts")
        );
    }

    /// The project's plugins are every target's, and a target's own are added
    /// to them rather than replacing them: a project that compiles `.mdx`
    /// compiles it for the server bundle and the browser one.
    #[test]
    fn a_targets_plugins_add_to_the_projects() {
        let project = read(
            r#"{
              "plugins": ["./plugins/mdx.js"],
              "targets": {
                "api": { "entry": "src/api.ts", "out": "dist/api.js" },
                "web": { "entry": "src/web.ts", "out": "dist/web.js",
                         "plugins": ["./plugins/only-web.js"] }
              }
            }"#,
        )
        .expect("parsed");

        assert_eq!(
            project
                .plugins
                .iter()
                .map(|p| p.module.as_str())
                .collect::<Vec<_>>(),
            ["./plugins/mdx.js", "./plugins/only-web.js"],
        );
        // Sorted by name, so `api` is first.
        assert_eq!(project.targets[0].plugins, [0]);
        assert_eq!(project.targets[1].plugins, [0, 1]);
    }

    /// The call a JSON file cannot make. A plugin that takes options is a
    /// factory, so the file carries the argument and esdev makes the call.
    #[test]
    fn a_plugin_may_name_its_export_and_its_options() {
        let project = read(
            r#"{
              "plugins": [{ "module": "@otfw/compiler", "export": "compiler",
                            "options": { "jsx": "automatic" } }],
              "targets": { "web": { "entry": "src/web.ts", "out": "dist/web.js" } }
            }"#,
        )
        .expect("parsed");
        let plugin = &project.plugins[0];
        assert_eq!(plugin.module, "@otfw/compiler");
        assert_eq!(plugin.export.as_deref(), Some("compiler"));
        assert_eq!(plugin.options.as_ref().unwrap()["jsx"], "automatic");
    }

    /// A plugin with no `module` names nothing to import, and a mistyped key
    /// beside it is a setting that would silently do nothing.
    #[test]
    fn a_plugin_entry_has_to_name_a_module() {
        let refused = read(
            r#"{ "plugins": [{ "options": {} }],
                 "targets": { "web": { "entry": "a.ts", "out": "dist/a.js" } } }"#,
        )
        .expect_err("no module");
        assert!(refused.contains("has no `module`"), "{refused}");

        let mistyped = read(
            r#"{ "plugins": [{ "module": "./p.js", "option": {} }],
                 "targets": { "web": { "entry": "a.ts", "out": "dist/a.js" } } }"#,
        )
        .expect_err("mistyped key");
        assert!(mistyped.contains("option"), "{mistyped}");
    }

    /// `plugins` is a list. A bare string is the shape somebody reaches for
    /// first, and accepting it silently would build with one plugin where the
    /// file says one plugin's *characters*.
    #[test]
    fn plugins_is_a_list() {
        let refused = read(
            r#"{ "plugins": "./p.js",
                 "targets": { "web": { "entry": "a.ts", "out": "dist/a.js" } } }"#,
        )
        .expect_err("not a list");
        assert!(refused.contains("list of plugins"), "{refused}");
    }

    /// `refresh` names a scheme, and **esdev implements none**. It provides
    /// the generic half — `import.meta.hot`, the update channel — and a scheme
    /// is a plugin. So a target that names one and has no plugins has named
    /// something nothing can implement, which is the one case worth refusing:
    /// a `refresh` that silently did nothing is a project whose components
    /// stop keeping their state one day, with the reason unread in a config
    /// file.
    #[test]
    fn a_refresh_scheme_needs_a_plugin_that_could_implement_it() {
        let refused =
            read(r#"{ "targets": { "web": { "entry": "index.html", "refresh": "otfw" } } }"#)
                .expect_err("no plugin to implement it");
        assert!(
            refused.contains("no plugins that could implement"),
            "{refused}"
        );

        // React is not privileged. It was the only name this took for a while,
        // and it is now a plugin like any other — the react template's own.
        let react =
            read(r#"{ "targets": { "web": { "entry": "index.html", "refresh": "react" } } }"#)
                .expect_err("react is not built in either");
        assert!(react.contains("no plugins that could implement"), "{react}");

        for scheme in ["otfw", "react"] {
            let accepted = read(&format!(
                r#"{{ "plugins": ["./plugins/{scheme}.js"],
                     "targets": {{ "web": {{ "entry": "index.html", "refresh": "{scheme}" }} }} }}"#
            ))
            .expect("a plugin can implement it");
            assert_eq!(accepted.targets[0].refresh.as_deref(), Some(scheme));
        }
    }

    /// The same default the command line has: a config that omits `out` and a
    /// command line that omits `--out` must write the same file.
    #[test]
    fn the_output_defaults_to_dist_beside_the_entry_name() {
        let project = read(r#"{ "targets": { "app": { "entry": "src/app.ts" } } }"#).expect("ok");
        assert!(matches!(&project.targets[0].output, Output::File(out) if out == "dist/app.js"));
    }

    #[test]
    fn targets_come_back_in_name_order() {
        let project = read(
            r#"{ "targets": {
                   "server": { "entry": "s.ts" },
                   "browser": { "entry": "c.tsx", "outdir": "dist/client", "platform": "browser" }
                 } }"#,
        )
        .expect("parsed");
        let names: Vec<&str> = project.targets.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["browser", "server"]);
        assert_eq!(project.targets[0].platform, Platform::Browser);
    }

    /// A mistyped key is a setting that silently does nothing, so it is an
    /// error — and the message names the key it was nearly.
    #[test]
    fn a_mistyped_key_is_named_and_corrected() {
        let err = read(r#"{ "targets": { "a": { "entry": "a.ts", "outDir": "dist" } } }"#)
            .expect_err("refused");
        assert!(err.contains("unknown key `outDir`"), "{err}");
        assert!(err.contains("Did you mean `outdir`?"), "{err}");

        let top = read(r#"{ "target": {} }"#).expect_err("refused");
        assert!(top.contains("Did you mean `targets`?"), "{top}");
    }

    /// The two output shapes are different things, and a target that asks for
    /// both has not decided which.
    #[test]
    fn out_and_outdir_are_not_both() {
        let err =
            read(r#"{ "targets": { "a": { "entry": "a.ts", "out": "d/a.js", "outdir": "d" } } }"#)
                .expect_err("refused");
        assert!(err.contains("both `out` and `outdir`"), "{err}");
    }

    /// `out` naming a directory would produce a directory literally called
    /// `dist`, and `outdir` naming a file the reverse.
    #[test]
    fn the_output_shape_must_match_the_key() {
        let file = read(r#"{ "targets": { "a": { "entry": "a.ts", "out": "dist" } } }"#)
            .expect_err("refused");
        assert!(file.contains("names a directory"), "{file}");

        let dir = read(r#"{ "targets": { "a": { "entry": "a.ts", "outdir": "dist/a.js" } } }"#)
            .expect_err("refused");
        assert!(dir.contains("names a file"), "{dir}");
    }

    #[test]
    fn a_target_without_an_entry_is_refused() {
        let err = read(r#"{ "targets": { "a": { "out": "dist/a.js" } } }"#).expect_err("refused");
        assert!(err.contains("has no `entry`"), "{err}");
    }

    #[test]
    fn the_platform_is_one_of_two_words() {
        let err = read(r#"{ "targets": { "a": { "entry": "a.ts", "platform": "node" } } }"#)
            .expect_err("refused");
        assert!(err.contains("\"server\""), "{err}");
        assert!(err.contains("\"browser\""), "{err}");
    }

    /// A browser bundle expects a `document`; running it here is a mistake with
    /// a confusing failure, so it is refused where it is written.
    #[test]
    fn a_browser_target_cannot_be_run_after_the_build() {
        let err = read(
            r#"{ "targets": { "a": { "entry": "a.tsx", "outdir": "d", "platform": "browser", "then": "run" } } }"#,
        )
        .expect_err("refused");
        assert!(err.contains("browser target"), "{err}");
    }

    #[test]
    fn then_run_is_the_only_thing_a_build_does_next() {
        let ok = read(r#"{ "targets": { "a": { "entry": "a.ts", "then": "run" } } }"#).expect("ok");
        assert!(ok.targets[0].run_after_build);

        let err = read(r#"{ "targets": { "a": { "entry": "a.ts", "then": "deploy" } } }"#)
            .expect_err("refused");
        assert!(err.contains("\"run\""), "{err}");
    }

    /// The type of a replacement is what was written, which is the part a
    /// hand-written `--define` gets wrong once the shell has eaten the quotes.
    #[test]
    fn a_define_keeps_the_json_type_it_was_written_with() {
        let project = read(
            r#"{ "targets": { "a": { "entry": "a.ts",
                 "define": { "MODE": "dev", "PORT": 8080, "DEBUG": false } } } }"#,
        )
        .expect("parsed");
        let define = &project.targets[0].define;
        assert!(define.contains(&("MODE".to_string(), "\"dev\"".to_string())));
        assert!(define.contains(&("PORT".to_string(), "8080".to_string())));
        assert!(define.contains(&("DEBUG".to_string(), "false".to_string())));
    }

    #[test]
    fn a_define_of_a_whole_object_is_refused() {
        let err =
            read(r#"{ "targets": { "a": { "entry": "a.ts", "define": { "X": { "y": 1 } } } } }"#)
                .expect_err("refused");
        assert!(err.contains("a single value"), "{err}");
    }

    /// `start` is validated by the command that reads the file, not held until
    /// the day somebody runs `esdev start`.
    #[test]
    fn start_must_name_targets_that_exist() {
        let err = read(
            r#"{ "targets": { "server": { "entry": "s.ts" } },
                 "start": { "run": "sever" } }"#,
        )
        .expect_err("refused");
        assert!(err.contains("is not a target"), "{err}");
        assert!(err.contains("Did you mean \"server\"?"), "{err}");

        read(
            r#"{ "targets": { "server": { "entry": "s.ts" } },
                 "start": { "run": "server", "watch": ["server"], "port": 5173 } }"#,
        )
        .expect("parsed");
    }

    /// The dev loop writes into `.dev` unless the file says otherwise, and the
    /// directory it names has to be a directory of its own: overlapping a
    /// deploy output recreates the overwrite the separation exists to prevent.
    #[test]
    fn devdir_defaults_and_stays_clear_of_deploy_outputs() {
        let silent =
            read(r#"{ "targets": { "server": { "entry": "s.ts", "out": "dist/server.js" } } }"#)
                .expect("parsed");
        assert_eq!(silent.start.devdir(), DEFAULT_DEV_DIR);
        assert_eq!(silent.start.devdir, None);

        let named = read(
            r#"{ "targets": { "server": { "entry": "s.ts", "out": "dist/server.js" } },
                 "start": { "devdir": "tmp-dev" } }"#,
        )
        .expect("parsed");
        assert_eq!(named.start.devdir(), "tmp-dev");

        // Each overlap in turn: the output itself, the directory holding it,
        // and a directory beneath an output directory. All three would put
        // dev builds where `esdev build` deploys.
        let err = read(
            r#"{ "targets": { "server": { "entry": "s.ts", "out": "dist/server.js" } },
                 "start": { "devdir": "dist/server.js" } }"#,
        )
        .expect_err("refused");
        assert!(err.contains("overlaps"), "{err}");

        let err = read(
            r#"{ "targets": { "server": { "entry": "s.ts", "out": "dist/server.js" } },
                 "start": { "devdir": "dist" } }"#,
        )
        .expect_err("refused");
        assert!(err.contains("overlaps"), "{err}");

        let err = read(
            r#"{ "targets": { "web": { "entry": "index.html", "outdir": "dist" } },
                 "start": { "devdir": "dist/dev" } }"#,
        )
        .expect_err("refused");
        assert!(err.contains("overlaps"), "{err}");

        // And the three ways to leave the project or name nothing at all.
        for (devdir, needle) in [
            (".", "project root"),
            ("/tmp/dev", "absolute"),
            ("../shared", "escapes"),
        ] {
            let err = read(&format!(
                r#"{{ "targets": {{ "server": {{ "entry": "s.ts" }} }},
                     "start": {{ "devdir": "{devdir}" }} }}"#,
            ))
            .expect_err("refused");
            assert!(err.contains(needle), "{err}");
        }

        let empty = read(
            r#"{ "targets": { "server": { "entry": "s.ts" } },
                 "start": { "devdir": "" } }"#,
        )
        .expect_err("refused");
        assert!(empty.contains("empty"), "{empty}");
    }

    /// Permissions go through the flag parser, so the file cannot mean anything
    /// the command line does not.
    #[test]
    fn permissions_are_checked_by_the_flag_parser() {
        read(
            r#"{ "targets": { "a": { "entry": "a.ts" } },
                 "permissions": { "deny": ["all"], "allow": { "read": ["./data"], "listen": true } } }"#,
        )
        .expect("parsed");

        let unknown = read(
            r#"{ "targets": { "a": { "entry": "a.ts" } },
                 "permissions": { "deny": ["all"], "allow": { "filesystem": true } } }"#,
        )
        .expect_err("refused");
        assert!(unknown.contains("permissions"), "{unknown}");

        // A bare grant is the whole point after D65: the block states a deploy
        // grant, and a deployment starts from nothing.
        let project = read(
            r#"{ "targets": { "a": { "entry": "a.ts" } },
                 "permissions": { "allow": { "read": true } } }"#,
        )
        .expect("parsed");
        // ...and it is pinned to that mode on the way out, so `esdev start` —
        // whose own baseline is everything — spawns the child under the same
        // grant `esrun` would.
        assert_eq!(project.permissions, ["--deny-all", "--allow-read"]);

        // A denial with nothing granted is the flag parser's error, reported here.
        let ungrounded = read(
            r#"{ "targets": { "a": { "entry": "a.ts" } },
                 "permissions": { "deny": ["read"] } }"#,
        )
        .expect_err("refused");
        assert!(ungrounded.contains("requires --allow-all"), "{ungrounded}");

        // Which the file says as `"allow": {"all": true}`, the shape that means
        // "everything, minus these".
        read(
            r#"{ "targets": { "a": { "entry": "a.ts" } },
                 "permissions": { "deny": ["read"], "allow": { "all": true } } }"#,
        )
        .expect("parsed");
    }

    /// A document decides its own shape, so the keys that would decide it here
    /// are refused rather than quietly ignored.
    #[test]
    fn an_html_target_refuses_the_keys_a_document_already_answers() {
        let out = read(
            r#"{ "targets": { "web": { "entry": "index.html", "out": "dist/index.html" } } }"#,
        )
        .expect_err("refused");
        assert!(out.contains("`out` names one output"), "{out}");

        let platform =
            read(r#"{ "targets": { "web": { "entry": "index.html", "platform": "browser" } } }"#)
                .expect_err("refused");
        assert!(platform.contains("run in a browser"), "{platform}");

        let then = read(r#"{ "targets": { "web": { "entry": "index.html", "then": "run" } } }"#)
            .expect_err("refused");
        assert!(then.contains("nothing to execute"), "{then}");
    }

    /// A document's output is a directory however you look at it: the file, the
    /// bundles its scripts became, the chunks those split into.
    #[test]
    fn an_html_target_defaults_to_a_directory() {
        let project = read(r#"{ "targets": { "web": { "entry": "index.html" } } }"#).expect("ok");
        assert!(project.targets[0].is_html());
        assert!(matches!(&project.targets[0].output, Output::Dir(dir) if dir == "dist"));
    }

    #[test]
    fn a_file_with_no_targets_says_so() {
        // `start` builds and serves targets, so a file naming one without them
        // is incomplete — and so is a file that says nothing at all.
        let missing = read(r#"{ "start": { "run": "a" } }"#).expect_err("refused");
        assert!(missing.contains("no `targets`"), "{missing}");
        assert!(read(r#"{ }"#).is_err());

        let empty = read(r#"{ "targets": {} }"#).expect_err("refused");
        assert!(empty.contains("no targets"), "{empty}");
    }

    #[test]
    fn a_config_that_only_configures_tests_or_jsx_needs_no_targets() {
        let tested = read(r#"{ "test": { "jobs": 2 } }"#).expect("read");
        assert!(tested.targets.is_empty());
        assert_eq!(tested.test.jobs, Some(2));

        // A factory is the function the module already has; an import source is
        // the package the compiler imports one from. Which key is there says
        // which, so there is no mode to name.
        let called =
            read(r#"{ "jsx": { "factory": "h", "fragment": "Fragment" } }"#).expect("read");
        assert_eq!(
            called.jsx.function,
            Some(crate::transform::JsxFunction::InScope {
                factory: "h".to_string(),
                fragment: Some("Fragment".to_string()),
            })
        );

        let imported = read(r#"{ "jsx": { "importSource": "preact" } }"#).expect("read");
        assert_eq!(
            imported.jsx.function,
            Some(crate::transform::JsxFunction::Imported {
                source: "preact".to_string(),
            })
        );

        // Nothing said is nothing assumed.
        assert_eq!(read(r#"{ "test": {} }"#).expect("read").jsx.function, None);
    }

    #[test]
    fn a_test_section_names_its_global_setup_as_one_module_or_several() {
        let one = read(r#"{ "test": { "globalSetup": "./db.ts" } }"#).expect("read");
        assert_eq!(one.test.global_setup, ["./db.ts"]);
        let two =
            read(r#"{ "test": { "globalSetup": ["./db.ts", "./server.ts"] } }"#).expect("read");
        assert_eq!(two.test.global_setup, ["./db.ts", "./server.ts"]);
        assert!(
            read(r#"{ "test": {} }"#)
                .expect("read")
                .test
                .global_setup
                .is_empty()
        );
        let err = read(r#"{ "test": { "globalSetup": 1 } }"#).expect_err("refused");
        assert!(err.contains("globalSetup"), "{err}");
    }

    #[test]
    fn a_coverage_section_says_what_to_measure_and_how_much_is_enough() {
        let section = read(
            r#"{ "test": { "coverage": {
                "enabled": true, "include": ["src"], "exclude": ["src/gen"],
                "reporter": "json", "reportsDirectory": "out",
                "thresholds": { "lines": 90, "functions": -3 } } } }"#,
        )
        .expect("read")
        .test
        .coverage
        .expect("a coverage section");
        assert!(section.enabled);
        assert_eq!(section.settings.include, ["src"]);
        assert_eq!(section.settings.exclude, ["src/gen"]);
        assert_eq!(section.settings.reporters, ["json"]);
        assert_eq!(section.settings.directory, "out");
        assert_eq!(section.settings.thresholds.lines, Some(90.0));
        assert_eq!(section.settings.thresholds.functions, Some(-3.0));
        assert_eq!(section.settings.thresholds.branches, None);

        let defaults = read(r#"{ "test": { "coverage": {} } }"#)
            .expect("read")
            .test
            .coverage
            .unwrap();
        assert!(!defaults.enabled);
        assert_eq!(defaults.settings, crate::coverage::Settings::default());

        for (json, says) in [
            (
                r#"{ "test": { "coverage": { "reporter": "html" } } }"#,
                "`html` is not a coverage reporter",
            ),
            (
                r#"{ "test": { "coverage": { "thresholds": { "lines": 120 } } } }"#,
                "A percentage up to 100",
            ),
            (
                r#"{ "test": { "coverage": { "thresholds": { "rows": 1 } } } }"#,
                "unknown key `rows`",
            ),
            (
                r#"{ "test": { "coverage": { "enabled": "yes" } } }"#,
                "true or false",
            ),
        ] {
            let err = read(json).expect_err("refused");
            assert!(err.contains(says), "{json}: {err}");
        }
    }

    #[test]
    fn max_concurrency_is_a_whole_number_above_zero() {
        let settings = read(r#"{ "test": { "maxConcurrency": 8 } }"#)
            .expect("read")
            .test;
        assert_eq!(settings.max_concurrency, Some(8));
        let err = read(r#"{ "test": { "maxConcurrency": 0 } }"#).expect_err("refused");
        assert!(err.contains("above zero"), "{err}");
    }

    #[test]
    fn a_tags_section_defines_names_and_options() {
        let settings = read(
            r#"{ "test": { "strictTags": false, "tags": [
                { "name": "db", "description": "Database.", "timeout": 60000 },
                { "name": "flaky", "retry": 3, "priority": 1 } ] } }"#,
        )
        .expect("read")
        .test;
        assert_eq!(settings.strict_tags, Some(false));
        assert_eq!(settings.tags[0].name, "db");
        assert_eq!(settings.tags[0].timeout, Some(60000));
        assert_eq!(settings.tags[1].retry, Some(3));
        assert_eq!(settings.tags[1].priority, Some(1.0));
        assert_eq!(
            read(r#"{ "test": {} }"#).expect("read").test.strict_tags,
            None
        );
        for (json, says) in [
            (
                r#"{ "test": { "tags": [{ "name": "and" }] } }"#,
                "combines tags",
            ),
            (r#"{ "test": { "tags": [{ "name": "a b" }] } }"#, "spaces"),
            (
                r#"{ "test": { "tags": [{ "timeout": 1 }] } }"#,
                "needs a \"name\"",
            ),
            (
                r#"{ "test": { "tags": [{ "name": "x", "timeout": 0 }] } }"#,
                "above zero",
            ),
            (
                r#"{ "test": { "tags": [{ "name": "x", "skip": true }] } }"#,
                "unknown key `skip`",
            ),
            (r#"{ "test": { "tags": "db" } }"#, "An array"),
        ] {
            let err = read(json).expect_err("refused");
            assert!(err.contains(says), "{json}: {err}");
        }
    }

    #[test]
    fn a_test_section_names_its_browser() {
        use crate::browser::{Browser, Choice};
        assert_eq!(read(r#"{ "test": {} }"#).expect("read").test.browser, None);
        let auto = read(r#"{ "test": { "browser": "auto" } }"#).expect("read");
        assert_eq!(auto.test.browser, Some(Choice::Auto));
        let firefox = read(r#"{ "test": { "browser": "firefox" } }"#).expect("read");
        assert_eq!(firefox.test.browser, Some(Choice::Named(Browser::Firefox)));
        let several = read(r#"{ "test": { "browser": ["firefox", "edge"] } }"#).expect("read");
        assert_eq!(
            several.test.browser,
            Some(Choice::Several(vec![Browser::Firefox, Browser::Edge]))
        );
        let listed_auto =
            read(r#"{ "test": { "browser": ["auto", "edge"] } }"#).expect_err("refused");
        assert!(
            listed_auto.contains("cannot be one of several"),
            "{listed_auto}"
        );

        let unknown = read(r#"{ "test": { "browser": "opera" } }"#).expect_err("refused");
        assert!(unknown.contains("`test`'s `browser`"), "{unknown}");
        assert!(unknown.contains("`opera` is not a browser"), "{unknown}");
        let wrong = read(r#"{ "test": { "browser": true } }"#).expect_err("refused");
        assert!(
            wrong.contains("names the browser the test files run in"),
            "{wrong}"
        );
    }

    #[test]
    fn a_jsx_section_refuses_what_it_cannot_mean() {
        // The word every React-descended toolchain uses, answered with the two
        // keys that exist here rather than with a list.
        let runtime = read(r#"{ "jsx": { "runtime": "classic" } }"#).expect_err("refused");
        assert!(runtime.contains("not a key here"), "{runtime}");
        assert!(runtime.contains("importSource"), "{runtime}");

        let both = read(r#"{ "jsx": { "importSource": "preact", "factory": "h" } }"#)
            .expect_err("refused");
        assert!(
            both.contains("two ways to reach the same function"),
            "{both}"
        );

        let orphan = read(r#"{ "jsx": { "importSource": "preact", "fragment": "F" } }"#)
            .expect_err("refused");
        assert!(orphan.contains("goes with `jsx.factory`"), "{orphan}");

        let nothing = read(r#"{ "jsx": { } }"#).expect_err("refused");
        assert!(nothing.contains("says nothing"), "{nothing}");

        let empty = read(r#"{ "jsx": { "factory": "" } }"#).expect_err("refused");
        assert!(empty.contains("cannot be empty"), "{empty}");

        let unknown = read(r#"{ "jsx": { "pragma": "h" } }"#).expect_err("refused");
        assert!(unknown.contains("pragma"), "{unknown}");
    }

    #[test]
    fn invalid_json_says_it_is_data() {
        let err = read(r#"{ "targets": { /* a comment */ } }"#).expect_err("refused");
        assert!(err.contains("not valid JSON"), "{err}");
        assert!(err.contains("no comments"), "{err}");
    }
}
