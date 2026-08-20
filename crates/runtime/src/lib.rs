//! The driven runtime (ARCHITECTURE.md §5; DECISIONS.md D4).
//!
//! [`Runtime`] wires host ops into the engine and exposes the **tick/poll** API
//! the embedder drives. It owns no thread and no loop of its own: one
//! [`Runtime::tick`] advances the world by one step and returns; the embedder
//! decides when to call it again — owning no thread is what keeps the loop
//! testable and its ordering deterministic.
//!
//! The runtime is built on the [`Engine`](es_runtime_engine::Engine) abstraction
//! and names **no** V8 type (DECISIONS.md D3): a second engine could be slotted
//! in without changing this crate. The V8-coupled op/promise/timer machinery
//! lives in `engine`; here we own the orchestration and the timer schedule.

// No `unsafe` in the runtime; it is confined to `engine` (ARCHITECTURE.md §7).
#![forbid(unsafe_code)]

mod base64_ops;
mod builtins;
mod compression_ops;
mod crypto_ops;
mod curve25519_ops;
mod db_ops;
mod ec_ops;
mod encoding_ops;
mod fetch_ops;
mod fs_ops;
#[cfg(feature = "fuzzing")]
#[doc(hidden)]
pub mod fuzz;
mod handles;
mod hashing_ops;
mod http_ops;
mod module_ops;
mod msgpack;
mod net_ops;
mod prelude;
mod process_ops;
mod rsa_ops;
mod runtime_modules;
pub mod serialization_ops;
mod sync_fs_ops;
mod system_ops;
mod timer;
mod url_ops;
mod urlpattern_ops;
mod worker_ops;
mod ws_ops;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use crate::timer::TimerQueue;

// One-stop public surface for embedders: the engine abstraction + impl, the op
// types, values, capabilities, and the provider traits — all reachable here.
pub use es_runtime_common::{Capability, CapabilitySet, UncaughtError};
pub use es_runtime_engine::{
    AsyncOp, CapabilityObserver, Engine, InspectorOptions, InspectorTransport, InterruptHandle,
    ModuleEvalState, ModuleId, OpDecl, OpError, OpResult, SharedObserver, V8Engine, Value,
};
pub use es_runtime_providers::{
    BroadcastHub, ChildStatus, ChildStream, Clock, CommandProvider, CommandSpec, Console,
    ConsoleLevel, EmbeddedDb, Entropy, FileSystem, HttpServerProvider, ModuleLoader, ModuleSource,
    NetProvider, NetTransport, PortHub, Process, Signals, Stdio, SyncFileSystem, WebSocketProvider,
    WorkerHost, WorkerScope,
};

/// Runtime-layer error (DECISIONS.md D12).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// An error from the engine layer, surfaced through the runtime.
    #[error(transparent)]
    Engine(#[from] es_runtime_engine::Error),

    /// A lower-layer (`common`) error surfaced directly by the runtime — e.g. a
    /// capability the runtime itself gates (module loading needs `FileSystem`).
    #[error(transparent)]
    Common(#[from] es_runtime_common::Error),

    /// A module could not be resolved or loaded by the [`ModuleLoader`] while
    /// building a module graph ([`Runtime::load_module_source`]).
    #[error("module loading failed: {0}")]
    ModuleLoad(String),

    /// An import was refused because the agent lacks the `FileSystem`
    /// capability, worded for the agent that hit it — see
    /// [`Runtime::require_module_capability`].
    ///
    /// Separate from [`Common`](Self::Common) only to carry that wording: it
    /// surfaces as the same `NotAllowedError` with the same
    /// [`ErrorCode::PermissionDenied`](es_runtime_common::ErrorCode), so
    /// anything catching a permission failure is unaffected.
    #[error("{0}")]
    ImportDenied(String),
}

impl es_runtime_common::IntoException for Error {
    fn exception_class(&self) -> es_runtime_common::ExceptionClass {
        match self {
            Error::Engine(e) => e.exception_class(),
            Error::Common(e) => e.exception_class(),
            Error::ModuleLoad(_) => es_runtime_common::ExceptionClass::Error,
            Error::ImportDenied(_) => es_runtime_common::ExceptionClass::NOT_ALLOWED,
        }
    }

    fn exception_code(&self) -> Option<es_runtime_common::ErrorCode> {
        match self {
            Error::Engine(e) => e.exception_code(),
            Error::Common(e) => e.exception_code(),
            Error::ModuleLoad(_) => None,
            // The same code the bare capability denial carried, so a guest that
            // branches on `e.code` sees no difference from the better wording.
            Error::ImportDenied(_) => {
                es_runtime_common::Error::CapabilityDenied(Capability::FileSystem).exception_code()
            }
        }
    }
}

/// Runtime result alias.
pub type Result<T> = std::result::Result<T, Error>;

/// The outcome of one [`Runtime::tick`].
///
/// Lets the embedder learn what happened and decide whether to park: when
/// [`has_pending_work`](Self::has_pending_work) is `false` and
/// [`next_timer_deadline_ms`](Self::next_timer_deadline_ms) is `None`, there is
/// nothing to do until new work is submitted.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct TickStatus {
    /// Timer callbacks invoked this tick.
    pub timers_fired: usize,
    /// Async ops whose promises were settled this tick.
    pub async_ops_settled: usize,
    /// Promise rejections that went unhandled this tick — those the guest did
    /// not claim with `preventDefault()` on an `unhandledrejection` listener.
    pub unhandled_rejections: Vec<UncaughtError>,
    /// Exceptions that escaped a timer callback this tick and that no `error`
    /// listener claimed. A timer throw has no caller to propagate to, so this is
    /// the only place it surfaces.
    pub uncaught_errors: Vec<UncaughtError>,
    /// Whether any async op or timer remains after this tick.
    pub has_pending_work: bool,
    /// The earliest pending timer deadline (embedder ms), if any — a hint for
    /// how long the embedder may park.
    pub next_timer_deadline_ms: Option<u64>,
    /// Whether V8 is still finishing work on its *own* background threads —
    /// async WebAssembly compilation today.
    ///
    /// This is pending work no waker can announce. An op future signals the
    /// driver's waker when it becomes ready; V8 instead posts a foreground task
    /// that only [`tick`](Runtime::tick) discovers, and it does so without
    /// touching anything the embedder is parked on. An embedder that parks on a
    /// timeout while this is set therefore adds its whole park to the latency of
    /// every compile — so it should re-tick promptly instead of sleeping.
    pub v8_background_work: bool,
    /// Whether execution has been **terminated** — `process.exit()`, a
    /// watchdog, a `Worker.terminate()`, or the heap guard.
    ///
    /// An embedder must stop driving when this is set, and cannot decide it from
    /// [`has_pending_work`](Self::has_pending_work) instead. A termination
    /// unwinds whatever was running without settling it: a module suspended at
    /// a top-level `await` stays pending, an op stays outstanding, and no
    /// further JS can run to finish either. The loop would then have work that
    /// can never complete, and would spin on it for as long as the process
    /// lives.
    pub terminated: bool,
}

/// The host providers a [`Runtime`] consumes for its web APIs.
///
/// Phase 4 consumes a [`Clock`] (for `performance`) and a [`Console`] sink (for
/// `console.*`). Further providers are bundled here as the APIs that need them
/// land (Entropy → Phase 7, NetTransport → Phase 6). Cloning is cheap (the
/// providers are behind `Arc`).
#[derive(Clone)]
pub struct HostProviders {
    clock: Arc<dyn Clock>,
    console: Arc<dyn Console>,
    net: Arc<dyn NetTransport>,
    entropy: Arc<dyn Entropy>,
    process: Option<Arc<dyn Process>>,
    signals: Option<Arc<dyn Signals>>,
    embedded_db: Option<Arc<dyn EmbeddedDb>>,
    file_system: Option<Arc<dyn FileSystem>>,
    sync_file_system: Option<Arc<dyn SyncFileSystem>>,
    net_provider: Option<Arc<dyn NetProvider>>,
    http_server: Option<Arc<dyn HttpServerProvider>>,
    web_socket: Option<Arc<dyn WebSocketProvider>>,
    commands: Option<Arc<dyn CommandProvider>>,
    workers: Option<Arc<dyn WorkerHost>>,
    worker_scope: Option<Arc<dyn WorkerScope>>,
    broadcast: Option<Arc<dyn BroadcastHub>>,
    ports: Option<Arc<dyn PortHub>>,
}

impl HostProviders {
    /// Bundles the providers a runtime needs (clock, console, net, entropy).
    /// Host process info (`runtime:process`) is opt-in via
    /// [`with_process`](Self::with_process); absent, the `runtime:process` ops
    /// fail cleanly (like a denied capability).
    pub fn new(
        clock: Arc<dyn Clock>,
        console: Arc<dyn Console>,
        net: Arc<dyn NetTransport>,
        entropy: Arc<dyn Entropy>,
    ) -> Self {
        HostProviders {
            clock,
            console,
            net,
            entropy,
            process: None,
            signals: None,
            embedded_db: None,
            file_system: None,
            sync_file_system: None,
            net_provider: None,
            http_server: None,
            web_socket: None,
            commands: None,
            workers: None,
            worker_scope: None,
            broadcast: None,
            ports: None,
        }
    }

    /// Adds the [`Process`] view backing `runtime:process` (env/args/cwd/
    /// platform/exit). Capability-gated on [`Capability::Env`].
    #[must_use]
    pub fn with_process(mut self, process: Arc<dyn Process>) -> Self {
        self.process = Some(process);
        self
    }

    /// Adds the [`Signals`] source backing `runtime:process` `onSignal`.
    /// Capability-gated on [`Capability::Signals`](es_runtime_common::Capability::Signals) —
    /// separately from [`Env`](es_runtime_common::Capability::Env), because a
    /// watch suppresses the signal's default action rather than reading state.
    /// Absent, the signal ops fail cleanly like a denied capability.
    #[must_use]
    pub fn with_signals(mut self, signals: Arc<dyn Signals>) -> Self {
        self.signals = Some(signals);
        self
    }

    /// Adds the [`EmbeddedDb`] backing `runtime:db`'s embedded schemes
    /// (`sqlite:`). Opening is capability-gated on
    /// [`FileRead`](es_runtime_common::Capability::FileRead), and additionally on
    /// [`FileWrite`](es_runtime_common::Capability::FileWrite) unless the open is
    /// read-only — a database is a file, and is scoped as one. Networked
    /// backends need nothing here: they are JS over the [`NetProvider`].
    #[must_use]
    pub fn with_embedded_db(mut self, embedded_db: Arc<dyn EmbeddedDb>) -> Self {
        self.embedded_db = Some(embedded_db);
        self
    }

    /// Adds the [`FileSystem`] view backing `runtime:fs`. Reads are
    /// capability-gated on [`Capability::FileRead`](es_runtime_common::Capability::FileRead)
    /// and mutations on [`FileWrite`](es_runtime_common::Capability::FileWrite);
    /// the provider confines all access to its root jail.
    #[must_use]
    pub fn with_file_system(mut self, file_system: Arc<dyn FileSystem>) -> Self {
        self.file_system = Some(file_system);
        self
    }

    /// Adds the [`SyncFileSystem`] backing `runtime:wasi`'s file calls.
    ///
    /// Separate from [`with_file_system`](Self::with_file_system) because WASI's
    /// syscalls are synchronous and cannot await; gated on the same
    /// [`FileRead`](es_runtime_common::Capability::FileRead) /
    /// [`FileWrite`](es_runtime_common::Capability::FileWrite) capabilities.
    /// Without one, WASI reports `ENOTCAPABLE` for every file call.
    #[must_use]
    pub fn with_sync_file_system(mut self, sync_file_system: Arc<dyn SyncFileSystem>) -> Self {
        self.sync_file_system = Some(sync_file_system);
        self
    }

    /// Adds the [`NetProvider`] backing `runtime:net` (TCP sockets + listeners).
    /// `connect` is capability-gated on [`Capability::Net`](es_runtime_common::Capability::Net),
    /// `listen` on [`NetListen`](es_runtime_common::Capability::NetListen).
    #[must_use]
    pub fn with_net_provider(mut self, net_provider: Arc<dyn NetProvider>) -> Self {
        self.net_provider = Some(net_provider);
        self
    }

    /// Adds the [`HttpServerProvider`] backing `runtime:http` (`serve`). Binding
    /// a server is capability-gated on
    /// [`NetListen`](es_runtime_common::Capability::NetListen), like
    /// `runtime:net` `listen`.
    #[must_use]
    pub fn with_http_server(mut self, http_server: Arc<dyn HttpServerProvider>) -> Self {
        self.http_server = Some(http_server);
        self
    }

    /// Adds the [`WebSocketProvider`] backing the `WebSocket` global (DECISIONS
    /// D29). `connect` is capability-gated on
    /// [`Capability::Net`](es_runtime_common::Capability::Net), the same gate as
    /// `fetch` and `runtime:net` `connect`. Absent, `new WebSocket(...)` fails
    /// cleanly (an `error`/`close` with code 1006), like a denied capability.
    #[must_use]
    pub fn with_web_socket(mut self, web_socket: Arc<dyn WebSocketProvider>) -> Self {
        self.web_socket = Some(web_socket);
        self
    }

    /// Adds the [`CommandProvider`] backing `runtime:system` (child processes,
    /// DECISIONS D37). Spawning is capability-gated on
    /// [`Capability::Run`](es_runtime_common::Capability::Run) — the grant that
    /// starts a process outside every confinement this runtime applies, so it is
    /// never implied by another capability. Absent, the `runtime:system` ops
    /// fail cleanly, like a denied capability.
    #[must_use]
    pub fn with_commands(mut self, commands: Arc<dyn CommandProvider>) -> Self {
        self.commands = Some(commands);
        self
    }

    /// Adds the [`WorkerHost`] backing the `Worker` global, gated on
    /// [`Capability::Worker`]. Absent, `new Worker(...)` fails cleanly, like a
    /// denied capability — an embedder that installs no host has no workers.
    #[must_use]
    pub fn with_workers(mut self, workers: Arc<dyn WorkerHost>) -> Self {
        self.workers = Some(workers);
        self
    }

    /// Marks this runtime as **being** a worker agent, and gives it its channel
    /// back to the parent.
    ///
    /// Install this only on a runtime a [`WorkerHost`] is constructing. Its
    /// presence is what puts `postMessage`/`onmessage`/`close()` on the global
    /// scope, so setting it on the agent that drives the process would claim
    /// that agent has a parent to talk to.
    #[must_use]
    pub fn with_worker_scope(mut self, scope: Arc<dyn WorkerScope>) -> Self {
        self.worker_scope = Some(scope);
        self
    }

    fn clock(&self) -> Arc<dyn Clock> {
        self.clock.clone()
    }

    fn console(&self) -> Arc<dyn Console> {
        self.console.clone()
    }

    fn net(&self) -> Arc<dyn NetTransport> {
        self.net.clone()
    }

    fn entropy(&self) -> Arc<dyn Entropy> {
        self.entropy.clone()
    }

    fn process(&self) -> Option<Arc<dyn Process>> {
        self.process.clone()
    }

    fn signals(&self) -> Option<Arc<dyn Signals>> {
        self.signals.clone()
    }

    fn embedded_db(&self) -> Option<Arc<dyn EmbeddedDb>> {
        self.embedded_db.clone()
    }

    fn file_system(&self) -> Option<Arc<dyn FileSystem>> {
        self.file_system.clone()
    }

    fn sync_file_system(&self) -> Option<Arc<dyn SyncFileSystem>> {
        self.sync_file_system.clone()
    }

    fn net_provider(&self) -> Option<Arc<dyn NetProvider>> {
        self.net_provider.clone()
    }

    fn http_server(&self) -> Option<Arc<dyn HttpServerProvider>> {
        self.http_server.clone()
    }

    fn web_socket(&self) -> Option<Arc<dyn WebSocketProvider>> {
        self.web_socket.clone()
    }

    /// Adds the [`BroadcastHub`] that carries `BroadcastChannel` messages
    /// between agents. Absent, a channel reaches only its own agent — which is
    /// all there is to reach without workers.
    #[must_use]
    pub fn with_broadcast(mut self, broadcast: Arc<dyn BroadcastHub>) -> Self {
        self.broadcast = Some(broadcast);
        self
    }

    /// Adds the [`PortHub`] that owns `MessagePort` queues, which is what lets
    /// a port be transferred to another agent. Absent, ports stay agent-local
    /// and transferring one is a `DataCloneError` — there being nowhere to
    /// transfer it to without workers.
    #[must_use]
    pub fn with_ports(mut self, ports: Arc<dyn PortHub>) -> Self {
        self.ports = Some(ports);
        self
    }

    fn ports(&self) -> Option<Arc<dyn PortHub>> {
        self.ports.clone()
    }

    fn broadcast(&self) -> Option<Arc<dyn BroadcastHub>> {
        self.broadcast.clone()
    }

    fn workers(&self) -> Option<Arc<dyn WorkerHost>> {
        self.workers.clone()
    }

    fn worker_scope(&self) -> Option<Arc<dyn WorkerScope>> {
        self.worker_scope.clone()
    }

    fn commands(&self) -> Option<Arc<dyn CommandProvider>> {
        self.commands.clone()
    }
}

/// The entry module's specifier, shared with the op that reports it.
pub(crate) type EntrySlot = Rc<RefCell<Option<String>>>;

/// The embeddable runtime: an engine plus the driven loop's scheduling state.
pub struct Runtime {
    engine: Box<dyn Engine>,
    /// Watching capability checks, when something asked to (D59). Held here as
    /// well as in the engine because one gated decision is made *above* the op
    /// boundary — whether a module may be loaded — and a trace that missed it
    /// would print a deploy line without `--allow-imports`, which is a line that
    /// does not run.
    observer: Option<es_runtime_engine::SharedObserver>,
    timers: TimerQueue,
    /// The runtime's current notion of time (embedder ms), last set by
    /// [`tick`](Self::tick). Timers created by [`eval`](Self::eval) between ticks
    /// are anchored here, so a `setTimeout(cb, d)` measures `d` from "now"
    /// rather than from whenever the next tick happens to arrive.
    now_ms: u64,
    /// Set while a module graph evaluation kicked off by
    /// [`load_module_source`](Self::load_module_source) has not yet settled, so
    /// [`tick`](Self::tick) keeps reporting pending work (top-level await) until
    /// the evaluation promise resolves or rejects.
    module_eval_pending: bool,
    /// The realm's module map: canonical specifier → compiled [`ModuleId`].
    /// Shared by the initial graph load and dynamic `import()` so a module
    /// imported both statically and dynamically is the **same instance**.
    module_map: HashMap<String, ModuleId>,
    /// The entry module's specifier, remembered so a relative `new Worker(url)`
    /// has the base URL a browser would take from the document. `import.meta.url`
    /// is the precise form (`new URL("./w.js", import.meta.url)`), and the one
    /// to reach for from a non-entry module.
    entry_specifier: EntrySlot,
    /// The loader used for static graph loading and dynamic `import()`. Stored
    /// (not passed per-call) so dynamic imports raised mid-execution can reach
    /// it — and shared with the `module_resolve_sync` op, which serves
    /// `import.meta.resolve` and is registered long before a loader exists (D41).
    module_loader: Rc<RefCell<Option<Arc<dyn ModuleLoader>>>>,
    /// How many `Worker` objects are currently *referenced* — Node's handle
    /// ref-counting, and the reason `worker.unref()` can mean anything.
    ///
    /// A live worker is a reason for the loop to keep running, and the pending
    /// `worker_recv` used to be what said so. That could not be undone: an idle
    /// worker's receive is already in flight, so an `unref()` would not take
    /// effect until a message arrived — and for an idle pooled worker, none
    /// ever does. The receive no longer keeps the loop alive on its own, and
    /// this counter does, so the answer changes the moment it is asked to.
    ///
    /// [`Cell`], not the engine: this is the runtime's own notion of pending
    /// work, and the ops that move it are registered here.
    handle_refs: Rc<Cell<u32>>,
    /// Whether this agent is a worker — it was given a [`WorkerScope`] to reach
    /// its parent through.
    ///
    /// Only ever consulted to word a refusal. A worker is granted its
    /// capabilities at the spawn rather than on the command line, so
    /// "add `--allow-imports`" is the wrong advice inside one, and the right
    /// advice is unreachable from where the failure happens.
    is_worker: bool,
    /// A read-only mirror of the engine's capability set, shared with the ops
    /// that report on it (`runtime:process` `permissions`, D38).
    ///
    /// The engine's own set stays the security boundary — this is never consulted
    /// to *authorize* anything, only to answer "what am I allowed to do?".
    /// [`set_capabilities`](Self::set_capabilities) writes both.
    capabilities: Rc<Cell<CapabilitySet>>,
    /// `runtime:` modules the **embedder** added, alongside the baked ones —
    /// see [`register_module`](Self::register_module).
    ///
    /// Empty in `esrun`, and that is the point: the namespace a program can
    /// import is the same everywhere the runtime is embedded, plus whatever the
    /// binary in front of it deliberately put there. `esdev` puts
    /// `runtime:build` and `runtime:watch` here, which is how a development
    /// module exists without existing in production.
    host_modules: HashMap<String, Rc<str>>,
}

impl Runtime {
    /// Builds a runtime over the given engine, installing the host ops and the
    /// JS prelude that together provide the WinterTC web-API surface.
    ///
    /// Taking a `Box<dyn Engine>` keeps the boundary clean: the caller chooses
    /// the engine (today [`V8Engine`]), the runtime drives it through the trait.
    /// `providers` supply the host capabilities the prelude needs (a [`Clock`]
    /// and a [`Console`] sink in Phase 4).
    ///
    /// Fails if op registration or prelude evaluation fails — both indicate a
    /// build-time bug, surfaced loudly rather than left half-initialized.
    pub fn new(engine: Box<dyn Engine>, providers: HostProviders) -> Result<Self> {
        let mut runtime = Runtime {
            engine,
            observer: None,
            timers: TimerQueue::default(),
            now_ms: 0,
            module_eval_pending: false,
            module_map: HashMap::new(),
            entry_specifier: Rc::new(RefCell::new(None)),
            module_loader: Rc::new(RefCell::new(None)),
            handle_refs: Rc::new(Cell::new(0)),
            is_worker: providers.worker_scope.is_some(),
            capabilities: Rc::new(Cell::new(CapabilitySet::none())),
            host_modules: HashMap::new(),
        };
        // Register the world-touching ops, then evaluate the prelude that builds
        // the pure-JS APIs on top of them (DECISIONS.md D8).
        let capabilities = runtime.capabilities.clone();
        let loader_slot = runtime.module_loader.clone();
        let entry_slot = runtime.entry_specifier.clone();
        let handle_refs = runtime.handle_refs.clone();
        builtins::install(
            runtime.engine.as_mut(),
            &providers,
            capabilities,
            loader_slot,
            entry_slot,
            handle_refs,
        )?;
        runtime.engine.eval(&prelude::source())?;
        runtime.engine.eval(&prelude::post_snapshot_source())?;
        Ok(runtime)
    }

    /// Builds a startup-snapshot blob with the host ops' JS shells and the whole
    /// prelude baked in (DECISIONS.md D8). Restoring a runtime from it via
    /// [`with_snapshot`](Self::with_snapshot) skips both compiling *and* running
    /// the prelude, which is the bulk of [`new`](Self::new)'s cost.
    ///
    /// `providers` are consumed only to satisfy op registration while building;
    /// the Rust handler closures are **not** serialized, so the choice of
    /// providers here does not affect the blob — it captures only the op
    /// names/order and the prelude's global state. Build once at startup, before
    /// any engine exists (V8 forbids concurrent snapshot creation).
    pub fn build_snapshot(providers: &HostProviders) -> Result<Vec<u8>> {
        Ok(V8Engine::build_snapshot(
            es_runtime_common::Limits::default(),
            |engine| {
                // `builtins::install` yields the runtime `Error`; it only ever
                // produces the engine variant here, so re-surface that and treat
                // any other (impossible) variant as an internal error.
                // Handler closures are not serialized, so the capability view
                // captured here never outlives the build — only the op table's
                // names and order end up in the blob.
                let capabilities = Rc::new(Cell::new(CapabilitySet::none()));
                let loader_slot = Rc::new(RefCell::new(None));
                let entry_slot = Rc::new(RefCell::new(None));
                let handle_refs = Rc::new(Cell::new(0));
                builtins::install(
                    engine,
                    providers,
                    capabilities,
                    loader_slot,
                    entry_slot,
                    handle_refs,
                )
                .map_err(|e| match e {
                    Error::Engine(e) => e,
                    other => es_runtime_engine::Error::Internal(other.to_string()),
                })?;
                engine.eval(&prelude::source())?;
                Ok(())
            },
        )?)
    }

    /// Restores a runtime from a [`build_snapshot`](Self::build_snapshot) blob.
    ///
    /// The prelude and `__ops.<name>` shells come from the snapshot, so this only
    /// rebinds the Rust op handlers — in the **same order** `build_snapshot` used,
    /// which [`builtins::install`] guarantees — and does not re-evaluate the
    /// prelude. Equivalent in behaviour to [`new`](Self::new), far cheaper.
    pub fn with_snapshot(snapshot: Vec<u8>, providers: HostProviders) -> Result<Self> {
        Self::with_snapshot_and_limits(snapshot, es_runtime_common::Limits::default(), providers)
    }

    /// [`with_snapshot`](Self::with_snapshot) with the isolate's [`Limits`]
    /// chosen by the caller.
    ///
    /// Two reasons this exists. A worker agent needs
    /// [`Limits::can_block`](es_runtime_common::Limits::can_block) set — it owns
    /// its thread, so `Atomics.wait` is legal there and a `TypeError` on the
    /// agent that drives the loop. And a host running many agents wants to size
    /// each one's heap ceiling, rather than handing every worker the 256 MiB
    /// single-isolate default.
    ///
    /// The blob is `impl Into<Cow<'static, [u8]>>`, so a `&'static [u8]` — the
    /// `include_bytes!` snapshot a binary carries — is shared across every
    /// agent rather than copied per agent.
    pub fn with_snapshot_and_limits(
        snapshot: impl Into<std::borrow::Cow<'static, [u8]>>,
        limits: es_runtime_common::Limits,
        providers: HostProviders,
    ) -> Result<Self> {
        let engine = V8Engine::with_snapshot_baked_ops(limits, snapshot)?;
        let mut runtime = Runtime {
            engine: Box::new(engine),
            observer: None,
            timers: TimerQueue::default(),
            now_ms: 0,
            module_eval_pending: false,
            module_map: HashMap::new(),
            entry_specifier: Rc::new(RefCell::new(None)),
            module_loader: Rc::new(RefCell::new(None)),
            handle_refs: Rc::new(Cell::new(0)),
            is_worker: providers.worker_scope.is_some(),
            capabilities: Rc::new(Cell::new(CapabilitySet::none())),
            host_modules: HashMap::new(),
        };
        // Rebind handlers only; the engine skips the (baked) JS shells and the
        // prelude is already present in the restored context.
        let capabilities = runtime.capabilities.clone();
        let loader_slot = runtime.module_loader.clone();
        let entry_slot = runtime.entry_specifier.clone();
        let handle_refs = runtime.handle_refs.clone();
        builtins::install(
            runtime.engine.as_mut(),
            &providers,
            capabilities,
            loader_slot,
            entry_slot,
            handle_refs,
        )?;
        // Every op the snapshot has a shell for is now bound; an op registered
        // after this point (an embedder's — `esdev`'s bundler and watcher) is
        // one the snapshot never saw, and gets its shell installed for real.
        runtime.engine.finish_baked_ops();
        // Except the fragments that cannot be baked — `WebAssembly` exists only
        // now, in a real isolate, so its wrappers are installed per-launch.
        runtime.engine.eval(&prelude::post_snapshot_source())?;
        Ok(runtime)
    }

    /// The isolate ceilings this runtime was built with — see
    /// [`Limits`](es_runtime_common::Limits).
    #[must_use]
    pub fn limits(&self) -> es_runtime_common::Limits {
        self.engine.limits()
    }

    /// Whether this agent was terminated by its heap guard rather than by a
    /// `process.exit()`, a watchdog or a `terminate()`.
    ///
    /// The four are indistinguishable from outside — execution simply stops —
    /// and only this one means "it asked for more memory than it was allowed",
    /// which is the difference between a job worth retrying and one that will
    /// fail the same way every time.
    #[must_use]
    pub fn heap_limit_exceeded(&self) -> bool {
        self.engine.heap_limit_exceeded()
    }

    /// Registers a host op, callable from JS as `globalThis.__ops.<name>`.
    pub fn register_op(&mut self, op: OpDecl) -> Result<()> {
        self.engine.register_op(op)?;
        Ok(())
    }

    /// Adds a `runtime:` module served by **this embedder**, on the same terms
    /// as the baked ones: no loader, no filesystem, no capability to import it
    /// (D26/D38) — its ops carry the gates, exactly like `runtime:fs`.
    ///
    /// The namespace is otherwise fixed, and deliberately so: a program's
    /// imports must mean the same thing under every embedding of this runtime.
    /// What this seam adds is the case where they *cannot* — a module that only
    /// a development binary can honour, because the machinery behind it is not
    /// in the production one. `esdev` registers `runtime:build` (the bundler)
    /// and `runtime:watch` (file events); `esrun` registers nothing, so an
    /// `import "runtime:build"` there fails at load with "unknown built-in
    /// module" rather than half-working.
    ///
    /// # Errors
    ///
    /// A specifier outside the `runtime:` scheme, or one a baked module already
    /// answers to. Shadowing a built-in is refused rather than resolved in some
    /// order, because the order would be the only thing standing between a
    /// program and a redefined `runtime:fs`.
    pub fn register_module(&mut self, specifier: &str, source: &str) -> Result<()> {
        if !runtime_modules::is_builtin_scheme(specifier) {
            return Err(Error::ModuleLoad(format!(
                "host module {specifier:?} is not in the runtime: namespace"
            )));
        }
        if runtime_modules::source(specifier).is_some() {
            return Err(Error::ModuleLoad(format!(
                "{specifier:?} is a built-in module and cannot be replaced"
            )));
        }
        self.host_modules
            .insert(specifier.to_string(), Rc::from(source));
        Ok(())
    }

    /// Replaces the capability set checked before capability-gated ops dispatch
    /// (DECISIONS.md D7). Deny-by-default until granted.
    ///
    /// Also refreshes the view `runtime:process` `permissions` reports (D38), so
    /// the guest's answer to "what am I allowed to do?" is never stale.
    pub fn set_capabilities(&mut self, capabilities: CapabilitySet) {
        self.engine.set_capabilities(capabilities);
        self.capabilities.set(capabilities);
    }

    /// Watches every capability check this runtime makes, in the engine and at
    /// the module loader (DECISIONS.md D59) — how `esdev --trace-permissions`
    /// learns what a run actually reached for.
    ///
    /// Observation only: an observer cannot grant, refuse or alter anything, so
    /// unlike the inspector there is nothing here to keep out of a production
    /// build. `esrun` simply never calls it.
    pub fn set_capability_observer(&mut self, observer: es_runtime_engine::SharedObserver) {
        self.engine.set_capability_observer(observer.clone());
        self.observer = Some(observer);
    }

    /// Attaches a debugger to this runtime, speaking the Chrome DevTools
    /// Protocol over `transport` (DECISIONS.md D59).
    ///
    /// Forwarded to [`Engine::attach_inspector`], which is where the whole story
    /// is: an inspector is a bypass of the capability model, so it exists only in
    /// a binary built with `ES_RUNTIME_INSPECTOR=1` and otherwise reports that it
    /// does not. With [`InspectorOptions::wait_for_debugger`] set this blocks
    /// until a client attaches and releases the program.
    ///
    /// Call it before loading the entry module: V8 announces a script to the
    /// debugger as it is compiled, so a session opened afterwards sees none of
    /// the program's own sources.
    pub fn attach_inspector(
        &mut self,
        transport: std::rc::Rc<dyn InspectorTransport>,
        options: &InspectorOptions,
    ) -> Result<()> {
        self.engine.attach_inspector(transport, options)?;
        Ok(())
    }

    /// Returns a thread-safe handle for interrupting this runtime's execution —
    /// e.g. for a watchdog thread that bounds execution time (SPEC §4). Calling
    /// [`InterruptHandle::terminate`] stops the running script; the in-flight
    /// [`eval`](Self::eval)/[`tick`](Self::tick) then surfaces a termination
    /// rather than hanging.
    pub fn interrupt_handle(&self) -> InterruptHandle {
        self.engine.interrupt_handle()
    }

    /// Compiles and runs `source`, returning the marshaled result. Pending work
    /// it schedules (async ops, timers) is advanced by subsequent [`tick`](Self::tick)s.
    pub fn eval(&mut self, source: &str) -> Result<Value> {
        let value = self.engine.eval(source)?;
        // Anchor any timers the script created at the current time, so their
        // delays are measured from now, not from the next tick's clock.
        self.drain_new_timers(self.now_ms);
        Ok(value)
    }

    /// Loads, instantiates, and begins evaluating an ES module graph rooted at
    /// `entry_specifier` with the already-read `entry_source` (SPEC §2.1).
    ///
    /// V8 resolves a module graph synchronously, so the whole graph is fetched
    /// and compiled *before* instantiation: this walks the entry's imports,
    /// [`resolve`](ModuleLoader::resolve)s each specifier and
    /// [`load`](ModuleLoader::load)s its source through `loader`, compiling each
    /// distinct module once (so diamonds and import cycles load a module a single
    /// time), then instantiates and kicks off evaluation. Evaluation (which may
    /// top-level-await) is then advanced by [`tick`](Self::tick); poll
    /// [`module_eval_state`](Self::module_eval_state) for the outcome once
    /// [`has_pending_work`](Self::has_pending_work) reports quiescence.
    ///
    /// The entry source is supplied by the caller (so a CLI can run a file it
    /// already read, or an inline snippet), and loading it needs no capability;
    /// following any `import`, however, consults `loader` and so requires
    /// [`Capability::FileSystem`] for a file-backed loader. A self-contained
    /// module (no imports) therefore runs even when that capability is denied.
    ///
    /// `loader` is stored on the runtime so that dynamic `import()` raised during
    /// evaluation can reach it (drive it with
    /// [`process_dynamic_imports`](Self::process_dynamic_imports)).
    pub async fn load_module_source(
        &mut self,
        entry_specifier: &str,
        entry_source: &str,
        loader: Arc<dyn ModuleLoader>,
    ) -> Result<()> {
        let entry = self
            .instantiate_module_source(entry_specifier, entry_source, loader)
            .await?;
        self.begin_evaluation(entry)
    }

    /// The first half of [`load_module_source`](Self::load_module_source):
    /// compile the entry, load and compile its whole static graph, and
    /// instantiate it. Returns the entry's id, for
    /// [`begin_evaluation`](Self::begin_evaluation).
    ///
    /// **No guest code runs here** — instantiation only links bindings — which
    /// is the reason the two halves are separable at all. An embedder that
    /// loads a program under one capability set and runs it under a narrower
    /// one (a worker agent loading its graph with the authority of the agent
    /// that named it, then evaluating with its own) can only do that safely if
    /// nothing the guest wrote executes in between.
    pub async fn instantiate_module_source(
        &mut self,
        entry_specifier: &str,
        entry_source: &str,
        loader: Arc<dyn ModuleLoader>,
    ) -> Result<ModuleId> {
        *self.module_loader.borrow_mut() = Some(loader);
        if self.entry_specifier.borrow().is_none() {
            *self.entry_specifier.borrow_mut() = Some(entry_specifier.to_string());
        }

        let entry_id = self.engine.compile_module(entry_specifier, entry_source)?;
        self.module_map
            .insert(entry_specifier.to_string(), entry_id);
        let resolved = self
            .build_graph(entry_id, entry_specifier.to_string())
            .await?;

        self.engine.instantiate_module(entry_id, &resolved)?;
        Ok(entry_id)
    }

    /// The second half: start evaluating an instantiated entry module. Its top
    /// level runs now, so whatever capability set is current is the one the
    /// guest gets. Advance it with [`tick`](Self::tick) and read the outcome
    /// from [`module_eval_state`](Self::module_eval_state).
    pub fn begin_evaluation(&mut self, entry: ModuleId) -> Result<()> {
        self.engine.evaluate_module(entry)?;
        self.module_eval_pending = true;
        // Anchor any timers the synchronous portion of evaluation created.
        self.drain_new_timers(self.now_ms);
        Ok(())
    }

    /// Transpiles raw JSON text into an ES module that exports the parsed value.
    /// The entire document is escaped into a single JS string literal (a JSON
    /// string is a valid JS string literal), so `JSON.parse` sees exactly the
    /// file bytes and the JSON can never break out into executable code — a JSON
    /// module cannot run.
    fn json_module_source(raw: &str) -> String {
        // `to_string` of a `&str` is infallible; the fallback only keeps us
        // panic-free and is never reached in practice.
        let escaped = serde_json::to_string(raw).unwrap_or_else(|_| "null".to_string());
        format!("export default JSON.parse({escaped});")
    }

