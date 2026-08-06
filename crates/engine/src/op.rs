//! The op system: the bridge between JS and Rust host functionality
//! (ARCHITECTURE.md §4), plus the V8-side machinery the driven loop needs
//! (async-op promise resolution, timers, unhandled-rejection tracking).
//!
//! All of this is V8-coupled and therefore lives in `engine`, behind the
//! [`Engine`](crate::Engine) trait. The orchestration — *when* to poll, fire
//! timers, and run microtasks — lives in `runtime` (ARCHITECTURE.md §5); this
//! module only provides the primitives.
//!
//! Dispatch uses a single non-capturing callback ([`op_dispatch`]) installed for
//! every op, distinguished by the op id carried in the function's `data`. The op
//! table and pending-work registries live in [`OpState`], stored in an isolate
//! slot as `Rc<RefCell<…>>` so the callback can reach them without borrowing the
//! isolate (which it is already holding as the scope).

use std::cell::RefCell;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use es_runtime_common::{
    Capability, CapabilitySet, Error as CommonError, ErrorCode, ExceptionClass, IntoException,
};

use crate::convert::{build_exception, marshal, throw, value_to_js};
use crate::error::{Error, Result};
use crate::value::Value;

/// Identifier for a scheduled timer, returned by `setTimeout`/`setInterval`.
pub type TimerId = u64;

/// A rejected promise and the reason it rejected with — the two values an
/// `unhandledrejection` event carries.
type Rejection = (v8::Global<v8::Promise>, v8::Global<v8::Value>);

/// The result a host op produces: a [`Value`] or an [`OpError`] to throw.
pub type OpResult = std::result::Result<Value, OpError>;

/// A pending async op: a future resolving to an [`OpResult`], polled on tick.
pub type AsyncOp = Pin<Box<dyn Future<Output = OpResult>>>;

/// An error a host op raises, carrying the JS exception class it surfaces as
/// and, when one exists, the stable guest-facing [`ErrorCode`] set as the JS
/// exception's `code` property (SPEC §6 Phase 13).
#[derive(Debug, Clone)]
pub struct OpError {
    class: ExceptionClass,
    message: String,
    code: Option<ErrorCode>,
}

impl OpError {
    /// Constructs an op error with an explicit JS exception class.
    pub fn new(class: ExceptionClass, message: impl Into<String>) -> Self {
        OpError {
            class,
            message: message.into(),
            code: None,
        }
    }

    /// Attaches a stable guest-facing code (builder style).
    #[must_use]
    pub fn with_code(mut self, code: ErrorCode) -> Self {
        self.code = Some(code);
        self
    }

    /// Attaches a code when one is present (builder style) — the common shape
    /// when forwarding a [`ProviderError`]'s optional classification.
    #[must_use]
    pub fn with_code_opt(mut self, code: Option<ErrorCode>) -> Self {
        self.code = code;
        self
    }

    /// A `TypeError` — the usual class for a bad argument from JS.
    pub fn type_error(message: impl Into<String>) -> Self {
        OpError::new(ExceptionClass::TypeError, message)
    }

    /// A `RangeError`.
    pub fn range_error(message: impl Into<String>) -> Self {
        OpError::new(ExceptionClass::RangeError, message)
    }
}

impl std::fmt::Display for OpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for OpError {}

impl IntoException for OpError {
    fn exception_class(&self) -> ExceptionClass {
        self.class
    }

    fn exception_code(&self) -> Option<ErrorCode> {
        self.code
    }
}

/// A host handler invoked when JS calls an op.
///
/// Handlers receive marshaled arguments and are `FnMut` so they may own and
/// mutate host state across calls. A sync handler returns immediately; an async
/// handler returns a future tracked as pending work.
pub enum OpHandler {
    /// Executes and returns immediately.
    Sync(Box<dyn FnMut(Vec<Value>) -> OpResult>),
    /// Returns a future; the op's JS `Promise` resolves when it completes.
    Async(Box<dyn FnMut(Vec<Value>) -> AsyncOp>),
}

/// A registration request for one op: its JS name, the capabilities it requires
/// (if any), and its handler.
pub struct OpDecl {
    /// The name the op is exposed as: `globalThis.__ops.<name>`.
    pub name: String,
    /// Every capability checked before dispatch (ARCHITECTURE.md §4); empty for
    /// pure, non-side-effecting ops. **All** must be held — an op that both
    /// reads and writes (a file copy, say) is only as available as its
    /// scarcer grant, and gating it on one of the two would be a hole.
    pub required_capabilities: Vec<Capability>,
    /// The handler to invoke.
    pub handler: OpHandler,
    /// Whether a call to this op that is still in flight should keep the
    /// embedder's loop running. See [`unref`](Self::unref).
    pub keeps_loop_alive: bool,
}

