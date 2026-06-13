//! Isolate + context lifecycle and script evaluation.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use es_runtime_common::{CapabilitySet, Limits};

use crate::convert::{describe_exception, marshal};
use crate::error::{Error, Result};
use crate::module::{ModuleEvalState, ModuleId, ModuleRegistry};
use crate::op::{OpDecl, OpState, TimerId, install_op};
use crate::value::Value;

/// A thread-safe handle for interrupting a running engine — typically held by a
/// watchdog thread that bounds execution time (SPEC §4). Calling
/// [`terminate`](Self::terminate) stops the engine's currently running
/// JavaScript as soon as V8 reaches an interruption point; the in-flight
/// [`Engine::eval`] then returns [`Error::Terminated`] rather than hanging.
///
/// Names no V8 type, so it stays within the engine boundary (DECISIONS.md D3).
/// It is `Send + Sync` (V8's `IsolateHandle` is), so the watchdog can live on
/// another thread while the engine is driven on its own.
#[derive(Clone)]
pub struct InterruptHandle(v8::IsolateHandle);

impl InterruptHandle {
    /// Terminates the engine's currently executing JavaScript. Safe to call from
    /// any thread and idempotent; a no-op if nothing is running.
    pub fn terminate(&self) {
        self.0.terminate_execution();
    }

    /// Whether the engine is currently in the terminating state (a `terminate`
    /// has fired and not yet been cleared). Lets a driver stop ticking a
    /// runtime that has been interrupted. Safe to call from any thread.
    pub fn is_terminating(&self) -> bool {
        self.0.is_execution_terminating()
    }
}

/// Data for the near-heap-limit callback. Kept behind a stable (boxed) address
/// for the isolate's lifetime so the raw `*mut c_void` we hand V8 stays valid.
struct HeapGuard {
    handle: v8::IsolateHandle,
    /// Set when the guard trips, read by `eval` to label the termination reason.
    tripped: Arc<AtomicBool>,
    /// Extra bytes granted to the callback's returned limit so V8 has room to
    /// unwind to the termination instead of hard-OOMing the process.
    headroom: usize,
}

/// Near-heap-limit callback: terminate execution (so the host never OOMs) and
/// grant a little headroom so V8 can unwind to that termination cleanly.
unsafe extern "C" fn near_heap_limit(
    data: *mut c_void,
    current_limit: usize,
    _initial_limit: usize,
) -> usize {
    // SAFETY: `data` is the `&HeapGuard` registered in `wire`, which lives in a
    // box owned by the `V8Engine` for at least as long as this isolate.
    let guard = unsafe { &*(data as *const HeapGuard) };
    guard.tripped.store(true, Ordering::SeqCst);
    guard.handle.terminate_execution();
    current_limit.saturating_add(guard.headroom)
}

/// The engine abstraction `runtime` depends on (ARCHITECTURE.md §3, DECISIONS.md
/// D3).
///
/// This is the boundary that lets a second engine be slotted in without editing
/// `runtime`: every method is expressed in plain Rust and [`es_runtime_common`]
/// types — no V8 type appears. [`V8Engine`] is the V8 implementation; a future
/// JavaScriptCore engine would implement the same trait.
///
/// The trait is object-safe so embedders can hold a `Box<dyn Engine>`.
pub trait Engine {
    /// Compiles and runs `source` in the engine's context, returning the
    /// marshaled result. A compile failure is [`Error::Compile`]; an uncaught JS
    /// exception is [`Error::Execution`] — never an unwind across the boundary.
    fn eval(&mut self, source: &str) -> Result<Value>;

    /// Registers a host op callable from JS as `globalThis.__ops.<name>`
    /// (ARCHITECTURE.md §4).
    fn register_op(&mut self, op: OpDecl) -> Result<()>;

    /// Replaces the capability set checked before capability-gated ops dispatch
    /// (DECISIONS.md D7). Deny-by-default: the initial set grants nothing.
    fn set_capabilities(&mut self, capabilities: CapabilitySet);

    /// Polls pending async ops once; resolves/rejects the JS promises of any
    /// that completed and returns how many were settled. Poll-on-tick: there is
    /// no reactor, so readiness is observed only when this is called
    /// (ARCHITECTURE.md §5).
    fn poll_async_ops(&mut self) -> usize;

    /// Whether any async op is still awaiting completion.
    fn has_pending_async_ops(&self) -> bool;

