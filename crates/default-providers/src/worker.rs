//! The reference [`WorkerHost`]: one OS thread and one isolate per worker.
//!
//! # Why a thread rather than a task
//!
//! `V8Engine` is `!Send` by V8's own threading model, so a worker's runtime has
//! to be *constructed on* the thread that will drive it — which is why the
//! factory below is called inside the spawned thread rather than before it.
//! Deno's `WebWorker` is built the same way, for the same reason.
//!
//! # Where the loop comes from
//!
//! The worker thread runs a single-threaded tokio runtime and drives its
//! `Runtime` with the ordinary [`Driver`](crate::Driver). The runtime crate
//! still owns no loop; this file is the only place a thread is created, and it
//! is the seam a scheduler-backed host replaces wholesale.
//!
//! # Or a pool
//!
//! [`ThreadWorkerHost::on_pool`] is the same host over an [`IsolatePool`]: a
//! few threads, each driving many agents on one single-threaded tokio runtime
//! (D227). An agent is built on the pool thread it is given and never leaves
//! it; the thread enters its isolate for each poll of its drive and leaves it
//! after, so the agents on a thread take turns. No `Locker` is involved,
//! because no isolate is ever touched by a second thread.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use es_runtime::{ModuleLoader, Runtime};
use es_runtime_common::UncaughtError;

use crate::DriveFailure;
use es_runtime_providers::{
    BoxFuture, ProviderError, WorkerHost, WorkerIncoming, WorkerScope, WorkerSpec,
};
use tokio::sync::{Mutex, mpsc};

/// A worker thread's stack: 8 MiB, as the main thread has (DECISIONS D189).
/// Address space, committed only as it is used.
const WORKER_STACK: usize = 8 * 1024 * 1024;

/// Builds the runtime for one worker agent, **on that worker's own thread**.
///
/// Supplied by the embedder because only it knows the snapshot, the provider
/// set and the module loader — and because keeping the choice out here is what
/// lets a second engine implementation serve workers without this file
/// changing.
///
/// The returned loader backs dynamic `import()` from inside the worker, which
/// runs under the worker's own (narrowed) capabilities, unlike the static graph.
pub trait WorkerRuntimeFactory: Send + Sync {
    /// Constructs the worker's runtime and the loader it will use.
    fn build(
        &self,
        spec: &WorkerSpec,
        scope: Arc<dyn WorkerScope>,
    ) -> Result<(Runtime, Arc<dyn ModuleLoader>), ProviderError>;
}

impl<F> WorkerRuntimeFactory for F
where
    F: Fn(
            &WorkerSpec,
            Arc<dyn WorkerScope>,
        ) -> Result<(Runtime, Arc<dyn ModuleLoader>), ProviderError>
        + Send
        + Sync,
{
    fn build(
        &self,
        spec: &WorkerSpec,
        scope: Arc<dyn WorkerScope>,
    ) -> Result<(Runtime, Arc<dyn ModuleLoader>), ProviderError> {
        self(spec, scope)
    }
}

/// The `WorkerScope` handed to a worker's own runtime: its half of the two
/// channels, plus the flag `self.close()` sets.
struct ThreadScope {
    name: String,
    /// Whether this agent is an `Isolate` (D227).
    isolate: bool,
    /// The entry module's absolute URL, which is what `location` reports.
    url: String,
    to_parent: mpsc::UnboundedSender<WorkerIncoming>,
    // `Arc`, not a plain field: `BoxFuture` is `'static`, so a future returned
    // from `&self` cannot borrow — it has to own a handle.
    from_parent: Arc<Mutex<mpsc::UnboundedReceiver<Vec<u8>>>>,
    closed: Arc<AtomicBool>,
    /// Wakes a `recv` that is *already* parked. The flag alone only stops the
    /// next one from parking, which is not enough: `self.close()` is normally
    /// called from a callback while the pump sits waiting for the message that
    /// will never come, and the worker would then never finish.
    closing: Arc<tokio::sync::Notify>,
    /// Messages this worker has sent to its parent and the parent has not yet
    /// taken — `WorkerScope::queued`.
    outbound: Arc<AtomicUsize>,
    /// The mirror: messages the parent has posted and this worker has not yet
    /// taken. Shared with the parent's `Live`, which increments as it posts.
    inbound: Arc<AtomicUsize>,
}

impl WorkerScope for ThreadScope {
    fn name(&self) -> String {
        self.name.clone()
    }

    fn url(&self) -> String {
        self.url.clone()
    }

    fn is_isolate(&self) -> bool {
        self.isolate
    }

    fn post(&self, message: Vec<u8>) -> Result<(), ProviderError> {
        // A send failure means the parent is gone. That is not the worker's
        // error to raise — it is about to be terminated anyway — so the message
        // is dropped, exactly as a browser drops one to a collected parent.
        self.outbound.fetch_add(1, Ordering::SeqCst);
        if self
            .to_parent
            .send(WorkerIncoming::Message(message))
            .is_err()
        {
            self.outbound.fetch_sub(1, Ordering::SeqCst);
        }
        Ok(())
    }

    fn queued(&self) -> usize {
        self.outbound.load(Ordering::SeqCst)
    }

    fn recv(&self) -> BoxFuture<Result<Option<Vec<u8>>, ProviderError>> {
        let inbox = self.from_parent.clone();
        let closed = self.closed.clone();
        let closing = self.closing.clone();
        let inbound = self.inbound.clone();
        Box::pin(async move {
            if closed.load(Ordering::SeqCst) {
                return Ok(None);
            }
            let mut inbox = inbox.lock().await;
            let taken = tokio::select! {
                message = inbox.recv() => message,
                () = closing.notified() => None,
            };
            if taken.is_some() {
                inbound.fetch_sub(1, Ordering::SeqCst);
            }
            Ok(taken)
        })
    }

    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        // `notify_waiters` rather than `notify_one`: a permit stored for a
        // future waiter is pointless here, since the flag above already turns
        // the next `recv` back immediately.
        self.closing.notify_waiters();
    }

    fn report_error(&self, error: UncaughtError) {
        let _ = self.to_parent.send(WorkerIncoming::Error { error });
        // Ending the agent is half the contract — see the trait's docs. The
        // drive loop reads the flag after the current tick, so nothing further
        // of the worker's runs, and the `Closed` its thread sends on the way
        // out is what tells the parent the agent is gone.
        self.close();
    }
}

/// The scope of an agent built before anyone asked for it (`Isolate.prepare`,
/// D227). A runtime's ops capture its scope when it is built, so a prepared
/// one is built with this, and the real channel is bound when a spawn claims
/// it. Before that nothing of the agent's runs that could use it: its receive
/// pump waits for the binding, and its name and URL read empty.
#[derive(Clone)]
struct LateScope(Arc<LateInner>);

struct LateInner {
    bound: std::sync::OnceLock<Arc<ThreadScope>>,
    binding: tokio::sync::Notify,
}

impl LateScope {
    fn new() -> Self {
        LateScope(Arc::new(LateInner {
            bound: std::sync::OnceLock::new(),
            binding: tokio::sync::Notify::new(),
        }))
    }

    fn bind(&self, scope: Arc<ThreadScope>) {
        let _ = self.0.bound.set(scope);
        self.0.binding.notify_waiters();
    }

    fn bound(&self) -> Option<&Arc<ThreadScope>> {
        self.0.bound.get()
    }
}

impl WorkerScope for LateScope {
    fn name(&self) -> String {
        self.bound().map(|s| s.name()).unwrap_or_default()
    }

    fn url(&self) -> String {
        self.bound().map(|s| s.url()).unwrap_or_default()
    }

    fn is_isolate(&self) -> bool {
        true
    }

    fn post(&self, message: Vec<u8>) -> Result<(), ProviderError> {
        self.bound().map_or(Ok(()), |s| s.post(message))
    }

    fn queued(&self) -> usize {
        self.bound().map_or(0, |s| s.queued())
    }

