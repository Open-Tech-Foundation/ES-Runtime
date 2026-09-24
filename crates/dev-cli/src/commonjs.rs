//! CommonJS packages, converted for a run that is not bundled.
//!
//! The runtime loads ES module packages only (D22), and `esdev build` converts
//! CommonJS on the way in. A module `esdev` runs **unbundled** — a test file,
//! `esdev app.ts` — had neither, so a project whose dependency ships CommonJS
//! built and ran, and could not be tested. This converts such a package, once,
//! the first time something asks for it, with the bundler `esdev build` uses.
//!
//! # What is made
//!
//! Three files per package, under the project's `node_modules/.esdev/deps/`,
//! in a directory for the install ([`deps_dir`]):
//!
//! ```text
//!   intl-messageformat-3f2a….mjs        the module an import gets
//!   intl-messageformat-3f2a….body.mjs   the package, bundled by rolldown
//!   intl-messageformat-3f2a….deps.mjs   what the package `require`s from others
//! ```
//!
//! The module an import gets is what makes `import { IntlMessageFormat } from
//! "intl-messageformat"` link: an ES module has to *state* its exports, and a
//! CommonJS module only assigns them at run time. The names are read from the
//! source the way Node's own loader reads them (`exports.x =`,
//! `Object.defineProperty(exports, "x", …)`, a re-export of another file), and
//! the default export is what a bundler would make it — `module.exports`, or
//! its `default` when the package says it was compiled from ES modules.
//!
//! # One copy of each package
//!
//! Each package is bundled **alone**. Another package it `require`s is not put
//! in its bundle: react-dom carrying a react of its own would be two Reacts in
//! one program, and hooks would break in a way nothing names. So a `require` of
//! another package reads what `.deps.mjs` imported — that package's own
//! converted module, or the ES module the runtime would have loaded anyway, by
//! the same URL, so it is the same instance.
//!
//! # Why resolving builds nothing
//!
//! `mock.module("pkg", …)` and `import.meta.resolve` resolve synchronously and
//! have to reach the id an `import` does. So [`resolve`](PackageConverter::resolve)
//! only names the file, deterministically, and [`prepare`](PackageConverter::prepare)
//! makes it the first time it is loaded. The name carries a hash of the package,
//! and the directory one of the project's lockfile, so an install is a new name
//! rather than a stale file, and every process — each test file is one — agrees
//! on it. The first conversion of a new install removes the old install's.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use es_runtime_cli_common::run::PackageConverter;
use rolldown::plugin::{
    HookLoadArgs, HookLoadOutput, HookLoadReturn, HookResolveIdArgs, HookResolveIdOutput,
    HookResolveIdReturn, HookUsage, Plugin, PluginContext, SharedLoadPluginContext,
};
use rolldown_common::{ImportKind, ResolvedExternal};

/// Changes when what this module writes changes, so an older conversion is not
/// reused by a newer `esdev`.
const FORMAT: &str = concat!("esdev-commonjs-1-", env!("CARGO_PKG_VERSION"));

/// Where a converted package's `require`s of other packages read from.
const REGISTRY: &str = r#"(globalThis[Symbol.for("esdev.commonjs")] ??= new Map())"#;

/// The bundle's entry: the package, as `require` would return it.
const ENTRY: &str = "\0esdev-commonjs-entry";
const REQUIRE: &str = "\0esdev-require:";
const MISSING: &str = "\0esdev-missing:";

/// One package, and where its converted module goes.
#[derive(Clone, Debug)]
struct Plan {
    /// As it was imported, for messages.
    specifier: String,
    /// The file the package resolved to.
    entry: PathBuf,
    /// The module an import gets. The other two files sit beside it.
    module: PathBuf,
}

impl Plan {
    fn sibling(&self, suffix: &str) -> PathBuf {
        let stem = self
            .module
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.module.with_file_name(format!("{stem}.{suffix}.mjs"))
    }
}

/// Converts the CommonJS packages a run asks for.
pub struct Converter {
    resolver: oxc_resolver::Resolver,
    /// Answers already given, by specifier and the directory it was asked from.
    answered: Mutex<HashMap<(String, PathBuf), Option<Plan>>>,
    /// What each named module stands for.
    planned: Mutex<HashMap<PathBuf, Plan>>,
}

/// The converter a run installs. One per process: what it has resolved and
/// planned is worth keeping between the runs a watch loop starts.
pub fn converter() -> Arc<dyn PackageConverter> {
    static ONE: OnceLock<Arc<Converter>> = OnceLock::new();
    ONE.get_or_init(|| Arc::new(Converter::new())).clone()
}

