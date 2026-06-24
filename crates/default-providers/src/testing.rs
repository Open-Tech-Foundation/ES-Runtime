//! Deterministic providers for reproducible tests (DECISIONS.md D5).
//!
//! These are **not** for production: [`SeededEntropy`] is a plain PRNG, not a
//! CSPRNG, and [`ManualClock`]/[`ManualTimers`] advance only when told to. Their
//! purpose is reproducibility — the same inputs and the same seed/clock yield
//! byte-identical runs. [`ManualTimers`] advances a linked [`ManualClock`] on
//! each `sleep`, so a [`Driver`](crate::Driver) can run timer-driven code to
//! completion with no real waiting and no wall-clock flakiness.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use futures_util::{StreamExt, stream};

use es_runtime_providers::{
    BoxFuture, ByteStream, Clock, Console, ConsoleLevel, Entropy, HttpRequest, HttpResponse,
    NetTransport, ProviderError, RequestBody, TaskSpawner, Timers,
};

/// A [`Clock`] whose monotonic time advances only via [`advance`](Self::advance)
/// / [`set`](Self::set). Cheaply cloneable; clones share the same time.
#[derive(Clone)]
pub struct ManualClock {
    monotonic_ms: Arc<AtomicU64>,
    wall_ms: u64,
}

impl ManualClock {
    /// A clock starting at monotonic 0 with the given wall time.
    pub fn new(wall_ms: u64) -> Self {
        ManualClock {
            monotonic_ms: Arc::new(AtomicU64::new(0)),
            wall_ms,
        }
    }

    /// Advances monotonic time by `ms`.
    pub fn advance(&self, ms: u64) {
        self.monotonic_ms.fetch_add(ms, Ordering::SeqCst);
    }

    /// Sets monotonic time to `ms`.
    pub fn set(&self, ms: u64) {
        self.monotonic_ms.store(ms, Ordering::SeqCst);
    }
}

impl Default for ManualClock {
    fn default() -> Self {
        ManualClock::new(0)
    }
}

impl Clock for ManualClock {
    fn monotonic_ms(&self) -> u64 {
        self.monotonic_ms.load(Ordering::SeqCst)
    }

    fn wall_ms(&self) -> u64 {
        self.wall_ms
    }
}

/// A [`Timers`] provider that, instead of waiting, advances a linked
/// [`ManualClock`] by the requested delay and returns immediately. With a
/// [`Driver`](crate::Driver) this turns timer-driven code into a deterministic,
/// instant run.
pub struct ManualTimers {
    clock: ManualClock,
}

impl ManualTimers {
    /// Builds timers that drive `clock` forward as they are awaited.
    pub fn new(clock: ManualClock) -> Self {
        ManualTimers { clock }
    }
}

impl Timers for ManualTimers {
    fn sleep(&self, delay_ms: u64) -> BoxFuture<()> {
        self.clock.advance(delay_ms);
        Box::pin(std::future::ready(()))
    }
}

/// A deterministic, **non-cryptographic** [`Entropy`] source (seeded xorshift64).
/// For reproducible tests only — never production.
pub struct SeededEntropy {
    state: Mutex<u64>,
}

impl SeededEntropy {
    /// Seeds the generator. The seed is forced non-zero (xorshift requires it).
    pub fn new(seed: u64) -> Self {
        SeededEntropy {
            state: Mutex::new(seed | 1),
        }
    }
}

impl Entropy for SeededEntropy {
    fn fill(&self, dest: &mut [u8]) -> Result<(), ProviderError> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        for byte in dest.iter_mut() {
            let mut x = *state;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            *state = x;
            *byte = (x & 0xff) as u8;
        }
        Ok(())
    }
}

/// A [`Console`] that records every message in memory, so tests can assert on
/// guest output deterministically instead of poking a global subscriber.
#[derive(Default)]
pub struct CapturingConsole {
    messages: Mutex<Vec<(ConsoleLevel, String)>>,
}

impl CapturingConsole {
    /// An empty capturing console.
    pub fn new() -> Self {
        CapturingConsole::default()
    }

    /// A snapshot of every `(level, message)` captured so far.
    pub fn messages(&self) -> Vec<(ConsoleLevel, String)> {
        self.messages
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// A snapshot of just the message texts captured so far.
    pub fn lines(&self) -> Vec<String> {
        self.messages()
            .into_iter()
            .map(|(_, message)| message)
            .collect()
    }
}

impl Console for CapturingConsole {
    fn write(&self, level: ConsoleLevel, message: &str) {
        self.messages
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((level, message.to_string()));
    }
}

/// A canned HTTP response for [`MockTransport`].
#[derive(Clone)]
pub struct MockResponse {
    /// HTTP status code.
    pub status: u16,
    /// Status reason phrase.
    pub status_text: String,
    /// Response headers.
    pub headers: Vec<(String, String)>,
    /// Body delivered as these chunks (lets tests exercise streaming).
    pub chunks: Vec<Vec<u8>>,
}

impl MockResponse {
    /// A `200 OK` response with `body` as a single chunk.
    pub fn ok(body: impl Into<Vec<u8>>) -> Self {
        MockResponse {
            status: 200,
            status_text: "OK".to_string(),
            headers: Vec::new(),
            chunks: vec![body.into()],
        }
    }

