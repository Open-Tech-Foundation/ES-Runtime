//! The bundler, on a thread of its own.
//!
//! Rolldown wants threads: it parses, resolves and transforms modules in
//! parallel, and its whole speed argument rests on that. The isolate wants the
//! opposite — one thread, its own, never blocked. Running the bundler *inside*
//! the isolate's current-thread runtime satisfies neither: the build would be
//! serialized onto one core, and every heavy stretch of it would be time the
//! guest's own program (the dev server answering requests) was not running.
//!
//! So the bundler lives on a dedicated thread with a multi-threaded runtime of
//! its own, and the two sides speak over channels. A `build()` from the guest
//! is a command; its result is a reply; and the plugin hooks that have to run
//! in the isolate go back the other way through the [`Bridge`]. Nothing
//! rolldown touches is ever on the isolate's thread, and nothing V8 owns is
//! ever on rolldown's.
//!
//! The thread starts on the first `build()` and lives for the rest of the run,
//! because a dev server builds continuously and paying for a thread and a
//! runtime per rebuild would be paying for it forty times a minute.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use rolldown::{
    BundlerBuilder, BundlerOptions, InputItem, IsExternal, OutputFormat, Platform,
    RawMinifyOptions, ResolveOptions, SourceMapType, TreeshakeOptions,
};
use rolldown_common::CodeSplittingMode;
use tokio::sync::{mpsc, oneshot};

use super::plugin::{Bridge, JsPlugin};

/// Everything `build()` was given, in a form that can cross a thread.
#[derive(Default)]
pub struct Options {
    pub cwd: Option<PathBuf>,
    /// Entries, as `(name, import)`. A name is what the chunk is called; most
    /// callers pass one nameless entry.
    pub input: Vec<(Option<String>, String)>,
    pub external: Option<External>,
    pub platform: Option<String>,
    pub conditions: Vec<String>,
    pub main_fields: Vec<String>,
    /// `find` → the replacements tried in order, rolldown's own alias shape.
    pub alias: Vec<(String, Vec<String>)>,
    pub extensions: Vec<String>,
    pub define: Vec<(String, String)>,
    pub plugins: Vec<Arc<crate::guest::build::contract::Plugin>>,
    pub minify: bool,
    pub treeshake: Option<bool>,
    pub output: OutputOptions,
}

/// The half of the options that describes what comes *out*, which
/// `generate()`/`write()` may override per call.
#[derive(Clone, Default)]
pub struct OutputOptions {
    pub format: Option<String>,
    pub dir: Option<String>,
    pub file: Option<String>,
    pub entry_filenames: Option<String>,
    pub chunk_filenames: Option<String>,
    pub asset_filenames: Option<String>,
    /// `false` puts everything reachable in one chunk, dynamic `import()`
    /// included — the setting a dev server that serves chunks from memory needs
    /// when it is building one route at a time.
    pub code_splitting: Option<bool>,
    pub sourcemap: Option<String>,
    pub banner: Option<String>,
    pub footer: Option<String>,
}

impl OutputOptions {
    /// Fields the caller set here win; the rest stay as `build()` left them.
    fn over(self, base: &OutputOptions) -> OutputOptions {
        OutputOptions {
            format: self.format.or_else(|| base.format.clone()),
            dir: self.dir.or_else(|| base.dir.clone()),
            file: self.file.or_else(|| base.file.clone()),
            entry_filenames: self
                .entry_filenames
                .or_else(|| base.entry_filenames.clone()),
            chunk_filenames: self
                .chunk_filenames
                .or_else(|| base.chunk_filenames.clone()),
            asset_filenames: self
                .asset_filenames
                .or_else(|| base.asset_filenames.clone()),
            code_splitting: self.code_splitting.or(base.code_splitting),
            sourcemap: self.sourcemap.or_else(|| base.sourcemap.clone()),
            banner: self.banner.or_else(|| base.banner.clone()),
            footer: self.footer.or_else(|| base.footer.clone()),
        }
    }
}

/// What `external` was: a list of specifiers, or a function in the guest.
#[derive(Clone)]
pub enum External {
    List(Vec<String>),
    /// The guest's handle for the predicate, called through the same bridge as
    /// a plugin hook. A predicate rather than a list is not a nicety: a dev
    /// server externalises `/__route/*` — a shape, not a set.
    Predicate(f64),
}

/// One chunk of a finished build.
pub struct Chunk {
    pub file_name: String,
    pub name: String,
    pub code: String,
    pub is_entry: bool,
    pub is_dynamic_entry: bool,
    /// Every module that went into it — half of what invalidation needs.
    pub module_ids: Vec<String>,
    pub imports: Vec<String>,
    pub dynamic_imports: Vec<String>,
    pub map: Option<String>,
}

