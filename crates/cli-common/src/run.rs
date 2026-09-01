//! The standalone embedding: wire the default tokio providers, build a
//! [`Runtime`], load the entry as an ES module, and drive it to completion.
//!
//! This is the part of a binary that is not about *its* command line. `esrun`
//! and `esdev` differ in what they offer around a run — subcommands, watching,
//! building — but a program must behave identically under both, so the run
//! itself lives here once.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use es_runtime::{HostProviders, ModuleEvalState, ModuleLoader, Process, Runtime};
use es_runtime_common::{Capability, CapabilitySet};
use es_runtime_default_providers::{DriveFailure, Driver};
use es_runtime_default_providers::{
    ImportPolicy, NodeModuleLoader, OsEntropy, ProcessBroadcastHub, ProcessPortHub,
    ReqwestTransport, SystemClock, SystemCommands, SystemEmbeddedDb, SystemFileSystem,
    SystemHttpServer, SystemNet, SystemProcess, SystemSignals, SystemSyncFileSystem,
    SystemWebSocket, ThreadWorkerHost, TokioTimers, WorkerProcess, path,
};
use es_runtime_providers::{ModuleSource, ProviderError, WorkerScope, WorkerSpec};
use url::Url;

use crate::SNAPSHOT;
use crate::args::RunOptions;
use crate::console::StdoutConsole;
use crate::diagnostics::{Failure, flush_failures, timeout_message};
use crate::dotenv;
use crate::extension::{ExtensionContext, HostExtension};
use crate::permissions::{Scopes, address_scope, path_scope, signal_scope};
use crate::shutdown::{SHUTDOWN_CODE, spawn_shutdown_watcher};

/// What to run, parsed from argv.
pub enum Source {
    /// A module file at this path.
    File(String),
    /// An inline module snippet, from `-e`.
    Inline(String),
}

/// Everything a run needs, once a binary's own command line has been parsed.
pub struct Config {
    /// The entry module.
    pub source: Source,
    /// User arguments after the script/`-e` code, exposed as `runtime:process`
    /// `args` (the runtime binary and the script/code are excluded).
    pub args: Vec<String>,
    /// What the script may reach, from `--deny-all` / `--deny-<name>` (D38).
    /// [`CapabilitySet::all`] unless denials were asked for.
    pub capabilities: CapabilitySet,
    /// The scope lists from `--allow-<name>=a,b`, keyed by capability (D38).
    /// A capability that is granted but absent here is granted **unnarrowed**;
    /// the narrowing itself is enforced provider-side, not by the capability
    /// bit — the bit only says whether the door exists.
    pub scopes: Scopes,
    /// The shared flags that shape the run.
    pub options: RunOptions,
    /// An optional rewrite applied to every module's source before the engine
    /// sees it — how `esdev` strips TypeScript and compiles JSX.
    ///
    /// `esrun` passes `None` and therefore carries neither the dependency nor
    /// the behaviour: the code that turns a `.ts` into JavaScript belongs on the
    /// developer's machine, not in the binary that serves production. The seam
    /// is here rather than in each binary because the **entry** file is read
    /// directly (before a loader exists), so a transform that only wrapped the
    /// loader would silently miss the one file the user named.
    pub transform: Option<Arc<dyn SourceTransform>>,
    /// Whether a specifier that names no file is retried the way a bundler
    /// resolves one — `./util` and `./util.js` finding `util.ts`.
    ///
    /// `esdev` sets it and `esrun` does not. See [`BundlerStyleLoader`] for why
    /// the two binaries differ here, and why they still agree about every
    /// specifier that resolves under both.
    pub bundler_style_resolution: bool,
    /// Watches what the run actually reaches for — how `esdev` serves
    /// `--trace-permissions` (D59).
    ///
    /// `esrun` passes `None`. Unlike the inspector this can only observe, so
    /// what is kept out of the production binary is not the hook (a single
    /// `Option` read per op dispatch) but the report: turning a set of
    /// capabilities into the `esrun` command line that would grant exactly them
    /// is `esdev`'s job, and lives there.
    pub observer: Option<es_runtime::SharedObserver>,
    /// `runtime:` modules and ops this binary adds to the namespace — how
    /// `esdev` serves `runtime:build` and `runtime:watch`
    /// ([`HostExtension`](crate::HostExtension)).
    ///
    /// `esrun` passes none, and that is the whole point of the field: a
    /// production binary carries neither a bundler nor a file watcher, so a
    /// program that imports one fails at load there rather than behaving
    /// differently. Registered on the main agent only — a worker gets the
    /// standard namespace.
    pub extensions: Vec<Box<dyn HostExtension>>,
    /// A debugger to attach before the entry module is loaded — how `esdev`
    /// serves `--inspect` (D59).
    ///
    /// `esrun` passes `None` and could not pass anything else: an inspector port
    /// is a total bypass of the capability model, so the code that speaks the
    /// protocol is compiled only into a build that asked for it, and the server
    /// that carries it lives in `esdev`. What crosses this seam is a transport
    /// and a flag — the same shape, and for the same reason, as `transform`
    /// above.
    pub inspector: Option<Inspector>,
}