    /// Adds a response header.
    #[must_use]
    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    /// Replaces the body with multiple chunks (to exercise streamed reads).
    #[must_use]
    pub fn chunks(mut self, chunks: Vec<Vec<u8>>) -> Self {
        self.chunks = chunks;
        self
    }
}

/// A deterministic [`NetTransport`] that answers from a handler closure — no
/// network. Tests assert on fetch behavior without connectivity.
pub struct MockTransport {
    handler: Arc<dyn Fn(HttpRequest) -> MockResponse + Send + Sync>,
}

impl MockTransport {
    /// Builds a transport whose `handler` decides the response per request.
    pub fn new(handler: impl Fn(HttpRequest) -> MockResponse + Send + Sync + 'static) -> Self {
        MockTransport {
            handler: Arc::new(handler),
        }
    }

    /// Builds a transport that always returns `response`.
    pub fn constant(response: MockResponse) -> Self {
        MockTransport::new(move |_req| response.clone())
    }
}

impl NetTransport for MockTransport {
    fn fetch(&self, mut request: HttpRequest) -> BoxFuture<Result<HttpResponse, ProviderError>> {
        // Drain a streaming request body to bytes first, so the handler always
        // sees a buffered `RequestBody::Bytes` and tests can assert the uploaded
        // content regardless of how the guest produced it.
        let handler = self.handler.clone();
        Box::pin(async move {
            if let RequestBody::Stream(mut stream) = request.body {
                let mut buf = Vec::new();
                while let Some(chunk) = stream.next().await {
                    buf.extend_from_slice(&chunk?);
                }
                request.body = RequestBody::Bytes(buf);
            }
            let url = request.url.clone();
            let response = (handler)(request);
            let chunks: Vec<Result<Vec<u8>, ProviderError>> =
                response.chunks.into_iter().map(Ok).collect();
            let body: ByteStream = Box::pin(stream::iter(chunks));
            Ok(HttpResponse {
                status: response.status,
                status_text: response.status_text,
                url,
                headers: response.headers,
                body,
            })
        })
    }
}

/// A [`TaskSpawner`] that runs work inline on the calling thread — no real
/// offloading, but deterministic and dependency-free.
pub struct InlineTaskSpawner;

impl TaskSpawner for InlineTaskSpawner {
    fn spawn_blocking(&self, work: Box<dyn FnOnce() + Send + 'static>) -> BoxFuture<()> {
        work();
        Box::pin(std::future::ready(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_clock_advances_only_on_demand() {
        let clock = ManualClock::new(1_000);
        assert_eq!(clock.monotonic_ms(), 0);
        assert_eq!(clock.wall_ms(), 1_000);
        clock.advance(25);
        assert_eq!(clock.monotonic_ms(), 25);
        clock.set(5);
        assert_eq!(clock.monotonic_ms(), 5);
    }

    #[test]
    fn manual_timers_advance_the_clock() {
        let clock = ManualClock::default();
        let timers = ManualTimers::new(clock.clone());
        // Awaiting a manual sleep is synchronous; poll it to completion.
        futures_block(timers.sleep(40));
        assert_eq!(clock.monotonic_ms(), 40);
    }

    #[test]
    fn seeded_entropy_is_reproducible_and_seed_sensitive() {
        let mut a = [0u8; 32];
        let mut b = [0u8; 32];
        SeededEntropy::new(1234).fill(&mut a).unwrap();
        SeededEntropy::new(1234).fill(&mut b).unwrap();
        assert_eq!(a, b, "same seed must reproduce");

        let mut c = [0u8; 32];
        SeededEntropy::new(5678).fill(&mut c).unwrap();
        assert_ne!(a, c, "different seed must differ");
    }

    #[test]
    fn inline_spawner_runs_immediately() {
        let ran = Arc::new(AtomicU64::new(0));
        let counter = ran.clone();
        futures_block(InlineTaskSpawner.spawn_blocking(Box::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        })));
        assert_eq!(ran.load(Ordering::SeqCst), 1);
    }

    /// Minimal synchronous executor for the ready futures these providers
    /// return, so the deterministic providers can be unit-tested without tokio.
    fn futures_block<T>(future: BoxFuture<T>) -> T {
        use std::task::{Context, Poll, Waker};
        let mut future = future;
        let waker = Waker::noop();
        let mut cx = Context::from_waker(waker);
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("deterministic provider future was not immediately ready"),
        }
    }
}
