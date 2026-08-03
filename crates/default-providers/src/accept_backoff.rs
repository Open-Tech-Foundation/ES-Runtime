//! The shared retry policy for a failed `accept()` on a listening socket.
//!
//! A failed `accept` is not a failed listener. `ECONNABORTED` is what a client
//! that hangs up between the SYN and the accept looks like; `EMFILE`/`ENFILE`
//! is a descriptor table that is momentarily full; `EINTR` is a signal. All
//! three are ordinary events on a busy public port, and none of them says the
//! socket is finished. An acceptor that leaves its loop on the first error
//! reaches the worst of both outcomes: it serves nothing, while the port stays
//! bound so nothing else can take the address either — and it does so silently.
//! So every error is retried instead, and the loop ends only when the server is
//! closed.
//!
//! The delay is what makes retrying safe. Without it an error that *persists* —
//! a descriptor limit that stays hit, a wedged socket — would spin a core at the
//! speed of the syscall. The wait doubles from [`MIN`] to [`MAX`] for as long as
//! errors keep coming and resets the moment a connection is accepted, so one
//! transient failure costs 5ms while a sustained one settles at a wakeup a
//! second.

use std::time::Duration;

/// Delay after the first failure in a run.
const MIN: Duration = Duration::from_millis(5);

/// Ceiling the delay doubles up to.
const MAX: Duration = Duration::from_secs(1);

/// A doubling delay between failed `accept()` calls, reset by each success.
pub(crate) struct AcceptBackoff(Duration);

impl AcceptBackoff {
    pub(crate) fn new() -> Self {
        Self(MIN)
    }

    /// How long to wait before accepting again, doubling the wait (up to
    /// [`MAX`]) for the failure after this one.
    pub(crate) fn next_delay(&mut self) -> Duration {
        let delay = self.0;
        self.0 = (self.0 * 2).min(MAX);
        delay
    }

    /// Back to [`MIN`], because a connection came through.
    pub(crate) fn reset(&mut self) {
        self.0 = MIN;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_wait_is_short_and_each_one_after_it_doubles() {
        let mut backoff = AcceptBackoff::new();
        assert_eq!(backoff.next_delay(), Duration::from_millis(5));
        assert_eq!(backoff.next_delay(), Duration::from_millis(10));
        assert_eq!(backoff.next_delay(), Duration::from_millis(20));
        assert_eq!(backoff.next_delay(), Duration::from_millis(40));
    }

    #[test]
    fn the_wait_stops_doubling_at_the_ceiling() {
        let mut backoff = AcceptBackoff::new();
        // Long enough to be well past 1s if it kept doubling unchecked.
        for _ in 0..20 {
            assert!(backoff.next_delay() <= MAX);
        }
        assert_eq!(backoff.next_delay(), MAX);
    }

    #[test]
    fn a_successful_accept_puts_the_wait_back_to_the_start() {
        let mut backoff = AcceptBackoff::new();
        for _ in 0..10 {
            backoff.next_delay();
        }
        backoff.reset();
        assert_eq!(backoff.next_delay(), Duration::from_millis(5));
    }
}
