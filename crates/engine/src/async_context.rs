//! Async context propagation: the host half of `runtime:context`.
//!
//! One **mapping** — an opaque JS value the module owns, never interpreted here
//! — is current at any moment. This module's whole job is to make the right
//! mapping current when a continuation runs, and to put the previous one back
//! afterwards. Everything about *what* a mapping contains (the context objects,
//! their values, the trace id) lives in `runtime_modules/context.js`; the host
//! only ever moves the value around, so the two halves cannot disagree about a
//! representation neither of them parses.
//!
//! The propagation points:
//!
//! * **Promise continuations** (`await`, `.then`, `queueMicrotask`) — V8's
//!   promise hook. `Init` stamps the promise with the mapping current when it
//!   was created; `Before`/`After` bracket its reaction job with that mapping
//!   installed. Capturing at `Init` is what makes it *schedule* time rather than
//!   settle time: `p.then(f)` creates its derived promise where `.then` is
//!   written, and `await p` creates its throwaway promise at the `await`.
//! * **Timers** — [`crate::op`]'s `TimerEntry` carries a [`Capture`] taken in
//!   `setTimeout`/`setInterval` and installed around each firing.
//! * **Unhandled rejections** — captured where the rejection *originated*, in
//!   the promise-reject callback, and installed around the guest's
//!   `unhandledrejection` dispatch.
//!
//! `EventTarget` dispatch is deliberately **not** in that list, and needs no
//! code to stay out of it: dispatch is synchronous JS, so a listener already
//! runs in the mapping of whoever called `dispatchEvent`. See DECISIONS.md D88.
//!
//! # Why this is off until asked for
//!
//! A promise hook is isolate-wide and fires for *every* promise, so installing
//! one unconditionally would tax every program in the runtime — including the
//! overwhelming majority that never reads a context. It is therefore installed
//! by [`crate::Engine::enable_async_context`], which `runtime` calls the first
//! time it serves the `runtime:context` module source. Before that the mapping
//! is empty everywhere, which is exactly what a program with no contexts would
//! observe anyway, so laziness costs no observable behaviour.

use std::cell::RefCell;
use std::rc::Rc;

/// Identity of one async task — `currentTask().id`, Node's `executionAsyncId`.
pub(crate) type TaskId = u64;

/// The task every agent starts in: whatever runs outside any continuation.
/// Its `parentId` is `None`, which the module reports as `null`.
const ROOT_TASK: TaskId = 0;

/// The task identity JS sees for "no parent" (`parentId: null`). A negative
/// number, so it cannot collide with a real id.
const NO_PARENT: f64 = -1.0;

/// Everything that must be swapped together to move into another async scope:
/// the mapping plus the task identity that goes with it.
///
/// Cloneable because a repeating timer installs the same capture on every
/// firing, and a `Global` clone is a refcount bump rather than a copy.
#[derive(Clone, Default)]
pub(crate) struct Capture {
    /// The mapping, opaque to the host. `None` is the **root** mapping — the
    /// module reads it as "every context at its default", and it is what a
    /// continuation with no stamp gets, so a promise created before the hook was
    /// installed can never inherit a mapping it was not part of.
    frame: Option<v8::Global<v8::Value>>,
    task: TaskId,
    parent: Option<TaskId>,
}

/// The current mapping, the task counter, and the reaction-job stack.
///
/// Lives in an isolate slot (like [`crate::op::OpState`]) so the promise hook —
/// a bare `extern "C"` function with no closure to carry state — can reach it.
pub(crate) struct ContextState {
    /// Whether the promise hook has been installed. Set once; never cleared,
    /// because there is no safe moment to stop stamping promises that
    /// continuations already in flight are going to read back.
    enabled: bool,
    /// The one guest context of this isolate, entered by the hook so that
    /// reading and writing the promise's private slot has a context to work in.
    /// V8 does not guarantee one is entered when the hook fires.
    context: Option<v8::Global<v8::Context>>,
    /// The private slot each promise carries its [`Capture`] in, as
    /// `[frame, taskId, parentId]`.
    ///
    /// A private symbol rather than a side table keyed by the promise: a side
    /// table would have to be told when a promise dies, and there is no such
    /// signal. Hanging the record off the promise makes the two collectable
    /// together, which is the same reason a `WeakMap` would work and a `Map`
    /// would leak.
    key: Option<v8::Global<v8::Private>>,
    /// The mapping and task in force right now.
    current: Capture,
    /// Saved captures for reaction jobs currently on the stack. V8 brackets
    /// every `PromiseReactionJob` with `Before`/`After`, including one whose
    /// handler throws, so this is balanced in normal operation; [`reset`] exists
    /// for the abnormal one.
    ///
    /// [`reset`]: ContextState::reset
    stack: Vec<Capture>,
    /// Source of task ids. Starts past [`ROOT_TASK`] so `0` always means "the
    /// task the agent started in".
    next_task: TaskId,
}