impl Converter {
    fn new() -> Converter {
        let mut conditions = crate::resolve::conditions(crate::resolve::Target::Server, Vec::new());
        // `import` first in intent, but a package that offers only `require`
        // is exactly the one this is for — and a manifest's own order, not
        // this list's, decides which of them wins.
        conditions.extend(["import", "require", "module", "default"].map(String::from));
        Converter {
            resolver: oxc_resolver::Resolver::new(oxc_resolver::ResolveOptions {
                condition_names: conditions,
                main_fields: crate::resolve::main_fields(crate::resolve::Target::Server)
                    .unwrap_or_else(|| vec!["module".to_string(), "main".to_string()]),
                extensions: [".js", ".cjs", ".mjs", ".json"].map(String::from).to_vec(),
                ..oxc_resolver::ResolveOptions::default()
            }),
            answered: Mutex::default(),
            planned: Mutex::default(),
        }
    }

    /// The package `specifier` names from `dir`, when it is one to convert.
    fn plan(&self, specifier: &str, dir: &Path) -> Option<Plan> {
        if !is_package(specifier) {
            return None;
        }
        let key = (specifier.to_string(), dir.to_path_buf());
        if let Some(answer) = self.answered.lock().ok()?.get(&key) {
            return answer.clone();
        }
        let answer = self.plan_uncached(specifier, dir);
        if let Ok(mut answered) = self.answered.lock() {
            answered.insert(key, answer.clone());
        }
        if let Some(plan) = &answer
            && let Ok(mut planned) = self.planned.lock()
        {
            planned.insert(plan.module.clone(), plan.clone());
        }
        answer
    }

    fn plan_uncached(&self, specifier: &str, dir: &Path) -> Option<Plan> {
        let entry = self.resolver.resolve(dir, specifier).ok()?.full_path();
        let entry = dunce::simplified(&entry).to_path_buf();
        if !is_commonjs(&entry) {
            return None;
        }
        let deps = deps_dir(&entry)?;
        // The first conversion of a new install clears the previous ones.
        let fresh = !deps.exists();
        std::fs::create_dir_all(&deps).ok()?;
        if fresh {
            prune_other_installs(&deps);
        }
        let deps = dunce::canonicalize(&deps).ok()?;
        let name = format!("{}-{}.mjs", slug(specifier), fingerprint(&entry));
        Some(Plan {
            specifier: specifier.to_string(),
            entry,
            module: deps.join(name),
        })
    }

    fn planned(&self, module: &Path) -> Option<Plan> {
        self.planned.lock().ok()?.get(module).cloned()
    }
}

impl PackageConverter for Converter {
    fn resolve(&self, specifier: &str, referrer: &str) -> Option<String> {
        let plan = self.plan(specifier, &referrer_dir(referrer))?;
        url::Url::from_file_path(&plan.module)
            .ok()
            .map(|url| url.to_string())
    }

    fn prepare(&self, id: &str) -> Option<es_runtime_providers::BoxFuture<Result<(), String>>> {
        let path = url::Url::parse(id).ok()?.to_file_path().ok()?;
        let plan = self.planned(&path)?;
        if plan.module.is_file() {
            return Some(Box::pin(std::future::ready(Ok(()))));
        }
        // On a thread of its own: the bundler runs its own tasks, and the run
        // asking for this is in the middle of driving an isolate on this one.
        let (tx, rx) = tokio::sync::oneshot::channel();
        let spawned = std::thread::Builder::new()
            .name("esdev-commonjs".to_string())
            .spawn(move || {
                let converted = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| format!("cannot start the CommonJS conversion: {e}"))
                    .and_then(|runtime| runtime.block_on(convert_all(plan, &mut HashSet::new())));
                let _ = tx.send(converted);
            });
        Some(Box::pin(async move {
            spawned.map_err(|e| format!("cannot start the CommonJS conversion: {e}"))?;
            rx.await
                .unwrap_or_else(|_| Err("the CommonJS conversion stopped".to_string()))
        }))
    }
}