/// One emitted asset (`this.emitFile({ type: "asset" })`, or a stylesheet).
pub struct Asset {
    pub file_name: String,
    pub source: AssetSource,
}

pub enum AssetSource {
    Text(String),
    Bytes(Vec<u8>),
}

/// A finished build.
pub struct Built {
    pub chunks: Vec<Chunk>,
    pub assets: Vec<Asset>,
    /// Every file the build read, plus every file a plugin declared through
    /// `this.addWatchFile()`. The **other** half of invalidation: without it a
    /// consumer can only drop everything on a change, and the whole point of a
    /// lazy per-route cache is dropping three chunks out of forty.
    pub watch_files: Vec<String>,
    pub warnings: Vec<String>,
}

enum Command {
    Create {
        options: Box<Options>,
        reply: oneshot::Sender<u64>,
    },
    Generate {
        id: u64,
        write: bool,
        output: Box<OutputOptions>,
        reply: oneshot::Sender<Result<Built, String>>,
    },
    Close {
        id: u64,
    },
}

/// The handle the ops hold: a channel to the bundler thread.
pub struct BuildServer {
    commands: mpsc::UnboundedSender<Command>,
}

impl BuildServer {
    /// Starts the bundler thread. Called once, on the first `build()`.
    pub fn start(bridge: Arc<Bridge>) -> Result<BuildServer, String> {
        let (tx, rx) = mpsc::unbounded_channel();
        std::thread::Builder::new()
            .name("esdev-bundler".to_string())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    // The thread is the bundler; without a runtime there is
                    // nothing for it to do, and every command will fail as
                    // "the bundler is gone" when the channel closes.
                    Err(_) => return,
                };
                runtime.block_on(serve(rx, bridge));
            })
            .map_err(|e| format!("cannot start the bundler thread: {e}"))?;
        Ok(BuildServer { commands: tx })
    }

    pub async fn create(&self, options: Options) -> Result<u64, String> {
        let (reply, answer) = oneshot::channel();
        self.send(Command::Create {
            options: Box::new(options),
            reply,
        })?;
        answer.await.map_err(|_| gone())
    }

    pub async fn generate(
        &self,
        id: u64,
        write: bool,
        output: OutputOptions,
    ) -> Result<Built, String> {
        let (reply, answer) = oneshot::channel();
        self.send(Command::Generate {
            id,
            write,
            output: Box::new(output),
            reply,
        })?;
        answer.await.map_err(|_| gone())?
    }

    pub fn close(&self, id: u64) {
        let _ = self.send(Command::Close { id });
    }

    fn send(&self, command: Command) -> Result<(), String> {
        self.commands.send(command).map_err(|_| gone())
    }
}

fn gone() -> String {
    "the bundler stopped".to_string()
}

/// The bundler thread's loop.
///
/// Each command is spawned rather than awaited in turn: two `generate()` calls
/// on different builds are independent — a dev server builds several routes at
/// once — and serializing them here would make the queue the bottleneck instead
/// of the work.
async fn serve(mut commands: mpsc::UnboundedReceiver<Command>, bridge: Arc<Bridge>) {
    let builds: Arc<std::sync::Mutex<HashMap<u64, Arc<Options>>>> =
        Arc::new(std::sync::Mutex::new(HashMap::new()));
    let next_id = AtomicU64::new(1);

    while let Some(command) = commands.recv().await {
        match command {
            Command::Create { options, reply } => {
                let id = next_id.fetch_add(1, Ordering::Relaxed);
                builds
                    .lock()
                    .expect("builds")
                    .insert(id, Arc::new(*options));
                let _ = reply.send(id);
            }
            Command::Close { id } => {
                builds.lock().expect("builds").remove(&id);
            }
            Command::Generate {
                id,
                write,
                output,
                reply,
            } => {
                let options = builds.lock().expect("builds").get(&id).cloned();
                let bridge = bridge.clone();
                tokio::spawn(async move {
                    let result = match options {
                        Some(options) => run(&options, *output, write, bridge).await,
                        None => Err("this build is closed".to_string()),
                    };
                    let _ = reply.send(result);
                });
            }
        }
    }
}

/// Rolldown reports a failure as a *batch* of diagnostics; the guest gets one
/// string with all of them, because a thrown `Error` has one message and hiding
/// the other four behind "and 4 more" is how a build error becomes a bug
/// report. Each is named by the module it came from.
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