    /// Runs the V8 microtask checkpoint (drains the microtask queue, e.g.
    /// promise reactions). Microtasks are explicit, never auto-run mid-eval.
    fn run_microtasks(&mut self);

    /// Drains timers created by `setTimeout`/`setInterval` since the last call,
    /// as `(id, delay_ms, repeat)`. The embedder/`runtime` owns scheduling; the
    /// engine only holds the JS callbacks (ARCHITECTURE.md §5).
    fn take_new_timers(&mut self) -> Vec<(TimerId, u64, bool)>;

    /// Invokes the JS callback registered for timer `id`. Returns `false` if no
    /// such timer is active (already cleared). One-shot timers are removed; the
    /// caller is responsible for rescheduling repeating ones.
    fn fire_timer(&mut self, id: TimerId) -> bool;

    /// Whether timer `id` is still active (not cleared).
    fn timer_is_active(&self, id: TimerId) -> bool;

    /// Drains promise rejections that went unhandled since the last call, as
    /// their stringified messages (ARCHITECTURE.md §5).
    fn take_unhandled_rejections(&mut self) -> Vec<String>;

    /// Returns a thread-safe handle for interrupting this engine's execution
    /// (e.g. from a watchdog thread). See [`InterruptHandle`].
    fn interrupt_handle(&self) -> InterruptHandle;

    /// The capabilities currently granted (DECISIONS.md D7). Lets the caller gate
    /// effects it performs itself — e.g. `runtime` checks `FileSystem` before
    /// loading a module graph — using the same deny-by-default set the ops see.
    fn capabilities(&self) -> CapabilitySet;

    /// Compiles `source` as an ES module identified by `specifier`, returning its
    /// [`ModuleId`]. Compilation runs no user code; it only parses and records the
    /// module's imports (read via [`module_requests`](Self::module_requests)).
    fn compile_module(&mut self, specifier: &str, source: &str) -> Result<ModuleId>;

    /// The import specifiers `id` requests, in source order — for the caller to
    /// resolve and load before instantiation (V8 resolves synchronously, so the
    /// whole graph must be compiled first; ARCHITECTURE.md §5).
    fn module_requests(&mut self, id: ModuleId) -> Result<Vec<String>>;

    /// Instantiates `id`, resolving each `(referrer, specifier)` through
    /// `resolved`. Every referenced module must already be compiled. Wires the
    /// import graph; runs no user code.
    fn instantiate_module(
        &mut self,
        id: ModuleId,
        resolved: &HashMap<(ModuleId, String), ModuleId>,
    ) -> Result<()>;

    /// Begins evaluating instantiated module `id`. Evaluation returns a promise
    /// (top-level await); poll [`module_eval_state`](Self::module_eval_state)
    /// across ticks to observe completion or failure.
    fn evaluate_module(&mut self, id: ModuleId) -> Result<()>;

    /// The state of the most recent [`evaluate_module`](Self::evaluate_module):
    /// pending, completed, or failed (with the stringified reason).
    fn module_eval_state(&mut self) -> ModuleEvalState;
}

/// An embedded V8 instance: one isolate and one persistent context, plus the op
/// table and pending-work registries.
///
/// Created with [`V8Engine::new`] (fresh context) or [`V8Engine::with_snapshot`]
/// (context restored from a startup snapshot). State persists across [`Engine::eval`]
/// calls, since the context is held for the engine's lifetime.
///
/// The isolate is `!Send`/`!Sync` (V8's threading model): a `V8Engine` is driven
/// by a single thread, which is exactly the embedder's drive model
/// (ARCHITECTURE.md §5).
pub struct V8Engine {
    isolate: v8::OwnedIsolate,
    /// The persistent context evaluations run in, held across the isolate's life.
    context: v8::Global<v8::Context>,
    /// Op table + pending-work registries, shared with the in-isolate dispatch
    /// callback via an isolate slot (see [`OpState`]).
    op_state: std::rc::Rc<std::cell::RefCell<OpState>>,
    /// Compiled-module registry, shared with the in-isolate resolve and
    /// import-meta callbacks via an isolate slot (see [`ModuleRegistry`]).
    modules: std::rc::Rc<std::cell::RefCell<ModuleRegistry>>,
    /// When the engine was restored from a snapshot whose `__ops.<name>` shells
    /// are already baked in, [`register_op`](Engine::register_op) binds only the
    /// Rust handler (the JS function is present), rather than re-creating it. The
    /// caller must register ops in the **same order** used to build the snapshot
    /// so op ids line up (DECISIONS.md D8).
    ops_baked: bool,
    /// Thread-safe interrupt handle, cloned for [`InterruptHandle`] and used by
    /// `eval` to detect a watchdog/heap termination.
    interrupt: v8::IsolateHandle,
    /// Set by the near-heap-limit callback; lets `eval` label a termination as a
    /// heap-limit hit vs a watchdog interrupt.
    heap_tripped: Arc<AtomicBool>,
    /// Keeps the near-heap-limit callback's data alive for the isolate's life
    /// (its address was handed to V8). `None` for snapshot-builder engines,
    /// which install no heap guard. Dropped after `isolate` (field order).
    _heap_guard: Option<Box<HeapGuard>>,
}

