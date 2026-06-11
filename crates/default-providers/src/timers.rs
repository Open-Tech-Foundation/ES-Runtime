//! tokio-backed [`Timers`].

use std::time::Duration;

use es_runtime_providers::{BoxFuture, Timers};

/// A [`Timers`] provider that parks on the tokio timer wheel.
pub struct TokioTimers;

impl Timers for TokioTimers {
    fn sleep(&self, delay_ms: u64) -> BoxFuture<()> {
        Box::pin(tokio::time::sleep(Duration::from_millis(delay_ms)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn sleep_waits_at_least_the_delay() {
        let start = std::time::Instant::now();
        TokioTimers.sleep(15).await;
        assert!(start.elapsed() >= Duration::from_millis(15));
    }
}
