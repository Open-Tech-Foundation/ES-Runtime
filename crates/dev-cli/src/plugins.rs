//! A project's configured plugins, loaded into an isolate of their own.
//!
//! `esdev.json` names plugin modules ([`crate::config::PluginSpec`]); this is
//! what turns those names into [`Pass`](crate::contract::Pass)es the `build`
//! subcommand can install beside its own.
//!
//! # Why a whole isolate
//!
//! Because a plugin is JavaScript, and the only thing in this binary that runs
//! JavaScript is a V8 isolate. There is no smaller unit: a hook is a closure
//! over module state, so the module has to be *evaluated*, and evaluating it
//! means a runtime with a module loader, a filesystem view and a capability
//! set — which is a run.
//!
//! So esdev starts one. Its program is generated ([`driver`]): it imports the
//! modules the config names, calls each factory with the options the config
//! carries, and hands the results to `runtime:build`'s `host()`, which does not
//! return until the build is over. That pending call is what keeps the isolate
//! alive to answer hooks.
//!
//! ```text
//!   esdev's thread                     the plugin isolate's thread
//!   ──────────────────────             ───────────────────────────
//!   PluginHost::start()  ─ spawn ─▶    run(driver program)
//!                                        import ./plugins/mdx.js
//!                        ◀─ declared ─   host([mdx()])
//!   build() with GuestPasses                (pumping hooks)
//!     transform(code, id) ─ Bridge ─▶      mdx.transform(...)
//!                        ◀───────────
//!   drop(PluginHost)     ─ shutdown ─▶   host() resolves, program exits
//! ```
//!
//! The [`Bridge`](crate::guest::build::plugin::Bridge) doing the crossing is
//! the one `runtime:build` already uses. Nothing about a hook call is different
//! here; what differs is only *who* started the build — the subcommand rather
//! than the program.
//!
//! # Why it is started once and kept
//!
//! `esdev start` rebuilds on every save. Evaluating a plugin's module — and
//! whatever it initialises: a compiler, a template cache, a Tailwind context —
//! forty times a minute would be paying a startup cost per keystroke, and a
//! plugin that holds state across builds (every incremental compiler does)
//! could not exist at all. So the host is process-wide and lives from the first
//! build to the last.
//!
//! # What a plugin may do
//!
//! Whatever a program may do. It runs under `esdev`'s own grant, in the project
//! directory, with the same `runtime:` namespace any other program gets — which
//! is the honest position: a plugin you configured is code you chose to run,
//! exactly like the dev server that used to have to call `build()` itself.

use std::sync::{Arc, Mutex, OnceLock};

use crate::config::PluginSpec;
use crate::contract;
use crate::guest::build::plugin::{Bridge, GuestPass};