impl OpDecl {
    /// Declares a synchronous op.
    pub fn sync(
        name: impl Into<String>,
        handler: impl FnMut(Vec<Value>) -> OpResult + 'static,
    ) -> Self {
        OpDecl {
            name: name.into(),
            required_capabilities: Vec::new(),
            handler: OpHandler::Sync(Box::new(handler)),
            keeps_loop_alive: true,
        }
    }

    /// Declares an asynchronous op.
    pub fn r#async(
        name: impl Into<String>,
        handler: impl FnMut(Vec<Value>) -> AsyncOp + 'static,
    ) -> Self {
        OpDecl {
            name: name.into(),
            required_capabilities: Vec::new(),
            handler: OpHandler::Async(Box::new(handler)),
            keeps_loop_alive: true,
        }
    }

    /// Adds a capability required before this op may dispatch (builder style).
    ///
    /// Additive: calling it twice demands both. An op is expected to name every
    /// authority it actually exercises, so a call that reads one path and writes
    /// another says so rather than picking whichever gate is convenient.
    #[must_use]
    pub fn requires(mut self, capability: Capability) -> Self {
        if !self.required_capabilities.contains(&capability) {
            self.required_capabilities.push(capability);
        }
        self
    }

    /// Marks this op as **not** keeping the loop alive on its own — Node's
    /// `unref`.
    ///
    /// An in-flight call is still polled and still resolves; it simply stops
    /// counting towards [`has_pending_async_ops`](crate::Engine::has_pending_async_ops),
    /// so a loop with nothing else to do may finish rather than waiting on it
    /// forever.
    ///
    /// For a receive that only something *inside this agent* could satisfy. A
    /// `MessagePort` whose peer is in this isolate is the case: with the loop
    /// otherwise idle there is no code left to post to it, so waiting is
    /// provably futile, and a program that merely opened a channel would
    /// otherwise never exit. Do **not** use it where the sender is elsewhere —
    /// another agent, a socket, a timer — because there the wait is exactly the
    /// point.
    #[must_use]
    pub fn unref(mut self) -> Self {
        self.keeps_loop_alive = false;
        self
    }
}

struct OpEntry {
    required_capabilities: Vec<Capability>,
    handler: OpHandler,
    keeps_loop_alive: bool,
}

struct PendingAsync {
    future: AsyncOp,
    resolver: v8::Global<v8::PromiseResolver>,
    /// Copied from the op's declaration; see [`OpDecl::unref`].
    keeps_loop_alive: bool,
}

struct TimerEntry {
    callback: v8::Global<v8::Function>,
    repeat: bool,
    /// Arguments after `delay`, forwarded to the callback on every firing
    /// (`setTimeout(fn, ms, a, b)` calls `fn(a, b)`).
    args: Vec<v8::Global<v8::Value>>,
}

/// Op table and pending-work registries, shared with the in-isolate dispatch
/// and reject callbacks via an isolate slot.
pub(crate) struct OpState {
    ops: Vec<OpEntry>,
    /// Capabilities granted to ops; deny-by-default (DECISIONS.md D7).
    pub capabilities: CapabilitySet,
    pending_async: Vec<PendingAsync>,
    timers: HashMap<TimerId, TimerEntry>,
    /// Timers created since the last [`take_new_timers`](Self::take_new_timers),
    /// reported to `runtime` for scheduling: `(id, delay_ms, repeat)`.
    new_timers: Vec<(TimerId, u64, bool)>,
    next_timer_id: TimerId,
    /// Promises rejected without a handler, keyed by identity hash so a later
    /// "handler added" event can revoke the entry. The promise is kept
    /// alongside the reason because the `unhandledrejection` event carries both.
    unhandled_rejections: HashMap<i32, Rejection>,
    /// Identity hashes of rejections already dispatched *and* reported, so that
    /// attaching a handler afterwards can fire `rejectionhandled` — the spec's
    /// retraction of a report that has already gone out. Only the key is kept
    /// (four bytes): V8 hands the promise itself back in the callback, so
    /// nothing needs to be retained alive for it.
    reported_rejections: std::collections::HashSet<i32>,
    /// Promises whose earlier unhandled report has just been retracted, awaiting
    /// a `rejectionhandled` dispatch on the next drain.
    rejections_handled: Vec<v8::Global<v8::Promise>>,
    /// Exceptions that escaped a host-invoked callback (a timer) and that no
    /// `error` listener claimed, formatted for the embedder to report.
    uncaught_errors: Vec<String>,
    /// Upper bound on concurrently pending async ops (DECISIONS.md D7 / SPEC §4).
    /// Dispatching a new async op past this throws, so adversarial JS can't pile
    /// up unbounded host work. `usize::MAX` until set from the engine's limits.
    max_pending_ops: usize,
    /// Waker used to poll pending async-op futures. An embedder with a reactor
    /// (e.g. the standalone driver) injects a real waker so a future signals
    /// readiness the instant its backing task makes progress, instead of being
    /// re-polled on a blind interval. `None` ⇒ a no-op waker (poll-on-tick).
    async_waker: Option<Waker>,
    /// `WebAssembly` async compiles/instantiates currently in flight, reported by
    /// the prelude wrappers. V8 runs these on its own threads with no op-future
    /// to poll, so this count is what keeps the driver's loop alive until they
    /// settle (see [`Engine::has_pending_wasm`](crate::Engine::has_pending_wasm)).
    pub pending_wasm: u32,
    /// Compiled WebAssembly modules awaiting instantiation by the JS wrapper the
    /// ESM integration generates, keyed by the handle baked into that wrapper.
    /// Read back through the `__wasm_module` builtin.
    pub wasm_modules: HashMap<u32, v8::Global<v8::WasmModuleObject>>,
    /// Next handle handed out by [`Engine::compile_wasm`](crate::Engine::compile_wasm).
    pub next_wasm_handle: u32,
}