    /// Turns what the loader returned into the source the engine compiles.
    ///
    /// WebAssembly is compiled here and represented by a generated wrapper (see
    /// [`wasm_module_source`](Self::wasm_module_source)); JSON is wrapped when
    /// the import carried `with { type: "json" }` — an attribute, not an
    /// extension. Plain text passes through.
    fn module_source(&mut self, loaded: ModuleSource, is_json: bool) -> Result<String> {
        Ok(match loaded {
            ModuleSource::Wasm(bytes) => {
                let info = self.engine.compile_wasm(&bytes)?;
                Self::wasm_module_source(&info)
            }
            ModuleSource::Text(text) if is_json => Self::json_module_source(&text),
            ModuleSource::Text(text) => text,
        })
    }

    /// Synthesizes the JS module that stands in for a `.wasm` file in the graph
    /// (the WebAssembly ES-module integration).
    ///
    /// V8's synthetic modules cannot declare imports of their own, so rather than
    /// building one, the wasm module is represented by a *generated* ES module.
    /// That buys the whole existing pipeline for free — its wasm imports become
    /// ordinary `import` statements that resolve, dedupe, and cycle-check exactly
    /// like any other edge, and its exports become real named exports.
    ///
    /// The bytes never enter this source: they stay compiled in the engine, and
    /// the wrapper reclaims the module by handle.
    ///
    /// Per the integration, the *module* half of each wasm import is an ordinary
    /// module specifier, so `(import "./env.js" "log")` imports that file's
    /// namespace and reads `log` off it. Each distinct specifier is imported once.
    fn wasm_module_source(info: &es_runtime_engine::WasmModuleInfo) -> String {
        let js_string = |s: &str| serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string());

        // Distinct import specifiers, in first-seen order, each bound to a
        // namespace local.
        let mut specifiers: Vec<&str> = Vec::new();
        for (module, _) in &info.imports {
            if !specifiers.contains(&module.as_str()) {
                specifiers.push(module);
            }
        }

        let mut out = String::new();
        for (i, spec) in specifiers.iter().enumerate() {
            out.push_str(&format!("import * as $ns{i} from {};\n", js_string(spec)));
        }

        out.push_str(&format!("const $mod = __wasm_module({});\n", info.handle));
        out.push_str("const $imports = { __proto__: null };\n");
        for (i, spec) in specifiers.iter().enumerate() {
            out.push_str(&format!("$imports[{}] = $ns{i};\n", js_string(spec)));
        }
        out.push_str("const $exports = new WebAssembly.Instance($mod, $imports).exports;\n");