/// Converts `plan`, then every CommonJS package it `require`s, so everything
/// its `.deps.mjs` imports is there before anything reads it.
fn convert_all<'a>(
    plan: Plan,
    visiting: &'a mut HashSet<PathBuf>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + 'a>> {
    Box::pin(async move {
        // In this process, one conversion at a time: two test files asking for
        // react at once should convert it once.
        static ONE_AT_A_TIME: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
        if plan.module.is_file() || !visiting.insert(plan.module.clone()) {
            return Ok(());
        }
        let others = {
            let _held = ONE_AT_A_TIME.lock().await;
            if plan.module.is_file() {
                Vec::new()
            } else {
                convert(&plan).await.map_err(|e| {
                    format!(
                        "cannot convert the CommonJS package {:?} ({}) to an ES module:\n{e}",
                        plan.specifier,
                        plan.entry.display()
                    )
                })?
            }
        };
        for other in others {
            convert_all(other, visiting).await?;
        }
        Ok(())
    })
}

/// What one converted package `require`s from outside itself.
#[derive(Clone, Debug)]
enum Other {
    /// An ES module package, by the URL the runtime loads it by.
    Module(String),
    /// A CommonJS package, by its own converted module.
    Converted(Plan),
    /// Nothing that loads here — a `node:` builtin, a package not installed.
    /// Refused when it is required rather than now, since a package often
    /// requires one only on a branch that never runs here.
    Missing,
}

/// Bundles one package and writes its three files. Returns the CommonJS
/// packages it requires, which have to be converted too.
async fn convert(plan: &Plan) -> Result<Vec<Plan>, String> {
    let names = exported_names(&plan.entry);
    let required = Arc::new(Mutex::new(Vec::<(String, Other)>::new()));
    let staging =
        plan.module
            .with_file_name(format!(".staging-{}-{}", std::process::id(), unique()));
    let package = plan.entry.parent().unwrap_or(Path::new(".")).to_path_buf();
    let options = crate::bundler::Options {
        cwd: Some(package),
        input: vec![(Some("body".to_string()), ENTRY.to_string())],
        platform: crate::resolve::Target::Server,
        define: vec![(
            "process.env.NODE_ENV".to_string(),
            "\"development\"".to_string(),
        )],
        output: crate::bundler::OutputOptions {
            format: Some("esm".to_string()),
            dir: Some(staging.to_string_lossy().into_owned()),
            entry_filenames: Some("[name].mjs".to_string()),
            code_splitting: Some(false),
            ..crate::bundler::OutputOptions::default()
        },
        ..crate::bundler::Options::default()
    };
    let translated = crate::bundler::translate(&options, options.output.clone(), None)?;
    let mut bundler = rolldown::BundlerBuilder::default()
        .with_options(translated)
        .with_plugins(vec![Arc::new(Packaging {
            entry: plan.entry.clone(),
            required: required.clone(),
        }) as Arc<dyn rolldown::plugin::Pluginable>])
        .build()
        .map_err(|e| crate::bundler::report(&crate::failures!(e)))?;
    let written = bundler.write().await;
    let written = written.map_err(|e| crate::bundler::report(&crate::failures!(e)));
    let body = written.and_then(|_| {
        std::fs::read_to_string(staging.join("body.mjs"))
            .map_err(|e| format!("the bundle was not written: {e}"))
    });
    let _ = std::fs::remove_dir_all(&staging);
    let body = body?;

    let required = required.lock().map(|r| r.clone()).unwrap_or_default();
    let deps = registration(&required);
    let module = facade(plan, &names, !required.is_empty());

    // The module an import gets goes last: its being there is what says the
    // other two are.
    if !required.is_empty() {
        place(&plan.sibling("deps"), &deps)?;
    }
    place(&plan.sibling("body"), &body)?;
    place(&plan.module, &module)?;
    Ok(required
        .into_iter()
        .filter_map(|(_, other)| match other {
            Other::Converted(plan) => Some(plan),
            _ => None,
        })
        .collect())
}

/// The module an import gets.
fn facade(plan: &Plan, names: &[String], has_deps: bool) -> String {
    let stem = plan
        .module
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut out = format!(
        "// {:?}, converted from CommonJS by esdev for a run that is not bundled.\n",
        plan.specifier
    );
    if has_deps {
        out.push_str(&format!("import \"./{stem}.deps.mjs\";\n"));
    }
    out.push_str(&format!("import __cjs from \"./{stem}.body.mjs\";\n"));
    // What a bundler makes the default: `module.exports`, unless the package
    // says it was compiled from ES modules, in which case its own `default`.
    out.push_str(
        "const __default = __cjs != null && __cjs.__esModule ? __cjs.default : __cjs;\n\
         export { __cjs as \"module.exports\", __default as default };\n",
    );
    for (index, name) in names.iter().enumerate() {
        let quoted = serde_json::Value::String(name.clone());
        out.push_str(&format!(
            "const __e{index} = __cjs == null ? undefined : __cjs[{quoted}];\n\
             export {{ __e{index} as {quoted} }};\n"
        ));
    }
    out
}

