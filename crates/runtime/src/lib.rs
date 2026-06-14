//! The driven runtime (ARCHITECTURE.md §5; DECISIONS.md D4).
//!
//! [`Runtime`] wires host ops into the engine and exposes the **tick/poll** API
//! the embedder drives. It owns no thread and no loop of its own: one
//! [`Runtime::tick`] advances the world by one step and returns; the embedder
//! decides when to call it again. This is the exact seam Layer B replaces with
//! its scheduler.
//!
//! The runtime is built on the [`Engine`](es_runtime_engine::Engine) abstraction
//! and names **no** V8 type (DECISIONS.md D3): a second engine could be slotted
//! in without changing this crate. The V8-coupled op/promise/timer machinery
//! lives in `engine`; here we own the orchestration and the timer schedule.

// No `unsafe` in the runtime; it is confined to `engine` (ARCHITECTURE.md §7).
#![forbid(unsafe_code)]

mod base64_ops;
mod builtins;
mod crypto_ops;
mod ec_ops;
mod encoding_ops;
mod fetch_ops;
mod fs_ops;
mod prelude;
mod process_ops;
mod rsa_ops;
mod runtime_modules;
mod timer;
mod url_ops;

use std::collections::HashMap;
use std::sync::Arc;

use crate::timer::TimerQueue;

// One-stop public surface for embedders: the engine abstraction + impl, the op
// types, values, capabilities, and the provider traits — all reachable here.
pub use es_runtime_common::{Capability, CapabilitySet};
pub use es_runtime_engine::{
    AsyncOp, Engine, InterruptHandle, ModuleEvalState, ModuleId, OpDecl, OpError, OpResult,
    V8Engine, Value,
};
pub use es_runtime_providers::{
    Clock, Console, ConsoleLevel, Entropy, FileSystem, ModuleLoader, NetTransport, Process,
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
}

impl es_runtime_common::IntoException for Error {
    fn exception_class(&self) -> es_runtime_common::ExceptionClass {
        match self {
            Error::Engine(e) => e.exception_class(),
            Error::Common(e) => e.exception_class(),
            Error::ModuleLoad(_) => es_runtime_common::ExceptionClass::Error,
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
    /// Messages of promise rejections that went unhandled this tick.
    pub unhandled_rejections: Vec<String>,
    /// Whether any async op or timer remains after this tick.
    pub has_pending_work: bool,
    /// The earliest pending timer deadline (embedder ms), if any — a hint for
    /// how long the embedder may park.
    pub next_timer_deadline_ms: Option<u64>,
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
    file_system: Option<Arc<dyn FileSystem>>,
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
            file_system: None,
        }
    }

