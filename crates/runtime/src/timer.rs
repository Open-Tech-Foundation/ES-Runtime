//! Deadline-ordered timer scheduling for the driven loop (ARCHITECTURE.md §5).
//!
//! The queue owns only *scheduling* — deadlines, ordering, repeat bookkeeping.
//! The JS callbacks themselves live in the engine (it owns the V8 handles); the
//! queue refers to them by [`TimerId`]. Time is supplied by the embedder at each
//! tick, so the runtime owns no clock (the `Clock`/`Timers` providers become the
//! source of that time in Phase 3).

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use es_runtime_engine::TimerId;

/// A single scheduled firing.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Scheduled {
    /// Absolute deadline in embedder-supplied milliseconds.
    deadline_ms: u64,
    id: TimerId,
    /// Interval to re-arm a repeating timer with.
    interval_ms: u64,
    repeat: bool,
}

// Ordering is by deadline, then id, so the queue is a deterministic min-heap
// (via `Reverse`): earlier deadlines — and, on ties, lower ids (insertion-ish
// order) — fire first.
impl Ord for Scheduled {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.deadline_ms
            .cmp(&other.deadline_ms)
            .then(self.id.cmp(&other.id))
    }
}

impl PartialOrd for Scheduled {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// A min-heap of pending timer firings keyed by deadline.
#[derive(Default)]
pub(crate) struct TimerQueue {
    heap: BinaryHeap<Reverse<Scheduled>>,
}

impl TimerQueue {
    /// Schedules timer `id` to fire `delay_ms` after `now_ms`. A `repeat` timer
    /// re-arms with the same delay each time it fires.
    pub(crate) fn schedule(&mut self, id: TimerId, now_ms: u64, delay_ms: u64, repeat: bool) {
        self.heap.push(Reverse(Scheduled {
            deadline_ms: now_ms.saturating_add(delay_ms),
            id,
            interval_ms: delay_ms,
            repeat,
        }));
    }

    /// Removes and returns every timer whose deadline is at or before `now_ms`,
    /// in fire order. Timers scheduled *after* this call (e.g. by a callback)
    /// are not returned until a later tick, which prevents a zero-delay timer
    /// from starving the loop within a single tick.
    pub(crate) fn take_due(&mut self, now_ms: u64) -> Vec<DueTimer> {
        let mut due = Vec::new();
        while let Some(Reverse(top)) = self.heap.peek() {
            if top.deadline_ms > now_ms {
                break;
            }
            let Reverse(slot) = self.heap.pop().expect("peeked");
            due.push(DueTimer {
                id: slot.id,
                interval_ms: slot.interval_ms,
                repeat: slot.repeat,
            });
        }
        due
    }

    /// Discards already-cleared timers from the front of the queue.
    ///
    /// `clearTimeout`/`clearInterval` deactivate a timer engine-side but cannot
    /// reach into this heap, so a cancelled entry would otherwise sit here until
    /// its original deadline — keeping [`is_empty`](Self::is_empty) false and
    /// [`next_deadline_ms`](Self::next_deadline_ms) pointing at a firing that
    /// will never happen, which makes a driver wait out the full delay before
    /// the process can exit.
    ///
    /// Pruning only the front is enough to keep both of those answers correct:
    /// popping stops at the first still-active timer, which is by definition the
    /// earliest one left, and if every entry is cancelled the queue empties. A
    /// cancelled entry deeper in the heap is harmless — an active timer ahead of
    /// it means there is genuinely still work — and is reclaimed once it reaches
    /// the front. That keeps the cost amortized O(1) per cancellation rather
    /// than a full O(n) rebuild on every tick.
    pub(crate) fn prune_cleared(&mut self, is_active: impl Fn(TimerId) -> bool) {
        while let Some(Reverse(top)) = self.heap.peek() {
            if is_active(top.id) {
                break;
            }
            self.heap.pop();
        }
    }

    /// The earliest pending deadline, if any.
    pub(crate) fn next_deadline_ms(&self) -> Option<u64> {
        self.heap.peek().map(|Reverse(s)| s.deadline_ms)
    }

    /// Whether any timer is scheduled.
    pub(crate) fn is_empty(&self) -> bool {
        self.heap.is_empty()
    }
}

/// A timer that has come due, returned by [`TimerQueue::take_due`].
pub(crate) struct DueTimer {
    pub(crate) id: TimerId,
    pub(crate) interval_ms: u64,
    pub(crate) repeat: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fires_in_deadline_order() {
        let mut q = TimerQueue::default();
        q.schedule(1, 0, 30, false);
        q.schedule(2, 0, 10, false);
        q.schedule(3, 0, 20, false);
        let due = q.take_due(100);
        let ids: Vec<_> = due.iter().map(|d| d.id).collect();
        assert_eq!(ids, vec![2, 3, 1]);
    }

    #[test]
    fn only_due_timers_are_taken() {
        let mut q = TimerQueue::default();
        q.schedule(1, 0, 10, false);
        q.schedule(2, 0, 50, false);
        let due = q.take_due(20);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].id, 1);
        assert_eq!(q.next_deadline_ms(), Some(50));
    }

    #[test]
    fn cleared_timers_do_not_hold_the_queue() {
        // A long timer that JS has since cleared must not keep the queue alive,
        // or a driver waits out its full delay before the process can exit.
        let mut q = TimerQueue::default();
        q.schedule(1, 0, 60_000, false);
        q.prune_cleared(|_| false);
        assert!(q.is_empty());
        assert_eq!(q.next_deadline_ms(), None);
    }

    #[test]
    fn pruning_stops_at_the_first_live_timer() {
        // Clearing the earliest timer must expose the next one's deadline, not
        // drop it and not leave the stale one in front.
        let mut q = TimerQueue::default();
        q.schedule(1, 0, 10, false);
        q.schedule(2, 0, 50, false);
        q.prune_cleared(|id| id != 1);
        assert!(!q.is_empty());
        assert_eq!(q.next_deadline_ms(), Some(50));
        assert_eq!(q.take_due(100).len(), 1);
    }

    #[test]
    fn a_cleared_timer_behind_a_live_one_is_reclaimed_later() {
        // Only the front is pruned, so the cancelled entry survives this pass —
        // correctly, since the live timer means there is still work — and is
        // dropped once it reaches the front.
        let mut q = TimerQueue::default();
        q.schedule(1, 0, 10, false);
        q.schedule(2, 0, 60_000, false);
        q.prune_cleared(|id| id != 2);
        assert_eq!(q.next_deadline_ms(), Some(10));
        assert_eq!(q.take_due(10).len(), 1);
        q.prune_cleared(|id| id != 2);
        assert!(q.is_empty());
    }

    #[test]
    fn empty_until_scheduled() {
        let mut q = TimerQueue::default();
        assert!(q.is_empty());
        assert_eq!(q.next_deadline_ms(), None);
        q.schedule(1, 5, 10, true);
        assert!(!q.is_empty());
        assert_eq!(q.next_deadline_ms(), Some(15));
    }
}