    fn recv(&self) -> BoxFuture<Result<Option<Vec<u8>>, ProviderError>> {
        if let Some(scope) = self.bound() {
            return scope.recv();
        }
        let inner = self.0.clone();
        Box::pin(async move {
            loop {
                // Registered before the check, so a binding in between wakes it.
                let woken = inner.binding.notified();
                if let Some(scope) = inner.bound.get() {
                    return scope.recv().await;
                }
                woken.await;
            }
        })
    }

    fn close(&self) {
        if let Some(scope) = self.bound() {
            scope.close();
        }
    }

    fn report_error(&self, error: UncaughtError) {
        if let Some(scope) = self.bound() {
            scope.report_error(error);
        }
    }
}

/// One live worker, from the parent's side.
struct Live {
    /// `Option`, so termination can *drop* it: closing the channel is what
    /// wakes a worker parked waiting for its next message.
    ///
    /// A std mutex, because `post` is synchronous and only ever swaps the
    /// handle in or out — never held across an await.
    to_worker: std::sync::Mutex<Option<mpsc::UnboundedSender<Vec<u8>>>>,
    /// Messages posted to this worker and not yet taken by it — the depth
    /// `WorkerHost::queued` reports. Shared with the worker's own scope, which
    /// decrements as it receives.
    inbound: Arc<AtomicUsize>,
    /// The other direction: messages this worker has sent that this agent has
    /// not yet taken. Shared with the worker's scope, which increments as it
    /// posts and reads it back as `WorkerScope::queued`; the decrement belongs
    /// here because the receiving end is what knows when one is taken.
    outbound: Arc<AtomicUsize>,
    from_worker: Mutex<mpsc::UnboundedReceiver<WorkerIncoming>>,
    /// V8's cross-thread interrupt for this worker's isolate — how
    /// `terminate()` stops an agent that is not cooperating. Available only
    /// once the thread has built its runtime, so a terminate that arrives
    /// before then falls back to the flag below.
    interrupt: Mutex<Option<es_runtime::InterruptHandle>>,
    /// Set by `terminate` and by `self.close()`; the worker's loop checks it.
    closed: Arc<AtomicBool>,
    /// How to wait for the agent to be over: its thread, or its pool task.
    // A std mutex: only ever held to swap the handle in or out, never across an
    // await.
    join: std::sync::Mutex<Option<Ending>>,
    /// The workers this one started. HTML terminates a worker's own workers
    /// along with it, and ids are otherwise flat, so this is the only record of
    /// who belongs to whom.
    ///
    /// Pruned as each child retires — see [`retire`]. Left to grow it was a slow
    /// leak in exactly the shape a job queue has: a supervisor that starts one
    /// worker per job holds an id for every job it has ever run, and walks all
    /// of them on each `terminate`.
    children: std::sync::Mutex<Vec<u64>>,
    /// The worker that started this one, so retiring it can unhook it from that
    /// worker's `children`. `None` for a worker the driver agent started — the
    /// root has no entry in the registry.
    parent: Option<u64>,
}

/// What a finished agent leaves to wait on.
enum Ending {
    /// Its own OS thread, joined.
    Thread(std::thread::JoinHandle<()>),
    /// Its task on a pool thread, which says when it is done.
    Task(tokio::sync::oneshot::Receiver<()>),
}

impl Ending {
    async fn wait(self) {
        match self {
            // Joining blocks, so it goes to the blocking pool rather than
            // stalling the caller's reactor.
            Ending::Thread(join) => {
                let _ = tokio::task::spawn_blocking(move || join.join()).await;
            }
            Ending::Task(done) => {
                let _ = done.await;
            }
        }
    }
}

/// A [`WorkerHost`] that runs each agent on its own OS thread — or, built with
/// [`on_pool`](Self::on_pool), on a thread of an [`IsolatePool`].
pub struct ThreadWorkerHost {
    factory: Arc<dyn WorkerRuntimeFactory>,
    registry: Arc<std::sync::Mutex<HashMap<u64, Arc<Live>>>>,
    /// Which agent is running on which thread.
    ///
    /// `spawn` carries no caller identity — `WorkerSpec` says *what* to start,
    /// not who asked — but one agent per thread is this host's whole model, so
    /// the calling thread is exactly the calling agent. The driver agent's
    /// thread is simply absent from the map, which is what makes it the root.
    agent_threads: Arc<std::sync::Mutex<HashMap<std::thread::ThreadId, u64>>>,
    next_id: AtomicU64,
    /// Ceiling on live workers. A worker costs a thread and an isolate heap, so
    /// an unbounded `new Worker()` in a loop is a way to exhaust both — the
    /// same reasoning as the child-process provider's `max_children`.
    max_workers: usize,
    /// Where agents run: `None` for a thread each, or the pool they share.
    pool: Option<Arc<IsolatePool>>,
}

impl ThreadWorkerHost {
    /// Default ceiling on concurrently live workers.
    pub const DEFAULT_MAX_WORKERS: usize = 64;

    /// Builds a host that constructs each worker's runtime with `factory`.
    pub fn new(factory: Arc<dyn WorkerRuntimeFactory>) -> Self {
        ThreadWorkerHost {
            factory,
            registry: Arc::new(std::sync::Mutex::new(HashMap::new())),
            agent_threads: Arc::new(std::sync::Mutex::new(HashMap::new())),
            next_id: AtomicU64::new(1),
            max_workers: Self::DEFAULT_MAX_WORKERS,
            pool: None,
        }
    }

    /// Default ceiling on concurrently live agents on a pool. Far above a
    /// thread host's, because an agent there costs an isolate heap and no
    /// thread; it still exists, so a loop of `new Isolate()` ends in an error
    /// rather than in exhausted memory.
    pub const DEFAULT_MAX_POOLED: usize = 10_000;

    /// Builds a host whose agents share `pool`'s threads instead of having one
    /// each. The agents `factory` builds must not block a thread: a blocked
    /// agent holds up every other agent on its pool thread.
    pub fn on_pool(factory: Arc<dyn WorkerRuntimeFactory>, pool: Arc<IsolatePool>) -> Self {
        ThreadWorkerHost {
            max_workers: Self::DEFAULT_MAX_POOLED,
            pool: Some(pool),
            ..Self::new(factory)
        }
    }

    /// Replaces the ceiling on concurrently live workers.
    #[must_use]
    pub fn with_max_workers(mut self, max: usize) -> Self {
        self.max_workers = max;
        self
    }

    fn live(&self, id: u64) -> Option<Arc<Live>> {
        self.registry.lock().ok()?.get(&id).cloned()
    }
}