/// A debugger session waiting for the runtime it will be attached to.
pub struct Inspector {
    /// The channel the Chrome DevTools Protocol is spoken over. Not `Send`
    /// because it never leaves this thread: the isolate's thread is the one that
    /// answers a debugger, and blocks on it while paused.
    pub transport: std::rc::Rc<dyn es_runtime::InspectorTransport>,
    /// Hold the program before its first statement until a client attaches and
    /// releases it (`--inspect-brk`).
    pub wait: bool,
}

/// Rewrites a module's source before it reaches the engine.
///
/// Implementations must be pure and deterministic: the same specifier and text
/// produce the same output, because a module is loaded once per realm and the
/// result is what `import.meta.url` and every stack frame will refer to.
pub trait SourceTransform: Send + Sync {
    /// Rewrites `source` for the module at `specifier` (a `file:` URL), or
    /// reports why it could not. Returning `source` unchanged is the correct
    /// answer for a file this transform has nothing to do with.
    fn transform(&self, specifier: &str, source: String) -> Result<String, String>;
}

/// Wraps a [`ModuleLoader`] so a specifier that names no file is retried the
/// way a bundler would resolve it.
///
/// # Why a development binary resolves differently from a production one
///
/// `esrun` resolves strictly: a specifier names the file that exists, extension
/// and all. That is right for a deployment — it is unambiguous, it does no
/// filesystem probing per import, and what it runs is a bundle whose specifiers
/// the build already settled.
///
/// It is wrong for the file a developer is editing. Source written for *any*
/// bundler — which is most of the ecosystem, and every project with
/// `"moduleResolution": "bundler"` — spells a sibling `./util`, and TypeScript's
/// own `node16` convention spells `./util.ts` as `./util.js`, a file that does
/// not exist until a build makes it. `esdev build` resolves both, because the
/// bundler does. So without this the same project *builds* and cannot be *run*,
/// and `esdev test` cannot run a test that imports its own source tree — which
/// is the whole of what a development binary is for.
///
/// **Strict resolution is tried first and always wins**, so this can only ever
/// answer a specifier that would otherwise have been an error. What it does with
/// one is what `esdev build` does with it:
///
/// * `./util` → `./util.ts`, `.tsx`, `.mts`, `.js`, `.jsx`, `.mjs`
/// * `./util` → `./util/index.*`, for a directory
/// * `./util.js` → `./util.ts`, `.tsx`, `.mts` — TypeScript's rewrite, in
///   reverse, for source that has not been through a build yet
///
/// A bare specifier is untouched: `node_modules` resolution is the loader's own
/// and already answers those.
struct BundlerStyleLoader {
    inner: Arc<dyn ModuleLoader>,
}

impl BundlerStyleLoader {
    /// Whether a resolved id names a directory rather than a module.
    ///
    /// The strict resolver confines a path and checks that it is *there*, and a
    /// directory is there — so `./src` resolves, and then fails at load with
    /// "is a directory". That answer is a miss, not a hit: what the specifier
    /// means is the directory's index, which is what the candidates below find.
    fn is_a_directory(id: &str) -> bool {
        url::Url::parse(id)
            .ok()
            .filter(|url| url.scheme() == "file")
            .and_then(|url| url.to_file_path().ok())
            .is_some_and(|path| path.is_dir())
    }

    /// The spellings to try for a specifier that named nothing, in order.
    ///
    /// Empty for anything this must not guess at — a bare name, a URL, a
    /// specifier that already carries an extension this runtime loads.
    fn candidates(specifier: &str) -> Vec<String> {
        const SOURCE: &[&str] = &["ts", "tsx", "mts", "js", "jsx", "mjs"];
        const TYPESCRIPT: &[&str] = &["ts", "tsx", "mts"];

        if !(specifier.starts_with("./")
            || specifier.starts_with("../")
            || specifier.starts_with('/'))
        {
            return Vec::new();
        }
        let last = specifier.rsplit('/').next().unwrap_or_default();
        // `./util.js` is what TypeScript tells you to write for `util.ts`, and
        // it is also a real file in a built tree. The strict attempt covered the
        // second; this is the first.
        for extension in ["js", "mjs", "jsx"] {
            if let Some(stem) = specifier.strip_suffix(&format!(".{extension}")) {
                return TYPESCRIPT
                    .iter()
                    .map(|typescript| format!("{stem}.{typescript}"))
                    .collect();
            }
        }
        if last.contains('.') {
            // An extension this runtime knows and did not find, or a name with a
            // dot in it. Either way, guessing would be inventing a file.
            return Vec::new();
        }
        let directory = specifier.trim_end_matches('/');
        SOURCE
            .iter()
            .map(|extension| format!("{specifier}.{extension}"))
            .chain(
                SOURCE
                    .iter()
                    .map(|extension| format!("{directory}/index.{extension}")),
            )
            .collect()
    }
}

impl ModuleLoader for BundlerStyleLoader {
    fn resolve(
        &self,
        specifier: &str,
        referrer: &str,
    ) -> es_runtime_providers::BoxFuture<Result<String, ProviderError>> {
        let inner = self.inner.clone();
        let specifier = specifier.to_string();
        let referrer = referrer.to_string();
        Box::pin(async move {
            let strict = match inner.resolve(&specifier, &referrer).await {
                Ok(id) if !Self::is_a_directory(&id) => return Ok(id),
                Ok(id) => ProviderError::Other(format!("cannot read {id}: it is a directory")),
                Err(strict) => strict,
            };
            for candidate in Self::candidates(&specifier) {
                if let Ok(id) = inner.resolve(&candidate, &referrer).await
                    && !Self::is_a_directory(&id)
                {
                    return Ok(id);
                }
            }
            // The strict error, not one about the last thing tried: what the
            // developer wrote is what they want to read about.
            Err(strict)
        })
    }

