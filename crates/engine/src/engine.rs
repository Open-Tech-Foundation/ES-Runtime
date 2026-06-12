//! Isolate + context lifecycle and script evaluation.

use es_runtime_common::{CapabilitySet, Limits};

use crate::convert::{describe_exception, marshal};
use crate::error::{Error, Result};
use crate::op::{OpDecl, OpState, TimerId, install_op};
use crate::value::Value;

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
    /// When the engine was restored from a snapshot whose `__ops.<name>` shells
    /// are already baked in, [`register_op`](Engine::register_op) binds only the
    /// Rust handler (the JS function is present), rather than re-creating it. The
    /// caller must register ops in the **same order** used to build the snapshot
    /// so op ids line up (DECISIONS.md D8).
    ops_baked: bool,
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
        Self::wire(isolate, false)
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
        Self::wire(isolate, ops_baked)
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
        let mut engine = Self::wire(creator, false)?;
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
    fn wire(mut isolate: v8::OwnedIsolate, ops_baked: bool) -> Result<Self> {
        // Microtasks run only at our explicit checkpoint, never implicitly when
        // a JS call returns — the embedder owns when reactions fire (D4).
        isolate.set_microtasks_policy(v8::MicrotasksPolicy::Explicit);

        let op_state = std::rc::Rc::new(std::cell::RefCell::new(OpState::new()));
        // The dispatch callback and reject callback reach this via the slot.
        isolate.set_slot(op_state.clone());
        crate::op::install_promise_reject_callback(&mut isolate);

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
            ops_baked,
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
        v8::scope!(let scope, &mut self.isolate);
        let context = v8::Local::new(scope, &self.context);
        let scope = &mut v8::ContextScope::new(scope, context);
        v8::tc_scope!(let scope, scope);

        let Some(code) = v8::String::new(scope, source) else {
            return Err(Error::Internal(
                "source string exceeds V8's maximum length".into(),
            ));
        };

        let Some(script) = v8::Script::compile(scope, code, None) else {
            return Err(Error::Compile {
                message: describe_exception(scope, "compilation failed"),
            });
        };

        let Some(result) = script.run(scope) else {
            return Err(Error::Execution {
                message: describe_exception(scope, "execution failed"),
            });
        };

        Ok(marshal(scope, result))
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
}
