//! A ceiling on one isolate's `ArrayBuffer` storage (DECISIONS D227).
//!
//! V8's heap limit covers the JS heap and nothing else: an `ArrayBuffer`'s
//! bytes come from a separate allocator, so a program allocating buffers can
//! grow far past a heap ceiling without approaching it. An agent given a
//! buffer ceiling gets an allocator of its own that counts what it holds, and
//! refuses an allocation that would take it past the ceiling.
//!
//! Refused, not terminated. A buffer that is garbage still holds its bytes
//! until a collection frees it, so a program that allocates and drops buffers
//! in a loop meets the ceiling with most of it already dead. V8 answers a
//! refused allocation by collecting and asking again — twice, then once more
//! after collecting everything it can — so only memory that is really live
//! stays refused, and the allocation then throws `RangeError: Array buffer
//! allocation failed`, which the program can handle. Terminating on the first
//! refusal would end isolates whose live data was well inside the ceiling.
//!
//! One more step makes the retry honest. V8 hands a dead buffer's bytes back
//! to this allocator only when it finalises the sweep that found it dead, on
//! the isolate's own thread at a later point — so its retry can come before
//! the bytes do. Rarely on an idle machine; on a busy one, a loop that
//! allocated and dropped 8 MiB buffers under a 16 MiB ceiling was ended in most
//! runs. During V8's retry sequence the allocator therefore lets the isolate
//! borrow against what the collection freed and has not yet returned, up to
//! twice the ceiling: a retry that comes close behind a refusal may go past the
//! ceiling, a fresh allocation may not, and no single buffer may be larger
//! than the ceiling. Live data past it still fails, at most at twice it.
//!
//! **Rejected: V8's `--no-concurrent-array-buffer-sweeping`,** which finalises
//! frees during the collection itself and fixes the race outright. The flag is
//! process-wide: measured on a release build, it made the main program's
//! buffer churn 2.4x faster at 1 KiB and 1.3–1.7x slower at 64 KiB and 1 MiB —
//! a change to every program's collector for one isolate feature.
//!
//! Counted at allocation, so a buffer is charged for its full length from the
//! moment it exists, and released when V8 frees it.

use std::alloc::Layout;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// How close behind a refusal another must come to be V8's retry of it. V8
/// collects and asks again at once, but a full collection on a busy machine
/// takes a while; generous, because what a retry may borrow is capped anyway.
const RETRY_WINDOW: Duration = Duration::from_millis(500);

/// What one isolate's buffers may hold, and what they hold now.
struct BufferBudget {
    limit: usize,
    used: AtomicUsize,
    /// When the last refusal was, as nanoseconds since `epoch`; 0 for never.
    refused_at: AtomicU64,
    epoch: Instant,
}

impl BufferBudget {
    /// Reserves `len` bytes within the ceiling — or, as V8's retry after a
    /// collection, within twice it, borrowing against frees not yet returned.
    fn reserve(&self, len: usize) -> bool {
        if self.try_reserve(len, self.limit) {
            return true;
        }
        let now = self.nanos();
        let before = self.refused_at.swap(now, Ordering::SeqCst);
        let is_retry = before != 0 && now.saturating_sub(before) <= nanos(RETRY_WINDOW);
        is_retry && len <= self.limit && self.try_reserve(len, self.limit.saturating_mul(2))
    }

    fn try_reserve(&self, len: usize, ceiling: usize) -> bool {
        let mut used = self.used.load(Ordering::SeqCst);
        loop {
            let Some(next) = used.checked_add(len).filter(|&n| n <= ceiling) else {
                return false;
            };
            match self
                .used
                .compare_exchange(used, next, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => return true,
                Err(actual) => used = actual,
            }
        }
    }

    fn nanos(&self) -> u64 {
        nanos(self.epoch.elapsed()).max(1)
    }
}

fn nanos(d: Duration) -> u64 {
    u64::try_from(d.as_nanos()).unwrap_or(u64::MAX)
}

/// With a buffer ceiling, `params` with a counting allocator installed;
/// otherwise `params` unchanged.
pub(crate) fn install(limit: Option<usize>, params: v8::CreateParams) -> v8::CreateParams {
    let Some(limit) = limit else {
        return params;
    };
    let budget = Arc::new(BufferBudget {
        limit,
        used: AtomicUsize::new(0),
        refused_at: AtomicU64::new(0),
        epoch: Instant::now(),
    });
    // SAFETY: the allocator owns the reference `into_raw` gives it and gives it
    // back in `drop`, which V8 calls once the allocator is released.
    let allocator = unsafe { v8::new_rust_allocator(Arc::into_raw(budget), &VTABLE) };
    params.array_buffer_allocator(allocator.make_shared())
}

static VTABLE: v8::RustAllocatorVtable<BufferBudget> = v8::RustAllocatorVtable {
    allocate,
    allocate_uninitialized,
    free,
    drop: drop_budget,
};

/// Every block is laid out as if at least one byte long, so a zero-length
/// buffer gets a real allocation and `free` can rebuild the same layout.
fn layout(len: usize) -> Option<Layout> {
    Layout::from_size_align(len.max(1), 16).ok()
}

unsafe extern "C" fn allocate(budget: &BufferBudget, len: usize) -> *mut std::ffi::c_void {
    allocate_with(budget, len, true)
}

unsafe extern "C" fn allocate_uninitialized(
    budget: &BufferBudget,
    len: usize,
) -> *mut std::ffi::c_void {
    allocate_with(budget, len, false)
}

fn allocate_with(budget: &BufferBudget, len: usize, zeroed: bool) -> *mut std::ffi::c_void {
    let Some(layout) = layout(len) else {
        return std::ptr::null_mut();
    };
    if !budget.reserve(len) {
        return std::ptr::null_mut();
    }
    // SAFETY: `layout` has a non-zero size.
    let data = unsafe {
        if zeroed {
            std::alloc::alloc_zeroed(layout)
        } else {
            std::alloc::alloc(layout)
        }
    };
    if data.is_null() {
        budget.used.fetch_sub(len, Ordering::SeqCst);
    }
    data.cast()
}

unsafe extern "C" fn free(budget: &BufferBudget, data: *mut std::ffi::c_void, len: usize) {
    if data.is_null() {
        return;
    }
    if let Some(layout) = layout(len) {
        // SAFETY: `data` came from `allocate_with` with this same length, and
        // so this same layout.
        unsafe { std::alloc::dealloc(data.cast(), layout) };
        budget.used.fetch_sub(len, Ordering::SeqCst);
    }
}

unsafe extern "C" fn drop_budget(budget: *const BufferBudget) {
    // SAFETY: the reference `install` handed over with `into_raw`.
    drop(unsafe { Arc::from_raw(budget) });
}