    /// The same answer, synchronously — `import.meta.resolve` has nowhere to
    /// await, and a loader whose two resolutions disagree is worse than one that
    /// refuses (D41).
    fn resolve_sync(
        &self,
        specifier: &str,
        referrer: &str,
    ) -> Option<Result<String, ProviderError>> {
        let strict = match self.inner.resolve_sync(specifier, referrer)? {
            Ok(id) if !Self::is_a_directory(&id) => return Some(Ok(id)),
            Ok(id) => ProviderError::Other(format!("cannot read {id}: it is a directory")),
            Err(strict) => strict,
        };
        for candidate in Self::candidates(specifier) {
            if let Some(Ok(id)) = self.inner.resolve_sync(&candidate, referrer)
                && !Self::is_a_directory(&id)
            {
                return Some(Ok(id));
            }
        }
        Some(Err(strict))
    }

    fn load(
        &self,
        specifier: &str,
    ) -> es_runtime_providers::BoxFuture<Result<ModuleSource, ProviderError>> {
        self.inner.load(specifier)
    }
}

/// Wraps a [`ModuleLoader`] so every module it returns passes through a
/// [`SourceTransform`] on the way out.
///
/// Resolution is delegated untouched — including `resolve_sync`, so
/// `import.meta.resolve` and `import()` cannot disagree (D41). Only
/// [`ModuleSource::Text`] is rewritten; a `.wasm` module is bytes and has no
/// source to strip.
struct TransformingLoader {
    inner: Arc<dyn ModuleLoader>,
    transform: Arc<dyn SourceTransform>,
}

impl ModuleLoader for TransformingLoader {
    fn resolve(
        &self,
        specifier: &str,
        referrer: &str,
    ) -> es_runtime_providers::BoxFuture<Result<String, ProviderError>> {
        self.inner.resolve(specifier, referrer)
    }

    fn resolve_sync(
        &self,
        specifier: &str,
        referrer: &str,
    ) -> Option<Result<String, ProviderError>> {
        self.inner.resolve_sync(specifier, referrer)
    }

    fn load(
        &self,
        specifier: &str,
    ) -> es_runtime_providers::BoxFuture<Result<ModuleSource, ProviderError>> {
        let inner = self.inner.clone();
        let transform = self.transform.clone();
        let specifier = specifier.to_string();
        Box::pin(async move {
            match inner.load(&specifier).await? {
                ModuleSource::Text(text) => transform
                    .transform(&specifier, text)
                    .map(ModuleSource::Text)
                    .map_err(ProviderError::Other),
                wasm => Ok(wasm),
            }
        })
    }
}

/// Runs `config` to completion. `bin` names the calling binary in the messages
/// only it can be responsible for (the shutdown drain notice).
pub async fn run(bin: &'static str, config: Config) -> Result<(), String> {
    // The capability trace is summarised however the run ends, which is why this
    // wrapper exists rather than a call at the bottom of the function below: the
    // run a developer most wants a report from is the one that *failed* for want
    // of a permission, and that one leaves through an early `return Err`.
    let observer = config.observer.clone();
    let result = execute(bin, config).await;
    finish_trace(observer.as_ref());
    result
}