        // Wasm export names are arbitrary strings, so bind each to a generated
        // local and re-export it under its real name via a string alias — which
        // covers names that are not identifiers (`foo-bar`, `0`, `default`).
        for (i, name) in info.exports.iter().enumerate() {
            out.push_str(&format!("const $x{i} = $exports[{}];\n", js_string(name)));
        }
        if !info.exports.is_empty() {
            let aliases: Vec<String> = info
                .exports
                .iter()
                .enumerate()
                .map(|(i, name)| format!("$x{i} as {}", js_string(name)))
                .collect();
            out.push_str(&format!("export {{ {} }};\n", aliases.join(", ")));
        }
        out
    }

    /// Walks the import graph reachable from `root_id`, compiling each distinct
    /// canonical specifier once (deduped via the realm [`module_map`], so
    /// diamonds and cycles load a module a single time and shared modules are one
    /// instance), and returns the `(referrer, specifier) → target` map covering
    /// the whole subgraph for [`instantiate_module`](Engine::instantiate_module).
    async fn build_graph(
        &mut self,
        root_id: ModuleId,
        root_spec: String,
    ) -> Result<HashMap<(ModuleId, String), ModuleId>> {
        let mut resolved: HashMap<(ModuleId, String), ModuleId> = HashMap::new();
        let mut seen: std::collections::HashSet<ModuleId> = std::collections::HashSet::new();
        let mut frontier = vec![(root_id, root_spec)];

        while let Some((referrer_id, referrer_spec)) = frontier.pop() {
            // Record each module's edges once per build (also breaks cycles).
            if !seen.insert(referrer_id) {
                continue;
            }
            let requests = self.engine.module_requests(referrer_id)?;
            for req in requests {
                let raw = req.specifier;
                let is_json = req.import_type.as_deref() == Some("json");
                let (target_id, newly_compiled) = if runtime_modules::is_builtin_scheme(&raw) {
                    // `runtime:` built-ins are served by the runtime itself — no
                    // loader, no FileSystem capability (their ops are gated).
                    self.resolve_builtin(&raw)?
                } else {
                    // A file / node_modules import: the capability-gated,
                    // loader-touching path.
                    self.require_module_capability(&raw)?;
                    let loader = self.loader()?;
                    let canonical = loader
                        .resolve(&raw, &referrer_spec)
                        .await
                        .map_err(|e| Error::ModuleLoad(e.to_string()))?;
                    match self.module_map.get(&canonical) {
                        Some(&id) => (id, None),
                        None => {
                            let loaded = loader
                                .load(&canonical)
                                .await
                                .map_err(|e| Error::ModuleLoad(e.to_string()))?;
                            let source = self.module_source(loaded, is_json)?;
                            let id = self.engine.compile_module(&canonical, &source)?;
                            self.module_map.insert(canonical.clone(), id);
                            (id, Some(canonical))
                        }
                    }
                };
                if let Some(canonical) = newly_compiled {
                    frontier.push((target_id, canonical));
                }
                resolved.insert((referrer_id, raw), target_id);
            }
        }
        Ok(resolved)
    }

    /// Resolves a `runtime:` built-in to a compiled [`ModuleId`], compiling its
    /// baked source on first use (deduped via the realm module map). Returns the
    /// id and, when newly compiled, its canonical specifier to walk.
    fn resolve_builtin(&mut self, specifier: &str) -> Result<(ModuleId, Option<String>)> {
        if let Some(&id) = self.module_map.get(specifier) {
            return Ok((id, None));
        }
        // A baked module first, then whatever the embedder added
        // ([`register_module`](Self::register_module) refuses to shadow one, so
        // the order here is a formality rather than a policy).
        let source: Rc<str> = match runtime_modules::source(specifier) {
            Some(baked) => Rc::from(baked),
            None => self.host_modules.get(specifier).cloned().ok_or_else(|| {
                Error::ModuleLoad(format!("unknown built-in module {specifier:?}"))
            })?,
        };
        let id = self.engine.compile_module(specifier, &source)?;
        self.module_map.insert(specifier.to_string(), id);
        Ok((id, Some(specifier.to_string())))
    }

    /// Loads, instantiates, and begins evaluating dynamic `import()` requests
    /// raised since the last call, settling each request's promise with the
    /// module namespace (or rejecting it). Async because resolution/loading is
    /// I/O; the embedder/driver calls this each loop iteration alongside
    /// [`tick`](Self::tick). A no-op when nothing dynamic is pending.
    ///
    /// Returns whether it linked or rejected anything. A caller that parks
    /// between iterations **must not park when this is `true`**: the request's
    /// promise is settled by the *next* [`tick`](Self::tick), so parking first
    /// charges the whole park to the `import()`. Parking on an unrelated 3-second
    /// timer made a dynamic import take three seconds.
    pub async fn process_dynamic_imports(&mut self) -> Result<bool> {
        let mut linked = false;
        // Re-drain: linking a module evaluates it, which can synchronously raise
        // further `import()` calls; loop until none remain.
        loop {
            let pending = self.engine.take_pending_dynamic_imports();
            if pending.is_empty() {
                return Ok(linked);
            }
            linked = true;
            for (reqid, specifier, referrer, import_type) in pending {
                match self
                    .load_for_dynamic_import(&specifier, &referrer, import_type.as_deref())
                    .await
                {
                    Ok(id) => self.engine.link_dynamic_import(reqid, id)?,
                    Err(err) => {
                        // The failure itself, so the rejection carries its class
                        // and its code: a module with a syntax error rejects
                        // with a `SyntaxError`, and an import refused for want
                        // of a capability with a `NotAllowedError`.
                        self.engine.reject_dynamic_import(reqid, &err)?;
                    }
                }
            }
            self.drain_new_timers(self.now_ms);
        }
    }

    /// Resolves + loads + instantiates the graph for one dynamic `import()`,
    /// reusing the realm module map (so a dynamically imported module that was
    /// also imported statically is the same instance). Returns its [`ModuleId`].
    async fn load_for_dynamic_import(
        &mut self,
        specifier: &str,
        referrer: &str,
        import_type: Option<&str>,
    ) -> Result<ModuleId> {
        // A dynamic import() of a `runtime:` built-in (e.g. `import("runtime:process")`).
        if runtime_modules::is_builtin_scheme(specifier) {
            let (id, _) = self.resolve_builtin(specifier)?;
            let resolved = self.build_graph(id, specifier.to_string()).await?;
            self.engine.instantiate_module(id, &resolved)?;
            return Ok(id);
        }
        self.require_module_capability(specifier)?;
        let loader = self.loader()?;
        let canonical = loader
            .resolve(specifier, referrer)
            .await
            .map_err(|e| Error::ModuleLoad(e.to_string()))?;
        let id = match self.module_map.get(&canonical) {
            Some(&id) => id,
            None => {
                let loaded = loader
                    .load(&canonical)
                    .await
                    .map_err(|e| Error::ModuleLoad(e.to_string()))?;
                let source = self.module_source(loaded, import_type == Some("json"))?;
                let id = self.engine.compile_module(&canonical, &source)?;
                self.module_map.insert(canonical.clone(), id);
                id
            }
        };
        let resolved = self.build_graph(id, canonical).await?;
        // Idempotent if the module is already instantiated (shared instance).
        self.engine.instantiate_module(id, &resolved)?;
        Ok(id)
    }

    /// The configured module loader, or an error if none was set (no loader =
    /// imports denied, like a denied capability).
    fn loader(&self) -> Result<Arc<dyn ModuleLoader>> {
        self.module_loader.borrow().clone().ok_or_else(|| {
            Error::ModuleLoad("no module loader configured (imports are not permitted)".into())
        })
    }

    /// The outcome of the module evaluation started by
    /// [`load_module_source`](Self::load_module_source): pending, completed, or
    /// failed (with the stringified reason). [`ModuleEvalState::Pending`] before
    /// any module is loaded.
    pub fn module_eval_state(&mut self) -> ModuleEvalState {
        self.engine.module_eval_state()
    }

    /// Errors unless the `FileSystem` capability needed to load modules is
    /// granted, saying which import failed and where the grant is made.
    ///
    /// The bare denial — `capability denied: FileSystem (permission "imports")`
    /// — names an internal capability and a permission the author may never
    /// have mentioned, and says nothing about where to fix it. That matters
    /// most inside a worker: a worker's grants are set at the spawn, in the
    /// parent, so the one place `--allow-imports` cannot help is the one place
    /// this is most likely to be hit — a worker's static graph is resolved by
    /// its parent up front, so `import` works and `import()` does not.
    fn require_module_capability(&self, specifier: &str) -> Result<()> {
        let granted = self.engine.capabilities().contains(Capability::FileSystem);
        if let Some(observer) = &self.observer {
            // Named `import` rather than an op name: this check has no op behind
            // it, and `import` is the word the developer wrote.
            observer.observed("import", Capability::FileSystem, granted);
        }
        if granted {
            return Ok(());
        }
        let remedy = if self.is_worker {
            "this worker was not granted the \"imports\" permission — grant it at \
             the spawn, new Worker(url, { permissions: [\"imports\"] })"
        } else {
            "the \"imports\" permission is not granted — add --allow-imports"
        };
        Err(Error::ImportDenied(format!(
            "cannot import {specifier:?}: {remedy}"
        )))
    }

    /// Injects the [`Waker`](std::task::Waker) the engine uses to poll pending
    /// async ops. A driver wires this to its own wakeup primitive so a ready
    /// op-future wakes the loop immediately instead of waiting for the next
    /// blind re-poll. Forwarded to [`Engine::set_async_waker`].
    pub fn set_async_waker(&mut self, waker: std::task::Waker) {
        self.engine.set_async_waker(waker);
    }

    /// Advances the loop by one step (ARCHITECTURE.md §5), in order:
    /// due **timers** → ready **async ops** → **microtask checkpoint** →
    /// **unhandled-rejection** collection. `now_ms` is the embedder's current
    /// time; the runtime holds no clock of its own.
    pub fn tick(&mut self, now_ms: u64) -> TickStatus {
        self.now_ms = now_ms;
        // 0. Anything an attached debugger asked for since the last tick — set a
        // breakpoint, read a source, resume. Before the guest's own work, so a
        // breakpoint set a moment ago is in place for the code about to run. A
        // no-op unless a debugger is attached, which is every production run.
        self.engine.poll_inspector();
        // Schedule timers created since the last drain (e.g. during `eval`).
        self.drain_new_timers(now_ms);

        // 1. Fire due timers, re-arming still-active repeating ones.
        let mut timers_fired = 0;
        for due in self.timers.take_due(now_ms) {
            if self.engine.fire_timer(due.id) {
                timers_fired += 1;
                if due.repeat && self.engine.timer_is_active(due.id) {
                    self.timers.schedule(due.id, now_ms, due.interval_ms, true);
                }
            }
        }
        // Timers created by those callbacks fire no earlier than the next tick.
        self.drain_new_timers(now_ms);

        // 2. Settle ready async ops (resolving promises enqueues microtasks).
        let async_ops_settled = self.engine.poll_async_ops();

        // 2a. Drain V8's own foreground task queue. Work V8 runs on its internal
        // threads — async WebAssembly compilation — reports back as a task here,
        // and resolves its promise only when the task runs. Before the checkpoint
        // below, so those reactions run in the same tick.
        self.engine.pump_message_loop();

        // 2b. Settle dynamic import() promises whose module evaluation has
        // completed (resolving with the namespace, or rejecting), so their
        // reactions run in the checkpoint below.
        self.engine.settle_dynamic_imports();

        // 3. Microtask checkpoint (promise reactions, queueMicrotask).
        self.engine.run_microtasks();
        self.drain_new_timers(now_ms);

        // 4. Collect failures the guest did not claim: rejections that stayed
        //    unhandled, and exceptions that escaped a timer callback. Both are
        //    offered to the guest's listeners first (inside the engine, which is
        //    where the values live); what comes back is what nothing claimed.
        let unhandled_rejections = self.engine.take_unhandled_rejections();
        let uncaught_errors = self.engine.take_uncaught_errors();

        // A kicked-off module evaluation stops being pending work once its
        // promise settles (completed or failed); the outcome is read by the
        // embedder via [`module_eval_state`](Self::module_eval_state).
        if self.module_eval_pending && self.engine.module_eval_state() != ModuleEvalState::Pending {
            self.module_eval_pending = false;
        }

        TickStatus {
            timers_fired,
            async_ops_settled,
            unhandled_rejections,
            uncaught_errors,
            has_pending_work: self.has_pending_work(),
            next_timer_deadline_ms: self.timers.next_deadline_ms(),
            v8_background_work: self.engine.has_pending_wasm(),
            // Read after the JS above rather than before: a sync op can request
            // it mid-tick, and `process.exit()` is exactly that.
            terminated: self.engine.interrupt_handle().is_terminating(),
        }
    }

    /// Whether any async op, timer, or unsettled module evaluation is still
    /// outstanding.
    pub fn has_pending_work(&self) -> bool {
        self.engine.has_pending_async_ops()
            || !self.timers.is_empty()
            || self.module_eval_pending
            || self.engine.has_pending_dynamic_imports()
            || self.engine.has_pending_wasm()
            || self.handle_refs.get() > 0
    }

    /// Moves newly created engine timers into the schedule, anchored at `now_ms`,
    /// and drops any the guest has since cleared.
    ///
    /// Both halves run at every point JS could have touched a timer, so the
    /// schedule never outlives a `clearTimeout` — see
    /// [`TimerQueue::prune_cleared`].
    fn drain_new_timers(&mut self, now_ms: u64) {
        for (id, delay_ms, repeat) in self.engine.take_new_timers() {
            self.timers.schedule(id, now_ms, delay_ms, repeat);
        }
        let engine = &self.engine;
        self.timers.prune_cleared(|id| engine.timer_is_active(id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use es_runtime_common::Limits;
    use std::sync::Mutex;

    /// Serializes V8-touching tests in this binary (see the engine crate's note:
    /// V8's snapshot/isolate global state is not safe under the parallel harness).
    fn v8_guard() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Ticks `rt` until `ready` holds, bounded by **wall-clock time** rather
    /// than a tick count, and panics naming `what` if the budget runs out.
    ///
    /// Some work the loop waits on happens off-thread: a WebAssembly compile
    /// runs on V8's background threads and only lands once the platform's
    /// foreground queue is pumped. How many ticks that takes is a property of
    /// how busy the machine is, not of the runtime — so a fixed spin count
    /// passes on an idle box and fails on a loaded CI runner (or a full
    /// `cargo test --workspace`, where the other test binaries run alongside).
    /// Worse, a count-bounded loop *falls through silently*: the test then
    /// asserts against a result that simply had not arrived yet.
    ///
    /// The first `HOT_SPINS` iterations spin, which is what everything settling
    /// inside a single tick needs and keeps `eval_async` cheap enough for the
    /// 20k-iteration soak. Past that the loop sleeps a millisecond per
    /// turn, which also hands the core to the background threads it is waiting
    /// on instead of starving them.
    fn pump_until(rt: &mut Runtime, what: &str, ready: impl FnMut(&mut Runtime) -> bool) {
        /// Generous enough that only a genuine hang trips it.
        const BUDGET: std::time::Duration = std::time::Duration::from_secs(10);
        pump_until_within(rt, BUDGET, what, ready);
    }

    /// [`pump_until`] with an explicit budget, so the timeout path is testable
    /// without a ten-second test.
    fn pump_until_within(
        rt: &mut Runtime,
        budget: std::time::Duration,
        what: &str,
        mut ready: impl FnMut(&mut Runtime) -> bool,
    ) {
        /// Iterations run without sleeping before falling back to 1ms naps.
        const HOT_SPINS: u32 = 256;

        let deadline = std::time::Instant::now() + budget;
        let mut spins = 0u32;
        loop {
            rt.tick(0);
            if ready(rt) {
                return;
            }
            spins += 1;
            if spins > HOT_SPINS {
                assert!(
                    std::time::Instant::now() < deadline,
                    "timed out after {budget:?} waiting for {what}"
                );
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
    }

    /// A capturing console sink for assertions.
    #[derive(Default)]
    struct TestConsole {
        lines: Mutex<Vec<(ConsoleLevel, String)>>,
    }
    impl Console for TestConsole {
        fn write(&self, level: ConsoleLevel, message: &str) {
            self.lines
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push((level, message.to_string()));
        }
    }

    /// A clock returning fixed monotonic/wall readings.
    struct FixedClock {
        monotonic: u64,
        wall: u64,
    }
    impl Clock for FixedClock {
        fn monotonic_ms(&self) -> u64 {
            self.monotonic
        }
        fn wall_ms(&self) -> u64 {
            self.wall
        }
    }

    /// A canned NetTransport for fetch tests (no real network).
    struct MockNet {
        status: u16,
        headers: Vec<(String, String)>,
        chunks: Vec<Vec<u8>>,
        fail: bool,
    }
    impl MockNet {
        fn ok(body: &str) -> Self {
            MockNet {
                status: 200,
                headers: vec![("content-type".into(), "text/plain".into())],
                chunks: vec![body.as_bytes().to_vec()],
                fail: false,
            }
        }
        /// A transport that errors — for runtimes whose tests never fetch.
        fn stub() -> Self {
            MockNet {
                status: 0,
                headers: Vec::new(),
                chunks: Vec::new(),
                fail: true,
            }
        }
    }
    impl es_runtime_providers::NetTransport for MockNet {
        fn fetch(
            &self,
            request: es_runtime_providers::HttpRequest,
        ) -> es_runtime_providers::BoxFuture<
            std::result::Result<
                es_runtime_providers::HttpResponse,
                es_runtime_providers::ProviderError,
            >,
        > {
            if self.fail {
                return Box::pin(async {
                    Err(es_runtime_providers::ProviderError::Other(
                        "no network".into(),
                    ))
                });
            }
            let (status, headers, chunks, url) = (
                self.status,
                self.headers.clone(),
                self.chunks.clone(),
                request.url,
            );
            Box::pin(async move {
                let body: es_runtime_providers::ByteStream =
                    Box::pin(futures_util::stream::iter(chunks.into_iter().map(Ok)));
                Ok(es_runtime_providers::HttpResponse {
                    status,
                    status_text: "OK".into(),
                    url,
                    redirected: false,
                    headers,
                    body,
                    trailers: None,
                })
            })
        }
    }

    /// A test transport that drains the **request** body (buffered or streamed)
    /// and echoes it back as the response body, recording what it received so a
    /// test can assert both the uploaded content and that a streamed body arrived
    /// as a `RequestBody::Stream` (i.e. was not buffered in JS).
    struct EchoNet {
        captured: Arc<std::sync::Mutex<Vec<u8>>>,
        saw_stream: Arc<std::sync::atomic::AtomicBool>,
    }
    impl EchoNet {
        fn new() -> Self {
            EchoNet {
                captured: Arc::new(std::sync::Mutex::new(Vec::new())),
                saw_stream: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            }
        }
    }
    impl es_runtime_providers::NetTransport for EchoNet {
        fn fetch(
            &self,
            request: es_runtime_providers::HttpRequest,
        ) -> es_runtime_providers::BoxFuture<
            std::result::Result<
                es_runtime_providers::HttpResponse,
                es_runtime_providers::ProviderError,
            >,
        > {
            use es_runtime_providers::RequestBody;
            use futures_util::StreamExt;
            let captured = self.captured.clone();
            let saw_stream = self.saw_stream.clone();
            Box::pin(async move {
                let body = match request.body {
                    RequestBody::Empty => Vec::new(),
                    RequestBody::Bytes(b) => b,
                    RequestBody::Stream(mut s) => {
                        saw_stream.store(true, std::sync::atomic::Ordering::SeqCst);
                        let mut buf = Vec::new();
                        while let Some(chunk) = s.next().await {
                            // A guest stream error (forwarded via close(id, err))
                            // surfaces here and aborts the request.
                            buf.extend_from_slice(&chunk?);
                        }
                        buf
                    }
                };
                *captured.lock().unwrap() = body.clone();
                let stream: es_runtime_providers::ByteStream =
                    Box::pin(futures_util::stream::iter(std::iter::once(Ok(body))));
                Ok(es_runtime_providers::HttpResponse {
                    status: 200,
                    status_text: "OK".into(),
                    url: request.url,
                    redirected: false,
                    headers: vec![],
                    body: stream,
                    trailers: None,
                })
            })
        }
    }

    /// A deterministic (non-crypto) entropy source for tests.
    struct TestEntropy {
        state: std::sync::atomic::AtomicU64,
    }
    impl TestEntropy {
        fn new() -> Self {
            TestEntropy {
                state: std::sync::atomic::AtomicU64::new(0x1234_5678_9abc_def0),
            }
        }
    }
    impl Entropy for TestEntropy {
        fn fill(
            &self,
            dest: &mut [u8],
        ) -> std::result::Result<(), es_runtime_providers::ProviderError> {
            use std::sync::atomic::Ordering;
            let mut x = self.state.load(Ordering::SeqCst) | 1;
            for b in dest.iter_mut() {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                *b = (x & 0xff) as u8;
            }
            self.state.store(x, Ordering::SeqCst);
            Ok(())
        }
    }

    fn runtime() -> Runtime {
        runtime_full(
            Arc::new(TestConsole::default()),
            Arc::new(FixedClock {
                monotonic: 0,
                wall: 0,
            }),
            Arc::new(MockNet::stub()),
            Arc::new(TestEntropy::new()),
        )
    }

    fn runtime_with(console: Arc<dyn Console>, clock: Arc<dyn Clock>) -> Runtime {
        runtime_full(
            console,
            clock,
            Arc::new(MockNet::stub()),
            Arc::new(TestEntropy::new()),
        )
    }

    fn runtime_with_net(net: Arc<dyn NetTransport>) -> Runtime {
        runtime_full(
            Arc::new(TestConsole::default()),
            Arc::new(FixedClock {
                monotonic: 0,
                wall: 0,
            }),
            net,
            Arc::new(TestEntropy::new()),
        )
    }

    fn runtime_full(
        console: Arc<dyn Console>,
        clock: Arc<dyn Clock>,
        net: Arc<dyn NetTransport>,
        entropy: Arc<dyn Entropy>,
    ) -> Runtime {
        let engine = V8Engine::new(Limits::default()).expect("engine");
        Runtime::new(
            Box::new(engine),
            HostProviders::new(clock, console, net, entropy)
                .with_ports(Arc::new(TestPortHub::default())),
        )
        .expect("runtime")
    }

    /// A [`Signals`] that delivers only what a test hands it — no OS involved,
    /// so signal dispatch is exercised deterministically.
    #[derive(Clone, Default)]
    struct TestSignals {
        watched: Arc<Mutex<Vec<es_runtime_providers::Signal>>>,
        queued: Arc<Mutex<Vec<es_runtime_providers::Signal>>>,
    }

    impl TestSignals {
        /// Queues `signal` for the next `next()`, as the OS would.
        fn deliver(&self, signal: es_runtime_providers::Signal) {
            self.queued.lock().unwrap().push(signal);
        }
        fn is_watched(&self, signal: es_runtime_providers::Signal) -> bool {
            self.watched.lock().unwrap().contains(&signal)
        }
    }

    impl es_runtime_providers::Signals for TestSignals {
        fn available(&self) -> Vec<es_runtime_providers::Signal> {
            use es_runtime_providers::Signal as S;
            vec![S::Int, S::Term, S::Hup]
        }

        fn watch(
            &self,
            signal: es_runtime_providers::Signal,
        ) -> std::result::Result<(), es_runtime_providers::ProviderError> {
            if !self.available().contains(&signal) {
                return Err(es_runtime_providers::ProviderError::Other(format!(
                    "{} unavailable",
                    signal.name()
                )));
            }
            let mut watched = self.watched.lock().unwrap();
            if !watched.contains(&signal) {
                watched.push(signal);
            }
            Ok(())
        }

        fn unwatch(&self, signal: es_runtime_providers::Signal) {
            self.watched.lock().unwrap().retain(|s| *s != signal);
        }

        fn next(&self) -> es_runtime_providers::BoxFuture<Option<es_runtime_providers::Signal>> {
            // Synchronous by construction: a queued delivery comes straight
            // back, and an empty queue ends the pump rather than parking it, so
            // a test never depends on wall-clock timing.
            let watched = self.watched.lock().unwrap().clone();
            let mut queued = self.queued.lock().unwrap();
            let next = queued
                .iter()
                .position(|s| watched.contains(s))
                .map(|i| queued.remove(i));
            Box::pin(std::future::ready(next))
        }
    }

    /// The minimum [`Process`] view `runtime:process` needs to evaluate at all —
    /// the module snapshots env/args/platform on import, so the signal tests
    /// below cannot load it without one.
    struct StubProcess;
    impl es_runtime_providers::Process for StubProcess {
        fn env(&self) -> Vec<(String, String)> {
            Vec::new()
        }
        fn args(&self) -> Vec<String> {
            Vec::new()
        }
        fn cwd(&self) -> std::result::Result<String, es_runtime_providers::ProviderError> {
            Ok("/".to_string())
        }
        fn platform(&self) -> String {
            "test".to_string()
        }
        fn arch(&self) -> String {
            "test".to_string()
        }
        fn exit(&self, _code: i32) {}
        fn requested_exit_code(&self) -> Option<i32> {
            None
        }
    }

    fn signal_runtime(signals: Arc<TestSignals>) -> Runtime {
        let engine = V8Engine::new(Limits::default()).expect("engine");
        let providers = HostProviders::new(
            Arc::new(FixedClock {
                monotonic: 0,
                wall: 0,
            }),
            Arc::new(TestConsole::default()),
            Arc::new(MockNet::stub()),
            Arc::new(TestEntropy::new()),
        )
        .with_signals(signals)
        .with_process(Arc::new(StubProcess));
        Runtime::new(Box::new(engine), providers).expect("runtime")
    }

    #[test]
    fn signal_ops_require_the_signals_capability() {
        let _g = v8_guard();
        let mut rt = signal_runtime(Arc::new(TestSignals::default()));
        // Everything except the capability under test, so a denial can only be
        // Signals — not some other gate the op happens to trip first.
        let mut caps = CapabilitySet::all();
        caps.revoke(Capability::Signals);
        rt.set_capabilities(caps);
        let out = eval_async(
            &mut rt,
            "try { __ops.signal_watch('SIGTERM'); return 'no throw'; } catch (e) { return e.code; }",
        );
        assert_eq!(out, Value::String("ERR_CAPABILITY_DENIED".into()));
    }

    #[test]
    fn env_capability_alone_does_not_grant_signals() {
        // The whole reason Signals is its own capability: reading process state
        // must not carry the privilege to suppress termination.
        let _g = v8_guard();
        let mut rt = signal_runtime(Arc::new(TestSignals::default()));
        rt.set_capabilities(CapabilitySet::none().with(Capability::Env));
        let out = eval_async(
            &mut rt,
            "try { __ops.signal_watch('SIGTERM'); return 'no throw'; } catch (e) { return e.code; }",
        );
        assert_eq!(out, Value::String("ERR_CAPABILITY_DENIED".into()));
    }

    /// `runtime:process` is a module, so these go through the module path (a
    /// bare `eval` cannot import) and read their answer back off `globalThis`.
    fn run_signal_module(signals: Arc<TestSignals>, source: &str) -> (Runtime, ModuleEvalState) {
        let mut rt = signal_runtime(signals);
        let state = run_module(&mut rt, source, MapLoader::new(&[]));
        (rt, state)
    }

    #[test]
    fn a_watched_signal_reaches_its_handler() {
        let _g = v8_guard();
        let signals = Arc::new(TestSignals::default());
        signals.deliver(es_runtime_providers::Signal::Term);
        let (mut rt, state) = run_signal_module(
            signals,
            "import { onSignal } from 'runtime:process'; \
             globalThis.result = 'none'; \
             onSignal('SIGTERM', (name) => { globalThis.result = name; });",
        );
        assert_eq!(state, ModuleEvalState::Completed);
        assert_eq!(
            rt.eval("globalThis.result").unwrap(),
            Value::String("SIGTERM".into())
        );
    }

    #[test]
    fn a_signal_that_is_not_watched_is_never_dispatched() {
        let _g = v8_guard();
        let signals = Arc::new(TestSignals::default());
        signals.deliver(es_runtime_providers::Signal::Int); // nobody asked for it
        let (mut rt, state) = run_signal_module(
            signals,
            "import { onSignal } from 'runtime:process'; \
             globalThis.result = 'none'; \
             onSignal('SIGTERM', (name) => { globalThis.result = name; });",
        );
        assert_eq!(state, ModuleEvalState::Completed);
        assert_eq!(
            rt.eval("globalThis.result").unwrap(),
            Value::String("none".into())
        );
    }

    #[test]
    fn the_watch_lasts_until_the_last_handler_is_removed() {
        let _g = v8_guard();
        let signals = Arc::new(TestSignals::default());
        let (_rt, state) = run_signal_module(
            signals.clone(),
            "import { onSignal, offSignal } from 'runtime:process'; \
             const a = () => {}; const b = () => {}; \
             onSignal('SIGHUP', a); onSignal('SIGHUP', b); \
             offSignal('SIGHUP', a); \
             globalThis.stillWatched = true; \
             offSignal('SIGHUP', b);",
        );
        assert_eq!(state, ModuleEvalState::Completed);
        assert!(
            !signals.is_watched(es_runtime_providers::Signal::Hup),
            "the last handler was removed, so the watch must be dropped"
        );
    }

    #[test]
    fn one_handler_of_several_keeps_the_watch() {
        let _g = v8_guard();
        let signals = Arc::new(TestSignals::default());
        let (_rt, state) = run_signal_module(
            signals.clone(),
            "import { onSignal, offSignal } from 'runtime:process'; \
             const a = () => {}; const b = () => {}; \
             onSignal('SIGHUP', a); onSignal('SIGHUP', b); \
             offSignal('SIGHUP', a);",
        );
        assert_eq!(state, ModuleEvalState::Completed);
        assert!(
            signals.is_watched(es_runtime_providers::Signal::Hup),
            "one handler remains, so the watch must stay"
        );
    }

    #[test]
    fn a_signal_the_platform_lacks_throws_rather_than_registering() {
        // A handler that could never fire is worse than a clear failure.
        let _g = v8_guard();
        let (mut rt, state) = run_signal_module(
            Arc::new(TestSignals::default()),
            "import { onSignal } from 'runtime:process'; \
             try { onSignal('SIGUSR1', () => {}); globalThis.result = 'no throw'; } \
             catch (e) { globalThis.result = 'threw'; }",
        );
        assert_eq!(state, ModuleEvalState::Completed);
        assert_eq!(
            rt.eval("globalThis.result").unwrap(),
            Value::String("threw".into())
        );
    }

    #[test]
    fn an_unknown_signal_name_is_a_type_error() {
        let _g = v8_guard();
        let (mut rt, state) = run_signal_module(
            Arc::new(TestSignals::default()),
            "import { onSignal } from 'runtime:process'; \
             try { onSignal('SIGNOPE', () => {}); globalThis.result = 'no throw'; } \
             catch (e) { globalThis.result = e.constructor.name; }",
        );
        assert_eq!(state, ModuleEvalState::Completed);
        assert_eq!(
            rt.eval("globalThis.result").unwrap(),
            Value::String("TypeError".into())
        );
    }

    #[test]
    fn signals_reports_what_the_platform_can_deliver() {
        let _g = v8_guard();
        let (mut rt, state) = run_signal_module(
            Arc::new(TestSignals::default()),
            "import { signals } from 'runtime:process'; \
             globalThis.result = signals().join(',');",
        );
        assert_eq!(state, ModuleEvalState::Completed);
        assert_eq!(
            rt.eval("globalThis.result").unwrap(),
            Value::String("SIGINT,SIGTERM,SIGHUP".into())
        );
    }

    /// A [`SyncFileSystem`] that records what it was asked to do and never
    /// touches a real disk — enough to prove the *gate* is what stops a call,
    /// rather than the filesystem happening to fail.
    #[derive(Default)]
    struct RecordingSyncFs {
        calls: Mutex<Vec<String>>,
    }

    impl es_runtime_providers::SyncFileSystem for RecordingSyncFs {
        fn open(
            &self,
            path: &str,
            _options: es_runtime_providers::SyncOpenOptions,
        ) -> std::result::Result<u32, es_runtime_providers::ProviderError> {
            self.calls.lock().unwrap().push(format!("open {path}"));
            Ok(1)
        }
        fn read(
            &self,
            _fd: u32,
            buf: &mut [u8],
        ) -> std::result::Result<usize, es_runtime_providers::ProviderError> {
            self.calls.lock().unwrap().push("read".into());
            let data = b"data";
            let n = data.len().min(buf.len());
            buf[..n].copy_from_slice(&data[..n]);
            Ok(n)
        }
        fn write(
            &self,
            _fd: u32,
            data: &[u8],
        ) -> std::result::Result<usize, es_runtime_providers::ProviderError> {
            self.calls.lock().unwrap().push("write".into());
            Ok(data.len())
        }
        fn seek(
            &self,
            _fd: u32,
            _offset: i64,
            _whence: es_runtime_providers::SyncWhence,
        ) -> std::result::Result<u64, es_runtime_providers::ProviderError> {
            Ok(0)
        }
        fn close(&self, _fd: u32) -> std::result::Result<(), es_runtime_providers::ProviderError> {
            Ok(())
        }
        fn fstat(
            &self,
            _fd: u32,
        ) -> std::result::Result<es_runtime_providers::FileStat, es_runtime_providers::ProviderError>
        {
            Ok(es_runtime_providers::FileStat {
                size: 4,
                is_file: true,
                is_dir: false,
                is_symlink: false,
                mtime_ms: None,
            })
        }
        fn stat(
            &self,
            _path: &str,
        ) -> std::result::Result<es_runtime_providers::FileStat, es_runtime_providers::ProviderError>
        {
            Ok(es_runtime_providers::FileStat {
                size: 4,
                is_file: true,
                is_dir: false,
                is_symlink: false,
                mtime_ms: None,
            })
        }
        fn read_dir(
            &self,
            _path: &str,
        ) -> std::result::Result<
            Vec<es_runtime_providers::DirEntry>,
            es_runtime_providers::ProviderError,
        > {
            Ok(Vec::new())
        }
        fn mkdir(
            &self,
            path: &str,
        ) -> std::result::Result<(), es_runtime_providers::ProviderError> {
            self.calls.lock().unwrap().push(format!("mkdir {path}"));
            Ok(())
        }
        fn remove_file(
            &self,
            _path: &str,
        ) -> std::result::Result<(), es_runtime_providers::ProviderError> {
            Ok(())
        }
        fn remove_dir(
            &self,
            _path: &str,
        ) -> std::result::Result<(), es_runtime_providers::ProviderError> {
            Ok(())
        }
        fn rename(
            &self,
            _from: &str,
            _to: &str,
        ) -> std::result::Result<(), es_runtime_providers::ProviderError> {
            Ok(())
        }
    }

    fn runtime_with_sync_fs(fs: Arc<RecordingSyncFs>) -> Runtime {
        let engine = V8Engine::new(Limits::default()).expect("engine");
        Runtime::new(Box::new(engine), test_providers().with_sync_file_system(fs)).expect("runtime")
    }

    /// An in-memory [`FileSystem`], so a test can exercise `runtime:fs` without
    /// a real disk — no temp directory to clean up, no file left in the repo,
    /// and every future resolves immediately, which is what lets a synchronous
    /// runner drive it. Path strings are keys: jailing and realpath resolution
    /// belong to the *default* provider (D25), not to this trait.
    #[derive(Default)]
    struct MemoryFs {
        files: Mutex<std::collections::BTreeMap<String, Vec<u8>>>,
    }

    impl MemoryFs {
        /// Boxes an immediately-ready result as the trait's future.
        fn ready<T: Send + 'static>(
            value: std::result::Result<T, es_runtime_providers::ProviderError>,
        ) -> es_runtime_providers::BoxFuture<
            std::result::Result<T, es_runtime_providers::ProviderError>,
        > {
            Box::pin(std::future::ready(value))
        }

        /// The error a real filesystem raises for a path that is not there.
        fn not_found(path: &str) -> es_runtime_providers::ProviderError {
            es_runtime_providers::ProviderError::Coded {
                code: es_runtime_common::ErrorCode::NotFound,
                message: format!("no such file or directory: {path}"),
            }
        }
    }

    impl FileSystem for MemoryFs {
        fn read(
            &self,
            path: String,
        ) -> es_runtime_providers::BoxFuture<
            std::result::Result<Vec<u8>, es_runtime_providers::ProviderError>,
        > {
            let found = self.files.lock().unwrap().get(&path).cloned();
            Self::ready(found.ok_or_else(|| Self::not_found(&path)))
        }

        fn write(
            &self,
            path: String,
            data: Vec<u8>,
            append: bool,
        ) -> es_runtime_providers::BoxFuture<
            std::result::Result<u64, es_runtime_providers::ProviderError>,
        > {
            let written = data.len() as u64;
            let mut files = self.files.lock().unwrap();
            let entry = files.entry(path).or_default();
            if !append {
                entry.clear();
            }
            entry.extend_from_slice(&data);
            Self::ready(Ok(written))
        }

        fn stat(
            &self,
            path: String,
        ) -> es_runtime_providers::BoxFuture<
            std::result::Result<
                es_runtime_providers::FileStat,
                es_runtime_providers::ProviderError,
            >,
        > {
            let size = self.files.lock().unwrap().get(&path).map(Vec::len);
            Self::ready(size.map_or_else(
                || Err(Self::not_found(&path)),
                |size| {
                    Ok(es_runtime_providers::FileStat {
                        size: size as u64,
                        is_file: true,
                        is_dir: false,
                        is_symlink: false,
                        mtime_ms: None,
                    })
                },
            ))
        }

        fn exists(
            &self,
            path: String,
        ) -> es_runtime_providers::BoxFuture<
            std::result::Result<bool, es_runtime_providers::ProviderError>,
        > {
            Self::ready(Ok(self.files.lock().unwrap().contains_key(&path)))
        }

        fn read_dir(
            &self,
            _path: String,
        ) -> es_runtime_providers::BoxFuture<
            std::result::Result<
                Vec<es_runtime_providers::DirEntry>,
                es_runtime_providers::ProviderError,
            >,
        > {
            Self::ready(Ok(Vec::new()))
        }

        fn mkdir(
            &self,
            _path: String,
            _recursive: bool,
        ) -> es_runtime_providers::BoxFuture<
            std::result::Result<(), es_runtime_providers::ProviderError>,
        > {
            // A flat map has no directories to create; a write makes its own path.
            Self::ready(Ok(()))
        }

        fn remove(
            &self,
            path: String,
            _recursive: bool,
        ) -> es_runtime_providers::BoxFuture<
            std::result::Result<(), es_runtime_providers::ProviderError>,
        > {
            let removed = self.files.lock().unwrap().remove(&path);
            Self::ready(if removed.is_some() {
                Ok(())
            } else {
                Err(Self::not_found(&path))
            })
        }

        fn rename(
            &self,
            from: String,
            to: String,
        ) -> es_runtime_providers::BoxFuture<
            std::result::Result<(), es_runtime_providers::ProviderError>,
        > {
            let mut files = self.files.lock().unwrap();
            let moved = files.remove(&from);
            Self::ready(match moved {
                Some(bytes) => {
                    files.insert(to, bytes);
                    Ok(())
                }
                None => Err(Self::not_found(&from)),
            })
        }

        fn glob_match(
            &self,
            _pattern: &str,
            _path: &str,
        ) -> std::result::Result<bool, es_runtime_providers::ProviderError> {
            Err(es_runtime_providers::ProviderError::Other(
                "the in-memory test filesystem does not implement globbing".into(),
            ))
        }

        fn glob_scan(
            &self,
            _base: String,
            _pattern: String,
            _opts: es_runtime_providers::GlobScanOptions,
        ) -> es_runtime_providers::BoxFuture<
            std::result::Result<Vec<String>, es_runtime_providers::ProviderError>,
        > {
            Self::ready(Err(es_runtime_providers::ProviderError::Other(
                "the in-memory test filesystem does not implement globbing".into(),
            )))
        }

        fn copy(
            &self,
            from: String,
            to: String,
        ) -> es_runtime_providers::BoxFuture<
            std::result::Result<u64, es_runtime_providers::ProviderError>,
        > {
            let mut files = self.files.lock().unwrap();
            // Copy-and-insert would make this a harmless no-op, but the contract
            // the system filesystem enforces is a refusal (there it is a wipe),
            // and the conformance suite runs against this double.
            if from == to && files.contains_key(&from) {
                return Self::ready(Err(es_runtime_providers::ProviderError::Coded {
                    code: es_runtime_common::ErrorCode::SameFile,
                    message: format!(
                        "Source and destination paths refer to the same file: copy '{from}' -> '{to}'"
                    ),
                }));
            }
            match files.get(&from).cloned() {
                Some(bytes) => {
                    let n = bytes.len() as u64;
                    files.insert(to, bytes);
                    Self::ready(Ok(n))
                }
                None => Self::ready(Err(Self::not_found(&from))),
            }
        }

        fn real_path(
            &self,
            path: String,
        ) -> es_runtime_providers::BoxFuture<
            std::result::Result<String, es_runtime_providers::ProviderError>,
        > {
            // Path strings are keys here — there is nothing to canonicalize.
            if self.files.lock().unwrap().contains_key(&path) {
                Self::ready(Ok(path))
            } else {
                Self::ready(Err(Self::not_found(&path)))
            }
        }

        fn read_link(
            &self,
            path: String,
        ) -> es_runtime_providers::BoxFuture<
            std::result::Result<String, es_runtime_providers::ProviderError>,
        > {
            // No links in an in-memory map; the real jail behaviour is tested
            // against the default provider.
            Self::ready(Err(Self::not_found(&path)))
        }

        fn symlink(
            &self,
            _target: String,
            path: String,
            _kind: Option<es_runtime_providers::SymlinkKind>,
        ) -> es_runtime_providers::BoxFuture<
            std::result::Result<(), es_runtime_providers::ProviderError>,
        > {
            // No links in an in-memory map, for the same reason `read_link` has
            // none: what a link does is a filesystem's behaviour, and it is
            // tested against the default provider.
            Self::ready(Err(Self::not_found(&path)))
        }

        fn truncate(
            &self,
            path: String,
            len: u64,
        ) -> es_runtime_providers::BoxFuture<
            std::result::Result<(), es_runtime_providers::ProviderError>,
        > {
            let mut files = self.files.lock().unwrap();
            match files.get_mut(&path) {
                Some(bytes) => {
                    bytes.resize(len as usize, 0);
                    Self::ready(Ok(()))
                }
                None => Self::ready(Err(Self::not_found(&path))),
            }
        }

        fn chmod(
            &self,
            _path: String,
            _mode: u32,
        ) -> es_runtime_providers::BoxFuture<
            std::result::Result<(), es_runtime_providers::ProviderError>,
        > {
            // No permission bits to set; accepted so a caller's flow still runs.
            Self::ready(Ok(()))
        }

        fn make_temp_dir(
            &self,
            dir: String,
            prefix: String,
        ) -> es_runtime_providers::BoxFuture<
            std::result::Result<String, es_runtime_providers::ProviderError>,
        > {
            Self::ready(Ok(format!("{dir}/{prefix}memtmp")))
        }

        fn make_temp_file(
            &self,
            dir: String,
            prefix: String,
        ) -> es_runtime_providers::BoxFuture<
            std::result::Result<String, es_runtime_providers::ProviderError>,
        > {
            let path = format!("{dir}/{prefix}memtmp");
            self.files.lock().unwrap().insert(path.clone(), Vec::new());
            Self::ready(Ok(path))
        }
    }

    fn runtime_with_memory_fs() -> Runtime {
        let engine = V8Engine::new(Limits::default()).expect("engine");
        Runtime::new(
            Box::new(engine),
            test_providers().with_file_system(Arc::new(MemoryFs::default())),
        )
        .expect("runtime")
    }

    /// A copy reads one path and writes another, so it must hold **both**
    /// grants. Gating it on the write alone would let a guest with no read
    /// access duplicate a file it cannot see into somewhere it can reach by
    /// another route — an exfiltration primitive out of a write-only grant.
    #[test]
    fn copy_requires_both_file_capabilities() {
        let _g = v8_guard();
        let mut rt = runtime_with_memory_fs();

        rt.set_capabilities(CapabilitySet::none().with(Capability::FileWrite));
        assert!(
            rt.eval("__ops.fs_copy('/a', '/b')").is_err(),
            "FileWrite alone must not permit a copy"
        );

        rt.set_capabilities(CapabilitySet::none().with(Capability::FileRead));
        assert!(
            rt.eval("__ops.fs_copy('/a', '/b')").is_err(),
            "FileRead alone must not permit a copy"
        );

        // Both together dispatch (and fail on the missing source, not the gate).
        rt.set_capabilities(
            CapabilitySet::none()
                .with(Capability::FileRead)
                .with(Capability::FileWrite),
        );
        let out = eval_async(
            &mut rt,
            "try { await __ops.fs_copy('/a', '/b'); return 'no throw'; } catch (e) { return e.code; }",
        );
        assert_eq!(out, Value::String("ERR_NOT_FOUND".into()));
    }

    /// Copying a file onto itself has to be refused rather than performed: on the
    /// real filesystem the destination is truncated before the source is read, so
    /// it emptied the file and reported `0` bytes copied.
    #[test]
    fn copying_a_file_onto_itself_is_refused() {
        let _g = v8_guard();
        let mut rt = runtime_with_memory_fs();
        rt.set_capabilities(
            CapabilitySet::none()
                .with(Capability::FileRead)
                .with(Capability::FileWrite),
        );
        let out = eval_async(
            &mut rt,
            "await __ops.fs_write('/a', new TextEncoder().encode('payload')); \
             try { await __ops.fs_copy('/a', '/a'); return 'no throw'; } catch (e) { return e.code; }",
        );
        assert_eq!(out, Value::String("ERR_SAME_FILE".into()));

        let survived = eval_async(
            &mut rt,
            "return new TextDecoder().decode(await __ops.fs_read('/a'));",
        );
        assert_eq!(survived, Value::String("payload".into()));
    }

    /// The read-side and write-side additions land on the gate that matches what
    /// they do, rather than all defaulting to one.
    #[test]
    fn the_new_fs_ops_are_gated_by_what_they_do() {
        let _g = v8_guard();
        let mut rt = runtime_with_memory_fs();

        rt.set_capabilities(CapabilitySet::none().with(Capability::FileWrite));
        for read_op in ["__ops.fs_real_path('/a')", "__ops.fs_read_link('/a')"] {
            assert!(
                rt.eval(read_op).is_err(),
                "{read_op} must need FileRead, not FileWrite"
            );
        }

        rt.set_capabilities(CapabilitySet::none().with(Capability::FileRead));
        for write_op in [
            "__ops.fs_truncate('/a', 0)",
            "__ops.fs_chmod('/a', 384)",
            // Creating a link stores a string and reads nothing; following one
            // later is a read, and gated where reads are.
            "__ops.fs_symlink('/a', '/b', null)",
            "__ops.fs_make_temp_dir('', 't-')",
            "__ops.fs_make_temp_file('', 't-')",
        ] {
            assert!(
                rt.eval(write_op).is_err(),
                "{write_op} must need FileWrite, not FileRead"
            );
        }
    }

    /// The synchronous filesystem is behind the same gates as the async one:
    /// without `FileWrite`, a mutating op is denied before the provider is
    /// consulted at all.
    #[test]
    fn sync_fs_ops_are_capability_gated() {
        let _g = v8_guard();
        let fs = Arc::new(RecordingSyncFs::default());
        let mut rt = runtime_with_sync_fs(fs.clone());

        // Deny-by-default: nothing granted, so even a read is refused.
        rt.set_capabilities(CapabilitySet::none());
        assert!(
            rt.eval("__ops.sync_fs_open('/x', { read: true })").is_err(),
            "a read open must need FileRead"
        );

        // FileRead alone permits reading but not mutating.
        rt.set_capabilities(CapabilitySet::none().with(Capability::FileRead));
        rt.eval("__ops.sync_fs_open('/x', { read: true })")
            .expect("read open is permitted by FileRead");
        assert!(
            rt.eval("__ops.sync_fs_mkdir('/d')").is_err(),
            "mkdir must need FileWrite"
        );
        assert!(
            rt.eval("__ops.sync_fs_open_write('/x', { write: true })")
                .is_err(),
            "a write open must need FileWrite"
        );

        // The provider saw only the permitted call — the denials never reached it.
        let calls = fs.calls.lock().unwrap().clone();
        assert_eq!(calls, vec!["open /x".to_string()], "calls: {calls:?}");

        // Granting FileWrite opens the mutating paths.
        rt.set_capabilities(
            CapabilitySet::none()
                .with(Capability::FileRead)
                .with(Capability::FileWrite),
        );
        rt.eval("__ops.sync_fs_mkdir('/d')")
            .expect("mkdir is permitted by FileWrite");
    }

    /// The read-only open op refuses a mutating mode outright, so the
    /// `FileWrite` gate on `sync_fs_open_write` cannot be sidestepped by asking
    /// the `FileRead`-gated op to create a file.
    #[test]
    fn the_read_open_op_refuses_a_mutating_mode() {
        let _g = v8_guard();
        let fs = Arc::new(RecordingSyncFs::default());
        let mut rt = runtime_with_sync_fs(fs.clone());
        rt.set_capabilities(CapabilitySet::none().with(Capability::FileRead));

        for mode in [
            "{ write: true }",
            "{ create: true }",
            "{ truncate: true }",
            "{ createNew: true }",
        ] {
            let err = rt
                .eval(&format!("__ops.sync_fs_open('/x', {mode})"))
                .unwrap_err();
            assert!(
                format!("{err}").contains("sync_fs_open_write"),
                "mode {mode} should be redirected, got: {err}"
            );
        }
        assert!(
            fs.calls.lock().unwrap().is_empty(),
            "no mutating open should have reached the provider"
        );
    }

    /// With no synchronous filesystem installed, the ops fail cleanly rather
    /// than panicking — which is what makes WASI report `ENOTCAPABLE`.
    #[test]
    fn sync_fs_ops_without_a_provider_fail_cleanly() {
        let _g = v8_guard();
        let mut rt = runtime();
        rt.set_capabilities(
            CapabilitySet::none()
                .with(Capability::FileRead)
                .with(Capability::FileWrite),
        );
        let err = rt
            .eval("__ops.sync_fs_open('/x', { read: true })")
            .unwrap_err();
        assert!(
            format!("{err}").contains("no synchronous filesystem"),
            "got: {err}"
        );
    }

    /// An in-process [`PortHub`] for the tests, so `MessagePort` exercises the
    /// host-backed path the CLI uses rather than the agent-local fallback.
    /// Deliberately simple: one inbox per port, delivery to the peer.
    #[derive(Default)]
    struct TestPortHub {
        ports: Arc<std::sync::Mutex<HashMap<u64, TestPort>>>,
        next: std::sync::atomic::AtomicU64,
    }

    #[derive(Default)]
    struct TestPort {
        peer: Option<u64>,
        queue: std::collections::VecDeque<Vec<u8>>,
    }

    impl es_runtime_providers::PortHub for TestPortHub {
        fn create(&self) -> std::result::Result<(u64, u64), es_runtime_providers::ProviderError> {
            let mut ports = self.ports.lock().expect("port lock");
            let a = self.next.fetch_add(2, std::sync::atomic::Ordering::SeqCst) + 1;
            let b = a + 1;
            ports.insert(
                a,
                TestPort {
                    peer: Some(b),
                    ..TestPort::default()
                },
            );
            ports.insert(
                b,
                TestPort {
                    peer: Some(a),
                    ..TestPort::default()
                },
            );
            Ok((a, b))
        }

        fn post(
            &self,
            id: u64,
            message: Vec<u8>,
        ) -> std::result::Result<(), es_runtime_providers::ProviderError> {
            let mut ports = self.ports.lock().expect("port lock");
            if let Some(peer) = ports.get(&id).and_then(|port| port.peer)
                && let Some(port) = ports.get_mut(&peer)
            {
                port.queue.push_back(message);
            }
            Ok(())
        }

        fn recv(
            &self,
            id: u64,
        ) -> es_runtime_providers::BoxFuture<
            std::result::Result<Option<Vec<u8>>, es_runtime_providers::ProviderError>,
        > {
            // Stays `Pending` while the queue is empty. No waker is registered
            // because none is needed: the runtime re-polls every pending op on
            // each tick, which is exactly what these tests drive by hand.
            let ports = self.ports.clone();
            Box::pin(std::future::poll_fn(move |_cx| {
                let mut ports = ports.lock().expect("port lock");
                match ports.get_mut(&id) {
                    None => std::task::Poll::Ready(Ok(None)),
                    Some(port) => match port.queue.pop_front() {
                        Some(message) => std::task::Poll::Ready(Ok(Some(message))),
                        None => std::task::Poll::Pending,
                    },
                }
            }))
        }

        fn detach_reader(&self, _id: u64) {}

        fn close(&self, id: u64) {
            let mut ports = self.ports.lock().expect("port lock");
            if let Some(port) = ports.remove(&id)
                && let Some(peer) = port.peer
                && let Some(peer) = ports.get_mut(&peer)
            {
                peer.peer = None;
            }
        }
    }

    use es_runtime_providers::{WorkerIncoming, WorkerSpec};

    /// A [`WorkerHost`] that starts no threads and runs no isolates.
    ///
    /// The end-to-end worker tests drive the real host through `esrun`, which
    /// costs an OS thread and a V8 isolate per case and can only observe what a
    /// worker prints. This sees the other side: exactly what the runtime asks
    /// the host to do, in order, with nothing in between. It is also the only
    /// way to assert what an embedder's own `WorkerHost` will be called with.
    #[derive(Default)]
    struct TestWorkerHost {
        /// Every call, in order — `spawn`, `post`, `terminate`.
        calls: Arc<std::sync::Mutex<Vec<String>>>,
        /// What each worker's `recv` hands back, oldest first. Drained, so a
        /// worker with nothing left reports `Closed` and is done.
        inbox: Arc<std::sync::Mutex<Vec<WorkerIncoming>>>,
        /// The `WorkerSpec` of the most recent spawn, so a test can assert what
        /// the runtime narrowed before asking for an agent.
        last: Arc<std::sync::Mutex<Option<WorkerSpec>>>,
    }

    impl TestWorkerHost {
        fn calls(&self) -> Vec<String> {
            self.calls.lock().expect("calls").clone()
        }

        fn queue(&self, event: WorkerIncoming) {
            self.inbox.lock().expect("inbox").push(event);
        }

        fn last_spec(&self) -> WorkerSpec {
            self.last
                .lock()
                .expect("last")
                .clone()
                .expect("a worker was spawned")
        }
    }

    impl WorkerHost for TestWorkerHost {
        fn spawn(
            &self,
            spec: WorkerSpec,
        ) -> es_runtime_providers::BoxFuture<
            std::result::Result<u64, es_runtime_providers::ProviderError>,
        > {
            self.calls
                .lock()
                .expect("calls")
                .push(format!("spawn {}", spec.specifier));
            *self.last.lock().expect("last") = Some(spec);
            Box::pin(std::future::ready(Ok(1)))
        }

        fn post(
            &self,
            id: u64,
            message: Vec<u8>,
        ) -> std::result::Result<(), es_runtime_providers::ProviderError> {
            self.calls
                .lock()
                .expect("calls")
                .push(format!("post {id} ({} bytes)", message.len()));
            Ok(())
        }

        fn queued(&self, _id: u64) -> usize {
            0
        }

        fn recv(
            &self,
            _id: u64,
        ) -> es_runtime_providers::BoxFuture<
            std::result::Result<Option<WorkerIncoming>, es_runtime_providers::ProviderError>,
        > {
            let next = {
                let mut inbox = self.inbox.lock().expect("inbox");
                if inbox.is_empty() {
                    None
                } else {
                    Some(inbox.remove(0))
                }
            };
            Box::pin(std::future::ready(Ok(next)))
        }

        fn terminate(
            &self,
            id: u64,
        ) -> es_runtime_providers::BoxFuture<
            std::result::Result<(), es_runtime_providers::ProviderError>,
        > {
            self.calls
                .lock()
                .expect("calls")
                .push(format!("terminate {id}"));
            Box::pin(std::future::ready(Ok(())))
        }
    }

    /// A runtime that can start workers, holding every capability so the tests
    /// below are about the worker seam rather than about a denial.
    fn runtime_with_workers(host: Arc<TestWorkerHost>) -> Runtime {
        let mut rt = Runtime::new(
            Box::new(V8Engine::new(es_runtime_common::Limits::default()).expect("engine")),
            HostProviders::new(
                Arc::new(FixedClock {
                    monotonic: 0,
                    wall: 0,
                }),
                Arc::new(TestConsole::default()),
                Arc::new(MockNet::stub()),
                Arc::new(TestEntropy::new()),
            )
            .with_workers(host),
        )
        .expect("runtime");
        rt.set_capabilities(CapabilitySet::all());
        rt
    }

    /// Loads `main` as the entry module with a worker script available at
    /// `./w.mjs`, then drives the loop far enough for the spawn and the first
    /// receives to settle.
    ///
    /// The loader has to be installed the way a real program installs one —
    /// through a module load — because reading a worker's entry goes through
    /// it, and a runtime with no loader refuses to start a worker at all.
    fn drive_worker_main(rt: &mut Runtime, main: &str) {
        block_on(rt.load_module_source(
            "file:///app/main.mjs",
            main,
            MapLoader::new(&[("./w.mjs", "// the worker's own body never runs here")]),
        ))
        .expect("load main");
        for _ in 0..16 {
            rt.tick(0);
        }
    }

    /// What the runtime actually asks a host to start — the narrowing every
    /// other worker test can only observe indirectly, asserted at the seam.
    #[test]
    fn a_spawn_hands_the_host_a_narrowed_spec() {
        let _g = v8_guard();
        let host = Arc::new(TestWorkerHost::default());
        let mut rt = runtime_with_workers(host.clone());
        drive_worker_main(
            &mut rt,
            "globalThis.__w = new Worker('./w.mjs', \
               { name: 'jobs', permissions: ['net'], memory: 64 });",
        );

        let spec = host.last_spec();
        assert_eq!(spec.name, "jobs");
        assert_eq!(spec.specifier, "file:///app/w.mjs");
        // Exactly what was asked for, and the ungated ones that ride along.
        assert!(spec.capabilities.contains(Capability::Net));
        assert!(!spec.capabilities.contains(Capability::FileWrite));
        assert!(!spec.capabilities.contains(Capability::Worker));
        // Loading the child's graph is the *parent's* authority, and only that.
        assert!(spec.load_capabilities.contains(Capability::FileSystem));
        assert!(!spec.load_capabilities.contains(Capability::Net));
        // `{ memory: 64 }` in megabytes, lowered from this agent's own ceiling.
        assert_eq!(spec.limits.heap_limit_bytes, Some(64 * 1024 * 1024));
        // A worker owns its thread, so `Atomics.wait` is legal there.
        assert!(spec.limits.can_block);
    }

    /// `permissions: "inherit"` is a ceiling, not an escape: the host is asked
    /// for what this agent holds and no more.
    #[test]
    fn inherited_permissions_are_bounded_by_the_parents_own() {
        let _g = v8_guard();
        let host = Arc::new(TestWorkerHost::default());
        let mut rt = runtime_with_workers(host.clone());
        let mut narrowed = CapabilitySet::all();
        narrowed.revoke(Capability::Net);
        rt.set_capabilities(narrowed);
        drive_worker_main(
            &mut rt,
            "globalThis.__w = new Worker('./w.mjs', { permissions: 'inherit' });",
        );

        let spec = host.last_spec();
        assert!(spec.capabilities.contains(Capability::FileRead));
        assert!(
            !spec.capabilities.contains(Capability::Net),
            "a worker cannot inherit what its parent does not hold"
        );
    }

    /// Messages queued before the spawn resolves reach the host in the order
    /// they were posted — the regression this seam exists to pin down, since
    /// end-to-end it can only be observed by what the worker prints.
    #[test]
    fn queued_messages_reach_the_host_in_order() {
        let _g = v8_guard();
        let host = Arc::new(TestWorkerHost::default());
        let mut rt = runtime_with_workers(host.clone());
        drive_worker_main(
            &mut rt,
            "globalThis.__w = new Worker('./w.mjs'); \
             for (let i = 0; i < 3; i++) __w.postMessage(new Uint8Array(i + 1));",
        );

        let calls = host.calls();
        assert_eq!(calls[0], "spawn file:///app/w.mjs", "{calls:?}");
        // Byte lengths stand in for identity: growing means the order held.
        let posts: Vec<&String> = calls.iter().filter(|c| c.starts_with("post")).collect();
        assert_eq!(posts.len(), 3, "{calls:?}");
        assert!(posts[0] < posts[1] && posts[1] < posts[2], "{posts:?}");
    }

    /// A failure the host reports reaches the parent's `onerror` with its class
    /// rebuilt — the whole point of describing a failure rather than formatting
    /// it, checked without a second isolate to throw in.
    #[test]
    fn a_hosts_error_reaches_onerror_with_its_class() {
        let _g = v8_guard();
        let host = Arc::new(TestWorkerHost::default());
        host.queue(WorkerIncoming::Error {
            error: UncaughtError::new(
                "RangeError",
                "out of range",
                "RangeError: out of range\n    at inner (file:///w.mjs:2:9)",
            ),
        });
        let mut rt = runtime_with_workers(host.clone());
        drive_worker_main(
            &mut rt,
            "globalThis.__seen = null; \
             globalThis.__w = new Worker('./w.mjs'); \
             __w.onerror = (e) => { \
               __seen = [e.error.name, e.message, e.filename, e.lineno, e.colno, \
                         e.error instanceof RangeError].join('|'); \
               e.preventDefault(); \
             };",
        );

        assert_eq!(
            rt.eval("globalThis.__seen").unwrap(),
            Value::String("RangeError|out of range|file:///w.mjs|2|9|true".into())
        );
    }

    /// `terminate()` reaches the host, and stops the runtime asking for more.
    #[test]
    fn terminate_reaches_the_host_and_ends_the_pump() {
        let _g = v8_guard();
        let host = Arc::new(TestWorkerHost::default());
        let mut rt = runtime_with_workers(host.clone());
        drive_worker_main(&mut rt, "globalThis.__w = new Worker('./w.mjs');");
        rt.eval("__w.terminate(); __w.postMessage('dropped');")
            .unwrap();
        for _ in 0..8 {
            rt.tick(0);
        }

        let calls = host.calls();
        assert!(calls.contains(&"terminate 1".to_string()), "{calls:?}");
        // Nothing is posted after a terminate, even though the host would
        // happily have taken it.
        let after = calls
            .iter()
            .position(|c| c == "terminate 1")
            .expect("terminate");
        assert!(
            !calls[after..].iter().any(|c| c.starts_with("post")),
            "{calls:?}"
        );
    }

    fn test_providers() -> HostProviders {
        HostProviders::new(
            Arc::new(FixedClock {
                monotonic: 0,
                wall: 0,
            }),
            Arc::new(TestConsole::default()),
            Arc::new(MockNet::stub()),
            Arc::new(TestEntropy::new()),
        )
        .with_ports(Arc::new(TestPortHub::default()))
    }

    #[test]
    fn snapshot_runtime_runs_baked_prelude() {
        // Bake the real ops + full prelude into a snapshot, restore a runtime
        // from it, and exercise several op-backed APIs to prove the baked
        // context behaves like a freshly-built one (DECISIONS.md D8).
        let _g = v8_guard();
        let blob = Runtime::build_snapshot(&test_providers()).expect("build snapshot");
        let mut rt = Runtime::with_snapshot(blob, test_providers()).expect("restore");
        let out = eval_async(
            &mut rt,
            "const u = new URL('https://x.test/a?b=1'); \
             const h = await crypto.subtle.digest('SHA-256', new TextEncoder().encode('abc')); \
             const id = crypto.randomUUID(); \
             console.log('from snapshot'); \
             return `${u.host}|${new Uint8Array(h).length}|${id.length}`;",
        );
        assert_eq!(out, Value::String("x.test|32|36".into()));
    }

    #[test]
    fn snapshot_runtime_async_ops_and_timers_work() {
        // The driven loop (timers + async settling) must work over a restored
        // engine just as over a fresh one.
        let _g = v8_guard();
        let blob = Runtime::build_snapshot(&test_providers()).expect("build snapshot");
        let mut rt = Runtime::with_snapshot(blob, test_providers()).expect("restore");
        // `eval_async` drives ticks at now=0, so use a 0ms timer (fires at 0);
        // this still exercises the baked `setTimeout` builtin + the driven loop.
        let out = eval_async(
            &mut rt,
            "let v = 0; \
             await new Promise((r) => setTimeout(() => { v = 7; r(); }, 0)); \
             return v;",
        );
        assert_eq!(out, Value::Number(7.0));
    }

    /// A throw out of a timer callback used to be swallowed whole: the callback
    /// has no caller left to propagate to, and nothing reported it. It must
    /// reach the embedder.
    #[test]
    fn an_exception_out_of_a_timer_reaches_the_embedder() {
        let _g = v8_guard();
        let mut rt = runtime();
        rt.eval("setTimeout(() => { throw new TypeError('from a timer'); }, 0);")
            .unwrap();
        let status = rt.tick(0);
        assert_eq!(status.timers_fired, 1);
        assert_eq!(
            status.uncaught_errors.len(),
            1,
            "{:?}",
            status.uncaught_errors
        );
        assert!(
            status.uncaught_errors[0]
                .to_string()
                .contains("from a timer"),
            "{}",
            status.uncaught_errors[0]
        );
        // Drained, not repeated on the next tick.
        assert!(rt.tick(1).uncaught_errors.is_empty());
    }

    /// …unless a listener claims it. `preventDefault()` is how guest code takes
    /// responsibility, and the host must then stay quiet.
    #[test]
    fn an_error_listener_can_claim_a_timer_exception() {
        let _g = v8_guard();
        let mut rt = runtime();
        rt.eval(
            "globalThis.seen = null; \
             globalThis.addEventListener('error', (e) => { \
               globalThis.seen = e.message; e.preventDefault(); }); \
             setTimeout(() => { throw new TypeError('claimed'); }, 0);",
        )
        .unwrap();
        let status = rt.tick(0);
        assert!(
            status.uncaught_errors.is_empty(),
            "{:?}",
            status.uncaught_errors
        );
        assert_eq!(
            rt.eval("globalThis.seen").unwrap(),
            Value::String("claimed".into())
        );
    }

    /// A rejection nobody handled is reported — and a listener that claims it
    /// with `preventDefault()` keeps it away from the embedder entirely.
    #[test]
    fn an_unhandled_rejection_can_be_claimed_by_a_listener() {
        let _g = v8_guard();
        let mut rt = runtime();
        rt.eval("Promise.reject(new Error('unclaimed'));").unwrap();
        let status = rt.tick(0);
        assert_eq!(
            status.unhandled_rejections.len(),
            1,
            "{:?}",
            status.unhandled_rejections
        );

        let mut rt = runtime();
        rt.eval(
            "globalThis.reason = null; \
             globalThis.addEventListener('unhandledrejection', (e) => { \
               globalThis.reason = e.reason.message; e.preventDefault(); }); \
             Promise.reject(new Error('claimed'));",
        )
        .unwrap();
        let status = rt.tick(0);
        assert!(
            status.unhandled_rejections.is_empty(),
            "{:?}",
            status.unhandled_rejections
        );
        assert_eq!(
            rt.eval("globalThis.reason").unwrap(),
            Value::String("claimed".into())
        );
    }

    /// Attaching a handler *after* the report has gone out fires
    /// `rejectionhandled` — the spec's retraction. It cannot be covered by the
    /// conformance suite, which would have to leave the report unclaimed and so
    /// fail the `esrun` runner it also runs under.
    #[test]
    fn a_late_handler_fires_rejectionhandled() {
        let _g = v8_guard();
        let mut rt = runtime();
        rt.eval(
            "globalThis.retracted = 0; \
             globalThis.addEventListener('rejectionhandled', (e) => { \
               if (e.promise === globalThis.p) globalThis.retracted++; }); \
             globalThis.p = Promise.reject(new Error('late'));",
        )
        .unwrap();
        // The report goes out: no listener claimed it.
        assert_eq!(rt.tick(0).unhandled_rejections.len(), 1);
        assert_eq!(rt.eval("globalThis.retracted").unwrap(), Value::Number(0.0));

        // Now a handler arrives, retracting it.
        rt.eval("globalThis.p.catch(() => {});").unwrap();
        rt.tick(1);
        assert_eq!(rt.eval("globalThis.retracted").unwrap(), Value::Number(1.0));
    }

    #[test]
    fn clearing_a_timer_releases_the_loop_immediately() {
        // Regression: `clearTimeout` cancelled the callback but left the entry
        // in the schedule, so the runtime reported pending work — and a driver
        // slept — until the original delay elapsed. A cleared 60s timer must
        // leave nothing outstanding and no deadline to wait on.
        let _g = v8_guard();
        let mut rt = runtime();
        rt.eval("const id = setTimeout(() => {}, 60000); clearTimeout(id);")
            .unwrap();
        let outcome = rt.tick(0);
        assert_eq!(outcome.timers_fired, 0);
        assert_eq!(outcome.next_timer_deadline_ms, None);
        assert!(!outcome.has_pending_work);
        assert!(!rt.has_pending_work());
    }

    #[test]
    fn clearing_one_timer_leaves_the_others_scheduled() {
        // Pruning must not take live timers with it: the surviving timer keeps
        // its own deadline and still fires.
        let _g = v8_guard();
        let mut rt = runtime();
        rt.eval(
            "globalThis.fired = false; \
             const dead = setTimeout(() => {}, 60000); \
             setTimeout(() => { globalThis.fired = true; }, 10); \
             clearTimeout(dead);",
        )
        .unwrap();
        assert_eq!(rt.tick(0).next_timer_deadline_ms, Some(10));
        let outcome = rt.tick(10);
        assert_eq!(outcome.timers_fired, 1);
        assert_eq!(rt.eval("globalThis.fired").unwrap(), Value::Bool(true));
        assert!(!rt.has_pending_work());
    }

    #[test]
    fn sync_op_is_callable_from_js() {
        let _g = v8_guard();
        let mut rt = runtime();
        rt.register_op(OpDecl::sync("add", |args| {
            let a = args.first().and_then(Value::as_number).unwrap_or(0.0);
            let b = args.get(1).and_then(Value::as_number).unwrap_or(0.0);
            Ok(Value::Number(a + b))
        }))
        .unwrap();
        assert_eq!(rt.eval("__ops.add(2, 3)").unwrap(), Value::Number(5.0));
    }

    #[test]
    fn capability_gated_op_denies_then_allows() {
        let _g = v8_guard();
        let mut rt = runtime();
        rt.register_op(
            OpDecl::sync("netcall", |_args| Ok(Value::Bool(true))).requires(Capability::Net),
        )
        .unwrap();

        // Deny-by-default: the op throws before its handler runs.
        assert!(rt.eval("__ops.netcall()").is_err());

        rt.set_capabilities(CapabilitySet::none().with(Capability::Net));
        assert_eq!(rt.eval("__ops.netcall()").unwrap(), Value::Bool(true));
    }

    #[test]
    fn async_op_resolves_across_a_tick() {
        let _g = v8_guard();
        let mut rt = runtime();
        // Pending on the first poll, ready on the second — so this op takes the
        // registration path rather than the dispatcher's eager one, which is
        // what `async_ops_settled` counts.
        rt.register_op(OpDecl::r#async("answer", |_args| -> AsyncOp {
            let mut polled = false;
            Box::pin(async move {
                std::future::poll_fn(move |cx| {
                    if polled {
                        std::task::Poll::Ready(())
                    } else {
                        polled = true;
                        cx.waker().wake_by_ref();
                        std::task::Poll::Pending
                    }
                })
                .await;
                Ok(Value::Number(42.0))
            })
        }))
        .unwrap();

        // The op returns a pending promise; its `.then` has not run yet.
        rt.eval("globalThis.result = 0; __ops.answer().then((v) => { globalThis.result = v; });")
            .unwrap();
        assert_eq!(rt.eval("globalThis.result").unwrap(), Value::Number(0.0));

        // One tick settles the op and runs the microtask that observes it.
        let status = rt.tick(0);
        assert_eq!(status.async_ops_settled, 1);
        assert_eq!(rt.eval("globalThis.result").unwrap(), Value::Number(42.0));
        assert!(!rt.has_pending_work());
    }

    /// An async op whose future is ready on its first poll settles its promise
    /// during the call, without ever being registered as pending work — but its
    /// *reactions* still wait for the microtask checkpoint, so `await` behaves
    /// exactly as it does for an op that took a trip through the loop.
    ///
    /// This is the difference between "resolved" and "delivered", and getting it
    /// wrong would run guest code inside a host op call. The ops this affects are
    /// the ones that are async only in shape (`fs.exists` answers from a single
    /// syscall), which were paying a full driver round trip each.
    #[test]
    fn an_immediately_ready_async_op_settles_without_a_loop_turn() {
        let _g = v8_guard();
        let mut rt = runtime();
        rt.register_op(OpDecl::r#async("answer", |_args| -> AsyncOp {
            Box::pin(async { Ok(Value::Number(42.0)) })
        }))
        .unwrap();

        // Still not delivered synchronously: the reaction is a microtask.
        rt.eval("globalThis.result = 0; __ops.answer().then((v) => { globalThis.result = v; });")
            .unwrap();
        assert_eq!(rt.eval("globalThis.result").unwrap(), Value::Number(0.0));

        // Nothing was ever registered, so the tick settles no pending op — and
        // the value arrives all the same.
        let status = rt.tick(0);
        assert_eq!(status.async_ops_settled, 0);
        assert_eq!(rt.eval("globalThis.result").unwrap(), Value::Number(42.0));
        assert!(!rt.has_pending_work());
    }

    /// A rejected eager op rejects its promise, rather than resolving it or
    /// throwing synchronously out of the op call.
    #[test]
    fn an_immediately_failing_async_op_rejects_its_promise() {
        let _g = v8_guard();
        let mut rt = runtime();
        rt.register_op(OpDecl::r#async("boom", |_args| -> AsyncOp {
            Box::pin(async { Err(OpError::type_error("no")) })
        }))
        .unwrap();

        rt.eval(
            "globalThis.caught = ''; __ops.boom().catch((e) => { globalThis.caught = e.message; });",
        )
        .unwrap();
        rt.tick(0);
        assert_eq!(
            rt.eval("globalThis.caught").unwrap(),
            Value::String("no".into())
        );
    }

    /// The smallest valid module: `(func (export "add") (param i32 i32)
    /// (result i32) local.get 0 local.get 1 i32.add)`.
    const ADD_WASM: &str = "new Uint8Array([\
        0,97,115,109,1,0,0,0,\
        1,7,1,96,2,127,127,1,127,\
        3,2,1,0,\
        7,7,1,3,97,100,100,0,0,\
        10,9,1,7,0,32,0,32,1,106,11])";

    /// The waiting helper must stop the moment the condition holds — the hot
    /// path is every `eval_async` in this file, including a 20k-iteration soak.
    #[test]
    fn pump_until_returns_as_soon_as_the_condition_holds() {
        let _g = v8_guard();
        let mut rt = runtime();
        let mut ticks = 0u32;
        let started = std::time::Instant::now();
        pump_until(&mut rt, "three ticks", |_| {
            ticks += 1;
            ticks == 3
        });
        assert_eq!(ticks, 3);
        // Three spins, no sleeps: this is microseconds, not milliseconds.
        assert!(started.elapsed() < std::time::Duration::from_millis(100));
    }

    /// The bug this helper replaced: a count-bounded loop gave up *silently*,
    /// so the test went on to assert against a result that had not arrived and
    /// reported the miss as a behaviour failure. Running out of time must be a
    /// panic that names what was awaited.
    #[test]
    #[should_panic(expected = "waiting for something that never happens")]
    fn pump_until_panics_instead_of_falling_through() {
        let _g = v8_guard();
        let mut rt = runtime();
        pump_until_within(
            &mut rt,
            std::time::Duration::from_millis(50),
            "something that never happens",
            |_| false,
        );
    }

    /// An async WebAssembly compile settles only because the loop pumps V8's
    /// foreground task queue, and counts as pending work until it does — without
    /// which a driver would exit (or park forever) mid-compile. The conformance
    /// suite cannot cover this: it runs files without a driver.
    #[test]
    fn async_wasm_compile_settles_and_holds_the_loop_open() {
        let _g = v8_guard();
        let mut rt = runtime();

        rt.eval(&format!(
            "globalThis.mod = null; \
             WebAssembly.compile({ADD_WASM}).then((m) => {{ globalThis.mod = m; }});"
        ))
        .unwrap();

        // In flight: nothing has resolved, and the runtime must report work
        // outstanding even though no op, timer, or import is pending.
        assert_eq!(rt.eval("globalThis.mod").unwrap(), Value::Null);
        assert!(
            rt.has_pending_work(),
            "an in-flight wasm compile must keep the loop alive"
        );

        // Ticking pumps the platform until V8's compile task lands. How long
        // that takes is up to V8's background threads, so wait on the clock.
        pump_until(&mut rt, "the wasm compile to settle", |rt| {
            !rt.has_pending_work()
        });

        assert_eq!(
            rt.eval("globalThis.mod instanceof WebAssembly.Module")
                .unwrap(),
            Value::Bool(true),
            "the compile promise never settled"
        );
        assert!(
            !rt.has_pending_work(),
            "the compile should no longer be pending"
        );
    }

    /// A rejected compile must release its pending-work claim too, or the loop
    /// would never quiesce after a bad module.
    #[test]
    fn failed_async_wasm_compile_releases_pending_work() {
        let _g = v8_guard();
        let mut rt = runtime();

        rt.eval(
            "globalThis.err = null; \
             WebAssembly.compile(new Uint8Array([0,1,2,3])).catch((e) => { globalThis.err = e.name; });",
        )
        .unwrap();

        pump_until(&mut rt, "the failed wasm compile to settle", |rt| {
            !rt.has_pending_work()
        });

        assert_eq!(
            rt.eval("globalThis.err").unwrap(),
            Value::String("CompileError".into())
        );
        assert!(!rt.has_pending_work());
    }

    #[test]
    fn set_timeout_fires_only_after_its_deadline() {
        let _g = v8_guard();
        let mut rt = runtime();
        rt.eval("globalThis.fired = false; setTimeout(() => { globalThis.fired = true; }, 50);")
            .unwrap();

        // Before the deadline: scheduled, not fired.
        let early = rt.tick(10);
        assert_eq!(early.timers_fired, 0);
        assert_eq!(early.next_timer_deadline_ms, Some(50));
        assert_eq!(rt.eval("globalThis.fired").unwrap(), Value::Bool(false));

        // At/after the deadline: fires exactly once, then no work remains.
        let late = rt.tick(50);
        assert_eq!(late.timers_fired, 1);
        assert_eq!(rt.eval("globalThis.fired").unwrap(), Value::Bool(true));
        assert!(!rt.has_pending_work());
    }

    #[test]
    fn clear_timeout_prevents_firing() {
        let _g = v8_guard();
        let mut rt = runtime();
        rt.eval(
            "globalThis.fired = false; \
             const id = setTimeout(() => { globalThis.fired = true; }, 20); \
             clearTimeout(id);",
        )
        .unwrap();
        let status = rt.tick(100);
        assert_eq!(status.timers_fired, 0);
        assert_eq!(rt.eval("globalThis.fired").unwrap(), Value::Bool(false));
    }

    #[test]
    fn unhandled_rejection_is_reported() {
        let _g = v8_guard();
        let mut rt = runtime();
        rt.eval("Promise.reject('boom'); undefined").unwrap();
        let status = rt.tick(0);
        assert!(
            status
                .unhandled_rejections
                .iter()
                .any(|error| error.to_string().contains("boom")),
            "got {:?}",
            status.unhandled_rejections
        );
    }

    #[test]
    fn idle_runtime_reports_no_work() {
        let _g = v8_guard();
        let mut rt = runtime();
        rt.eval("1 + 1").unwrap();
        let status = rt.tick(0);
        assert!(!status.has_pending_work);
        assert_eq!(status.next_timer_deadline_ms, None);
    }

    #[test]
    fn console_routes_to_the_injected_sink() {
        let _g = v8_guard();
        let console = Arc::new(TestConsole::default());
        let mut rt = runtime_with(
            console.clone(),
            Arc::new(FixedClock {
                monotonic: 0,
                wall: 0,
            }),
        );
        rt.eval(r#"console.log("hi", 42); console.error("boom");"#)
            .unwrap();
        let lines = console.lines.lock().unwrap().clone();
        assert_eq!(
            lines,
            vec![
                (ConsoleLevel::Log, "hi 42".to_string()),
                (ConsoleLevel::Error, "boom".to_string()),
            ]
        );
    }

    /// Captures everything the guest logged, as `level: message` lines.
    fn console_lines(script: &str) -> Vec<String> {
        let console = Arc::new(TestConsole::default());
        let mut rt = runtime_with(
            console.clone(),
            Arc::new(FixedClock {
                monotonic: 0,
                wall: 0,
            }),
        );
        rt.eval(script).unwrap();
        let lines = console.lines.lock().unwrap().clone();
        lines
            .into_iter()
            .map(|(level, message)| format!("{level:?}: {message}").to_lowercase())
            .collect()
    }

    /// The Console Standard's Formatter: `%s`/`%d`/`%i`/`%f`/`%o`/`%O`/`%j`
    /// consume the following arguments, `%%` is a literal, and anything left
    /// over is appended.
    #[test]
    fn console_applies_format_specifiers() {
        let _g = v8_guard();
        let lines = console_lines(
            r#"console.log("%s is %d and %f", "Ada", 36.7, 1.5);
               console.log("%o|%j", { a: [1, 2] }, { b: 2 });
               console.log("100%% sure");
               console.log("%s", "one", "extra", 2);
               console.log("%d", "not a number");
               console.log("no specifiers", 1, { a: 1 });"#,
        );
        assert_eq!(
            lines,
            vec![
                "log: ada is 36 and 1.5",
                "log: { a: [ 1, 2 ] }|{\"b\":2}",
                // A lone string with no arguments is not a format string.
                "log: 100%% sure",
                "log: one extra 2",
                "log: nan",
                "log: no specifiers 1 { a: 1 }",
            ]
        );
    }

    /// A group indents everything until it closes, including the inner lines of
    /// a multi-line value.
    #[test]
    fn console_group_indents_until_closed() {
        let _g = v8_guard();
        let lines = console_lines(
            r#"console.group("outer");
               console.log("a\nb");
               console.group();
               console.log("deep");
               console.groupEnd();
               console.groupEnd();
               console.log("flush left");"#,
        );
        assert_eq!(
            lines,
            vec![
                "log: outer",
                "log:   a\n  b",
                "log:     deep",
                "log: flush left",
            ]
        );
    }

    /// `count`/`countReset` and `time`/`timeLog`/`timeEnd` were no-ops that
    /// silently discarded what they were asked to measure.
    #[test]
    fn console_counts_and_times() {
        let _g = v8_guard();
        let lines = console_lines(
            r#"console.count(); console.count(); console.count("x");
               console.countReset(); console.count();
               console.time("t"); console.timeLog("t", "mid"); console.timeEnd("t");
               console.timeEnd("t");
               console.time("t"); console.time("t");"#,
        );
        assert_eq!(lines[0], "log: default: 1");
        assert_eq!(lines[1], "log: default: 2");
        assert_eq!(lines[2], "log: x: 1");
        assert_eq!(lines[3], "log: default: 1");
        assert!(lines[4].starts_with("log: t: "), "{}", lines[4]);
        assert!(lines[4].ends_with("ms mid"), "{}", lines[4]);
        assert!(lines[5].starts_with("log: t: "), "{}", lines[5]);
        // Ending a timer that is not running, and starting one twice, both warn.
        assert_eq!(lines[6], "warn: timer 't' does not exist");
        assert_eq!(lines[7], "warn: timer 't' already exists");
    }

    /// `table` renders rows and columns; dumping the object would have made the
    /// method pointless.
    #[test]
    fn console_table_renders_a_table() {
        let _g = v8_guard();
        let lines = console_lines(r#"console.table([{ a: 1, b: "x" }, { a: 22 }]);"#);
        assert_eq!(
            lines[0],
            "log: ┌─────────┬────┬─────┐\n\
             │ (index) │ a  │ b   │\n\
             ├─────────┼────┼─────┤\n\
             │ 0       │ 1  │ 'x' │\n\
             │ 1       │ 22 │     │\n\
             └─────────┴────┴─────┘"
        );

        // Primitive rows go in a Values column, and an object's keys index it.
        let lines = console_lines(r#"console.table({ r: 1 });"#);
        assert!(lines[0].contains("(key)"), "{}", lines[0]);
        assert!(lines[0].contains("values"), "{}", lines[0]);
    }

    #[test]
    fn console_trace_includes_the_stack() {
        let _g = v8_guard();
        let lines = console_lines(r#"console.trace("why");"#);
        assert!(lines[0].starts_with("error: trace: why\n"), "{}", lines[0]);
        assert!(lines[0].contains("    at "), "{}", lines[0]);
    }

    #[test]
    fn performance_reads_the_clock_provider() {
        let _g = v8_guard();
        let mut rt = runtime_with(
            Arc::new(TestConsole::default()),
            Arc::new(FixedClock {
                monotonic: 1234,
                wall: 5678,
            }),
        );
        assert_eq!(rt.eval("performance.now()").unwrap(), Value::Number(1234.0));
        assert_eq!(
            rt.eval("performance.timeOrigin").unwrap(),
            Value::Number(5678.0)
        );
    }

    /// `navigator.userAgent` reports the crate's own version. The conformance
    /// suite can only check the *shape* of the string; this is what stops the
    /// substituted version drifting from the binary doing the reporting.
    #[test]
    fn navigator_user_agent_carries_the_crate_version() {
        let _g = v8_guard();
        let mut rt = runtime();
        assert_eq!(
            rt.eval("navigator.userAgent").unwrap(),
            Value::String(concat!("ES-Runtime/", env!("CARGO_PKG_VERSION")).into())
        );
        // The placeholder must be substituted everywhere, not just here.
        assert!(
            !prelude::source().contains("__ES_RUNTIME_VERSION__"),
            "an unsubstituted version token is left in the prelude"
        );
    }

    #[test]
    fn queue_microtask_runs_at_the_checkpoint() {
        let _g = v8_guard();
        let mut rt = runtime();
        rt.eval("globalThis.x = 0; queueMicrotask(() => { globalThis.x = 1; });")
            .unwrap();
        // Explicit microtask policy: not run until the tick's checkpoint.
        assert_eq!(rt.eval("globalThis.x").unwrap(), Value::Number(0.0));
        rt.tick(0);
        assert_eq!(rt.eval("globalThis.x").unwrap(), Value::Number(1.0));
    }

    #[test]
    fn self_aliases_global_this() {
        let _g = v8_guard();
        let mut rt = runtime();
        assert_eq!(rt.eval("self === globalThis").unwrap(), Value::Bool(true));
    }

    /// Asserts a JS expression evaluates to `true`.
    fn assert_true(rt: &mut Runtime, expr: &str) {
        assert_eq!(rt.eval(expr).unwrap(), Value::Bool(true), "expr: {expr}");
    }

    /// Runs an async JS `body` (which should `return` a value) to completion by
    /// ticking the microtask loop, then returns the resolved value. A rejection
    /// is returned as a `Value::String` prefixed with `ERR:`.
    ///
    /// A body that never settles is a **panic**, not an `undefined` result: this
    /// used to give up after a fixed tick count and read `__result` regardless,
    /// so work that had not finished was indistinguishable from work that
    /// finished with no value — and a caller's assertion then blamed the
    /// behaviour under test rather than the wait.
    fn eval_async(rt: &mut Runtime, body: &str) -> Value {
        rt.eval(&format!(
            "globalThis.__done = false; globalThis.__result = undefined; \
             (async () => {{ {body} }})().then( \
               (r) => {{ globalThis.__result = r; globalThis.__done = true; }}, \
               (e) => {{ globalThis.__result = 'ERR:' + ((e && e.message) || e); \
                         globalThis.__done = true; }});"
        ))
        .unwrap();
        pump_until(rt, "the async body to settle", |rt| {
            rt.eval("globalThis.__done").unwrap() == Value::Bool(true)
        });
        rt.eval("globalThis.__result").unwrap()
    }

    #[test]
    fn text_encoder_decoder_round_trip() {
        let _g = v8_guard();
        let mut rt = runtime();
        // "héllo😀": é is 2 UTF-8 bytes, 😀 is 4 → 1+2+1+1+1+4 = 10 bytes.
        assert_eq!(
            rt.eval(r#"new TextEncoder().encode("héllo😀").length"#)
                .unwrap(),
            Value::Number(10.0)
        );
        assert_true(
            &mut rt,
            r#"new TextDecoder().decode(new TextEncoder().encode("héllo😀")) === "héllo😀""#,
        );
    }

    #[test]
    fn atob_btoa_round_trip() {
        let _g = v8_guard();
        let mut rt = runtime();
        assert_true(&mut rt, r#"btoa("hello") === "aGVsbG8=""#);
        assert_true(&mut rt, r#"atob("aGVsbG8=") === "hello""#);
        assert_true(
            &mut rt,
            r#"(() => { try { btoa("Ā"); return false; } catch (e) { return e.name === "InvalidCharacterError"; } })()"#,
        );
    }

    #[test]
    fn structured_clone_deep_copies_with_cycles() {
        let _g = v8_guard();
        let mut rt = runtime();
        assert_true(
            &mut rt,
            "(() => { const o = { a: [1, 2], m: new Map([['k', 3]]) }; o.self = o; \
             const c = structuredClone(o); \
             return c !== o && c.a[0] === 1 && c.a !== o.a && c.self === c && c.m.get('k') === 3; })()",
        );
    }

    #[test]
    fn structured_clone_rejects_functions() {
        let _g = v8_guard();
        let mut rt = runtime();
        assert_true(
            &mut rt,
            "(() => { try { structuredClone(() => {}); return false; } \
             catch (e) { return e.name === 'DataCloneError'; } })()",
        );
    }

    #[test]
    fn dom_exception_is_an_error_with_name_and_code() {
        let _g = v8_guard();
        let mut rt = runtime();
        assert_true(
            &mut rt,
            "(() => { const e = new DOMException('x', 'AbortError'); \
             return e instanceof Error && e.name === 'AbortError' && e.message === 'x' \
             && new DOMException('', 'DataCloneError').code === 25; })()",
        );
    }

    #[test]
    fn url_parses_and_exposes_components() {
        let _g = v8_guard();
        let mut rt = runtime();
        assert_true(
            &mut rt,
            "(() => { const u = new URL('https://user:pw@example.com:8080/a/b?x=1&y=2#frag'); \
             return u.protocol === 'https:' && u.hostname === 'example.com' && u.port === '8080' \
             && u.pathname === '/a/b' && u.search === '?x=1&y=2' && u.hash === '#frag' \
             && u.username === 'user' && u.origin === 'https://example.com:8080'; })()",
        );
    }

    #[test]
    fn url_resolves_against_a_base_and_rejects_invalid() {
        let _g = v8_guard();
        let mut rt = runtime();
        assert_true(
            &mut rt,
            "new URL('../c', 'https://example.com/a/b').href === 'https://example.com/c'",
        );
        assert_true(&mut rt, "URL.canParse('https://ok.test/') === true");
        assert_true(&mut rt, "URL.canParse('not a url') === false");
        assert_true(
            &mut rt,
            "(() => { try { new URL('not a url'); return false; } catch (e) { return e instanceof TypeError; } })()",
        );
    }

    /// The host keeps a small cache of parsed URLs so a component setter need not
    /// re-parse the href it was just handed. `href -> Url` is a pure function, so
    /// a hit must be indistinguishable from a miss — this drives enough distinct
    /// URLs through it to force eviction, interleaves two objects so neither
    /// keeps the other's entry warm, and checks every result against the same
    /// value reached by a fresh `new URL()`.
    #[test]
    fn url_setters_agree_whether_or_not_the_parse_was_cached() {
        let _g = v8_guard();
        let mut rt = runtime();
        assert_true(
            &mut rt,
            "(() => { \
               const a = new URL('https://a.test/p?q=1#f'); \
               const b = new URL('http://b.test:81/x'); \
               for (let i = 0; i < 40; i++) { \
                 a.hostname = 'h' + i + '.test'; \
                 b.pathname = '/p' + i; \
                 b.host = 'k' + i + '.test:' + (8000 + i); \
                 a.search = '?n=' + i; \
                 if (a.href !== new URL('https://h' + i + '.test/p?n=' + i + '#f').href) return false; \
                 if (b.href !== new URL('http://k' + i + '.test:' + (8000 + i) + '/p' + i).href) return false; \
                 if (a.origin !== 'https://h' + i + '.test') return false; \
               } \
               return true; })()",
        );
    }

    /// An invalid component assignment is a silent no-op per WHATWG, and must
    /// leave the cache holding the URL it did *not* change — a stale entry here
    /// would make the next setter act on a URL the guest never had.
    #[test]
    fn a_rejected_url_setter_leaves_the_next_one_correct() {
        let _g = v8_guard();
        let mut rt = runtime();
        assert_true(
            &mut rt,
            "(() => { \
               const u = new URL('https://ok.test/a'); \
               u.hostname = 'bad host'; \
               if (u.href !== 'https://ok.test/a') return false; \
               u.port = 'notaport'; \
               if (u.href !== 'https://ok.test/a') return false; \
               u.pathname = '/b'; \
               return u.href === 'https://ok.test/b'; })()",
        );
    }

    #[test]
    fn url_search_params_stay_in_sync() {
        let _g = v8_guard();
        let mut rt = runtime();
        assert_true(
            &mut rt,
            "(() => { const u = new URL('https://h.test/?a=1'); \
             u.searchParams.append('b', '2'); \
             return u.search === '?a=1&b=2' && u.searchParams.get('a') === '1' \
             && u.searchParams.getAll('b').length === 1; })()",
        );
        assert_true(
            &mut rt,
            "new URLSearchParams('x=1&x=2&y=3').getAll('x').join(',') === '1,2'",
        );
    }

    #[test]
    fn event_target_dispatches_to_listeners() {
        let _g = v8_guard();
        let mut rt = runtime();
        assert_true(
            &mut rt,
            "(() => { const t = new EventTarget(); let got = null; \
             t.addEventListener('x', (e) => { got = e.detail; }); \
             t.dispatchEvent(new CustomEvent('x', { detail: 42 })); return got === 42; })()",
        );
        // once: fires at most once.
        assert_true(
            &mut rt,
            "(() => { const t = new EventTarget(); let n = 0; \
             t.addEventListener('x', () => n++, { once: true }); \
             t.dispatchEvent(new Event('x')); t.dispatchEvent(new Event('x')); return n === 1; })()",
        );
        // preventDefault on a cancelable event → dispatchEvent returns false.
        assert_true(
            &mut rt,
            "(() => { const t = new EventTarget(); t.addEventListener('x', (e) => e.preventDefault()); \
             return t.dispatchEvent(new Event('x', { cancelable: true })) === false; })()",
        );
    }

    #[test]
    fn abort_controller_signals_abort() {
        let _g = v8_guard();
        let mut rt = runtime();
        assert_true(
            &mut rt,
            "(() => { const c = new AbortController(); let reason = null; \
             c.signal.addEventListener('abort', () => { reason = c.signal.reason; }); \
             c.abort('stop'); return c.signal.aborted === true && reason === 'stop'; })()",
        );
        // Default reason is an AbortError DOMException.
        assert_true(
            &mut rt,
            "(() => { const c = new AbortController(); c.abort(); \
             return c.signal.reason instanceof DOMException && c.signal.reason.name === 'AbortError'; })()",
        );
        // AbortSignal.any follows the first source to abort.
        assert_true(
            &mut rt,
            "(() => { const a = new AbortController(); const b = new AbortController(); \
             const any = AbortSignal.any([a.signal, b.signal]); let fired = false; \
             any.addEventListener('abort', () => { fired = true; }); \
             b.abort('z'); return any.aborted && any.reason === 'z' && fired; })()",
        );
        // throwIfAborted throws the reason.
        assert_true(
            &mut rt,
            "(() => { try { AbortSignal.abort('e').throwIfAborted(); return false; } \
             catch (err) { return err === 'e'; } })()",
        );
    }

    #[test]
    fn abort_signal_timeout_fires_on_tick() {
        let _g = v8_guard();
        let mut rt = runtime();
        rt.eval(
            "globalThis.timedOut = false; \
             const s = AbortSignal.timeout(50); \
             s.addEventListener('abort', () => { globalThis.timedOut = true; });",
        )
        .unwrap();
        // Not yet due.
        assert_eq!(rt.tick(10).timers_fired, 0);
        assert_eq!(rt.eval("globalThis.timedOut").unwrap(), Value::Bool(false));
        // Past the deadline: the timer fires and aborts the signal.
        assert_eq!(rt.tick(50).timers_fired, 1);
        assert_true(&mut rt, "globalThis.timedOut === true");
        assert_true(
            &mut rt,
            "AbortSignal.timeout(0), true", // smoke: constructor path works with 0
        );
    }

    #[test]
    fn readable_stream_reads_enqueued_chunks() {
        let _g = v8_guard();
        let mut rt = runtime();
        let out = eval_async(
            &mut rt,
            "const rs = new ReadableStream({ start(c) { c.enqueue('a'); c.enqueue('b'); c.close(); } }); \
             const r = rs.getReader(); const got = []; let x; \
             while (!(x = await r.read()).done) got.push(x.value); \
             return got.join(',');",
        );
        assert_eq!(out, Value::String("a,b".into()));
    }

    #[test]
    fn readable_stream_pull_drives_the_source() {
        let _g = v8_guard();
        let mut rt = runtime();
        let out = eval_async(
            &mut rt,
            "let i = 0; \
             const rs = new ReadableStream({ pull(c) { c.enqueue(i++); if (i === 3) c.close(); } }); \
             const r = rs.getReader(); const got = []; let x; \
             while (!(x = await r.read()).done) got.push(x.value); \
             return got.join(',');",
        );
        assert_eq!(out, Value::String("0,1,2".into()));
    }

    #[test]
    fn readable_stream_cancel_calls_the_source() {
        let _g = v8_guard();
        let mut rt = runtime();
        let out = eval_async(
            &mut rt,
            "let cancelled = null; \
             const rs = new ReadableStream({ cancel(reason) { cancelled = reason; } }); \
             await rs.getReader().cancel('stop'); return cancelled;",
        );
        assert_eq!(out, Value::String("stop".into()));
    }

    #[test]
    fn readable_stream_tee_duplicates_chunks() {
        let _g = v8_guard();
        let mut rt = runtime();
        let out = eval_async(
            &mut rt,
            "const rs = new ReadableStream({ start(c) { c.enqueue(1); c.enqueue(2); c.close(); } }); \
             const [a, b] = rs.tee(); \
             const drain = async (s) => { const r = s.getReader(); const o = []; let x; \
               while (!(x = await r.read()).done) o.push(x.value); return o.join(','); }; \
             const [sa, sb] = await Promise.all([drain(a), drain(b)]); \
             return sa + '|' + sb;",
        );
        assert_eq!(out, Value::String("1,2|1,2".into()));
    }

    #[test]
    fn count_queuing_strategy_reports_backpressure() {
        let _g = v8_guard();
        let mut rt = runtime();
        let out = eval_async(
            &mut rt,
            "const rs = new ReadableStream( \
               { start(c) { globalThis.a = c.desiredSize; c.enqueue('x'); globalThis.b = c.desiredSize; } }, \
               new CountQueuingStrategy({ highWaterMark: 2 })); \
             await Promise.resolve(); return globalThis.a + ',' + globalThis.b;",
        );
        assert_eq!(out, Value::String("2,1".into()));
    }

    #[test]
    fn writable_stream_writes_and_closes() {
        let _g = v8_guard();
        let mut rt = runtime();
        let out = eval_async(
            &mut rt,
            "const written = []; \
             const ws = new WritableStream({ write(chunk) { written.push(chunk); } }); \
             const w = ws.getWriter(); \
             await w.write('a'); await w.write('b'); await w.close(); \
             return written.join(',');",
        );
        assert_eq!(out, Value::String("a,b".into()));
    }

    #[test]
    fn transform_stream_maps_chunks() {
        let _g = v8_guard();
        let mut rt = runtime();
        let out = eval_async(
            &mut rt,
            "const ts = new TransformStream({ transform(chunk, c) { c.enqueue(chunk * 2); } }); \
             const w = ts.writable.getWriter(); const r = ts.readable.getReader(); \
             w.write(1); w.write(2); w.close(); \
             const got = []; let x; \
             while (!(x = await r.read()).done) got.push(x.value); \
             return got.join(',');",
        );
        assert_eq!(out, Value::String("2,4".into()));
    }

    #[test]
    fn pipe_to_moves_chunks_to_the_sink() {
        let _g = v8_guard();
        let mut rt = runtime();
        let out = eval_async(
            &mut rt,
            "const src = new ReadableStream({ start(c) { c.enqueue('x'); c.enqueue('y'); c.close(); } }); \
             const sink = []; \
             const dest = new WritableStream({ write(chunk) { sink.push(chunk); } }); \
             await src.pipeTo(dest); return sink.join(',');",
        );
        assert_eq!(out, Value::String("x,y".into()));
    }

    #[test]
    fn pipe_through_a_transform() {
        let _g = v8_guard();
        let mut rt = runtime();
        let out = eval_async(
            &mut rt,
            "const src = new ReadableStream({ start(c) { c.enqueue(1); c.enqueue(2); c.close(); } }); \
             const ts = new TransformStream({ transform(chunk, c) { c.enqueue(chunk + 10); } }); \
             const r = src.pipeThrough(ts).getReader(); \
             const got = []; let x; \
             while (!(x = await r.read()).done) got.push(x.value); \
             return got.join(',');",
        );
        assert_eq!(out, Value::String("11,12".into()));
    }

    #[test]
    fn text_encoder_stream_round_trips() {
        let _g = v8_guard();
        let mut rt = runtime();
        let out = eval_async(
            &mut rt,
            "const tes = new TextEncoderStream(); \
             const w = tes.writable.getWriter(); const r = tes.readable.getReader(); \
             w.write('hé'); w.write('llo'); w.close(); \
             const bytes = []; let x; \
             while (!(x = await r.read()).done) bytes.push(...x.value); \
             return new TextDecoder().decode(new Uint8Array(bytes));",
        );
        assert_eq!(out, Value::String("héllo".into()));
    }

    #[test]
    fn text_decoder_stream_handles_split_multibyte() {
        let _g = v8_guard();
        let mut rt = runtime();
        // "é" is 0xC3 0xA9, split across two chunks.
        let out = eval_async(
            &mut rt,
            "const tds = new TextDecoderStream(); \
             const w = tds.writable.getWriter(); const r = tds.readable.getReader(); \
             w.write(new Uint8Array([0x68, 0xC3])); \
             w.write(new Uint8Array([0xA9, 0x6F])); \
             w.close(); \
             let s = ''; let x; \
             while (!(x = await r.read()).done) s += x.value; \
             return s;",
        );
        assert_eq!(out, Value::String("héo".into()));
    }

    #[test]
    fn headers_are_case_insensitive_and_combine() {
        let _g = v8_guard();
        let mut rt = runtime();
        assert_true(
            &mut rt,
            "(() => { const h = new Headers(); h.append('X-A', '1'); h.append('x-a', '2'); \
             return h.get('X-A') === '1, 2' && h.has('x-a') && !h.has('y'); })()",
        );
    }

    #[test]
    fn blob_concatenates_and_reads() {
        let _g = v8_guard();
        let mut rt = runtime();
        let out = eval_async(
            &mut rt,
            "const b = new Blob(['hello', ' ', 'world'], { type: 'text/plain' }); \
             return b.size + '|' + b.type + '|' + (await b.text());",
        );
        assert_eq!(out, Value::String("11|text/plain|hello world".into()));
    }

    #[test]
    fn form_data_basic_operations() {
        let _g = v8_guard();
        let mut rt = runtime();
        assert_true(
            &mut rt,
            "(() => { const f = new FormData(); f.append('a', '1'); f.append('b', '2'); f.append('a', '3'); \
             return f.get('a') === '1' && f.getAll('a').join(',') === '1,3' && f.has('b'); })()",
        );
    }

    #[test]
    fn fetch_requires_net_capability() {
        let _g = v8_guard();
        let mut rt = runtime_with_net(Arc::new(MockNet::ok("x")));
        // Deny-by-default: no Net capability granted.
        let out = eval_async(&mut rt, "await fetch('https://x.test/'); return 'ok';");
        match out {
            Value::String(s) => assert!(
                s.starts_with("ERR:") && (s.contains("capability") || s.contains("NotAllowed")),
                "expected capability denial, got {s}"
            ),
            other => panic!("expected rejection, got {other:?}"),
        }
    }

    #[test]
    fn fetch_returns_response_with_capability() {
        let _g = v8_guard();
        let mut rt = runtime_with_net(Arc::new(MockNet::ok("hello world")));
        rt.set_capabilities(CapabilitySet::none().with(Capability::Net));
        let out = eval_async(
            &mut rt,
            "const r = await fetch('https://x.test/data'); \
             return r.status + '|' + r.ok + '|' + r.headers.get('content-type') + '|' + (await r.text());",
        );
        assert_eq!(out, Value::String("200|true|text/plain|hello world".into()));
    }

    #[test]
    fn fetch_streams_a_chunked_response_body() {
        let _g = v8_guard();
        let net = MockNet {
            status: 200,
            headers: vec![],
            chunks: vec![b"foo".to_vec(), b"bar".to_vec(), b"baz".to_vec()],
            fail: false,
        };
        let mut rt = runtime_with_net(Arc::new(net));
        rt.set_capabilities(CapabilitySet::none().with(Capability::Net));
        // Drain the response body stream chunk by chunk.
        let out = eval_async(
            &mut rt,
            "const r = await fetch('https://x.test/'); const reader = r.body.getReader(); \
             const dec = new TextDecoder(); let s = ''; let x; \
             while (!(x = await reader.read()).done) s += dec.decode(x.value); \
             return s;",
        );
        assert_eq!(out, Value::String("foobarbaz".into()));
    }

    /// A transport that redirects `/hop/<n>` to `/hop/<n+1>` and answers
    /// anything else `200 "landed"`, following the chain itself exactly as a
    /// real transport does — so the redirect *mode* is what the tests below
    /// vary, not the JS side's interpretation of a canned response.
    struct RedirectNet {
        /// How many hops before the chain lands. `usize::MAX` never lands, to
        /// drive the cap.
        hops: usize,
    }
    impl es_runtime_providers::NetTransport for RedirectNet {
        fn fetch(
            &self,
            request: es_runtime_providers::HttpRequest,
        ) -> es_runtime_providers::BoxFuture<
            std::result::Result<
                es_runtime_providers::HttpResponse,
                es_runtime_providers::ProviderError,
            >,
        > {
            const CAP: usize = 20;
            let follow = request.redirect == es_runtime_providers::RedirectMode::Follow;
            let total = self.hops;
            let mut url = request.url;
            Box::pin(async move {
                // `n` is the hop this URL represents; the start URL is hop 0.
                let mut n = url
                    .rsplit_once("/hop/")
                    .and_then(|(_, n)| n.parse::<usize>().ok())
                    .unwrap_or(0);
                let mut redirected = false;
                let mut followed = 0;
                while n < total {
                    if !follow {
                        // Hand back the redirect itself, unfollowed.
                        let body: es_runtime_providers::ByteStream =
                            Box::pin(futures_util::stream::iter(std::iter::empty()));
                        return Ok(es_runtime_providers::HttpResponse {
                            status: 302,
                            status_text: "Found".into(),
                            url,
                            redirected: false,
                            headers: vec![(
                                "location".into(),
                                format!("https://x.test/hop/{}", n + 1),
                            )],
                            body,
                            trailers: None,
                        });
                    }
                    followed += 1;
                    if followed > CAP {
                        return Err(es_runtime_providers::ProviderError::Coded {
                            code: es_runtime_common::ErrorCode::TooManyRedirects,
                            message: format!("too many redirects (more than {CAP})"),
                        });
                    }
                    n += 1;
                    url = format!("https://x.test/hop/{n}");
                    redirected = true;
                }
                let body: es_runtime_providers::ByteStream = Box::pin(futures_util::stream::iter(
                    std::iter::once(Ok(b"landed".to_vec())),
                ));
                Ok(es_runtime_providers::HttpResponse {
                    status: 200,
                    status_text: "OK".into(),
                    url,
                    redirected,
                    headers: vec![],
                    body,
                    trailers: None,
                })
            })
        }
    }

    fn redirect_runtime(hops: usize) -> Runtime {
        let mut rt = runtime_with_net(Arc::new(RedirectNet { hops }));
        rt.set_capabilities(CapabilitySet::none().with(Capability::Net));
        rt
    }

    #[test]
    fn fetch_follows_redirects_by_default_and_reports_the_final_url() {
        let _g = v8_guard();
        let mut rt = redirect_runtime(2);
        let out = eval_async(
            &mut rt,
            "const r = await fetch('https://x.test/hop/0'); \
             return r.status + '|' + r.redirected + '|' + r.url + '|' + (await r.text());",
        );
        assert_eq!(
            out,
            Value::String("200|true|https://x.test/hop/2|landed".into())
        );
    }

    #[test]
    fn fetch_redirect_manual_returns_the_redirect_response_unfollowed() {
        let _g = v8_guard();
        let mut rt = redirect_runtime(2);
        let out = eval_async(
            &mut rt,
            "const r = await fetch('https://x.test/hop/0', { redirect: 'manual' }); \
             return r.status + '|' + r.redirected + '|' + r.headers.get('location');",
        );
        assert_eq!(out, Value::String("302|false|https://x.test/hop/1".into()));
    }

    #[test]
    fn fetch_redirect_error_rejects_with_a_type_error() {
        let _g = v8_guard();
        let mut rt = redirect_runtime(2);
        let out = eval_async(
            &mut rt,
            "try { await fetch('https://x.test/hop/0', { redirect: 'error' }); return 'no throw'; } \
             catch (e) { return e.constructor.name + '|' + e.message.includes('302'); }",
        );
        assert_eq!(out, Value::String("TypeError|true".into()));
    }

    #[test]
    fn fetch_reports_not_redirected_when_nothing_redirected() {
        let _g = v8_guard();
        let mut rt = redirect_runtime(0);
        let out = eval_async(
            &mut rt,
            "const r = await fetch('https://x.test/hop/0'); return String(r.redirected);",
        );
        assert_eq!(out, Value::String("false".into()));
    }

    #[test]
    fn fetch_rejects_a_redirect_chain_past_the_cap_with_a_stable_code() {
        let _g = v8_guard();
        let mut rt = redirect_runtime(usize::MAX);
        let out = eval_async(
            &mut rt,
            "try { await fetch('https://x.test/hop/0'); return 'no throw'; } \
             catch (e) { return e.code; }",
        );
        assert_eq!(out, Value::String("ERR_TOO_MANY_REDIRECTS".into()));
    }

    #[test]
    fn request_rejects_an_unknown_redirect_mode() {
        let _g = v8_guard();
        let mut rt = redirect_runtime(0);
        let out = eval_async(
            &mut rt,
            "try { new Request('https://x.test/', { redirect: 'manaul' }); return 'no throw'; } \
             catch (e) { return e.constructor.name; }",
        );
        assert_eq!(out, Value::String("TypeError".into()));
    }

    #[test]
    fn a_script_constructed_response_cannot_claim_to_be_redirected() {
        let _g = v8_guard();
        let mut rt = redirect_runtime(0);
        let out = eval_async(
            &mut rt,
            "const r = new Response('x', { redirected: true }); return String(r.redirected);",
        );
        assert_eq!(out, Value::String("false".into()));
    }

    /// A transport whose request future never resolves, and which records how
    /// many times it was polled into existence. Lets an abort test observe that
    /// the *in-flight* request is torn down rather than merely ignored: when the
    /// abort wins the race, `fetch_ops` drops this future.
    struct HangingNet {
        started: Arc<std::sync::atomic::AtomicUsize>,
        dropped: Arc<std::sync::atomic::AtomicUsize>,
    }
    /// Bumps `dropped` when the request future is discarded — i.e. cancelled.
    struct DropProbe(Arc<std::sync::atomic::AtomicUsize>);
    impl Drop for DropProbe {
        fn drop(&mut self) {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }
    impl es_runtime_providers::NetTransport for HangingNet {
        fn fetch(
            &self,
            _request: es_runtime_providers::HttpRequest,
        ) -> es_runtime_providers::BoxFuture<
            std::result::Result<
                es_runtime_providers::HttpResponse,
                es_runtime_providers::ProviderError,
            >,
        > {
            self.started
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let probe = DropProbe(self.dropped.clone());
            Box::pin(async move {
                let _probe = probe;
                std::future::pending().await
            })
        }
    }

    #[test]
    fn object_url_fetch_needs_no_net_capability() {
        let _g = v8_guard();
        // The transport errors on any real request; a blob: URL must never
        // reach it, and must work with Net denied — nothing leaves the isolate.
        let mut rt = runtime_with_net(Arc::new(MockNet::stub()));
        rt.set_capabilities(CapabilitySet::none());
        let out = eval_async(
            &mut rt,
            "const u = URL.createObjectURL(new Blob(['local'], { type: 'text/plain' })); \
             const r = await fetch(u); \
             const body = await r.text(); \
             URL.revokeObjectURL(u); \
             return r.status + '|' + body;",
        );
        assert_eq!(out, Value::String("200|local".into()));
    }

    #[test]
    fn message_port_delivers_asynchronously_and_in_order() {
        let _g = v8_guard();
        let mut rt = runtime();
        // `sync` captures that nothing is delivered synchronously: postMessage
        // queues a task, so the count is still 0 at the point of the send.
        let out = eval_async(
            &mut rt,
            "const ch = new MessageChannel(); \
             const seen = []; \
             ch.port2.onmessage = (e) => seen.push(e.data); \
             ch.port1.postMessage('a'); \
             ch.port1.postMessage('b'); \
             const sync = seen.length; \
             await new Promise((r) => setTimeout(r, 0)); \
             await new Promise((r) => setTimeout(r, 0)); \
             return sync + '|' + seen.join(',');",
        );
        assert_eq!(out, Value::String("0|a,b".into()));
    }

    #[test]
    fn message_port_structured_clones_the_payload() {
        let _g = v8_guard();
        let mut rt = runtime();
        // The receiver must see the value as it was at postMessage time, and
        // must not share identity with the sender's object.
        let out = eval_async(
            &mut rt,
            "const ch = new MessageChannel(); \
             let got = null; \
             ch.port2.onmessage = (e) => { got = e.data; }; \
             const payload = { n: 1, nested: { deep: true } }; \
             ch.port1.postMessage(payload); \
             payload.n = 999; \
             await new Promise((r) => setTimeout(r, 0)); \
             await new Promise((r) => setTimeout(r, 0)); \
             return got.n + '|' + got.nested.deep + '|' + (got === payload);",
        );
        assert_eq!(out, Value::String("1|true|false".into()));
    }

    #[test]
    fn message_port_buffers_until_started() {
        let _g = v8_guard();
        let mut rt = runtime();
        // addEventListener does not start a port; only start() (or assigning
        // onmessage) does. Messages sent before then are buffered, not dropped.
        //
        // Delivery after `start()` is a *task*, not a synchronous flush inside
        // the call — so the turn below is part of the assertion, not padding.
        // (This read `seen` immediately after `start()` while ports were
        // pure-JS objects and the queue was drained inline, which was the
        // shortcut rather than the spec.)
        let out = eval_async(
            &mut rt,
            "const ch = new MessageChannel(); \
             const seen = []; \
             ch.port2.addEventListener('message', (e) => seen.push(e.data)); \
             ch.port1.postMessage('early'); \
             await new Promise((r) => setTimeout(r, 0)); \
             await new Promise((r) => setTimeout(r, 0)); \
             const beforeStart = seen.length; \
             ch.port2.start(); \
             await new Promise((r) => setTimeout(r, 0)); \
             return beforeStart + '|' + seen.join(',');",
        );
        assert_eq!(out, Value::String("0|early".into()));
    }

    #[test]
    fn message_port_close_stops_delivery() {
        let _g = v8_guard();
        let mut rt = runtime();
        let out = eval_async(
            &mut rt,
            "const ch = new MessageChannel(); \
             const seen = []; \
             ch.port2.onmessage = (e) => seen.push(e.data); \
             ch.port1.postMessage('before'); \
             ch.port2.close(); \
             ch.port1.postMessage('after'); \
             await new Promise((r) => setTimeout(r, 0)); \
             await new Promise((r) => setTimeout(r, 0)); \
             return String(seen.length);",
        );
        assert_eq!(out, Value::String("0".into()));
    }

    #[test]
    fn broadcast_channel_reaches_peers_but_not_the_sender() {
        let _g = v8_guard();
        let mut rt = runtime();
        let out = eval_async(
            &mut rt,
            "const a = new BroadcastChannel('room'); \
             const b = new BroadcastChannel('room'); \
             const c = new BroadcastChannel('other'); \
             const seen = []; \
             a.onmessage = () => seen.push('a'); \
             b.onmessage = () => seen.push('b'); \
             c.onmessage = () => seen.push('c'); \
             a.postMessage('hello'); \
             await new Promise((r) => setTimeout(r, 0)); \
             await new Promise((r) => setTimeout(r, 0)); \
             a.close(); b.close(); c.close(); \
             return seen.join(',');",
        );
        // Only the same-named peer, and never the sender itself.
        assert_eq!(out, Value::String("b".into()));
    }

    #[test]
    fn broadcast_channel_close_unsubscribes() {
        let _g = v8_guard();
        let mut rt = runtime();
        let out = eval_async(
            &mut rt,
            "const a = new BroadcastChannel('r2'); \
             const b = new BroadcastChannel('r2'); \
             let n = 0; \
             b.onmessage = () => n++; \
             b.close(); \
             a.postMessage('x'); \
             await new Promise((r) => setTimeout(r, 0)); \
             await new Promise((r) => setTimeout(r, 0)); \
             a.close(); \
             return String(n);",
        );
        assert_eq!(out, Value::String("0".into()));
    }

    #[test]
    fn set_timeout_forwards_trailing_arguments_to_the_callback() {
        let _g = v8_guard();
        let mut rt = runtime();
        let out = eval_async(
            &mut rt,
            "return await new Promise((resolve) => \
               setTimeout((a, b, c) => resolve(`${a}|${b}|${c}`), 0, 1, 'two', null));",
        );
        assert_eq!(out, Value::String("1|two|null".into()));
    }

    #[test]
    fn set_timeout_with_no_extra_arguments_passes_none() {
        let _g = v8_guard();
        let mut rt = runtime();
        let out = eval_async(
            &mut rt,
            "return await new Promise((resolve) => \
               setTimeout((...args) => resolve(args.length), 0));",
        );
        assert_eq!(out, Value::Number(0.0));
    }

    #[test]
    fn set_interval_forwards_its_arguments_on_every_firing() {
        let _g = v8_guard();
        let mut rt = runtime();
        // A repeating timer keeps its arguments across firings, not just the
        // first — they are stored with the timer, not consumed by it.
        let out = eval_async(
            &mut rt,
            "return await new Promise((resolve) => { \
               let seen = ''; \
               const id = setInterval((tag) => { \
                 seen += tag; \
                 if (seen.length === 3) { clearInterval(id); resolve(seen); } \
               }, 0, 'x'); \
             });",
        );
        assert_eq!(out, Value::String("xxx".into()));
    }

    #[test]
    fn fetch_rejects_an_already_aborted_signal_without_touching_the_network() {
        let _g = v8_guard();
        let started = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let net = Arc::new(HangingNet {
            started: started.clone(),
            dropped: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        });
        let mut rt = runtime_with_net(net);
        rt.set_capabilities(CapabilitySet::none().with(Capability::Net));
        let out = eval_async(
            &mut rt,
            "const c = new AbortController(); c.abort('too late'); \
             try { await fetch('https://x.test/', { signal: c.signal }); return 'no-throw'; } \
             catch (e) { return String(e); }",
        );
        assert_eq!(out, Value::String("too late".into()));
        // The transport was never consulted.
        assert_eq!(started.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[test]
    fn fetch_aborts_an_in_flight_request_and_drops_the_transport_future() {
        let _g = v8_guard();
        let started = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let dropped = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let net = Arc::new(HangingNet {
            started: started.clone(),
            dropped: dropped.clone(),
        });
        let mut rt = runtime_with_net(net);
        rt.set_capabilities(CapabilitySet::none().with(Capability::Net));
        // The transport never responds. Draining a few microtask turns lets
        // `fetch` get past body materialization and actually issue the request,
        // so the abort below lands on an in-flight call rather than pre-empting
        // it (which is a different path, covered above).
        let out = eval_async(
            &mut rt,
            "const c = new AbortController(); \
             const p = fetch('https://x.test/', { signal: c.signal }); \
             for (let i = 0; i < 10; i++) await Promise.resolve(); \
             c.abort(new DOMException('gone', 'AbortError')); \
             try { await p; return 'no-throw'; } \
             catch (e) { return e.name + ':' + e.message; }",
        );
        assert_eq!(out, Value::String("AbortError:gone".into()));
        // The guest promise rejects as soon as the signal fires. Host-side
        // teardown lands on a later turn of the loop, when the op future is
        // polled and the abort wins its race, so drive a few more ticks.
        for _ in 0..10 {
            rt.tick(0);
        }
        assert_eq!(started.load(std::sync::atomic::Ordering::SeqCst), 1);
        // The pending transport future was dropped — that is the cancellation.
        assert_eq!(dropped.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn fetch_without_a_signal_still_completes() {
        let _g = v8_guard();
        let mut rt = runtime_with_net(Arc::new(MockNet::ok("fine")));
        rt.set_capabilities(CapabilitySet::none().with(Capability::Net));
        let out = eval_async(
            &mut rt,
            "const r = await fetch('https://x.test/'); return await r.text();",
        );
        assert_eq!(out, Value::String("fine".into()));
    }

    #[test]
    fn fetch_default_signal_is_unaborted_and_does_not_leak_abort_handles() {
        let _g = v8_guard();
        let mut rt = runtime_with_net(Arc::new(MockNet::ok("x")));
        rt.set_capabilities(CapabilitySet::none().with(Capability::Net));
        let out = eval_async(
            &mut rt,
            "const r = new Request('https://x.test/'); \
             const before = r.signal.aborted; \
             const res = await fetch('https://x.test/'); await res.text(); \
             return before + '|' + r.signal.aborted;",
        );
        assert_eq!(out, Value::String("false|false".into()));
    }

    #[test]
    fn fetch_streams_a_request_body_from_a_readable_stream() {
        let _g = v8_guard();
        let net = Arc::new(EchoNet::new());
        let mut rt = runtime_with_net(net.clone());
        rt.set_capabilities(CapabilitySet::none().with(Capability::Net));
        // A ReadableStream body of three chunks streams to the host (not buffered
        // in JS); the EchoNet echoes the concatenation back.
        let out = eval_async(
            &mut rt,
            "const enc = new TextEncoder(); \
             const body = new ReadableStream({ start(c) { \
               c.enqueue(enc.encode('alpha-')); c.enqueue(enc.encode('beta-')); \
               c.enqueue(enc.encode('gamma')); c.close(); } }); \
             const r = await fetch('https://x.test/up', { method: 'POST', body }); \
             return await r.text();",
        );
        assert_eq!(out, Value::String("alpha-beta-gamma".into()));
        assert_eq!(&*net.captured.lock().unwrap(), b"alpha-beta-gamma");
        assert!(
            net.saw_stream.load(std::sync::atomic::Ordering::SeqCst),
            "request body should have arrived as a stream, not buffered"
        );
    }

    #[test]
    fn fetch_buffers_a_string_request_body() {
        let _g = v8_guard();
        let net = Arc::new(EchoNet::new());
        let mut rt = runtime_with_net(net.clone());
        rt.set_capabilities(CapabilitySet::none().with(Capability::Net));
        // A non-stream body still travels buffered (RequestBody::Bytes), so the
        // transport must NOT see a stream.
        let out = eval_async(
            &mut rt,
            "const r = await fetch('https://x.test/up', { method: 'POST', body: 'hello body' }); \
             return await r.text();",
        );
        assert_eq!(out, Value::String("hello body".into()));
        assert_eq!(&*net.captured.lock().unwrap(), b"hello body");
        assert!(
            !net.saw_stream.load(std::sync::atomic::Ordering::SeqCst),
            "a string body must be sent buffered, not streamed"
        );
    }

    #[test]
    fn fetch_streams_an_empty_request_body() {
        let _g = v8_guard();
        let net = Arc::new(EchoNet::new());
        let mut rt = runtime_with_net(net.clone());
        rt.set_capabilities(CapabilitySet::none().with(Capability::Net));
        // A ReadableStream that closes immediately: still the streaming path, but
        // zero bytes uploaded.
        let out = eval_async(
            &mut rt,
            "const body = new ReadableStream({ start(c) { c.close(); } }); \
             const r = await fetch('https://x.test/up', { method: 'POST', body }); \
             return (await r.text()).length;",
        );
        assert_eq!(out, Value::Number(0.0));
        assert!(net.captured.lock().unwrap().is_empty());
        assert!(net.saw_stream.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn fetch_aborts_when_the_request_body_stream_errors() {
        let _g = v8_guard();
        let net = Arc::new(EchoNet::new());
        let mut rt = runtime_with_net(net.clone());
        rt.set_capabilities(CapabilitySet::none().with(Capability::Net));
        // A body stream that errors mid-flight: the error is forwarded to the
        // host, which aborts the request, and `fetch` rejects.
        let out = eval_async(
            &mut rt,
            "const enc = new TextEncoder(); let n = 0; \
             const body = new ReadableStream({ pull(c) { \
               if (n++ === 0) c.enqueue(enc.encode('partial')); \
               else c.error(new Error('boom')); } }); \
             const r = await fetch('https://x.test/up', { method: 'POST', body }); \
             return await r.text();",
        );
        match out {
            Value::String(s) => assert!(
                s.starts_with("ERR:") && s.contains("boom"),
                "expected the body-stream error to reject fetch, got {s}"
            ),
            other => panic!("expected rejection, got {other:?}"),
        }
    }

    #[test]
    fn fetch_streaming_request_body_requires_net_capability() {
        let _g = v8_guard();
        let net = Arc::new(EchoNet::new());
        let mut rt = runtime_with_net(net);
        // No Net capability: allocating the body stream channel must itself be
        // gated, so the streaming path is denied just like the buffered one.
        let out = eval_async(
            &mut rt,
            "const body = new ReadableStream({ start(c) { c.close(); } }); \
             await fetch('https://x.test/up', { method: 'POST', body }); return 'ok';",
        );
        match out {
            Value::String(s) => assert!(
                s.starts_with("ERR:") && (s.contains("capability") || s.contains("NotAllowed")),
                "expected capability denial, got {s}"
            ),
            other => panic!("expected rejection, got {other:?}"),
        }
    }

    /// Resident set size in KiB (Linux), for the soak/leak test. Elsewhere 0, so
    /// the growth bound is trivially satisfied (the registry-drain assertion is
    /// the portable leak guard).
    #[cfg(target_os = "linux")]
    fn resident_kib() -> u64 {
        let statm = std::fs::read_to_string("/proc/self/statm").unwrap_or_default();
        let pages: u64 = statm
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        pages * 4 // 4 KiB pages
    }
    #[cfg(not(target_os = "linux"))]
    fn resident_kib() -> u64 {
        0
    }

    /// Soak: hammer the streaming-request-body path and prove it neither leaks
    /// (the three body registries drain to zero each time, and RSS stays bounded)
    /// nor deadlocks/corrupts over many iterations. Opt-in:
    ///   cargo test -p es-runtime -- --ignored soak
    #[test]
    #[ignore = "soak/leak: run with `cargo test -p es-runtime -- --ignored soak_streaming_fetch`"]
    fn soak_streaming_fetch_does_not_leak() {
        let _g = v8_guard();
        let net = Arc::new(EchoNet::new());
        let mut rt = runtime_with_net(net.clone());
        rt.set_capabilities(CapabilitySet::none().with(Capability::Net));

        const ITERS: usize = 20_000;
        // Warm up the heap/allocator before sampling so we measure steady-state
        // growth, not first-touch.
        let script = "const enc = new TextEncoder(); \
             const body = new ReadableStream({ start(c) { \
               c.enqueue(enc.encode('alpha-')); c.enqueue(enc.encode('beta-')); \
               c.enqueue(enc.encode('gamma')); c.close(); } }); \
             const r = await fetch('https://x.test/up', { method: 'POST', body }); \
             return await r.text();";
        for _ in 0..500 {
            assert_eq!(
                eval_async(&mut rt, script),
                Value::String("alpha-beta-gamma".into())
            );
        }

        let mut rss_mid = 0u64;
        for k in 0..ITERS {
            let out = eval_async(&mut rt, script);
            assert_eq!(out, Value::String("alpha-beta-gamma".into()), "iter {k}");
            // The precise leak guard: the three body registries must be empty
            // between requests — no leaked response body stream, request sender,
            // or receiver. This holds regardless of V8 heap behavior.
            let inflight = rt.eval("globalThis.__ops.__fetch_inflight()").unwrap();
            assert_eq!(
                inflight,
                Value::Array(vec![
                    Value::Number(0.0),
                    Value::Number(0.0),
                    Value::Number(0.0)
                ]),
                "registry not drained at iter {k}"
            );
            // Sample resident set at the halfway mark, so the comparison below
            // measures *steady-state* growth (the V8 heap/code-cache warm-up is
            // a one-time cost paid in the first half).
            if k == ITERS / 2 {
                rss_mid = resident_kib();
            }
        }

        let rss_after = resident_kib();
        let second_half = rss_after.saturating_sub(rss_mid);
        // A per-iteration native leak would keep growing linearly into the second
        // half; a warmed V8 heap plateaus. Require the second-half growth to stay
        // small (the registry-drain assertion above is the precise guard).
        assert!(
            second_half < 16 * 1024,
            "RSS grew {second_half} KiB over the second {} streaming fetches — possible leak",
            ITERS / 2
        );
    }

    /// A scripted WebSocketProvider for the `WebSocket` global (DECISIONS D29):
    /// `connect` hands back a fixed protocol; `recv` replays a pre-seeded frame
    /// sequence (each future Ready on first poll, so no waker is needed under the
    /// tick driver); `send`/`close` are no-ops.
    struct MockWs {
        inbound: std::sync::Mutex<std::collections::VecDeque<es_runtime_providers::WsIncoming>>,
        protocol: String,
        /// Every `serve()` this provider was asked for, in order. What the
        /// prelude sent is the thing under test for the server options — the
        /// bind itself is the provider's business.
        served: std::sync::Mutex<Vec<es_runtime_providers::WsServeOptions>>,
        /// Connection ids `accept` still has to hand out before reporting the
        /// server closed. Lets a test get real `WebSocketConnection` objects,
        /// which is what `broadcast` takes.
        pending_accepts: std::sync::Mutex<std::collections::VecDeque<u64>>,
        /// The id lists `broadcast` was called with, in order.
        broadcasts: std::sync::Mutex<Vec<Vec<u64>>>,
    }
    impl MockWs {
        fn new(frames: Vec<es_runtime_providers::WsIncoming>, protocol: &str) -> Self {
            MockWs {
                inbound: std::sync::Mutex::new(frames.into()),
                protocol: protocol.to_string(),
                served: std::sync::Mutex::new(Vec::new()),
                pending_accepts: std::sync::Mutex::new(std::collections::VecDeque::new()),
                broadcasts: std::sync::Mutex::new(Vec::new()),
            }
        }

        /// Queues `n` connections for `accept` to yield before it reports the
        /// server closed.
        fn accepting(self, n: u64) -> Self {
            *self.pending_accepts.lock().unwrap() = (1..=n).collect();
            self
        }
    }
    impl es_runtime_providers::WebSocketProvider for MockWs {
        fn connect(
            &self,
            _url: String,
            _protocols: Vec<String>,
        ) -> es_runtime_providers::BoxFuture<
            std::result::Result<
                (u64, es_runtime_providers::WebSocketInfo),
                es_runtime_providers::ProviderError,
            >,
        > {
            let protocol = self.protocol.clone();
            Box::pin(async move {
                Ok((
                    1u64,
                    es_runtime_providers::WebSocketInfo {
                        protocol,
                        extensions: String::new(),
                    },
                ))
            })
        }
        fn send(
            &self,
            _id: u64,
            _message: es_runtime_providers::WsMessage,
        ) -> es_runtime_providers::BoxFuture<
            std::result::Result<(), es_runtime_providers::ProviderError>,
        > {
            Box::pin(async { Ok(()) })
        }
        fn broadcast(
            &self,
            ids: Vec<u64>,
            _message: es_runtime_providers::WsMessage,
        ) -> es_runtime_providers::BoxFuture<
            std::result::Result<(), es_runtime_providers::ProviderError>,
        > {
            self.broadcasts.lock().unwrap().push(ids);
            Box::pin(async { Ok(()) })
        }
        fn recv(
            &self,
            _id: u64,
        ) -> es_runtime_providers::BoxFuture<
            std::result::Result<
                Option<es_runtime_providers::WsIncoming>,
                es_runtime_providers::ProviderError,
            >,
        > {
            let item = self.inbound.lock().unwrap().pop_front();
            Box::pin(async move { Ok(item) })
        }
        fn close(
            &self,
            _id: u64,
            _code: Option<u16>,
            _reason: String,
        ) -> es_runtime_providers::BoxFuture<
            std::result::Result<(), es_runtime_providers::ProviderError>,
        > {
            Box::pin(async { Ok(()) })
        }
        fn serve(
            &self,
            options: es_runtime_providers::WsServeOptions,
        ) -> es_runtime_providers::BoxFuture<
            std::result::Result<
                (u64, es_runtime_providers::SocketInfo),
                es_runtime_providers::ProviderError,
            >,
        > {
            let port = options.port;
            self.served.lock().unwrap().push(options);
            Box::pin(async move {
                Ok((
                    1u64,
                    es_runtime_providers::SocketInfo {
                        local_address: "127.0.0.1".to_string(),
                        local_port: port,
                        ..Default::default()
                    },
                ))
            })
        }
        fn accept(
            &self,
            _id: u64,
        ) -> es_runtime_providers::BoxFuture<
            std::result::Result<
                Option<(u64, es_runtime_providers::WebSocketInfo)>,
                es_runtime_providers::ProviderError,
            >,
        > {
            let next = self.pending_accepts.lock().unwrap().pop_front();
            let protocol = self.protocol.clone();
            Box::pin(async move {
                Ok(next.map(|id| {
                    (
                        id,
                        es_runtime_providers::WebSocketInfo {
                            protocol,
                            extensions: String::new(),
                        },
                    )
                }))
            })
        }
        fn close_server(
            &self,
            _id: u64,
        ) -> es_runtime_providers::BoxFuture<
            std::result::Result<(), es_runtime_providers::ProviderError>,
        > {
            Box::pin(async { Ok(()) })
        }
    }

    fn runtime_with_ws(ws: Arc<dyn es_runtime_providers::WebSocketProvider>) -> Runtime {
        let engine = V8Engine::new(Limits::default()).expect("engine");
        Runtime::new(
            Box::new(engine),
            HostProviders::new(
                Arc::new(FixedClock {
                    monotonic: 0,
                    wall: 0,
                }),
                Arc::new(TestConsole::default()),
                Arc::new(MockNet::stub()),
                Arc::new(TestEntropy::new()),
            )
            .with_web_socket(ws),
        )
        .expect("runtime")
    }

    #[test]
    fn websocket_open_message_close_round_trip() {
        use es_runtime_providers::WsIncoming;
        let _g = v8_guard();
        let ws = MockWs::new(
            vec![
                WsIncoming::Text("hello".into()),
                WsIncoming::Binary(vec![1, 2, 3]),
                WsIncoming::Close {
                    code: 1000,
                    reason: "bye".into(),
                },
            ],
            "chat",
        );
        let mut rt = runtime_with_ws(Arc::new(ws));
        rt.set_capabilities(CapabilitySet::none().with(Capability::Net));
        let out = eval_async(
            &mut rt,
            "const log = []; \
             const ws = new WebSocket('ws://echo.test/sub', 'chat'); \
             ws.binaryType = 'arraybuffer'; \
             await new Promise((resolve) => { \
               ws.addEventListener('open', () => { log.push('open:' + ws.readyState + ':' + ws.protocol); ws.send('ping'); }); \
               ws.addEventListener('message', (e) => { \
                 if (typeof e.data === 'string') log.push('txt:' + e.data + ':' + e.origin); \
                 else log.push('bin:' + new Uint8Array(e.data).join(',')); \
               }); \
               ws.addEventListener('close', (e) => { log.push('close:' + e.code + ':' + e.reason + ':' + e.wasClean + ':' + ws.readyState); resolve(); }); \
               ws.addEventListener('error', () => { log.push('error'); resolve(); }); \
             }); \
             return log.join('|');",
        );
        assert_eq!(
            out,
            Value::String(
                "open:1:chat|txt:hello:ws://echo.test|bin:1,2,3|close:1000:bye:true:3".into()
            )
        );
    }

    #[test]
    fn websocket_validates_url_state_and_close_args() {
        let _g = v8_guard();
        let mut rt = runtime_with_ws(Arc::new(MockWs::new(vec![], "")));
        rt.set_capabilities(CapabilitySet::none().with(Capability::Net));
        // All synchronous (no tick): the constructor's connect stays pending.
        assert_true(
            &mut rt,
            "(() => { const errs = []; \
             try { new WebSocket('http://x/'); } catch (e) { errs.push(e.name); } \
             try { new WebSocket('ws://x/#f'); } catch (e) { errs.push(e.name); } \
             try { new WebSocket('ws://x/', ['a', 'a']); } catch (e) { errs.push(e.name); } \
             const ws = new WebSocket('ws://x/'); \
             try { ws.send('x'); } catch (e) { errs.push(e.name); } \
             try { ws.close(1234); } catch (e) { errs.push(e.name); } \
             try { ws.close(1000, 'x'.repeat(200)); } catch (e) { errs.push(e.name); } \
             return errs.join(',') === \
               'SyntaxError,SyntaxError,SyntaxError,InvalidStateError,InvalidAccessError,SyntaxError'; })()",
        );
    }

    #[test]
    fn websocket_connect_requires_net_capability() {
        let _g = v8_guard();
        // Provider present, but Net is denied by default — the op gate fails the
        // connect, surfacing as a non-clean close (1006).
        let mut rt = runtime_with_ws(Arc::new(MockWs::new(vec![], "")));
        let out = eval_async(
            &mut rt,
            "const ws = new WebSocket('ws://x/'); \
             return await new Promise((resolve) => { \
               ws.addEventListener('close', (e) => resolve(e.code + ':' + e.wasClean + ':' + ws.readyState)); \
             });",
        );
        assert_eq!(out, Value::String("1006:false:3".into()));
    }

    #[test]
    fn websocketstream_round_trip_with_backpressured_reads() {
        use es_runtime_providers::WsIncoming;
        let _g = v8_guard();
        let ws = MockWs::new(
            vec![
                WsIncoming::Text("hello".into()),
                WsIncoming::Binary(vec![1, 2, 3]),
                WsIncoming::Close {
                    code: 1000,
                    reason: "bye".into(),
                },
            ],
            "chat",
        );
        let mut rt = runtime_with_ws(Arc::new(ws));
        rt.set_capabilities(CapabilitySet::none().with(Capability::Net));
        let out = eval_async(
            &mut rt,
            "const wss = new WebSocketStream('ws://echo.test/', { protocols: ['chat'] }); \
             const { readable, writable, protocol } = await wss.opened; \
             const log = ['open:' + protocol]; \
             const writer = writable.getWriter(); \
             await writer.write('ping'); \
             await writer.write(new Uint8Array([9])); \
             writer.releaseLock(); \
             const reader = readable.getReader(); \
             for (;;) { \
               const { value, done } = await reader.read(); \
               if (done) { log.push('done'); break; } \
               log.push(typeof value === 'string' ? 'txt:' + value : 'bin:' + Array.from(value).join(',')); \
             } \
             const c = await wss.closed; \
             log.push('close:' + c.closeCode + ':' + c.reason); \
             return log.join('|');",
        );
        assert_eq!(
            out,
            Value::String("open:chat|txt:hello|bin:1,2,3|done|close:1000:bye".into())
        );
    }

    #[test]
    fn websocketstream_local_close_settles_closed_via_drain() {
        use es_runtime_providers::WsIncoming;
        let _g = v8_guard();
        // The guest closes without reading `readable`; the internal drain must
        // still observe the peer's close frame and settle `closed`.
        let ws = MockWs::new(
            vec![WsIncoming::Close {
                code: 1000,
                reason: "ok".into(),
            }],
            "",
        );
        let mut rt = runtime_with_ws(Arc::new(ws));
        rt.set_capabilities(CapabilitySet::none().with(Capability::Net));
        let out = eval_async(
            &mut rt,
            "const wss = new WebSocketStream('ws://x/'); \
             await wss.opened; \
             wss.close({ closeCode: 1000, reason: 'done' }); \
             const c = await wss.closed; \
             return c.closeCode + ':' + c.reason;",
        );
        assert_eq!(out, Value::String("1000:ok".into()));
    }

    #[test]
    fn websocketstream_abnormal_close_errors_reads_and_closed() {
        let _g = v8_guard();
        // No inbound frames: the first recv returns null (abnormal close), so a
        // read and the `closed` promise both reject with a WebSocketError.
        let mut rt = runtime_with_ws(Arc::new(MockWs::new(vec![], "")));
        rt.set_capabilities(CapabilitySet::none().with(Capability::Net));
        let out = eval_async(
            &mut rt,
            "const wss = new WebSocketStream('ws://x/'); \
             const { readable } = await wss.opened; \
             const log = []; \
             try { await readable.getReader().read(); log.push('read-ok'); } \
             catch (e) { log.push('read:' + e.name); } \
             try { await wss.closed; } \
             catch (e) { log.push('closed:' + e.name + ':' + (e instanceof WebSocketError) + ':' + (e instanceof DOMException)); } \
             return log.join('|');",
        );
        assert_eq!(
            out,
            Value::String("read:WebSocketError|closed:WebSocketError:true:true".into())
        );
    }

    #[test]
    fn websocketstream_validates_url_close_args_and_wserror_init() {
        let _g = v8_guard();
        let mut rt = runtime_with_ws(Arc::new(MockWs::new(vec![], "")));
        rt.set_capabilities(CapabilitySet::none().with(Capability::Net));
        assert_true(
            &mut rt,
            "(() => { const errs = []; \
             try { new WebSocketStream('http://x/'); } catch (e) { errs.push(e.name); } \
             const wss = new WebSocketStream('ws://x/'); \
             try { wss.close({ closeCode: 1234 }); } catch (e) { errs.push(e.name); } \
             try { wss.close({ reason: 'x'.repeat(200) }); } catch (e) { errs.push(e.name); } \
             try { new WebSocketError('m', { closeCode: 1234 }); } catch (e) { errs.push(e.name); } \
             const we = new WebSocketError('m', { reason: 'r' }); \
             errs.push(we.name + ':' + we.closeCode + ':' + we.reason); \
             return errs.join(',') === \
               'SyntaxError,InvalidAccessError,SyntaxError,InvalidAccessError,WebSocketError:1000:r'; })()",
        );
    }

    #[test]
    fn websocketstream_connect_requires_net_capability() {
        let _g = v8_guard();
        // Provider present, Net denied: `opened` and `closed` both reject.
        let mut rt = runtime_with_ws(Arc::new(MockWs::new(vec![], "")));
        let out = eval_async(
            &mut rt,
            "const wss = new WebSocketStream('ws://x/'); \
             const log = []; \
             try { await wss.opened; } catch (e) { log.push('opened:' + e.name); } \
             try { await wss.closed; } catch (e) { log.push('closed:' + e.name); } \
             return log.join('|');",
        );
        assert_eq!(
            out,
            Value::String("opened:WebSocketError|closed:WebSocketError".into())
        );
    }

    /// A guest that says nothing about a WebSocket server's posture gets the
    /// provider's defaults. The prelude must send "unset" rather than a copy of
    /// the number, so this asserts what arrives equals `WsTimeouts::default()`
    /// rather than any literal written here — one copy of the value, on the
    /// Rust side, is what keeps the two from drifting.
    #[test]
    fn ws_serve_without_options_uses_the_provider_defaults() {
        let _g = v8_guard();
        let ws = Arc::new(MockWs::new(vec![], ""));
        let mut rt = runtime_with_ws(ws.clone());
        run_module(
            &mut rt,
            "import { serve } from 'runtime:websocket'; \
             const s = serve({ port: 4001 }); \
             globalThis.result = (await s.addr).port;",
            MapLoader::new(&[]),
        );
        let served = ws.served.lock().unwrap();
        assert_eq!(
            served[0].timeouts,
            es_runtime_providers::WsTimeouts::default()
        );
        assert_eq!(served[0].max_connections, None);
        assert_eq!(served[0].max_connections_per_ip, None);
        // The one bound that is *on* unless the guest turns it off — a queue
        // nobody bounds is memory a peer can spend, and the right number here
        // does not depend on anything the deployment knows.
        assert_eq!(
            served[0].max_buffered_amount,
            Some(es_runtime_providers::WsServeOptions::DEFAULT_MAX_BUFFERED_AMOUNT)
        );
    }

    /// `0` is the guest turning the send-queue bound off, the same spelling the
    /// timeouts use. Reading it as "unset" would hand back the default, which
    /// is the opposite of what was asked.
    #[test]
    fn a_zero_buffer_bound_turns_it_off_rather_than_defaulting() {
        let _g = v8_guard();
        let ws = Arc::new(MockWs::new(vec![], ""));
        let mut rt = runtime_with_ws(ws.clone());
        run_module(
            &mut rt,
            "import { serve } from 'runtime:websocket'; \
             const a = serve({ port: 4001, maxBufferedAmount: 0 }); \
             const b = serve({ port: 4002, maxBufferedAmount: 1024 }); \
             globalThis.result = (await a.addr).port + (await b.addr).port;",
            MapLoader::new(&[]),
        );
        let served = ws.served.lock().unwrap();
        assert_eq!(served[0].max_buffered_amount, None);
        assert_eq!(served[1].max_buffered_amount, Some(1024));
    }

    /// The two knobs cross as they are written: milliseconds for the timeout,
    /// and an explicit `null` meaning "off" rather than "use the default".
    #[test]
    fn ws_serve_options_cross_as_written() {
        let _g = v8_guard();
        let ws = Arc::new(MockWs::new(vec![], ""));
        let mut rt = runtime_with_ws(ws.clone());
        run_module(
            &mut rt,
            "import { serve } from 'runtime:websocket'; \
             const a = serve({ port: 4001, timeouts: { handshake: 1500 }, maxConnections: 64 }); \
             const b = serve({ port: 4002, timeouts: { handshake: null } }); \
             globalThis.result = (await a.addr).port + (await b.addr).port;",
            MapLoader::new(&[]),
        );
        let served = ws.served.lock().unwrap();
        assert_eq!(
            served[0].timeouts.handshake,
            Some(std::time::Duration::from_millis(1500))
        );
        assert_eq!(served[0].max_connections, Some(64));
        assert_eq!(
            served[1].timeouts.handshake, None,
            "an explicit null must disable the timeout, not fall back to the default",
        );
    }

    /// `broadcast` skipped anything it did not recognize, so
    /// `broadcast([...room, undefined], msg)` delivered to the rest and said
    /// nothing, and a list that was entirely the wrong type broadcast to nobody
    /// and still returned normally — the failure mode where a chat room goes
    /// quiet and every call looks like it worked.
    ///
    /// The connection id is set in the constructor and never removed, so its
    /// absence is a brand check rather than a liveness one: a *closed*
    /// connection still has one and is still passed to the host, which owns the
    /// live socket table. Only something that was never a connection throws.
    #[test]
    fn ws_broadcast_refuses_an_element_that_is_not_a_connection() {
        let _g = v8_guard();
        let ws = Arc::new(MockWs::new(vec![], "").accepting(2));
        let mut rt = runtime_with_ws(ws.clone());
        run_module(
            &mut rt,
            "import { serve, broadcast } from 'runtime:websocket'; \
             const s = serve({ port: 4001 }); \
             const conns = []; \
             for await (const c of s) conns.push(c); \
             const names = []; \
             for (const bad of [null, undefined, {}, 42, 'conn']) { \
               try { broadcast([...conns, bad], 'x'); names.push('no throw'); } \
               catch (e) { names.push(e.constructor.name); } \
             } \
             globalThis.result = conns.length + ':' + names.join(',');",
            MapLoader::new(&[]),
        );
        assert_eq!(
            rt.eval("globalThis.result").unwrap(),
            Value::String("2:TypeError,TypeError,TypeError,TypeError,TypeError".into()),
        );
        assert!(
            ws.broadcasts.lock().unwrap().is_empty(),
            "a bad element must fail the whole call, not half-deliver it",
        );
    }

    /// The other half: the check must not cost valid connections their
    /// broadcast, which is the regression the brand test could have introduced.
    #[test]
    fn ws_broadcast_still_reaches_every_valid_connection() {
        let _g = v8_guard();
        let ws = Arc::new(MockWs::new(vec![], "").accepting(3));
        let mut rt = runtime_with_ws(ws.clone());
        run_module(
            &mut rt,
            "import { serve, broadcast } from 'runtime:websocket'; \
             const s = serve({ port: 4001 }); \
             const conns = []; \
             for await (const c of s) conns.push(c); \
             broadcast(conns, 'hello'); \
             broadcast([], 'to nobody'); \
             globalThis.result = conns.length;",
            MapLoader::new(&[]),
        );
        assert_eq!(rt.eval("globalThis.result").unwrap(), Value::Number(3.0));
        let sent = ws.broadcasts.lock().unwrap();
        assert_eq!(
            sent.len(),
            1,
            "an empty list is a no-op, not a host crossing: {sent:?}"
        );
        assert_eq!(
            sent[0].len(),
            3,
            "every accepted connection must be sent to"
        );
    }

    /// Rejected at the call rather than after the bind: a server that is
    /// listening with a policy the guest did not ask for is worse than an error.
    #[test]
    fn ws_serve_rejects_unusable_options_before_the_port_is_bound() {
        let _g = v8_guard();
        let ws = Arc::new(MockWs::new(vec![], ""));
        let mut rt = runtime_with_ws(ws.clone());
        run_module(
            &mut rt,
            "import { serve } from 'runtime:websocket'; \
             const names = []; \
             for (const bad of ['lots', 0, -1, 1.5]) { \
               try { serve({ port: 4001, maxConnections: bad }); } \
               catch (e) { names.push(e.constructor.name); } \
               try { serve({ port: 4001, maxConnectionsPerIp: bad }); } \
               catch (e) { names.push(e.constructor.name); } \
             } \
             for (const bad of ['lots', -1, 1.5]) { \
               try { serve({ port: 4001, maxBufferedAmount: bad }); } \
               catch (e) { names.push(e.constructor.name); } \
             } \
             for (const bad of ['soon', -1, Infinity]) { \
               try { serve({ port: 4001, timeouts: { handshake: bad } }); } \
               catch (e) { names.push(e.constructor.name); } \
             } \
             globalThis.result = names.join(',');",
            MapLoader::new(&[]),
        );
        assert!(
            ws.served.lock().unwrap().is_empty(),
            "no bind may be attempted for an option that was rejected",
        );
    }

    #[test]
    fn compression_round_trips_all_formats_across_chunks() {
        let _g = v8_guard();
        let mut rt = runtime();
        // ~64 KiB of compressible text written in 1 KiB chunks, through a
        // CompressionStream piped into a DecompressionStream, per format.
        let out = eval_async(
            &mut rt,
            "const text = 'the quick brown fox jumps over the lazy dog. '.repeat(1500); \
             const bytes = new TextEncoder().encode(text); \
             const results = []; \
             for (const format of ['brotli', 'gzip', 'deflate', 'deflate-raw']) { \
               const pipeline = new ReadableStream({ \
                 start(c) { \
                   for (let i = 0; i < bytes.length; i += 1024) c.enqueue(bytes.slice(i, i + 1024)); \
                   c.close(); \
                 }, \
               }) \
                 .pipeThrough(new CompressionStream(format)) \
                 .pipeThrough(new DecompressionStream(format)); \
               let size = 0; \
               const parts = []; \
               for await (const chunk of pipeline) { parts.push(chunk); size += chunk.length; } \
               const merged = new Uint8Array(size); \
               let at = 0; \
               for (const p of parts) { merged.set(p, at); at += p.length; } \
               results.push(format + ':' + (new TextDecoder().decode(merged) === text)); \
             } \
             return results.join('|');",
        );
        assert_eq!(
            out,
            Value::String("brotli:true|gzip:true|deflate:true|deflate-raw:true".into())
        );
    }

    #[test]
    fn decompression_decodes_known_gzip_and_brotli_vectors() {
        let _g = v8_guard();
        let mut rt = runtime();
        // gzip: `printf 'hello world' | gzip -n | base64`; brotli: Node's
        // `zlib.brotliCompressSync('hello world')` (Google's C encoder) —
        // real-encoder interop for both.
        let out = eval_async(
            &mut rt,
            "const vectors = [ \
               ['gzip', 'H4sIAAAAAAAAA8tIzcnJVyjPL8pJAQCFEUoNCwAAAA=='], \
               ['brotli', 'CwWAaGVsbG8gd29ybGQD'], \
             ]; \
             const results = []; \
             for (const [format, b64] of vectors) { \
               const bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0)); \
               const ds = new DecompressionStream(format); \
               const writer = ds.writable.getWriter(); \
               writer.write(bytes); \
               writer.close(); \
               const parts = []; \
               for await (const chunk of ds.readable) parts.push(...chunk); \
               results.push(format + ':' + new TextDecoder().decode(new Uint8Array(parts))); \
             } \
             return results.join('|');",
        );
        assert_eq!(
            out,
            Value::String("gzip:hello world|brotli:hello world".into())
        );
    }

    #[test]
    fn compression_stream_validates_format_and_chunk_type() {
        let _g = v8_guard();
        let mut rt = runtime();
        let out = eval_async(
            &mut rt,
            "const log = []; \
             try { new CompressionStream('br'); } catch (e) { log.push('c:' + e.name); } \
             try { new DecompressionStream('zip'); } catch (e) { log.push('d:' + e.name); } \
             try { new CompressionStream(); } catch (e) { log.push('none:' + e.name); } \
             const cs = new CompressionStream('gzip'); \
             const w = cs.writable.getWriter(); \
             try { await w.write('not bytes'); } catch (e) { log.push('chunk:' + e.name); } \
             return log.join('|');",
        );
        assert_eq!(
            out,
            Value::String("c:TypeError|d:TypeError|none:TypeError|chunk:TypeError".into())
        );
    }

    #[test]
    fn decompression_errors_on_corrupt_and_truncated_input() {
        let _g = v8_guard();
        let mut rt = runtime();
        let out = eval_async(
            &mut rt,
            "const log = []; \
             { const ds = new DecompressionStream('gzip'); \
               const w = ds.writable.getWriter(); \
               const reads = ds.readable.getReader().read().catch((e) => log.push('corrupt-read:' + e.name)); \
               try { await w.write(new Uint8Array([1, 2, 3, 4, 5, 6, 7, 8])); await w.close(); } \
               catch (e) { log.push('corrupt:' + e.name); } \
               await reads; } \
             { const ds = new DecompressionStream('deflate'); \
               const w = ds.writable.getWriter(); \
               ds.readable.getReader().read().catch(() => {}); \
               try { await w.write(new Uint8Array([0x78, 0x9c])); await w.close(); } \
               catch (e) { log.push('truncated:' + e.name); } } \
             { const ds = new DecompressionStream('brotli'); \
               const w = ds.writable.getWriter(); \
               ds.readable.getReader().read().catch(() => {}); \
               try { await w.write(new Uint8Array([0x0b, 0x05])); await w.close(); } \
               catch (e) { log.push('br-truncated:' + e.name); } } \
             return log.join('|');",
        );
        assert_eq!(
            out,
            Value::String(
                "corrupt-read:TypeError|corrupt:TypeError|truncated:TypeError|br-truncated:TypeError"
                    .into()
            )
        );
    }

    #[test]
    fn transform_stream_cancel_hook_runs_on_abort_and_cancel() {
        let _g = v8_guard();
        let mut rt = runtime();
        // The transformer.cancel hook (Streams spec) fires exactly once on a
        // writable abort or a readable cancel — CompressionStream relies on it
        // to free its native context.
        let out = eval_async(
            &mut rt,
            "const log = []; \
             { const ts = new TransformStream({ cancel(r) { log.push('abort:' + r); } }); \
               await ts.writable.abort('w'); } \
             { const ts = new TransformStream({ cancel(r) { log.push('cancel:' + r); } }); \
               await ts.readable.cancel('r'); } \
             return log.join('|');",
        );
        assert_eq!(out, Value::String("abort:w|cancel:r".into()));
    }

    #[test]
    fn error_codes_capability_denied_then_provider_unavailable() {
        let _g = v8_guard();
        // Stable guest-facing codes (SPEC §6 Phase 13). The capability gate
        // runs before the handler, so with nothing granted the denial code
        // surfaces; with FileRead granted, the missing-provider code does.
        let mut rt = runtime();
        let out = eval_async(
            &mut rt,
            "try { await __ops.fs_read('/x'); return 'no-throw'; } \
             catch (e) { return e.code + ':' + e.name; }",
        );
        assert_eq!(
            out,
            Value::String("ERR_CAPABILITY_DENIED:NotAllowedError".into())
        );

        let mut rt = runtime();
        rt.set_capabilities(CapabilitySet::none().with(Capability::FileRead));
        let out = eval_async(
            &mut rt,
            "try { await __ops.fs_read('/x'); return 'no-throw'; } \
             catch (e) { return e.code + ':' + e.name; }",
        );
        assert_eq!(out, Value::String("ERR_PROVIDER_UNAVAILABLE:Error".into()));
    }

    #[test]
    fn get_random_values_fills_in_place() {
        let _g = v8_guard();
        let mut rt = runtime();
        assert_true(
            &mut rt,
            "(() => { const a = new Uint8Array(16); const r = crypto.getRandomValues(a); \
             return r === a && a.some((x) => x !== 0); })()",
        );
    }

    #[test]
    fn random_uuid_is_well_formed_v4() {
        let _g = v8_guard();
        let mut rt = runtime();
        assert_true(
            &mut rt,
            "/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(crypto.randomUUID())",
        );
    }

    #[test]
    fn subtle_digest_matches_known_sha256_vector() {
        let _g = v8_guard();
        let mut rt = runtime();
        // SHA-256("abc").
        let out = eval_async(
            &mut rt,
            "const h = await crypto.subtle.digest('SHA-256', new TextEncoder().encode('abc')); \
             return [...new Uint8Array(h)].map((b) => b.toString(16).padStart(2, '0')).join('');",
        );
        assert_eq!(
            out,
            Value::String(
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".into()
            )
        );
    }

    #[test]
    fn subtle_hmac_signs_and_verifies() {
        let _g = v8_guard();
        let mut rt = runtime();
        let out = eval_async(
            &mut rt,
            "const enc = new TextEncoder(); \
             const key = await crypto.subtle.importKey('raw', enc.encode('secret'), \
               { name: 'HMAC', hash: 'SHA-256' }, false, ['sign', 'verify']); \
             const sig = await crypto.subtle.sign('HMAC', key, enc.encode('message')); \
             const good = await crypto.subtle.verify('HMAC', key, sig, enc.encode('message')); \
             const bad = await crypto.subtle.verify('HMAC', key, sig, enc.encode('tampered')); \
             return good === true && bad === false;",
        );
        assert_eq!(out, Value::Bool(true));
    }

    #[test]
    fn subtle_aes_gcm_round_trips() {
        let _g = v8_guard();
        let mut rt = runtime();
        let out = eval_async(
            &mut rt,
            "const key = await crypto.subtle.generateKey({ name: 'AES-GCM', length: 256 }, true, ['encrypt', 'decrypt']); \
             const iv = crypto.getRandomValues(new Uint8Array(12)); \
             const pt = new TextEncoder().encode('hello gcm'); \
             const ct = await crypto.subtle.encrypt({ name: 'AES-GCM', iv }, key, pt); \
             const out = await crypto.subtle.decrypt({ name: 'AES-GCM', iv }, key, ct); \
             return new TextDecoder().decode(out);",
        );
        assert_eq!(out, Value::String("hello gcm".into()));
    }

    #[test]
    fn subtle_aes_gcm_rejects_tampered_ciphertext() {
        let _g = v8_guard();
        let mut rt = runtime();
        let out = eval_async(
            &mut rt,
            "const key = await crypto.subtle.generateKey({ name: 'AES-GCM', length: 128 }, true, ['encrypt', 'decrypt']); \
             const iv = crypto.getRandomValues(new Uint8Array(12)); \
             const ct = new Uint8Array(await crypto.subtle.encrypt({ name: 'AES-GCM', iv }, key, new TextEncoder().encode('data'))); \
             ct[0] ^= 0xff; \
             await crypto.subtle.decrypt({ name: 'AES-GCM', iv }, key, ct); return 'no-error';",
        );
        match out {
            Value::String(s) => assert!(
                s.starts_with("ERR:") && (s.contains("OperationError") || s.contains("decryption")),
                "expected OperationError, got {s}"
            ),
            other => panic!("expected rejection, got {other:?}"),
        }
    }

    #[test]
    fn subtle_aes_cbc_round_trips() {
        let _g = v8_guard();
        let mut rt = runtime();
        let out = eval_async(
            &mut rt,
            "const key = await crypto.subtle.generateKey({ name: 'AES-CBC', length: 256 }, true, ['encrypt', 'decrypt']); \
             const iv = crypto.getRandomValues(new Uint8Array(16)); \
             const pt = new TextEncoder().encode('hello cbc, longer than one block'); \
             const ct = await crypto.subtle.encrypt({ name: 'AES-CBC', iv }, key, pt); \
             const out = await crypto.subtle.decrypt({ name: 'AES-CBC', iv }, key, ct); \
             return new TextDecoder().decode(out);",
        );
        assert_eq!(
            out,
            Value::String("hello cbc, longer than one block".into())
        );
    }

    #[test]
    fn subtle_aes_cbc_known_answer() {
        // FIPS-197 / NIST SP 800-38A AES-128-CBC, first block of F.2.1.
        let _g = v8_guard();
        let mut rt = runtime();
        let out = eval_async(
            &mut rt,
            "const hex = (s) => Uint8Array.from(s.match(/../g).map((b) => parseInt(b, 16))); \
             const toHex = (a) => [...new Uint8Array(a)].map((b) => b.toString(16).padStart(2, '0')).join(''); \
             const key = await crypto.subtle.importKey('raw', hex('2b7e151628aed2a6abf7158809cf4f3c'), 'AES-CBC', false, ['encrypt']); \
             const iv = hex('000102030405060708090a0b0c0d0e0f'); \
             const ct = await crypto.subtle.encrypt({ name: 'AES-CBC', iv }, key, hex('6bc1bee22e409f96e93d7e117393172a')); \
             return toHex(ct).slice(0, 32);",
        );
        // Expected first ciphertext block for that vector.
        assert_eq!(
            out,
            Value::String("7649abac8119b246cee98e9b12e9197d".into())
        );
    }

    /// RFC 3394 §4.1: wrap 128 bits of key data with a 128-bit KEK. A published
    /// vector is the only way to know the wrap is AES-KW and not merely
    /// something that round-trips with itself.
    #[test]
    fn subtle_aes_kw_rfc3394_vector() {
        let _g = v8_guard();
        let mut rt = runtime();
        let out = eval_async(
            &mut rt,
            "const hex = (s) => Uint8Array.from(s.match(/../g).map((b) => parseInt(b, 16))); \
             const toHex = (a) => [...new Uint8Array(a)].map((b) => b.toString(16).padStart(2, '0')).join(''); \
             const kek = await crypto.subtle.importKey('raw', hex('000102030405060708090a0b0c0d0e0f'), 'AES-KW', false, ['wrapKey', 'unwrapKey']); \
             const target = await crypto.subtle.importKey('raw', hex('00112233445566778899aabbccddeeff'), { name: 'AES-CBC' }, true, ['encrypt']); \
             const wrapped = await crypto.subtle.wrapKey('raw', target, kek, 'AES-KW'); \
             const back = await crypto.subtle.unwrapKey('raw', wrapped, kek, 'AES-KW', { name: 'AES-CBC' }, true, ['encrypt']); \
             const raw = await crypto.subtle.exportKey('raw', back); \
             return toHex(wrapped) + '|' + toHex(raw);",
        );
        assert_eq!(
            out,
            Value::String(
                "1fa68b0a8112b447aef34bd8fb5a7b829d3e862371d2cfe5\
                 |00112233445566778899aabbccddeeff"
                    .into()
            )
        );
    }

    /// The integrity check is the point of AES-KW: a flipped bit must fail the
    /// unwrap rather than hand back wrong key material.
    #[test]
    fn subtle_aes_kw_rejects_tampered_ciphertext() {
        let _g = v8_guard();
        let mut rt = runtime();
        let out = eval_async(
            &mut rt,
            "const kek = await crypto.subtle.generateKey({ name: 'AES-KW', length: 256 }, true, ['wrapKey', 'unwrapKey']); \
             const target = await crypto.subtle.generateKey({ name: 'AES-GCM', length: 128 }, true, ['encrypt']); \
             const wrapped = new Uint8Array(await crypto.subtle.wrapKey('raw', target, kek, 'AES-KW')); \
             wrapped[3] ^= 1; \
             try { \
               await crypto.subtle.unwrapKey('raw', wrapped, kek, 'AES-KW', { name: 'AES-GCM' }, true, ['encrypt']); \
               return 'accepted'; \
             } catch (e) { return e.name; }",
        );
        assert_eq!(out, Value::String("OperationError".into()));
    }

    #[test]
    fn subtle_aes_ctr_round_trips() {
        let _g = v8_guard();
        let mut rt = runtime();
        let out = eval_async(
            &mut rt,
            "const key = await crypto.subtle.generateKey({ name: 'AES-CTR', length: 128 }, true, ['encrypt', 'decrypt']); \
             const counter = crypto.getRandomValues(new Uint8Array(16)); \
             const pt = new TextEncoder().encode('hello ctr'); \
             const ct = await crypto.subtle.encrypt({ name: 'AES-CTR', counter, length: 64 }, key, pt); \
             const out = await crypto.subtle.decrypt({ name: 'AES-CTR', counter, length: 64 }, key, ct); \
             return new TextDecoder().decode(out);",
        );
        assert_eq!(out, Value::String("hello ctr".into()));
    }

    #[test]
    fn subtle_aes_ctr_known_answer() {
        // NIST SP 800-38A F.5.1 CTR-AES128.Encrypt, first block (128-bit counter).
        let _g = v8_guard();
        let mut rt = runtime();
        let out = eval_async(
            &mut rt,
            "const hex = (s) => Uint8Array.from(s.match(/../g).map((b) => parseInt(b, 16))); \
             const toHex = (a) => [...new Uint8Array(a)].map((b) => b.toString(16).padStart(2, '0')).join(''); \
             const key = await crypto.subtle.importKey('raw', hex('2b7e151628aed2a6abf7158809cf4f3c'), 'AES-CTR', false, ['encrypt']); \
             const counter = hex('f0f1f2f3f4f5f6f7f8f9fafbfcfdfeff'); \
             const ct = await crypto.subtle.encrypt({ name: 'AES-CTR', counter, length: 128 }, key, hex('6bc1bee22e409f96e93d7e117393172a')); \
             return toHex(ct);",
        );
        assert_eq!(
            out,
            Value::String("874d6191b620e3261bef6864990db6ce".into())
        );
    }

    #[test]
    fn subtle_hkdf_rfc5869_test_case_1() {
        // RFC 5869 Appendix A.1 (SHA-256).
        let _g = v8_guard();
        let mut rt = runtime();
        let out = eval_async(
            &mut rt,
            "const hex = (s) => Uint8Array.from(s.match(/../g).map((b) => parseInt(b, 16))); \
             const toHex = (a) => [...new Uint8Array(a)].map((b) => b.toString(16).padStart(2, '0')).join(''); \
             const ikm = new Uint8Array(22).fill(0x0b); \
             const key = await crypto.subtle.importKey('raw', ikm, 'HKDF', false, ['deriveBits']); \
             const bits = await crypto.subtle.deriveBits({ name: 'HKDF', hash: 'SHA-256', salt: hex('000102030405060708090a0b0c'), info: hex('f0f1f2f3f4f5f6f7f8f9') }, key, 42 * 8); \
             return toHex(bits);",
        );
        assert_eq!(
            out,
            Value::String(
                "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865"
                    .into()
            )
        );
    }

    #[test]
    fn subtle_pbkdf2_rfc6070_vector() {
        // RFC 6070 PBKDF2-HMAC-SHA1, P="password" S="salt" c=1 dkLen=20.
        let _g = v8_guard();
        let mut rt = runtime();
        let out = eval_async(
            &mut rt,
            "const toHex = (a) => [...new Uint8Array(a)].map((b) => b.toString(16).padStart(2, '0')).join(''); \
             const enc = new TextEncoder(); \
             const key = await crypto.subtle.importKey('raw', enc.encode('password'), 'PBKDF2', false, ['deriveBits']); \
             const bits = await crypto.subtle.deriveBits({ name: 'PBKDF2', hash: 'SHA-1', salt: enc.encode('salt'), iterations: 1 }, key, 20 * 8); \
             return toHex(bits);",
        );
        assert_eq!(
            out,
            Value::String("0c60c80f961f0e71f3a9b524af6012062fe037a6".into())
        );
    }

    #[test]
    fn subtle_derive_key_then_aes_gcm_round_trips() {
        // deriveKey: PBKDF2 → AES-GCM key, used end-to-end.
        let _g = v8_guard();
        let mut rt = runtime();
        let out = eval_async(
            &mut rt,
            "const enc = new TextEncoder(); \
             const base = await crypto.subtle.importKey('raw', enc.encode('correct horse'), 'PBKDF2', false, ['deriveKey']); \
             const key = await crypto.subtle.deriveKey({ name: 'PBKDF2', hash: 'SHA-256', salt: enc.encode('battery'), iterations: 200 }, base, { name: 'AES-GCM', length: 256 }, false, ['encrypt', 'decrypt']); \
             const iv = crypto.getRandomValues(new Uint8Array(12)); \
             const ct = await crypto.subtle.encrypt({ name: 'AES-GCM', iv }, key, enc.encode('staple')); \
             const pt = await crypto.subtle.decrypt({ name: 'AES-GCM', iv }, key, ct); \
             return new TextDecoder().decode(pt);",
        );
        assert_eq!(out, Value::String("staple".into()));
    }

    #[test]
    fn subtle_ecdsa_p256_sign_verify_round_trips() {
        let _g = v8_guard();
        let mut rt = runtime();
        let out = eval_async(
            &mut rt,
            "const enc = new TextEncoder(); \
             const kp = await crypto.subtle.generateKey({ name: 'ECDSA', namedCurve: 'P-256' }, true, ['sign', 'verify']); \
             const data = enc.encode('sign me'); \
             const sig = await crypto.subtle.sign({ name: 'ECDSA', hash: 'SHA-256' }, kp.privateKey, data); \
             const good = await crypto.subtle.verify({ name: 'ECDSA', hash: 'SHA-256' }, kp.publicKey, sig, data); \
             const bad = await crypto.subtle.verify({ name: 'ECDSA', hash: 'SHA-256' }, kp.publicKey, sig, enc.encode('tampered')); \
             return `${good}:${bad}`;",
        );
        assert_eq!(out, Value::String("true:false".into()));
    }

    #[test]
    fn subtle_ecdsa_p521_sha512_round_trips() {
        // Exercises the divergent P-521 signing path (entropy-routed nonce).
        let _g = v8_guard();
        let mut rt = runtime();
        let out = eval_async(
            &mut rt,
            "const enc = new TextEncoder(); \
             const kp = await crypto.subtle.generateKey({ name: 'ECDSA', namedCurve: 'P-521' }, true, ['sign', 'verify']); \
             const data = enc.encode('p521'); \
             const sig = await crypto.subtle.sign({ name: 'ECDSA', hash: 'SHA-512' }, kp.privateKey, data); \
             return String(await crypto.subtle.verify({ name: 'ECDSA', hash: 'SHA-512' }, kp.publicKey, sig, data));",
        );
        assert_eq!(out, Value::String("true".into()));
    }

    #[test]
    fn subtle_ec_key_export_import_all_formats_round_trip() {
        // Export the keys to every format, re-import, and confirm a signature
        // from the original private key verifies under each re-imported public.
        let _g = v8_guard();
        let mut rt = runtime();
        let out = eval_async(
            &mut rt,
            "const enc = new TextEncoder(); const data = enc.encode('formats'); \
             const algo = { name: 'ECDSA', namedCurve: 'P-384' }; \
             const kp = await crypto.subtle.generateKey(algo, true, ['sign', 'verify']); \
             const sig = await crypto.subtle.sign({ name: 'ECDSA', hash: 'SHA-384' }, kp.privateKey, data); \
             const pkcs8 = await crypto.subtle.exportKey('pkcs8', kp.privateKey); \
             const spki = await crypto.subtle.exportKey('spki', kp.publicKey); \
             const raw = await crypto.subtle.exportKey('raw', kp.publicKey); \
             const jwkPub = await crypto.subtle.exportKey('jwk', kp.publicKey); \
             const jwkPriv = await crypto.subtle.exportKey('jwk', kp.privateKey); \
             const priv2 = await crypto.subtle.importKey('pkcs8', pkcs8, algo, true, ['sign']); \
             const sig2 = await crypto.subtle.sign({ name: 'ECDSA', hash: 'SHA-384' }, priv2, data); \
             const fromSpki = await crypto.subtle.importKey('spki', spki, algo, true, ['verify']); \
             const fromRaw = await crypto.subtle.importKey('raw', raw, algo, true, ['verify']); \
             const fromJwk = await crypto.subtle.importKey('jwk', jwkPub, algo, true, ['verify']); \
             const fromJwkPriv = await crypto.subtle.importKey('jwk', jwkPriv, algo, true, ['sign']); \
             const sig3 = await crypto.subtle.sign({ name: 'ECDSA', hash: 'SHA-384' }, fromJwkPriv, data); \
             const v = (k, s) => crypto.subtle.verify({ name: 'ECDSA', hash: 'SHA-384' }, k, s, data); \
             const results = [await v(fromSpki, sig), await v(fromRaw, sig), await v(fromJwk, sig2), await v(fromSpki, sig3)]; \
             return results.every((r) => r === true) ? 'all-ok' : 'mismatch';",
        );
        assert_eq!(out, Value::String("all-ok".into()));
    }

    #[test]
    fn subtle_ecdh_agreement_is_symmetric() {
        // Both parties derive the same shared secret (P-256).
        let _g = v8_guard();
        let mut rt = runtime();
        let out = eval_async(
            &mut rt,
            "const algo = { name: 'ECDH', namedCurve: 'P-256' }; \
             const a = await crypto.subtle.generateKey(algo, true, ['deriveBits']); \
             const b = await crypto.subtle.generateKey(algo, true, ['deriveBits']); \
             const toHex = (buf) => [...new Uint8Array(buf)].map((x) => x.toString(16).padStart(2, '0')).join(''); \
             const ab = await crypto.subtle.deriveBits({ name: 'ECDH', public: b.publicKey }, a.privateKey, 256); \
             const ba = await crypto.subtle.deriveBits({ name: 'ECDH', public: a.publicKey }, b.privateKey, 256); \
             return toHex(ab) === toHex(ba) ? 'agree' : 'disagree';",
        );
        assert_eq!(out, Value::String("agree".into()));
    }

    #[test]
    fn subtle_ecdh_derive_key_then_aes_gcm_round_trips() {
        // ECDH deriveKey → AES-GCM, used end-to-end between two parties.
        let _g = v8_guard();
        let mut rt = runtime();
        let out = eval_async(
            &mut rt,
            "const enc = new TextEncoder(); \
             const algo = { name: 'ECDH', namedCurve: 'P-256' }; \
             const a = await crypto.subtle.generateKey(algo, true, ['deriveKey']); \
             const b = await crypto.subtle.generateKey(algo, true, ['deriveKey']); \
             const keyA = await crypto.subtle.deriveKey({ name: 'ECDH', public: b.publicKey }, a.privateKey, { name: 'AES-GCM', length: 256 }, false, ['encrypt']); \
             const keyB = await crypto.subtle.deriveKey({ name: 'ECDH', public: a.publicKey }, b.privateKey, { name: 'AES-GCM', length: 256 }, false, ['decrypt']); \
             const iv = crypto.getRandomValues(new Uint8Array(12)); \
             const ct = await crypto.subtle.encrypt({ name: 'AES-GCM', iv }, keyA, enc.encode('shared')); \
             const pt = await crypto.subtle.decrypt({ name: 'AES-GCM', iv }, keyB, ct); \
             return new TextDecoder().decode(pt);",
        );
        assert_eq!(out, Value::String("shared".into()));
    }

    #[test]
    fn subtle_rsa_all_schemes_and_formats() {
        // One 2048-bit key generation (the expensive step) reused across every
        // scheme and key format: PKCS1-v1_5 + PSS sign/verify, OAEP round-trip,
        // and SPKI/PKCS8/JWK export→import.
        let _g = v8_guard();
        let mut rt = runtime();
        let out = eval_async(
            &mut rt,
            "const enc = new TextEncoder(); const data = enc.encode('rsa payload'); \
             const kp = await crypto.subtle.generateKey({ name: 'RSASSA-PKCS1-v1_5', modulusLength: 2048, publicExponent: new Uint8Array([1, 0, 1]), hash: 'SHA-256' }, true, ['sign', 'verify']); \
             const sig = await crypto.subtle.sign('RSASSA-PKCS1-v1_5', kp.privateKey, data); \
             const good = await crypto.subtle.verify('RSASSA-PKCS1-v1_5', kp.publicKey, sig, data); \
             const bad = await crypto.subtle.verify('RSASSA-PKCS1-v1_5', kp.publicKey, sig, enc.encode('tampered')); \
             const pkcs8 = await crypto.subtle.exportKey('pkcs8', kp.privateKey); \
             const spki = await crypto.subtle.exportKey('spki', kp.publicKey); \
             const pssPriv = await crypto.subtle.importKey('pkcs8', pkcs8, { name: 'RSA-PSS', hash: 'SHA-256' }, false, ['sign']); \
             const pssPub = await crypto.subtle.importKey('spki', spki, { name: 'RSA-PSS', hash: 'SHA-256' }, true, ['verify']); \
             const pssSig = await crypto.subtle.sign({ name: 'RSA-PSS', saltLength: 32 }, pssPriv, data); \
             const pssOk = await crypto.subtle.verify({ name: 'RSA-PSS', saltLength: 32 }, pssPub, pssSig, data); \
             const oaepPriv = await crypto.subtle.importKey('pkcs8', pkcs8, { name: 'RSA-OAEP', hash: 'SHA-256' }, false, ['decrypt']); \
             const oaepPub = await crypto.subtle.importKey('spki', spki, { name: 'RSA-OAEP', hash: 'SHA-256' }, true, ['encrypt']); \
             const ct = await crypto.subtle.encrypt({ name: 'RSA-OAEP' }, oaepPub, enc.encode('secret')); \
             const pt = new TextDecoder().decode(await crypto.subtle.decrypt({ name: 'RSA-OAEP' }, oaepPriv, ct)); \
             const ctL = await crypto.subtle.encrypt({ name: 'RSA-OAEP', label: enc.encode('ctx') }, oaepPub, enc.encode('labeled')); \
             const ptL = new TextDecoder().decode(await crypto.subtle.decrypt({ name: 'RSA-OAEP', label: enc.encode('ctx') }, oaepPriv, ctL)); \
             const jwkPriv = await crypto.subtle.exportKey('jwk', kp.privateKey); \
             const jwkPub = await crypto.subtle.exportKey('jwk', kp.publicKey); \
             const fromJwkPriv = await crypto.subtle.importKey('jwk', jwkPriv, { name: 'RSASSA-PKCS1-v1_5', hash: 'SHA-256' }, true, ['sign']); \
             const fromJwkPub = await crypto.subtle.importKey('jwk', jwkPub, { name: 'RSASSA-PKCS1-v1_5', hash: 'SHA-256' }, true, ['verify']); \
             const jwkSig = await crypto.subtle.sign('RSASSA-PKCS1-v1_5', fromJwkPriv, data); \
             const jwkOk = await crypto.subtle.verify('RSASSA-PKCS1-v1_5', fromJwkPub, jwkSig, data); \
             return [good === true, bad === false, pssOk === true, pt === 'secret', ptL === 'labeled', jwkOk === true].every(Boolean) ? 'all-ok' : 'mismatch';",
        );
        assert_eq!(out, Value::String("all-ok".into()));
    }

    #[test]
    fn capability_gate_survives_js_tampering() {
        // The security boundary is in Rust (OpState owns the op table + the
        // capability set), so guest JS cannot tamper its way past a gate
        // (SPEC §4 intrinsic integrity).
        let _g = v8_guard();
        let mut rt = runtime();
        rt.register_op(
            OpDecl::sync("guarded", |_args| Ok(Value::Bool(true))).requires(Capability::Net),
        )
        .unwrap();
        // No Net capability granted. Guest attempts to subvert the gate.
        let out = eval_async(
            &mut rt,
            "try { globalThis.__ops = { __fake: true }; } catch (e) {} \
             const reassigned = globalThis.__ops.__fake === true; \
             Object.prototype.granted = true; \
             globalThis.fetch = () => 'pwned'; \
             let denied = false; \
             try { __ops.guarded(); } catch (e) { denied = e instanceof Error; } \
             return `reassigned=${reassigned} denied=${denied}`;",
        );
        assert_eq!(out, Value::String("reassigned=false denied=true".into()));
    }

    #[test]
    fn op_table_binding_is_locked() {
        let _g = v8_guard();
        let mut rt = runtime();
        let out = rt
            .eval(
                "const before = globalThis.__ops; \
                 let redefThrew = false; \
                 try { Object.defineProperty(globalThis, '__ops', { value: {} }); } \
                 catch (e) { redefThrew = true; } \
                 const same = globalThis.__ops === before; \
                 const hidden = !Object.keys(globalThis).includes('__ops'); \
                 `same=${same} redefThrew=${redefThrew} hidden=${hidden}`",
            )
            .unwrap();
        assert_eq!(
            out,
            Value::String("same=true redefThrew=true hidden=true".into())
        );
    }

    #[test]
    fn op_dispatch_survives_prototype_pollution() {
        // Op dispatch + marshaling run in Rust, so polluting the JS primordials
        // cannot derail a host op call.
        let _g = v8_guard();
        let mut rt = runtime();
        rt.register_op(OpDecl::sync("ping", |_args| Ok(Value::Number(7.0))))
            .unwrap();
        let out = rt
            .eval(
                "Array.prototype.push = function () { throw new Error('polluted'); }; \
                 Object.prototype.evil = 1; \
                 __ops.ping();",
            )
            .unwrap();
        assert_eq!(out, Value::Number(7.0));
    }

    /// The in-JS harness the suite is written against — `test`/`todo`, the
    /// `assert*` helpers, and the `__results` tally.
    ///
    /// Read from the same file `conformance/run.js` loads, so the CI gate and
    /// the `esrun` runner cannot drift apart.
    const CONFORMANCE_HARNESS: &str = include_str!("../conformance/harness.js");

    /// Suite files skipped when collecting `conformance/*.js`: the harness
    /// itself (loading it again would reset the tally mid-run) and the `esrun`
    /// runner (a module, and it would recurse).
    const CONFORMANCE_NON_SUITE: &[&str] = &["harness.js", "run.js"];

    /// Runs every `conformance/*.js` spec-assertion file and records the
    /// pass-rate (SPEC §5 / §8). Gated on zero failures and a non-regressing
    /// count; the recorded snapshot lives in `conformance/RESULTS.md`.
    #[test]
    #[allow(clippy::print_stdout)] // reports the pass-rate under `--nocapture`
    fn conformance_suite_passes() {
        let _g = v8_guard();
        // An in-memory filesystem and a full capability set, so files exercising
        // `runtime:fs` are gated here rather than needing a real disk (and
        // leaving artifacts in the working tree).
        let engine = V8Engine::new(Limits::default()).expect("engine");
        let mut rt = Runtime::new(
            Box::new(engine),
            test_providers().with_file_system(Arc::new(MemoryFs::default())),
        )
        .expect("runtime");
        rt.set_capabilities(CapabilitySet::all());
        rt.eval(CONFORMANCE_HARNESS).expect("conformance harness");

        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/conformance");
        let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
            .expect("read conformance dir")
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "js"))
            .filter(|p| {
                !p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| CONFORMANCE_NON_SUITE.contains(&n))
            })
            .collect();
        files.sort();
        assert!(!files.is_empty(), "no conformance files found in {dir}");

        for path in &files {
            let src = std::fs::read_to_string(path).expect("read conformance file");
            rt.eval(&src)
                .unwrap_or_else(|e| panic!("loading {}: {e}", path.display()));
        }

        // Settle the async tests, then read the tallies.
        //
        // Several of them `await import("runtime:…")`. The engine raises that as
        // a *pending dynamic import* — host work the runtime resolves in the
        // async step, not a microtask — so ticking alone can never settle it.
        // That is why four files (every `runtime:serialization` assertion, plus
        // the `runtime:fs` pipeline) used to contribute nothing at all: the old
        // wait gave up after a fixed tick count and read the tallies anyway, so
        // they were silently uncounted rather than failing.
        rt.eval(
            "globalThis.__settled = false; \
             globalThis.__await_all().then(() => { globalThis.__settled = true; }, \
                                           () => { globalThis.__settled = true; });",
        )
        .expect("start settling the async assertions");
        pump_until(&mut rt, "the async conformance assertions", |rt| {
            block_on(rt.process_dynamic_imports()).expect("process dynamic imports");
            rt.eval("globalThis.__settled").unwrap() == Value::Bool(true)
        });

        let number = |rt: &mut Runtime, expr: &str| match rt.eval(expr).unwrap() {
            Value::Number(n) => n as u32,
            other => panic!("{expr} was not a number: {other:?}"),
        };
        let pass = number(&mut rt, "__results.pass");
        let fail = number(&mut rt, "__results.fail");
        let todo = number(&mut rt, "__results.todo");
        let failures = match rt.eval("__results.failures.join('\\n')").unwrap() {
            Value::String(s) => s,
            _ => String::new(),
        };
        let fixed = match rt.eval("__results.fixed.join('\\n')").unwrap() {
            Value::String(s) => s,
            _ => String::new(),
        };

        assert_eq!(fail, 0, "conformance failures ({fail}):\n{failures}");
        // A `todo` that passes means the deviation is fixed: promote it to
        // `test` (and bump BASELINE) so the behaviour is locked in.
        assert!(
            fixed.is_empty(),
            "these `todo` cases now pass — promote them to `test`:\n{fixed}"
        );
        // Non-regression floor; bump alongside conformance/RESULTS.md as the
        // suite grows so removed/skipped assertions are caught.
        const BASELINE: u32 = 373;

        assert!(
            pass >= BASELINE,
            "conformance pass count {pass} below baseline {BASELINE}"
        );
        println!(
            "conformance: {pass}/{} assertions passing across {} files \
             ({todo} known deviations)",
            pass + fail,
            files.len()
        );
    }

    // ----- ES module loading -------------------------------------------------

    /// Drives a synchronous future (the mock loader never truly pends) to its
    /// result, so the async `load_module_source` can be used from sync tests.
    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        use std::task::{Context, Poll};
        let waker = std::task::Waker::noop();
        let mut cx = Context::from_waker(waker);
        let mut future = std::pin::pin!(future);
        for _ in 0..10_000 {
            if let Poll::Ready(value) = future.as_mut().poll(&mut cx) {
                return value;
            }
        }
        panic!("future did not complete — the mock loader should be synchronous");
    }

    /// An in-memory [`ModuleLoader`]: resolves `file://` URLs the way
    /// `FsModuleLoader` does but serves sources from a map, so graph-walking
    /// (resolution, dedup, cycles) is exercised without touching disk.
    struct MapLoader {
        base: url::Url,
        files: std::collections::HashMap<String, String>,
    }
    impl MapLoader {
        // Returns a trait object (not Self) deliberately — tests pass it straight
        // to the Arc-taking module APIs.
        #[allow(clippy::new_ret_no_self)]
        fn new(files: &[(&str, &str)]) -> Arc<dyn ModuleLoader> {
            let base = url::Url::parse("file:///app/").unwrap();
            let files = files
                .iter()
                .map(|(spec, src)| (base.join(spec).unwrap().to_string(), src.to_string()))
                .collect();
            Arc::new(MapLoader { base, files })
        }
    }
    impl ModuleLoader for MapLoader {
        fn resolve(
            &self,
            specifier: &str,
            referrer: &str,
        ) -> es_runtime_providers::BoxFuture<
            std::result::Result<String, es_runtime_providers::ProviderError>,
        > {
            let base = self.base.clone();
            let specifier = specifier.to_string();
            let referrer = referrer.to_string();
            Box::pin(async move {
                let base = if referrer.is_empty() {
                    base
                } else {
                    url::Url::parse(&referrer)
                        .map_err(|e| es_runtime_providers::ProviderError::Other(e.to_string()))?
                };
                base.join(&specifier)
                    .map(|u| u.to_string())
                    .map_err(|e| es_runtime_providers::ProviderError::Other(e.to_string()))
            })
        }
        fn load(
            &self,
            specifier: &str,
        ) -> es_runtime_providers::BoxFuture<
            std::result::Result<ModuleSource, es_runtime_providers::ProviderError>,
        > {
            let result = self
                .files
                .get(specifier)
                .cloned()
                .map(ModuleSource::Text)
                .ok_or_else(|| {
                    es_runtime_providers::ProviderError::Other(format!("not found: {specifier}"))
                });
            Box::pin(async move { result })
        }
    }

    const ENTRY: &str = "file:///app/main.mjs";

    /// Loads + evaluates a module graph (granting FileSystem) and ticks to
    /// quiescence, returning the evaluation outcome.
    fn run_module(
        rt: &mut Runtime,
        source: &str,
        loader: Arc<dyn ModuleLoader>,
    ) -> ModuleEvalState {
        rt.set_capabilities(CapabilitySet::all());
        block_on(async {
            rt.load_module_source(ENTRY, source, loader)
                .await
                .expect("load module graph");
            for _ in 0..500 {
                rt.tick(0);
                rt.process_dynamic_imports()
                    .await
                    .expect("process dynamic imports");
                if !rt.has_pending_work() {
                    break;
                }
            }
        });
        rt.module_eval_state()
    }

    /// Every `runtime:` built-in must **import** with nothing granted — the gate
    /// is the op, never the import (D26, restated by D38's `--deny-all`). This
    /// caught `runtime:process` calling `Env`-gated ops at module-evaluation
    /// time, which made it unimportable under `--deny-env`.
    #[test]
    fn every_builtin_module_imports_with_no_capabilities() {
        let _g = v8_guard();
        for name in runtime_modules::NAMES {
            let engine = V8Engine::new(Limits::default()).expect("engine");
            // Providers are the *embedder's* wiring; capabilities are the
            // *guest's* authority. Only the latter is under test, so the process
            // provider is present (`runtime:process` reads the platform strings
            // through it at evaluation) and everything is denied.
            let providers = HostProviders::new(
                Arc::new(FixedClock {
                    monotonic: 0,
                    wall: 0,
                }),
                Arc::new(TestConsole::default()),
                Arc::new(MockNet::stub()),
                Arc::new(TestEntropy::new()),
            )
            .with_process(Arc::new(StubProcess));
            let mut rt = Runtime::new(Box::new(engine), providers).expect("runtime");
            rt.set_capabilities(CapabilitySet::none());
            let source = format!("import '{name}'; globalThis.ok = true;");
            let state = block_on(async {
                rt.load_module_source(ENTRY, &source, MapLoader::new(&[]))
                    .await
                    .unwrap_or_else(|e| {
                        panic!("{name} failed to import with no capabilities: {e}")
                    });
                for _ in 0..500 {
                    rt.tick(0);
                    if !rt.has_pending_work() {
                        break;
                    }
                }
                rt.module_eval_state()
            });
            assert!(
                matches!(state, ModuleEvalState::Completed),
                "{name} failed to evaluate with no capabilities: {state:?}"
            );
        }
    }

    /// The registry and the name list must not drift apart, or a module could be
    /// added and silently skipped by the guard above.
    #[test]
    fn builtin_names_match_the_source_registry() {
        for name in runtime_modules::NAMES {
            assert!(
                runtime_modules::source(name).is_some(),
                "{name} is listed but has no baked source"
            );
        }
    }

    /// A host module is importable exactly like a baked one — same scheme, same
    /// dedup, and (as with every `runtime:` module) no capability needed to
    /// import it.
    #[test]
    fn host_registered_module_is_importable() {
        let _g = v8_guard();
        let mut rt = runtime();
        rt.register_module("runtime:demo", "export const answer = 42;")
            .expect("register");
        let state = run_module(
            &mut rt,
            "import { answer } from 'runtime:demo'; globalThis.result = answer;",
            MapLoader::new(&[]),
        );
        assert_eq!(state, ModuleEvalState::Completed);
        assert_eq!(rt.eval("globalThis.result").unwrap(), Value::Number(42.0));
    }

    /// The namespace is not open season: a host module cannot take a baked
    /// module's name, because the program that imports `runtime:fs` must get
    /// `runtime:fs`.
    #[test]
    fn host_module_cannot_shadow_a_builtin() {
        let _g = v8_guard();
        let mut rt = runtime();
        let err = rt
            .register_module("runtime:fs", "export const nope = 1;")
            .expect_err("shadowing a built-in must be refused");
        assert!(err.to_string().contains("cannot be replaced"), "{err}");
    }

    /// And it is a `runtime:` seam, not a general module-injection one — an
    /// embedder that wants a bare specifier has a loader for that.
    #[test]
    fn host_module_must_be_in_the_runtime_namespace() {
        let _g = v8_guard();
        let mut rt = runtime();
        let err = rt
            .register_module("demo", "export const nope = 1;")
            .expect_err("a bare specifier must be refused");
        assert!(err.to_string().contains("runtime: namespace"), "{err}");
    }

    /// An unregistered `runtime:` specifier still fails at load rather than
    /// resolving through the loader — `runtime:build` under `esrun` is this
    /// case, and it has to be a clear failure rather than a mystery.
    #[test]
    fn unknown_runtime_module_fails_to_load() {
        let _g = v8_guard();
        let mut rt = runtime();
        rt.set_capabilities(CapabilitySet::all());
        let err =
            block_on(rt.load_module_source(ENTRY, "import 'runtime:nope';", MapLoader::new(&[])))
                .expect_err("an unknown built-in must not load");
        assert!(err.to_string().contains("unknown built-in module"), "{err}");
    }

    #[test]
    fn module_graph_resolves_imports_across_files() {
        let _g = v8_guard();
        let mut rt = runtime();
        let loader = MapLoader::new(&[
            (
                "./a.mjs",
                "import { base } from './b.mjs'; export const val = base + 1;",
            ),
            ("./b.mjs", "export const base = 41;"),
        ]);
        let state = run_module(
            &mut rt,
            "import { val } from './a.mjs'; globalThis.result = val;",
            loader.clone(),
        );
        assert_eq!(state, ModuleEvalState::Completed);
        assert_eq!(rt.eval("globalThis.result").unwrap(), Value::Number(42.0));
    }

    #[test]
    fn json_module_with_attribute_parses() {
        let _g = v8_guard();
        let mut rt = runtime();
        let loader = MapLoader::new(&[("./data.json", "{\"answer\": 42}")]);
        let state = run_module(
            &mut rt,
            "import data from './data.json' with { type: 'json' }; globalThis.result = data.answer;",
            loader,
        );
        assert_eq!(state, ModuleEvalState::Completed);
        assert_eq!(rt.eval("globalThis.result").unwrap(), Value::Number(42.0));
    }

    #[test]
    fn json_module_keys_on_attribute_not_extension() {
        // No `.json` extension, but the attribute says JSON — so it parses.
        let _g = v8_guard();
        let mut rt = runtime();
        let loader = MapLoader::new(&[("./data.conf", "{\"answer\": 7}")]);
        let state = run_module(
            &mut rt,
            "import d from './data.conf' with { type: 'json' }; globalThis.result = d.answer;",
            loader,
        );
        assert_eq!(state, ModuleEvalState::Completed);
        assert_eq!(rt.eval("globalThis.result").unwrap(), Value::Number(7.0));
    }

    #[test]
    fn dynamic_json_import_with_attribute_parses() {
        let _g = v8_guard();
        let mut rt = runtime();
        let loader = MapLoader::new(&[("./data.json", "{\"answer\": 9}")]);
        let state = run_module(
            &mut rt,
            "globalThis.result = 0; const m = await import('./data.json', { with: { type: 'json' } }); globalThis.result = m.default.answer;",
            loader,
        );
        assert_eq!(state, ModuleEvalState::Completed);
        assert_eq!(rt.eval("globalThis.result").unwrap(), Value::Number(9.0));
    }

    #[test]
    fn json_file_without_attribute_is_not_transpiled() {
        // A `.json` extension alone no longer triggers JSON transpilation; without
        // the attribute the raw JSON is compiled as JS and fails (per spec).
        let _g = v8_guard();
        let mut rt = runtime();
        rt.set_capabilities(CapabilitySet::all());
        let loader = MapLoader::new(&[("./data.json", "{\"answer\": 1}")]);
        let result = block_on(rt.load_module_source(
            ENTRY,
            "import data from './data.json'; globalThis.x = data;",
            loader,
        ));
        assert!(
            result.is_err(),
            "a .json imported without `with {{ type: \"json\" }}` must not be transpiled"
        );
    }

    #[test]
    fn dynamic_import_rejection_runs_catch_when_loop_would_be_idle() {
        // Regression: a dynamic import() that fails to load rejects its promise,
        // but the rejection reaction is queued as a microtask. Rejecting inline
        // during the post-tick dynamic-import drain left that microtask with no
        // checkpoint to run it once the loop went idle, so `.catch` never fired
        // (silent, exit 0). The rejection must be deferred into the tick so the
        // reaction runs — with nothing else keeping the loop alive.
        let _g = v8_guard();
        let mut rt = runtime();
        let loader = MapLoader::new(&[]); // any import fails to load
        let state = run_module(
            &mut rt,
            "globalThis.caught = false; \
             import('./missing.mjs').catch(() => { globalThis.caught = true; });",
            loader,
        );
        assert_eq!(state, ModuleEvalState::Completed);
        assert_eq!(rt.eval("globalThis.caught").unwrap(), Value::Bool(true));
    }

    #[test]
    fn caught_dynamic_import_eval_failure_is_not_reported_unhandled() {
        // Regression: dynamically importing a module that throws at top level and
        // catching it still reported the module's *evaluation* promise as an
        // unhandled rejection (and the CLI exited nonzero). The engine observes
        // that promise by polling and forwards its result to the import()
        // promise, so it must be marked handled — the guest's `.catch` is what
        // handles the failure.
        let _g = v8_guard();
        let mut rt = runtime();
        rt.set_capabilities(CapabilitySet::all());
        let loader = MapLoader::new(&[("./boom.mjs", "throw new Error('boom');")]);
        let mut unhandled: Vec<String> = Vec::new();
        block_on(async {
            rt.load_module_source(
                ENTRY,
                "globalThis.caught = false; \
                 import('./boom.mjs').catch(() => { globalThis.caught = true; });",
                loader,
            )
            .await
            .expect("load module graph");
            for _ in 0..500 {
                let status = rt.tick(0);
                unhandled.extend(status.unhandled_rejections.iter().map(ToString::to_string));
                rt.process_dynamic_imports()
                    .await
                    .expect("process dynamic imports");
                if !rt.has_pending_work() {
                    break;
                }
            }
        });
        assert_eq!(rt.eval("globalThis.caught").unwrap(), Value::Bool(true));
        assert!(
            unhandled.is_empty(),
            "caught dynamic import wrongly reported unhandled: {unhandled:?}"
        );
    }

    #[test]
    fn reimport_of_errored_cycle_member_rejects_without_crashing() {
        // Regression: dynamically importing a member of an async (top-level
        // await) cycle whose evaluation already threw re-instantiated/re-evaluated
        // an `Errored` module, tripping a V8 CHECK and aborting the process
        // (SIGABRT) — guest-triggerable. It must instead reject with the cycle's
        // recorded evaluation error. Cycle: a→b→c→a, all async; b throws.
        let _g = v8_guard();
        let mut rt = runtime();
        let loader = MapLoader::new(&[
            ("./a.mjs", "import './b.mjs'; import './x.mjs';"),
            (
                "./b.mjs",
                "import './c.mjs'; await Promise.resolve(0); throw new Error('boom B');",
            ),
            ("./c.mjs", "import './d.mjs'; await Promise.resolve(0);"),
            ("./d.mjs", "import './b.mjs'; await Promise.resolve(0);"),
            ("./x.mjs", "import './d.mjs'; await Promise.resolve(0);"),
        ]);
        let state = run_module(
            &mut rt,
            "globalThis.first = null; globalThis.second = null; \
             try { await import('./a.mjs'); } catch (e) { globalThis.first = e.message; } \
             try { await import('./c.mjs'); } catch (e) { globalThis.second = e.message; }",
            loader,
        );
        assert_eq!(state, ModuleEvalState::Completed);
        // Both imports reject with the cycle's recorded error, no crash.
        assert_eq!(
            rt.eval("globalThis.first").unwrap(),
            Value::String("boom B".into())
        );
        assert_eq!(
            rt.eval("globalThis.second").unwrap(),
            Value::String("boom B".into())
        );
    }

    #[test]
    fn dynamic_import_of_syntax_error_rejects_with_syntax_error() {
        // Regression: a dynamic import() of a module that fails to compile
        // rejected with a generic `Error`, so a `.catch` checking
        // `error.name === 'SyntaxError'` (a common pattern, and what test262's
        // dynamic-import/catch tests assert) saw `'Error'`. The rejection must
        // carry the SyntaxError class.
        let _g = v8_guard();
        let mut rt = runtime();
        // `var x; function x(){}` is legal as a script but a duplicate-declaration
        // SyntaxError as a module.
        let loader = MapLoader::new(&[("./bad.mjs", "var x; function x(){}")]);
        let state = run_module(
            &mut rt,
            "globalThis.name = ''; \
             import('./bad.mjs').catch((e) => { globalThis.name = e.name; });",
            loader,
        );
        assert_eq!(state, ModuleEvalState::Completed);
        assert_eq!(
            rt.eval("globalThis.name").unwrap(),
            Value::String("SyntaxError".into())
        );
    }

    #[test]
    fn diamond_evaluates_shared_dependency_once() {
        let _g = v8_guard();
        let mut rt = runtime();
        let loader = MapLoader::new(&[
            ("./a.mjs", "import './c.mjs';"),
            ("./b.mjs", "import './c.mjs';"),
            (
                "./c.mjs",
                "globalThis.cCount = (globalThis.cCount || 0) + 1;",
            ),
        ]);
        let state = run_module(
            &mut rt,
            "import './a.mjs'; import './b.mjs';",
            loader.clone(),
        );
        assert_eq!(state, ModuleEvalState::Completed);
        // c is reachable via both a and b but compiled + evaluated exactly once.
        assert_eq!(rt.eval("globalThis.cCount").unwrap(), Value::Number(1.0));
    }

    #[test]
    fn import_cycle_completes() {
        let _g = v8_guard();
        let mut rt = runtime();
        let loader = MapLoader::new(&[
            ("./a.mjs", "import './b.mjs'; globalThis.aRan = true;"),
            ("./b.mjs", "import './a.mjs'; globalThis.bRan = true;"),
        ]);
        let state = run_module(&mut rt, "import './a.mjs';", loader.clone());
        assert_eq!(state, ModuleEvalState::Completed);
        assert_true(&mut rt, "globalThis.aRan && globalThis.bRan");
    }

    #[test]
    fn top_level_await_settles_across_ticks() {
        let _g = v8_guard();
        let mut rt = runtime();
        rt.set_capabilities(CapabilitySet::all());
        let loader = MapLoader::new(&[]);
        block_on(rt.load_module_source(
            ENTRY,
            "await new Promise((r) => setTimeout(r, 0)); globalThis.tla = 7;",
            loader.clone(),
        ))
        .expect("load");
        // The graph is async (TLA), so it is not done before any tick runs.
        assert_eq!(rt.module_eval_state(), ModuleEvalState::Pending);
        pump_until(&mut rt, "the top-level await to complete", |rt| {
            !rt.has_pending_work()
        });
        assert_eq!(rt.module_eval_state(), ModuleEvalState::Completed);
        assert_eq!(rt.eval("globalThis.tla").unwrap(), Value::Number(7.0));
    }

    #[test]
    fn import_meta_url_is_the_module_specifier() {
        let _g = v8_guard();
        let mut rt = runtime();
        let loader = MapLoader::new(&[]);
        let state = run_module(
            &mut rt,
            "globalThis.metaUrl = import.meta.url;",
            loader.clone(),
        );
        assert_eq!(state, ModuleEvalState::Completed);
        assert_eq!(
            rt.eval("globalThis.metaUrl").unwrap(),
            Value::String(ENTRY.into())
        );
    }

    #[test]
    fn module_top_level_throw_is_failed() {
        let _g = v8_guard();
        let mut rt = runtime();
        let loader = MapLoader::new(&[]);
        match run_module(&mut rt, "throw new Error('nope');", loader.clone()) {
            ModuleEvalState::Failed(error) => {
                assert!(error.to_string().contains("nope"), "{error}");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn missing_module_is_a_load_error() {
        let _g = v8_guard();
        let mut rt = runtime();
        rt.set_capabilities(CapabilitySet::all());
        let loader = MapLoader::new(&[]); // ./gone.mjs is absent
        let err = block_on(rt.load_module_source(ENTRY, "import './gone.mjs';", loader.clone()))
            .unwrap_err();
        assert!(matches!(err, Error::ModuleLoad(_)), "got {err:?}");
    }

    #[test]
    fn imports_denied_without_filesystem_capability() {
        let _g = v8_guard();
        let mut rt = runtime(); // deny-by-default: no FileSystem capability
        let loader = MapLoader::new(&[("./a.mjs", "export const v = 1;")]);
        let err = block_on(rt.load_module_source(ENTRY, "import './a.mjs';", loader.clone()))
            .unwrap_err();
        assert!(matches!(err, Error::ImportDenied(_)), "got {err:?}");
        // Names the import that failed and where the grant is made — and stays
        // the exception a permission failure has always been.
        assert!(
            err.to_string().contains(r#"cannot import "./a.mjs""#),
            "{err}"
        );
        assert!(err.to_string().contains("--allow-imports"), "{err}");
        use es_runtime_common::IntoException;
        assert_eq!(
            err.exception_class(),
            es_runtime_common::ExceptionClass::NOT_ALLOWED
        );
        assert_eq!(
            err.exception_code(),
            es_runtime_common::Error::CapabilityDenied(Capability::FileSystem).exception_code()
        );
    }

    #[test]
    fn self_contained_module_runs_without_capability() {
        let _g = v8_guard();
        let mut rt = runtime(); // no capabilities granted
        let loader = MapLoader::new(&[]);
        // No imports → the loader is never consulted → no capability needed.
        block_on(rt.load_module_source(ENTRY, "globalThis.ok = 5;", loader.clone())).expect("load");
        pump_until(&mut rt, "the module to evaluate", |rt| {
            !rt.has_pending_work()
        });
        assert_eq!(rt.module_eval_state(), ModuleEvalState::Completed);
        assert_eq!(rt.eval("globalThis.ok").unwrap(), Value::Number(5.0));
    }

    // ----- ES module semantics ----------------------------------------------

    #[test]
    fn default_export_and_import() {
        let _g = v8_guard();
        let mut rt = runtime();
        let loader = MapLoader::new(&[("./greet.mjs", "export default (name) => 'hi ' + name;")]);
        let state = run_module(
            &mut rt,
            "import greet from './greet.mjs'; globalThis.greeting = greet('x');",
            loader.clone(),
        );
        assert_eq!(state, ModuleEvalState::Completed);
        assert_eq!(
            rt.eval("globalThis.greeting").unwrap(),
            Value::String("hi x".into())
        );
    }

    #[test]
    fn namespace_import_exposes_all_exports() {
        let _g = v8_guard();
        let mut rt = runtime();
        let loader = MapLoader::new(&[("./m.mjs", "export const a = 1; export const b = 2;")]);
        let state = run_module(
            &mut rt,
            "import * as ns from './m.mjs'; \
             globalThis.keys = Object.keys(ns).sort().join(','); globalThis.sum = ns.a + ns.b;",
            loader.clone(),
        );
        assert_eq!(state, ModuleEvalState::Completed);
        assert_eq!(
            rt.eval("globalThis.keys").unwrap(),
            Value::String("a,b".into())
        );
        assert_eq!(rt.eval("globalThis.sum").unwrap(), Value::Number(3.0));
    }

    #[test]
    fn module_instance_is_shared_across_importers() {
        // A module imported by two others is evaluated once and its namespace is
        // the same object on both sides (module identity).
        let _g = v8_guard();
        let mut rt = runtime();
        let loader = MapLoader::new(&[
            ("./shared.mjs", "export const x = 1;"),
            (
                "./a.mjs",
                "import * as s from './shared.mjs'; export const sa = s;",
            ),
            (
                "./b.mjs",
                "import * as s from './shared.mjs'; export const sb = s;",
            ),
        ]);
        let state = run_module(
            &mut rt,
            "import { sa } from './a.mjs'; import { sb } from './b.mjs'; \
             globalThis.same = sa === sb;",
            loader.clone(),
        );
        assert_eq!(state, ModuleEvalState::Completed);
        assert_true(&mut rt, "globalThis.same");
    }

    #[test]
    fn re_export_forwards_a_binding() {
        let _g = v8_guard();
        let mut rt = runtime();
        let loader = MapLoader::new(&[
            ("./b.mjs", "export const val = 7;"),
            ("./a.mjs", "export { val } from './b.mjs';"),
        ]);
        let state = run_module(
            &mut rt,
            "import { val } from './a.mjs'; globalThis.reexport = val;",
            loader.clone(),
        );
        assert_eq!(state, ModuleEvalState::Completed);
        assert_eq!(rt.eval("globalThis.reexport").unwrap(), Value::Number(7.0));
    }

    #[test]
    fn imported_binding_is_live() {
        // A `let` export mutated by the module is observed through the importer's
        // binding (ESM live bindings, not a value copy).
        let _g = v8_guard();
        let mut rt = runtime();
        let loader = MapLoader::new(&[(
            "./c.mjs",
            "export let count = 0; export function bump() { count += 1; }",
        )]);
        let state = run_module(
            &mut rt,
            "import { count, bump } from './c.mjs'; bump(); bump(); globalThis.live = count;",
            loader.clone(),
        );
        assert_eq!(state, ModuleEvalState::Completed);
        assert_eq!(rt.eval("globalThis.live").unwrap(), Value::Number(2.0));
    }

    #[test]
    fn dependencies_evaluate_before_dependents() {
        // Post-order: a depends on b, main on a → b, then a, then main.
        let _g = v8_guard();
        let mut rt = runtime();
        let loader = MapLoader::new(&[
            (
                "./a.mjs",
                "import './b.mjs'; globalThis.order = (globalThis.order||'') + 'a';",
            ),
            (
                "./b.mjs",
                "globalThis.order = (globalThis.order||'') + 'b';",
            ),
        ]);
        let state = run_module(
            &mut rt,
            "import './a.mjs'; globalThis.order = (globalThis.order||'') + 'main';",
            loader.clone(),
        );
        assert_eq!(state, ModuleEvalState::Completed);
        assert_eq!(
            rt.eval("globalThis.order").unwrap(),
            Value::String("bamain".into())
        );
    }

    #[test]
    fn cyclic_imports_resolve_via_function_hoisting() {
        // The canonical working ESM cycle: a calls into b which calls back into a
        // through hoisted function declarations.
        let _g = v8_guard();
        let mut rt = runtime();
        let loader = MapLoader::new(&[
            (
                "./a.mjs",
                "import { getB } from './b.mjs'; \
                 export function getA() { return 'A'; } \
                 globalThis.cycleResult = getB();",
            ),
            (
                "./b.mjs",
                "import { getA } from './a.mjs'; \
                 export function getB() { return 'B+' + getA(); }",
            ),
        ]);
        let state = run_module(&mut rt, "import './a.mjs';", loader.clone());
        assert_eq!(state, ModuleEvalState::Completed);
        assert_eq!(
            rt.eval("globalThis.cycleResult").unwrap(),
            Value::String("B+A".into())
        );
    }

    #[test]
    fn duplicate_import_of_one_module_evaluates_it_once() {
        let _g = v8_guard();
        let mut rt = runtime();
        let loader = MapLoader::new(&[(
            "./m.mjs",
            "globalThis.mCount = (globalThis.mCount || 0) + 1; export const v = 21;",
        )]);
        let state = run_module(
            &mut rt,
            "import { v } from './m.mjs'; import { v as v2 } from './m.mjs'; \
             globalThis.dup = v + v2;",
            loader.clone(),
        );
        assert_eq!(state, ModuleEvalState::Completed);
        assert_eq!(rt.eval("globalThis.mCount").unwrap(), Value::Number(1.0));
        assert_eq!(rt.eval("globalThis.dup").unwrap(), Value::Number(42.0));
    }

    #[test]
    fn three_level_graph_resolves() {
        let _g = v8_guard();
        let mut rt = runtime();
        let loader = MapLoader::new(&[
            (
                "./a.mjs",
                "import { b } from './b.mjs'; export const a = b + 1;",
            ),
            (
                "./b.mjs",
                "import { c } from './c.mjs'; export const b = c + 1;",
            ),
            ("./c.mjs", "export const c = 1;"),
        ]);
        let state = run_module(
            &mut rt,
            "import { a } from './a.mjs'; globalThis.deep = a;",
            loader.clone(),
        );
        assert_eq!(state, ModuleEvalState::Completed);
        assert_eq!(rt.eval("globalThis.deep").unwrap(), Value::Number(3.0));
    }

    #[test]
    fn dependency_top_level_await_blocks_dependent() {
        // main must observe the dependency's TLA having completed before main's
        // own body runs.
        let _g = v8_guard();
        let mut rt = runtime();
        let loader = MapLoader::new(&[(
            "./a.mjs",
            "await new Promise((r) => setTimeout(r, 0)); globalThis.depReady = true;",
        )]);
        let state = run_module(
            &mut rt,
            "import './a.mjs'; globalThis.mainSawDep = globalThis.depReady === true;",
            loader.clone(),
        );
        assert_eq!(state, ModuleEvalState::Completed);
        assert_true(&mut rt, "globalThis.mainSawDep");
    }

    #[test]
    fn throw_in_a_dependency_fails_the_graph() {
        let _g = v8_guard();
        let mut rt = runtime();
        let loader = MapLoader::new(&[("./a.mjs", "throw new Error('dep boom');")]);
        match run_module(
            &mut rt,
            "import './a.mjs'; globalThis.reached = true;",
            loader.clone(),
        ) {
            ModuleEvalState::Failed(error) => {
                assert!(error.to_string().contains("dep boom"), "{error}");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
        // The dependent's body must not have run.
        assert_eq!(rt.eval("globalThis.reached").unwrap(), Value::Undefined);
    }

    #[test]
    fn syntax_error_in_a_dependency_is_a_load_error() {
        let _g = v8_guard();
        let mut rt = runtime();
        rt.set_capabilities(CapabilitySet::all());
        let loader = MapLoader::new(&[("./a.mjs", "export const = ;")]);
        // The error surfaces while compiling the dependency during the load walk.
        let err = block_on(rt.load_module_source(ENTRY, "import './a.mjs';", loader.clone()))
            .unwrap_err();
        assert!(matches!(err, Error::Engine(_)), "got {err:?}");
    }

    // ----- dynamic import() -------------------------------------------------

    #[test]
    fn dynamic_import_resolves_to_namespace() {
        let _g = v8_guard();
        let mut rt = runtime();
        let loader = MapLoader::new(&[("./dep.mjs", "export const value = 55;")]);
        let state = run_module(
            &mut rt,
            "const m = await import('./dep.mjs'); globalThis.v = m.value;",
            loader,
        );
        assert_eq!(state, ModuleEvalState::Completed);
        assert_eq!(rt.eval("globalThis.v").unwrap(), Value::Number(55.0));
    }

    #[test]
    fn dynamic_import_then_chain_without_tla() {
        let _g = v8_guard();
        let mut rt = runtime();
        let loader = MapLoader::new(&[("./dep.mjs", "export const value = 9;")]);
        // The entry is synchronous; the import() resolves over later ticks.
        let state = run_module(
            &mut rt,
            "globalThis.v = 0; import('./dep.mjs').then((m) => { globalThis.v = m.value; });",
            loader,
        );
        assert_eq!(state, ModuleEvalState::Completed);
        assert_eq!(rt.eval("globalThis.v").unwrap(), Value::Number(9.0));
    }

    #[test]
    fn dynamic_import_shares_instance_with_static_import() {
        let _g = v8_guard();
        let mut rt = runtime();
        let loader = MapLoader::new(&[(
            "./shared.mjs",
            "globalThis.n = (globalThis.n || 0) + 1; export const x = 1;",
        )]);
        // Imported statically and dynamically: evaluated once, same namespace.
        let state = run_module(
            &mut rt,
            "import './shared.mjs'; const m = await import('./shared.mjs'); \
             globalThis.same = globalThis.n === 1 && m.x === 1;",
            loader,
        );
        assert_eq!(state, ModuleEvalState::Completed);
        assert_true(&mut rt, "globalThis.same");
    }

    #[test]
    fn dynamic_import_of_missing_module_rejects() {
        let _g = v8_guard();
        let mut rt = runtime();
        let loader = MapLoader::new(&[]); // ./gone.mjs absent
        let state = run_module(
            &mut rt,
            "globalThis.err = ''; try { await import('./gone.mjs'); } \
             catch (e) { globalThis.err = String(e.message || e); }",
            loader,
        );
        assert_eq!(state, ModuleEvalState::Completed);
        match rt.eval("globalThis.err").unwrap() {
            Value::String(s) => assert!(s.contains("not found"), "{s}"),
            other => panic!("expected string, got {other:?}"),
        }
    }

    #[test]
    fn dynamic_import_of_top_level_await_module() {
        let _g = v8_guard();
        let mut rt = runtime();
        let loader = MapLoader::new(&[("./tla.mjs", "export const v = await Promise.resolve(7);")]);
        let state = run_module(
            &mut rt,
            "const m = await import('./tla.mjs'); globalThis.tla = m.v;",
            loader,
        );
        assert_eq!(state, ModuleEvalState::Completed);
        assert_eq!(rt.eval("globalThis.tla").unwrap(), Value::Number(7.0));
    }

    // ----- console.log inspection -------------------------------------------

    /// Captures the last console line emitted by `source`.
    fn console_line(source: &str) -> String {
        let console = Arc::new(TestConsole::default());
        let mut rt = runtime_with(
            console.clone(),
            Arc::new(FixedClock {
                monotonic: 0,
                wall: 0,
            }),
        );
        rt.eval(source).expect("eval");
        let lines = console.lines.lock().unwrap_or_else(|e| e.into_inner());
        lines.last().expect("a console line").1.clone()
    }

    #[test]
    fn console_inspects_objects_without_dropping_functions() {
        // The regression behind the moderndash report: an object/namespace of
        // functions must not render as `{}` (JSON.stringify drops functions).
        let line =
            console_line("console.log({ n: 1, fn: function foo() {}, arr: [1, 'two', { x: 3 }] })");
        assert!(line.contains("n: 1"), "{line}");
        assert!(line.contains("fn: [Function: foo]"), "{line}");
        assert!(line.contains("arr: [ 1, 'two', { x: 3 } ]"), "{line}");
    }

    #[test]
    fn console_inspects_a_namespace_of_functions() {
        // Name inference applies to bindings/literals, not `obj.x = fn`, so the
        // arrow is anonymous and the named function keeps its name.
        let line = console_line(
            "const ns = Object.create(null); ns.a = () => {}; ns.b = function bee() {}; \
             console.log(ns);",
        );
        assert!(line.starts_with("[Object: null prototype]"), "{line}");
        assert!(line.contains("a: [Function (anonymous)]"), "{line}");
        assert!(line.contains("b: [Function: bee]"), "{line}");
    }

    #[test]
    fn console_top_level_string_is_bare_nested_is_quoted() {
        assert_eq!(console_line("console.log('hello', 42)"), "hello 42");
        assert_eq!(console_line("console.log(['hello'])"), "[ 'hello' ]");
    }

    #[test]
    fn console_handles_class_circular_and_builtins() {
        assert!(console_line("class P {} console.log(P)").contains("[class P]"));
        assert!(console_line("const o = {}; o.self = o; console.log(o)").contains("[Circular]"));
        assert!(console_line("console.log(new Map([['k', 1]]))").contains("Map(1) { 'k' => 1 }"));
    }

    // ---- runtime:system (child processes, DECISIONS D37) --------------------

    /// What a spawn asked for, recorded so a test can assert on the *seam*
    /// rather than on a real process: the environment a child would have been
    /// given is the whole point of the D37 design, and no OS is needed to check
    /// it.
    #[derive(Clone, Debug, Default)]
    struct Spawned {
        program: String,
        args: Vec<String>,
        env: Vec<(String, String)>,
    }

    /// A [`CommandProvider`] that starts nothing: it records the spec, hands
    /// back canned output, and reports a canned status.
    #[derive(Clone, Default)]
    struct TestCommands {
        spawned: Arc<Mutex<Vec<Spawned>>>,
        stdout: Arc<Mutex<Vec<Vec<u8>>>>,
        stderr: Arc<Mutex<Vec<Vec<u8>>>>,
        written: Arc<Mutex<Vec<u8>>>,
        killed: Arc<Mutex<Vec<String>>>,
        closed: Arc<Mutex<Vec<u64>>>,
        status: Arc<Mutex<es_runtime_providers::ChildStatus>>,
    }

    impl TestCommands {
        fn with_stdout(chunks: &[&str]) -> Arc<TestCommands> {
            let commands = TestCommands::default();
            *commands.stdout.lock().unwrap() =
                chunks.iter().map(|c| c.as_bytes().to_vec()).collect();
            *commands.status.lock().unwrap() = es_runtime_providers::ChildStatus {
                success: true,
                code: Some(0),
                signal: None,
            };
            Arc::new(commands)
        }

        fn last_spawn(&self) -> Spawned {
            self.spawned.lock().unwrap().last().cloned().unwrap()
        }
    }

    impl es_runtime_providers::CommandProvider for TestCommands {
        fn spawn(
            &self,
            spec: es_runtime_providers::CommandSpec,
        ) -> es_runtime_providers::BoxFuture<
            std::result::Result<(u64, u32), es_runtime_providers::ProviderError>,
        > {
            self.spawned.lock().unwrap().push(Spawned {
                program: spec.program,
                args: spec.args,
                env: spec.env,
            });
            Box::pin(std::future::ready(Ok((1, 4242))))
        }

        fn read(
            &self,
            _id: u64,
            stream: es_runtime_providers::ChildStream,
        ) -> es_runtime_providers::BoxFuture<
            std::result::Result<Option<Vec<u8>>, es_runtime_providers::ProviderError>,
        > {
            let queue = match stream {
                es_runtime_providers::ChildStream::Stdout => &self.stdout,
                es_runtime_providers::ChildStream::Stderr => &self.stderr,
            };
            let mut queue = queue.lock().unwrap();
            let next = if queue.is_empty() {
                None
            } else {
                Some(queue.remove(0))
            };
            Box::pin(std::future::ready(Ok(next)))
        }

        fn write(
            &self,
            _id: u64,
            data: Vec<u8>,
        ) -> es_runtime_providers::BoxFuture<
            std::result::Result<(), es_runtime_providers::ProviderError>,
        > {
            self.written.lock().unwrap().extend_from_slice(&data);
            Box::pin(std::future::ready(Ok(())))
        }

        fn close_stdin(
            &self,
            _id: u64,
        ) -> es_runtime_providers::BoxFuture<
            std::result::Result<(), es_runtime_providers::ProviderError>,
        > {
            Box::pin(std::future::ready(Ok(())))
        }

        fn wait(
            &self,
            _id: u64,
        ) -> es_runtime_providers::BoxFuture<
            std::result::Result<
                es_runtime_providers::ChildStatus,
                es_runtime_providers::ProviderError,
            >,
        > {
            let status = self.status.lock().unwrap().clone();
            Box::pin(std::future::ready(Ok(status)))
        }

        fn kill(
            &self,
            _id: u64,
            signal: es_runtime_providers::Signal,
        ) -> es_runtime_providers::BoxFuture<
            std::result::Result<(), es_runtime_providers::ProviderError>,
        > {
            self.killed.lock().unwrap().push(signal.name().to_string());
            Box::pin(std::future::ready(Ok(())))
        }

        fn close(
            &self,
            id: u64,
        ) -> es_runtime_providers::BoxFuture<
            std::result::Result<(), es_runtime_providers::ProviderError>,
        > {
            self.closed.lock().unwrap().push(id);
            Box::pin(std::future::ready(Ok(())))
        }
    }

    /// A [`Process`] view with a fixed environment, so the Secret-masking and
    /// inheritance paths can be exercised without touching the host's.
    struct EnvProcess(Vec<(String, String)>);
    impl es_runtime_providers::Process for EnvProcess {
        fn env(&self) -> Vec<(String, String)> {
            self.0.clone()
        }
        fn args(&self) -> Vec<String> {
            Vec::new()
        }
        fn cwd(&self) -> std::result::Result<String, es_runtime_providers::ProviderError> {
            Ok("/".to_string())
        }
        fn platform(&self) -> String {
            "test".to_string()
        }
        fn arch(&self) -> String {
            "test".to_string()
        }
        fn exit(&self, _code: i32) {}
        fn requested_exit_code(&self) -> Option<i32> {
            None
        }
    }

    fn command_runtime(commands: Arc<TestCommands>, env: &[(&str, &str)]) -> Runtime {
        let engine = V8Engine::new(Limits::default()).expect("engine");
        let providers = HostProviders::new(
            Arc::new(FixedClock {
                monotonic: 0,
                wall: 0,
            }),
            Arc::new(TestConsole::default()),
            Arc::new(MockNet::stub()),
            Arc::new(TestEntropy::new()),
        )
        .with_commands(commands)
        .with_process(Arc::new(EnvProcess(
            env.iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
        )));
        Runtime::new(Box::new(engine), providers).expect("runtime")
    }

    /// Runs a module against a capability set of the test's choosing (unlike
    /// [`run_module`], which grants everything), and reads the answer back off
    /// `globalThis.result`.
    fn run_system_module(rt: &mut Runtime, caps: CapabilitySet, source: &str) -> Value {
        rt.set_capabilities(caps);
        block_on(async {
            rt.load_module_source(ENTRY, source, MapLoader::new(&[]))
                .await
                .expect("load module graph");
            for _ in 0..500 {
                rt.tick(0);
                rt.process_dynamic_imports()
                    .await
                    .expect("process dynamic imports");
                if !rt.has_pending_work() {
                    break;
                }
            }
        });
        rt.eval("globalThis.result").unwrap()
    }

    #[test]
    fn spawning_requires_the_run_capability() {
        let _g = v8_guard();
        let mut rt = command_runtime(TestCommands::with_stdout(&[]), &[]);
        // Everything except the capability under test, so a denial can only be
        // Run — not some other gate the op happens to trip first.
        let mut caps = CapabilitySet::all();
        caps.revoke(Capability::Run);
        rt.set_capabilities(caps);
        let out = eval_async(
            &mut rt,
            "try { await __ops.system_spawn({ program: 'echo' }); return 'no throw'; } \
             catch (e) { return e.code; }",
        );
        assert_eq!(out, Value::String("ERR_CAPABILITY_DENIED".into()));
    }

    #[test]
    fn every_other_capability_together_does_not_grant_run() {
        // Run is never implied: a child process escapes every confinement this
        // runtime applies, so it must be asked for by name (D37).
        let _g = v8_guard();
        let mut rt = command_runtime(TestCommands::with_stdout(&[]), &[]);
        let mut caps = CapabilitySet::none();
        for cap in [
            Capability::Env,
            Capability::FileSystem,
            Capability::FileRead,
            Capability::FileWrite,
            Capability::Net,
            Capability::NetListen,
            Capability::Signals,
        ] {
            caps.grant(cap);
        }
        rt.set_capabilities(caps);
        let out = eval_async(
            &mut rt,
            "try { await __ops.system_spawn({ program: 'echo' }); return 'no throw'; } \
             catch (e) { return e.code; }",
        );
        assert_eq!(out, Value::String("ERR_CAPABILITY_DENIED".into()));
    }

    #[test]
    fn a_child_gets_no_environment_unless_one_is_passed() {
        let _g = v8_guard();
        let commands = TestCommands::with_stdout(&["out"]);
        let mut rt = command_runtime(commands.clone(), &[("HOST_SECRET", "leaked")]);
        let result = run_system_module(
            &mut rt,
            CapabilitySet::all(),
            "import { Command } from 'runtime:system'; \
             const out = await new Command('echo', { args: ['hi'], env: { ONLY: 'this' } }).output(); \
             globalThis.result = new TextDecoder().decode(out.stdout);",
        );
        assert_eq!(result, Value::String("out".into()));
        let spawn = commands.last_spawn();
        assert_eq!(spawn.program, "echo");
        assert_eq!(spawn.args, vec!["hi".to_string()]);
        assert_eq!(spawn.env, vec![("ONLY".to_string(), "this".to_string())]);
    }

    #[test]
    fn inheriting_the_environment_needs_the_env_capability_too() {
        // The point of the split: Run starts a program, Env is what lets that
        // program be handed the host's environment.
        let _g = v8_guard();
        let commands = TestCommands::with_stdout(&[]);
        let mut rt = command_runtime(commands, &[("HOST_SECRET", "leaked")]);
        let result = run_system_module(
            &mut rt,
            CapabilitySet::none().with(Capability::Run),
            "import { Command } from 'runtime:system'; \
             try { new Command('echo', { inheritEnv: true }); globalThis.result = 'no throw'; } \
             catch (e) { globalThis.result = e.code; }",
        );
        assert_eq!(result, Value::String("ERR_CAPABILITY_DENIED".into()));
    }

    #[test]
    fn inherited_environment_reaches_the_child_when_both_are_granted() {
        let _g = v8_guard();
        let commands = TestCommands::with_stdout(&[]);
        let mut rt = command_runtime(commands.clone(), &[("HOST_VAR", "value")]);
        run_system_module(
            &mut rt,
            CapabilitySet::all(),
            "import { Command } from 'runtime:system'; \
             await new Command('echo', { inheritEnv: true, env: { EXTRA: 'x' } }).output(); \
             globalThis.result = 'done';",
        );
        let env = commands.last_spawn().env;
        assert!(env.contains(&("HOST_VAR".to_string(), "value".to_string())));
        assert!(env.contains(&("EXTRA".to_string(), "x".to_string())));
    }

    #[test]
    fn a_secret_env_value_reaches_the_child_unmasked() {
        // A masked value stringifies to "[redacted]"; handing that to a child
        // would be a silent, undebuggable failure (D30 masks accidents, and
        // this is not one).
        let _g = v8_guard();
        let commands = TestCommands::with_stdout(&[]);
        let mut rt = command_runtime(commands.clone(), &[("API_TOKEN", "s3cret")]);
        run_system_module(
            &mut rt,
            CapabilitySet::all(),
            "import { Command } from 'runtime:system'; \
             import { env } from 'runtime:process'; \
             await new Command('echo', { env: { API_TOKEN: env.API_TOKEN } }).output(); \
             globalThis.result = String(env.API_TOKEN);",
        );
        assert_eq!(
            rt.eval("globalThis.result").unwrap(),
            Value::String("[redacted]".into()),
            "the Secret still masks everywhere else"
        );
        assert_eq!(
            commands.last_spawn().env,
            vec![("API_TOKEN".to_string(), "s3cret".to_string())]
        );
    }

    #[test]
    fn output_collects_both_streams_and_the_status() {
        let _g = v8_guard();
        let commands = TestCommands::with_stdout(&["one ", "two"]);
        *commands.stderr.lock().unwrap() = vec![b"warned".to_vec()];
        *commands.status.lock().unwrap() = es_runtime_providers::ChildStatus {
            success: false,
            code: Some(3),
            signal: None,
        };
        let mut rt = command_runtime(commands.clone(), &[]);
        let result = run_system_module(
            &mut rt,
            CapabilitySet::all(),
            "import { Command } from 'runtime:system'; \
             const o = await new Command('x').output(); \
             const d = new TextDecoder(); \
             globalThis.result = [o.success, o.code, d.decode(o.stdout), d.decode(o.stderr)].join('|');",
        );
        assert_eq!(result, Value::String("false|3|one two|warned".into()));
        // Reaped and drained ⇒ the child's pipes are released to the host.
        assert_eq!(*commands.closed.lock().unwrap(), vec![1]);
    }

    #[test]
    fn output_past_max_buffer_kills_the_child_and_says_so() {
        let _g = v8_guard();
        let commands = TestCommands::with_stdout(&["0123456789", "0123456789"]);
        let mut rt = command_runtime(commands.clone(), &[]);
        let result = run_system_module(
            &mut rt,
            CapabilitySet::all(),
            "import { Command } from 'runtime:system'; \
             try { await new Command('x', { maxBuffer: 12 }).output(); globalThis.result = 'no throw'; } \
             catch (e) { globalThis.result = e.code; }",
        );
        assert_eq!(result, Value::String("ERR_MAX_BUFFER".into()));
        assert_eq!(
            *commands.killed.lock().unwrap(),
            vec!["SIGTERM".to_string()]
        );
    }

    #[test]
    fn a_written_stdin_reaches_the_child_and_closes() {
        let _g = v8_guard();
        let commands = TestCommands::with_stdout(&[]);
        let mut rt = command_runtime(commands.clone(), &[]);
        run_system_module(
            &mut rt,
            CapabilitySet::all(),
            "import { Command } from 'runtime:system'; \
             const child = await new Command('cat', { stdin: 'a body' }).spawn(); \
             await child.status; \
             globalThis.result = 'done';",
        );
        assert_eq!(
            String::from_utf8(commands.written.lock().unwrap().clone()).unwrap(),
            "a body"
        );
    }

    #[test]
    fn an_unknown_kill_signal_is_a_type_error_not_a_silent_terminate() {
        let _g = v8_guard();
        let commands = TestCommands::with_stdout(&[]);
        let mut rt = command_runtime(commands.clone(), &[]);
        let result = run_system_module(
            &mut rt,
            CapabilitySet::all(),
            "import { Command } from 'runtime:system'; \
             const child = await new Command('x').spawn(); \
             try { await child.kill('SIGNOPE'); globalThis.result = 'no throw'; } \
             catch (e) { globalThis.result = e.constructor.name; }",
        );
        assert_eq!(result, Value::String("TypeError".into()));
        assert!(commands.killed.lock().unwrap().is_empty());
    }

    #[test]
    fn arguments_are_never_a_shell_command_line() {
        // There is no shell in this module: whatever is in `args` reaches the
        // child as one argument, metacharacters and all.
        let _g = v8_guard();
        let commands = TestCommands::with_stdout(&[]);
        let mut rt = command_runtime(commands.clone(), &[]);
        run_system_module(
            &mut rt,
            CapabilitySet::all(),
            "import { Command } from 'runtime:system'; \
             await new Command('echo', { args: ['a; rm -rf /', '$(id)'] }).output(); \
             globalThis.result = 'done';",
        );
        assert_eq!(
            commands.last_spawn().args,
            vec!["a; rm -rf /".to_string(), "$(id)".to_string()]
        );
    }

    /// An [`HttpServerProvider`] that records what `serve` was asked for and
    /// binds nothing. The ALPN list a guest's `serve()` produces is a documented
    /// default and part of what a client negotiates against, so it is pinned
    /// here — at the layer that decides it — rather than inferred from a
    /// handshake.
    #[derive(Default)]
    struct RecordingHttpServer {
        options: Arc<Mutex<Vec<es_runtime_providers::HttpServeOptions>>>,
        /// Requests to hand over, one batch, before reporting the server
        /// closed. Lets a test drive a handler without a socket — which is the
        /// only way to reach the "the host has no peer to report" branch, since
        /// a real connection always has one.
        deliver: Arc<Mutex<Vec<es_runtime_providers::HttpServerRequest>>>,
        /// What the handler answered, in order.
        responses: Arc<Mutex<Vec<es_runtime_providers::HttpServerResponse>>>,
    }

    /// A request with no body, as a provider with no socket would report it.
    fn canned_request(
        remote_address: &str,
        remote_port: u16,
    ) -> es_runtime_providers::HttpServerRequest {
        es_runtime_providers::HttpServerRequest {
            method: "GET".to_string(),
            url: "http://127.0.0.1:8443/who".to_string(),
            headers: vec![],
            body: es_runtime_providers::HttpServerBody::Empty,
            remote_address: remote_address.to_string(),
            remote_port,
        }
    }

    impl es_runtime_providers::HttpServerProvider for RecordingHttpServer {
        fn serve(
            &self,
            options: es_runtime_providers::HttpServeOptions,
        ) -> es_runtime_providers::BoxFuture<
            std::result::Result<
                (u64, es_runtime_providers::SocketInfo),
                es_runtime_providers::ProviderError,
            >,
        > {
            let info = es_runtime_providers::SocketInfo {
                remote_address: String::new(),
                remote_port: 0,
                local_address: options.host.clone(),
                local_port: 8443,
                alpn: None,
            };
            self.options.lock().unwrap().push(options);
            Box::pin(std::future::ready(Ok((1, info))))
        }

        fn next_requests(
            &self,
            _id: u64,
            _max: usize,
        ) -> es_runtime_providers::BoxFuture<
            std::result::Result<
                Vec<(u64, es_runtime_providers::HttpServerRequest)>,
                es_runtime_providers::ProviderError,
            >,
        > {
            // Empty ⇒ "the server is closed", which ends the accept loop rather
            // than holding the module open on a server nothing will call. A
            // queued batch goes over first, and is only handed out once.
            let queued: Vec<_> = std::mem::take(&mut *self.deliver.lock().unwrap())
                .into_iter()
                .enumerate()
                .map(|(i, req)| (i as u64 + 1, req))
                .collect();
            Box::pin(std::future::ready(Ok(queued)))
        }

        fn respond(
            &self,
            _request_id: u64,
            response: es_runtime_providers::HttpServerResponse,
        ) -> es_runtime_providers::BoxFuture<
            std::result::Result<(), es_runtime_providers::ProviderError>,
        > {
            self.responses.lock().unwrap().push(response);
            Box::pin(std::future::ready(Ok(())))
        }

        fn close(
            &self,
            _id: u64,
        ) -> es_runtime_providers::BoxFuture<
            std::result::Result<(), es_runtime_providers::ProviderError>,
        > {
            Box::pin(std::future::ready(Ok(())))
        }
    }

    fn http_runtime(http: Arc<RecordingHttpServer>) -> Runtime {
        let engine = V8Engine::new(Limits::default()).expect("engine");
        let providers = HostProviders::new(
            Arc::new(FixedClock {
                monotonic: 0,
                wall: 0,
            }),
            Arc::new(TestConsole::default()),
            Arc::new(MockNet::stub()),
            Arc::new(TestEntropy::new()),
        )
        .with_http_server(http);
        Runtime::new(Box::new(engine), providers).expect("runtime")
    }

    /// The TLS ALPN list a `serve()` with no `alpn` advertises. Both versions,
    /// h2 first — ALPN order is the server's preference, so this is what decides
    /// that an h2-capable client gets HTTP/2.
    #[test]
    fn serve_advertises_h2_then_http11_by_default() {
        let _g = v8_guard();
        let http = Arc::new(RecordingHttpServer::default());
        let mut rt = http_runtime(http.clone());
        run_module(
            &mut rt,
            "import { serve } from 'runtime:http'; \
             const s = serve({ hostname: '127.0.0.1', port: 8443, secureTransport: 'on', \
                               cert: 'PEM-CERT', key: 'PEM-KEY' }, () => new Response('x')); \
             globalThis.result = (await s.addr).port;",
            MapLoader::new(&[]),
        );
        let options = http.options.lock().unwrap();
        let tls = options[0].tls.as_ref().expect("secureTransport: on");
        assert_eq!(tls.alpn, vec!["h2".to_string(), "http/1.1".to_string()]);
    }

    /// …and naming `alpn` replaces that list rather than adding to it, which is
    /// what pins a listener to one version for a client that mishandles h2.
    #[test]
    fn an_explicit_alpn_replaces_the_default() {
        let _g = v8_guard();
        let http = Arc::new(RecordingHttpServer::default());
        let mut rt = http_runtime(http.clone());
        run_module(
            &mut rt,
            "import { serve } from 'runtime:http'; \
             const s = serve({ port: 8443, secureTransport: 'on', cert: 'C', key: 'K', \
                               alpn: ['http/1.1'] }, () => new Response('x')); \
             globalThis.result = (await s.addr).port;",
            MapLoader::new(&[]),
        );
        let options = http.options.lock().unwrap();
        let tls = options[0].tls.as_ref().expect("secureTransport: on");
        assert_eq!(tls.alpn, vec!["http/1.1".to_string()]);
    }

    /// A cleartext `serve()` carries no TLS at all — h2c is decided by the
    /// connection preface on the wire, so there is nothing for ALPN to say here.
    #[test]
    fn a_cleartext_serve_sends_no_tls_options() {
        let _g = v8_guard();
        let http = Arc::new(RecordingHttpServer::default());
        let mut rt = http_runtime(http.clone());
        run_module(
            &mut rt,
            "import { serve } from 'runtime:http'; \
             const s = serve({ port: 8080 }, () => new Response('x')); \
             globalThis.result = (await s.addr).port;",
            MapLoader::new(&[]),
        );
        assert!(http.options.lock().unwrap()[0].tls.is_none());
    }

    /// A guest that says nothing about timeouts gets the provider's defaults —
    /// the point of them. The prelude must send "unset" rather than a copy of
    /// the numbers, so this asserts the values arrive equal to
    /// `HttpTimeouts::default()` rather than to any literal written here.
    #[test]
    fn serve_without_timeouts_uses_the_provider_defaults() {
        let _g = v8_guard();
        let http = Arc::new(RecordingHttpServer::default());
        let mut rt = http_runtime(http.clone());
        run_module(
            &mut rt,
            "import { serve } from 'runtime:http'; \
             const s = serve({ port: 8080 }, () => new Response('x')); \
             globalThis.result = (await s.addr).port;",
            MapLoader::new(&[]),
        );
        assert_eq!(
            http.options.lock().unwrap()[0].timeouts,
            es_runtime_providers::HttpTimeouts::default()
        );
    }

    #[test]
    fn each_timeout_can_be_set_and_they_cross_as_milliseconds() {
        let _g = v8_guard();
        let http = Arc::new(RecordingHttpServer::default());
        let mut rt = http_runtime(http.clone());
        run_module(
            &mut rt,
            "import { serve } from 'runtime:http'; \
             const s = serve({ port: 8080, timeouts: { handshake: 1000, headerRead: 2000, \
                               h2KeepAlive: 3000 } }, () => new Response('x')); \
             globalThis.result = (await s.addr).port;",
            MapLoader::new(&[]),
        );
        let options = http.options.lock().unwrap();
        assert_eq!(
            options[0].timeouts,
            es_runtime_providers::HttpTimeouts {
                handshake: Some(std::time::Duration::from_secs(1)),
                header_read: Some(std::time::Duration::from_secs(2)),
                h2_keep_alive: Some(std::time::Duration::from_secs(3)),
                ..es_runtime_providers::HttpTimeouts::default()
            }
        );
    }

    /// The body bound is two numbers that mean different things — a duration
    /// and a rate — so both have to arrive as themselves.
    #[test]
    fn the_body_bound_crosses_as_a_grace_and_a_rate() {
        let _g = v8_guard();
        let http = Arc::new(RecordingHttpServer::default());
        let mut rt = http_runtime(http.clone());
        run_module(
            &mut rt,
            "import { serve } from 'runtime:http'; \
             const s = serve({ port: 8080, timeouts: { bodyRead: 5000, bodyMinRate: 4096 } }, \
                             () => new Response('x')); \
             globalThis.result = (await s.addr).port;",
            MapLoader::new(&[]),
        );
        let options = http.options.lock().unwrap();
        assert_eq!(
            options[0].timeouts.body_read,
            Some(std::time::Duration::from_secs(5))
        );
        assert_eq!(options[0].timeouts.body_min_rate, 4096);
    }

    /// `0` is a rate, not a disabled timeout: it says a slow body earns no
    /// extension, which turns the grace into a flat cap. Reading it as "unset"
    /// would hand back the default allowance instead — the opposite of what
    /// was asked for.
    #[test]
    fn a_zero_body_rate_is_a_rate_and_not_an_omission() {
        let _g = v8_guard();
        let http = Arc::new(RecordingHttpServer::default());
        let mut rt = http_runtime(http.clone());
        run_module(
            &mut rt,
            "import { serve } from 'runtime:http'; \
             const s = serve({ port: 8080, timeouts: { bodyMinRate: 0 } }, \
                             () => new Response('x')); \
             globalThis.result = (await s.addr).port;",
            MapLoader::new(&[]),
        );
        let defaults = es_runtime_providers::HttpTimeouts::default();
        let options = http.options.lock().unwrap();
        assert_eq!(options[0].timeouts.body_min_rate, 0);
        // And naming the rate alone leaves the grace where it was.
        assert_eq!(options[0].timeouts.body_read, defaults.body_read);
    }

    /// `null` is off, and it has to survive the crossing as off rather than as
    /// "unset" — otherwise a guest asking for no timeout would silently get the
    /// default instead, which is the one mistake here that fails safe-looking.
    #[test]
    fn a_null_timeout_disables_it_rather_than_falling_back_to_the_default() {
        let _g = v8_guard();
        let http = Arc::new(RecordingHttpServer::default());
        let mut rt = http_runtime(http.clone());
        run_module(
            &mut rt,
            "import { serve } from 'runtime:http'; \
             const s = serve({ port: 8080, timeouts: { handshake: null, headerRead: null, \
                               h2KeepAlive: null } }, () => new Response('x')); \
             globalThis.result = (await s.addr).port;",
            MapLoader::new(&[]),
        );
        let options = http.options.lock().unwrap();
        assert_eq!(options[0].timeouts.handshake, None);
        assert_eq!(options[0].timeouts.header_read, None);
        assert_eq!(options[0].timeouts.h2_keep_alive, None);
    }

    /// The body bound is removed by `bodyRead: null`, the same spelling as the
    /// rest — the rate has nothing to extend once there is no grace.
    #[test]
    fn a_null_body_read_removes_the_bound_entirely() {
        let _g = v8_guard();
        let http = Arc::new(RecordingHttpServer::default());
        let mut rt = http_runtime(http.clone());
        run_module(
            &mut rt,
            "import { serve } from 'runtime:http'; \
             const s = serve({ port: 8080, timeouts: { bodyRead: null } }, \
                             () => new Response('x')); \
             globalThis.result = (await s.addr).port;",
            MapLoader::new(&[]),
        );
        assert_eq!(http.options.lock().unwrap()[0].timeouts.body_read, None);
    }

    /// One timeout named, the rest defaulted — the common case, and the one a
    /// naive "read the object" implementation gets wrong by zeroing the others.
    #[test]
    fn naming_one_timeout_leaves_the_others_at_their_defaults() {
        let _g = v8_guard();
        let http = Arc::new(RecordingHttpServer::default());
        let mut rt = http_runtime(http.clone());
        run_module(
            &mut rt,
            "import { serve } from 'runtime:http'; \
             const s = serve({ port: 8080, timeouts: { headerRead: 5000 } }, \
                             () => new Response('x')); \
             globalThis.result = (await s.addr).port;",
            MapLoader::new(&[]),
        );
        let defaults = es_runtime_providers::HttpTimeouts::default();
        let options = http.options.lock().unwrap();
        assert_eq!(
            options[0].timeouts.header_read,
            Some(std::time::Duration::from_secs(5))
        );
        assert_eq!(options[0].timeouts.handshake, defaults.handshake);
        assert_eq!(options[0].timeouts.h2_keep_alive, defaults.h2_keep_alive);
        assert_eq!(options[0].timeouts.body_read, defaults.body_read);
        assert_eq!(options[0].timeouts.body_min_rate, defaults.body_min_rate);
    }

    /// Body of the single response the handler produced.
    fn recorded_body(http: &RecordingHttpServer) -> String {
        let responses = http.responses.lock().unwrap();
        let one = responses.first().expect("the handler answered");
        match &one.body {
            es_runtime_providers::HttpServerBody::Bytes(b) => {
                String::from_utf8_lossy(b).into_owned()
            }
            _ => panic!("expected a buffered body"),
        }
    }

    /// The handler's second argument carries the peer, in the shape
    /// `Deno.serve` passes — so the same handler runs on either runtime.
    #[test]
    fn a_handler_is_told_which_peer_a_request_came_from() {
        let _g = v8_guard();
        let http = Arc::new(RecordingHttpServer::default());
        http.deliver
            .lock()
            .unwrap()
            .push(canned_request("203.0.113.7", 54321));
        let mut rt = http_runtime(http.clone());
        run_module(
            &mut rt,
            "import { serve } from 'runtime:http'; \
             serve({ port: 8443 }, (request, info) => \
               new Response(`${info.remoteAddr.transport}/${info.remoteAddr.hostname}/\
${info.remoteAddr.port}`));",
            MapLoader::new(&[]),
        );
        assert_eq!(recorded_body(&http), "tcp/203.0.113.7/54321");
    }

    /// A provider with no socket — a mock, an embedder's own transport — has no
    /// peer to report, and saying so is `null`. An address-shaped object full of
    /// blanks would be worse than useless: a handler would happily key a rate
    /// limit on the empty string.
    #[test]
    fn an_unknown_peer_is_null_rather_than_a_blank_address() {
        let _g = v8_guard();
        let http = Arc::new(RecordingHttpServer::default());
        http.deliver.lock().unwrap().push(canned_request("", 0));
        let mut rt = http_runtime(http.clone());
        run_module(
            &mut rt,
            "import { serve } from 'runtime:http'; \
             serve({ port: 8443 }, (request, info) => \
               new Response(String(info.remoteAddr)));",
            MapLoader::new(&[]),
        );
        assert_eq!(recorded_body(&http), "null");
    }

    /// The argument is additive: every handler written before it exists takes
    /// one parameter and must keep working untouched.
    #[test]
    fn a_handler_that_ignores_the_second_argument_still_works() {
        let _g = v8_guard();
        let http = Arc::new(RecordingHttpServer::default());
        http.deliver
            .lock()
            .unwrap()
            .push(canned_request("203.0.113.7", 54321));
        let mut rt = http_runtime(http.clone());
        run_module(
            &mut rt,
            "import { serve } from 'runtime:http'; \
             serve({ port: 8443 }, (request) => new Response(new URL(request.url).pathname));",
            MapLoader::new(&[]),
        );
        assert_eq!(recorded_body(&http), "/who");
    }

    /// No cap unless one is asked for: the right number follows from a
    /// deployment's descriptor budget, and one guessed here would throttle real
    /// traffic with nothing to explain it.
    #[test]
    fn serve_is_uncapped_unless_max_connections_says_otherwise() {
        let _g = v8_guard();
        let http = Arc::new(RecordingHttpServer::default());
        let mut rt = http_runtime(http.clone());
        run_module(
            &mut rt,
            "import { serve } from 'runtime:http'; \
             const a = serve({ port: 8080 }, () => new Response('x')); \
             const b = serve({ port: 8081, maxConnections: 512 }, () => new Response('x')); \
             globalThis.result = (await a.addr).port + (await b.addr).port;",
            MapLoader::new(&[]),
        );
        let options = http.options.lock().unwrap();
        assert_eq!(options[0].max_connections, None);
        assert_eq!(options[1].max_connections, Some(512));
        // The per-peer half is separately off: a whole-server cap says nothing
        // about whose connections fill it, and asking for one must not imply
        // the other.
        assert_eq!(options[0].max_connections_per_ip, None);
        assert_eq!(options[1].max_connections_per_ip, None);
    }

    /// The two caps are independent numbers that answer different questions —
    /// how much the deployment spends, and whose connections it spends it on.
    #[test]
    fn the_per_peer_cap_crosses_independently_of_the_whole_server_one() {
        let _g = v8_guard();
        let http = Arc::new(RecordingHttpServer::default());
        let mut rt = http_runtime(http.clone());
        run_module(
            &mut rt,
            "import { serve } from 'runtime:http'; \
             const a = serve({ port: 8080, maxConnectionsPerIp: 4 }, () => new Response('x')); \
             const b = serve({ port: 8081, maxConnections: 512, maxConnectionsPerIp: 8 }, \
                             () => new Response('x')); \
             globalThis.result = (await a.addr).port + (await b.addr).port;",
            MapLoader::new(&[]),
        );
        let options = http.options.lock().unwrap();
        assert_eq!(options[0].max_connections, None);
        assert_eq!(options[0].max_connections_per_ip, Some(4));
        assert_eq!(options[1].max_connections, Some(512));
        assert_eq!(options[1].max_connections_per_ip, Some(8));
    }

    /// A cap of zero would serve nothing at all, and a fractional one is a
    /// mistake rather than an intention — both are rejected at the call.
    #[test]
    fn an_unusable_connection_cap_is_rejected_before_the_port_is_bound() {
        let _g = v8_guard();
        let http = Arc::new(RecordingHttpServer::default());
        let mut rt = http_runtime(http.clone());
        run_module(
            &mut rt,
            "import { serve } from 'runtime:http'; \
             const names = []; \
             for (const bad of ['lots', 0, -1, 1.5]) { \
               try { serve({ port: 8080, maxConnections: bad }, () => new Response('x')); } \
               catch (e) { names.push(e.constructor.name); } \
               try { serve({ port: 8080, maxConnectionsPerIp: bad }, () => new Response('x')); } \
               catch (e) { names.push(e.constructor.name); } \
             } \
             globalThis.result = names.join(',');",
            MapLoader::new(&[]),
        );
        assert_eq!(
            rt.eval("globalThis.result").unwrap(),
            Value::String(
                "TypeError,TypeError,RangeError,RangeError,RangeError,RangeError,RangeError,\
                 RangeError"
                    .to_string()
            )
        );
        assert!(
            http.options.lock().unwrap().is_empty(),
            "a rejected option must not have reached the provider"
        );
    }

    /// Rejected in the prelude, before the bind: a typo in a timeout must not
    /// claim a port and then serve with a silently different policy.
    #[test]
    fn a_bad_timeout_is_rejected_before_the_port_is_bound() {
        let _g = v8_guard();
        let http = Arc::new(RecordingHttpServer::default());
        let mut rt = http_runtime(http.clone());
        run_module(
            &mut rt,
            "import { serve } from 'runtime:http'; \
             const bad = [['handshake', 'soon'], ['headerRead', -1], ['h2KeepAlive', NaN], \
                          ['bodyRead', 'later'], ['bodyMinRate', -1], ['bodyMinRate', 'fast']]; \
             const names = []; \
             for (const [key, value] of bad) { \
               try { serve({ port: 8080, timeouts: { [key]: value } }, () => new Response('x')); } \
               catch (e) { names.push(e.constructor.name); } \
             } \
             try { serve({ port: 8080, timeouts: 5 }, () => new Response('x')); } \
             catch (e) { names.push(e.constructor.name); } \
             globalThis.result = names.join(',');",
            MapLoader::new(&[]),
        );
        assert_eq!(
            rt.eval("globalThis.result").unwrap(),
            Value::String(
                "TypeError,RangeError,RangeError,TypeError,RangeError,TypeError,TypeError"
                    .to_string()
            )
        );
        assert!(
            http.options.lock().unwrap().is_empty(),
            "a rejected option must not have reached the provider"
        );
    }
}
