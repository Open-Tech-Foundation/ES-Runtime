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
}

impl Driver {
    /// Builds a driver from a clock and a timer source.
    pub fn new(clock: Arc<dyn Clock>, timers: Arc<dyn Timers>) -> Self {
        Driver { clock, timers }
    }

    /// Advances `runtime` until no async ops or timers remain, parking between
    /// ticks rather than busy-waiting on timers. Returns every
    /// unhandled-rejection message observed along the way.
    ///
    /// Note: a never-completing async op would loop forever here by design — the
    /// execution-time watchdog that bounds that is a hardening-phase concern
    /// (SPEC.md §6.9), not the driver's.
    pub async fn run_to_completion(&self, runtime: &mut Runtime) -> Vec<String> {
        let mut rejections = Vec::new();

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
            rejections.extend(status.unhandled_rejections);

            // Resolve + load any dynamic import()s raised this tick (async I/O),
            // linking each so a later tick settles its promise. A processing
            // error here is an internal failure, not a guest rejection.
            if let Err(err) = runtime.process_dynamic_imports().await {
                rejections.push(format!("dynamic import failed: {err}"));
                break;
            }

            // `has_pending_work` (re-read after processing) now also covers
            // in-flight dynamic imports awaiting their module's evaluation.
            if !runtime.has_pending_work() {
                break;
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
        rejections
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

        let rejections = driver.run_to_completion(&mut rt).await;

        assert!(rejections.is_empty(), "got {rejections:?}");
        assert_eq!(rt.eval("globalThis.answer").unwrap(), Value::Number(42.0));
        assert_eq!(rt.eval("globalThis.fired").unwrap(), Value::Bool(true));
        assert!(clock.monotonic_ms() >= 50);
        assert!(!rt.has_pending_work());
    }
}