/// The batch, unpacked. A macro rather than a function because the diagnostic
/// type belongs to a crate this one does not depend on directly — it arrives
/// through rolldown, and naming it would mean declaring a dependency to write
/// one signature. The same reason, and the same shape, as `esdev build`'s.
macro_rules! reported {
    () => {
        |error| {
            diagnostics(
                error
                    .into_vec()
                    .into_iter()
                    .map(|diagnostic| (diagnostic.id(), diagnostic))
                    .collect(),
            )
        }
    };
}

/// One build, start to finish.
///
/// The `Bundler` is constructed here rather than kept between calls, and that
/// is deliberate: its options include the output ones, which `generate()` may
/// override per call, and a bundler carrying stale options is a build that
/// silently ignores what it was asked for. Construction is bookkeeping; the
/// cost is in the scan, which happens either way.
async fn run(
    options: &Options,
    output: OutputOptions,
    write: bool,
    bridge: Arc<Bridge>,
) -> Result<Built, String> {
    // What `generate()`/`write()` said wins; what only `build()` said stands.
    let output = output.over(&options.output);
    // Whatever a plugin says through `this.warn()` during this build. Rolldown
    // routes those to `on_log` and nowhere else, so without a sink here a
    // plugin's diagnostics would go to the same place as a `console.log` in a
    // detached thread: nowhere the developer can see.
    let logs: Arc<std::sync::Mutex<Vec<String>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    // The passes this toolchain owns come first, and they are installed here
    // for the same reason `esdev build` installs them: a component importing
    // `./x.module.css` renders `className={styles.button}`, and a build that
    // did not scope that name identically produces markup that does not match
    // the stylesheet the browser fetched.
    //
    // They were **missing** here when this module first shipped, so a guest
    // build and the `build` subcommand disagreed about the same project — the
    // exact failure that follows from our own passes and the guest's plugins
    // being two unrelated concepts. One list, in one order, is the fix.
    let cwd = options
        .cwd
        .clone()
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let mut plugins: Vec<rolldown::plugin::__inner::SharedPluginable> =
        vec![Arc::new(crate::cssmodules::CssModules::new(
            &cwd,
            crate::cssmodules::Collected::new(),
            options.minify,
        ))];
    // Then one per declaration, each carrying its own filters. Built per build
    // rather than kept, because a build is where they are used and the options
    // they came from outlive them.
    plugins.extend(options.plugins.iter().map(|declared| {
        Arc::new(JsPlugin::new(bridge.clone(), Arc::clone(declared)))
            as rolldown::plugin::__inner::SharedPluginable
    }));

    let mut bundler = BundlerBuilder::default()
        .with_options(bundler_options(options, output, bridge, logs.clone())?)
        .with_plugins(plugins)
        .build()
        .map_err(reported!())?;

    let built = if write {
        bundler.write().await.map_err(reported!())?
    } else {
        bundler.generate().await.map_err(reported!())?
    };

    // Read *after* the build: the set is what this run actually touched,
    // including whatever a plugin declared while it ran.
    let watch_files = bundler
        .watch_files()
        .iter()
        .map(|f| f.to_string())
        .collect();

    let mut chunks = Vec::new();
    let mut assets = Vec::new();
    for output in built.assets {
        match output {
            rolldown_common::Output::Chunk(chunk) => chunks.push(Chunk {
                file_name: chunk.filename.to_string(),
                name: chunk.name.to_string(),
                code: chunk.code.clone(),
                is_entry: chunk.is_entry,
                is_dynamic_entry: chunk.is_dynamic_entry,
                module_ids: chunk.module_ids.iter().map(ToString::to_string).collect(),
                imports: chunk.imports.iter().map(ToString::to_string).collect(),
                dynamic_imports: chunk
                    .dynamic_imports
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
                map: chunk.map.as_ref().map(|map| map.to_json_string()),
            }),
            rolldown_common::Output::Asset(asset) => assets.push(Asset {
                file_name: asset.filename.to_string(),
                source: match asset.source.clone() {
                    rolldown_common::StrOrBytes::Str(text) => AssetSource::Text(text),
                    rolldown_common::StrOrBytes::Bytes(bytes) => AssetSource::Bytes(bytes),
                },
            }),
        }
    }

    // The bundler's own warnings first, then the plugins' — the order they
    // would have been printed in if anything had been printing.
    let mut warnings: Vec<String> = built.warnings.iter().map(ToString::to_string).collect();
    warnings.extend(logs.lock().expect("logs").drain(..));

    Ok(Built {
        chunks,
        assets,
        watch_files,
        warnings,
    })
}