/// The plugins one project loaded, and the run holding them open.
pub struct PluginHost {
    bridge: Arc<Bridge>,
    /// Aligned with the [`PluginSpec`]s it was started from, so a target's
    /// indices select from it directly.
    plugins: Vec<Arc<contract::Plugin>>,
    /// Dropped to tell the driver its work is done. The isolate's `host()` call
    /// resolves, its program finishes, and the thread ends.
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl PluginHost {
    /// What the **compiler** has to do for this target's plugins.
    ///
    /// Read from the declarations rather than from a hook, because it decides
    /// how the bundler is built and the bundler is built before any hook runs.
    pub fn jsx(&self, which: &[usize]) -> contract::Jsx {
        which.iter().filter_map(|i| self.plugins.get(*i)).fold(
            contract::Jsx::default(),
            |wanted, plugin| contract::Jsx {
                refresh: wanted.refresh || plugin.jsx.refresh,
            },
        )
    }

    /// The passes a target's plugin indices name, in the order they were
    /// declared.
    ///
    /// `refresh` is the hot-reload scheme that target named, and only when the
    /// build is the dev loop's and the loop is hot — the plugins read it as
    /// `ctx.refresh`. It is passed per target rather than held per plugin
    /// because one plugin object serves every target, and a browser target can
    /// be hot while the server target beside it is not.
    ///
    /// An index that is out of range is skipped rather than panicking: the
    /// indices come from the config that produced this list, so it cannot
    /// happen — and a build that fell over on an internal accounting slip
    /// would be a worse answer than one that built.
    pub fn passes(&self, which: &[usize], refresh: Option<&str>) -> Vec<Arc<dyn contract::Pass>> {
        which
            .iter()
            .filter_map(|i| self.plugins.get(*i))
            .map(|plugin| {
                Arc::new(GuestPass::new(
                    self.bridge.clone(),
                    Arc::clone(plugin),
                    refresh.map(str::to_string),
                )) as Arc<dyn contract::Pass>
            })
            .collect()
    }
}

impl Drop for PluginHost {
    fn drop(&mut self) {
        drop(self.shutdown.take());
        // Joined rather than detached: the isolate is holding a bridge this
        // process still owns, and a thread tearing V8 down while the process
        // exits underneath it is the kind of race that shows up once in a
        // hundred CI runs.
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The one host a run has, started on the first build that needs it.
static HOST: OnceLock<Mutex<Option<Arc<PluginHost>>>> = OnceLock::new();

/// The plugins for this project, starting the host if this is the first build.
///
/// `Ok(None)` is a project with no plugins, which is most of them: nothing is
/// started, no isolate exists, and a build costs exactly what it always did.
pub async fn host(
    dir: &std::path::Path,
    specs: &[PluginSpec],
) -> Result<Option<Arc<PluginHost>>, String> {
    if specs.is_empty() {
        return Ok(None);
    }
    let cell = HOST.get_or_init(|| Mutex::new(None));
    if let Some(existing) = cell.lock().expect("plugin host").clone() {
        return Ok(Some(existing));
    }
    let started = Arc::new(start(dir, specs).await?);
    let mut held = cell.lock().expect("plugin host");
    // Another build got there first while this one was starting. Keep theirs —
    // one isolate is the whole point — and let this one's drop.
    Ok(Some(held.get_or_insert(started).clone()))
}

/// Starts the isolate and waits for it to declare what it loaded.
async fn start(dir: &std::path::Path, specs: &[PluginSpec]) -> Result<PluginHost, String> {
    let (bridge, hooks) = Bridge::new();
    let (declared, told) = tokio::sync::oneshot::channel();
    let (shutdown, ended) = tokio::sync::oneshot::channel();
    let source = driver(dir, specs)?;
    // Where the host thread leaves the reason its run failed. A program that
    // fails *before* declaring — a plugin module with no default export — closes
    // the declaration channel by ending, and then the only thing the waiting
    // side knows is that nothing arrived. Printing the cause from the thread
    // races the process exiting, so it is handed over instead.
    let failure: Arc<std::sync::Mutex<Option<String>>> = Arc::new(std::sync::Mutex::new(None));
    let reported = Arc::clone(&failure);

    let hosted = crate::guest::build::Hosted {
        bridge: bridge.clone(),
        hooks,
        declared,
        shutdown: ended,
    };

    // A thread of its own with a current-thread runtime, because a V8 isolate
    // belongs to the thread that made it and this one has to keep running while
    // the bundler works on other threads entirely.
    let thread = std::thread::Builder::new()
        .name("esdev-plugins".to_string())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                // Nothing will ever declare anything; the caller is waiting on
                // a channel whose sender goes out of scope with `hosted`, and
                // reads that as "the plugin host stopped".
                Err(_) => return,
            };
            let config = es_runtime_cli_common::Config {
                source: es_runtime_cli_common::Source::Inline(source),
                args: Vec::new(),
                capabilities: es_runtime_common::CapabilitySet::all(),
                scopes: std::collections::HashMap::new(),
                options: es_runtime_cli_common::args::RunOptions::default(),
                transform: Some(Arc::new(crate::transform::TypeStripper::new())),
                // A plugin is source somebody is editing, like everything else
                // this binary runs.
                bundler_style_resolution: true,
                package_converter: Some(crate::commonjs::converter()),
                extensions: crate::guest::extensions_hosting(hosted),
                observer: None,
                inspector: None,
            };
            if let Err(err) = runtime.block_on(es_runtime_cli_common::run("esdev", config)) {
                let message = err.to_string();
                // Recorded for the waiting side, which reports it if the run
                // failed before declaring anything. If it failed afterwards
                // nobody is waiting any more, so say it here.
                match reported.lock() {
                    Ok(mut slot) if slot.is_none() => *slot = Some(message),
                    _ => eprintln!("esdev: the project's plugins stopped: {message}"),
                }
            }
        })
        .map_err(|e| format!("cannot start the plugin host: {e}"))?;

