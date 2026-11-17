//! Leaving an isolate between turns, so one thread can serve many (D227).
//!
//! V8 keeps a stack of *entered* isolates per thread, and an isolate runs code
//! only while it is the innermost. An `OwnedIsolate` is entered when it is made
//! and left when it is dropped, so a thread holding two could only ever run the
//! newer. A pool thread that owns many isolates instead leaves each after its
//! turn and enters it again for the next. V8 needs no `Locker` for that: the
//! lock is for an isolate that more than one thread touches, and these never
//! leave the thread they were made on (`V8Engine` is `!Send`, and so is
//! [`TurnHandle`]).
//!
//! V8 requires an isolate to be the innermost when it is left, and does not
//! check it in a release build. rusty_v8 does not expose V8's stack, so this
//! module keeps a copy of it, and a wrong order is a panic naming the mistake
//! rather than a corrupted stack.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

thread_local! {
    /// The isolates entered on this thread through this module, innermost
    /// last, by address. Per thread because V8's is: it is the thread's
    /// entered stack, not anything belonging to one agent.
    static ENTERED: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
}

/// One isolate's place on its thread: the isolate, by V8's address for it —
/// which, unlike the address of the `OwnedIsolate` that holds it, does not move
/// when the engine does — whether it is entered, and whether it still exists.
struct State {
    isolate: v8::UnsafeRawIsolatePtr,
    left: Cell<bool>,
    alive: Cell<bool>,
}

impl State {
    fn leave(&self) {
        if !self.alive.get() || self.left.get() {
            return;
        }
        let me = key(self.isolate);
        ENTERED.with(|stack| {
            let mut stack = stack.borrow_mut();
            assert_eq!(
                stack.last(),
                Some(&me),
                "an isolate can be left only while it is the innermost one entered on its thread"
            );
            stack.pop();
        });
        // SAFETY: alive, entered on this thread and innermost, checked above.
        unsafe { self.isolate().exit() };
        self.left.set(true);
    }

    fn enter(&self) {
        if !self.alive.get() || !self.left.get() {
            return;
        }
        // SAFETY: alive — the engine that owns it clears `alive` before it
        // goes — and on the thread that made it, since nothing here is `Send`.
        unsafe { self.isolate().enter() };
        ENTERED.with(|stack| stack.borrow_mut().push(key(self.isolate)));
        self.left.set(false);
    }

    /// A non-owning view of the isolate, for `enter`/`exit`.
    ///
    /// # Safety
    ///
    /// The isolate must still exist.
    unsafe fn isolate(&self) -> v8::Isolate {
        // SAFETY: per the contract, a live isolate's address.
        unsafe { v8::Isolate::from_raw_isolate_ptr(self.isolate) }
    }
}

/// The engine's own record, dropped before the isolate (it is declared first).
pub(crate) struct Turn(Rc<State>);

impl Turn {
    /// For an isolate just made, and so entered and innermost.
    pub(crate) fn entered(isolate: &v8::Isolate) -> Self {
        // SAFETY: reading the address; nothing is dereferenced.
        let isolate = unsafe { isolate.as_raw_isolate_ptr() };
        ENTERED.with(|stack| stack.borrow_mut().push(key(isolate)));
        Turn(Rc::new(State {
            isolate,
            left: Cell::new(false),
            alive: Cell::new(true),
        }))
    }

    /// Leaves the isolate on this thread. Idempotent.
    ///
    /// # Panics
    ///
    /// If another isolate was entered on this thread after this one and is
    /// still entered: V8 leaves only the innermost.
    pub(crate) fn leave(&self) {
        self.0.leave();
    }

    /// Enters the isolate on this thread, making it the innermost. Idempotent.
    pub(crate) fn enter(&self) {
        self.0.enter();
    }

    pub(crate) fn handle(&self) -> TurnHandle {
        TurnHandle(self.0.clone())
    }
}

/// Leaves the isolate as `OwnedIsolate::drop` requires it — entered, innermost,
/// and off this module's stack, since that drop is what leaves it for good —
/// and tells every [`TurnHandle`] it is gone.
impl Drop for Turn {
    fn drop(&mut self) {
        let state = &self.0;
        if state.left.get() {
            // SAFETY: the isolate still exists — the `OwnedIsolate` field drops
            // after this one — and is on the thread that made it.
            unsafe { state.isolate().enter() };
            state.left.set(false);
        } else {
            let me = key(state.isolate);
            ENTERED.with(|stack| {
                let mut stack = stack.borrow_mut();
                if let Some(at) = stack.iter().rposition(|&k| k == me) {
                    stack.remove(at);
                }
            });
        }
        state.alive.set(false);
    }
}

/// Enters and leaves one engine's isolate without borrowing the engine.
///
/// What a pool thread wraps around an agent's drive: the drive owns the
/// runtime and borrows it for as long as it runs, so the thread enters the
/// isolate before each poll and leaves it after through this instead. Once the
/// engine is dropped — a drive that ends drops its runtime mid-poll — both are
/// no-ops. `!Send`: the isolate never leaves its thread.
#[derive(Clone)]
pub struct TurnHandle(Rc<State>);

impl TurnHandle {
    /// Enters the isolate on this thread. Idempotent; a no-op once the engine
    /// is gone.
    pub fn enter(&self) {
        self.0.enter();
    }

    /// Leaves the isolate on this thread. Idempotent; a no-op once the engine
    /// is gone.
    ///
    /// # Panics
    ///
    /// If another isolate entered on this thread after this one is still
    /// entered.
    pub fn leave(&self) {
        self.0.leave();
    }
}

fn key(isolate: v8::UnsafeRawIsolatePtr) -> usize {
    // The address is the identity; `UnsafeRawIsolatePtr` exposes no other.
    // SAFETY: `UnsafeRawIsolatePtr` is `repr(transparent)` over a pointer.
    unsafe { std::mem::transmute::<v8::UnsafeRawIsolatePtr, usize>(isolate) }
}
