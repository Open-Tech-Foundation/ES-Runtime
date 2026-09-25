//! Async context propagation: the host half of `runtime:context`.
//!
//! One **mapping** — an opaque JS value the module owns, never interpreted here
//! — is current at any moment. Everything about *what* a mapping contains (the
//! context objects and their values) lives in `runtime_modules/context.js`; the
//! host only ever moves the value around, so the two halves cannot disagree
//! about a representation neither of them parses.
//!
//! # Where "current" lives: V8's continuation data (DECISIONS.md D131)
//!
//! The current scope is a small JS **record**, `[mapping, task, parent, trace,
//! span]`, held in V8's *continuation-preserved embedder data*. V8 captures that
//! value when a promise reaction is created — `.then`, `await`,
//! `queueMicrotask` — and restores it when the reaction runs, entirely inside
//! V8. So propagation across every promise continuation costs what it costs a
//! program with no contexts at all.
//!
//! This replaced a promise hook (D88), which called into the host on every
//! promise: `Init` stamped each new promise with a record, `Before`/`After`
//! swapped it around each reaction job. Loading `runtime:context` made every
//! promise in the process ~27× slower, whether or not its context was read. A
//! record is now created only when a scope *changes* — `run()`, a timer firing,
//! a request, `withTrace`, a span — and read only when something needs it.
//!
//! The propagation points:
//!
//! * **Promise continuations** (`await`, `.then`, `queueMicrotask`) — V8.
//! * **Timers** — [`crate::op`]'s `TimerEntry` carries a [`Capture`] taken in
//!   `setTimeout`/`setInterval` and installed around each firing, as a new task.
//! * **Unhandled rejections** — captured where the rejection *originated*, in
//!   the promise-reject callback, and installed around the guest's
//!   `unhandledrejection` dispatch.
//!
//! `EventTarget` dispatch is deliberately **not** in that list, and needs no
//! code to stay out of it: dispatch is synchronous JS, so a listener already
//! runs in the mapping of whoever called `dispatchEvent`. See DECISIONS.md D88.
//!
//! # Tasks
//!
//! A task is a unit of work the host starts — the root, a timer firing, an
//! inbound request — and a promise continuation belongs to the task that
//! scheduled it. A per-promise id would need a per-promise callback, which is
//! the cost D131 removed.
//!
//! The **trace id** rides in the record rather than in the mapping, because
//! `runtime:diagnostics` attributes records to it on the host's recording path.
//! The module still owns the value: it mints the id and pushes it down with
//! `__ctx_swap_trace`, so there is one source of truth.

use std::cell::RefCell;
use std::rc::Rc;

/// Identity of one async task — `currentTask().id`.
pub(crate) type TaskId = u64;

/// The task every agent starts in: whatever runs outside any other task.
/// Its `parentId` is `None`, which the module reports as `null`.
const ROOT_TASK: TaskId = 0;

/// The task identity JS sees for "no parent" (`parentId: null`), and the span
/// id for "no enclosing span". Negative, so neither collides with a real id.
const NONE: f64 = -1.0;

/// The record's slots, in order.
const FRAME: u32 = 0;
const TASK: u32 = 1;
const PARENT: u32 = 2;
const TRACE: u32 = 3;
const SPAN: u32 = 4;

/// One scope, decoded: the mapping plus the task identity and the
/// observability ids that go with it — what a timer stores at schedule time and
/// puts back when it fires.
///
/// Cloneable because a repeating timer installs the same capture on every
/// firing, and a `Global` clone is a refcount bump rather than a copy.
#[derive(Clone, Default)]
pub(crate) struct Capture {
    /// The mapping, opaque to the host. `None` is the **root** mapping — the
    /// module reads it as "every context at its default".
    frame: Option<v8::Global<v8::Value>>,
    task: TaskId,
    parent: Option<TaskId>,
    /// The W3C trace id this scope runs under, or `None` for the agent's root
    /// trace (which may not be minted yet).
    ///
    /// An `Rc<str>` so that copying a capture is a refcount bump.
    trace: Option<Rc<str>>,
    /// The diagnostics span this scope is running *inside*, if any — what turns
    /// a flat list of records into a tree. `None` at the root.
    span: Option<u64>,
}

