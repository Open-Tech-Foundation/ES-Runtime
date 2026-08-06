//! The standalone driver: the concrete event loop the `runtime` crate does not
//! own (ARCHITECTURE.md §5, DECISIONS.md D4).

use std::sync::Arc;
use std::task::Wake;

use es_runtime::Runtime;
use es_runtime_providers::{Clock, Timers};
use tokio::sync::Notify;

/// The waker the driver injects into the runtime: when a pending async-op future
/// signals readiness (its backing tokio task made progress), it notifies the
/// driver, which re-ticks immediately instead of waiting out a blind interval.
struct DriverWaker {
    notify: Arc<Notify>,
}

impl Wake for DriverWaker {
    fn wake(self: Arc<Self>) {
        self.notify.notify_one();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.notify.notify_one();
    }
}

/// Fallback re-poll interval while async work is pending. The injected waker
/// wakes us the instant an op-future is ready, so this only bounds the wait for
/// readiness a waker can't deliver (e.g. a reactor-driven client future polled
/// outside a task). Kept at 1ms — the prior poll-on-interval behavior — so this
/// path is never slower than before; the waker just removes the wait when it can.
const ASYNC_FALLBACK_MS: u64 = 1;

/// Drives a [`Runtime`] to quiescence on tokio.
///
/// Each iteration reads the current time from the [`Clock`], advances the
/// runtime one [`tick`](Runtime::tick), then parks on the [`Timers`] source
/// until the next deadline (or yields if only async work is pending). This is
/// the seam Layer B replaces with its scheduler: the runtime is unchanged; only
/// the driver differs.
pub struct Driver {
    clock: Arc<dyn Clock>,
    timers: Arc<dyn Timers>,
    on_failure: Option<Arc<dyn Fn(DriveFailure) + Send + Sync>>,
}

/// One failure the guest did not claim, handed over the tick it happened.
///
/// The two arms are the same two the [`DriveOutcome`] carries; a sink gets them
/// separately because only the consumer knows how to word them.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum DriveFailure {
    /// An exception that escaped a host-invoked callback.
    UncaughtError(String),
    /// A promise rejection no listener took responsibility for.
    UnhandledRejection(String),
}

/// What a drive to quiescence surfaced that the guest did not take
/// responsibility for.
///
/// Both kinds are already past the guest's own chance to claim them: an
/// `unhandledrejection` or `error` listener calling `preventDefault()` keeps the
/// failure out of here entirely. What is left is for the embedder to report.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct DriveOutcome {
    /// Promise rejections that reached quiescence with no handler.
    pub unhandled_rejections: Vec<String>,
    /// Exceptions that escaped a timer callback. These have no caller to
    /// propagate to, so without this they would be lost.
    pub uncaught_errors: Vec<String>,
}

impl DriveOutcome {
    /// Whether the drive finished with nothing unclaimed.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.unhandled_rejections.is_empty() && self.uncaught_errors.is_empty()
    }
}

impl Driver {
    /// Builds a driver from a clock and a timer source.
    pub fn new(clock: Arc<dyn Clock>, timers: Arc<dyn Timers>) -> Self {
        Driver {
            clock,
            timers,
            on_failure: None,
        }
    }

    /// Hands each unclaimed failure to `sink` **the tick it happens**, instead of
    /// collecting it into the [`DriveOutcome`] returned at the end.
    ///
    /// For an agent that never reaches quiescence, the outcome arrives too late
    /// to be a report: a worker holds its receive pump open on purpose, so a
    /// throw inside `onmessage` would sit in the outcome until the worker was
    /// terminated — by which time whoever needed to hear about it has usually
    /// given up. A sink is how a worker's failures reach its parent while the
    /// worker is still running.
    ///
    /// Failures given to the sink are **not** also in the outcome; a consumer
    /// picks one or the other, never both.
    #[must_use]
    pub fn reporting_failures_to<F>(mut self, sink: F) -> Self
    where
        F: Fn(DriveFailure) + Send + Sync + 'static,
    {
        self.on_failure = Some(Arc::new(sink));
        self
    }

