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

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use es_runtime::{ModuleLoader, Runtime};
use es_runtime_common::UncaughtError;

use crate::DriveFailure;
use es_runtime_providers::{
    BoxFuture, ProviderError, WorkerHost, WorkerIncoming, WorkerScope, WorkerSpec,
};
use tokio::sync::{Mutex, mpsc};

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
}

impl WorkerScope for ThreadScope {
    fn name(&self) -> String {
        self.name.clone()
    }

    fn url(&self) -> String {
        self.url.clone()
    }

    fn post(&self, message: Vec<u8>) -> BoxFuture<Result<(), ProviderError>> {
        // A send failure means the parent is gone. That is not the worker's
        // error to raise — it is about to be terminated anyway — so the message
        // is dropped, exactly as a browser drops one to a collected parent.
        let _ = self.to_parent.send(WorkerIncoming::Message(message));
        Box::pin(async { Ok(()) })
    }

    fn recv(&self) -> BoxFuture<Result<Option<Vec<u8>>, ProviderError>> {
        let inbox = self.from_parent.clone();
        let closed = self.closed.clone();
        let closing = self.closing.clone();
        Box::pin(async move {
            if closed.load(Ordering::SeqCst) {
                return Ok(None);
            }
            let mut inbox = inbox.lock().await;
            tokio::select! {
                message = inbox.recv() => Ok(message),
                () = closing.notified() => Ok(None),
            }
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

/// One live worker, from the parent's side.
struct Live {
    /// `Option`, so termination can *drop* it: closing the channel is what
    /// wakes a worker parked waiting for its next message.
    to_worker: Mutex<Option<mpsc::UnboundedSender<Vec<u8>>>>,
    from_worker: Mutex<mpsc::UnboundedReceiver<WorkerIncoming>>,
    /// V8's cross-thread interrupt for this worker's isolate — how
    /// `terminate()` stops an agent that is not cooperating. Available only
    /// once the thread has built its runtime, so a terminate that arrives
    /// before then falls back to the flag below.
    interrupt: Mutex<Option<es_runtime::InterruptHandle>>,
    /// Set by `terminate` and by `self.close()`; the worker's loop checks it.
    closed: Arc<AtomicBool>,
    // A std mutex: only ever held to swap the handle in or out, never across an
    // await.
    join: std::sync::Mutex<Option<std::thread::JoinHandle<()>>>,
    /// The workers this one started. HTML terminates a worker's own workers
    /// along with it, and ids are otherwise flat, so this is the only record of
    /// who belongs to whom.
    children: std::sync::Mutex<Vec<u64>>,
}

/// A [`WorkerHost`] that runs each agent on its own OS thread.
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

        let live = Arc::new(Live {
            to_worker: Mutex::new(Some(to_worker)),
            from_worker: Mutex::new(parent_inbox),
            interrupt: Mutex::new(None),
            closed: closed.clone(),
            join: std::sync::Mutex::new(None),
            children: std::sync::Mutex::new(Vec::new()),
        });

        // Whoever is on this thread is the agent doing the spawning; absent
        // means the driver agent, which is nobody's child.
        let parent = self
            .agent_threads
            .lock()
            .ok()
            .and_then(|threads| threads.get(&std::thread::current().id()).copied());

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
            url: spec.specifier.clone(),
            to_parent: to_parent.clone(),
            from_parent: Arc::new(Mutex::new(worker_inbox)),
            closed: closed.clone(),
            closing: Arc::new(tokio::sync::Notify::new()),
        });

        let thread_live = live.clone();
        let registry = self.registry.clone();
        let agent_threads = self.agent_threads.clone();
        let thread = std::thread::Builder::new()
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
                    *slot = Some(handle);
                }
                Box::pin(async move { Ok(id) })
            }
            Err(err) => {
                if let Ok(mut workers) = self.registry.lock() {
                    workers.remove(&id);
                }
                Box::pin(async move { Err(ProviderError::from_io("worker thread", &err)) })
            }
        }
    }

    fn post(&self, id: u64, message: Vec<u8>) -> BoxFuture<Result<(), ProviderError>> {
        let live = self.live(id);
        Box::pin(async move {
            // A message to a worker that has already finished is dropped, not an
            // error: the spec has no delivery guarantee, and racing a `close()`
            // is ordinary rather than a fault in the sender.
            if let Some(live) = live
                && let Some(sender) = live.to_worker.lock().await.as_ref()
            {
                let _ = sender.send(message);
            }
            Ok(())
        })
    }

    fn recv(&self, id: u64) -> BoxFuture<Result<Option<WorkerIncoming>, ProviderError>> {
        let live = self.live(id);
        // Cloned rather than borrowed: the future is `'static`, and it has to
        // reach the registry to retire the worker below.
        let registry = self.registry.clone();
        Box::pin(async move {
            let Some(live) = live else { return Ok(None) };
            let event = live.from_worker.lock().await.recv().await;

            // A worker that ended on its own is retired here rather than
            // waiting for a `terminate()` that may never come. Without this it
            // would stay "live" forever, and `has_live_workers` — which the
            // embedder's loop reads to decide the process still has work —
            // would never go quiet.
            if matches!(event, Some(WorkerIncoming::Closed) | None) {
                if let Ok(mut workers) = registry.lock() {
                    workers.remove(&id);
                }
                let join = live.join.lock().ok().and_then(|mut slot| slot.take());
                if let Some(join) = join {
                    // Already finishing, but joining still blocks; keep it off
                    // the caller's reactor.
                    let _ = tokio::task::spawn_blocking(move || join.join()).await;
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

    fn has_live_workers(&self) -> bool {
        self.registry.lock().is_ok_and(|w| !w.is_empty())
    }

    fn shutdown(&self) -> BoxFuture<()> {
        let all: Vec<Arc<Live>> = self
            .registry
            .lock()
            .map(|mut w| w.drain().map(|(_, live)| live).collect())
            .unwrap_or_default();
        Box::pin(async move { terminate_all(&all).await })
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
    live.to_worker.lock().await.take();
}

/// Waits for a stopped worker's thread to end.
async fn join_live(live: &Live) {
    let join = live.join.lock().ok().and_then(|mut slot| slot.take());
    if let Some(join) = join {
        // Joining blocks, so it goes to the blocking pool rather than stalling
        // the caller's reactor.
        let _ = tokio::task::spawn_blocking(move || join.join()).await;
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

/// The worker thread's body: build the runtime, load under the parent's
/// authority, narrow, evaluate, drive.
fn run_worker(
    factory: &Arc<dyn WorkerRuntimeFactory>,
    spec: WorkerSpec,
    scope: Arc<ThreadScope>,
    to_parent: &mpsc::UnboundedSender<WorkerIncoming>,
    live: &Arc<Live>,
    closed: &Arc<AtomicBool>,
    registry: &std::sync::Mutex<HashMap<u64, Arc<Live>>>,
) {
    let report = |error: UncaughtError| {
        let _ = to_parent.send(WorkerIncoming::Error { error });
    };

    // A single-threaded tokio runtime: this agent's own reactor, so its timers
    // and sockets are driven here rather than on the parent's.
    let tokio_runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(err) => {
            report(UncaughtError::from_message(format!(
                "worker runtime could not start: {err}"
            )));
            let _ = to_parent.send(WorkerIncoming::Closed);
            return;
        }
    };

    let (mut runtime, loader) = match factory.build(&spec, scope.clone()) {
        Ok(built) => built,
        Err(err) => {
            report(UncaughtError::from_message(format!(
                "worker could not be created: {err}"
            )));
            let _ = to_parent.send(WorkerIncoming::Closed);
            return;
        }
    };

    // Publish the interrupt handle now, so a `terminate()` racing startup can
    // still stop this isolate rather than waiting out its first long task.
    tokio_runtime.block_on(async {
        *live.interrupt.lock().await = Some(runtime.interrupt_handle());
    });

    let outcome = tokio_runtime.block_on(async {
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
        tokio_runtime.block_on(terminate_all(&orphans));
    }
    // This worker's own entry is deliberately left in place: `recv` retires it
    // when the parent sees the `Closed` below, and removing it from here would
    // race that — a parent between two receives would find no entry, and the
    // messages still queued in it would go nowhere.

    let _ = to_parent.send(WorkerIncoming::Closed);
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