impl OpState {
    pub(crate) fn new() -> Self {
        OpState {
            ops: Vec::new(),
            capabilities: CapabilitySet::none(),
            pending_async: Vec::new(),
            timers: HashMap::new(),
            new_timers: Vec::new(),
            next_timer_id: 1,
            unhandled_rejections: HashMap::new(),
            reported_rejections: std::collections::HashSet::new(),
            rejections_handled: Vec::new(),
            uncaught_errors: Vec::new(),
            max_pending_ops: usize::MAX,
            async_waker: None,
            pending_wasm: 0,
            wasm_modules: HashMap::new(),
            next_wasm_handle: 1,
        }
    }

    /// Sets the bound on concurrently pending async ops (from engine limits).
    pub(crate) fn set_max_pending_ops(&mut self, max: usize) {
        self.max_pending_ops = max;
    }

    /// Injects the waker used when polling pending async ops (see the field).
    pub(crate) fn set_async_waker(&mut self, waker: Waker) {
        self.async_waker = Some(waker);
    }

    /// Adds an op to the table and returns its id (its index).
    pub(crate) fn add_op(
        &mut self,
        required_capabilities: Vec<Capability>,
        handler: OpHandler,
        keeps_loop_alive: bool,
    ) -> i32 {
        let id = self.ops.len() as i32;
        self.ops.push(OpEntry {
            required_capabilities,
            handler,
            keeps_loop_alive,
        });
        id
    }

    /// Whether any async op is still awaiting completion.
    /// Whether any in-flight async op should hold the loop open. An unref'd op
    /// is still pending and still polled — it just does not, by itself, stop
    /// the embedder finishing (see [`OpDecl::unref`]).
    pub(crate) fn has_pending_async(&self) -> bool {
        self.pending_async.iter().any(|op| op.keeps_loop_alive)
    }

    /// Drains timers created since the previous call.
    pub(crate) fn take_new_timers(&mut self) -> Vec<(TimerId, u64, bool)> {
        std::mem::take(&mut self.new_timers)
    }

    /// Whether timer `id` is still active (not cleared).
    pub(crate) fn timer_is_active(&self, id: TimerId) -> bool {
        self.timers.contains_key(&id)
    }
}

/// Clones the `Rc<RefCell<OpState>>` out of the isolate slot, if present. The
/// clone decouples op-state access from the isolate borrow held by the scope.
fn op_state(scope: &v8::PinScope<'_, '_>) -> Option<Rc<RefCell<OpState>>> {
    scope.get_slot::<Rc<RefCell<OpState>>>().cloned()
}

/// The op callback V8 invokes. Wraps [`op_dispatch_inner`] in `catch_unwind` so
/// a panic in a host op handler (or in marshaling) is contained as a JS
/// exception instead of unwinding across V8's C++ frames, which is undefined
/// behaviour (DECISIONS.md D15). Containment assumes `panic = "unwind"`; under
/// `panic = "abort"` the process aborts, which is then the chosen policy.
fn op_dispatch(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    rv: v8::ReturnValue<v8::Value>,
) {
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        op_dispatch_inner(&mut *scope, args, rv);
    }));
    if caught.is_err() && !scope.is_execution_terminating() {
        throw(
            scope,
            &OpError::new(ExceptionClass::Error, "internal error in host op"),
        );
    }
}