impl ContextState {
    pub(crate) fn new() -> Self {
        ContextState {
            enabled: false,
            context: None,
            key: None,
            current: Capture::default(),
            stack: Vec::new(),
            next_task: ROOT_TASK + 1,
        }
    }

    /// Whether `runtime:context` has been loaded and the hook installed. Read
    /// from JS as `__ctx_enabled()`: `runtime:http` uses it to skip minting a
    /// trace id for a request nobody can observe.
    pub(crate) fn enabled(&self) -> bool {
        self.enabled
    }

    /// The capture to reinstall later — what a timer stores at schedule time.
    pub(crate) fn capture(&self) -> Capture {
        self.current.clone()
    }

    /// Makes `next` current, handing back what was current so the caller can put
    /// it back. The caller is responsible for the restore; every caller here
    /// does it on both the normal and the throwing path.
    pub(crate) fn enter(&mut self, next: Capture) -> Capture {
        std::mem::replace(&mut self.current, next)
    }

    /// Enters `parent`'s mapping as a **new task** parented to it.
    ///
    /// For a firing timer: the mapping is the one the timer was armed in, but
    /// the callback is not the code that armed it, so it gets an id of its own
    /// with the scheduler as its `parentId` — which is what `executionAsyncId`
    /// and `triggerAsyncId` report for a timer in Node. A repeating timer is a
    /// new task on every firing, for the same reason.
    pub(crate) fn enter_child(&mut self, parent: Capture) -> Capture {
        let id = self.next_task;
        self.next_task += 1;
        self.enter(Capture {
            frame: parent.frame,
            task: id,
            parent: Some(parent.task),
        })
    }

    /// Returns to the root mapping and drops any half-finished reaction-job
    /// stack.
    ///
    /// Called once per tick, when no JS is on the stack and the answer is
    /// therefore known rather than guessed. It matters because a `try/finally`
    /// restore does **not** run when execution is *terminated* mid-callback — a
    /// `process.exit()`, the watchdog, the heap guard — and without this a
    /// mapping from a killed callback would still be current on the next tick.
    /// It is also what keeps a `FinalizationRegistry` cleanup callback on the
    /// empty mapping: cleanup runs as its own microtask, never nested inside a
    /// reaction job, so "whatever the tick started with" is the root.
    pub(crate) fn reset(&mut self) {
        self.current = Capture::default();
        self.stack.clear();
    }
}

/// Clones the state out of the isolate slot. Mirrors [`crate::op`]'s
/// `op_state`: the clone decouples access from the isolate borrow the scope
/// already holds.
pub(crate) fn state(scope: &v8::PinScope<'_, '_>) -> Option<Rc<RefCell<ContextState>>> {
    scope.get_slot::<Rc<RefCell<ContextState>>>().cloned()
}

/// The mapping current right now, for a caller that will reinstall it later —
/// `setTimeout` storing one on the timer, the promise-reject callback storing
/// one on the rejection.
pub(crate) fn capture(scope: &v8::PinScope<'_, '_>) -> Capture {
    state(scope)
        .map(|st| st.borrow().capture())
        .unwrap_or_default()
}

/// Makes `next` current, handing back what was — to be passed to [`leave`].
///
/// Paired with [`leave`] rather than taking a closure because both call sites
/// need the scope mutably in between, and the work in between cannot unwind:
/// each runs the guest behind a `TryCatch`, so a throw becomes a caught
/// exception rather than a Rust panic that would skip the restore.
pub(crate) fn enter(scope: &v8::PinScope<'_, '_>, next: Capture) -> Option<Capture> {
    state(scope).map(|st| st.borrow_mut().enter(next))
}

