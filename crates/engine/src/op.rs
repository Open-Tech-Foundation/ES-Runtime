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
    Capability, CapabilitySet, Error as CommonError, ExceptionClass, IntoException,
};

use crate::convert::{build_exception, js_to_string, marshal, throw, value_to_js};
use crate::error::{Error, Result};
use crate::value::Value;

/// Identifier for a scheduled timer, returned by `setTimeout`/`setInterval`.
pub type TimerId = u64;

/// The result a host op produces: a [`Value`] or an [`OpError`] to throw.
pub type OpResult = std::result::Result<Value, OpError>;

/// A pending async op: a future resolving to an [`OpResult`], polled on tick.
pub type AsyncOp = Pin<Box<dyn Future<Output = OpResult>>>;

/// An error a host op raises, carrying the JS exception class it surfaces as.
#[derive(Debug, Clone)]
pub struct OpError {
    class: ExceptionClass,
    message: String,
}

impl OpError {
    /// Constructs an op error with an explicit JS exception class.
    pub fn new(class: ExceptionClass, message: impl Into<String>) -> Self {
        OpError {
            class,
            message: message.into(),
        }
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

/// A registration request for one op: its JS name, the capability it requires
/// (if any), and its handler.
pub struct OpDecl {
    /// The name the op is exposed as: `globalThis.__ops.<name>`.
    pub name: String,
    /// The capability checked before dispatch (ARCHITECTURE.md §4); `None` for
    /// pure, non-side-effecting ops.
    pub required_capability: Option<Capability>,
    /// The handler to invoke.
    pub handler: OpHandler,
}

impl OpDecl {
    /// Declares a synchronous op.
    pub fn sync(
        name: impl Into<String>,
        handler: impl FnMut(Vec<Value>) -> OpResult + 'static,
    ) -> Self {
        OpDecl {
            name: name.into(),
            required_capability: None,
            handler: OpHandler::Sync(Box::new(handler)),
        }
    }

    /// Declares an asynchronous op.
    pub fn r#async(
        name: impl Into<String>,
        handler: impl FnMut(Vec<Value>) -> AsyncOp + 'static,
    ) -> Self {
        OpDecl {
            name: name.into(),
            required_capability: None,
            handler: OpHandler::Async(Box::new(handler)),
        }
    }

    /// Sets the capability required before this op may dispatch (builder style).
    #[must_use]
    pub fn requires(mut self, capability: Capability) -> Self {
        self.required_capability = Some(capability);
        self
    }
}

struct OpEntry {
    required_capability: Option<Capability>,
    handler: OpHandler,
}

struct PendingAsync {
    future: AsyncOp,
    resolver: v8::Global<v8::PromiseResolver>,
}

struct TimerEntry {
    callback: v8::Global<v8::Function>,
    repeat: bool,
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
    /// "handler added" event can revoke the entry.
    unhandled_rejections: HashMap<i32, v8::Global<v8::Value>>,
    /// Upper bound on concurrently pending async ops (DECISIONS.md D7 / SPEC §4).
    /// Dispatching a new async op past this throws, so adversarial JS can't pile
    /// up unbounded host work. `usize::MAX` until set from the engine's limits.
    max_pending_ops: usize,
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
            max_pending_ops: usize::MAX,
        }
    }

    /// Sets the bound on concurrently pending async ops (from engine limits).
    pub(crate) fn set_max_pending_ops(&mut self, max: usize) {
        self.max_pending_ops = max;
    }

    /// Adds an op to the table and returns its id (its index).
    pub(crate) fn add_op(
        &mut self,
        required_capability: Option<Capability>,
        handler: OpHandler,
    ) -> i32 {
        let id = self.ops.len() as i32;
        self.ops.push(OpEntry {
            required_capability,
            handler,
        });
        id
    }

    /// Whether any async op is still awaiting completion.
    pub(crate) fn has_pending_async(&self) -> bool {
        !self.pending_async.is_empty()
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
    if let Some(cap) = state.ops[idx].required_capability
        && !state.capabilities.contains(cap)
    {
        drop(state);
        throw(scope, &CommonError::CapabilityDenied(cap));
        return;
    }

    // Compute the outcome under the field borrow, then release it before using
    // the scope to marshal results / build the promise.
    enum Outcome {
        Ok(Value),
        Err(OpError),
        Async(AsyncOp),
    }
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
            let js = value_to_js(scope, &value);
            rv.set(js);
        }
        Outcome::Err(err) => {
            drop(state);
            throw(scope, &err);
        }
        Outcome::Async(future) => {
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
            state.pending_async.push(PendingAsync { future, resolver });
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
    std::borrow::Cow::Owned(vec![
        v8::ExternalReference {
            function: op_dispatch.map_fn_to(),
        },
        v8::ExternalReference {
            function: timer_set.map_fn_to(),
        },
        v8::ExternalReference {
            function: timer_clear.map_fn_to(),
        },
    ])
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
    // No reactor: a no-op waker, and readiness is observed only here.
    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);

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
                let js = value_to_js(scope, &value);
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
        state.timers.insert(id, TimerEntry { callback, repeat });
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
pub(crate) fn fire_timer(
    isolate: &mut v8::OwnedIsolate,
    context: &v8::Global<v8::Context>,
    op_state: &Rc<RefCell<OpState>>,
    id: TimerId,
) -> bool {
    if !op_state.borrow().timers.contains_key(&id) {
        return false;
    }

    v8::scope!(let scope, isolate);
    let context = v8::Local::new(scope, context);
    let scope = &mut v8::ContextScope::new(scope, context);
    v8::tc_scope!(let scope, scope);

    let (callback, repeat) = {
        let state = op_state.borrow();
        let entry = state.timers.get(&id).expect("checked above");
        (v8::Local::new(scope, &entry.callback), entry.repeat)
    };
    if !repeat {
        op_state.borrow_mut().timers.remove(&id);
    }

    let recv: v8::Local<v8::Value> = v8::undefined(scope).into();
    // A throw inside the callback is caught by the TryCatch (no host unwind);
    // surfacing it as an unhandled error is a later-phase concern.
    let _ = callback.call(scope, recv, &[]);
    true
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
                state_rc
                    .borrow_mut()
                    .unhandled_rejections
                    .insert(key, value);
            }
        }
        v8::PromiseRejectEvent::PromiseHandlerAddedAfterReject => {
            state_rc.borrow_mut().unhandled_rejections.remove(&key);
        }
        // Resolve-after-resolved / reject-after-resolved: not unhandled.
        _ => {}
    }
}

/// Drains and stringifies promise rejections that remained unhandled.
pub(crate) fn take_unhandled_rejections(
    isolate: &mut v8::OwnedIsolate,
    context: &v8::Global<v8::Context>,
    op_state: &Rc<RefCell<OpState>>,
) -> Vec<String> {
    let rejections: Vec<v8::Global<v8::Value>> = {
        let mut state = op_state.borrow_mut();
        state
            .unhandled_rejections
            .drain()
            .map(|(_, value)| value)
            .collect()
    };
    if rejections.is_empty() {
        return Vec::new();
    }

    v8::scope!(let scope, isolate);
    let context = v8::Local::new(scope, context);
    let scope = &mut v8::ContextScope::new(scope, context);
    rejections
        .iter()
        .map(|value| {
            let value = v8::Local::new(scope, value);
            js_to_string(scope, value)
        })
        .collect()
}

/// Builds a V8 string, mapping the (vanishingly rare) over-length failure to a
/// typed internal error rather than panicking.
fn string<'s>(scope: &mut v8::PinScope<'s, '_>, s: &str) -> Result<v8::Local<'s, v8::String>> {
    v8::String::new(scope, s).ok_or_else(|| Error::Internal("string exceeds V8 maximum".into()))
}