fn op_dispatch_inner(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    let op_id = args.data().int32_value(scope).unwrap_or(-1);

    let Some(state_rc) = op_state(scope) else {
        throw(
            scope,
            &OpError::new(ExceptionClass::Error, "op state unavailable"),
        );
        return;
    };

    // Marshal every argument before touching op state, so the scope is free.
    let argc = args.length();
    let mut argv = Vec::with_capacity(argc.max(0) as usize);
    for i in 0..argc {
        argv.push(marshal(scope, args.get(i)));
    }

    let mut state = state_rc.borrow_mut();
    let idx = op_id as usize;
    if op_id < 0 || idx >= state.ops.len() {
        drop(state);
        throw(
            scope,
            &OpError::new(ExceptionClass::Error, format!("unknown op id {op_id}")),
        );
        return;
    }

    // Capability check first: a denial is a clean exception, never a partial
    // effect (ARCHITECTURE.md §4).
    // Every named capability must be held: the first one missing is the one
    // reported, so the message names something the caller can act on.
    if let Some(missing) = state.ops[idx]
        .required_capabilities
        .iter()
        .copied()
        .find(|cap| !state.capabilities.contains(*cap))
    {
        drop(state);
        throw(scope, &CommonError::CapabilityDenied(missing));
        return;
    }

    // Compute the outcome under the field borrow, then release it before using
    // the scope to marshal results / build the promise.
    enum Outcome {
        Ok(Value),
        Err(OpError),
        Async(AsyncOp),
    }
    // Read before the handler borrow, so it is available when the pending entry
    // is built below.
    let keeps_loop_alive = state.ops[idx].keeps_loop_alive;
    let outcome = match &mut state.ops[idx].handler {
        OpHandler::Sync(handler) => match handler(argv) {
            Ok(value) => Outcome::Ok(value),
            Err(err) => Outcome::Err(err),
        },
        OpHandler::Async(handler) => Outcome::Async(handler(argv)),
    };

    match outcome {
        Outcome::Ok(value) => {
            drop(state);
            let js = value_to_js(scope, value);
            rv.set(js);
        }
        Outcome::Err(err) => {
            drop(state);
            throw(scope, &err);
        }
        Outcome::Async(mut future) => {
            // Poll once before registering. Plenty of "async" ops are only async
            // in shape: a provider that answers from memory, or one whose work is
            // a single fast syscall, hands back `std::future::ready(..)` — the
            // filesystem's `exists` is exactly that. Registering such a future
            // costs it a full trip out to the driver and back before its promise
            // can settle, so a guest awaiting them in sequence pays one loop
            // round trip per call: 11.4µs each on the bench's `exists` row,
            // against a `try_exists` syscall of well under one.
            //
            // This does not make the op synchronous. The promise is resolved, not
            // its reactions run — those still wait for the microtask checkpoint,
            // so `await` suspends and resumes exactly as before. What goes away is
            // only the wait for a loop turn that had nothing left to do.
            //
            // Polled with the driver's waker (not a no-op one): a future that
            // registers the waker and *then* returns `Pending` must still be able
            // to wake the loop, or it would sit until the fallback timer.
            let eager = {
                let waker = state
                    .async_waker
                    .clone()
                    .unwrap_or_else(|| Waker::noop().clone());
                let mut cx = Context::from_waker(&waker);
                match future.as_mut().poll(&mut cx) {
                    Poll::Ready(result) => Some(result),
                    Poll::Pending => None,
                }
            };
            if let Some(result) = eager {
                drop(state);
                let Some(resolver) = v8::PromiseResolver::new(scope) else {
                    throw(
                        scope,
                        &OpError::new(ExceptionClass::Error, "could not create promise"),
                    );
                    return;
                };
                let promise = resolver.get_promise(scope);
                match result {
                    Ok(value) => {
                        let js = value_to_js(scope, value);
                        resolver.resolve(scope, js);
                    }
                    Err(err) => {
                        let exception = build_exception(scope, &err);
                        resolver.reject(scope, exception);
                    }
                }
                rv.set(promise.into());
                return;
            }

            // Bound in-flight async work so adversarial JS can't pile up
            // unbounded pending ops (SPEC §4).
            if state.pending_async.len() >= state.max_pending_ops {
                drop(state);
                throw(
                    scope,
                    &OpError::range_error("too many concurrent async operations"),
                );
                return;
            }
            let Some(resolver) = v8::PromiseResolver::new(scope) else {
                drop(state);
                throw(
                    scope,
                    &OpError::new(ExceptionClass::Error, "could not create promise"),
                );
                return;
            };
            let promise = resolver.get_promise(scope);
            let resolver = v8::Global::new(scope, resolver);
            state.pending_async.push(PendingAsync {
                future,
                resolver,
                keeps_loop_alive,
            });
            // Wake the driver so it re-ticks and polls this future now, rather
            // than parking first: the future hasn't been polled yet, so it has
            // registered no waker and nothing else would wake the loop until a
            // blind fallback — the per-op latency that dominates sequential I/O.
            if let Some(waker) = &state.async_waker {
                waker.wake_by_ref();
            }
            drop(state);
            rv.set(promise.into());
        }
    }
}