    let plugins = match told.await {
        Ok(result) => result?,
        Err(_) => {
            // The thread is ending or has ended; wait for it so the reason it
            // failed is there to report rather than racing this message.
            let _ = thread.join();
            let reason = failure.lock().ok().and_then(|mut slot| slot.take());
            return Err(match reason {
                Some(reason) => format!("the project's plugins could not be loaded: {reason}"),
                None => "the project's plugins could not be loaded — the run that loads them \
                         ended before it declared any"
                    .to_string(),
            });
        }
    };
    if plugins.len() != specs.len() {
        return Err(format!(
            "the project declares {} plugin{}, and the modules named produced {}.\n\n\
             A plugin module's export is one plugin; a module that exports several \
             is not usable from `plugins`, where each entry is one thing to load.",
            specs.len(),
            if specs.len() == 1 { "" } else { "s" },
            plugins.len()
        ));
    }
    // A run that failed after declaring reports itself from the thread, so the
    // host keeps the handle for shutdown as before.
    Ok(PluginHost {
        bridge,
        plugins,
        shutdown: Some(shutdown),
        thread: Some(thread),
    })
}

/// The program the plugin isolate runs.
///
/// Generated rather than written, because what it imports is what the config
/// said. Dynamic `import()` for every entry, so a named export needs no
/// identifier to be minted for it and a module that is missing is reported
/// against the specifier the file wrote rather than as a syntax error in a
/// program nobody typed.
fn driver(dir: &std::path::Path, specs: &[PluginSpec]) -> Result<String, String> {
    let mut described = Vec::with_capacity(specs.len());
    for spec in specs {
        described.push(serde_json::json!({
            "module": specifier(dir, &spec.module)?,
            "named": spec.export,
            "options": spec.options,
            "wrote": spec.module,
        }));
    }
    let specs = serde_json::Value::Array(described);
    Ok(format!(
        r#"// esdev: the project's plugins, held open for the build.
import {{ host }} from "runtime:build";

const specs = {specs};
const plugins = [];

for (const spec of specs) {{
  const module = await import(spec.module);
  const name = spec.named ?? "default";
  if (!(name in module)) {{
    throw new Error(
      name === "default"
        ? `${{spec.wrote}} has no default export — name the one the plugin is with "export"`
        : `${{spec.wrote}} has no export named "${{name}}"`,
    );
  }}
  let plugin = module[name];
  if (typeof plugin === "function") {{
    // A plugin that takes options is a factory, and this is the call a JSON
    // config cannot make for itself.
    plugin = await plugin(spec.options ?? undefined);
  }} else if (spec.options != null) {{
    throw new TypeError(
      `${{spec.wrote}}: options were given, but its ${{name}} export is a plugin already, ` +
        `not a function to call with them`,
    );
  }}
  plugins.push(plugin);
}}

await host(plugins);
"#
    ))
}

/// How the driver names one plugin module.
///
/// A relative or absolute path is resolved against the **project** and handed
/// over as a `file:` URL, because the driver has no file of its own for a
/// relative specifier to be relative to. A bare specifier is left alone: it is
/// a package, and finding a package is the loader's `node_modules` walk.
fn specifier(dir: &std::path::Path, module: &str) -> Result<String, String> {
    if !(module.starts_with("./") || module.starts_with("../") || module.starts_with('/')) {
        return Ok(module.to_string());
    }
    let path = dir.join(module);
    // Stripped of the Windows verbatim prefix: the next call turns this into a
    // `file:` URL, and `Url::from_file_path` refuses `\\?\`-prefixed paths —
    // so without this every file-path plugin fails on Windows with "cannot
    // name the plugin as a module".
    let path = dunce::canonicalize(&path).map_err(|e| {
        format!(
            "cannot read the plugin {module}: {e}\n\n\
             Plugin paths are relative to the project, like every other path in \
             esdev.json."
        )
    })?;
    url::Url::from_file_path(&path)
        .map(|url| url.to_string())
        .map_err(|()| format!("cannot name the plugin {module} as a module"))
}

