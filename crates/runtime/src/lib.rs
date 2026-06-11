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

mod timer;

use crate::timer::TimerQueue;

// One-stop public surface for embedders: the engine abstraction + impl, the op
// types, values, and capabilities — all reachable from this crate.
pub use es_runtime_common::{Capability, CapabilitySet};
pub use es_runtime_engine::{AsyncOp, Engine, OpDecl, OpError, OpResult, V8Engine, Value};

/// Runtime-layer error (DECISIONS.md D12).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// An error from the engine layer, surfaced through the runtime.
    #[error(transparent)]
    Engine(#[from] es_runtime_engine::Error),
}

impl es_runtime_common::IntoException for Error {
    fn exception_class(&self) -> es_runtime_common::ExceptionClass {
        match self {
            Error::Engine(e) => e.exception_class(),
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

/// The embeddable runtime: an engine plus the driven loop's scheduling state.
pub struct Runtime {
    engine: Box<dyn Engine>,
    timers: TimerQueue,
    /// The runtime's current notion of time (embedder ms), last set by
    /// [`tick`](Self::tick). Timers created by [`eval`](Self::eval) between ticks
    /// are anchored here, so a `setTimeout(cb, d)` measures `d` from "now"
    /// rather than from whenever the next tick happens to arrive.
    now_ms: u64,
}

impl Runtime {
    /// Builds a runtime over the given engine.
    ///
    /// Taking a `Box<dyn Engine>` keeps the boundary clean: the caller chooses
    /// the engine (today [`V8Engine`]), the runtime drives it through the trait.
    pub fn new(engine: Box<dyn Engine>) -> Self {
        Runtime {
            engine,
            timers: TimerQueue::default(),
            now_ms: 0,
        }
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

    /// Compiles and runs `source`, returning the marshaled result. Pending work
    /// it schedules (async ops, timers) is advanced by subsequent [`tick`](Self::tick)s.
    pub fn eval(&mut self, source: &str) -> Result<Value> {
        let value = self.engine.eval(source)?;
        // Anchor any timers the script created at the current time, so their
        // delays are measured from now, not from the next tick's clock.
        self.drain_new_timers(self.now_ms);
        Ok(value)
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

        // 3. Microtask checkpoint (promise reactions, queueMicrotask).
        self.engine.run_microtasks();
        self.drain_new_timers(now_ms);

        // 4. Collect rejections that remained unhandled.
        let unhandled_rejections = self.engine.take_unhandled_rejections();

        TickStatus {
            timers_fired,
            async_ops_settled,
            unhandled_rejections,
            has_pending_work: self.has_pending_work(),
            next_timer_deadline_ms: self.timers.next_deadline_ms(),
        }
    }

    /// Whether any async op or timer is still outstanding.
    pub fn has_pending_work(&self) -> bool {
        self.engine.has_pending_async_ops() || !self.timers.is_empty()
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

    /// Serializes V8-touching tests in this binary (see the engine crate's note:
    /// V8's snapshot/isolate global state is not safe under the parallel harness).
    fn v8_guard() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn runtime() -> Runtime {
        let engine = V8Engine::new(Limits::default()).expect("engine");
        Runtime::new(Box::new(engine))
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
}