/// `.deps.mjs`: imports what the package requires from other packages, and
/// leaves each where the package's `require` reads it.
fn registration(required: &[(String, Other)]) -> String {
    let mut out = format!("const registry = {REGISTRY};\n");
    for (index, (key, other)) in required.iter().enumerate() {
        let url = match other {
            Other::Module(url) => url.clone(),
            Other::Converted(plan) => match url::Url::from_file_path(&plan.module) {
                Ok(url) => url.to_string(),
                Err(()) => continue,
            },
            Other::Missing => continue,
        };
        let quoted_url = serde_json::Value::String(url);
        let quoted_key = serde_json::Value::String(key.clone());
        out.push_str(&format!(
            "import * as __d{index} from {quoted_url};\n\
             registry.set({quoted_key}, \"module.exports\" in __d{index} ? __d{index}[\"module.exports\"] : __d{index});\n"
        ));
    }
    out
}

/// Writes `path` whole or not at all: another process may be converting the
/// same package, and may read it the moment it exists.
fn place(path: &Path, contents: &str) -> Result<(), String> {
    let staged = path.with_extension(format!("tmp-{}-{}", std::process::id(), unique()));
    std::fs::write(&staged, contents)
        .map_err(|e| format!("cannot write {}: {e}", staged.display()))?;
    std::fs::rename(&staged, path).map_err(|e| {
        let _ = std::fs::remove_file(&staged);
        format!("cannot write {}: {e}", path.display())
    })
}

fn unique() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// The bundler plugin: the package's own files go in, other packages stay out.
#[derive(Debug)]
struct Packaging {
    entry: PathBuf,
    required: Arc<Mutex<Vec<(String, Other)>>>,
}

impl Packaging {
    /// What `specifier`, written in `importer`, is — decided once per pair.
    fn other(&self, specifier: &str, importer: &Path) -> (String, Other) {
        let dir = importer.parent().unwrap_or(Path::new("/"));
        let key = format!("{}\0{specifier}", dir.display());
        if let Ok(required) = self.required.lock()
            && let Some((_, other)) = required.iter().find(|(k, _)| *k == key)
        {
            return (key, other.clone());
        }
        let other = classify(specifier, importer);
        if let Ok(mut required) = self.required.lock() {
            required.push((key.clone(), other.clone()));
        }
        (key, other)
    }
}

/// Where the runtime would find `specifier` from `importer`, or which package
/// converting it would be.
fn classify(specifier: &str, importer: &Path) -> Other {
    let dir = importer.parent().unwrap_or(Path::new("/"));
    if specifier.starts_with("node:") || !is_package(specifier) {
        return Other::Missing;
    }
    // The runtime's own answer, so this is the instance everything else in the
    // run imports.
    let referrer = url::Url::from_file_path(importer)
        .map(|url| url.to_string())
        .unwrap_or_default();
    if let Ok(loader) = es_runtime_default_providers::NodeModuleLoader::with_base_dir(dir)
        && let Some(Ok(url)) =
            es_runtime_providers::ModuleLoader::resolve_sync(&loader, specifier, &referrer)
    {
        return Other::Module(url);
    }
    match ONE_CONVERTER
        .get_or_init(Converter::new)
        .plan(specifier, dir)
    {
        Some(plan) => Other::Converted(plan),
        None => Other::Missing,
    }
}

/// The converter a conversion plans the packages it requires with. Its own,
/// rather than the run's: it runs on a thread of its own, and a plan is only
/// a name — the run plans the same names when it is asked.
static ONE_CONVERTER: OnceLock<Converter> = OnceLock::new();