/// `then`, behind the project's plugins when it has any — what `esdev test`
/// loads every module through.
pub async fn transform(
    dir: &std::path::Path,
    specs: &[PluginSpec],
    then: Arc<dyn es_runtime_cli_common::run::SourceTransform>,
) -> Result<Arc<dyn es_runtime_cli_common::run::SourceTransform>, String> {
    Ok(match host(dir, specs).await? {
        Some(host) => Arc::new(PluginTransform::new(&host, dir, then)),
        None => then,
    })
}

/// A module source transform that puts every module through the project's
/// plugins first, the way a build does.
///
/// # Why a test needs this
///
/// A build hands each module to the plugins' `transform` hooks and then
/// compiles what they return. A test file is not bundled — it is loaded one
/// module at a time by the runtime, through a [`SourceTransform`] — so without
/// this a project whose `.jsx` means what its framework's compiler says it
/// means would build, and then fail every test that imported a component.
///
/// Only `transform` is run. `resolve` and `load` answer the bundler's graph
/// walk, and a test run has no graph: the runtime's own loader finds files.
///
/// # Why it blocks
///
/// The runtime asks for a module's source synchronously, and a hook's answer
/// comes from another thread — the plugin isolate. Waiting on it here is what
/// every module load already does for the file read; the isolate never waits on
/// this thread, so there is nothing for the two to deadlock over.
///
/// [`SourceTransform`]: es_runtime_cli_common::run::SourceTransform
pub struct PluginTransform {
    /// In the order a build calls them: `pre`, then unordered, then `post`,
    /// each in the order the project declared them.
    passes: Vec<Arc<dyn contract::Pass>>,
    /// What compiles the module once the plugins are done with it.
    then: Arc<dyn es_runtime_cli_common::run::SourceTransform>,
    ctx: Arc<dyn contract::Context>,
}

impl PluginTransform {
    /// Every one of `host`'s plugins, ahead of `then`.
    pub fn new(
        host: &PluginHost,
        dir: &std::path::Path,
        then: Arc<dyn es_runtime_cli_common::run::SourceTransform>,
    ) -> PluginTransform {
        let every: Vec<usize> = (0..host.plugins.len()).collect();
        let mut passes = host.passes(&every, None);
        // Stable, so plugins of one order keep the order they were declared in.
        passes.sort_by_key(
            |pass| match pass.hooks().transform.as_ref().map(|h| h.order) {
                Some(contract::Order::Pre) => 0,
                Some(contract::Order::Post) => 2,
                _ => 1,
            },
        );
        passes.retain(|pass| pass.hooks().transform.is_some());
        PluginTransform {
            passes,
            then,
            ctx: Arc::new(TestContext {
                dir: dir.to_path_buf(),
            }),
        }
    }

    /// `source` after every plugin whose filter admits `id`.
    fn through_plugins(&self, id: &str, mut source: String) -> Result<String, String> {
        let mut module_type = module_type(id);
        for pass in &self.passes {
            let admitted = pass
                .hooks()
                .transform
                .as_ref()
                .is_some_and(|hook| hook.filter.admits(id, Some(&source)));
            if !admitted {
                continue;
            }
            let answer = wait(pass.transform(&source, id, &module_type, &self.ctx))
                .map_err(|e| format!("[plugin {}] {id}\n{e}", pass.name()))?;
            if let Some(result) = answer {
                source = result.code;
                if let Some(changed) = result.module_type {
                    module_type = changed;
                }
            }
        }
        Ok(source)
    }
}