/// What the host keeps about contexts. The scope itself is not here: it is
/// V8's continuation data, read with [`current`].
pub(crate) struct ContextState {
    /// Whether `runtime:context` has been loaded. Nothing is installed any more
    /// (D131), but `runtime:http` still asks, so that a server whose program
    /// cannot read a trace id does not draw entropy to mint one.
    enabled: bool,
    /// Source of task ids. Starts past [`ROOT_TASK`] so `0` always means "the
    /// task the agent started in".
    next_task: TaskId,
    /// The trace the **root** runs in, minted once per agent. A record whose
    /// trace slot is empty reads as this one.
    root_trace: Option<Rc<str>>,
}

impl ContextState {
    pub(crate) fn new() -> Self {
        ContextState {
            enabled: false,
            next_task: ROOT_TASK + 1,
            root_trace: None,
        }
    }

    /// Whether `runtime:context` has been loaded. Read from JS as
    /// `__ctx_enabled()`.
    pub(crate) fn enabled(&self) -> bool {
        self.enabled
    }

    /// Installs the trace every task under the root runs in. Ignored once a
    /// trace exists, so it cannot overwrite what the program chose.
    pub(crate) fn set_root_trace(&mut self, trace: Rc<str>) {
        if self.root_trace.is_none() {
            self.root_trace = Some(trace);
        }
    }

    fn next_task(&mut self) -> TaskId {
        let id = self.next_task;
        self.next_task += 1;
        id
    }
}

/// Clones the state out of the isolate slot. Mirrors [`crate::op`]'s
/// `op_state`: the clone decouples access from the isolate borrow the scope
/// already holds.
pub(crate) fn state(scope: &v8::PinScope<'_, '_>) -> Option<Rc<RefCell<ContextState>>> {
    scope.get_slot::<Rc<RefCell<ContextState>>>().cloned()
}

/// The record current right now, or `None` at the root.
fn record<'s>(scope: &v8::PinScope<'s, '_>) -> Option<v8::Local<'s, v8::Array>> {
    let value = scope.get_continuation_preserved_embedder_data();
    v8::Local::<v8::Array>::try_from(value).ok()
}

fn slot<'s>(
    scope: &v8::PinScope<'s, '_>,
    record: v8::Local<'s, v8::Array>,
    index: u32,
) -> v8::Local<'s, v8::Value> {
    record
        .get_index(scope, index)
        .unwrap_or_else(|| v8::undefined(scope).into())
}

fn number(scope: &v8::PinScope<'_, '_>, value: v8::Local<'_, v8::Value>) -> Option<u64> {
    value
        .number_value(scope)
        .filter(|n| *n >= 0.0)
        .map(|n| n as u64)
}

fn trace_of(scope: &v8::PinScope<'_, '_>, value: v8::Local<'_, v8::Value>) -> Option<Rc<str>> {
    (!value.is_undefined() && !value.is_null()).then(|| Rc::from(value.to_rust_string_lossy(scope)))
}

/// Makes a record from its five slots and makes it current.
fn install_slots(scope: &v8::PinScope<'_, '_>, slots: &[v8::Local<'_, v8::Value>; 5]) {
    let record = v8::Array::new_with_elements(scope, slots);
    scope.set_continuation_preserved_embedder_data(record.into());
}

/// The current record's slots, or the root's when there is none.
fn slots<'s>(scope: &v8::PinScope<'s, '_>) -> [v8::Local<'s, v8::Value>; 5] {
    match record(scope) {
        Some(record) => [
            slot(scope, record, FRAME),
            slot(scope, record, TASK),
            slot(scope, record, PARENT),
            slot(scope, record, TRACE),
            slot(scope, record, SPAN),
        ],
        None => [
            v8::undefined(scope).into(),
            v8::Number::new(scope, ROOT_TASK as f64).into(),
            v8::Number::new(scope, NONE).into(),
            v8::undefined(scope).into(),
            v8::Number::new(scope, NONE).into(),
        ],
    }
}