/// The canonical list of native callbacks that may be embedded in a snapshotted
/// heap, in a **fixed order**. V8 matches external references by index (not by
/// address), so the same list — built fresh each call, which is correct under
/// ASLR — must be supplied at both snapshot creation and restore
/// (DECISIONS.md D8). Every native callback reachable from a `v8::Function`
/// that can survive into a snapshot belongs here: the op dispatcher and the two
/// timer setters/clearers. (The promise-reject callback is isolate-level
/// configuration re-applied at restore, not a heap-embedded reference.)
pub(crate) fn external_references() -> std::borrow::Cow<'static, [v8::ExternalReference]> {
    use v8::MapFnTo;
    let mut refs = vec![
        v8::ExternalReference {
            function: op_dispatch.map_fn_to(),
        },
        v8::ExternalReference {
            function: timer_set.map_fn_to(),
        },
        v8::ExternalReference {
            function: timer_clear.map_fn_to(),
        },
        v8::ExternalReference {
            function: wasm_pending.map_fn_to(),
        },
        v8::ExternalReference {
            function: wasm_module.map_fn_to(),
        },
    ];
    // Appended last so the indices above are unchanged; the two
    // structured-clone builtins are heap-reachable from a snapshot the same way
    // the timer setters are.
    refs.extend(crate::serialize::external_references());
    std::borrow::Cow::Owned(refs)
}

/// Installs op `op_id` as `globalThis.__ops.<name>`, creating the `__ops`
/// holder object on first use.
pub(crate) fn install_op(
    scope: &mut v8::PinScope,
    context: v8::Local<v8::Context>,
    name: &str,
    op_id: i32,
) -> Result<()> {
    let global = context.global(scope);
    let ops_key = string(scope, "__ops")?;
    let existing = global.get(scope, ops_key.into());
    let ops_obj: v8::Local<v8::Object> = match existing {
        Some(v) if v.is_object() => v.try_into().expect("checked is_object"),
        _ => {
            let obj = v8::Object::new(scope);
            global.set(scope, ops_key.into(), obj.into());
            obj
        }
    };

    let data = v8::Integer::new(scope, op_id);
    let func = v8::Function::builder(op_dispatch)
        .data(data.into())
        .build(scope)
        .ok_or_else(|| Error::Internal("could not build op function".into()))?;
    let name_key = string(scope, name)?;
    ops_obj.set(scope, name_key.into(), func.into());
    Ok(())
}

/// Polls every pending async op once; resolves/rejects the promises of those
/// that completed and returns how many were settled (ARCHITECTURE.md §5).
pub(crate) fn poll_async_ops(
    isolate: &mut v8::OwnedIsolate,
    context: &v8::Global<v8::Context>,
    op_state: &Rc<RefCell<OpState>>,
) -> usize {
    // Poll with the embedder's waker if one was injected (a driver with a
    // reactor wires this so a ready future wakes it immediately); otherwise a
    // no-op waker, and readiness is observed only on the next tick.
    let waker = op_state
        .borrow()
        .async_waker
        .clone()
        .unwrap_or_else(|| Waker::noop().clone());
    let mut cx = Context::from_waker(&waker);

    let mut ready: Vec<(v8::Global<v8::PromiseResolver>, OpResult)> = Vec::new();
    {
        let mut state = op_state.borrow_mut();
        let mut i = 0;
        while i < state.pending_async.len() {
            match state.pending_async[i].future.as_mut().poll(&mut cx) {
                Poll::Ready(result) => {
                    let done = state.pending_async.remove(i);
                    ready.push((done.resolver, result));
                }
                Poll::Pending => i += 1,
            }
        }
    }

    if ready.is_empty() {
        return 0;
    }
    let settled = ready.len();

    v8::scope!(let scope, isolate);
    let context = v8::Local::new(scope, context);
    let scope = &mut v8::ContextScope::new(scope, context);
    for (resolver, result) in ready {
        let resolver = v8::Local::new(scope, &resolver);
        match result {
            Ok(value) => {
                let js = value_to_js(scope, value);
                resolver.resolve(scope, js);
            }
            Err(err) => {
                let exception = build_exception(scope, &err);
                resolver.reject(scope, exception);
            }
        }
    }
    settled
}

/// Installs the four timer builtins (`setTimeout`, `setInterval`,
/// `clearTimeout`, `clearInterval`) on the global. The repeat flag rides in each
/// setter's `data`.
pub(crate) fn install_timer_builtins(
    scope: &mut v8::PinScope,
    context: v8::Local<v8::Context>,
) -> Result<()> {
    let global = context.global(scope);
    install_global_fn(scope, global, "setTimeout", timer_set, Some(false))?;
    install_global_fn(scope, global, "setInterval", timer_set, Some(true))?;
    install_global_fn(scope, global, "clearTimeout", timer_clear, None)?;
    install_global_fn(scope, global, "clearInterval", timer_clear, None)?;
    Ok(())
}

/// Installs `__wasm_pending(delta)`, through which the prelude's `WebAssembly`
/// wrappers report async compiles entering (+1) and leaving (-1) flight.
///
/// This is a builtin rather than a regular op because the count lives in
/// [`OpState`], which op handlers (plain closures over their provider) cannot
/// reach — the same reason the timer builtins are installed here.
pub(crate) fn install_wasm_builtins(
    scope: &mut v8::PinScope,
    context: v8::Local<v8::Context>,
) -> Result<()> {
    let global = context.global(scope);
    install_global_fn(scope, global, "__wasm_pending", wasm_pending, None)?;
    install_global_fn(scope, global, "__wasm_module", wasm_module, None)
}

