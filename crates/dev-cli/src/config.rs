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

/// A parsed `esdev.json`.
#[derive(Debug)]
pub struct Project {
    /// The directory the file was found in.
    ///
    /// Every path in the file is relative to *this*, not to the working
    /// directory — so a config describes its own project the same way whether
    /// esdev was run from the project root or pointed at it from elsewhere.
    pub dir: PathBuf,
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
    /// `"refresh": "react"` — the framework whose hot-reload scheme this
    /// target's modules should be prepared for, applied in the dev loop only.
    ///
    /// A name rather than a boolean because the schemes are not one thing:
    /// React's registers components and matches hook signatures, and another
    /// framework's would do something else entirely. Only `"react"` is
    /// implemented; an unknown name is refused rather than ignored.
    pub refresh: Option<String>,
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
fn is_html_entry(entry: &str) -> bool {
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
    "define",
    "conditions",
    "then",
    "refresh",
];

/// The keys the file may carry at the top level.
const TOP_LEVEL_KEYS: &[&str] = &["$schema", "targets", "start", "permissions", "plugins"];

/// The keys `start` may carry.
///
/// Read in full here, and *consumed* by `esdev start`. Validating a key the
/// command that uses it has not been written yet is deliberate: a typo in
/// `start` should be reported by the build that read the file, not held until
/// the day somebody runs the other command.
const START_KEYS: &[&str] = &["run", "watch", "serve", "port"];

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
    // to be — which is the same place only by coincidence.
    let dir = dir
        .canonicalize()
        .map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
    parse(&text, dir, &path.display().to_string())
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

    let targets = root.get("targets").ok_or_else(|| {
        format!(
            "{name} has no `targets`.\n\n\
             A target is one thing the project builds — an entry, and where its \
             output goes:\n\n  \
             \"targets\": {{ \"server\": {{ \"entry\": \"src/server.ts\", \"out\": \"dist/server.js\" }} }}"
        )
    })?;
    let targets = object(targets, name, "`targets`")?;
    if targets.is_empty() {
        return Err(format!(
            "{name} has no targets in `targets`.\n\n\
             An empty object builds nothing; remove the file or name what it builds."
        ));
    }
    // The project's own plugins load first, and every target gets them. A
    // target's list adds to that rather than replacing it: a project that
    // compiles `.mdx` compiles it for the server bundle and the browser one,
    // and a config where naming one extra plugin silently dropped the shared
    // ones would be a build that differs between targets for no stated reason.
    let mut plugins = plugin_specs(root.get("plugins"), name, "`plugins`")?;
    let shared: Vec<usize> = (0..plugins.len()).collect();

    let mut targets = targets
        .iter()
        .map(|(target_name, value)| target(target_name, value, name, &shared, &mut plugins))
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
    Ok(Some(Project {
        dir,
        targets,
        start,
        permissions,
        plugins,
    }))
}

/// Parses one entry of `targets`.
fn target(
    name: &str,
    value: &Value,
    file: &str,
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

    let built = Target {
        name: name.to_string(),
        entry,
        output,
        platform,
        assets: string_array(map.get("assets"), file, &format!("{at}'s `assets`"))?,
        minify: flag(map.get("minify"), file, &format!("{at}'s `minify`"))?,
        define: defines(map.get("define"), file, &at)?,
        conditions: string_array(map.get("conditions"), file, &format!("{at}'s `conditions`"))?,
        refresh: refresh(map.get("refresh"), file, &at, !mine.is_empty())?,
        plugins: mine,
        run_after_build,
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
    Ok(Start {
        run,
        watch,
        serve,
        port,
    })
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

/// The scheme esdev implements itself. Others come from plugins.
pub const BUILT_IN_REFRESH: &str = "react";

/// `refresh`, checked against what can actually implement it.
///
/// A name is refused rather than ignored, because a name that is quietly
/// dropped is a project whose components stop keeping their state one day, with
/// the reason sitting unread in a config file.
///
/// **But esdev is no longer the only thing that can implement one.** `"react"`
/// is built in ([`crate::refresh`]) and was for a while the only name allowed,
/// which meant every framework that was not React got a full page reload on
/// every edit while the React template kept its state — not because the
/// mechanism was missing (`import.meta.hot` is the same for everyone) but
/// because the config would not let the target say it had a scheme. A plugin
/// can now write the per-module half itself, against the same
/// [`Pass`](crate::contract::Pass) `react-refresh` uses.
///
/// So an unknown name is accepted **when the target has plugins**, and refused
/// when it does not — where there is nothing that could implement it, the name
/// is a typo, and saying so is the whole point of checking.
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
    if name != BUILT_IN_REFRESH && !has_plugins {
        return Err(format!(
            "{file}: {at}'s `refresh` is \"{name}\", and the only scheme esdev \
             implements is \"{BUILT_IN_REFRESH}\".\n\n\
             It names the framework whose hot-reload convention this target's modules \
             are prepared for — React's registers components and matches hook \
             signatures so a component keeps its state across an edit. Another \
             framework's is a `plugins` entry: a transform against the same contract, \
             and the name here is what tells it the dev loop is hot."
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

    /// `refresh` names a scheme, and esdev implements one. A name it does not
    /// know is a typo when nothing could implement it — and a plugin's job when
    /// the target has plugins, which is what stopped every non-React framework
    /// from having a hot loop at all.
    #[test]
    fn an_unknown_refresh_scheme_needs_a_plugin_that_could_implement_it() {
        let refused =
            read(r#"{ "targets": { "web": { "entry": "index.html", "refresh": "otfw" } } }"#)
                .expect_err("no plugin to implement it");
        assert!(
            refused.contains("only scheme esdev implements"),
            "{refused}"
        );

        let accepted = read(
            r#"{ "plugins": ["./plugins/otfw.js"],
                 "targets": { "web": { "entry": "index.html", "refresh": "otfw" } } }"#,
        )
        .expect("a plugin can implement it");
        assert_eq!(accepted.targets[0].refresh.as_deref(), Some("otfw"));

        // The built-in still needs nothing.
        let react =
            read(r#"{ "targets": { "web": { "entry": "index.html", "refresh": "react" } } }"#)
                .expect("the built-in scheme");
        assert_eq!(react.targets[0].refresh.as_deref(), Some("react"));
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
        let missing = read(r#"{ "start": { "run": "a" } }"#).expect_err("refused");
        assert!(missing.contains("no `targets`"), "{missing}");

        let empty = read(r#"{ "targets": {} }"#).expect_err("refused");
        assert!(empty.contains("no targets"), "{empty}");
    }

    #[test]
    fn invalid_json_says_it_is_data() {
        let err = read(r#"{ "targets": { /* a comment */ } }"#).expect_err("refused");
        assert!(err.contains("not valid JSON"), "{err}");
        assert!(err.contains("no comments"), "{err}");
    }
}