impl V8Engine {
    /// Creates an engine with a fresh, empty context.
    ///
    /// `limits` are validated up front ([`Limits::validate`]); the heap ceiling
    /// is installed on the isolate so V8 enforces it (ARCHITECTURE.md §7). The
    /// near-limit graceful-termination callback is a later hardening item
    /// (Phase 9) — here the cap simply exists.
    pub fn new(limits: Limits) -> Result<Self> {
        limits.validate()?;
        crate::ensure_v8_initialized();

        let params = v8::CreateParams::default().heap_limits(0, limits.heap_limit_bytes);
        let isolate = v8::Isolate::new(params);
        Self::wire(
            isolate,
            false,
            Some(limits.heap_limit_bytes),
            limits.max_pending_ops as usize,
        )
    }

    /// Restores an engine from a startup snapshot built by
    /// [`snapshot::build`](crate::snapshot::build).
    ///
    /// With a snapshot blob installed, the new context is deserialized from the
    /// snapshot's default context — including any prelude baked into it
    /// (DECISIONS.md D8) — so global state captured at build time is present
    /// immediately.
    pub fn with_snapshot(limits: Limits, snapshot: Vec<u8>) -> Result<Self> {
        Self::restore(limits, snapshot, false)
    }

    /// Restores an engine from a snapshot whose `globalThis.__ops.<name>` shells
    /// and prelude are already baked in (built via [`V8Engine::build_snapshot`]).
    ///
    /// Unlike [`with_snapshot`](Self::with_snapshot), subsequent
    /// [`register_op`](Engine::register_op) calls bind **only the Rust handler** —
    /// the JS function is already present from the snapshot. Ops must be
    /// registered in the same order used at build time so ids line up
    /// (DECISIONS.md D8).
    pub fn with_snapshot_baked_ops(limits: Limits, snapshot: Vec<u8>) -> Result<Self> {
        Self::restore(limits, snapshot, true)
    }

    fn restore(limits: Limits, snapshot: Vec<u8>, ops_baked: bool) -> Result<Self> {
        limits.validate()?;
        crate::ensure_v8_initialized();

        // The blob may embed our native callbacks by external-reference index;
        // the same canonical list used at build must be supplied here (D8).
        let params = v8::CreateParams::default()
            .heap_limits(0, limits.heap_limit_bytes)
            .external_references(crate::op::external_references())
            .snapshot_blob(snapshot.into());
        let isolate = v8::Isolate::new(params);
        Self::wire(
            isolate,
            ops_baked,
            Some(limits.heap_limit_bytes),
            limits.max_pending_ops as usize,
        )
    }

    /// Builds a V8 startup-snapshot blob with the prelude and op shells baked in
    /// (DECISIONS.md D8).
    ///
    /// `configure` is run against a creator-backed engine — it registers the ops
    /// and evaluates the prelude exactly as a live engine would. Only the JS heap
    /// (the context, the `__ops.<name>` function shells with their op ids, and the
    /// prelude's global state) is serialized; the Rust handler closures are not,
    /// so they are rebound at restore by replaying the same registration order.
    ///
    /// # Concurrency
    ///
    /// V8 forbids snapshot creation concurrent with other isolate creation in the
    /// process; build once at startup before any [`V8Engine`] exists (a D3a note).
    pub fn build_snapshot<F>(limits: Limits, configure: F) -> Result<Vec<u8>>
    where
        F: FnOnce(&mut dyn Engine) -> Result<()>,
    {
        limits.validate()?;
        crate::ensure_v8_initialized();

        let creator = v8::Isolate::snapshot_creator(Some(crate::op::external_references()), None);
        // No heap guard for the short-lived builder isolate — its callback data
        // would dangle through `create_blob` (which itself runs a GC).
        let mut engine = Self::wire(creator, false, None, limits.max_pending_ops as usize)?;
        configure(&mut engine)?;
        engine.into_snapshot_blob()
    }