/// The run itself. Everything that can end it early lives in here, so its one
/// caller above can be the single place that reports afterwards.
async fn execute(bin: &'static str, config: Config) -> Result<(), String> {
    // Returns the module's canonical specifier (a file: URL — also
    // import.meta.url and the referrer its imports resolve against), its source,
    // a short diagnostic label, and the **base directory** (the entry's own
    // directory, or cwd for `-e`) from which the loader detects the sandbox root.
    let (specifier, source, label, base_dir) = match config.source {
        Source::File(path) => {
            let code =
                std::fs::read_to_string(&path).map_err(|e| format!("cannot read {path}: {e}"))?;
            // Canonicalize the entry path (resolving relative components and
            // symlinks, and normalizing the Windows verbatim prefix) into a
            // file: URL via the shared cross-OS path layer (D25). This is a
            // filesystem path, not a module specifier, so it bypasses the
            // loader's specifier rules.
            let abs =
                path::canonicalize(&path).map_err(|e| format!("cannot resolve {path}: {e}"))?;
            let dir = abs
                .parent()
                .map(std::path::Path::to_path_buf)
                .ok_or_else(|| format!("entry path has no parent directory: {path}"))?;
            let url = path::to_file_url(&abs).map_err(|e| e.to_string())?;
            (url, code, path, dir)
        }
        Source::Inline(code) => {
            // A synthetic file: id in the working directory, so the snippet's
            // relative imports resolve against the cwd.
            let cwd = std::env::current_dir()
                .map_err(|e| format!("cannot read working directory: {e}"))?;
            let base = Url::from_directory_path(&cwd)
                .map_err(|()| "working directory is not absolute".to_string())?;
            let url = base
                .join("[eval]")
                .map_err(|e| format!("cannot derive eval specifier: {e}"))?;
            (url.to_string(), code, "<eval>".to_string(), cwd)
        }
    };

    // The entry is read directly rather than through the loader, so it needs the
    // transform applied here — otherwise `esdev app.ts` would strip every file
    // the program imports and choke on the one the user actually named.
    let source = match &config.transform {
        Some(transform) => transform
            .transform(&specifier, source)
            .map_err(|e| format!("{label}: {e}"))?,
        None => source,
    };

    let options = config.options;

    // Default providers — the standalone embedding's host surface.
    let clock = Arc::new(SystemClock::new());
    let timers = Arc::new(TokioTimers);
    // `--allow-net=<hosts>` / `--allow-listen=<addresses>` narrow the addresses
    // the guest may reach and bind (D38). Every provider that opens a socket
    // consults the same list for its half of the pair — `net` and `listen` are
    // one capability each, and which API the guest used to get there is not
    // something the policy should care about.
    let allow_net = address_scope(&config.scopes, Capability::Net)?;
    let allow_listen = address_scope(&config.scopes, Capability::NetListen)?;
    let transport = ReqwestTransport::new().map_err(|e| format!("http transport: {e}"))?;
    let net = Arc::new(match allow_net.clone() {
        Some(allow) => transport.with_allowlist(allow),
        None => transport,
    });
    // Host process view for runtime:process (env/cwd/platform from the OS; args
    // are the user's, after the script/-e). A concrete handle is kept to read
    // the exit code a guest `process.exit()` may request. The `.env` file is
    // loaded only via explicit --env-file (never auto-discovered, D30); its
    // values override the OS env only with --env-override.
    let env_overlay = match &options.env_file {
        Some(file) => dotenv::load(std::path::Path::new(file))?,
        None => Vec::new(),
    };
    // `--allow-env=<names>` narrows the environment snapshot to those names
    // (D38): the capability bit opens the door, the provider decides what is
    // behind it. Unlisted variables are absent rather than unreadable, so the
    // guest cannot even enumerate the names of the host's secrets.
    // Read before `config` is taken apart below, and used for every agent this
    // process builds: the main one here, and each worker through `spec.limits`.
    let max_heap_bytes = options.max_heap_bytes;
    let mut system_process =
        SystemProcess::new(config.args).with_env(env_overlay, options.env_override);
    if let Some(names) = config.scopes.get(&Capability::Env) {
        system_process = system_process.with_env_allowlist(names.clone());
    }
    let process = Arc::new(system_process);
    // Filesystem view for runtime:fs: relative paths resolve under the entry's
    // directory, jailed to the **working directory** (D79). One computation,
    // shared with the module loader below, so the jail and the `node_modules`
    // walk cannot disagree about where the project begins. Nothing on the
    // command line moves it: a sandbox whose boundary is an argument is a
    // boundary the deployment line can widen by accident.
    let fs_root = project_root()?;
    // The program belongs to the directory it is run from. Checked here, once,
    // because left to the loader it surfaces as *every* import escaping the
    // root — one mistake reported a hundred times, in a message about the
    // wrong thing.
    if !path::within_root(&base_dir, &fs_root) {
        return Err(format!(
            "{} is outside the project root {} — run it from its own directory \
             (the sandbox is the working directory, never an argument)",
            base_dir.display(),
            fs_root.display()
        ));
    }
    // `--allow-read=<paths>` / `--allow-write=<paths>` narrow the jail (D38).
    // Entries are resolved against the *working directory*, because that is
    // where the user typed them; the jail's own base is the entry file's
    // directory, which is not what `./data` means on a command line.
    let flag_dir = std::env::current_dir().unwrap_or_else(|_| base_dir.clone());
    let allow_read = path_scope(&config.scopes, Capability::FileRead, &flag_dir)?;
    let allow_write = path_scope(&config.scopes, Capability::FileWrite, &flag_dir)?;
    // An entry outside the jail is not an error: it *adds* that subtree (D54).
    // The jail is still the default boundary and guest code can never move it —
    // only a path typed here can, which is the deployment operator naming a
    // location the project does not contain. A TLS certificate under
    // /etc/letsencrypt is the case this exists for.
    // Both filesystem views take the same lists: `runtime:fs` and `runtime:wasi`
    // are two doors onto one filesystem, and a policy that differed between
    // them would be a bug wearing a feature's clothes.
    let mut file_system = SystemFileSystem::new(&base_dir, &fs_root);
    let mut sync_file_system = SystemSyncFileSystem::new(&base_dir, &fs_root);
    if let Some(allow) = &allow_read {
        file_system = file_system.with_read_allowlist(allow.clone());
        sync_file_system = sync_file_system.with_read_allowlist(allow.clone());
    }
    if let Some(allow) = &allow_write {
        file_system = file_system.with_write_allowlist(allow.clone());
        sync_file_system = sync_file_system.with_write_allowlist(allow.clone());
    }
    let file_system = Arc::new(file_system);
    // The same view, synchronously, for `runtime:wasi` — WASI's syscalls cannot
    // await. Same base and same jail, so both paths agree on what is reachable.
    let sync_file_system = Arc::new(sync_file_system);
    // Held here as well as in the providers: the interrupt watcher below needs
    // to ask the signal registry what the guest is watching, and to tell the
    // HTTP servers to stop accepting.
    // `--allow-signals=<names>` narrows which signals may be watched. A watch
    // suppresses the default action, so this is the privilege to decline to die
    // on request, granted one signal at a time.
    let signals = Arc::new(match signal_scope(&config.scopes) {
        Some(names) => SystemSignals::new().with_allowlist(names),
        None => SystemSignals::new(),
    });
    let http_server = Arc::new(match allow_listen.clone() {
        Some(allow) => SystemHttpServer::new().with_listen_allowlist(allow),
        None => SystemHttpServer::new(),
    });
    let mut system_net = SystemNet::new();
    if let Some(allow) = allow_net.clone() {
        system_net = system_net.with_allowlist(allow);
    }
    if let Some(allow) = allow_listen {
        system_net = system_net.with_listen_allowlist(allow);
    }
    let web_socket = match allow_net {
        Some(allow) => SystemWebSocket::new().with_allowlist(allow),
        None => SystemWebSocket::new(),
    };
    let commands = match config.scopes.get(&Capability::Run) {
        Some(programs) => SystemCommands::new().with_allowlist(programs.clone()),
        None => SystemCommands::new(),
    };
    let providers = HostProviders::new(
        clock.clone(),
        Arc::new(StdoutConsole),
        net,
        Arc::new(OsEntropy),
    )
    .with_process(process.clone())
    .with_signals(signals.clone())
    .with_file_system(file_system.clone())
    // `runtime:db`'s embedded engine resolves through the *same* filesystem
    // view, so a database is scoped by `--allow-read`/`--allow-write` exactly
    // as a file is — and the write-ahead log the engine opens beside it is
    // judged by the same list, rather than by nothing.
    .with_embedded_db(Arc::new(SystemEmbeddedDb::new(file_system.clone())))
    .with_sync_file_system(sync_file_system)
    .with_net_provider(Arc::new(system_net))
    .with_http_server(http_server.clone())
    .with_web_socket(Arc::new(web_socket))
    // Child processes for runtime:system. Unrestricted unless
    // `--allow-run=<programs>` named the ones that may be spawned (D38) — the
    // same provider seam an embedder uses to grant Run without granting a shell.
    .with_commands(Arc::new(commands))
    // BroadcastChannel's agent cluster is this process: every worker the binary
    // starts shares the hub, so a channel opened in one reaches the rest.
    .with_broadcast(Arc::new(ProcessBroadcastHub::new()))
    // MessagePort queues, so a port transferred into a worker keeps working.
    .with_ports(Arc::new(ProcessPortHub::new()));
    // Module loader: relative/absolute/file: specifiers resolve as local files,
    // bare specifiers through node_modules (ESM packages only). Based at the
    // entry's directory and rooted at the same project root the filesystem is
    // jailed to — the working directory's project (D25, D79) — so resolution is
    // confined under it and the node_modules walk stops there. Held behind an
    // Arc so dynamic import() can reach it.
    // `--import-policy=<file>` governs what the loader may resolve (D39) — a
    // layer above the `imports` capability, which governs whether it runs at
    // all. The entry file is unaffected: it is read before a loader exists, and
    // the user named it on the command line.
    let mut loader_impl = NodeModuleLoader::with_base_and_root(&base_dir, &fs_root)
        .map_err(|e| format!("module loader: {e}"))?;
    if let Some(file) = &options.import_policy {
        loader_impl = loader_impl.with_policy(ImportPolicy::from_file(std::path::Path::new(file))?);
    }
    let resolved: Arc<dyn ModuleLoader> = if config.bundler_style_resolution {
        Arc::new(BundlerStyleLoader {
            inner: Arc::new(loader_impl),
        })
    } else {
        Arc::new(loader_impl)
    };
    let loader: Arc<dyn ModuleLoader> = match &config.transform {
        Some(transform) => Arc::new(TransformingLoader {
            inner: resolved,
            transform: transform.clone(),
        }),
        None => resolved,
    };

    // Workers. Each gets its own thread and its own isolate, built by this
    // factory *on that thread* — `V8Engine` is `!Send`, so it cannot be built
    // here and moved. Passing the factory rather than reaching for the engine
    // inside the provider is what keeps the worker path engine-agnostic.
    //
    // The snapshot is shared, not copied: `&'static [u8]` from `include_bytes!`,
    // so a worker restores the same blob the main agent did and starts as
    // cheaply.
    let worker_providers = providers.clone();
    let worker_process = process.clone();
    let worker_loader = loader.clone();
    let worker_observer = config.observer.clone();
    // Late-bound, because the host and the runtimes it builds each need the
    // other: a worker must itself be able to start workers (the spec allows
    // nesting, and the capability chain is what bounds it), so the bundle its
    // runtime gets has to name the very host that is being constructed here.
    // Filled in immediately below, and only read on a worker thread — long
    // after.
    let worker_host_slot: Arc<std::sync::OnceLock<Arc<dyn es_runtime_providers::WorkerHost>>> =
        Arc::new(std::sync::OnceLock::new());
    let factory_slot = worker_host_slot.clone();
    let workers = Arc::new(ThreadWorkerHost::new(Arc::new(
        move |spec: &WorkerSpec, scope: Arc<dyn WorkerScope>| {
            let mut providers = worker_providers
                .clone()
                .with_worker_scope(scope)
                // `exit()` inside a worker stops that worker, not the program:
                // halting is already per-agent, but the exit *code* is recorded
                // on a shared provider, so a worker would otherwise decide what
                // the process exits with.
                .with_process(Arc::new(
                    WorkerProcess::new(worker_process.clone()).with_env(spec.env.clone()),
                ));
            if let Some(host) = factory_slot.get() {
                providers = providers.with_workers(host.clone());
            }
            let mut runtime = Runtime::with_snapshot_and_limits(SNAPSHOT, spec.limits, providers)
                .map_err(|e| ProviderError::Other(e.to_string()))?;
            // A worker is traced too, into the same report. Its grants are set
            // at the spawn rather than on the command line, which is exactly
            // where they are hardest to get right — leaving them out would make
            // the trace confidently wrong about a program that uses workers.
            if let Some(observer) = &worker_observer {
                runtime.set_capability_observer(observer.clone());
            }
            Ok((runtime, worker_loader.clone()))
        },
    )));
    let _ = worker_host_slot.set(workers.clone());
    let providers = providers.with_workers(workers.clone());

    // Restore the prelude from the snapshot baked in at build time (build.rs)
    // instead of compiling + evaluating it — the bulk of construction cost.
    let mut runtime =
        Runtime::with_snapshot_and_limits(SNAPSHOT, heap_limits(max_heap_bytes), providers)
            .map_err(|e| e.to_string())?;
    // A local script is trusted by default: the full capability set (incl.
    // FileSystem, which module loading requires). `--deny-all` / `--deny-<name>`
    // narrow it (D38); the entry file has already been read by this point, so a
    // fully denied run still executes what the user named.
    runtime.set_capabilities(config.capabilities);
    if let Some(observer) = &config.observer {
        runtime.set_capability_observer(observer.clone());
    }

    // The binary's own additions to the `runtime:` namespace (`esdev`'s bundler
    // and watcher). After the capability set, so an op that asks what it is
    // allowed to do at registration reads the real answer; before the entry
    // module, because the entry is free to import one on its first line.
    let extension_ctx = ExtensionContext {
        file_system: file_system.clone(),
        base_dir: &base_dir,
    };
    for extension in &config.extensions {
        for op in extension.ops(&extension_ctx) {
            runtime.register_op(op).map_err(|e| e.to_string())?;
        }
        for module in extension.modules() {
            runtime
                .register_module(module.specifier, module.source)
                .map_err(|e| e.to_string())?;
        }
    }

    // Attach the debugger before the entry module is loaded: V8 announces each
    // script to a session as it is compiled, so a debugger that arrives later
    // sees an empty Sources pane. With `--inspect-brk` this blocks here, which
    // is the point — the program has not run a statement yet.
    if let Some(inspector) = config.inspector {
        runtime
            .attach_inspector(
                inspector.transport,
                &es_runtime::InspectorOptions {
                    wait_for_debugger: inspector.wait,
                    context_name: label.clone(),
                },
            )
            .map_err(|e| e.to_string())?;
    }

    // Graceful shutdown on ^C / SIGTERM. Installed before the module runs, so a
    // server that binds immediately is covered from its first request.
    spawn_shutdown_watcher(
        bin,
        signals,
        http_server.clone(),
        runtime.interrupt_handle(),
        options.shutdown_grace,
    );

    // Execution-time watchdog (SPEC §4): a separate thread terminates the engine
    // after the deadline. Cross-thread V8 termination means even a synchronous
    // infinite loop in a module's top level is stopped. `timed_out` lets us
    // report a timeout distinctly from an ordinary error.
    let timed_out = Arc::new(AtomicBool::new(false));
    if let Some(deadline) = options.timeout {
        let handle = runtime.interrupt_handle();
        let flag = timed_out.clone();
        std::thread::spawn(move || {
            std::thread::sleep(deadline);
            flag.store(true, Ordering::SeqCst);
            handle.terminate();
        });
    }

    // Load the module graph (resolving + reading any imports) and begin
    // evaluating it. Top-level await is native to modules, so no wrapper is
    // needed. A compile/instantiation error or a missing import surfaces here;
    // a top-level throw rejects the evaluation, observed after the drive below.
    let load = runtime.load_module_source(&specifier, &source, loader);
    let loaded = match options.timeout {
        Some(deadline) => match tokio::time::timeout(deadline, load).await {
            Ok(result) => result,
            Err(_) => {
                runtime.interrupt_handle().terminate();
                return Err(timeout_message(options.timeout));
            }
        },
        None => load.await,
    };
    // A guest `process.exit(code)` during the synchronous top level halts the
    // load via the interrupt; exit with that code (not as an error).
    if let Some(code) = process.requested_exit_code() {
        finish_trace(config.observer.as_ref());
        std::process::exit(code);
    }
    if let Err(err) = loaded {
        if timed_out.load(Ordering::SeqCst) {
            return Err(timeout_message(options.timeout));
        }
        return Err(format!("{label}: {err}"));
    }

    // Drive async work (top-level await, fetch, setTimeout, promise reactions)
    // to quiescence. The timeout is a backstop for runaways that live in async
    // callbacks, which yield to the executor (where a blocking watchdog can't
    // preempt them).
    // Failures are handed over *as they happen* rather than collected for a
    // quiescence that a server never reaches: an unhandled rejection or a throw
    // out of a timer in a long-running program was only printed when the
    // process finally exited, which for a listening server is never. They are
    // still counted, so the exit status is unchanged.
    // Buffered for one tick rather than printed from the sink directly, because
    // the entry module's *own* top-level throw also arrives here as an unhandled
    // rejection — and that failure is reported once, below, as an uncaught
    // exception naming the file. Holding each batch until the module's
    // evaluation has settled is what lets that one be dropped instead of
    // printed twice; for a long-running server (the case this exists for) the
    // module settled long ago and a tick is no delay at all.
    let pending = Arc::new(Mutex::new(Vec::<Failure>::new()));
    let sink = pending.clone();
    let driver = Driver::new(clock, timers).reporting_failures_to(move |failure| {
        let (headline, error) = match failure {
            DriveFailure::UncaughtError(e) => ("uncaught exception in a timer callback", e),
            DriveFailure::UnhandledRejection(e) => ("unhandled promise rejection", e),
            // The enum is non-exhaustive; anything added later is still reported
            // rather than silently dropped.
            other => {
                sink.lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(Failure {
                        text: format!("error: {other:?}"),
                        body: String::new(),
                    });
                return;
            }
        };
        let body = error.to_string();
        sink.lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(Failure {
                text: format!("error: {headline}\n{body}"),
                body,
            });
    });
    let reported = Arc::new(AtomicI32::new(0));
    // Stopped as soon as the entry module's evaluation *fails*, rather than at
    // quiescence. A program whose top-level code threw has already failed, and
    // anything it started before throwing — a server holding a listener is the
    // ordinary case — keeps the loop alive forever, so waiting for the drive to
    // return meant the exception was never reported and the process never
    // exited. It ran on, serving, with the error discarded.
    //
    // Reported below by the existing `ModuleEvalState::Failed` check, which
    // until now could only be reached by programs that happened to quiesce.
    let flush_pending = pending.clone();
    let flush_count = reported.clone();
    let drive = driver.drive_while(&mut runtime, move |rt| {
        let state = rt.module_eval_state();
        if !matches!(state, ModuleEvalState::Pending) {
            flush_failures(&flush_pending, &flush_count, &state);
        }
        !matches!(state, ModuleEvalState::Failed(_))
    });
    let outcome = match options.timeout {
        Some(deadline) => match tokio::time::timeout(deadline, drive).await {
            Ok(outcome) => outcome,
            Err(_) => {
                runtime.interrupt_handle().terminate();
                return Err(timeout_message(options.timeout));
            }
        },
        None => drive.await,
    };

    // A guest `process.exit(code)` from async code halts the drive via the
    // interrupt; exit with that code rather than reporting the termination.
    if let Some(code) = process.requested_exit_code() {
        finish_trace(config.observer.as_ref());
        std::process::exit(code);
    }

    // The drive returned because a graceful shutdown drained the servers. The
    // guest is done, but its last responses have only been *handed* to the HTTP
    // transport — exiting now would turn them into empty replies, which is the
    // very failure the drain exists to prevent. Wait for the connections to
    // close, then report the interruption in the status an orchestrator reads.
    let shutdown_code = SHUTDOWN_CODE.load(Ordering::SeqCst);
    if shutdown_code != 0 {
        if !http_server.wait_for_idle(options.shutdown_grace).await {
            eprintln!("{bin}: shutdown grace expired with requests still in flight");
        }
        finish_trace(config.observer.as_ref());
        std::process::exit(shutdown_code);
    }

    // A top-level throw (or a rejected top-level await) fails the module's
    // evaluation. Report it as the primary error — its rejection also shows up
    // in `rejections`, so it is the one uncaught-rejection we don't re-report.
    if let ModuleEvalState::Failed(message) = runtime.module_eval_state() {
        return Err(format!("uncaught exception in {label}\n{message}"));
    }

    // Anything the drive stopped before flushing, on the same terms.
    flush_failures(&pending, &reported, &runtime.module_eval_state());

    // Everything was printed the moment it happened, so this is only the exit
    // status: repeating the messages would report each failure twice.
    let count = reported.load(Ordering::SeqCst);
    if count > 0 {
        let plural = if count == 1 { "" } else { "s" };
        return Err(format!(
            "{count} unhandled failure{plural} — reported above"
        ));
    }
    let _ = &outcome;
    Ok(())
}