impl es_runtime_cli_common::run::SourceTransform for PluginTransform {
    fn reserved_query(&self) -> Option<&'static str> {
        self.then.reserved_query()
    }

    fn transform(&self, specifier: &str, source: String) -> Result<String, String> {
        // A mocked module is not the file's source at all, and the compiler
        // after this replaces it whole; a plugin has nothing to say about it.
        if self.passes.is_empty() || crate::module_mocks::synthetic(specifier).is_some() {
            return self.then.transform(specifier, source);
        }
        // Plugins name modules by path, as the bundler hands them over. Only a
        // file is theirs; a `runtime:` module is this binary's own.
        let path = url::Url::parse(specifier)
            .ok()
            .filter(|url| url.scheme() == "file")
            .and_then(|mut url| {
                url.set_query(None);
                url.set_fragment(None);
                url.to_file_path().ok()
            });
        let source = match path {
            Some(path) => self.through_plugins(&path.to_string_lossy(), source)?,
            None => source,
        };
        self.then.transform(specifier, source)
    }
}

/// What a module is before any plugin has changed it, spelled the way the
/// bundler would tell a hook: its extension, with the JavaScript and
/// TypeScript family names folded to the four the compiler knows.
fn module_type(id: &str) -> String {
    match std::path::Path::new(id)
        .extension()
        .and_then(|e| e.to_str())
    {
        Some("js" | "mjs" | "cjs") => "js".to_string(),
        Some("ts" | "mts" | "cts") => "ts".to_string(),
        Some(other) => other.to_string(),
        None => "js".to_string(),
    }
}

/// Drives a hook's answer to completion on this thread.
///
/// Not a nested async runtime: the caller is already inside one, which refuses
/// to be entered twice. The future only ever waits on the plugin isolate's
/// reply, so parking until it is woken is all this needs to do.
fn wait<T>(future: contract::Answer<'_, T>) -> Result<T, String> {
    struct Unpark(std::thread::Thread);
    impl std::task::Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = std::task::Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut cx = std::task::Context::from_waker(&waker);
    let mut future = future;
    loop {
        if let std::task::Poll::Ready(answer) = future.as_mut().poll(&mut cx) {
            return answer;
        }
        std::thread::park();
    }
}

/// What a hook's `ctx` can do in a test run.
///
/// A test run is not a build: nothing is bundled, so there is no graph to add
/// an entry to and no output to put an asset beside.
struct TestContext {
    dir: std::path::PathBuf,
}

impl contract::Context for TestContext {
    fn resolve<'a>(
        &'a self,
        specifier: &'a str,
        importer: Option<&'a str>,
        _skip_self: bool,
    ) -> contract::Answer<'a, Option<contract::ResolvedId>> {
        Box::pin(async move { Ok(resolve_file(specifier, importer)) })
    }

    fn emit(&self, _emit: contract::Emit) -> Result<String, String> {
        Err(
            "ctx.emit() adds to a build's output, and a test run writes none — \
             this plugin's transform cannot run under esdev test"
                .to_string(),
        )
    }

    fn log(&self, level: &str, message: String) {
        eprintln!("esdev: plugin {level}: {message}");
    }

    fn depends_on(&self, _file: &str) {}

    fn cwd(&self) -> std::path::PathBuf {
        self.dir.clone()
    }
}