    /// Consumes a creator-backed engine: marks its context as the snapshot's
    /// default and serializes the blob. All live handles bar the default context
    /// are released first (a V8 requirement for `create_blob`).
    fn into_snapshot_blob(self) -> Result<Vec<u8>> {
        let V8Engine {
            mut isolate,
            context,
            op_state,
            ..
        } = self;
        {
            v8::scope!(let scope, &mut isolate);
            let local = v8::Local::new(scope, &context);
            scope.set_default_context(local);
        }
        // Release the only persistent handles before create_blob: the context
        // Global (the default context is now held by V8 itself) and the op-state
        // Rc (its timer/rejection registries are empty at build time).
        drop(context);
        drop(op_state);

        let blob = isolate.create_blob(v8::FunctionCodeHandling::Keep);
        blob.map(|data| data.to_vec())
            .ok_or_else(|| Error::Internal("V8 returned no snapshot blob".into()))
    }

    /// Common construction: configure the isolate, build the context, install
    /// the timer builtins, and wire the shared [`OpState`] into an isolate slot
    /// so the dispatch and reject callbacks can reach it.
    fn wire(
        mut isolate: v8::OwnedIsolate,
        ops_baked: bool,
        heap_limit: Option<usize>,
        max_pending_ops: usize,
    ) -> Result<Self> {
        // Microtasks run only at our explicit checkpoint, never implicitly when
        // a JS call returns — the embedder owns when reactions fire (D4).
        isolate.set_microtasks_policy(v8::MicrotasksPolicy::Explicit);

        let op_state = std::rc::Rc::new(std::cell::RefCell::new(OpState::new()));
        op_state.borrow_mut().set_max_pending_ops(max_pending_ops);
        // The dispatch callback and reject callback reach this via the slot.
        isolate.set_slot(op_state.clone());
        crate::op::install_promise_reject_callback(&mut isolate);

        // Module registry slot + the import.meta initializer. Both are reached by
        // the in-isolate module callbacks; the initializer is isolate-level config
        // (not serialized), so a snapshot-restored isolate gets it here too.
        let modules = std::rc::Rc::new(std::cell::RefCell::new(ModuleRegistry::new()));
        isolate.set_slot(modules.clone());
        crate::module::install_import_meta_callback(&mut isolate);

        let interrupt = isolate.thread_safe_handle();
        let heap_tripped = Arc::new(AtomicBool::new(false));

        // Install the near-heap-limit guard so a heap-bomb terminates cleanly
        // instead of OOM-ing the host (SPEC §4). Skipped for snapshot builders.
        let _heap_guard = heap_limit.map(|limit| {
            let guard = Box::new(HeapGuard {
                handle: interrupt.clone(),
                tripped: heap_tripped.clone(),
                // A little room (≥2 MiB) for V8 to unwind to the termination.
                headroom: (limit / 8).max(2 * 1024 * 1024),
            });
            let data = (&*guard as *const HeapGuard) as *mut c_void;
            isolate.add_near_heap_limit_callback(near_heap_limit, data);
            guard
        });

        let context = Self::make_context(&mut isolate);

        // Timer builtins (`setTimeout` &c.) are part of the driven loop, not a
        // user op; install them on the global once.
        {
            v8::scope!(let scope, &mut isolate);
            let context = v8::Local::new(scope, &context);
            let scope = &mut v8::ContextScope::new(scope, context);
            crate::op::install_timer_builtins(scope, context)?;
        }

        Ok(V8Engine {
            isolate,
            context,
            op_state,
            modules,
            ops_baked,
            interrupt,
            heap_tripped,
            _heap_guard,
        })
    }

    /// Builds a context in `isolate` and globalizes a handle to it. When the
    /// isolate was created with a snapshot blob, this restores the snapshot's
    /// default context rather than an empty one.
    fn make_context(isolate: &mut v8::OwnedIsolate) -> v8::Global<v8::Context> {
        v8::scope!(let scope, isolate);
        let context = v8::Context::new(scope, v8::ContextOptions::default());
        v8::Global::new(scope, context)
    }
}

