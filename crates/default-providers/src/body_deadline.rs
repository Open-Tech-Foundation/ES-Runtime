//! A deadline on a request body that the body itself extends by arriving.
//!
//! The head timeout (`HttpTimeouts::header_read`) is the classic slowloris
//! bound, and it stops the moment the head is complete. A peer that sends a
//! well-formed head and then dribbles its body one byte at a time is past it,
//! and holds a connection, a task, a descriptor, and the handler awaiting the
//! body for as long as it cares to — the same attack, one phase later, against
//! a timer that has already stopped.
//!
//! A flat cap is not the answer. Over elapsed time alone a 100 MiB upload on a
//! slow link and a dribbler look identical, so any cap generous enough for the
//! upload is generous enough for the attack, and any cap tight enough for the
//! attack breaks the upload. What separates them is not how long they take but
//! how much they send while taking it.
//!
//! So the deadline is **earned**:
//!
//! ```text
//! deadline = start + grace + received / min_rate
//! ```
//!
//! At the defaults (30s, 1 KiB/s) a 100 MiB upload has over a day, a 1 GiB one
//! over a week, and a peer sending a byte a minute is closed at ~30s having
//! earned about 10 milliseconds. The rate is a floor to beat, not a rate to
//! sustain: a fast upload finishes long before its own deadline matters, and a
//! genuinely slow client only has to do better than a hundredth of a dial-up
//! modem. This is the shape Apache's `mod_reqtimeout` uses, for the same reason.
//!
//! The wrapper is inert when constructed with `None`, and holds no timer then.

use std::pin::Pin;
use std::task::{Context, Poll, ready};
use std::time::Duration;

use es_runtime_common::ErrorCode;
use es_runtime_providers::{ByteStream, ProviderError};
use futures_core::Stream;
use tokio::time::{Instant, Sleep};

/// The bound: what a body starts with, and what arriving buys it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct BodyLimit {
    /// The allowance a body begins with.
    pub(crate) grace: Duration,
    /// Bytes per second that extend it. `0` ⇒ `grace` is a flat cap.
    pub(crate) min_rate: u64,
}

/// Wraps a body stream with a [`BodyLimit`].
pub(crate) struct BodyDeadline {
    inner: ByteStream,
    limit: BodyLimit,
    start: Instant,
    /// The current deadline, moved forward as bytes arrive.
    sleep: Pin<Box<Sleep>>,
    received: u64,
    /// Set once the deadline has fired, so the stream ends after its error
    /// rather than reporting the same failure to every later poll.
    expired: bool,
}

impl BodyDeadline {
    /// `body` bounded by `limit`, or `body` unchanged when there is no limit.
    pub(crate) fn wrap(body: ByteStream, limit: Option<BodyLimit>) -> ByteStream {
        let Some(limit) = limit else {
            return body;
        };
        let start = Instant::now();
        Box::pin(BodyDeadline {
            inner: body,
            limit,
            start,
            sleep: Box::pin(tokio::time::sleep_until(start + limit.grace)),
            received: 0,
            expired: false,
        })
    }

    /// The deadline `received` bytes have earned.
    ///
    /// Saturating throughout: a body large enough to overflow the arithmetic
    /// has earned more time than any deployment will wait, and the honest
    /// reading of that is "effectively unbounded", not "expired".
    fn earned(&self) -> Instant {
        if self.limit.min_rate == 0 {
            return self.start + self.limit.grace;
        }
        let seconds = self.received / self.limit.min_rate;
        self.start
            + self
                .limit
                .grace
                .saturating_add(Duration::from_secs(seconds.min(u64::from(u32::MAX))))
    }
}