impl WorkerHost for ThreadWorkerHost {
    fn spawn(&self, spec: WorkerSpec) -> BoxFuture<Result<u64, ProviderError>> {
        let (to_worker, worker_inbox) = mpsc::unbounded_channel::<Vec<u8>>();
        let (to_parent, parent_inbox) = mpsc::unbounded_channel::<WorkerIncoming>();
        let closed = Arc::new(AtomicBool::new(false));
        // One counter per direction, each shared by the two ends of its queue:
        // whoever posts increments, whoever receives decrements.
        let inbound = Arc::new(AtomicUsize::new(0));
        let outbound = Arc::new(AtomicUsize::new(0));

        // Whoever is on this thread is the agent doing the spawning; absent
        // means the driver agent, which is nobody's child.
        let parent = self
            .agent_threads
            .lock()
            .ok()
            .and_then(|threads| threads.get(&std::thread::current().id()).copied());

        let live = Arc::new(Live {
            to_worker: std::sync::Mutex::new(Some(to_worker)),
            from_worker: Mutex::new(parent_inbox),
            interrupt: Mutex::new(None),
            closed: closed.clone(),
            join: std::sync::Mutex::new(None),
            children: std::sync::Mutex::new(Vec::new()),
            parent,
            inbound: inbound.clone(),
            outbound: outbound.clone(),
        });

        let id = {
            let Ok(mut workers) = self.registry.lock() else {
                return Box::pin(async {
                    Err(ProviderError::Other("worker registry poisoned".into()))
                });
            };
            if workers.len() >= self.max_workers {
                let (live, max) = (workers.len(), self.max_workers);
                return Box::pin(async move {
                    Err(ProviderError::Other(format!(
                        "too many workers: {live} already running (limit {max})"
                    )))
                });
            }
            let id = self.next_id.fetch_add(1, Ordering::SeqCst);
            workers.insert(id, live.clone());
            if let Some(parent) = parent
                && let Some(parent) = workers.get(&parent)
                && let Ok(mut children) = parent.children.lock()
            {
                children.push(id);
            }
            id
        };

        let factory = self.factory.clone();
        let scope = Arc::new(ThreadScope {
            name: spec.name.clone(),
            isolate: spec.isolate,
            url: spec.specifier.clone(),
            to_parent: to_parent.clone(),
            from_parent: Arc::new(Mutex::new(worker_inbox)),
            closed: closed.clone(),
            closing: Arc::new(tokio::sync::Notify::new()),
            outbound: outbound.clone(),
            inbound: inbound.clone(),
        });

        let thread_live = live.clone();
        let registry = self.registry.clone();

        if let Some(pool) = &self.pool {
            let (done, ending) = tokio::sync::oneshot::channel();
            if let Ok(mut slot) = live.join.lock() {
                *slot = Some(Ending::Task(ending));
            }
            let job: Job = Box::new(move || {
                Box::pin(async move {
                    run_agent(
                        &factory,
                        spec,
                        scope,
                        &to_parent,
                        &thread_live,
                        &closed,
                        &registry,
                        true,
                    )
                    .await;
                    let _ = done.send(());
                })
            });
            return match pool.run(job, true) {
                Ok(()) => Box::pin(async move { Ok(id) }),
                Err(err) => {
                    retire(&self.registry, id);
                    Box::pin(async move { Err(err) })
                }
            };
        }

        let agent_threads = self.agent_threads.clone();
        let thread = std::thread::Builder::new()
            // The same stack as the main thread on every OS (D189), rather
            // than Rust's 2 MiB default: a worker runs the same code.
            .stack_size(WORKER_STACK)
            .name(if spec.name.is_empty() {
                format!("worker-{id}")
            } else {
                spec.name.clone()
            })
            .spawn(move || {
                if let Ok(mut threads) = agent_threads.lock() {
                    threads.insert(std::thread::current().id(), id);
                }
                run_worker(
                    &factory,
                    spec,
                    scope,
                    &to_parent,
                    &thread_live,
                    &closed,
                    &registry,
                );
                if let Ok(mut threads) = agent_threads.lock() {
                    threads.remove(&std::thread::current().id());
                }
            });

        match thread {
            Ok(handle) => {
                if let Ok(mut slot) = live.join.lock() {
                    *slot = Some(Ending::Thread(handle));
                }
                Box::pin(async move { Ok(id) })
            }
            Err(err) => {
                // Registered a moment ago, so undo both halves of that.
                retire(&self.registry, id);
                Box::pin(async move { Err(ProviderError::from_io("worker thread", &err)) })
            }
        }
    }

    fn post(&self, id: u64, message: Vec<u8>) -> Result<(), ProviderError> {
        // A message to a worker that has already finished is dropped, not an
        // error: the spec has no delivery guarantee, and racing a `close()` is
        // ordinary rather than a fault in the sender.
        let Some(live) = self.live(id) else {
            return Ok(());
        };
        let Ok(sender) = live.to_worker.lock() else {
            return Ok(());
        };
        if let Some(sender) = sender.as_ref() {
            live.inbound.fetch_add(1, Ordering::SeqCst);
            if sender.send(message).is_err() {
                live.inbound.fetch_sub(1, Ordering::SeqCst);
            }
        }
        Ok(())
    }

    fn queued(&self, id: u64) -> usize {
        self.live(id)
            .map_or(0, |live| live.inbound.load(Ordering::SeqCst))
    }

    fn recv(&self, id: u64) -> BoxFuture<Result<Option<WorkerIncoming>, ProviderError>> {
        let live = self.live(id);
        // Cloned rather than borrowed: the future is `'static`, and it has to
        // reach the registry to retire the worker below.
        let registry = self.registry.clone();
        Box::pin(async move {
            let Some(live) = live else { return Ok(None) };
            let event = live.from_worker.lock().await.recv().await;
            if matches!(event, Some(WorkerIncoming::Message(_))) {
                live.outbound.fetch_sub(1, Ordering::SeqCst);
            }

            // A worker that ended on its own is retired here rather than
            // waiting for a `terminate()` that may never come. Without this it
            // would stay "live" forever, and `has_live_workers` — which the
            // embedder's loop reads to decide the process still has work —
            // would never go quiet.
            if matches!(event, Some(WorkerIncoming::Closed) | None) {
                retire(&registry, id);
                let ending = live.join.lock().ok().and_then(|mut slot| slot.take());
                if let Some(ending) = ending {
                    ending.wait().await;
                }
            }
            Ok(event)
        })
    }

    fn terminate(&self, id: u64) -> BoxFuture<Result<(), ProviderError>> {
        // The whole subtree: HTML's "terminate a worker" destroys the global
        // scope, and destroying it terminates the workers started from it. Left
        // running, a nested worker is unreachable — its parent is gone — and
        // still holds the process open, since a live worker is a reason not to
        // exit.
        let subtree = take_subtree(&self.registry, id);
        Box::pin(async move {
            terminate_all(&subtree).await;
            Ok(())
        })
    }

    /// Builds agents ahead of time on the pool, each left waiting on the
    /// thread it was built on until a spawn there claims it (D227). Only a
    /// pool prepares: a thread host's agent is built on a thread that does not
    /// exist until the spawn.
    fn prepared(&self) -> usize {
        self.pool.as_ref().map_or(0, |pool| pool.prepared())
    }

    fn prepare(
        &self,
        template: WorkerSpec,
        count: usize,
    ) -> BoxFuture<Result<usize, ProviderError>> {
        let Some(pool) = self.pool.clone() else {
            return Box::pin(async { Ok(0) });
        };
        // Exactly `count`: how many warm isolates are worth their memory is the
        // caller's decision, bounded by the runtime against the process's
        // memory limit before it gets here (D250), never clamped silently.
        let mut built = Vec::new();
        for _ in 0..count {
            let (done, finished) = tokio::sync::oneshot::channel::<bool>();
            let factory = self.factory.clone();
            let template = template.clone();
            let job: Job = Box::new(move || {
                Box::pin(async move {
                    let _ = done.send(prepare_here(&factory, &template));
                })
            });
            match pool.prepare_on(job) {
                Ok(()) => built.push(finished),
                Err(err) => return Box::pin(async move { Err(err) }),
            }
        }
        Box::pin(async move {
            for finished in built {
                let _ = finished.await;
            }
            Ok(pool.prepared())
        })
    }
}

/// One agent built ahead of time, waiting on its pool thread.
struct Prepared {
    runtime: Runtime,
    loader: Arc<dyn ModuleLoader>,
    scope: LateScope,
    limits: es_runtime_common::Limits,
}

thread_local! {
    /// The agents prepared on this pool thread, newest last.
    static PREPARED: std::cell::RefCell<Vec<Prepared>> = const { std::cell::RefCell::new(Vec::new()) };
    /// This pool thread's count of them, shared with the pool for placement.
    static PREPARED_COUNT: std::cell::OnceCell<Arc<AtomicUsize>> = const { std::cell::OnceCell::new() };
}

/// Builds one agent from `template` on this pool thread and leaves it waiting.
fn prepare_here(factory: &Arc<dyn WorkerRuntimeFactory>, template: &WorkerSpec) -> bool {
    let scope = LateScope::new();
    let Ok((mut runtime, loader)) = factory.build(template, Arc::new(scope.clone())) else {
        PREPARED_COUNT.with(|c| c.get().map(|c| c.fetch_sub(1, Ordering::SeqCst)));
        return false;
    };
    runtime.leave();
    PREPARED.with(|ready| {
        ready.borrow_mut().push(Prepared {
            runtime,
            loader,
            scope,
            limits: template.limits,
        });
    });
    true
}