/// `__wasm_module(handle)`: hands back the compiled module the host stashed for
/// the WebAssembly ES-module integration, so the generated wrapper can
/// instantiate it without the bytes making a round trip through JS.
///
/// Guest-reachable but not a capability: the only thing it can return is a
/// module the guest's own `import` already caused to be compiled.
fn wasm_module(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    rv: v8::ReturnValue<v8::Value>,
) {
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        wasm_module_inner(&mut *scope, args, rv);
    }));
    if caught.is_err() && !scope.is_execution_terminating() {
        throw(
            scope,
            &OpError::new(ExceptionClass::Error, "internal error in __wasm_module"),
        );
    }
}

fn wasm_module_inner(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    let handle = args.get(0).number_value(scope).unwrap_or(-1.0);
    if !handle.is_finite() || handle < 0.0 {
        throw(scope, &OpError::type_error("invalid wasm module handle"));
        return;
    }
    let Some(state_rc) = op_state(scope) else {
        throw(
            scope,
            &OpError::new(ExceptionClass::Error, "op state unavailable"),
        );
        return;
    };
    let found = state_rc
        .borrow()
        .wasm_modules
        .get(&(handle as u32))
        .cloned();
    match found {
        Some(global) => {
            let local = v8::Local::new(scope, &global);
            rv.set(local.into());
        }
        None => throw(
            scope,
            &OpError::new(ExceptionClass::Error, "unknown wasm module handle"),
        ),
    }
}

/// `__wasm_pending(delta)`: adjusts the in-flight async-WebAssembly count.
/// Contains panics as a JS exception rather than unwinding across V8 (D15).
fn wasm_pending(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    rv: v8::ReturnValue<v8::Value>,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        wasm_pending_inner(&mut *scope, args, rv);
    }));
}

fn wasm_pending_inner(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue<v8::Value>,
) {
    let Some(delta) = args.get(0).number_value(scope) else {
        return;
    };
    let Some(state_rc) = op_state(scope) else {
        return;
    };
    let mut state = state_rc.borrow_mut();
    // Saturating both ways: a forged or unbalanced call from guest code must not
    // wrap the counter — the worst it can do is keep the loop alive, never make
    // it exit early on work that is still in flight.
    if delta > 0.0 {
        state.pending_wasm = state.pending_wasm.saturating_add(1);
    } else if delta < 0.0 {
        state.pending_wasm = state.pending_wasm.saturating_sub(1);
    }
}

fn install_global_fn(
    scope: &mut v8::PinScope,
    global: v8::Local<v8::Object>,
    name: &str,
    callback: impl v8::MapFnTo<v8::FunctionCallback>,
    repeat_data: Option<bool>,
) -> Result<()> {
    let mut builder = v8::Function::builder(callback);
    if let Some(repeat) = repeat_data {
        let data = v8::Boolean::new(scope, repeat);
        builder = builder.data(data.into());
    }
    let func = builder
        .build(scope)
        .ok_or_else(|| Error::Internal(format!("could not build builtin {name}")))?;
    let key = string(scope, name)?;
    global.set(scope, key.into(), func.into());
    Ok(())
}

/// `setTimeout(cb, delay)` / `setInterval(cb, delay)`: stores the callback and
/// reports the new timer to `runtime` for scheduling. Returns the timer id.
/// Contains panics as a JS exception rather than unwinding across V8 (D15).
fn timer_set(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    rv: v8::ReturnValue<v8::Value>,
) {
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        timer_set_inner(&mut *scope, args, rv);
    }));
    if caught.is_err() && !scope.is_execution_terminating() {
        throw(
            scope,
            &OpError::new(ExceptionClass::Error, "internal error in setTimeout"),
        );
    }
}

fn timer_set_inner(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    let callback = args.get(0);
    if !callback.is_function() {
        throw(
            scope,
            &OpError::type_error("timer callback must be a function"),
        );
        return;
    }
    let callback: v8::Local<v8::Function> = callback.try_into().expect("checked is_function");
    let delay = args.get(1).number_value(scope).unwrap_or(0.0);
    let delay_ms = if delay.is_finite() && delay > 0.0 {
        delay as u64
    } else {
        0
    };
    let repeat = args.data().boolean_value(scope);
    // Everything after `delay` is forwarded to the callback on each firing.
    let extra: Vec<v8::Global<v8::Value>> = (2..args.length())
        .map(|i| v8::Global::new(scope, args.get(i)))
        .collect();

    let Some(state_rc) = op_state(scope) else {
        throw(
            scope,
            &OpError::new(ExceptionClass::Error, "op state unavailable"),
        );
        return;
    };
    let callback = v8::Global::new(scope, callback);
    let id = {
        let mut state = state_rc.borrow_mut();
        let id = state.next_timer_id;
        state.next_timer_id += 1;
        state.timers.insert(
            id,
            TimerEntry {
                callback,
                repeat,
                args: extra,
            },
        );
        state.new_timers.push((id, delay_ms, repeat));
        id
    };
    rv.set(v8::Number::new(scope, id as f64).into());
}