/// [`enter`], but as a new task parented to `parent` — see
/// [`ContextState::enter_child`].
pub(crate) fn enter_child(scope: &v8::PinScope<'_, '_>, parent: Capture) -> Option<Capture> {
    state(scope).map(|st| st.borrow_mut().enter_child(parent))
}

/// Puts back what [`enter`] handed over.
pub(crate) fn leave(scope: &v8::PinScope<'_, '_>, previous: Option<Capture>) {
    if let (Some(st), Some(previous)) = (state(scope), previous) {
        st.borrow_mut().enter(previous);
    }
}

/// Installs the promise hook, and with it the private key and the context the
/// hook enters. Idempotent: the second call is the cheap `enabled` test.
pub(crate) fn enable(
    isolate: &mut v8::OwnedIsolate,
    context: &v8::Global<v8::Context>,
    state: &Rc<RefCell<ContextState>>,
) {
    if state.borrow().enabled {
        return;
    }
    {
        v8::scope!(let scope, isolate);
        let name = v8::String::new(scope, "es-runtime.asyncContext");
        let key = v8::Private::new(scope, name);
        let mut st = state.borrow_mut();
        st.key = Some(v8::Global::new(scope, key));
        st.context = Some(context.clone());
        st.enabled = true;
    }
    isolate.set_promise_hook(promise_hook);
}

/// V8's promise hook. `extern "C"`, so a panic here would unwind across C++
/// frames; contained for the same reason every other V8 callback in this crate
/// is (DECISIONS.md D15).
unsafe extern "C" fn promise_hook(
    kind: v8::PromiseHookType,
    promise: v8::Local<v8::Promise>,
    _parent: v8::Local<v8::Value>,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        promise_hook_inner(kind, promise);
    }));
}

fn promise_hook_inner(kind: v8::PromiseHookType, promise: v8::Local<v8::Promise>) {
    // `Resolve` fires at the head of a resolve/reject function, which is neither
    // a scheduling point nor a continuation — nothing to stamp, nothing to
    // install. Returning before building a scope keeps it off the cost of every
    // settled promise.
    if matches!(kind, v8::PromiseHookType::Resolve) {
        return;
    }

    // SAFETY: the hook is called by V8 with `promise` live for its duration,
    // which is what `CallbackScope` is for.
    v8::callback_scope!(unsafe scope, promise);
    v8::scope!(let scope, scope);

    let Some(state_rc) = state(scope) else {
        return;
    };
    let Some((context, key)) = ({
        let st = state_rc.borrow();
        match (&st.context, &st.key) {
            (Some(context), Some(key)) => Some((context.clone(), key.clone())),
            _ => None,
        }
    }) else {
        return;
    };

    // Entered explicitly rather than relying on whatever V8 happens to have
    // entered: the private-slot accessors read the *current* context, and a hook
    // can fire with none. This isolate has exactly one guest context, so there
    // is no ambiguity about which.
    let context = v8::Local::new(scope, &context);
    let scope = &mut v8::ContextScope::new(scope, context);
    let key = v8::Local::new(scope, &key);
    let object: v8::Local<v8::Object> = promise.into();

    match kind {
        v8::PromiseHookType::Init => {
            let (frame, parent_task, id) = {
                let mut st = state_rc.borrow_mut();
                let id = st.next_task;
                st.next_task += 1;
                (st.current.frame.clone(), st.current.task, id)
            };
            let frame: v8::Local<v8::Value> = match &frame {
                Some(frame) => v8::Local::new(scope, frame),
                None => v8::undefined(scope).into(),
            };
            let id = v8::Number::new(scope, id as f64);
            let parent = v8::Number::new(scope, parent_task as f64);
            let record = v8::Array::new_with_elements(scope, &[frame, id.into(), parent.into()]);
            let _ = object.set_private(scope, key, record.into());
        }
        v8::PromiseHookType::Before => {
            // A promise with no stamp was created before the hook existed. It
            // enters the *root*, not the caller's mapping: inheriting whatever
            // happened to be current would attach a request's values to a
            // continuation that was scheduled outside it.
            let next = read_record(scope, object, key).unwrap_or_default();
            let mut st = state_rc.borrow_mut();
            let previous = st.enter(next);
            st.stack.push(previous);
        }
        v8::PromiseHookType::After => {
            let mut st = state_rc.borrow_mut();
            // Balanced by V8 in normal operation; an empty stack means a `Before`
            // was missed (a hook installed mid-flight), and the root is the only
            // safe answer.
            let previous = st.stack.pop().unwrap_or_default();
            st.current = previous;
        }
        v8::PromiseHookType::Resolve => {}
    }
}