/// The scope current right now, decoded — for a caller that will reinstall it
/// later: `setTimeout` storing one on the timer, the promise-reject callback
/// storing one on the rejection.
pub(crate) fn capture(scope: &v8::PinScope<'_, '_>) -> Capture {
    let Some(record) = record(scope) else {
        return Capture::default();
    };
    let frame = slot(scope, record, FRAME);
    let task = slot(scope, record, TASK);
    let parent = slot(scope, record, PARENT);
    let trace = slot(scope, record, TRACE);
    let span = slot(scope, record, SPAN);
    Capture {
        frame: (!frame.is_undefined()).then(|| v8::Global::new(scope, frame)),
        task: number(scope, task).unwrap_or(ROOT_TASK),
        parent: number(scope, parent),
        trace: trace_of(scope, trace),
        span: number(scope, span),
    }
}

/// Makes `next` current.
fn install(scope: &v8::PinScope<'_, '_>, next: &Capture) {
    let frame: v8::Local<v8::Value> = match &next.frame {
        Some(frame) => v8::Local::new(scope, frame),
        None => v8::undefined(scope).into(),
    };
    let trace: v8::Local<v8::Value> = match next
        .trace
        .as_deref()
        .and_then(|t| v8::String::new(scope, t))
    {
        Some(trace) => trace.into(),
        None => v8::undefined(scope).into(),
    };
    install_slots(
        scope,
        &[
            frame,
            v8::Number::new(scope, next.task as f64).into(),
            v8::Number::new(scope, next.parent.map_or(NONE, |id| id as f64)).into(),
            trace,
            v8::Number::new(scope, next.span.map_or(NONE, |id| id as f64)).into(),
        ],
    );
}

/// Makes `next` current, handing back what was — to be passed to [`leave`].
///
/// Paired with [`leave`] rather than taking a closure because both call sites
/// need the scope mutably in between, and the work in between cannot unwind:
/// each runs the guest behind a `TryCatch`, so a throw becomes a caught
/// exception rather than a Rust panic that would skip the restore.
pub(crate) fn enter(scope: &v8::PinScope<'_, '_>, next: Capture) -> Option<Capture> {
    let previous = capture(scope);
    install(scope, &next);
    Some(previous)
}

/// [`enter`], but as a **new task** in `parent`'s mapping.
///
/// For a firing timer: the mapping is the one the timer was armed in, but the
/// callback is not the code that armed it, so it gets an id of its own with the
/// scheduler as its `parentId`. A repeating timer is a new task on every firing.
/// The trace and the enclosing span are the unit of work's, and carry over.
pub(crate) fn enter_child(scope: &v8::PinScope<'_, '_>, parent: Capture) -> Option<Capture> {
    let id = state(scope)?.borrow_mut().next_task();
    enter(
        scope,
        Capture {
            task: id,
            parent: Some(parent.task),
            ..parent
        },
    )
}

/// Puts back what [`enter`] handed over.
pub(crate) fn leave(scope: &v8::PinScope<'_, '_>, previous: Option<Capture>) {
    if let Some(previous) = previous {
        install(scope, &previous);
    }
}

/// The span the current scope is running inside — a new span's parent.
pub(crate) fn current_span(scope: &v8::PinScope<'_, '_>) -> Option<u64> {
    let record = record(scope)?;
    let span = slot(scope, record, SPAN);
    number(scope, span)
}