impl Plugin for Packaging {
    fn name(&self) -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("esdev:commonjs")
    }

    fn register_hook_usage(&self) -> HookUsage {
        HookUsage::ResolveId | HookUsage::Load
    }

    async fn resolve_id(
        &self,
        _ctx: &PluginContext,
        args: &HookResolveIdArgs<'_>,
    ) -> HookResolveIdReturn {
        if args.specifier == ENTRY {
            return Ok(Some(HookResolveIdOutput::from_id(ENTRY)));
        }
        let Some(importer) = args.importer.filter(|importer| !importer.starts_with('\0')) else {
            return Ok(None);
        };
        // The package's own files, by path, are bundled as usual.
        if !is_package(args.specifier) && !args.specifier.starts_with("node:") {
            return Ok(None);
        }
        let (key, other) = self.other(args.specifier, Path::new(importer));
        if matches!(args.kind, ImportKind::Require) {
            let id = match other {
                Other::Missing => format!("{MISSING}{}", args.specifier),
                _ => format!("{REQUIRE}{key}"),
            };
            return Ok(Some(HookResolveIdOutput::from_id(id)));
        }
        // An `import` of another package — a package that ships both kinds of
        // module — stays an import, of the file the runtime would load.
        Ok(match other {
            Other::Module(url) => Some(HookResolveIdOutput {
                external: Some(ResolvedExternal::Bool(true)),
                ..HookResolveIdOutput::from_id(url)
            }),
            Other::Converted(plan) => {
                url::Url::from_file_path(&plan.module)
                    .ok()
                    .map(|url| HookResolveIdOutput {
                        external: Some(ResolvedExternal::Bool(true)),
                        ..HookResolveIdOutput::from_id(url.to_string())
                    })
            }
            Other::Missing => None,
        })
    }

    async fn load(&self, _ctx: SharedLoadPluginContext, args: &HookLoadArgs<'_>) -> HookLoadReturn {
        let code = if args.id == ENTRY {
            let entry = serde_json::Value::String(self.entry.to_string_lossy().into_owned());
            format!("module.exports = require({entry});\n")
        } else if let Some(key) = args.id.strip_prefix(REQUIRE) {
            let key = serde_json::Value::String(key.to_string());
            format!("module.exports = {REGISTRY}.get({key});\n")
        } else if let Some(specifier) = args.id.strip_prefix(MISSING) {
            let message = serde_json::Value::String(format!(
                "this package requires {specifier:?}, which cannot be loaded here — \
                 node: builtins and packages that are not installed are not available \
                 to a CommonJS package esdev converted"
            ));
            format!("throw new Error({message});\n")
        } else {
            return Ok(None);
        };
        Ok(Some(HookLoadOutput {
            code: code.into(),
            ..HookLoadOutput::default()
        }))
    }
}

/// A package name, rather than a path, a URL or a `#private` import.
fn is_package(specifier: &str) -> bool {
    !(specifier.is_empty()
        || specifier.starts_with('.')
        || specifier.starts_with('/')
        || specifier.starts_with('\\')
        || specifier.starts_with('#')
        || specifier.contains(':'))
}

/// CommonJS by the same rule the runtime refuses it by: `.cjs`, or `.js`
/// outside a `"type": "module"` package.
fn is_commonjs(path: &Path) -> bool {
    match path.extension().and_then(|e| e.to_str()) {
        Some("cjs") => true,
        Some("js") => {
            nearest_manifest(path)
                .and_then(|manifest| std::fs::read_to_string(manifest).ok())
                .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
                .and_then(|json| {
                    json.get("type")
                        .and_then(|t| t.as_str())
                        .map(str::to_string)
                })
                .as_deref()
                != Some("module")
        }
        _ => false,
    }
}

fn nearest_manifest(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .skip(1)
        .map(|dir| dir.join("package.json"))
        .find(|manifest| manifest.is_file())
}

/// Where `entry`'s package is converted: `node_modules/.esdev/deps/<install>/`
/// of the project that installed it — the outermost `node_modules` above it,
/// so every package a project has, pnpm's store included, converts into one
/// place.
///
/// `<install>` is a hash of the project's lockfile. An install is what makes
/// a conversion stale — a package's `.deps.mjs` names the packages it requires,
/// by where they were installed — so one directory holds exactly one install's
/// conversions, and [`prune_other_installs`] removes the rest when a new one
/// starts. Pruning by package instead would be wrong: a project can hold two
/// copies of one package, both converted, both in use.
fn deps_dir(entry: &Path) -> Option<PathBuf> {
    let outermost = entry
        .ancestors()
        .filter(|dir| dir.file_name().is_some_and(|name| name == "node_modules"))
        .last()
        .map(Path::to_path_buf);
    let modules = match outermost {
        Some(modules) => modules,
        // A linked workspace package, outside any `node_modules`.
        None => nearest_manifest(entry)?.parent()?.join("node_modules"),
    };
    let install = install_hash(modules.parent()?);
    Some(modules.join(".esdev").join("deps").join(install))
}

