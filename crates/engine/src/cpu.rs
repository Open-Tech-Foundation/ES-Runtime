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

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::InterruptHandle;

/// One isolate's per-turn CPU budget. `Send + Sync`: begun and ended on the
/// isolate's thread, checked from any.
pub struct CpuBudget {
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
        Arc::new(CpuBudget {
            #[allow(clippy::cast_precision_loss)]
            limit_ms: limit_ms as f64,
            turn_start: AtomicU64::new(0),
            in_turn: AtomicBool::new(false),
            exceeded: AtomicBool::new(false),
            interrupt,
        })
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
        let data = Arc::into_raw(self.clone())
            .cast_mut()
            .cast::<std::ffi::c_void>();
        if !self.interrupt.isolate.request_interrupt(cpu_check, data) {
            // SAFETY: the request was refused, so the callback will never take
            // back the reference `into_raw` gave away; take it back here.
            drop(unsafe { Arc::from_raw(data.cast_const().cast::<CpuBudget>()) });
        }
    }
}

/// Runs on the isolate's own thread, at an interrupt point, so the CPU clock it
/// reads is the one the turn has been spending.
unsafe extern "C" fn cpu_check(_isolate: v8::UnsafeRawIsolatePtr, data: *mut std::ffi::c_void) {
    // SAFETY: `data` came from `Arc::into_raw` in `check`, once per request.
    let budget = unsafe { Arc::from_raw(data.cast_const().cast::<CpuBudget>()) };
    if !budget.in_turn.load(Ordering::SeqCst) {
        return;
    }
    let started = f64::from_bits(budget.turn_start.load(Ordering::SeqCst));
    if crate::sysinfo::thread_cpu_ms() - started > budget.limit_ms {
        budget.exceeded.store(true, Ordering::SeqCst);
        budget.interrupt.terminate();
    }
}