impl Engine for V8Engine {
    fn eval(&mut self, source: &str) -> Result<Value> {
        // What an evaluation produced, computed inside the scope and acted on
        // after it is dropped (so the isolate is free for `cancel_terminate`).
        enum Outcome {
            Ok(Value),
            Compile(String),
            Execution(String),
            OverlongSource,
            Terminated,
        }

        // Cloned before the scope borrows `self.isolate`; reading the terminating
        // flag this way avoids a second borrow of the isolate.
        let interrupt = self.interrupt.clone();

        let outcome = {
            v8::scope!(let scope, &mut self.isolate);
            let context = v8::Local::new(scope, &self.context);
            let scope = &mut v8::ContextScope::new(scope, context);
            v8::tc_scope!(let scope, scope);

            if let Some(code) = v8::String::new(scope, source) {
                if let Some(script) = v8::Script::compile(scope, code, None) {
                    let run = script.run(scope);
                    // A watchdog/heap termination makes `run` return `None` with
                    // the terminating flag set; distinguish it from a normal throw.
                    if interrupt.is_execution_terminating() {
                        Outcome::Terminated
                    } else if let Some(result) = run {
                        Outcome::Ok(marshal(scope, result))
                    } else {
                        Outcome::Execution(describe_exception(scope, "execution failed"))
                    }
                } else if interrupt.is_execution_terminating() {
                    Outcome::Terminated
                } else {
                    Outcome::Compile(describe_exception(scope, "compilation failed"))
                }
            } else {
                Outcome::OverlongSource
            }
        };

        match outcome {
            Outcome::Ok(value) => Ok(value),
            Outcome::Compile(message) => Err(Error::Compile { message }),
            Outcome::Execution(message) => Err(Error::Execution { message }),
            Outcome::OverlongSource => Err(Error::Internal(
                "source string exceeds V8's maximum length".into(),
            )),
            Outcome::Terminated => {
                // Clear the terminating state so the isolate can be dropped (or,
                // in principle, reused) cleanly, and label the cause.
                self.isolate.cancel_terminate_execution();
                let reason = if self.heap_tripped.swap(false, Ordering::SeqCst) {
                    "heap limit exceeded"
                } else {
                    "execution terminated"
                };
                Err(Error::Terminated {
                    reason: reason.into(),
                })
            }
        }
    }

    fn register_op(&mut self, op: OpDecl) -> Result<()> {
        let op_id = self
            .op_state
            .borrow_mut()
            .add_op(op.required_capability, op.handler);
        // When the op shells are baked into a restored snapshot, the JS function
        // already exists — binding the handler (above) is all that is needed, and
        // re-creating the shell would be wasted work (DECISIONS.md D8).
        if self.ops_baked {
            return Ok(());
        }
        v8::scope!(let scope, &mut self.isolate);
        let context = v8::Local::new(scope, &self.context);
        let scope = &mut v8::ContextScope::new(scope, context);
        install_op(scope, context, &op.name, op_id)
    }

    fn set_capabilities(&mut self, capabilities: CapabilitySet) {
        self.op_state.borrow_mut().capabilities = capabilities;
    }

    fn poll_async_ops(&mut self) -> usize {
        crate::op::poll_async_ops(&mut self.isolate, &self.context, &self.op_state)
    }

    fn has_pending_async_ops(&self) -> bool {
        self.op_state.borrow().has_pending_async()
    }

    fn run_microtasks(&mut self) {
        self.isolate.perform_microtask_checkpoint();
    }

    fn take_new_timers(&mut self) -> Vec<(TimerId, u64, bool)> {
        self.op_state.borrow_mut().take_new_timers()
    }

    fn fire_timer(&mut self, id: TimerId) -> bool {
        crate::op::fire_timer(&mut self.isolate, &self.context, &self.op_state, id)
    }

    fn timer_is_active(&self, id: TimerId) -> bool {
        self.op_state.borrow().timer_is_active(id)
    }

    fn take_unhandled_rejections(&mut self) -> Vec<String> {
        crate::op::take_unhandled_rejections(&mut self.isolate, &self.context, &self.op_state)
    }

    fn interrupt_handle(&self) -> InterruptHandle {
        InterruptHandle(self.interrupt.clone())
    }

    fn capabilities(&self) -> CapabilitySet {
        self.op_state.borrow().capabilities
    }

    fn compile_module(&mut self, specifier: &str, source: &str) -> Result<ModuleId> {
        crate::module::compile(
            &mut self.isolate,
            &self.context,
            &self.modules,
            &self.interrupt,
            specifier,
            source,
        )
    }

    fn module_requests(&mut self, id: ModuleId) -> Result<Vec<String>> {
        crate::module::requests(&mut self.isolate, &self.context, &self.modules, id)
    }