/// The trace the current scope runs in: its own, or the agent's root trace.
pub(crate) fn current_trace(scope: &v8::PinScope<'_, '_>) -> Option<Rc<str>> {
    let own = record(scope).and_then(|record| {
        let trace = slot(scope, record, TRACE);
        trace_of(scope, trace)
    });
    own.or_else(|| state(scope).and_then(|st| st.borrow().root_trace.clone()))
}

/// The executing task's identity, as `(id, parentId, traceId)`.
pub(crate) fn identity(scope: &v8::PinScope<'_, '_>) -> (TaskId, Option<TaskId>, Option<Rc<str>>) {
    let (task, parent) = match record(scope) {
        Some(record) => {
            let task = slot(scope, record, TASK);
            let parent = slot(scope, record, PARENT);
            (
                number(scope, task).unwrap_or(ROOT_TASK),
                number(scope, parent),
            )
        }
        None => (ROOT_TASK, None),
    };
    (task, parent, current_trace(scope))
}

/// Makes `next` the enclosing span for what follows, returning what was.
pub(crate) fn swap_span(scope: &v8::PinScope<'_, '_>, next: Option<u64>) -> Option<u64> {
    let mut slots = slots(scope);
    let previous = number(scope, slots[SPAN as usize]);
    slots[SPAN as usize] = v8::Number::new(scope, next.map_or(NONE, |id| id as f64)).into();
    install_slots(scope, &slots);
    previous
}

/// Puts back what [`swap_span`] handed over.
pub(crate) fn restore_span(scope: &v8::PinScope<'_, '_>, previous: Option<u64>) {
    swap_span(scope, previous);
}

/// Marks `runtime:context` as loaded. Nothing to install: propagation is V8's.
pub(crate) fn enable(state: &Rc<RefCell<ContextState>>) {
    state.borrow_mut().enabled = true;
}

/// Returns to the root scope.
///
/// Called once per tick, when no JS is on the stack and the answer is therefore
/// known rather than guessed. It matters because a `try/finally` restore does
/// **not** run when execution is *terminated* mid-callback — a
/// `process.exit()`, the watchdog, the heap guard — and without this a scope
/// from a killed callback would still be current on the next tick. It is also
/// what keeps a `FinalizationRegistry` cleanup callback on the empty mapping.
pub(crate) fn reset(scope: &v8::PinScope<'_, '_>) {
    scope.set_continuation_preserved_embedder_data(v8::undefined(scope).into());
}

/// Installs the four builtins `runtime:context` is written against.
///
/// Builtins rather than ops, for the same reason the timer setters are: the
/// mapping is a live JS value, and an op would flatten it through
/// [`Value`](crate::Value) on the way past — which would both copy it and lose
/// the object identity the module keys contexts on.
pub(crate) fn install_builtins(
    scope: &mut v8::PinScope,
    context: v8::Local<v8::Context>,
) -> crate::error::Result<()> {
    let global = context.global(scope);
    crate::op::install_global_fn(scope, global, "__ctx_enabled", ctx_enabled, None)?;
    crate::op::install_global_fn(scope, global, "__ctx_current", ctx_current, None)?;
    crate::op::install_global_fn(scope, global, "__ctx_install", ctx_install, None)?;
    crate::op::install_global_fn(scope, global, "__ctx_frame", ctx_frame, None)?;
    crate::op::install_global_fn(scope, global, "__ctx_swap", ctx_swap, None)?;
    crate::op::install_global_fn(scope, global, "__ctx_task", ctx_task, None)?;
    crate::op::install_global_fn(scope, global, "__ctx_parent", ctx_parent, None)?;
    crate::op::install_global_fn(scope, global, "__ctx_trace", ctx_trace, None)?;
    crate::op::install_global_fn(scope, global, "__ctx_swap_trace", ctx_swap_trace, None)?;
    crate::op::install_global_fn(scope, global, "__ctx_root_trace", ctx_root_trace, None)?;
    crate::op::install_global_fn(scope, global, "__ctx_span", ctx_span, None)?;
    crate::op::install_global_fn(scope, global, "__ctx_swap_span", ctx_swap_span, None)
}

