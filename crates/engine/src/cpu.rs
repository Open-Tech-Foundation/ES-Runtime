//! A CPU budget for one isolate's turns (DECISIONS D227).
//!
//! An `Isolate` shares its thread with others, so the one thing it must not do
//! is keep that thread: a turn — from when it is given the thread until it next
//! waits — may use at most so much CPU time. CPU, not wall time: a busy machine
//! that preempts the thread does not count against the isolate, and waiting is
//! never in a turn at all.
//!
//! Only the thread running the isolate can read its own CPU clock, so the check
//! runs there: something outside (the pool's watchdog) asks V8 to interrupt the
//! isolate, and the interrupt — which V8 runs on the isolate's thread at its
//! next check — compares the thread's CPU time with the turn's start, and
//! terminates the isolate if the turn is over its budget. The watchdog asks only
//! once a turn's *wall* time has passed the budget, since CPU time can never be
//! ahead of it, so a short turn costs nothing but two clock reads.
//!
//! The interrupt carries the budget's id, not the budget. V8 runs a requested
//! interrupt only if the isolate reaches an interrupt point, and an isolate
//! terminated and disposed with a request pending drops it silently, callback
//! and all. An owned reference handed over with the request leaked there, and
//! took the isolate's handle with it — about 64 bytes for every isolate a budget
//! ended. An id is looked up in [`BUDGETS`] when the interrupt runs, and is
//! nothing if the budget has gone.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, Weak};

use crate::InterruptHandle;

/// One isolate's per-turn CPU budget. `Send + Sync`: begun and ended on the
/// isolate's thread, checked from any.
pub struct CpuBudget {
    /// Its key in [`BUDGETS`], which is what an interrupt request carries.
    id: u64,
    limit_ms: f64,
    /// The thread's CPU clock when the current turn began, as `f64` bits.
    turn_start: AtomicU64,
    in_turn: AtomicBool,
    exceeded: AtomicBool,
    interrupt: InterruptHandle,
}

impl CpuBudget {
    /// A budget of `limit_ms` CPU milliseconds a turn, enforced by terminating
    /// through `interrupt`.
    pub fn new(limit_ms: u64, interrupt: InterruptHandle) -> Arc<Self> {
        let budget = Arc::new(CpuBudget {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            #[allow(clippy::cast_precision_loss)]
            limit_ms: limit_ms as f64,
            turn_start: AtomicU64::new(0),
            in_turn: AtomicBool::new(false),
            exceeded: AtomicBool::new(false),
            interrupt,
        });
        if let Ok(mut budgets) = BUDGETS.lock() {
            budgets.insert(budget.id, Arc::downgrade(&budget));
        }
        budget
    }

    /// The budget, in CPU milliseconds a turn.
    pub fn limit_ms(&self) -> f64 {
        self.limit_ms
    }

    /// A turn begins. Call on the isolate's thread.
    pub fn begin_turn(&self) {
        let now = crate::sysinfo::thread_cpu_ms();
        self.turn_start.store(now.to_bits(), Ordering::SeqCst);
        self.in_turn.store(true, Ordering::SeqCst);
    }

    /// The turn is over. Call on the isolate's thread.
    pub fn end_turn(&self) {
        self.in_turn.store(false, Ordering::SeqCst);
    }

    /// Whether this budget terminated its isolate. Sticky.
    pub fn exceeded(&self) -> bool {
        self.exceeded.load(Ordering::SeqCst)
    }

    /// Asks the isolate, from any thread, to check its turn against the budget
    /// at its next interrupt point. A no-op once the isolate is gone.
    pub fn check(self: &Arc<Self>) {
        // The id, as the pointer-sized value V8 hands back: never dereferenced,
        // so a request V8 drops owns nothing and leaks nothing.
        let data = usize::try_from(self.id).unwrap_or(usize::MAX) as *mut std::ffi::c_void;
        // Refused only when the isolate is gone, and then there is nothing to do.
        let _ = self.interrupt.isolate.request_interrupt(cpu_check, data);
    }
}

impl Drop for CpuBudget {
    fn drop(&mut self) {
        if let Ok(mut budgets) = BUDGETS.lock() {
            budgets.remove(&self.id);
        }
    }
}

/// Every budget still alive, by id, for the interrupt to find its own.
static BUDGETS: LazyLock<Mutex<HashMap<u64, Weak<CpuBudget>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Runs on the isolate's own thread, at an interrupt point, so the CPU clock it
/// reads is the one the turn has been spending.
unsafe extern "C" fn cpu_check(_isolate: v8::UnsafeRawIsolatePtr, data: *mut std::ffi::c_void) {
    let id = data as usize as u64;
    let Some(budget) = BUDGETS
        .lock()
        .ok()
        .and_then(|budgets| budgets.get(&id).and_then(Weak::upgrade))
    else {
        return;
    };
    if !budget.in_turn.load(Ordering::SeqCst) {
        return;
    }
    let started = f64::from_bits(budget.turn_start.load(Ordering::SeqCst));
    if crate::sysinfo::thread_cpu_ms() - started > budget.limit_ms {
        budget.exceeded.store(true, Ordering::SeqCst);
        budget.interrupt.terminate();
    }
}
