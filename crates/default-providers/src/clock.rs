//! OS-backed [`Clock`].

use std::time::{Instant, SystemTime, UNIX_EPOCH};

use es_runtime_providers::Clock;

/// A [`Clock`] reading the host's monotonic and wall clocks.
///
/// Monotonic time is measured from when the clock was created, so it starts near
/// zero and never goes backwards; wall time is the system clock.
pub struct SystemClock {
    base: Instant,
}

impl SystemClock {
    /// Creates a clock whose monotonic origin is now.
    pub fn new() -> Self {
        SystemClock {
            base: Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        SystemClock::new()
    }
}

impl Clock for SystemClock {
    fn monotonic_ms(&self) -> u64 {
        // Saturating cast: `u64` ms is ~584 million years of uptime.
        self.base.elapsed().as_millis() as u64
    }

    fn wall_ms(&self) -> u64 {
        // Before the Unix epoch should never happen; treat as 0 if it does.
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monotonic_does_not_go_backwards() {
        let clock = SystemClock::new();
        let a = clock.monotonic_ms();
        let b = clock.monotonic_ms();
        assert!(b >= a);
    }

    #[test]
    fn wall_clock_is_after_2020() {
        // 2020-01-01 in ms since epoch; a sanity check the wall clock is real.
        const Y2020_MS: u64 = 1_577_836_800_000;
        assert!(SystemClock::new().wall_ms() > Y2020_MS);
    }
}