    /// Adds the [`Process`] view backing `runtime:process` (env/args/cwd/
    /// platform/exit). Capability-gated on [`Capability::Env`].
    #[must_use]
    pub fn with_process(mut self, process: Arc<dyn Process>) -> Self {
        self.process = Some(process);
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

    fn file_system(&self) -> Option<Arc<dyn FileSystem>> {
        self.file_system.clone()
    }
}

/// The embeddable runtime: an engine plus the driven loop's scheduling state.
pub struct Runtime {
    engine: Box<dyn Engine>,
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
    /// The loader used for static graph loading and dynamic `import()`. Stored
    /// (not passed per-call) so dynamic imports raised mid-execution can reach it.
    module_loader: Option<Arc<dyn ModuleLoader>>,
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
            timers: TimerQueue::default(),
            now_ms: 0,
            module_eval_pending: false,
            module_map: HashMap::new(),
            module_loader: None,
        };
        // Register the world-touching ops, then evaluate the prelude that builds
        // the pure-JS APIs on top of them (DECISIONS.md D8).
        builtins::install(runtime.engine.as_mut(), &providers)?;
        runtime.engine.eval(&prelude::source())?;
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
                builtins::install(engine, providers).map_err(|e| match e {
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
        let engine =
            V8Engine::with_snapshot_baked_ops(es_runtime_common::Limits::default(), snapshot)?;
        let mut runtime = Runtime {
            engine: Box::new(engine),
            timers: TimerQueue::default(),
            now_ms: 0,
            module_eval_pending: false,
            module_map: HashMap::new(),
            module_loader: None,
        };
        // Rebind handlers only; the engine skips the (baked) JS shells and the
        // prelude is already present in the restored context.
        builtins::install(runtime.engine.as_mut(), &providers)?;
        Ok(runtime)
    }

    /// Registers a host op, callable from JS as `globalThis.__ops.<name>`.
    pub fn register_op(&mut self, op: OpDecl) -> Result<()> {
        self.engine.register_op(op)?;
        Ok(())
    }

    /// Replaces the capability set checked before capability-gated ops dispatch
    /// (DECISIONS.md D7). Deny-by-default until granted.
    pub fn set_capabilities(&mut self, capabilities: CapabilitySet) {
        self.engine.set_capabilities(capabilities);
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
        self.module_loader = Some(loader);

        let entry_id = self.engine.compile_module(entry_specifier, entry_source)?;
        self.module_map
            .insert(entry_specifier.to_string(), entry_id);
        let resolved = self
            .build_graph(entry_id, entry_specifier.to_string())
            .await?;

        self.engine.instantiate_module(entry_id, &resolved)?;
        self.engine.evaluate_module(entry_id)?;
        self.module_eval_pending = true;
        // Anchor any timers the synchronous portion of evaluation created.
        self.drain_new_timers(self.now_ms);
        Ok(())
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
            for raw in requests {
                let (target_id, newly_compiled) = if runtime_modules::is_builtin_scheme(&raw) {
                    // `runtime:` built-ins are served by the runtime itself — no
                    // loader, no FileSystem capability (their ops are gated).
                    self.resolve_builtin(&raw)?
                } else {
                    // A file / node_modules import: the capability-gated,
                    // loader-touching path.
                    self.require_module_capability()?;
                    let loader = self.loader()?;
                    let canonical = loader
                        .resolve(&raw, &referrer_spec)
                        .await
                        .map_err(|e| Error::ModuleLoad(e.to_string()))?;
                    match self.module_map.get(&canonical) {
                        Some(&id) => (id, None),
                        None => {
                            let source = loader
                                .load(&canonical)
                                .await
                                .map_err(|e| Error::ModuleLoad(e.to_string()))?;
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
        let source = runtime_modules::source(specifier)
            .ok_or_else(|| Error::ModuleLoad(format!("unknown built-in module {specifier:?}")))?;
        let id = self.engine.compile_module(specifier, source)?;
        self.module_map.insert(specifier.to_string(), id);
        Ok((id, Some(specifier.to_string())))
    }

    /// Loads, instantiates, and begins evaluating dynamic `import()` requests
    /// raised since the last call, settling each request's promise with the
    /// module namespace (or rejecting it). Async because resolution/loading is
    /// I/O; the embedder/driver calls this each loop iteration alongside
    /// [`tick`](Self::tick). A no-op when nothing dynamic is pending.
    pub async fn process_dynamic_imports(&mut self) -> Result<()> {
        // Re-drain: linking a module evaluates it, which can synchronously raise
        // further `import()` calls; loop until none remain.
        loop {
            let pending = self.engine.take_pending_dynamic_imports();
            if pending.is_empty() {
                return Ok(());
            }
            for (reqid, specifier, referrer) in pending {
                match self.load_for_dynamic_import(&specifier, &referrer).await {
                    Ok(id) => self.engine.link_dynamic_import(reqid, id)?,
                    Err(err) => self.engine.reject_dynamic_import(reqid, &err.to_string())?,
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
    ) -> Result<ModuleId> {
        // A dynamic import() of a `runtime:` built-in (e.g. `import("runtime:process")`).
        if runtime_modules::is_builtin_scheme(specifier) {
            let (id, _) = self.resolve_builtin(specifier)?;
            let resolved = self.build_graph(id, specifier.to_string()).await?;
            self.engine.instantiate_module(id, &resolved)?;
            return Ok(id);
        }
        self.require_module_capability()?;
        let loader = self.loader()?;
        let canonical = loader
            .resolve(specifier, referrer)
            .await
            .map_err(|e| Error::ModuleLoad(e.to_string()))?;
        let id = match self.module_map.get(&canonical) {
            Some(&id) => id,
            None => {
                let source = loader
                    .load(&canonical)
                    .await
                    .map_err(|e| Error::ModuleLoad(e.to_string()))?;
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
        self.module_loader.clone().ok_or_else(|| {
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

    /// Errors unless the `FileSystem` capability needed to load modules is granted.
    fn require_module_capability(&self) -> Result<()> {
        if self.engine.capabilities().contains(Capability::FileSystem) {
            Ok(())
        } else {
            Err(es_runtime_common::Error::CapabilityDenied(Capability::FileSystem).into())
        }
    }

    /// Advances the loop by one step (ARCHITECTURE.md §5), in order:
    /// due **timers** → ready **async ops** → **microtask checkpoint** →
    /// **unhandled-rejection** collection. `now_ms` is the embedder's current
    /// time; the runtime holds no clock of its own.
    pub fn tick(&mut self, now_ms: u64) -> TickStatus {
        self.now_ms = now_ms;
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

        // 2b. Settle dynamic import() promises whose module evaluation has
        // completed (resolving with the namespace, or rejecting), so their
        // reactions run in the checkpoint below.
        self.engine.settle_dynamic_imports();

        // 3. Microtask checkpoint (promise reactions, queueMicrotask).
        self.engine.run_microtasks();
        self.drain_new_timers(now_ms);

        // 4. Collect rejections that remained unhandled.
        let unhandled_rejections = self.engine.take_unhandled_rejections();

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
            has_pending_work: self.has_pending_work(),
            next_timer_deadline_ms: self.timers.next_deadline_ms(),
        }
    }

    /// Whether any async op, timer, or unsettled module evaluation is still
    /// outstanding.
    pub fn has_pending_work(&self) -> bool {
        self.engine.has_pending_async_ops()
            || !self.timers.is_empty()
            || self.module_eval_pending
            || self.engine.has_pending_dynamic_imports()
    }

    /// Moves newly created engine timers into the schedule, anchored at `now_ms`.
    fn drain_new_timers(&mut self, now_ms: u64) {
        for (id, delay_ms, repeat) in self.engine.take_new_timers() {
            self.timers.schedule(id, now_ms, delay_ms, repeat);
        }
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
                    headers,
                    body,
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
            HostProviders::new(clock, console, net, entropy),
        )
        .expect("runtime")
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
        rt.register_op(OpDecl::r#async("answer", |_args| -> AsyncOp {
            Box::pin(async { Ok(Value::Number(42.0)) })
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
                .any(|m| m.contains("boom")),
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
    fn eval_async(rt: &mut Runtime, body: &str) -> Value {
        rt.eval(&format!(
            "globalThis.__done = false; globalThis.__result = undefined; \
             (async () => {{ {body} }})().then( \
               (r) => {{ globalThis.__result = r; globalThis.__done = true; }}, \
               (e) => {{ globalThis.__result = 'ERR:' + ((e && e.message) || e); \
                         globalThis.__done = true; }});"
        ))
        .unwrap();
        for _ in 0..200 {
            rt.tick(0);
            if rt.eval("globalThis.__done").unwrap() == Value::Bool(true) {
                break;
            }
        }
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

    /// In-JS test harness for the conformance suite: `test(name, fn)` (sync or
    /// async), `assert*` helpers, and a `__results` tally read back by the runner.
    const CONFORMANCE_HARNESS: &str = r#"
        globalThis.__results = { pass: 0, fail: 0, failures: [] };
        globalThis.__pending = [];
        globalThis.test = (name, fn) => {
          let r;
          try { r = fn(); }
          catch (e) { __results.fail++; __results.failures.push(name + ": " + ((e && e.message) || e)); return; }
          if (r && typeof r.then === "function") {
            __pending.push(r.then(
              () => { __results.pass++; },
              (e) => { __results.fail++; __results.failures.push(name + ": " + ((e && e.message) || e)); },
            ));
          } else { __results.pass++; }
        };
        globalThis.assert = (cond, msg) => { if (!cond) throw new Error(msg || "assertion failed"); };
        globalThis.assertEquals = (actual, expected, msg) => {
          if (actual !== expected) {
            throw new Error((msg ? msg + ": " : "") + `expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`);
          }
        };
        globalThis.assertThrows = (fn, name) => {
          let threw = null;
          try { fn(); } catch (e) { threw = e; }
          if (!threw) throw new Error("expected a throw, but none occurred");
          if (name && threw.name !== name) throw new Error(`expected ${name}, got ${threw.name}`);
        };
        globalThis.__await_all = () => Promise.all(__pending);
    "#;

    /// Runs every `conformance/*.js` spec-assertion file and records the
    /// pass-rate (SPEC §5 / §8). Gated on zero failures and a non-regressing
    /// count; the recorded snapshot lives in `conformance/RESULTS.md`.
    #[test]
    #[allow(clippy::print_stdout)] // reports the pass-rate under `--nocapture`
    fn conformance_suite_passes() {
        let _g = v8_guard();
        let mut rt = runtime();
        rt.eval(CONFORMANCE_HARNESS).expect("conformance harness");

        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/conformance");
        let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
            .expect("read conformance dir")
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "js"))
            .collect();
        files.sort();
        assert!(!files.is_empty(), "no conformance files found in {dir}");

        for path in &files {
            let src = std::fs::read_to_string(path).expect("read conformance file");
            rt.eval(&src)
                .unwrap_or_else(|e| panic!("loading {}: {e}", path.display()));
        }

        // Settle the async tests, then read the tallies.
        eval_async(&mut rt, "await globalThis.__await_all(); return 'done';");

        let number = |rt: &mut Runtime, expr: &str| match rt.eval(expr).unwrap() {
            Value::Number(n) => n as u32,
            other => panic!("{expr} was not a number: {other:?}"),
        };
        let pass = number(&mut rt, "__results.pass");
        let fail = number(&mut rt, "__results.fail");
        let failures = match rt.eval("__results.failures.join('\\n')").unwrap() {
            Value::String(s) => s,
            _ => String::new(),
        };

        assert_eq!(fail, 0, "conformance failures ({fail}):\n{failures}");
        // Non-regression floor; bump alongside conformance/RESULTS.md as the
        // suite grows so removed/skipped assertions are caught.
        const BASELINE: u32 = 62;
        assert!(
            pass >= BASELINE,
            "conformance pass count {pass} below baseline {BASELINE}"
        );
        println!(
            "conformance: {pass}/{} assertions passing across {} files",
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
            std::result::Result<String, es_runtime_providers::ProviderError>,
        > {
            let result = self.files.get(specifier).cloned().ok_or_else(|| {
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
        for _ in 0..200 {
            if !rt.tick(0).has_pending_work {
                break;
            }
        }
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
            ModuleEvalState::Failed(message) => assert!(message.contains("nope"), "{message}"),
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
        assert!(matches!(err, Error::Common(_)), "got {err:?}");
    }

    #[test]
    fn self_contained_module_runs_without_capability() {
        let _g = v8_guard();
        let mut rt = runtime(); // no capabilities granted
        let loader = MapLoader::new(&[]);
        // No imports → the loader is never consulted → no capability needed.
        block_on(rt.load_module_source(ENTRY, "globalThis.ok = 5;", loader.clone())).expect("load");
        for _ in 0..200 {
            if !rt.tick(0).has_pending_work {
                break;
            }
        }
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
            ModuleEvalState::Failed(message) => assert!(message.contains("dep boom"), "{message}"),
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
}
