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
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use rolldown::BundlerBuilder;
use tokio::sync::{mpsc, oneshot};

use crate::adapter::Adapter;
use crate::bundler::Failure;
use crate::failures;

use super::plugin::{Bridge, GuestPass};

pub use crate::bundler::{External, OutputOptions};

/// Everything `build()` was given.
///
/// The bundler half is [`crate::bundler::Options`], which every build in this
/// binary describes itself with; the plugins are the guest's own, declared
/// against this project's contract and dispatched back into the isolate.
pub struct Options {
    pub bundler: crate::bundler::Options,
    pub plugins: Vec<Arc<crate::contract::Plugin>>,
}

/// One chunk of a finished build.
pub struct Chunk {
    pub file_name: String,
    pub name: String,
    pub code: String,
    pub is_entry: bool,
    pub is_dynamic_entry: bool,
    /// The module this chunk *is* — the entry it was made for, or the module
    /// behind a dynamic import. `None` for a shared chunk, which is nobody's
    /// facade.
    ///
    /// The field that turns "which chunk is my route" from a guess into a
    /// lookup. Without it the only way to find an entry's chunk is
    /// `find(isEntry)`, and an emitted worker chunk is an entry too — so a
    /// build with one picks the wrong chunk, silently, and the consumer
    /// preloads somebody else's modules.
    pub facade_module_id: Option<String>,
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
        reply: oneshot::Sender<Result<Built, Vec<Failure>>>,
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
    ) -> Result<Built, Vec<Failure>> {
        let (reply, answer) = oneshot::channel();
        self.send(Command::Generate {
            id,
            write,
            output: Box::new(output),
            reply,
        })
        .map_err(|e| vec![plain("BUNDLER_STOPPED", e)])?;
        answer.await.map_err(|_| vec![gone_failure()])?
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

/// The same, in the shape a failed build reports in.
fn gone_failure() -> Failure {
    plain("BUNDLER_STOPPED", gone())
}

/// A failure with nowhere to point: the build never got as far as a module.
fn plain(kind: &str, message: String) -> Failure {
    Failure {
        message,
        id: None,
        plugin: None,
        kind: kind.to_string(),
        line: None,
        column: None,
        frame: None,
    }
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
                        None => Err(vec![plain(
                            "BUILD_CLOSED",
                            "this build is closed".to_string(),
                        )]),
                    };
                    let _ = reply.send(result);
                });
            }
        }
    }
}

/// Rolldown reports a failure as a *batch* of diagnostics, and the guest gets
/// all of them: a thrown `Error` has one message, and hiding the other four
/// behind "and 4 more" is how a build error becomes a bug report.
///
/// Each arrives as a [`Failure`](crate::bundler::Failure) — the summary, the
/// module, and **the line, column and code frame** the bundler already computed
/// for its own output. A dev server's overlay is the reason it is not a string
/// any more: with a string it could name a file and then had to stop.
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
) -> Result<Built, Vec<Failure>> {
    // What `generate()`/`write()` said wins; what only `build()` said stands.
    let output = output.over(&options.bundler.output);
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
        .bundler
        .cwd
        .clone()
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let mut plugins: Vec<rolldown::plugin::__inner::SharedPluginable> = vec![Arc::new(
        Adapter::new(Arc::new(crate::cssmodules::CssModules::new(
            &cwd,
            crate::cssmodules::Collected::new(),
            options.bundler.minify,
        ))),
    )];
    // Then one per declaration, each carrying its own filters. Built per build
    // rather than kept, because a build is where they are used and the options
    // they came from outlive them.
    plugins.extend(options.plugins.iter().map(|declared| {
        // No refresh scheme: `build()` is a program describing its own build,
        // and `refresh` is a *target's* key in `esdev.json`. A program that
        // wants its plugins to know is the one that decided, and says so in
        // the options it already writes.
        let pass = Arc::new(GuestPass::new(bridge.clone(), Arc::clone(declared), None));
        Arc::new(Adapter::new(pass)) as rolldown::plugin::__inner::SharedPluginable
    }));

    let mut bundler = BundlerBuilder::default()
        .with_options(
            crate::bundler::translate(&options.bundler, output, {
                let logs = logs.clone();
                Some(Arc::new(move |line| logs.lock().expect("logs").push(line)))
            })
            .map_err(|e| vec![plain("INVALID_OPTIONS", e)])?,
        )
        .with_plugins(plugins)
        .build()
        .map_err(|e| failures!(e))?;

    let built = if write {
        bundler.write().await.map_err(|e| failures!(e))?
    } else {
        bundler.generate().await.map_err(|e| failures!(e))?
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
                // Stripped of the backend's NUL prefix like every other id
                // that reaches the guest: a plugin matches this against the
                // id it resolved, and that id has no NUL in it.
                facade_module_id: chunk
                    .facade_module_id
                    .as_ref()
                    .map(|id| crate::adapter::guest_id(id.as_ref()).to_string()),
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