/// The one root a run has: **the working directory**, exactly (D79).
///
/// No walk and no marker file. The cwd rather than the entry file, because an
/// entry is a path someone typed and a root derived from one moves when the
/// argument moves; and the cwd *itself* rather than the project detected around
/// it, because not walking is what makes the boundary safe — a `package.json`
/// two directories up is not a permission, and looking for one only means the
/// jail can silently be wider than the directory the operator is standing in.
///
/// A missing manifest is therefore not an error: an image holding `dist/` and
/// `node_modules/` and no `package.json` is a perfectly ordinary deployment,
/// and its jail is that directory either way.
///
/// Two working directories are refused instead, because they are enormous by
/// nature and being in one is always an accident:
/// - a **filesystem root** — an unset `WORKDIR` / missing `WorkingDirectory=`,
///   where the jail would be every file on the machine;
/// - the **home directory** — cron's default, where it would be every key,
///   credential and profile the user owns.
fn project_root() -> Result<std::path::PathBuf, String> {
    let cwd = std::env::current_dir().map_err(|e| format!("cannot read working directory: {e}"))?;
    let root = path::canonicalize(&cwd).unwrap_or(cwd);
    if root.parent().is_none() {
        return Err(format!(
            "refusing to run with {} as the project root — the sandbox is the \
             working directory, and this one is the whole filesystem.\n\n\
             Set the application directory: WORKDIR in a container, \
             WorkingDirectory= in a systemd unit, or cd there first.",
            root.display()
        ));
    }
    if home_dir().is_some_and(|home| home == root) {
        return Err(format!(
            "refusing to run with your home directory ({}) as the project root — \
             the sandbox is the working directory, and that one holds every key \
             and credential you own.\n\n\
             Run from the application's own directory (cron starts in $HOME; \
             give the job a cd, or the unit a WorkingDirectory=).",
            root.display()
        ));
    }
    Ok(root)
}