/// A prepared agent built with exactly `limits`, if this pool thread has one.
/// Limits are fixed when an isolate is made, so only an exact match will do.
fn take_prepared(limits: &es_runtime_common::Limits) -> Option<Prepared> {
    let taken = PREPARED.with(|ready| {
        let mut ready = ready.borrow_mut();
        let at = ready.iter().rposition(|p| p.limits == *limits)?;
        Some(ready.swap_remove(at))
    })?;
    PREPARED_COUNT.with(|c| c.get().map(|c| c.fetch_sub(1, Ordering::SeqCst)));
    Some(taken)
}

/// Retires one worker: out of the registry, and out of its parent's child list.
///
/// Both halves, always. Leaving the id behind in `children` cost nothing per
/// worker and everything over time — a supervisor that starts one worker per
/// job never terminates, so nothing ever pruned it, and the vec grew for the
/// life of the process while every `terminate` walked the dead entries.
///
/// The stale ids were never *wrong*: `next_id` only counts up, so one can never
/// name a later worker. That is what made this a leak rather than a bug, and
/// what let it go unnoticed.
fn retire(registry: &std::sync::Mutex<HashMap<u64, Arc<Live>>>, id: u64) {
    let Ok(mut workers) = registry.lock() else {
        return;
    };
    let Some(live) = workers.remove(&id) else {
        return;
    };
    unhook(&workers, &live, id);
}

/// Removes `id` from its parent's child list, if it had a parent and that
/// parent is still around. Expects the registry lock to be held — the same
/// order `take_subtree` uses, so the two can never deadlock against each other.
fn unhook(workers: &HashMap<u64, Arc<Live>>, live: &Live, id: u64) {
    let Some(parent) = live.parent else {
        return;
    };
    if let Some(parent) = workers.get(&parent)
        && let Ok(mut children) = parent.children.lock()
    {
        children.retain(|&child| child != id);
    }
}

/// Removes `id` and everything descended from it, returning them parent-first.
///
/// Removed under one lock, so a concurrent `terminate` of the same subtree gets
/// the empty half rather than a second chance to join a thread already being
/// joined. Nothing is awaited while the lock is held.
fn take_subtree(registry: &std::sync::Mutex<HashMap<u64, Arc<Live>>>, id: u64) -> Vec<Arc<Live>> {
    let Ok(mut workers) = registry.lock() else {
        return Vec::new();
    };
    let mut taken = Vec::new();
    let mut pending = vec![id];
    while let Some(next) = pending.pop() {
        let Some(live) = workers.remove(&next) else {
            continue;
        };
        if let Ok(children) = live.children.lock() {
            pending.extend(children.iter().copied());
        }
        // Only the root needs unhooking: every deeper one's parent is being
        // removed in this same walk, so its child list is about to be dropped
        // whole. The root's parent is the one that survives.
        if next == id {
            unhook(&workers, &live, next);
        }
        taken.push(live);
    }
    taken
}

/// Tells one worker to stop, without waiting for it. Interrupt first, then
/// close its channel: the flag alone only stops a *cooperative* agent, and a
/// worker spinning in a synchronous loop or parked in `Atomics.wait` needs V8's
/// own termination — which is exactly what `InterruptHandle` delivers
/// cross-thread.
async fn stop_live(live: &Live) {
    live.closed.store(true, Ordering::SeqCst);
    if let Some(handle) = live.interrupt.lock().await.as_ref() {
        handle.terminate();
    }
    // Drop the sender: closing the channel wakes a worker parked waiting for
    // its next message, which the interrupt above does not reach.
    if let Ok(mut sender) = live.to_worker.lock() {
        sender.take();
    }
}

/// Waits for a stopped worker's thread, or pool task, to end.
async fn join_live(live: &Live) {
    let ending = live.join.lock().ok().and_then(|mut slot| slot.take());
    if let Some(ending) = ending {
        ending.wait().await;
    }
}

/// Stops a whole set of workers and waits for all of them.
///
/// Every one is told to stop **before** any of them is joined, and the order
/// matters: a parent's loop holds an outstanding receive on each child, so it
/// cannot finish until that child's channel closes. Stopping the parent and
/// then joining it before touching the child is a deadlock — the parent waits
/// for the child, and the join waits for the parent.
async fn terminate_all(workers: &[Arc<Live>]) {
    for live in workers {
        stop_live(live).await;
    }
    for live in workers {
        join_live(live).await;
    }
}

/// A worker whose isolate hit its heap ceiling, worded so the parent can see
/// both what happened and the number it happened against.
///
/// Named like Node's `ERR_WORKER_OUT_OF_MEMORY` so the two are recognisably the
/// same condition; unlike Node's, the limit itself is in the message, because
/// the whole point of a per-worker ceiling is that it differs per worker.
fn out_of_memory(runtime: &Runtime) -> UncaughtError {
    let limit = runtime.limits().heap_limit_bytes;
    let ceiling = match limit {
        Some(bytes) => format!("{}MB", bytes / (1024 * 1024)),
        None => "the host's".to_string(),
    };
    UncaughtError::new(
        "ERR_WORKER_OUT_OF_MEMORY",
        format!("worker terminated: it reached its {ceiling} memory limit"),
        String::new(),
    )
}

/// Words a host-side failure, upgrading it to an out-of-memory report when that
/// is what actually stopped the agent.
///
/// Worth the check because the two are indistinguishable in the error itself: a
/// V8 that refuses to start evaluating gives the same "could not start" either
/// way, and only the heap guard's latch says which it was.
fn describe(runtime: &Runtime, message: String) -> UncaughtError {
    if runtime.heap_limit_exceeded() {
        return out_of_memory(runtime);
    }
    UncaughtError::from_message(message)
}

/// Hands one failure the driver surfaced to the agent's own
/// [`WorkerScope::report_error`], which is where the policy lives.
///
/// The two routes a failure can take into here are the two the guest cannot
/// intercept for itself: an exception escaping a host-invoked callback, and a
/// rejection nothing claimed. The third route — an exception thrown by an event
/// listener — is caught by `dispatchEvent` and reported from the prelude, which
/// calls `worker_self_fail` and lands in the same place.
///
/// Both arms pass the failure through unchanged. A rejection reason is not
/// re-worded as "unhandled rejection: …" on the way: the parent is usually a
/// supervisor, and prefixing the message would have buried the one thing it
/// reads — an `error.name` of `RangeError` says far more than the prose did.
fn fail_agent(scope: &ThreadScope, failure: DriveFailure) {
    match failure {
        DriveFailure::UncaughtError(error) | DriveFailure::UnhandledRejection(error) => {
            scope.report_error(error);
        }
    }
}

/// The worker thread's body: a single-threaded tokio runtime of its own, and
/// the agent driven on it.
fn run_worker(
    factory: &Arc<dyn WorkerRuntimeFactory>,
    spec: WorkerSpec,
    scope: Arc<ThreadScope>,
    to_parent: &mpsc::UnboundedSender<WorkerIncoming>,
    live: &Arc<Live>,
    closed: &Arc<AtomicBool>,
    registry: &std::sync::Mutex<HashMap<u64, Arc<Live>>>,
) {
    // A single-threaded tokio runtime: this agent's own reactor, so its timers
    // and sockets are driven here rather than on the parent's.
    let tokio_runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(err) => {
            let _ = to_parent.send(WorkerIncoming::Error {
                error: UncaughtError::from_message(format!(
                    "worker runtime could not start: {err}"
                )),
            });
            let _ = to_parent.send(WorkerIncoming::Closed);
            return;
        }
    };
    tokio_runtime.block_on(run_agent(
        factory, spec, scope, to_parent, live, closed, registry, false,
    ));
}