/// The native callbacks this module contributes to the snapshot's external
/// reference table (see [`crate::op::external_references`]).
pub(crate) fn external_references() -> Vec<v8::ExternalReference> {
    use v8::MapFnTo;
    vec![
        v8::ExternalReference {
            function: ctx_enabled.map_fn_to(),
        },
        v8::ExternalReference {
            function: ctx_current.map_fn_to(),
        },
        v8::ExternalReference {
            function: ctx_install.map_fn_to(),
        },
        v8::ExternalReference {
            function: ctx_frame.map_fn_to(),
        },
        v8::ExternalReference {
            function: ctx_swap.map_fn_to(),
        },
        v8::ExternalReference {
            function: ctx_task.map_fn_to(),
        },
        v8::ExternalReference {
            function: ctx_parent.map_fn_to(),
        },
        v8::ExternalReference {
            function: ctx_trace.map_fn_to(),
        },
        v8::ExternalReference {
            function: ctx_swap_trace.map_fn_to(),
        },
        v8::ExternalReference {
            function: ctx_root_trace.map_fn_to(),
        },
        v8::ExternalReference {
            function: ctx_span.map_fn_to(),
        },
        v8::ExternalReference {
            function: ctx_swap_span.map_fn_to(),
        },
    ]
}

/// Wraps a builtin body so a panic is contained rather than unwound across V8
/// (DECISIONS.md D15). These five only read and write host state, so there is
/// nothing to report and nothing to throw — a contained panic simply leaves the
/// return value unset, which reads as `undefined`.
macro_rules! contained {
    ($name:ident, $inner:ident) => {
        fn $name(
            scope: &mut v8::PinScope,
            args: v8::FunctionCallbackArguments,
            rv: v8::ReturnValue<v8::Value>,
        ) {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                $inner(&mut *scope, args, rv);
            }));
        }
    };
}

contained!(ctx_enabled, ctx_enabled_inner);
contained!(ctx_current, ctx_current_inner);
contained!(ctx_install, ctx_install_inner);
contained!(ctx_frame, ctx_frame_inner);
contained!(ctx_swap, ctx_swap_inner);
contained!(ctx_task, ctx_task_inner);
contained!(ctx_parent, ctx_parent_inner);
contained!(ctx_trace, ctx_trace_inner);
contained!(ctx_swap_trace, ctx_swap_trace_inner);
contained!(ctx_root_trace, ctx_root_trace_inner);
contained!(ctx_span, ctx_span_inner);
contained!(ctx_swap_span, ctx_swap_span_inner);

/// `__ctx_enabled()` → whether `runtime:context` has been loaded.
fn ctx_enabled_inner(
    scope: &mut v8::PinScope,
    _args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    let enabled = state(scope).is_some_and(|st| st.borrow().enabled());
    rv.set(v8::Boolean::new(scope, enabled).into());
}

/// `__ctx_current()` → the current record, or `undefined` at the root.
///
/// With [`ctx_install_inner`], the whole of `runtime:context`'s hot path: the
/// module reads and copies the record's slots in JS, where the JIT makes that
/// cheap, and crosses into the host only to fetch and to install one. Decoding
/// slot by slot through the V8 API is several calls per slot, which is what
/// made `run()` cost more than the propagation it exists for.
fn ctx_current_inner(
    scope: &mut v8::PinScope,
    _args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    rv.set(scope.get_continuation_preserved_embedder_data());
}

/// `__ctx_install(record)` → makes `record` current. `undefined` is the root.
/// The module builds records in the shape [`capture`] decodes.
fn ctx_install_inner(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue<v8::Value>,
) {
    scope.set_continuation_preserved_embedder_data(args.get(0));
}