/// The user's home directory, canonicalized, from the environment the process
/// was started in. `None` when it is unset or unreadable — one fewer directory
/// refused, never a run blocked by a variable that was not there.
fn home_dir() -> Option<std::path::PathBuf> {
    let raw = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|value| !value.is_empty())?;
    path::canonicalize(std::path::PathBuf::from(raw)).ok()
}

/// Tells the capability observer, if there is one, that the run is over.
///
/// Called once by [`run`] for every ordinary way out, and explicitly at the
/// three that leave through `process::exit` — which no wrapper can catch, and
/// which for a server stopped with ^C is *every* run of the thing this exists to
/// help with. Implementations report at most once, so both paths firing is safe.
fn finish_trace(observer: Option<&es_runtime::SharedObserver>) {
    if let Some(observer) = observer {
        observer.run_finished();
    }
}

/// The ceilings this run's agents are built with.
///
/// A standalone binary is not the embeddable library: it *is* the process, so it
/// takes the machine's answer rather than the library's conservative 256 MiB —
/// which on a 16 GiB host is a sixteenth of what Node and Deno would give the
/// same script, and does not move when the host does. `--max-heap=<mb>` pins it
/// instead.
///
/// It applies to workers as well, because a worker derives its limits from the
/// agent that started it: one number bounds the process, however many agents it
/// ends up with.
fn heap_limits(max_heap_bytes: Option<usize>) -> es_runtime_common::Limits {
    let limits = es_runtime_common::Limits::default();
    match max_heap_bytes {
        Some(bytes) => limits.with_heap_limit_bytes(bytes),
        None => limits.with_system_heap_limit(),
    }
}