/// The lockfile beside a project's `node_modules`, hashed: what names one
/// install. A project with none has one install as far as this can tell.
fn install_hash(project: &Path) -> String {
    use sha1::{Digest, Sha1};
    let mut hash = Sha1::new();
    hash.update(FORMAT.as_bytes());
    for lockfile in [
        "pnpm-lock.yaml",
        "package-lock.json",
        "yarn.lock",
        "bun.lock",
        "bun.lockb",
    ] {
        if let Ok(bytes) = std::fs::read(project.join(lockfile)) {
            hash.update(lockfile.as_bytes());
            hash.update(&bytes);
        }
    }
    hash.finalize()
        .iter()
        .take(6)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Removes the conversions of every install but `current`'s: they name
/// packages where an earlier install put them, and nothing will load them
/// again.
fn prune_other_installs(current: &Path) {
    let (Some(deps), Some(mine)) = (current.parent(), current.file_name()) else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(deps) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_name() != mine && path.is_dir() {
            let _ = std::fs::remove_dir_all(&path);
        } else if path.is_file() {
            // A conversion from before installs had directories of their own.
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// A file name for a specifier: `@scope/pkg/sub` → `scope__pkg__sub`.
fn slug(specifier: &str) -> String {
    specifier
        .trim_start_matches('@')
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect::<String>()
        .replace('_', "__")
}

/// What decides whether a conversion is still the right one within one
/// install: the file, its package's manifest, and this `esdev`.
fn fingerprint(entry: &Path) -> String {
    use sha1::{Digest, Sha1};
    let mut hash = Sha1::new();
    hash.update(FORMAT.as_bytes());
    hash.update(entry.to_string_lossy().as_bytes());
    if let Ok(meta) = std::fs::metadata(entry) {
        hash.update(meta.len().to_le_bytes());
        if let Ok(modified) = meta.modified()
            && let Ok(since) = modified.duration_since(std::time::UNIX_EPOCH)
        {
            hash.update(since.as_nanos().to_le_bytes());
        }
    }
    if let Some(manifest) = nearest_manifest(entry)
        && let Ok(bytes) = std::fs::read(manifest)
    {
        hash.update(&bytes);
    }
    hash.finalize()
        .iter()
        .take(6)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The directory an import was written in.
fn referrer_dir(referrer: &str) -> PathBuf {
    url::Url::parse(referrer)
        .ok()
        .filter(|url| url.scheme() == "file")
        .and_then(|url| url.to_file_path().ok())
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_default()
}

/// The names a CommonJS module exports, read the way Node's loader reads them
/// for an `import` of one: from the source, without running it.
///
/// Follows a module that re-exports another of the package's files
/// (`module.exports = require("./x")`, `__exportStar(require("./x"), exports)`)
/// and, for a file written as an ES module, its `export` declarations. What
/// only exists at run time — keys copied in a loop — is not found, and is still
/// reachable through the default export.
pub fn exported_names(entry: &Path) -> Vec<String> {
    let mut names = BTreeSet::new();
    let mut seen = HashSet::new();
    collect(entry, &mut names, &mut seen);
    names
        .into_iter()
        .filter(|name| !matches!(name.as_str(), "default" | "__esModule" | "module.exports"))
        .collect()
}

fn collect(path: &Path, names: &mut BTreeSet<String>, seen: &mut HashSet<PathBuf>) {
    if seen.len() > 256 || !seen.insert(path.to_path_buf()) {
        return;
    }
    let Ok(source) = std::fs::read_to_string(path) else {
        return;
    };
    let allocator = oxc::allocator::Allocator::default();
    let parsed =
        oxc::parser::Parser::new(&allocator, &source, oxc::span::SourceType::unambiguous()).parse();
    let dir = path.parent().unwrap_or(Path::new("/"));
    let mut found = Found::default();
    {
        use oxc::ast_visit::Visit;
        found.visit_program(&parsed.program);
    }
    names.extend(found.names);
    for relative in found.reexports {
        if let Some(file) = relative_file(dir, &relative) {
            collect(&file, names, seen);
        }
    }
}

/// A relative specifier inside the package, as the file it names.
fn relative_file(dir: &Path, specifier: &str) -> Option<PathBuf> {
    if !(specifier.starts_with("./") || specifier.starts_with("../")) {
        return None;
    }
    let base = dir.join(specifier);
    let mut candidates = vec![base.clone()];
    for extension in ["js", "cjs", "mjs"] {
        let mut name = base.clone().into_os_string();
        name.push(format!(".{extension}"));
        candidates.push(PathBuf::from(name));
        candidates.push(base.join(format!("index.{extension}")));
    }
    candidates.into_iter().find(|path| path.is_file())
}

#[derive(Default)]
struct Found {
    names: Vec<String>,
    reexports: Vec<String>,
}

use oxc::ast::ast::{
    Argument, AssignmentExpression, AssignmentTarget, CallExpression, Declaration,
    ExportAllDeclaration, ExportDeclaration, ExportFromDeclaration, ExportNamedDeclaration,
    Expression, ObjectPropertyKind, PropertyKey,
};

/// `exports` or `module.exports`.
fn is_exports(expression: &Expression<'_>) -> bool {
    match expression {
        Expression::Identifier(id) => id.name == "exports",
        Expression::StaticMemberExpression(member) => {
            member.property.name == "exports"
                && matches!(&member.object, Expression::Identifier(id) if id.name == "module")
        }
        _ => false,
    }
}

/// `require("./x")`'s specifier.
fn required(expression: &Expression<'_>) -> Option<String> {
    let Expression::CallExpression(call) = expression else {
        return None;
    };
    let Expression::Identifier(callee) = &call.callee else {
        return None;
    };
    if callee.name != "require" || call.arguments.len() != 1 {
        return None;
    }
    match &call.arguments[0] {
        Argument::StringLiteral(text) => Some(text.value.to_string()),
        _ => None,
    }
}

fn key_name(key: &PropertyKey<'_>) -> Option<String> {
    match key {
        PropertyKey::StaticIdentifier(id) => Some(id.name.to_string()),
        PropertyKey::StringLiteral(text) => Some(text.value.to_string()),
        _ => None,
    }
}

impl Found {
    /// `module.exports = { a, b: …, ...require("./x") }`, or `require("./x")`.
    fn assigned_whole(&mut self, value: &Expression<'_>) {
        if let Some(specifier) = required(value) {
            self.reexports.push(specifier);
            return;
        }
        if let Expression::ObjectExpression(object) = value {
            for property in &object.properties {
                match property {
                    ObjectPropertyKind::ObjectProperty(property) => {
                        if let Some(name) = key_name(&property.key) {
                            self.names.push(name);
                        }
                    }
                    ObjectPropertyKind::SpreadProperty(spread) => {
                        if let Some(specifier) = required(&spread.argument) {
                            self.reexports.push(specifier);
                        }
                    }
                }
            }
        }
    }
}

impl<'a> oxc::ast_visit::Visit<'a> for Found {
    fn visit_assignment_expression(&mut self, it: &AssignmentExpression<'a>) {
        match &it.left {
            // exports.x = … / module.exports.x = …
            AssignmentTarget::StaticMemberExpression(member) if is_exports(&member.object) => {
                self.names.push(member.property.name.to_string());
            }
            // exports["x"] = …
            AssignmentTarget::ComputedMemberExpression(member) if is_exports(&member.object) => {
                if let Expression::StringLiteral(text) = &member.expression {
                    self.names.push(text.value.to_string());
                }
            }
            // module.exports = …
            AssignmentTarget::StaticMemberExpression(member)
                if member.property.name == "exports"
                    && matches!(&member.object, Expression::Identifier(id) if id.name == "module") =>
            {
                self.assigned_whole(&it.right);
            }
            _ => {}
        }
        oxc::ast_visit::walk::walk_assignment_expression(self, it);
    }

    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        let callee = match &it.callee {
            Expression::Identifier(id) => Some(id.name.as_str()),
            Expression::StaticMemberExpression(member) => Some(member.property.name.as_str()),
            _ => None,
        };
        let argument = |index: usize| it.arguments.get(index).and_then(Argument::as_expression);
        match callee {
            // Object.defineProperty(exports, "x", …)
            Some("defineProperty") if argument(0).is_some_and(is_exports) => {
                if let Some(Expression::StringLiteral(text)) = argument(1) {
                    self.names.push(text.value.to_string());
                }
            }
            // TypeScript's and Babel's re-export helpers:
            // __exportStar(require("./x"), exports), __export(require("./x"))
            Some("__exportStar" | "__export" | "_exportStar") => {
                if let Some(specifier) = argument(0).and_then(required) {
                    self.reexports.push(specifier);
                } else if let Some(Expression::ObjectExpression(object)) = argument(1) {
                    // esbuild's: __export(target, { a: () => a, … })
                    for property in &object.properties {
                        if let ObjectPropertyKind::ObjectProperty(property) = property
                            && let Some(name) = key_name(&property.key)
                        {
                            self.names.push(name);
                        }
                    }
                }
            }
            _ => {}
        }
        oxc::ast_visit::walk::walk_call_expression(self, it);
    }

    // A `.js` that is really an ES module — a package's `"module"` entry in a
    // package that never said `"type": "module"`.
    fn visit_export_declaration(&mut self, it: &ExportDeclaration<'a>) {
        match &it.declaration {
            Declaration::VariableDeclaration(variables) => {
                for declarator in &variables.declarations {
                    for id in declarator.id.get_binding_identifiers() {
                        self.names.push(id.name.to_string());
                    }
                }
            }
            Declaration::FunctionDeclaration(function) => {
                if let Some(id) = &function.id {
                    self.names.push(id.name.to_string());
                }
            }
            Declaration::ClassDeclaration(class) => {
                if let Some(id) = &class.id {
                    self.names.push(id.name.to_string());
                }
            }
            _ => {}
        }
    }

    fn visit_export_named_declaration(&mut self, it: &ExportNamedDeclaration<'a>) {
        for specifier in &it.specifiers {
            self.names.push(specifier.exported.name().to_string());
        }
    }

    fn visit_export_from_declaration(&mut self, it: &ExportFromDeclaration<'a>) {
        for specifier in &it.specifiers {
            self.names.push(specifier.exported.name().to_string());
        }
    }

    fn visit_export_all_declaration(&mut self, it: &ExportAllDeclaration<'a>) {
        match &it.exported {
            Some(name) => self.names.push(name.name().to_string()),
            None => self.reexports.push(it.source.value.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names_of(files: &[(&str, &str)]) -> Vec<String> {
        let dir = std::env::temp_dir().join(format!(
            "esdev-commonjs-names-{}-{}",
            std::process::id(),
            unique()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        for (name, source) in files {
            let path = dir.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, source).unwrap();
        }
        let names = exported_names(&dir.join(files[0].0));
        std::fs::remove_dir_all(&dir).ok();
        names
    }

    /// The shapes compiled CommonJS is written in, as Node's loader reads them.
    #[test]
    fn names_are_read_from_what_commonjs_assigns() {
        let names = names_of(&[(
            "index.js",
            r#""use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.one = 1;
module.exports.two = 2;
exports["three"] = 3;
Object.defineProperty(exports, "four", { enumerable: true, get: function () { return 4; } });
exports.default = 5;
"#,
        )]);
        assert_eq!(names, ["four", "one", "three", "two"]);
    }

    /// React's shape: an index that picks a build and re-exports it whole, and
    /// the build assigning inside a wrapper function.
    #[test]
    fn a_reexported_file_is_followed() {
        let names = names_of(&[
            (
                "index.js",
                "if (process.env.NODE_ENV === 'production') { module.exports = require('./cjs/prod.js'); }\n\
                 else { module.exports = require('./cjs/dev.js'); }\n",
            ),
            ("cjs/prod.js", "exports.useState = 1;"),
            (
                "cjs/dev.js",
                "(function () { exports.useState = 1; exports.useRef = 2; })();",
            ),
        ]);
        assert_eq!(names, ["useRef", "useState"]);
    }

    /// TypeScript's re-export helper, and an object assigned whole.
    #[test]
    fn helpers_and_object_literals() {
        let names = names_of(&[
            (
                "index.js",
                "var tslib_1 = require('tslib');\n\
                 tslib_1.__exportStar(require('./core'), exports);\n\
                 module.exports = { ...require('./more'), a, 'b-c': 1 };\n",
            ),
            ("core.js", "exports.Core = 1;"),
            ("more.js", "exports.More = 1;"),
        ]);
        assert_eq!(names, ["Core", "More", "a", "b-c"]);
    }

    /// A `.js` written as an ES module in a package that never said so.
    #[test]
    fn a_module_written_as_esm_is_read_as_esm() {
        let names = names_of(&[
            (
                "lib/index.js",
                "export * from './core';\nexport { x as y } from './core';\n\
                 export const z = 1;\nexport default 2;\n",
            ),
            (
                "lib/core.js",
                "export class IntlMessageFormat {}\nexport function f() {}\n",
            ),
        ]);
        assert_eq!(names, ["IntlMessageFormat", "f", "y", "z"]);
    }

    #[test]
    fn a_package_specifier_is_not_a_path_or_a_url() {
        assert!(is_package("react"));
        assert!(is_package("@scope/pkg/sub"));
        assert!(!is_package("./x"));
        assert!(!is_package("/x"));
        assert!(!is_package("#internal"));
        assert!(!is_package("node:fs"));
        assert!(!is_package("file:///x"));
    }

    #[test]
    fn a_slug_is_a_file_name() {
        assert_eq!(slug("@formatjs/intl"), "formatjs__intl");
        assert_eq!(slug("react-dom/client"), "react-dom__client");
    }
}
