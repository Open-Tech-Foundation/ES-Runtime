//! Leaving an isolate between turns, so one thread can serve many (D227).
//!
//! V8 keeps a stack of *entered* isolates per thread, and an isolate runs code
//! only while it is the innermost. An `OwnedIsolate` is entered when it is made
//! and left when it is dropped, so a thread holding two could only ever run the
//! newer. A pool thread that owns many isolates instead leaves each after its
//! turn and enters it again for the next. V8 needs no `Locker` for that: the
//! lock is for an isolate that more than one thread touches, and these never
//! leave the thread they were made on (`V8Engine` is `!Send`).
//!
//! V8 requires an isolate to be the innermost when it is left, and does not
//! check it in a release build. rusty_v8 does not expose V8's stack, so this
//! module keeps a copy of it, and a wrong order is a panic naming the mistake
//! rather than a corrupted stack.

use std::cell::RefCell;

thread_local! {
    /// The isolates entered on this thread through this module, innermost
    /// last, by address. Per thread because V8's is: it is the thread's
    /// entered stack, not anything belonging to one agent.
    static ENTERED: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
}

/// Whether an engine's isolate is currently entered on its thread, and the
/// isolate itself, by V8's address for it — which, unlike the address of the
/// `OwnedIsolate` that holds it, does not move when the engine does.
pub(crate) struct Turn {
    isolate: v8::UnsafeRawIsolatePtr,
    left: bool,
}

impl Turn {
    /// For an isolate just made, and so entered and innermost.
    pub(crate) fn entered(isolate: &v8::Isolate) -> Self {
        // SAFETY: reading the address; nothing is dereferenced.
        let isolate = unsafe { isolate.as_raw_isolate_ptr() };
        ENTERED.with(|stack| stack.borrow_mut().push(key(isolate)));
        Turn {
            isolate,
            left: false,
        }
    }

    /// Leaves the isolate on this thread. Idempotent.
    ///
    /// # Panics
    ///
    /// If another isolate was entered on this thread after this one and is
    /// still entered: V8 leaves only the innermost.
    pub(crate) fn leave(&mut self, isolate: &v8::Isolate) {
        if self.left {
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
        // SAFETY: entered on this thread and innermost, checked above.
        unsafe { isolate.exit() };
        self.left = true;
    }

    /// Enters the isolate on this thread, making it the innermost. Idempotent.
    pub(crate) fn enter(&mut self, isolate: &v8::Isolate) {
        if !self.left {
            return;
        }
        // SAFETY: the isolate is alive (the engine owns it) and on the thread
        // that made it (`V8Engine` is `!Send`).
        unsafe { isolate.enter() };
        ENTERED.with(|stack| stack.borrow_mut().push(key(self.isolate)));
        self.left = false;
    }
}

/// Runs before the engine's other fields drop (it is declared first), and
/// leaves the isolate as `OwnedIsolate::drop` requires it: entered, innermost,
/// and off this module's stack, since that drop is what leaves it for good.
impl Drop for Turn {
    fn drop(&mut self) {
        if self.left {
            // SAFETY: the isolate is still alive — the `OwnedIsolate` field
            // drops after this one — and on the thread that made it.
            let isolate = unsafe { v8::Isolate::from_raw_isolate_ptr(self.isolate) };
            // SAFETY: as above.
            unsafe { isolate.enter() };
            self.left = false;
        } else {
            let me = key(self.isolate);
            ENTERED.with(|stack| {
                let mut stack = stack.borrow_mut();
                if let Some(at) = stack.iter().rposition(|&k| k == me) {
                    stack.remove(at);
                }
            });
        }
    }
}

fn key(isolate: v8::UnsafeRawIsolatePtr) -> usize {
    // The address is the identity; `UnsafeRawIsolatePtr` exposes no other.
    // SAFETY: `UnsafeRawIsolatePtr` is `repr(transparent)` over a pointer.
    unsafe { std::mem::transmute::<v8::UnsafeRawIsolatePtr, usize>(isolate) }
}
