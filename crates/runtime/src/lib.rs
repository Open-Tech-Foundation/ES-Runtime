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

mod archive;
mod archive_ops;
mod base64_ops;
mod blocking;
mod builtins;
mod cbor;
mod compression_ops;
mod cookie_ops;
mod crypto_ops;
mod csrf_ops;
mod csv;
mod curve25519_ops;
mod db_conns;
mod db_ops;
mod diagnostics_ops;
mod ec_ops;
mod encoding_ops;
mod fetch_ops;
mod fs_ops;
#[cfg(feature = "fuzzing")]
#[doc(hidden)]
pub mod fuzz;
pub mod glob;
mod handles;
mod hashing_ops;
mod http_ops;
mod image_ops;
mod js_number;
mod module_ops;
mod mongo_ops;
mod mongodb;
mod msgpack;
mod my_ops;
mod mysql;
mod net_ops;
/// The op-boundary fuzzer's harness (D253): the cargo-fuzz `ops` target, and a
/// seeded run in this crate's tests. Not public API.
#[cfg(any(test, feature = "fuzzing"))]
#[doc(hidden)]
pub mod op_fuzz;
pub mod otlp;
mod pdf_ops;
mod pg_ops;
mod postgres;
mod prelude;
mod process_ops;
mod rsa_ops;
mod runtime_modules;
mod scram;
/// Fuzz harnesses that run JavaScript in a live runtime: the cargo-fuzz
/// `db_wire` and `request_parsers` targets, and a seeded run of each in this
/// crate's tests. Not public API.
#[cfg(any(test, feature = "fuzzing"))]
#[doc(hidden)]
pub mod script_fuzz;
pub mod serialization_ops;
mod sync_fs_ops;
mod system_ops;
mod timer;
mod url_ops;
mod urlpattern_ops;
mod worker_ops;
mod ws_ops;
mod xml;
mod yaml;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use crate::timer::TimerQueue;

// One-stop public surface for embedders: the engine abstraction + impl, the op
// types, values, capabilities, and the provider traits — all reachable here.
pub use es_runtime_common::{Capability, CapabilitySet, UncaughtError};
pub use es_runtime_engine::diagnostics;
/// What every capability denial inside an `Isolate` ends with (D227): the
/// remedy a flag or a grant would be elsewhere does not exist there.
const ISOLATE_DENIAL: &str =
    "an isolate is computation only; pass it what it needs through a host function";

pub use es_runtime_engine::{
    AsyncOp, CapabilityObserver, CpuBudget, Engine, HAS_INSPECTOR, InspectorOptions,
    InspectorTransport, InterruptHandle, ModuleEvalState, ModuleId, NO_INSPECTOR_MESSAGE, OpDecl,
    OpError, OpResult, SharedObserver, TurnHandle, V8Engine, Value,
};
pub use es_runtime_providers::{
    BroadcastHub, ChildStatus, ChildStream, Clock, CommandProvider, CommandSpec, Console,
    ConsoleLevel, EmbeddedDb, Entropy, FileSystem, HttpServerProvider, ModuleLoader, ModuleSource,
    NetProvider, NetTransport, PortHub, Process, Signals, Stdio, SyncFileSystem, TaskSpawner,
    TelemetrySink, WebSocketProvider, WorkerHost, WorkerScope,
};

/// The exporter's live state: its subscription, the clock offset it converts
/// timestamps with, and where the payloads go.
struct Telemetry {
    config: TelemetryConfig,
    origin: otlp::TimeOrigin,
    subscription: u64,
    recorder: Rc<RefCell<diagnostics::Recorder>>,
    /// When counting began, and when metrics were last exported (D206):
    /// monotonic milliseconds.
    metrics_started_at: f64,
    metrics_exported_at: f64,
}

/// How often the request-duration metric is exported: OpenTelemetry's default
/// metric export interval (`OTEL_METRIC_EXPORT_INTERVAL`, 60s), and at exit.
const METRICS_INTERVAL_MS: f64 = 60_000.0;