#[cfg(test)]
mod bundler_style {
    use super::BundlerStyleLoader as Loader;

    /// A relative specifier with no extension is the whole reason this exists.
    #[test]
    fn an_extensionless_relative_specifier_gets_the_source_spellings() {
        let tried = Loader::candidates("./util");
        assert_eq!(tried.first().map(String::as_str), Some("./util.ts"));
        assert!(tried.contains(&"./util.tsx".to_string()));
        assert!(tried.contains(&"./util.js".to_string()));
        // …and then the directory it might be.
        assert!(tried.contains(&"./util/index.ts".to_string()));
        // TypeScript first: a project mid-migration has both, and the source
        // is the file being edited.
        let ts = tried.iter().position(|c| c == "./util.ts");
        let js = tried.iter().position(|c| c == "./util.js");
        assert!(ts < js, "{tried:?}");
    }

    /// `../../src` — a directory, which the strict resolver *finds* and then
    /// fails to read. Its index is what the specifier meant.
    #[test]
    fn a_directory_is_tried_as_its_index() {
        let tried = Loader::candidates("../../src");
        assert!(tried.contains(&"../../src/index.ts".to_string()), "{tried:?}");
        assert!(
            Loader::candidates("./src/").contains(&"./src/index.ts".to_string()),
            "a trailing slash must not double up"
        );
    }