    /// Advances `runtime` until no async ops or timers remain, parking between
    /// ticks rather than busy-waiting on timers. Returns everything the guest
    /// left unclaimed along the way (see [`DriveOutcome`]).
    ///
    /// Note: a never-completing async op would loop forever here by design — the
    /// execution-time watchdog that bounds that is a hardening-phase concern
    /// (SPEC.md §6.9), not the driver's.
    pub async fn run_to_completion(&self, runtime: &mut Runtime) -> DriveOutcome {
        self.drive_while(runtime, |_| true).await
    }

    /// [`run_to_completion`](Self::run_to_completion), but stopping early when
    /// `keep_going` returns `false` for the runtime after a tick.
    ///
    /// The reason this exists: an agent that keeps a pending op open on purpose
    /// — a worker waiting on `onmessage`, a server holding a listener — never
    /// reaches quiescence, so anything that must be observed *while* it runs
    /// cannot wait for the drive to return. A worker whose entry module throws
    /// is exactly that case: its parent's `error` event would otherwise not
    /// fire until the worker was terminated.
    ///
    /// Whatever the predicate stops for is still in the returned outcome; a
    /// caller that wants to carry on can call again on the same runtime.
    pub async fn drive_while<F>(&self, runtime: &mut Runtime, keep_going: F) -> DriveOutcome
    where
        F: Fn(&mut Runtime) -> bool,
    {
        let mut outcome = DriveOutcome::default();

        // Wire a real waker: a ready op-future will notify us so we re-tick at
        // once, rather than re-polling on a fixed interval (the latency floor
        // that otherwise dominates I/O-bound workloads like an HTTP server).
        let notify = Arc::new(Notify::new());
        let waker = std::task::Waker::from(Arc::new(DriverWaker {
            notify: notify.clone(),
        }));
        runtime.set_async_waker(waker);

        loop {
            let now = self.clock.monotonic_ms();
            let status = runtime.tick(now);
            match &self.on_failure {
                // Handed over now, while the agent that produced them is still
                // running and whoever is watching can still act.
                Some(sink) => {
                    for message in status.uncaught_errors {
                        sink(DriveFailure::UncaughtError(message));
                    }
                    for message in status.unhandled_rejections {
                        sink(DriveFailure::UnhandledRejection(message));
                    }
                }
                None => {
                    outcome
                        .unhandled_rejections
                        .extend(status.unhandled_rejections);
                    outcome.uncaught_errors.extend(status.uncaught_errors);
                }
            }

            // Execution was terminated — `process.exit()`, the watchdog, a
            // `Worker.terminate()`, the heap guard. Nothing more can run, and
            // whatever was suspended when it hit stays suspended: a module
            // waiting at a top-level `await` never settles, so quiescence never
            // arrives and this loop would spin for as long as the process
            // lives. Stopping is the only correct move, and the caller reads
            // the reason (an exit code, a deadline) from where it was recorded.
            if status.terminated {
                break;
            }

            // Resolve + load any dynamic import()s raised this tick (async I/O),
            // linking each so a later tick settles its promise. A processing
            // error here is an internal failure, not a guest rejection.
            let linked = match runtime.process_dynamic_imports().await {
                Ok(linked) => linked,
                Err(err) => {
                    outcome
                        .unhandled_rejections
                        .push(format!("dynamic import failed: {err}"));
                    break;
                }
            };

            // `has_pending_work` (re-read after processing) now also covers
            // in-flight dynamic imports awaiting their module's evaluation.
            if !runtime.has_pending_work() {
                break;
            }
            if !keep_going(runtime) {
                break;
            }

            // V8 finishing work on its own threads is pending work no waker can
            // announce: it lands as a foreground task that only `tick` finds,
            // and posting it touches nothing we are parked on. Parking on a
            // timeout here therefore charges the whole park to every async wasm
            // compile — measured at 1.78ms per compile on the bench's 60-module
            // row, against V8's own cost of about 0.6ms, which is the entire
            // reason esrun trailed Node and Deno there while running the same
            // compiler. Yield instead of sleeping: the scheduler comes back in
            // microseconds and the next tick drains the queue.
            //
            // Checked before the timer branch on purpose — an unrelated pending
            // timer must not put its deadline between a finished compile and the
            // promise it settles.
            // The same trap as V8's background work, from the other direction: a
            // dynamic import that was just linked has its promise settled by the
            // *next* tick, not by the linking. Parking first charges the whole
            // park to the import — an unrelated 3-second timer pending made
            // `await import("./x.js")` take three seconds, and with nothing
            // pending but the import it was the fallback park instead. Re-tick.
            if linked || status.v8_background_work {
                tokio::task::yield_now().await;
                continue;
            }

            match status.next_timer_deadline_ms {
                Some(deadline) => {
                    // Sleep until the timer is due, but let a ready async op cut
                    // the wait short (its future woke us) so I/O isn't blocked
                    // behind a pending timer.
                    let delay = deadline.saturating_sub(self.clock.monotonic_ms());
                    tokio::select! {
                        () = notify.notified() => {}
                        () = self.timers.sleep(delay) => {}
                    }
                }
                None => {
                    // Async work pending but no timer due (e.g. an open socket
                    // awaiting bytes). Wait for a pending op to wake us (its
                    // future signalled readiness on our waker) — re-polling at
                    // once with near-zero latency — with a bounded fallback so a
                    // future that registers no waker can't stall the loop. The
                    // CPU stays idle while parked.
                    tokio::select! {
                        () = notify.notified() => {}
                        () = self.timers.sleep(ASYNC_FALLBACK_MS) => {}
                    }
                }
            }
        }
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use es_runtime::{AsyncOp, HostProviders, OpDecl, Runtime, V8Engine, Value};
    use es_runtime_common::Limits;

    use crate::testing::{ManualClock, ManualTimers, MockResponse, MockTransport, SeededEntropy};
    use crate::{NullConsole, SystemClock};

    // This is the one V8-touching test in this crate, so it needs no
    // serialization guard (the others don't create an isolate).
    #[tokio::test]
    async fn drives_async_op_and_timer_to_completion_deterministically() {
        let engine = V8Engine::new(Limits::default()).expect("engine");
        let providers = HostProviders::new(
            Arc::new(SystemClock::new()),
            Arc::new(NullConsole),
            Arc::new(MockTransport::constant(MockResponse::ok("ok"))),
            Arc::new(SeededEntropy::new(1)),
        );
        let mut rt = Runtime::new(Box::new(engine), providers).expect("runtime");
        rt.register_op(OpDecl::r#async("answer", |_args| -> AsyncOp {
            Box::pin(async { Ok(Value::Number(42.0)) })
        }))
        .unwrap();

        // An async op (settles on the first tick) and a 50ms timer.
        rt.eval(
            "globalThis.answer = 0; globalThis.fired = false; \
             __ops.answer().then((v) => { globalThis.answer = v; }); \
             setTimeout(() => { globalThis.fired = true; }, 50);",
        )
        .unwrap();

        // Manual clock + manual timers make this run instant and deterministic:
        // awaiting the 50ms sleep advances the clock to the deadline.
        let clock = ManualClock::default();
        let timers = ManualTimers::new(clock.clone());
        let driver = Driver::new(Arc::new(clock.clone()), Arc::new(timers));

        let outcome = driver.run_to_completion(&mut rt).await;

        assert!(outcome.is_clean(), "got {outcome:?}");
        assert_eq!(rt.eval("globalThis.answer").unwrap(), Value::Number(42.0));
        assert_eq!(rt.eval("globalThis.fired").unwrap(), Value::Bool(true));
        assert!(clock.monotonic_ms() >= 50);
        assert!(!rt.has_pending_work());
    }
}