/// How a runtime exports OpenTelemetry (DECISIONS.md D89).
#[derive(Clone, Default)]
pub struct TelemetryConfig {
    /// Where payloads go. `None` disables export.
    pub sink: Option<Arc<dyn TelemetrySink>>,
    /// `service.name` on every exported resource.
    pub service_name: String,
    /// What to export. Defaults to every kind that has a trace.
    ///
    /// The filter is the *deployment's* to set and not the program's, and it is
    /// the only bound on volume: a trivial request produces about twenty spans,
    /// most of them microsecond-long pure computation, so a production exporter
    /// usually wants a `min_duration` or a `sample` rather than all of it.
    pub filter: diagnostics::Filter,
}

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

    /// The [`ModuleLoader`] refused a specifier or could not read a module.
    ///
    /// Kept whole rather than flattened to text, so the refusal's `code`
    /// survives to the guest: an `import()` refused by the import policy is
    /// `ERR_PERMISSION_DENIED` and one outside the root `ERR_JAIL_ESCAPE`, the
    /// same codes `runtime:fs` gives for the same refusals (D141).
    #[error("module loading failed: {0}")]
    Loader(es_runtime_providers::ProviderError),

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
            Error::ModuleLoad(_) | Error::Loader(_) => es_runtime_common::ExceptionClass::Error,
            Error::ImportDenied(_) => es_runtime_common::ExceptionClass::NOT_ALLOWED,
        }
    }

    fn exception_code(&self) -> Option<es_runtime_common::ErrorCode> {
        match self {
            Error::Engine(e) => e.exception_code(),
            Error::Common(e) => e.exception_code(),
            Error::ModuleLoad(_) => None,
            Error::Loader(e) => e.code(),
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
    isolates: Option<Arc<dyn WorkerHost>>,
    worker_scope: Option<Arc<dyn WorkerScope>>,
    broadcast: Option<Arc<dyn BroadcastHub>>,
    ports: Option<Arc<dyn PortHub>>,
    task_spawner: Option<Arc<dyn TaskSpawner>>,
    telemetry: Option<TelemetryConfig>,
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
            isolates: None,
            worker_scope: None,
            broadcast: None,
            ports: None,
            task_spawner: None,
            telemetry: None,
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

    /// The host `Isolate`s run on (DECISIONS D227): a [`WorkerHost`] over a
    /// pool, whose agents are built computation-only. Absent, `new Isolate()`
    /// is refused.
    pub fn with_isolates(mut self, isolates: Arc<dyn WorkerHost>) -> Self {
        self.isolates = Some(isolates);
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

    /// Adds the [`TaskSpawner`] that runs CPU-bound work off the thread driving
    /// the runtime — today, `runtime:images` pipelines (DECISIONS.md D159).
    /// Absent, that work runs inline: the answer is the same, but the event loop
    /// waits for it.
    #[must_use]
    pub fn with_task_spawner(mut self, task_spawner: Arc<dyn TaskSpawner>) -> Self {
        self.task_spawner = Some(task_spawner);
        self
    }

    fn task_spawner(&self) -> Option<Arc<dyn TaskSpawner>> {
        self.task_spawner.clone()
    }

    /// Exports spans to `sink` as OpenTelemetry, for every program this runtime
    /// runs (DECISIONS.md D89).
    ///
    /// Host-side and unconditional: the guest neither opts in nor needs a
    /// capability, which is the point — a deployment that wants traces should not
    /// have to grant its own code the power to read them or to reach the
    /// collector.
    pub fn with_telemetry(mut self, sink: Arc<dyn TelemetrySink>, config: TelemetryConfig) -> Self {
        self.telemetry = Some(TelemetryConfig {
            sink: Some(sink),
            ..config
        });
        self
    }

    fn broadcast(&self) -> Option<Arc<dyn BroadcastHub>> {
        self.broadcast.clone()
    }

    fn workers(&self) -> Option<Arc<dyn WorkerHost>> {
        self.workers.clone()
    }

    fn isolates(&self) -> Option<Arc<dyn WorkerHost>> {
        self.isolates.clone()
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
    /// The modules the runtime compiled from its own source — the `runtime:`
    /// built-ins, baked or an embedder's. Only these may import the op table
    /// (D253); a module's specifier is not proof, since a loader names modules.
    builtin_ids: std::collections::HashSet<ModuleId>,
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
    /// Whether this agent is an `Isolate` (D227), which is given the light
    /// `runtime:workers` — `host`, and nothing that reaches out.
    is_isolate: bool,
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
    /// The OTLP exporter, when the embedder installed a sink (DECISIONS.md D89).
    telemetry: Option<Telemetry>,
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
            builtin_ids: std::collections::HashSet::new(),
            entry_specifier: Rc::new(RefCell::new(None)),
            module_loader: Rc::new(RefCell::new(None)),
            handle_refs: Rc::new(Cell::new(0)),
            is_worker: providers.worker_scope.is_some(),
            is_isolate: providers
                .worker_scope
                .as_ref()
                .is_some_and(|scope| scope.is_isolate()),
            capabilities: Rc::new(Cell::new(CapabilitySet::none())),
            host_modules: HashMap::new(),
            telemetry: None,
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
        runtime.engine.eval_internal(&prelude::source())?;
        runtime
            .engine
            .eval_internal(&prelude::post_snapshot_source())?;
        runtime.wire_telemetry(&providers);
        if runtime.is_isolate {
            runtime.engine.set_denial_note(ISOLATE_DENIAL);
        }
        Ok(runtime)
    }

    /// Opens the host-side diagnostics subscription the OTLP exporter drains.
    ///
    /// `detail` is `true`: the deployment asked for telemetry, so it gets the
    /// paths, URLs and statements that make a span worth reading. This does not
    /// widen what *guest* code can see — the subscription is drained in Rust and
    /// its records never reach JS (DECISIONS.md D89).
    fn wire_telemetry(&mut self, providers: &HostProviders) {
        let Some(config) = providers.telemetry.clone() else {
            return;
        };
        if config.sink.is_none() {
            return;
        }
        let Some(recorder) = self.engine.diagnostics() else {
            return;
        };
        // Telemetry is the *deployment's* choice, so it cannot depend on the
        // program having imported `runtime:context`: without propagation a span
        // would not know which request it belongs to, and without a trace id
        // there is nothing to export at all. Both are arranged here.
        self.engine.enable_async_context();
        let mut bytes = [0u8; 16];
        if providers.entropy.fill(&mut bytes).is_ok() {
            let mut trace = String::with_capacity(32);
            for byte in bytes {
                use std::fmt::Write;
                let _ = write!(trace, "{byte:02x}");
            }
            self.engine.set_root_trace(&trace);
        }
        // Both clocks read together: the offset between them is what every
        // exported timestamp is built on.
        let clock = providers.clock.clone();
        let origin =
            otlp::TimeOrigin::new(clock.wall_ms(), clock.monotonic_micros() as f64 / 1_000.0);
        let id = recorder.borrow_mut().subscribe_to(
            config.filter.clone(),
            true,
            diagnostics::Sink::Host,
        );
        recorder.borrow_mut().enable_http_metrics();
        let now = recorder.borrow().now();
        self.telemetry = Some(Telemetry {
            config,
            origin,
            subscription: id,
            recorder,
            metrics_started_at: now,
            metrics_exported_at: now,
        });
    }

    /// Ships whatever the exporter's subscription collected this turn.
    ///
    /// One request per turn that produced anything, and none at all on a quiet
    /// one. A failure to export is the sink's to absorb: telemetry that cannot
    /// be delivered must never fail the program that produced it.
    fn export_telemetry(&mut self) {
        self.export_metrics(false);
        let Some(telemetry) = &self.telemetry else {
            return;
        };
        let batches = telemetry
            .recorder
            .borrow_mut()
            .drain_from(diagnostics::Sink::Host);
        for (id, records, _dropped) in batches {
            if id != telemetry.subscription {
                continue;
            }
            let Some(payload) =
                otlp::encode_traces(&records, &telemetry.config.service_name, telemetry.origin)
            else {
                continue;
            };
            if let Some(sink) = &telemetry.config.sink {
                // Fire and forget. The returned future is the sink's
                // acknowledgement that it has taken the payload, not that the
                // collector has it — driving delivery is the sink's own job, so
                // a slow collector cannot add latency to the request whose span
                // is being exported.
                drop(sink.export("traces", payload));
            }
        }
    }

    /// Ships `http.server.request.duration` (D206) when the interval has
    /// passed, or now when `force` — at exit. Cumulative, so each export
    /// carries everything since the start, and a lost one costs nothing.
    fn export_metrics(&mut self, force: bool) {
        let Some(telemetry) = &mut self.telemetry else {
            return;
        };
        let now = telemetry.recorder.borrow().now();
        if !force && now - telemetry.metrics_exported_at < METRICS_INTERVAL_MS {
            return;
        }
        telemetry.metrics_exported_at = now;
        let payload = {
            let recorder = telemetry.recorder.borrow();
            recorder.http_metrics().and_then(|metrics| {
                otlp::encode_metrics(
                    metrics,
                    &telemetry.config.service_name,
                    telemetry.origin,
                    telemetry.metrics_started_at,
                    now,
                )
            })
        };
        if let (Some(payload), Some(sink)) = (payload, &telemetry.config.sink) {
            drop(sink.export("metrics", payload));
        }
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
                engine.eval_internal(&prelude::source())?;
                Ok(())
            },
        )?)
    }

    /// Restores a runtime from a [`build_snapshot`](Self::build_snapshot) blob.
    ///
    /// The prelude and the op table's shells come from the snapshot, so this only
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
            builtin_ids: std::collections::HashSet::new(),
            entry_specifier: Rc::new(RefCell::new(None)),
            module_loader: Rc::new(RefCell::new(None)),
            handle_refs: Rc::new(Cell::new(0)),
            is_worker: providers.worker_scope.is_some(),
            is_isolate: providers
                .worker_scope
                .as_ref()
                .is_some_and(|scope| scope.is_isolate()),
            capabilities: Rc::new(Cell::new(CapabilitySet::none())),
            host_modules: HashMap::new(),
            telemetry: None,
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
        runtime
            .engine
            .eval_internal(&prelude::post_snapshot_source())?;
        runtime.wire_telemetry(&providers);
        if runtime.is_isolate {
            runtime.engine.set_denial_note(ISOLATE_DENIAL);
        }
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

    /// Puts the op table on the global as `__ops`, for a test that calls an op
    /// directly (D253). Tests only: the `test-hooks` feature, which only
    /// dev-dependencies enable, is how another crate's tests reach it.
    #[cfg(any(test, feature = "test-hooks"))]
    #[doc(hidden)]
    pub fn expose_ops_for_testing(&mut self) {
        self.engine.expose_ops_for_testing().expect("expose ops");
    }

    /// Registers a host op, callable from the runtime's own JavaScript as
    /// `__ops.<name>` and from nowhere else (D253).
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
        if runtime_modules::source(specifier).is_some() || specifier.starts_with("runtime:internal")
        {
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

    /// Records where each timer and async op is started, and makes
    /// `__esdev_pending_work` available to report what keeps the event loop
    /// alive — `esdev test --detect-async-leaks`. Call it before loading the
    /// entry module, so every origin is recorded.
    pub fn track_pending_work(&mut self) -> Result<()> {
        self.engine.track_pending_work()?;
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

    /// Gives this runtime's thread to another runtime made on it, until
    /// [`enter`](Self::enter): a pool thread serving many isolates leaves each
    /// after its turn (D227). Nothing else may be asked of a left runtime;
    /// dropping one is fine. Idempotent.
    ///
    /// # Panics
    ///
    /// If a runtime made or entered on this thread after this one is still
    /// entered — only the innermost can be left.
    pub fn leave(&mut self) {
        self.engine.leave();
    }

    /// Takes the thread back for this runtime after [`leave`](Self::leave).
    /// Idempotent.
    pub fn enter(&mut self) {
        self.engine.enter();
    }

    /// [`leave`](Self::leave) and [`enter`](Self::enter) without borrowing the
    /// runtime: what a pool thread wraps around a drive that owns it. Once the
    /// runtime is dropped, both do nothing. `None` for an engine with no
    /// isolate to give up.
    pub fn turn_handle(&self) -> Option<TurnHandle> {
        self.engine.turn_handle()
    }

    /// Calls the prelude hook `hook` with the namespace of `module`, once it
    /// has evaluated — how an isolate's prelude is handed the exports it
    /// serves (D227). `Ok(false)` when the hook is absent.
    pub fn deliver_module_namespace(&mut self, module: ModuleId, hook: &str) -> Result<bool> {
        Ok(self.engine.deliver_module_namespace(module, hook)?)
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
                let (target_id, newly_compiled) = if raw == es_runtime_engine::INTERNAL_OPS {
                    self.resolve_internal_ops(referrer_id)?
                } else if runtime_modules::is_builtin_scheme(&raw) {
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
                        .map_err(Error::Loader)?;
                    match self.module_map.get(&canonical) {
                        Some(&id) => (id, None),
                        None => {
                            let loaded = loader.load(&canonical).await.map_err(Error::Loader)?;
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

    /// Resolves `runtime:internal/ops`, the op table (D253), for `referrer` —
    /// which must be a module the runtime compiled from its own source. Any
    /// other importer is refused as if the module did not exist, since to it
    /// none does.
    fn resolve_internal_ops(&mut self, referrer: ModuleId) -> Result<(ModuleId, Option<String>)> {
        if !self.builtin_ids.contains(&referrer) {
            return Err(Error::ModuleLoad(format!(
                "unknown built-in module {:?}",
                es_runtime_engine::INTERNAL_OPS
            )));
        }
        if let Some(&id) = self.module_map.get(es_runtime_engine::INTERNAL_OPS) {
            return Ok((id, None));
        }
        let id = self.engine.internal_ops_module()?;
        self.module_map
            .insert(es_runtime_engine::INTERNAL_OPS.to_string(), id);
        Ok((id, None))
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
        let baked = if self.is_isolate {
            runtime_modules::isolate_source(specifier)
        } else {
            runtime_modules::source(specifier)
        };
        let source: Rc<str> = match baked {
            Some(baked) => Rc::from(baked),
            None => self.host_modules.get(specifier).cloned().ok_or_else(|| {
                Error::ModuleLoad(format!("unknown built-in module {specifier:?}"))
            })?,
        };
        // `runtime:context` is the one built-in whose *existence in the graph*
        // changes the engine: it needs the promise hook that carries a mapping
        // across a continuation, and that hook is off until something asks
        // (DECISIONS.md D88). Here, at compile rather than evaluation, so the
        // hook is already in place for every promise the module graph creates on
        // its way to running — including one created by a module that imports
        // this one and is evaluated first.
        if specifier == "runtime:context" {
            self.engine.enable_async_context();
        }
        // The op table arrives as an import the runtime adds, on the first line
        // so no line number moves (D253). The built-ins name it `__ops`.
        let source = format!(
            "import __ops from {:?}; {source}",
            es_runtime_engine::INTERNAL_OPS
        );
        let id = self.engine.compile_module(specifier, &source)?;
        self.module_map.insert(specifier.to_string(), id);
        self.builtin_ids.insert(id);
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
        // The op table is a static import of the built-ins and nothing else
        // (D253); a dynamic one is refused whoever asks.
        if specifier == es_runtime_engine::INTERNAL_OPS {
            return Err(Error::ModuleLoad(format!(
                "unknown built-in module {specifier:?}"
            )));
        }
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
            .map_err(Error::Loader)?;
        let id = match self.module_map.get(&canonical) {
            Some(&id) => id,
            None => {
                let loaded = loader.load(&canonical).await.map_err(Error::Loader)?;
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
        let remedy = if self.is_isolate {
            "an isolate is computation only and imports nothing at run time — \
             import what it needs statically, or pass it in through a host function"
        } else if self.is_worker {
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
        // Back to the root async-context mapping. No JS is on the stack here, so
        // the answer is known rather than guessed — and a callback that was
        // *terminated* mid-scope on the last tick skipped the `finally` that
        // would have restored it (DECISIONS.md D88).
        self.engine.reset_async_context();
        // Open this turn. Every span recorded below carries its number, which is
        // what makes "was this slow, or did it wait behind something?" answerable
        // (DECISIONS.md D89).
        self.engine.begin_tick();
        // Schedule timers created since the last drain (e.g. during `eval`).
        self.drain_new_timers(now_ms);

        // 1. Fire due timers, re-arming still-active repeating ones. Each
        // callback is its own task (HTML's timer initialization steps), so the
        // microtasks it queued run before the next timer does — a promise
        // reaction in one callback is never observed late by the one after it.
        let mut timers_fired = 0;
        for due in self.timers.take_due(now_ms) {
            if self.engine.fire_timer(due.id) {
                timers_fired += 1;
                self.engine.run_microtasks();
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

        // 5. Close the turn and hand each diagnostics subscriber its batch — one
        //    call into JS per subscription per tick, never one per record.
        //    Both are no-ops unless something subscribed.
        self.engine.end_tick();
        self.engine.deliver_diagnostics();
        self.export_telemetry();

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

    /// Exports whatever telemetry is still buffered and waits for it to be
    /// delivered (DECISIONS.md D89).
    ///
    /// Called once as a run winds down. Without it a short program loses its
    /// traces: export hands the payload to the sink and returns so the loop is
    /// never blocked on a collector, so at exit there is normally something
    /// still in flight — and a run that finishes quickly is often exactly the one
    /// worth having a trace of. A no-op when nothing is exporting.
    pub async fn flush_telemetry(&mut self) {
        // The last turn's records were never drained, because the drain happens
        // at the *end* of a turn and the loop has stopped.
        self.export_telemetry();
        self.export_metrics(true);
        let sink = self
            .telemetry
            .as_ref()
            .and_then(|telemetry| telemetry.config.sink.clone());
        if let Some(sink) = sink {
            sink.flush().await;
        }
    }

    /// Whether any async op, timer, or unsettled module evaluation is still
    /// outstanding.
    pub fn has_pending_work(&self) -> bool {
        self.module_eval_pending || self.has_work_besides_the_entry()
    }

    /// Whether the entry module's evaluation is waiting at a top-level `await`
    /// that nothing left can settle (D248): it is pending, and there is no op,
    /// referenced timer, import, compile or handle that could ever resolve the
    /// promise it waits on. Microtasks are drained every tick, so such a
    /// module stays pending for as long as the process lives.
    pub fn module_eval_stalled(&self) -> bool {
        // Not the keep-alive set: an unreferenced timer does not keep a
        // program alive, but the pending entry module does, and the timer can
        // still settle it — so it is waiting, not stalled (D254).
        self.module_eval_pending
            && !self.has_work_besides_the_entry()
            && !self.engine.has_unsettled_work()
    }

    /// Pending work other than the entry module's own evaluation.
    fn has_work_besides_the_entry(&self) -> bool {
        self.engine.has_pending_async_ops()
            // The engine's table rather than the schedule: it is exact about a
            // cleared timer, and knows which ones `unrefTimer` let go.
            || self.engine.has_referenced_timers()
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
mod tests;
