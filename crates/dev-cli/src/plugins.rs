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
                transform: Some(Arc::new(crate::transform::TypeStripper)),
                extensions: crate::guest::extensions_hosting(hosted),
                observer: None,
                inspector: None,
            };
            if let Err(err) = runtime.block_on(es_runtime_cli_common::run("esdev", config)) {
                // The declaration channel is already closed by the time a
                // program fails after declaring, so this is the only place a
                // plugin's *runtime* failure can be reported at all.
                eprintln!("esdev: the project's plugins stopped: {err}");
            }
        })
        .map_err(|e| format!("cannot start the plugin host: {e}"))?;

    let plugins = told.await.map_err(|_| {
        "the project's plugins could not be loaded — the run that loads them ended \
         before it declared any"
            .to_string()
    })??;
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
    let path = path.canonicalize().map_err(|e| {
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

    #[test]
    fn a_spec_is_a_module_and_what_to_call_it_with() {
        assert_eq!(spec("./a.js").module, "./a.js");
        assert!(spec("./a.js").options.is_none());
    }
}