/// Translates the guest's options into rolldown's.
fn bundler_options(
    options: &Options,
    output: OutputOptions,
    bridge: Arc<Bridge>,
    logs: Arc<std::sync::Mutex<Vec<String>>>,
) -> Result<BundlerOptions, String> {
    let input: Vec<InputItem> = options
        .input
        .iter()
        .map(|(name, import)| InputItem {
            name: name.clone(),
            import: import.clone(),
        })
        .collect();
    if input.is_empty() {
        return Err("build: input is required".to_string());
    }

    let external = match &options.external {
        None => None,
        Some(External::List(list)) => Some(IsExternal::from(list.clone())),
        // Every specifier the bundler meets is a question for the guest. The
        // answers are not cached here: the predicate is the guest's, and a
        // bundler that remembered its answers would be deciding when it stopped
        // being asked.
        Some(External::Predicate(id)) => {
            let id = *id;
            Some(IsExternal::Fn(Some(Arc::new(
                move |specifier: &str, importer: Option<&str>, resolved: bool| {
                    let bridge = bridge.clone();
                    let args = vec![
                        es_runtime_cli_common::Value::String(specifier.to_string()),
                        match importer {
                            Some(importer) => {
                                es_runtime_cli_common::Value::String(importer.to_string())
                            }
                            None => es_runtime_cli_common::Value::Null,
                        },
                        es_runtime_cli_common::Value::Bool(resolved),
                    ];
                    Box::pin(async move {
                        let answer = bridge.call(id, "external", args, Vec::new(), None).await?;
                        Ok(matches!(answer, es_runtime_cli_common::Value::Bool(true)))
                    })
                },
            ))))
        }
    };

    let resolve = ResolveOptions {
        condition_names: (!options.conditions.is_empty()).then(|| options.conditions.clone()),
        main_fields: (!options.main_fields.is_empty()).then(|| options.main_fields.clone()),
        extensions: (!options.extensions.is_empty()).then(|| options.extensions.clone()),
        alias: (!options.alias.is_empty()).then(|| {
            options
                .alias
                .iter()
                .map(|(find, to)| {
                    (
                        find.clone(),
                        to.iter().map(|t| Some(t.clone())).collect::<Vec<_>>(),
                    )
                })
                .collect()
        }),
        ..ResolveOptions::default()
    };

    Ok(BundlerOptions {
        input: Some(input),
        cwd: options.cwd.clone(),
        external,
        platform: match options.platform.as_deref() {
            Some("browser") => Some(Platform::Browser),
            Some("node") => Some(Platform::Node),
            // Neither a browser nor Node, which is what this runtime is: saying
            // either would pull in that platform's `main` fields and aliases.
            _ => Some(Platform::Neutral),
        },
        format: match output.format.as_deref() {
            Some("cjs") => Some(OutputFormat::Cjs),
            Some("iife") => Some(OutputFormat::Iife),
            Some("umd") => Some(OutputFormat::Umd),
            _ => Some(OutputFormat::Esm),
        },
        dir: output.dir,
        file: output.file,
        entry_filenames: output.entry_filenames.map(Into::into),
        chunk_filenames: output.chunk_filenames.map(Into::into),
        asset_filenames: output.asset_filenames.map(Into::into),
        code_splitting: output.code_splitting.map(CodeSplittingMode::Bool),
        sourcemap: match output.sourcemap.as_deref() {
            Some("inline") => Some(SourceMapType::Inline),
            Some("hidden") => Some(SourceMapType::Hidden),
            Some("external" | "true") => Some(SourceMapType::File),
            _ => None,
        },
        banner: output
            .banner
            .map(|text| rolldown_common::AddonOutputOption::String(Some(text))),
        footer: output
            .footer
            .map(|text| rolldown_common::AddonOutputOption::String(Some(text))),
        define: (!options.define.is_empty()).then(|| options.define.iter().cloned().collect()),
        minify: options.minify.then_some(RawMinifyOptions::Bool(true)),
        treeshake: match options.treeshake {
            Some(false) => TreeshakeOptions::Boolean(false),
            _ => TreeshakeOptions::default(),
        },
        resolve: Some(resolve),
        // Where `this.warn()` ends up. Info and debug are dropped: a build that
        // returned every `this.debug()` as a warning would train the consumer
        // to ignore the list.
        on_log: Some(rolldown_common::OnLog::new(Arc::new(
            move |level, log: rolldown_common::Log| {
                if matches!(level, rolldown_common::LogLevel::Warn) {
                    let line = match &log.plugin {
                        Some(plugin) => format!("{plugin}: {}", log.message),
                        None => log.message.clone(),
                    };
                    logs.lock().expect("logs").push(line);
                }
                Box::pin(async { Ok(()) })
            },
        ))),
        ..BundlerOptions::default()
    })
}