    fn instantiate_module(
        &mut self,
        id: ModuleId,
        resolved: &HashMap<(ModuleId, String), ModuleId>,
    ) -> Result<()> {
        crate::module::instantiate(
            &mut self.isolate,
            &self.context,
            &self.modules,
            &self.interrupt,
            id,
            resolved,
        )
    }

    fn evaluate_module(&mut self, id: ModuleId) -> Result<()> {
        crate::module::evaluate(
            &mut self.isolate,
            &self.context,
            &self.modules,
            &self.interrupt,
            id,
        )
    }

    fn module_eval_state(&mut self) -> ModuleEvalState {
        crate::module::eval_state(&mut self.isolate, &self.context, &self.modules)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> V8Engine {
        V8Engine::new(Limits::default()).expect("engine construction")
    }

    #[test]
    fn evaluates_one_plus_one() {
        // The Phase 1 end-to-end acceptance check (SPEC.md §6.1).
        let _v8 = crate::v8_test_guard();
        let mut engine = engine();
        let result = engine.eval("1 + 1").expect("eval");
        assert_eq!(result, Value::Number(2.0));
    }

    #[test]
    fn marshals_primitive_kinds() {
        let _v8 = crate::v8_test_guard();
        let mut engine = engine();
        assert_eq!(engine.eval("undefined").unwrap(), Value::Undefined);
        assert_eq!(engine.eval("null").unwrap(), Value::Null);
        assert_eq!(engine.eval("true").unwrap(), Value::Bool(true));
        assert_eq!(engine.eval("2 + 3").unwrap(), Value::Number(5.0));
        assert_eq!(
            engine.eval("'a' + 'b'").unwrap(),
            Value::String("ab".into())
        );
    }

    #[test]
    fn non_primitive_falls_back_to_other() {
        let _v8 = crate::v8_test_guard();
        let mut engine = engine();
        match engine.eval("({})").unwrap() {
            Value::Other(s) => assert_eq!(s, "[object Object]"),
            other => panic!("expected Other, got {other:?}"),
        }
    }

    #[test]
    fn context_state_persists_across_evals() {
        let _v8 = crate::v8_test_guard();
        let mut engine = engine();
        engine.eval("globalThis.counter = 41").unwrap();
        assert_eq!(
            engine.eval("globalThis.counter + 1").unwrap(),
            Value::Number(42.0)
        );
    }

    #[test]
    fn syntax_error_is_typed_compile_error() {
        let _v8 = crate::v8_test_guard();
        let mut engine = engine();
        let err = engine.eval("function (").unwrap_err();
        assert!(matches!(err, Error::Compile { .. }), "got {err:?}");
    }

    #[test]
    fn thrown_exception_is_typed_execution_error() {
        let _v8 = crate::v8_test_guard();
        let mut engine = engine();
        let err = engine.eval("throw new Error('boom')").unwrap_err();
        match err {
            Error::Execution { message } => assert!(message.contains("boom"), "{message}"),
            other => panic!("expected Execution, got {other:?}"),
        }
    }

    #[test]
    fn invalid_limits_rejected_before_v8() {
        let bad = Limits::default().with_heap_limit_bytes(0);
        assert!(matches!(V8Engine::new(bad), Err(Error::Common(_))));
    }

    #[test]
    fn marshals_uint8array_to_bytes() {
        let _v8 = crate::v8_test_guard();
        let mut engine = engine();
        assert_eq!(
            engine.eval("new Uint8Array([1, 2, 3])").unwrap(),
            Value::Bytes(vec![1, 2, 3])
        );
    }

    #[test]
    fn bytes_round_trip_through_an_op() {
        use crate::OpDecl;
        let _v8 = crate::v8_test_guard();
        let mut engine = engine();
        engine
            .register_op(OpDecl::sync("echo", |mut args| Ok(args.remove(0))))
            .unwrap();
        // Uint8Array → Value::Bytes (in) → Value::Bytes → Uint8Array (out).
        let ok = engine
            .eval(
                "const a = __ops.echo(new Uint8Array([9, 8, 7])); \
                 a instanceof Uint8Array && a.length === 3 && a[0] === 9 && a[2] === 7",
            )
            .unwrap();
        assert_eq!(ok, Value::Bool(true));
    }

    #[test]
    fn watchdog_terminates_a_runaway_loop() {
        let _v8 = crate::v8_test_guard();
        let mut engine = engine();
        let handle = engine.interrupt_handle();
        // Interrupt from another thread, like a real execution-time watchdog.
        let watchdog = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            handle.terminate();
        });
        let err = engine.eval("while (true) {}").unwrap_err();
        watchdog.join().unwrap();
        match err {
            Error::Terminated { reason } => assert_eq!(reason, "execution terminated"),
            other => panic!("expected Terminated, got {other:?}"),
        }
        // The flag was cleared, so the engine is usable again.
        assert_eq!(engine.eval("1 + 1").unwrap(), Value::Number(2.0));
    }

    #[test]
    fn deep_recursion_surfaces_as_an_error_not_a_crash() {
        // V8's native stack guard turns unbounded recursion into a catchable
        // RangeError, so a stack-depth bomb is a typed error, never UB or a hang
        // (SPEC §4). No host stack guard is needed on top of this.
        let _v8 = crate::v8_test_guard();
        let mut engine = engine();
        let err = engine
            .eval("function f() { return 1 + f(); } f();")
            .unwrap_err();
        match err {
            Error::Execution { message } => {
                assert!(
                    message.contains("stack"),
                    "expected a stack error, got {message}"
                );
            }
            other => panic!("expected Execution, got {other:?}"),
        }
        // The engine recovers and runs normally afterwards.
        assert_eq!(engine.eval("2 + 2").unwrap(), Value::Number(4.0));
    }

    #[test]
    fn panicking_op_is_contained_as_a_js_exception() {
        // A host op that panics must surface as a catchable JS exception, never
        // unwind across V8's C++ frames or abort the process (DECISIONS.md D15).
        use crate::OpDecl;
        let _v8 = crate::v8_test_guard();
        let mut engine = engine();
        engine
            .register_op(OpDecl::sync("boom", |_args| panic!("intentional op panic")))
            .unwrap();
        let out = engine
            .eval(
                "try { __ops.boom(); 'no-throw' } \
                 catch (e) { 'caught:' + (e instanceof Error) }",
            )
            .unwrap();
        assert_eq!(out, Value::String("caught:true".into()));
        // The engine is still usable afterwards.
        assert_eq!(engine.eval("1 + 1").unwrap(), Value::Number(2.0));
    }

    #[test]
    fn pending_async_op_bound_is_enforced() {
        use crate::OpDecl;
        let _v8 = crate::v8_test_guard();
        let limits = Limits::default().with_max_pending_ops(2);
        let mut engine = V8Engine::new(limits).expect("engine");
        // An op whose promise never settles, so each call stays pending.
        engine
            .register_op(OpDecl::r#async("pend", |_args| {
                Box::pin(std::future::pending())
            }))
            .unwrap();
        let out = engine
            .eval(
                "__ops.pend(); __ops.pend(); \
                 try { __ops.pend(); 'allowed' } \
                 catch (e) { 'bounded:' + (e instanceof RangeError) }",
            )
            .unwrap();
        assert_eq!(out, Value::String("bounded:true".into()));
    }

    /// Drives the loop to quiescence for a module evaluation: poll async ops +
    /// run microtasks until the evaluation settles or a bounded number of turns
    /// elapse (so a genuinely-stuck graph fails the test rather than hanging).
    fn settle_module(engine: &mut V8Engine) -> ModuleEvalState {
        for _ in 0..100 {
            match engine.module_eval_state() {
                ModuleEvalState::Pending => {
                    engine.poll_async_ops();
                    engine.run_microtasks();
                }
                done => return done,
            }
        }
        engine.module_eval_state()
    }

    #[test]
    fn module_with_no_imports_evaluates_and_runs() {
        let _v8 = crate::v8_test_guard();
        let mut engine = engine();
        let id = engine
            .compile_module("file:///main.mjs", "globalThis.ran = 7;")
            .expect("compile");
        assert!(engine.module_requests(id).unwrap().is_empty());
        engine
            .instantiate_module(id, &HashMap::new())
            .expect("instantiate");
        engine.evaluate_module(id).expect("evaluate");
        assert_eq!(settle_module(&mut engine), ModuleEvalState::Completed);
        assert_eq!(engine.eval("globalThis.ran").unwrap(), Value::Number(7.0));
    }

    #[test]
    fn two_module_graph_resolves_imports() {
        let _v8 = crate::v8_test_guard();
        let mut engine = engine();
        let util = engine
            .compile_module("file:///util.mjs", "export const v = 42;")
            .expect("compile util");
        let main = engine
            .compile_module(
                "file:///main.mjs",
                "import { v } from './util.mjs'; globalThis.result = v + 1;",
            )
            .expect("compile main");
        assert_eq!(engine.module_requests(main).unwrap(), ["./util.mjs"]);

        let mut resolved = HashMap::new();
        resolved.insert((main, "./util.mjs".to_string()), util);
        engine
            .instantiate_module(main, &resolved)
            .expect("instantiate");
        engine.evaluate_module(main).expect("evaluate");

        assert_eq!(settle_module(&mut engine), ModuleEvalState::Completed);
        assert_eq!(
            engine.eval("globalThis.result").unwrap(),
            Value::Number(43.0)
        );
    }

    #[test]
    fn top_level_await_settles_after_polling() {
        use crate::op::AsyncOp;
        let _v8 = crate::v8_test_guard();
        let mut engine = engine();
        engine
            .register_op(OpDecl::r#async("answer", |_args| -> AsyncOp {
                Box::pin(async { Ok(Value::Number(99.0)) })
            }))
            .unwrap();
        let id = engine
            .compile_module(
                "file:///tla.mjs",
                "const v = await __ops.answer(); globalThis.tla = v;",
            )
            .expect("compile");
        engine
            .instantiate_module(id, &HashMap::new())
            .expect("instantiate");
        engine.evaluate_module(id).expect("evaluate");
        // The graph is async (TLA), so it is not done the instant evaluate returns.
        assert_eq!(engine.module_eval_state(), ModuleEvalState::Pending);
        assert_eq!(settle_module(&mut engine), ModuleEvalState::Completed);
        assert_eq!(engine.eval("globalThis.tla").unwrap(), Value::Number(99.0));
    }

    #[test]
    fn top_level_throw_surfaces_as_failed() {
        let _v8 = crate::v8_test_guard();
        let mut engine = engine();
        let id = engine
            .compile_module("file:///boom.mjs", "throw new Error('boom');")
            .expect("compile");
        engine
            .instantiate_module(id, &HashMap::new())
            .expect("instantiate");
        engine.evaluate_module(id).expect("evaluate");
        match settle_module(&mut engine) {
            ModuleEvalState::Failed(message) => assert!(message.contains("boom"), "{message}"),
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn import_meta_url_is_the_specifier() {
        let _v8 = crate::v8_test_guard();
        let mut engine = engine();
        let id = engine
            .compile_module(
                "file:///app/main.mjs",
                "globalThis.metaUrl = import.meta.url;",
            )
            .expect("compile");
        engine
            .instantiate_module(id, &HashMap::new())
            .expect("instantiate");
        engine.evaluate_module(id).expect("evaluate");
        assert_eq!(settle_module(&mut engine), ModuleEvalState::Completed);
        assert_eq!(
            engine.eval("globalThis.metaUrl").unwrap(),
            Value::String("file:///app/main.mjs".into())
        );
    }

    #[test]
    fn missing_resolution_fails_instantiation() {
        let _v8 = crate::v8_test_guard();
        let mut engine = engine();
        let main = engine
            .compile_module("file:///main.mjs", "import './gone.mjs';")
            .expect("compile");
        // No entry for ('./gone.mjs') in the resolve map → resolve callback
        // returns None → V8 throws during instantiation.
        let err = engine
            .instantiate_module(main, &HashMap::new())
            .unwrap_err();
        assert!(matches!(err, Error::Execution { .. }), "got {err:?}");
    }

    #[test]
    fn compiling_invalid_module_is_a_compile_error() {
        let _v8 = crate::v8_test_guard();
        let mut engine = engine();
        let err = engine
            .compile_module("file:///bad.mjs", "export const = ;")
            .unwrap_err();
        assert!(matches!(err, Error::Compile { .. }), "got {err:?}");
    }

    #[test]
    fn heap_guard_terminates_a_heap_bomb_without_oom() {
        let _v8 = crate::v8_test_guard();
        // A small heap cap so the bomb trips quickly; the guard must terminate
        // before V8 hard-OOMs the process (SPEC §4).
        let limits = Limits::default().with_heap_limit_bytes(16 * 1024 * 1024);
        let mut engine = V8Engine::new(limits).expect("engine");
        let err = engine
            .eval("const a = []; for (;;) { a.push(new Array(100000).fill(7)); }")
            .unwrap_err();
        match err {
            Error::Terminated { reason } => {
                assert!(
                    reason.contains("heap"),
                    "expected heap reason, got {reason}"
                );
            }
            other => panic!("expected Terminated, got {other:?}"),
        }
    }
}