/// `__ctx_frame()` → the current mapping, or `undefined` for the root.
fn ctx_frame_inner(
    scope: &mut v8::PinScope,
    _args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    if let Some(record) = record(scope) {
        rv.set(slot(scope, record, FRAME));
    }
}

/// `__ctx_swap(next)` → makes `next` the current mapping and returns what was.
/// One call rather than a get/set pair, so the module's `run()` cannot leave the
/// two out of step on a throw. The hot path of `run()`: it copies four slots
/// and allocates one small array, and never decodes them.
fn ctx_swap_inner(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    let mut slots = slots(scope);
    let previous = std::mem::replace(&mut slots[FRAME as usize], args.get(0));
    install_slots(scope, &slots);
    rv.set(previous);
}

/// `__ctx_task()` → the executing task's id.
fn ctx_task_inner(
    scope: &mut v8::PinScope,
    _args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    let (task, _, _) = identity(scope);
    rv.set(v8::Number::new(scope, task as f64).into());
}

/// `__ctx_parent()` → the id of the task that started this one, or `-1`.
fn ctx_parent_inner(
    scope: &mut v8::PinScope,
    _args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    let (_, parent, _) = identity(scope);
    rv.set(v8::Number::new(scope, parent.map_or(NONE, |id| id as f64)).into());
}

/// `__ctx_trace()` → the trace id current in this scope, or `undefined` if none
/// has been minted yet — the module's signal to mint one.
fn ctx_trace_inner(
    scope: &mut v8::PinScope,
    _args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    if let Some(trace) = current_trace(scope)
        .as_deref()
        .and_then(|t| v8::String::new(scope, t))
    {
        rv.set(trace.into());
    }
}

/// `__ctx_swap_trace(next)` → makes `next` the current trace and returns what
/// was, mirroring `__ctx_swap` for the mapping. `undefined` in or out means the
/// agent's root trace.
fn ctx_swap_trace_inner(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    let mut slots = slots(scope);
    let next = args.get(0);
    let next = if next.is_null() {
        v8::undefined(scope).into()
    } else {
        next
    };
    let previous = std::mem::replace(&mut slots[TRACE as usize], next);
    install_slots(scope, &slots);
    if !previous.is_undefined() && !previous.is_null() {
        rv.set(previous);
    }
}

/// `__ctx_swap_span(id)` → makes `id` the enclosing span (`-1` for none) and
/// returns the previous one as a number, `-1` for none.
///
/// What `runtime:diagnostics` opens a user span with, and what `runtime:http`
/// opens a request span with.
fn ctx_swap_span_inner(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    let next = args.get(0).number_value(scope).unwrap_or(NONE);
    let previous = swap_span(scope, (next >= 0.0).then_some(next as u64));
    rv.set(v8::Number::new(scope, previous.map_or(NONE, |id| id as f64)).into());
}

/// `__ctx_span()` → the enclosing span id, or `-1` for none.
///
/// A plain read, because a span handle records its parent **without** becoming
/// active — the active one changes only inside a scoped run.
fn ctx_span_inner(
    scope: &mut v8::PinScope,
    _args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    let current = current_span(scope);
    rv.set(v8::Number::new(scope, current.map_or(NONE, |id| id as f64)).into());
}

/// `__ctx_root_trace(id)` → installs the agent's root trace, if it has none.
///
/// `runtime:context` mints the id (it has the entropy op; the engine has no
/// provider) and pushes it down here rather than through `__ctx_swap_trace`,
/// because the root trace belongs to the agent and survives the per-turn reset.
fn ctx_root_trace_inner(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue<v8::Value>,
) {
    let Some(state_rc) = state(scope) else {
        return;
    };
    let trace = args.get(0);
    if trace.is_undefined() || trace.is_null() {
        return;
    }
    let trace: Rc<str> = Rc::from(trace.to_rust_string_lossy(scope));
    state_rc.borrow_mut().set_root_trace(trace);
}