/// A relative or absolute specifier, as the file it names — with the
/// extensions and `index` files a bundler would try. A package is left
/// unresolved: `None` is an answer a plugin already has to handle.
fn resolve_file(specifier: &str, importer: Option<&str>) -> Option<contract::ResolvedId> {
    const EXTENSIONS: [&str; 6] = ["ts", "tsx", "mts", "js", "jsx", "mjs"];
    let base = if specifier.starts_with('/') {
        std::path::PathBuf::from(specifier)
    } else if specifier.starts_with("./") || specifier.starts_with("../") {
        std::path::Path::new(importer?).parent()?.join(specifier)
    } else {
        return None;
    };
    let candidates = std::iter::once(base.clone())
        .chain(EXTENSIONS.iter().map(|ext| {
            let mut name = base.clone().into_os_string();
            name.push(format!(".{ext}"));
            std::path::PathBuf::from(name)
        }))
        .chain(
            EXTENSIONS
                .iter()
                .map(|ext| base.join(format!("index.{ext}"))),
        );
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .and_then(|path| dunce::canonicalize(path).ok())
        .map(|path| contract::ResolvedId {
            id: path.to_string_lossy().into_owned(),
            external: false,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(module: &str) -> PluginSpec {
        PluginSpec {
            module: module.to_string(),
            export: None,
            options: None,
        }
    }

    /// A package is left as it was written — finding it is the loader's walk,
    /// not this module's business.
    #[test]
    fn a_bare_specifier_is_not_rewritten() {
        let dir = std::path::Path::new(".");
        assert_eq!(specifier(dir, "@otfw/compiler").unwrap(), "@otfw/compiler");
    }

    /// A relative path is resolved against the project, because the generated
    /// program has no file of its own to be relative to.
    #[test]
    fn a_relative_path_becomes_a_file_url_under_the_project() {
        let dir = std::env::temp_dir().join(format!("esdev-plugins-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("p.js"), "export default {};").unwrap();
        let named = specifier(&dir, "./p.js").unwrap();
        assert!(named.starts_with("file://"), "{named}");
        assert!(named.ends_with("/p.js"), "{named}");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A path that is not there is refused where it was written, rather than
    /// becoming a module-not-found in a program nobody typed.
    #[test]
    fn a_missing_plugin_is_named() {
        let dir = std::env::temp_dir();
        let refused = specifier(&dir, "./nothing-here-at-all.js").unwrap_err();
        assert!(refused.contains("cannot read the plugin"), "{refused}");
    }

    /// The driver carries the options through as JSON, since that is the call
    /// the config file cannot make.
    #[test]
    fn the_driver_carries_the_options_it_was_given() {
        let dir = std::path::Path::new(".");
        let source = driver(
            dir,
            &[PluginSpec {
                module: "@otfw/compiler".to_string(),
                export: Some("compiler".to_string()),
                options: Some(serde_json::json!({ "jsx": "automatic" })),
            }],
        )
        .unwrap();
        assert!(source.contains("@otfw/compiler"), "{source}");
        assert!(source.contains("\"compiler\""), "{source}");
        assert!(source.contains("automatic"), "{source}");
    }

    /// Nothing is loaded for a project with no plugins — no isolate, no thread,
    /// and a build that costs what it always did.
    #[tokio::test]
    async fn a_project_with_no_plugins_starts_nothing() {
        let none = host(std::path::Path::new("."), &[]).await.unwrap();
        assert!(none.is_none());
    }

    /// A hook is told what a module is in the bundler's vocabulary, so a
    /// plugin written against a build reads the same `ctx.type` in a test.
    #[test]
    fn a_modules_type_is_what_the_bundler_would_call_it() {
        assert_eq!(module_type("/a/b.mjs"), "js");
        assert_eq!(module_type("/a/b.mts"), "ts");
        assert_eq!(module_type("/a/b.jsx"), "jsx");
        assert_eq!(module_type("/a/b.mdx"), "mdx");
    }

    /// `ctx.resolve()` in a test finds what a bundler would, and leaves a
    /// package for the plugin's own fallback.
    #[test]
    fn a_test_run_resolves_files_the_way_a_bundler_would() {
        let dir =
            std::env::temp_dir().join(format!("esdev-plugins-resolve-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("lib")).unwrap();
        std::fs::write(dir.join("util.ts"), "").unwrap();
        std::fs::write(dir.join("lib/index.js"), "").unwrap();
        let importer = dir.join("main.ts").to_string_lossy().into_owned();
        let found = |specifier| resolve_file(specifier, Some(&importer)).map(|r| r.id);
        assert!(found("./util").unwrap().ends_with("util.ts"));
        assert!(found("./lib").unwrap().ends_with("index.js"));
        assert!(found("./missing").is_none());
        assert!(found("some-package").is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A hook's answer arrives from another thread; waiting for it parks
    /// this one rather than needing a runtime of its own.
    #[test]
    fn waiting_on_an_answer_from_another_thread() {
        let (tx, rx) = tokio::sync::oneshot::channel::<u32>();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(20));
            let _ = tx.send(7);
        });
        let answer: contract::Answer<'_, u32> =
            Box::pin(async move { rx.await.map_err(|e| e.to_string()) });
        assert_eq!(wait(answer).unwrap(), 7);
    }

    #[test]
    fn a_spec_is_a_module_and_what_to_call_it_with() {
        assert_eq!(spec("./a.js").module, "./a.js");
        assert!(spec("./a.js").options.is_none());
    }
}