/// One agent's whole life: build the runtime, load under the parent's
/// authority, narrow, evaluate, drive, and report how it ended.
///
/// `pooled` says the agent shares its thread with others (D227): its isolate
/// is left as soon as it is built, and entered only while its drive is being
/// polled, so the agents on a pool thread take turns. Nothing between the build
/// and the drive may await with the isolate still entered — another agent's
/// turn could start in that gap.
#[allow(clippy::too_many_arguments)]
async fn run_agent(
    factory: &Arc<dyn WorkerRuntimeFactory>,
    spec: WorkerSpec,
    scope: Arc<ThreadScope>,
    to_parent: &mpsc::UnboundedSender<WorkerIncoming>,
    live: &Arc<Live>,
    closed: &Arc<AtomicBool>,
    registry: &std::sync::Mutex<HashMap<u64, Arc<Live>>>,
    pooled: bool,
) {
    let report = |error: UncaughtError| {
        let _ = to_parent.send(WorkerIncoming::Error { error });
    };

    // A pool thread may hold an agent prepared for exactly these limits; then
    // it is this one, with its channel bound now.
    let claimed = if pooled && spec.isolate {
        take_prepared(&spec.limits)
    } else {
        None
    };
    let built = match claimed {
        Some(prepared) => {
            prepared.scope.bind(scope.clone());
            Ok((prepared.runtime, prepared.loader))
        }
        None => factory.build(&spec, scope.clone()),
    };
    let (mut runtime, loader) = match built {
        Ok(built) => built,
        Err(err) => {
            report(UncaughtError::from_message(format!(
                "worker could not be created: {err}"
            )));
            let _ = to_parent.send(WorkerIncoming::Closed);
            return;
        }
    };
    let turn = if pooled {
        runtime.leave();
        runtime.turn_handle()
    } else {
        None
    };
    // An isolate's CPU budget a turn (D227), enforced only where turns exist.
    let budget = match (pooled, spec.cpu_time_ms) {
        (true, Some(ms)) => Some(es_runtime::CpuBudget::new(ms, runtime.interrupt_handle())),
        _ => None,
    };
    let isolate = spec.isolate;

    // Publish the interrupt handle now, so a `terminate()` racing startup can
    // still stop this isolate rather than waiting out its first long task.
    *live.interrupt.lock().await = Some(runtime.interrupt_handle());

    // Owns the runtime, so the runtime is dropped inside it — on a pool, while
    // its isolate is entered for that last poll.
    let drive = async move {
        // Phase 1 — load and link the static graph under the *parent's*
        // authority. No guest code runs during instantiation, so nothing the
        // worker's author wrote executes under the wider set.
        runtime.set_capabilities(spec.load_capabilities);
        let entry = runtime
            .instantiate_module_source(&spec.specifier, &spec.source, loader)
            .await;

        // Phase 2 — narrow to the worker's own set before a single line of it
        // runs. Unconditional: a failed load must not leave the wider set in
        // place for whatever runs next.
        runtime.set_capabilities(spec.capabilities);

        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => return Err(describe(&runtime, err.to_string())),
        };
        if let Err(err) = runtime.begin_evaluation(entry) {
            return Err(describe(&runtime, err.to_string()));
        }

        let clock: Arc<dyn es_runtime_providers::Clock> = Arc::new(crate::SystemClock::new());
        let timers: Arc<dyn es_runtime_providers::Timers> = Arc::new(crate::TokioTimers);

        // Two stages, because a worker with an `onmessage` never reaches
        // quiescence — its receive pump is a deliberately outstanding op, which
        // is what keeps it alive between messages. Waiting for the drive to
        // return would mean an entry module that throws is not reported to the
        // parent until the worker is terminated.
        //
        // Stage one drives to the entry module settling, and does it *without* a
        // failure sink: a module that throws at its top level rejects its own
        // evaluation, which surfaces as an unhandled rejection as well, and the
        // `Failed` state below is the better-worded of the two reports.
        let started = crate::Driver::new(clock.clone(), timers.clone())
            .drive_while(&mut runtime, |rt| {
                rt.module_eval_state() == es_runtime::ModuleEvalState::Pending
            })
            .await;
        if let es_runtime::ModuleEvalState::Failed(error) = runtime.module_eval_state() {
            return Err(error);
        }
        if runtime.heap_limit_exceeded() {
            return Err(out_of_memory(&runtime));
        }
        for error in started.uncaught_errors {
            fail_agent(&scope, DriveFailure::UncaughtError(error));
        }
        for error in started.unhandled_rejections {
            fail_agent(&scope, DriveFailure::UnhandledRejection(error));
        }

        // An isolate's exports are what its parent calls, so its prelude is
        // handed them now that they exist — and only now: a call arriving
        // earlier waits in the envelope's queue rather than finding bindings
        // still in their temporal dead zone (D227).
        if spec.isolate && !closed.load(Ordering::SeqCst) {
            match runtime.deliver_module_namespace(entry, "__isolate_serve") {
                Ok(true) => {}
                Ok(false) => {
                    return Err(UncaughtError::from_message(
                        "isolate could not serve its exports: its prelude is missing".to_string(),
                    ));
                }
                Err(err) => return Err(describe(&runtime, err.to_string())),
            }
        }

        // Settled cleanly; carry on for as long as the worker has work — but
        // stop the moment it closes itself. `close()` only turns back the
        // receive pump, so an agent holding any *other* outstanding op would
        // drive on forever: a worker that started a worker of its own is
        // waiting on that child, and the child is waiting to be told to stop by
        // the parent that just ended. `close()` discarding the remaining tasks
        // is what makes this terminate rather than hang.
        //
        // Stage two reports through a sink, so a failure reaches the parent the
        // tick it happens rather than whenever this drive finally returns — and
        // `fail_agent` ends the worker, so the same flag stops this loop.
        let closing = closed.clone();
        let sink_scope = scope.clone();
        let _ = crate::Driver::new(clock, timers)
            .reporting_failures_to(move |failure| fail_agent(&sink_scope, failure))
            .drive_while(&mut runtime, |_| !closing.load(Ordering::SeqCst))
            .await;
        // Running out of memory stops an agent without going through any of the
        // failure paths above: V8 terminates the isolate, the drive loop sees
        // `terminated` and returns, and the worker would otherwise look to its
        // parent like one that had simply finished. It is the one ending a
        // supervisor must not mistake for a clean one.
        if runtime.heap_limit_exceeded() {
            return Err(out_of_memory(&runtime));
        }
        Ok(())
    };
    let outcome = match turn {
        Some(turn) => Entered::new(turn, budget.clone(), drive).await,
        None => drive.await,
    };
    // Whatever the drive made of its termination, a budget that ended it is
    // the reason, and the one a parent can act on.
    let outcome = match &budget {
        Some(budget) if budget.exceeded() => Err(UncaughtError::new(
            "ERR_ISOLATE_CPU_LIMIT",
            format!(
                "isolate terminated: a turn used more than its {} ms of CPU",
                budget.limit_ms()
            ),
            String::new(),
        )),
        _ => outcome,
    };
    // An isolate's heap limit has its own code (D227), so a parent handling
    // `ERR_ISOLATE_*` sees every limit an isolate can reach under one prefix.
    let outcome = outcome.map_err(|mut error| {
        if isolate && error.name == "ERR_WORKER_OUT_OF_MEMORY" {
            error.name = "ERR_ISOLATE_MEMORY_LIMIT".to_string();
            error.message = error
                .message
                .replacen("worker terminated", "isolate terminated", 1);
        }
        error
    });

    if let Err(error) = outcome {
        report(error);
    }

    closed.store(true, Ordering::SeqCst);

    // This agent is over — by `close()`, by quiescence, or by a failure above —
    // and its global scope goes with it, so the workers it started go too. The
    // same rule as `terminate()`, applied to the end a worker reaches on its
    // own; without it, a parent that simply finished would leave its children
    // running and the process unable to exit.
    //
    // Inside a runtime, because joining those threads is async work. Only the
    // children: this worker's own entry is the caller's to remove.
    let own_children: Vec<u64> = live
        .children
        .lock()
        .map(|children| children.clone())
        .unwrap_or_default();
    let orphans: Vec<Arc<Live>> = own_children
        .into_iter()
        .flat_map(|child| take_subtree(registry, child))
        .collect();
    if !orphans.is_empty() {
        terminate_all(&orphans).await;
    }
    // This worker's own entry is deliberately left in place: `recv` retires it
    // when the parent sees the `Closed` below, and removing it from here would
    // race that — a parent between two receives would find no entry, and the
    // messages still queued in it would go nowhere.

    let _ = to_parent.send(WorkerIncoming::Closed);
}