impl Stream for BodyDeadline {
    type Item = Result<Vec<u8>, ProviderError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if this.expired {
            return Poll::Ready(None);
        }
        // The body first: a chunk that is ready to be taken is taken, even on
        // the poll where the deadline also happens to have passed. Refusing a
        // body already in hand would fail uploads that merely finished close to
        // the line, and the point of the deadline is the bytes that never came.
        match Pin::new(&mut this.inner).poll_next(cx) {
            Poll::Ready(Some(Ok(chunk))) => {
                this.received = this.received.saturating_add(chunk.len() as u64);
                let earned = this.earned();
                if earned > this.sleep.deadline() {
                    this.sleep.as_mut().reset(earned);
                }
                Poll::Ready(Some(Ok(chunk)))
            }
            Poll::Ready(other) => Poll::Ready(other),
            Poll::Pending => {
                ready!(this.sleep.as_mut().poll(cx));
                this.expired = true;
                Poll::Ready(Some(Err(ProviderError::Coded {
                    code: ErrorCode::TimedOut,
                    message: format!(
                        "the request body stalled: {} bytes in {:.1}s, under the {} bytes/s this \
                         server requires of a slow body",
                        this.received,
                        this.start.elapsed().as_secs_f64(),
                        this.limit.min_rate,
                    ),
                })))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;

    /// A stream that yields each chunk after the delay beside it, then ends.
    fn dribble(chunks: Vec<(Duration, usize)>) -> ByteStream {
        Box::pin(futures_util::stream::unfold(
            chunks.into_iter(),
            |mut it| async move {
                let (delay, size) = it.next()?;
                tokio::time::sleep(delay).await;
                Some((Ok(vec![0u8; size]), it))
            },
        ))
    }

    fn limit(grace_ms: u64, min_rate: u64) -> Option<BodyLimit> {
        Some(BodyLimit {
            grace: Duration::from_millis(grace_ms),
            min_rate,
        })
    }

    /// Drains a stream, returning how many bytes arrived and whether it failed.
    async fn drain(mut body: ByteStream) -> (usize, bool) {
        let mut bytes = 0;
        while let Some(item) = body.next().await {
            match item {
                Ok(chunk) => bytes += chunk.len(),
                Err(_) => return (bytes, true),
            }
        }
        (bytes, false)
    }

    #[tokio::test(start_paused = true)]
    async fn a_body_that_arrives_within_its_grace_is_untouched() {
        let body = dribble(vec![(Duration::from_millis(10), 4); 3]);
        let (bytes, failed) = drain(BodyDeadline::wrap(body, limit(1000, 1024))).await;
        assert_eq!((bytes, failed), (12, false));
    }

    #[tokio::test(start_paused = true)]
    async fn a_dribbler_is_cut_off_having_earned_almost_nothing() {
        // One byte a minute: the attack. Each byte earns 1/1024 of a second, so
        // the grace is what decides, and it decides quickly.
        let body = dribble(vec![(Duration::from_secs(60), 1); 100]);
        let (bytes, failed) = drain(BodyDeadline::wrap(body, limit(30_000, 1024))).await;
        assert!(failed, "a dribbler ran to completion");
        assert!(bytes <= 1, "{bytes} bytes got through a 30s grace");
    }

    #[tokio::test(start_paused = true)]
    async fn a_large_slow_upload_extends_its_own_deadline() {
        // 64 chunks of 64 KiB, one every 2s: 128s in total, well past a 30s
        // grace, and never once below the floor. This is the upload a flat cap
        // would have to break in order to stop the dribbler above.
        let body = dribble(vec![(Duration::from_secs(2), 64 * 1024); 64]);
        let (bytes, failed) = drain(BodyDeadline::wrap(body, limit(30_000, 1024))).await;
        assert_eq!((bytes, failed), (64 * 64 * 1024, false));
    }

    #[tokio::test(start_paused = true)]
    async fn a_zero_rate_makes_the_grace_a_flat_cap() {
        let body = dribble(vec![(Duration::from_secs(2), 64 * 1024); 64]);
        let (_, failed) = drain(BodyDeadline::wrap(body, limit(30_000, 0))).await;
        assert!(failed, "min_rate 0 should not extend anything");
    }

    #[tokio::test(start_paused = true)]
    async fn no_limit_leaves_the_slowest_body_alone() {
        let body = dribble(vec![(Duration::from_secs(600), 1); 3]);
        let (bytes, failed) = drain(BodyDeadline::wrap(body, None)).await;
        assert_eq!((bytes, failed), (3, false));
    }

    #[tokio::test(start_paused = true)]
    async fn the_failure_says_what_the_peer_did() {
        let body = dribble(vec![(Duration::from_secs(60), 1); 8]);
        let mut body = BodyDeadline::wrap(body, limit(100, 1024));
        let err = loop {
            match body.next().await {
                Some(Err(e)) => break e,
                Some(Ok(_)) => continue,
                None => panic!("ended without failing"),
            }
        };
        assert_eq!(err.code(), Some(ErrorCode::TimedOut));
        let message = err.to_string();
        assert!(message.contains("stalled"), "{message}");
        assert!(message.contains("bytes/s"), "{message}");
        // And the stream is over rather than repeating itself.
        assert!(body.next().await.is_none());
    }
}