/// `clearTimeout(id)` / `clearInterval(id)`: deactivates the timer. A later
/// `fire_timer`/`timer_is_active` for it then reports inactive, so `runtime`
/// stops scheduling it. Best-effort; a panic is contained, never an unwind
/// across V8 (D15).
fn timer_clear(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    rv: v8::ReturnValue<v8::Value>,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        timer_clear_inner(&mut *scope, args, rv);
    }));
}

fn timer_clear_inner(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue<v8::Value>,
) {
    let Some(id) = args.get(0).number_value(scope) else {
        return;
    };
    if !id.is_finite() || id < 0.0 {
        return;
    }
    if let Some(state_rc) = op_state(scope) {
        state_rc.borrow_mut().timers.remove(&(id as TimerId));
    }
}

/// Invokes the JS callback for timer `id`. One-shot timers are removed first.
/// Returns `false` if the timer is no longer active (cleared).
/// Fires timer `id`, returning `(terminated, fired)` — whether the callback's
/// execution was *terminated* (`process.exit()`, a watchdog, the heap guard),
/// and whether there was a timer to fire at all.
///
/// The termination has to be reported rather than left in the isolate: the
/// TryCatch below absorbs it, and the caller re-raises it. Without that the
/// embedder's loop sees a runnable isolate with work that can never finish.
pub(crate) fn fire_timer(
    isolate: &mut v8::OwnedIsolate,
    context: &v8::Global<v8::Context>,
    op_state: &Rc<RefCell<OpState>>,
    id: TimerId,
) -> (bool, bool) {
    if !op_state.borrow().timers.contains_key(&id) {
        return (false, false);
    }

    v8::scope!(let scope, isolate);
    let context = v8::Local::new(scope, context);
    let scope = &mut v8::ContextScope::new(scope, context);
    v8::tc_scope!(let scope, scope);

    let (callback, repeat, args) = {
        let state = op_state.borrow();
        let entry = state.timers.get(&id).expect("checked above");
        let args: Vec<v8::Local<v8::Value>> = entry
            .args
            .iter()
            .map(|a| v8::Local::new(scope, a))
            .collect();
        (v8::Local::new(scope, &entry.callback), entry.repeat, args)
    };
    if !repeat {
        op_state.borrow_mut().timers.remove(&id);
    }

    let recv: v8::Local<v8::Value> = v8::undefined(scope).into();
    // A throw inside the callback is caught by the TryCatch (no host unwind).
    // It is *not* swallowed: there is no caller left to propagate it to, so the
    // platform reports it — an `error` event on the global scope, then the
    // embedder's report if no listener claimed it. Silently dropping it, as this
    // used to, loses the failure entirely.
    let mut terminated = false;
    if callback.call(scope, recv, &args).is_none() {
        // A termination is not a throw and has no exception to report: the
        // callback called `process.exit()`, or a watchdog or the heap guard
        // fired. The TryCatch has absorbed it, which would leave the isolate
        // looking runnable to the embedder's loop — so it is re-raised below,
        // once this scope is gone.
        terminated = scope.has_terminated();
        if !terminated && let Some(error) = scope.exception() {
            report_uncaught(scope, op_state, error);
        }
    }
    (terminated, true)
}

/// Installs the promise-reject callback that records unhandled rejections.
pub(crate) fn install_promise_reject_callback(isolate: &mut v8::OwnedIsolate) {
    isolate.set_promise_reject_callback(promise_reject_callback);
}

extern "C" fn promise_reject_callback(message: v8::PromiseRejectMessage) {
    // Contain any panic: this is invoked by V8, so an unwind here would cross
    // C++ frames (UB). The body only touches op state, but stay safe (D15).
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        promise_reject_callback_inner(message);
    }));
}

fn promise_reject_callback_inner(message: v8::PromiseRejectMessage) {
    // SAFETY: `message` is valid for the duration of this callback; the macro
    // builds a CallbackScope over it, which is exactly its intended use.
    v8::callback_scope!(unsafe scope, &message);
    v8::scope!(let scope, scope);

    let Some(state_rc) = op_state(scope) else {
        return;
    };
    let promise = message.get_promise();
    let key = promise.get_identity_hash().get();

    match message.get_event() {
        v8::PromiseRejectEvent::PromiseRejectWithNoHandler => {
            if let Some(value) = message.get_value() {
                let value = v8::Global::new(scope, value);
                let promise = v8::Global::new(scope, promise);
                state_rc
                    .borrow_mut()
                    .unhandled_rejections
                    .insert(key, (promise, value));
            }
        }
        v8::PromiseRejectEvent::PromiseHandlerAddedAfterReject => {
            let mut state = state_rc.borrow_mut();
            state.unhandled_rejections.remove(&key);
            // A handler arriving *after* the rejection was already reported
            // retracts that report: the spec fires `rejectionhandled`. A handler
            // arriving before the drain simply cancels the pending entry above,
            // and nothing was ever announced.
            if state.reported_rejections.remove(&key) {
                let promise = v8::Global::new(scope, promise);
                state.rejections_handled.push(promise);
            }
        }
        // Resolve-after-resolved / reject-after-resolved: not unhandled.
        _ => {}
    }
}

