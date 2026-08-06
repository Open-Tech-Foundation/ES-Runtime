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
}

/// A [`WorkerHost`] that runs each agent on its own OS thread.
pub struct ThreadWorkerHost {
    factory: Arc<dyn WorkerRuntimeFactory>,
    registry: Arc<std::sync::Mutex<HashMap<u64, Arc<Live>>>>,
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
            id
        };

        let factory = self.factory.clone();
        let scope = Arc::new(ThreadScope {
            name: spec.name.clone(),
            to_parent: to_parent.clone(),
            from_parent: Arc::new(Mutex::new(worker_inbox)),
            closed: closed.clone(),
            closing: Arc::new(tokio::sync::Notify::new()),
        });

        let thread_live = live.clone();
        let thread = std::thread::Builder::new()
            .name(if spec.name.is_empty() {
                format!("worker-{id}")
            } else {
                spec.name.clone()
            })
            .spawn(move || run_worker(&factory, spec, scope, &to_parent, &thread_live, &closed));

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
        let live = self.registry.lock().ok().and_then(|mut w| w.remove(&id));
        Box::pin(async move {
            let Some(live) = live else { return Ok(()) };
            terminate_live(&live).await;
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
        Box::pin(async move {
            for live in all {
                terminate_live(&live).await;
            }
        })
    }
}

/// Stops one worker and waits for its thread. Interrupt first, then join: the
/// flag alone only stops a *cooperative* agent, and a worker spinning in a
/// synchronous loop or parked in `Atomics.wait` needs V8's own termination —
/// which is exactly what `InterruptHandle` delivers cross-thread.
async fn terminate_live(live: &Live) {
    live.closed.store(true, Ordering::SeqCst);
    if let Some(handle) = live.interrupt.lock().await.as_ref() {
        handle.terminate();
    }
    // Drop the sender: closing the channel wakes a worker parked waiting for
    // its next message, which the interrupt above does not reach.
    live.to_worker.lock().await.take();

    let join = live.join.lock().ok().and_then(|mut slot| slot.take());
    if let Some(join) = join {
        // Joining blocks, so it goes to the blocking pool rather than stalling
        // the caller's reactor.
        let _ = tokio::task::spawn_blocking(move || join.join()).await;
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
) {
    let report = |message: String| {
        let _ = to_parent.send(WorkerIncoming::Error { message });
    };

    // A single-threaded tokio runtime: this agent's own reactor, so its timers
    // and sockets are driven here rather than on the parent's.
    let tokio_runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(err) => {
            report(format!("worker runtime could not start: {err}"));
            let _ = to_parent.send(WorkerIncoming::Closed);
            return;
        }
    };

    let (mut runtime, loader) = match factory.build(&spec, scope.clone()) {
        Ok(built) => built,
        Err(err) => {
            report(format!("worker could not be created: {err}"));
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
            Err(err) => return Err(format!("{err}")),
        };
        if let Err(err) = runtime.begin_evaluation(entry) {
            return Err(format!("{err}"));
        }

        let driver = crate::Driver::new(
            Arc::new(crate::SystemClock::new()),
            Arc::new(crate::TokioTimers),
        );

        // Two stages, because a worker with an `onmessage` never reaches
        // quiescence — its receive pump is a deliberately outstanding op, which
        // is what keeps it alive between messages. Waiting for the drive to
        // return would mean an entry module that throws is not reported to the
        // parent until the worker is terminated.
        let mut outcome = driver
            .drive_while(&mut runtime, |rt| {
                rt.module_eval_state() == es_runtime::ModuleEvalState::Pending
            })
            .await;
        if let es_runtime::ModuleEvalState::Failed(message) = runtime.module_eval_state() {
            return Err(message);
        }

        // Settled cleanly; carry on for as long as the worker has work.
        let rest = driver.run_to_completion(&mut runtime).await;
        outcome
            .unhandled_rejections
            .extend(rest.unhandled_rejections);
        outcome.uncaught_errors.extend(rest.uncaught_errors);
        Ok(outcome)
    });

    match outcome {
        Err(message) => report(message),
        Ok(drive) => {
            // Whatever the guest did not claim is the parent's to hear about,
            // the same way the CLI reports it for the main agent.
            for message in drive.uncaught_errors {
                report(message);
            }
            for message in drive.unhandled_rejections {
                report(format!("unhandled rejection: {message}"));
            }
        }
    }

    closed.store(true, Ordering::SeqCst);
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
pub struct WorkerProcess(Arc<dyn es_runtime_providers::Process>);

impl WorkerProcess {
    /// Wraps `inner` for use inside a worker agent.
    #[must_use]
    pub fn new(inner: Arc<dyn es_runtime_providers::Process>) -> Self {
        WorkerProcess(inner)
    }
}

impl es_runtime_providers::Process for WorkerProcess {
    fn env(&self) -> Vec<(String, String)> {
        self.0.env()
    }
    fn args(&self) -> Vec<String> {
        self.0.args()
    }
    fn cwd(&self) -> Result<String, ProviderError> {
        self.0.cwd()
    }
    fn platform(&self) -> String {
        self.0.platform()
    }
    fn arch(&self) -> String {
        self.0.arch()
    }
    fn hardware_concurrency(&self) -> u32 {
        self.0.hardware_concurrency()
    }
    fn exit(&self, _code: i32) {
        // Deliberately dropped — see the type's documentation.
    }
    fn requested_exit_code(&self) -> Option<i32> {
        None
    }
}