/// A future whose isolate is entered for each poll and left after it, so
/// other agents on the same pool thread can have their turns in between.
///
/// Left again even if the poll panics: a panic that unwound past an entered
/// isolate would leave every later turn on the thread refused.
struct Entered<F> {
    turn: es_runtime::TurnHandle,
    /// The isolate's CPU budget a turn, when it has one: each poll is a turn,
    /// begun here and posted where the pool's watchdog can see it.
    budget: Option<Arc<es_runtime::CpuBudget>>,
    inner: std::pin::Pin<Box<F>>,
}

impl<F> Entered<F> {
    fn new(
        turn: es_runtime::TurnHandle,
        budget: Option<Arc<es_runtime::CpuBudget>>,
        inner: F,
    ) -> Self {
        Entered {
            turn,
            budget,
            inner: Box::pin(inner),
        }
    }
}

impl<F: std::future::Future> std::future::Future for Entered<F> {
    type Output = F::Output;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<F::Output> {
        /// Ends the turn and leaves the isolate, even if the poll panics.
        struct Leave<'a>(
            &'a es_runtime::TurnHandle,
            Option<&'a es_runtime::CpuBudget>,
        );
        impl Drop for Leave<'_> {
            fn drop(&mut self) {
                if let Some(budget) = self.1 {
                    budget.end_turn();
                    POOL_TURN.with(|turn| {
                        if let Some(turn) = turn.get() {
                            turn.end();
                        }
                    });
                }
                self.0.leave();
            }
        }
        let this = &mut *self;
        this.turn.enter();
        if let Some(budget) = &this.budget {
            budget.begin_turn();
            POOL_TURN.with(|turn| {
                if let Some(turn) = turn.get() {
                    turn.begin(budget.clone());
                }
            });
        }
        let _leave = Leave(&this.turn, this.budget.as_deref());
        this.inner.as_mut().poll(cx)
    }
}

/// The turn running on one pool thread, as its watchdog sees it.
struct TurnSlot {
    running: std::sync::Mutex<Option<(Arc<es_runtime::CpuBudget>, std::time::Instant)>>,
    watchdog: std::thread::Thread,
}

impl TurnSlot {
    fn begin(&self, budget: Arc<es_runtime::CpuBudget>) {
        if let Ok(mut running) = self.running.lock() {
            *running = Some((budget, std::time::Instant::now()));
        }
        self.watchdog.unpark();
    }

    fn end(&self) {
        if let Ok(mut running) = self.running.lock() {
            *running = None;
        }
    }
}

thread_local! {
    /// On a pool thread, where it posts its budgeted turns; unset elsewhere.
    static POOL_TURN: std::cell::OnceCell<Arc<TurnSlot>> = const { std::cell::OnceCell::new() };
}

/// The pool's watchdog: for each thread running a budgeted turn whose wall
/// time has passed the budget, asks the isolate to check its CPU time — CPU
/// time cannot be ahead of wall time, so nothing earlier could be over. It
/// looks every 2 ms while any budgeted turn runs and sleeps otherwise; the
/// first such turn wakes it.
fn watch(slots: &[Arc<TurnSlot>]) {
    loop {
        let mut running = false;
        for slot in slots {
            let current = slot.running.lock().ok().and_then(|r| r.clone());
            if let Some((budget, started)) = current {
                running = true;
                if started.elapsed().as_secs_f64() * 1000.0 >= budget.limit_ms() {
                    budget.check();
                }
            }
        }
        if running {
            std::thread::sleep(std::time::Duration::from_millis(2));
        } else {
            std::thread::park();
        }
    }
}

/// Work for a pool thread: made on the spawning side, where it must be `Send`,
/// and turned into its future on the pool thread, where it need not be — the
/// runtime it builds never leaves that thread.
type Job = Box<dyn FnOnce() -> std::pin::Pin<Box<dyn std::future::Future<Output = ()>>> + Send>;

/// A few OS threads that drive many agents between them (D227).
///
/// Each thread runs one single-threaded tokio runtime and a `LocalSet`, and
/// every agent placed on it is a local task there. An agent goes to the thread
/// with the fewest agents when it starts and stays there for its life:
/// measured, moving isolates between threads was never faster and needs a
/// `Locker` on every turn.
///
/// The threads start with the first agent, so a program that never makes one
/// pays for none, and they are made after V8 is initialised — as V8 requires
/// of any thread that will run JavaScript.
pub struct IsolatePool {
    size: usize,
    threads: std::sync::OnceLock<Result<Vec<PoolThread>, String>>,
}

struct PoolThread {
    jobs: mpsc::UnboundedSender<Job>,
    /// Agents placed here and not yet finished.
    load: Arc<AtomicUsize>,
    /// Agents prepared here and not yet claimed.
    prepared: Arc<AtomicUsize>,
}

impl IsolatePool {
    /// A pool of `size` threads; at least one.
    pub fn new(size: usize) -> Self {
        IsolatePool {
            size: size.max(1),
            threads: std::sync::OnceLock::new(),
        }
    }

    /// A pool with a thread for each core the process may use.
    pub fn sized_to_cores() -> Self {
        Self::new(std::thread::available_parallelism().map_or(4, std::num::NonZero::get))
    }

    /// How many threads the pool has, or will have once started.
    pub fn size(&self) -> usize {
        self.size
    }

    fn threads(&self) -> Result<&[PoolThread], ProviderError> {
        self.threads
            .get_or_init(|| start_pool(self.size))
            .as_deref()
            .map_err(|err| ProviderError::Other(err.clone()))
    }

    /// Agents prepared on the pool and not yet claimed.
    fn prepared(&self) -> usize {
        self.threads().map_or(0, |threads| {
            threads
                .iter()
                .map(|t| t.prepared.load(Ordering::SeqCst))
                .sum()
        })
    }

    /// Places a preparing `job` on the thread with the fewest agents, running
    /// and prepared together, and counts it there at once so the next one
    /// goes elsewhere.
    fn prepare_on(&self, job: Job) -> Result<(), ProviderError> {
        let threads = self.threads()?;
        let Some(thread) = threads
            .iter()
            .min_by_key(|t| t.load.load(Ordering::SeqCst) + t.prepared.load(Ordering::SeqCst))
        else {
            return Err(ProviderError::Other("isolate pool has no threads".into()));
        };
        thread.prepared.fetch_add(1, Ordering::SeqCst);
        thread.jobs.send(job).map_err(|_| {
            thread.prepared.fetch_sub(1, Ordering::SeqCst);
            ProviderError::Other("isolate pool thread has stopped".into())
        })
    }

    /// Places `job` on the least-loaded thread — among those holding a
    /// prepared agent, when `prefer_prepared` and there are any.
    fn run(&self, job: Job, prefer_prepared: bool) -> Result<(), ProviderError> {
        let threads = self.threads()?;
        let with_prepared = || {
            threads
                .iter()
                .filter(|t| t.prepared.load(Ordering::SeqCst) > 0)
                .min_by_key(|t| t.load.load(Ordering::SeqCst))
        };
        let least_loaded = || threads.iter().min_by_key(|t| t.load.load(Ordering::SeqCst));
        let chosen = if prefer_prepared {
            with_prepared().or_else(least_loaded)
        } else {
            least_loaded()
        };
        let Some(thread) = chosen else {
            return Err(ProviderError::Other("isolate pool has no threads".into()));
        };
        thread.load.fetch_add(1, Ordering::SeqCst);
        let load = thread.load.clone();
        let counted: Job = Box::new(move || {
            let inner = job();
            Box::pin(async move {
                inner.await;
                load.fetch_sub(1, Ordering::SeqCst);
            })
        });
        thread.jobs.send(counted).map_err(|_| {
            thread.load.fetch_sub(1, Ordering::SeqCst);
            ProviderError::Other("isolate pool thread has stopped".into())
        })
    }
}