/// Calls a prelude-installed dispatch hook, if the guest realm has one.
///
/// This is the seam that lets the *engine* — which knows nothing of `Event`,
/// `ErrorEvent` or `PromiseRejectionEvent` — hand a failure to the JS layer that
/// does. The prelude installs these functions and `harden.js` locks the
/// bindings; a realm without a prelude (the engine's own tests) simply has none,
/// and the caller falls back to reporting directly.
///
/// Returns `Some(true)` when the hook ran and the guest **claimed** the failure
/// (a listener called `preventDefault`), `Some(false)` when it ran and nothing
/// claimed it, and `None` when there is no hook.
fn call_dispatch_hook(
    scope: &mut v8::PinScope<'_, '_>,
    name: &str,
    args: &[v8::Local<'_, v8::Value>],
) -> Option<bool> {
    let global = scope.get_current_context().global(scope);
    let key = v8::String::new(scope, name)?;
    let hook = global.get(scope, key.into())?;
    let hook = v8::Local::<v8::Function>::try_from(hook).ok()?;

    // The hook is prelude code, but it dispatches to *guest* listeners, which
    // may throw. A throw here must not escape into V8's callback frames.
    v8::tc_scope!(let scope, scope);
    let recv: v8::Local<v8::Value> = v8::undefined(scope).into();
    let handled = hook.call(scope, recv, args);
    Some(handled.is_some_and(|v| v.is_true()))
}

/// Routes an exception that escaped a host-invoked callback to the guest's
/// `error` listeners, and records it for the embedder if nothing claimed it.
fn report_uncaught(
    scope: &mut v8::PinScope<'_, '_>,
    state_rc: &Rc<RefCell<OpState>>,
    error: v8::Local<'_, v8::Value>,
) {
    let handled = call_dispatch_hook(scope, "__dispatch_error_event", &[error]);
    if handled == Some(true) {
        return;
    }
    let message = crate::convert::format_exception(scope, error);
    state_rc.borrow_mut().uncaught_errors.push(message);
}

/// Fires `unhandledrejection` for each rejection that is still unclaimed, then
/// drains and stringifies the ones no listener took responsibility for.
///
/// A listener calling `preventDefault()` is the spec's way for guest code to say
/// "this rejection is mine, do not report it" — so a prevented rejection is
/// dropped here and never reaches the embedder. Anything left is remembered by
/// identity hash, so attaching a handler later can retract the report with
/// `rejectionhandled`.
pub(crate) fn take_unhandled_rejections(
    isolate: &mut v8::OwnedIsolate,
    context: &v8::Global<v8::Context>,
    op_state: &Rc<RefCell<OpState>>,
) -> Vec<String> {
    let (rejections, handled) = {
        let mut state = op_state.borrow_mut();
        let rejections: Vec<(i32, Rejection)> = state.unhandled_rejections.drain().collect();
        let handled: Vec<v8::Global<v8::Promise>> = state.rejections_handled.drain(..).collect();
        (rejections, handled)
    };
    if rejections.is_empty() && handled.is_empty() {
        return Vec::new();
    }

    v8::scope!(let scope, isolate);
    let context = v8::Local::new(scope, context);
    let scope = &mut v8::ContextScope::new(scope, context);

    for promise in &handled {
        let promise = v8::Local::new(scope, promise);
        call_dispatch_hook(scope, "__dispatch_rejection_handled", &[promise.into()]);
    }

    let mut reported = Vec::new();
    for (key, (promise, value)) in &rejections {
        let value = v8::Local::new(scope, value);
        let promise = v8::Local::new(scope, promise);
        if call_dispatch_hook(
            scope,
            "__dispatch_unhandled_rejection",
            &[value, promise.into()],
        ) == Some(true)
        {
            continue;
        }
        op_state.borrow_mut().reported_rejections.insert(*key);
        reported.push(crate::convert::format_exception(scope, value));
    }
    reported
}

/// Drains exceptions that escaped a host-invoked callback and were not claimed
/// by an `error` listener.
pub(crate) fn take_uncaught_errors(op_state: &Rc<RefCell<OpState>>) -> Vec<String> {
    std::mem::take(&mut op_state.borrow_mut().uncaught_errors)
}

/// Builds a V8 string, mapping the (vanishingly rare) over-length failure to a
/// typed internal error rather than panicking.
fn string<'s>(scope: &mut v8::PinScope<'s, '_>, s: &str) -> Result<v8::Local<'s, v8::String>> {
    v8::String::new(scope, s).ok_or_else(|| Error::Internal("string exceeds V8 maximum".into()))
}