    /// TypeScript's own advice is to import `./util.js` for `util.ts`, so
    /// source that has not been built yet says `.js` and means `.ts`.
    #[test]
    fn a_js_specifier_falls_back_to_the_typescript_that_will_become_it() {
        assert_eq!(
            Loader::candidates("./util.js"),
            vec!["./util.ts", "./util.tsx", "./util.mts"]
        );
        assert_eq!(
            Loader::candidates("./util.mjs"),
            vec!["./util.ts", "./util.tsx", "./util.mts"]
        );
    }

    /// What it must **not** answer. A bare name is the loader's own business,
    /// and a typo with an extension is an error rather than an invitation to
    /// invent a file.
    #[test]
    fn nothing_else_is_guessed_at() {
        assert!(Loader::candidates("lodash").is_empty());
        assert!(Loader::candidates("runtime:fs").is_empty());
        assert!(Loader::candidates("https://example.com/m.js").is_empty());
        // `./util.tsx` was tried strictly and was not there.
        assert!(Loader::candidates("./util.tsx").is_empty());
        // A name with a dot in it is a name, not an extension to strip.
        assert!(Loader::candidates("./v1.2/thing.css").is_empty());
    }

    /// An absolute path is as relative as a relative one, for this purpose.
    #[test]
    fn an_absolute_path_is_answered_too() {
        assert!(Loader::candidates("/p/src/util").contains(&"/p/src/util.ts".to_string()));
    }
}