/// The pool's threads and the watchdog that enforces their isolates' CPU
/// budgets. The watchdog is made first, so each thread can be told where to
/// post its turns.
fn start_pool(size: usize) -> Result<Vec<PoolThread>, String> {
    let (handle, slots) = {
        let (tell, told) = std::sync::mpsc::channel::<Vec<Arc<TurnSlot>>>();
        let handle = std::thread::Builder::new()
            .name("isolate-pool-watchdog".into())
            .spawn(move || {
                if let Ok(slots) = told.recv() {
                    watch(&slots);
                }
            })
            .map_err(|err| format!("isolate pool watchdog could not start: {err}"))?;
        let watchdog = handle.thread().clone();
        let slots: Vec<Arc<TurnSlot>> = (0..size)
            .map(|_| {
                Arc::new(TurnSlot {
                    running: std::sync::Mutex::new(None),
                    watchdog: watchdog.clone(),
                })
            })
            .collect();
        let _ = tell.send(slots.clone());
        (handle, slots)
    };
    drop(handle);
    slots
        .into_iter()
        .enumerate()
        .map(|(index, slot)| start_pool_thread(index, slot))
        .collect()
}

fn start_pool_thread(index: usize, slot: Arc<TurnSlot>) -> Result<PoolThread, String> {
    let (jobs, mut inbox) = mpsc::unbounded_channel::<Job>();
    let prepared = Arc::new(AtomicUsize::new(0));
    let counted = prepared.clone();
    std::thread::Builder::new()
        // The same stack as every other JS thread (D189).
        .stack_size(WORKER_STACK)
        .name(format!("isolate-pool-{index}"))
        .spawn(move || {
            POOL_TURN.with(|turn| {
                let _ = turn.set(slot);
            });
            PREPARED_COUNT.with(|count| {
                let _ = count.set(counted);
            });
            let Ok(tokio_runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                // Dropping the inbox makes every later `run` on this thread
                // fail with a message, rather than queue work nothing runs.
                return;
            };
            let local = tokio::task::LocalSet::new();
            local.block_on(&tokio_runtime, async move {
                while let Some(job) = inbox.recv().await {
                    tokio::task::spawn_local(job());
                }
            });
        })
        .map_err(|err| format!("isolate pool thread could not start: {err}"))?;
    Ok(PoolThread {
        jobs,
        load: Arc::new(AtomicUsize::new(0)),
        prepared,
    })
}

/// A [`Process`] view for a worker agent: everything the host one reports, but
/// `exit` stops only this worker.
///
/// `runtime:process` `exit(code)` does two things — record the code, and halt
/// execution. Halting is already per-agent, because each runtime interrupts its
/// own isolate. Recording is not: the `Process` provider is shared, so a worker
/// calling `exit(3)` would set the code the *process* exits with, from a thread
/// the program may not know is running. A worker ending is not the program
/// ending, so the code is dropped and only the halt survives.
pub struct WorkerProcess {
    inner: Arc<dyn es_runtime_providers::Process>,
    /// What `new Worker(url, { env })` handed this worker, if anything.
    env: Option<Vec<(String, String)>>,
}

impl WorkerProcess {
    /// Wraps `inner` for use inside a worker agent.
    #[must_use]
    pub fn new(inner: Arc<dyn es_runtime_providers::Process>) -> Self {
        WorkerProcess { inner, env: None }
    }

    /// Serves `env` to this worker instead of the host environment.
    ///
    /// The parent could already read every value it passed, so this narrows
    /// what the worker sees rather than granting it anything — which is why
    /// reading it needs no capability.
    #[must_use]
    pub fn with_env(mut self, env: Option<Vec<(String, String)>>) -> Self {
        self.env = env;
        self
    }
}

impl es_runtime_providers::Process for WorkerProcess {
    fn env(&self) -> Vec<(String, String)> {
        // The handed environment wins where there is one: a worker given an
        // explicit `env` must not also see the host's, whatever it holds.
        match &self.env {
            Some(env) => env.clone(),
            None => self.inner.env(),
        }
    }
    fn provided_env(&self) -> Option<Vec<(String, String)>> {
        self.env.clone()
    }
    fn args(&self) -> Vec<String> {
        self.inner.args()
    }
    fn cwd(&self) -> Result<String, ProviderError> {
        self.inner.cwd()
    }
    fn platform(&self) -> String {
        self.inner.platform()
    }
    fn arch(&self) -> String {
        self.inner.arch()
    }
    fn hardware_concurrency(&self) -> u32 {
        self.inner.hardware_concurrency()
    }
    fn exit(&self, _code: i32) {
        // Deliberately dropped — see the type's documentation.
    }
    fn requested_exit_code(&self) -> Option<i32> {
        None
    }
}

/// The [`Process`](es_runtime_providers::Process) an `Isolate` sees
/// (DECISIONS D227): what computation needs and nothing about the deployment.
///
/// The platform and architecture are the host's, because `runtime:path`'s rules
/// follow them and a path computed in an isolate should mean what it means to
/// its parent. Everything else says the isolate has no place of its own: no
/// environment, no arguments, `/` as its directory — an isolate reads no file,
/// so the real one would only tell it where the program is installed — and an
/// `exit()` that ends the isolate alone.
pub struct IsolateProcess;

impl es_runtime_providers::Process for IsolateProcess {
    fn env(&self) -> Vec<(String, String)> {
        Vec::new()
    }
    fn provided_env(&self) -> Option<Vec<(String, String)>> {
        Some(Vec::new())
    }
    fn args(&self) -> Vec<String> {
        Vec::new()
    }
    fn cwd(&self) -> Result<String, ProviderError> {
        Ok(if cfg!(windows) {
            "C:\\".to_string()
        } else {
            "/".to_string()
        })
    }
    fn platform(&self) -> String {
        std::env::consts::OS.to_string()
    }
    fn arch(&self) -> String {
        std::env::consts::ARCH.to_string()
    }
    fn exit(&self, _code: i32) {
        // An isolate's exit ends that isolate; the code is not the process's.
    }
    fn requested_exit_code(&self) -> Option<i32> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A factory for agents that need nothing: fresh prelude, no imports.
    fn bare_factory() -> Arc<dyn WorkerRuntimeFactory> {
        Arc::new(|spec: &WorkerSpec, scope: Arc<dyn WorkerScope>| {
            let providers = es_runtime::HostProviders::new(
                Arc::new(crate::SystemClock::new()),
                Arc::new(crate::NullConsole),
                Arc::new(crate::testing::MockTransport::constant(
                    crate::testing::MockResponse::ok(""),
                )),
                Arc::new(crate::OsEntropy),
            )
            .with_worker_scope(scope);
            let engine = es_runtime::V8Engine::new(spec.limits)
                .map_err(|e| ProviderError::Other(e.to_string()))?;
            let runtime = Runtime::new(Box::new(engine), providers)
                .map_err(|e| ProviderError::Other(e.to_string()))?;
            let loader: Arc<dyn ModuleLoader> = Arc::new(crate::DenyModuleLoader);
            Ok((runtime, loader))
        })
    }

    fn spec(source: &str) -> WorkerSpec {
        WorkerSpec {
            specifier: "file:///agent.js".into(),
            source: source.into(),
            name: String::new(),
            env: None,
            capabilities: es_runtime_common::CapabilitySet::none(),
            load_capabilities: es_runtime_common::CapabilitySet::none(),
            limits: es_runtime_common::Limits::default(),
            isolate: false,
            cpu_time_ms: None,
        }
    }