/// Reads back the `[frame, taskId, parentId]` a previous `Init` stamped.
fn read_record(
    scope: &mut v8::PinScope<'_, '_>,
    object: v8::Local<v8::Object>,
    key: v8::Local<v8::Private>,
) -> Option<Capture> {
    let record = object.get_private(scope, key)?;
    let record = v8::Local::<v8::Array>::try_from(record).ok()?;
    let frame = record.get_index(scope, 0)?;
    let task = record.get_index(scope, 1)?.number_value(scope)?;
    let parent = record.get_index(scope, 2)?.number_value(scope)?;
    Some(Capture {
        frame: (!frame.is_undefined()).then(|| v8::Global::new(scope, frame)),
        task: task as TaskId,
        // Always `Some`: a stamped promise was created *by* a task, even if that
        // task was the root (id `ROOT_TASK`). `None` is reserved for the root
        // task itself, which nothing created — see [`Capture::default`].
        parent: Some(parent as TaskId),
    })
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
    crate::op::install_global_fn(scope, global, "__ctx_frame", ctx_frame, None)?;
    crate::op::install_global_fn(scope, global, "__ctx_swap", ctx_swap, None)?;
    crate::op::install_global_fn(scope, global, "__ctx_task", ctx_task, None)?;
    crate::op::install_global_fn(scope, global, "__ctx_parent", ctx_parent, None)
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
contained!(ctx_frame, ctx_frame_inner);
contained!(ctx_swap, ctx_swap_inner);
contained!(ctx_task, ctx_task_inner);
contained!(ctx_parent, ctx_parent_inner);

/// `__ctx_enabled()` → whether `runtime:context` has been loaded.
fn ctx_enabled_inner(
    scope: &mut v8::PinScope,
    _args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    let enabled = state(scope).is_some_and(|st| st.borrow().enabled());
    rv.set(v8::Boolean::new(scope, enabled).into());
}

/// `__ctx_frame()` → the current mapping, or `undefined` for the root.
fn ctx_frame_inner(
    scope: &mut v8::PinScope,
    _args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    let Some(state_rc) = state(scope) else {
        return;
    };
    let frame = state_rc.borrow().current.frame.clone();
    if let Some(frame) = frame {
        rv.set(v8::Local::new(scope, &frame));
    }
}

/// `__ctx_swap(next)` → makes `next` current and returns what was. One call
/// rather than a get/set pair, so the module's `run()` cannot leave the two out
/// of step on a throw.
fn ctx_swap_inner(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    let Some(state_rc) = state(scope) else {
        return;
    };
    let next = args.get(0);
    let next = (!next.is_undefined()).then(|| v8::Global::new(scope, next));
    let previous = {
        let mut st = state_rc.borrow_mut();
        std::mem::replace(&mut st.current.frame, next)
    };
    if let Some(previous) = previous {
        rv.set(v8::Local::new(scope, &previous));
    }
}

/// `__ctx_task()` → the executing task's id.
fn ctx_task_inner(
    scope: &mut v8::PinScope,
    _args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    let Some(state_rc) = state(scope) else {
        return;
    };
    let id = state_rc.borrow().current.task;
    rv.set(v8::Number::new(scope, id as f64).into());
}

/// `__ctx_parent()` → the id of the task that scheduled this one, or `-1`.
fn ctx_parent_inner(
    scope: &mut v8::PinScope,
    _args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    let Some(state_rc) = state(scope) else {
        return;
    };
    let parent = state_rc.borrow().current.parent;
    let parent = parent.map_or(NO_PARENT, |id| id as f64);
    rv.set(v8::Number::new(scope, parent).into());
}