    /// Many agents on a pool of two threads all start, run, answer, and stop
    /// (D227): each built on its pool thread, left between turns, and entered
    /// for each — with timers settling while the others take theirs.
    #[test]
    fn many_agents_share_a_pool_of_two_threads() {
        const AGENTS: usize = 24;
        let tokio = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio");
        let pool = Arc::new(IsolatePool::new(2));
        let host = ThreadWorkerHost::on_pool(bare_factory(), pool);
        tokio.block_on(async {
            let mut ids = Vec::new();
            for _ in 0..AGENTS {
                let id = host
                    .spawn(spec(
                        "await new Promise((r) => setTimeout(r, 5)); postMessage('up');",
                    ))
                    .await
                    .expect("spawn");
                ids.push(id);
            }
            for &id in &ids {
                match host.recv(id).await.expect("recv") {
                    Some(WorkerIncoming::Message(bytes)) => assert!(!bytes.is_empty()),
                    Some(WorkerIncoming::Error { error }) => panic!("agent failed: {error:?}"),
                    other => panic!("agent {id} ended without answering: {}", other.is_some()),
                }
            }
            for &id in &ids {
                host.terminate(id).await.expect("terminate");
            }
        });
    }

    /// Agents prepared ahead of time wait on the pool and are claimed by the
    /// spawns that match their limits; one that does not match starts as
    /// usual, and leaves the prepared ones waiting (D227).
    #[test]
    fn prepared_agents_are_claimed_by_matching_spawns() {
        let tokio = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio");
        let pool = Arc::new(IsolatePool::new(2));
        let host = ThreadWorkerHost::on_pool(bare_factory(), pool.clone());
        let isolate = |source: &str| WorkerSpec {
            isolate: true,
            ..spec(source)
        };
        tokio.block_on(async {
            let waiting = host.prepare(isolate(""), 4).await.expect("prepare");
            assert_eq!(waiting, 4);

            let other_limits = WorkerSpec {
                limits: es_runtime_common::Limits::default().with_heap_limit_bytes(64 << 20),
                ..isolate("postMessage('cold');")
            };
            let cold = host.spawn(other_limits).await.expect("spawn");
            assert!(matches!(
                host.recv(cold).await.expect("recv"),
                Some(WorkerIncoming::Message(_))
            ));
            assert_eq!(
                pool.prepared(),
                4,
                "a spawn with other limits took a prepared agent"
            );

            let mut ids = Vec::new();
            for _ in 0..3 {
                ids.push(
                    host.spawn(isolate("postMessage('warm');"))
                        .await
                        .expect("spawn"),
                );
            }
            for &id in &ids {
                assert!(matches!(
                    host.recv(id).await.expect("recv"),
                    Some(WorkerIncoming::Message(_))
                ));
            }
            assert_eq!(
                pool.prepared(),
                1,
                "matching spawns did not claim prepared agents"
            );

            for id in ids.into_iter().chain([cold]) {
                host.terminate(id).await.expect("terminate");
            }
        });
    }

    /// A thread host has nowhere to keep an agent before its spawn, so it
    /// prepares none.
    #[test]
    fn a_thread_host_prepares_nothing() {
        let tokio = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio");
        let host = ThreadWorkerHost::new(bare_factory());
        assert_eq!(
            tokio.block_on(host.prepare(spec(""), 3)).expect("prepare"),
            0
        );
    }

    /// An agent stuck in a loop on a pool thread is stopped by `terminate`,
    /// and the agents sharing its thread go on.
    #[test]
    fn terminating_a_spinning_agent_frees_its_pool_thread() {
        let tokio = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio");
        let pool = Arc::new(IsolatePool::new(1));
        let host = ThreadWorkerHost::on_pool(bare_factory(), pool);
        tokio.block_on(async {
            let spinning = host
                .spawn(spec("postMessage('spinning'); for (;;) {}"))
                .await
                .expect("spawn");
            assert!(matches!(
                host.recv(spinning).await.expect("recv"),
                Some(WorkerIncoming::Message(_))
            ));
            let waiting = host
                .spawn(spec("postMessage('after');"))
                .await
                .expect("spawn");
            host.terminate(spinning).await.expect("terminate");
            assert!(matches!(
                host.recv(waiting).await.expect("recv"),
                Some(WorkerIncoming::Message(_))
            ));
            host.terminate(waiting).await.expect("terminate");
        });
    }

    /// A `Live` with nothing attached: these tests are about the registry's
    /// bookkeeping, not about running an agent, and building one for real would
    /// mean a thread and a V8 isolate per case.
    fn live(parent: Option<u64>) -> Arc<Live> {
        let (to_worker, _worker_inbox) = mpsc::unbounded_channel::<Vec<u8>>();
        let (_to_parent, parent_inbox) = mpsc::unbounded_channel::<WorkerIncoming>();
        Arc::new(Live {
            to_worker: std::sync::Mutex::new(Some(to_worker)),
            from_worker: Mutex::new(parent_inbox),
            interrupt: Mutex::new(None),
            closed: Arc::new(AtomicBool::new(false)),
            join: std::sync::Mutex::new(None),
            children: std::sync::Mutex::new(Vec::new()),
            parent,
            inbound: Arc::new(AtomicUsize::new(0)),
            outbound: Arc::new(AtomicUsize::new(0)),
        })
    }

    /// Registers `child` under `parent`, the way `spawn` does.
    fn register(
        registry: &std::sync::Mutex<HashMap<u64, Arc<Live>>>,
        id: u64,
        parent: Option<u64>,
    ) {
        let mut workers = registry.lock().expect("registry");
        workers.insert(id, live(parent));
        if let Some(parent) = parent
            && let Some(parent) = workers.get(&parent)
        {
            parent.children.lock().expect("children").push(id);
        }
    }

    fn children_of(registry: &std::sync::Mutex<HashMap<u64, Arc<Live>>>, id: u64) -> Vec<u64> {
        registry
            .lock()
            .expect("registry")
            .get(&id)
            .map(|live| live.children.lock().expect("children").clone())
            .unwrap_or_default()
    }

    /// The leak this was: a supervisor that starts one worker per job and never
    /// terminates kept an id for every job it had ever run, and walked all of
    /// them on each `terminate`.
    #[test]
    fn a_retired_worker_leaves_its_parents_child_list() {
        let registry = std::sync::Mutex::new(HashMap::new());
        register(&registry, 1, None);
        for id in 2..=200 {
            register(&registry, id, Some(1));
            retire(&registry, id);
        }
        assert!(
            children_of(&registry, 1).is_empty(),
            "200 finished workers left {} ids behind",
            children_of(&registry, 1).len()
        );
        assert_eq!(registry.lock().expect("registry").len(), 1);
    }

    /// Terminating a subtree unhooks its root for the same reason — the parent
    /// it hung from is still around.
    #[test]
    fn taking_a_subtree_unhooks_its_root() {
        let registry = std::sync::Mutex::new(HashMap::new());
        register(&registry, 1, None);
        register(&registry, 2, Some(1));
        register(&registry, 3, Some(2));
        register(&registry, 4, Some(1));

        let taken = take_subtree(&registry, 2);
        assert_eq!(taken.len(), 2, "the worker and its own child");
        assert_eq!(children_of(&registry, 1), vec![4]);
    }

    /// Retiring a worker whose parent has already gone is a no-op rather than a
    /// panic: a parent's own end takes its children with it, so the two orders
    /// both happen.
    #[test]
    fn retiring_an_orphan_is_harmless() {
        let registry = std::sync::Mutex::new(HashMap::new());
        register(&registry, 1, None);
        register(&registry, 2, Some(1));
        registry.lock().expect("registry").remove(&1);
        retire(&registry, 2);
        assert!(registry.lock().expect("registry").is_empty());
    }

    /// Ids only ever count up, so a stale entry could never name a later
    /// worker. That is what made the leak a leak rather than a bug — and what
    /// let it go unnoticed.
    #[test]
    fn ids_are_never_reused() {
        let host = ThreadWorkerHost::new(Arc::new(|_: &WorkerSpec, _: Arc<dyn WorkerScope>| {
            unreachable!("no worker is started here")
        }));
        let first = host.next_id.fetch_add(1, Ordering::SeqCst);
        let second = host.next_id.fetch_add(1, Ordering::SeqCst);
        assert!(second > first);
    }
}
