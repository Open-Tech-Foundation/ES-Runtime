//! OTLP/HTTP [`TelemetrySink`] — ships encoded payloads to a collector
//! (DECISIONS.md D89).
//!
//! Transport and nothing else. The runtime encodes the OTLP, because what a span
//! means is runtime knowledge; this knows a URL, a header and how to give up.
//!
//! **Failure is absorbed here.** Telemetry that cannot be delivered must never
//! fail the program that produced it, so a refused connection, a 500 or a
//! timeout is logged once at `warn` and the payload is dropped. A deployment
//! whose collector is down gets a server that keeps serving.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use tokio::sync::Notify;

use es_runtime_providers::{BoxFuture, TelemetrySink};

/// Posts OTLP/JSON to an `OTEL_EXPORTER_OTLP_ENDPOINT`-style base URL.
pub struct OtlpHttpSink {
    client: reqwest::Client,
    endpoint: String,
    /// Latches the first delivery failure so a collector that is down produces
    /// one line in the log rather than one per turn.
    warned: Arc<AtomicBool>,
    /// Exports handed over but not yet finished, and a wake-up for whoever is
    /// waiting on them. Together they are what makes [`flush`](TelemetrySink::flush)
    /// possible without the export path ever blocking the loop.
    in_flight: Arc<AtomicUsize>,
    idle: Arc<Notify>,
}

impl OtlpHttpSink {
    /// Builds a sink posting to `endpoint` (the base, e.g.
    /// `http://127.0.0.1:4318`); the signal path is appended per export.
    ///
    /// The timeout is short on purpose: an export is best-effort, and a sink
    /// that waits a long time on a wedged collector holds a connection and a
    /// task for telemetry nobody is reading.
    pub fn new(endpoint: impl Into<String>) -> Self {
        let endpoint = endpoint.into();
        OtlpHttpSink {
            client: client(&crate::TrustStore::bundled()),
            endpoint: endpoint.trim_end_matches('/').to_string(),
            warned: Arc::new(AtomicBool::new(false)),
            in_flight: Arc::new(AtomicUsize::new(0)),
            idle: Arc::new(Notify::new()),
        }
    }
}

impl OtlpHttpSink {
    /// Verifies an `https` collector against `trust` — the process store
    /// `--tls-ca` describes (D197).
    #[must_use]
    pub fn with_trust(mut self, trust: &crate::TrustStore) -> Self {
        self.client = client(trust);
        self
    }
}

/// The exporter's HTTP client, verifying against `trust`.
fn client(trust: &crate::TrustStore) -> reqwest::Client {
    reqwest::Client::builder()
        // Not through a proxy the environment names, for the same reason
        // `fetch` is not: spans go to the `--otel` collector.
        .no_proxy()
        .tls_backend_preconfigured(trust.client_config(&["h2", "http/1.1"]))
        .timeout(Duration::from_secs(10))
        .build()
        // Never `unwrap_or_default`: reqwest's default client verifies against
        // the operating system's store, so falling back would quietly change
        // whom the exporter trusts. The build only fails if reqwest and
        // tokio-rustls stop sharing one rustls, which the test below catches.
        .expect("the exporter's HTTP client builds with the process trust store")
}

impl TelemetrySink for OtlpHttpSink {
    fn export(&self, signal: &'static str, payload: String) -> BoxFuture<()> {
        let url = format!("{}/v1/{signal}", self.endpoint);
        let client = self.client.clone();
        let warned = self.warned.clone();
        let in_flight = self.in_flight.clone();
        let idle = self.idle.clone();
        in_flight.fetch_add(1, Ordering::SeqCst);
        // Spawned rather than awaited by the caller: the runtime's loop hands
        // this over and moves on, so a slow collector never adds latency to the
        // request whose span is being exported.
        tokio::spawn(async move {
            let sent = client
                .post(&url)
                .header("content-type", "application/json")
                .body(payload)
                .send()
                .await;
            let failure = match sent {
                Ok(response) if response.status().is_success() => None,
                Ok(response) => Some(format!("{} returned {}", url, response.status())),
                Err(error) => Some(format!("{url}: {error}")),
            };
            if let Some(failure) = failure
                && !warned.swap(true, Ordering::Relaxed)
            {
                tracing::warn!(
                    "telemetry export failed and will be dropped silently from here: {failure}"
                );
            }
            if in_flight.fetch_sub(1, Ordering::SeqCst) == 1 {
                idle.notify_waiters();
            }
        });
        Box::pin(std::future::ready(()))
    }

    fn flush(&self) -> BoxFuture<()> {
        let in_flight = self.in_flight.clone();
        let idle = self.idle.clone();
        Box::pin(async move {
            // Bounded: a collector that has stopped answering must not stop the
            // process from exiting. The client's own timeout is the real bound;
            // this is the backstop for a task that never gets scheduled.
            let _ = tokio::time::timeout(Duration::from_secs(15), async {
                while in_flight.load(Ordering::SeqCst) > 0 {
                    let waiting = idle.notified();
                    if in_flight.load(Ordering::SeqCst) == 0 {
                        break;
                    }
                    waiting.await;
                }
            })
            .await;
        })
    }
}

#[cfg(test)]
mod tests {
    // The exporter's client is built with the process store; a failure would
    // panic at startup rather than fall back to the OS store, so make sure it
    // never does.
    #[test]
    fn the_exporter_client_builds_with_any_trust_store() {
        let _ = super::OtlpHttpSink::new("http://127.0.0.1:4318")
            .with_trust(&crate::TrustStore::bundled());
        let _ = super::OtlpHttpSink::new("http://127.0.0.1:4318")
            .with_trust(&crate::trust::test_pki::Ca::new("Collector CA").trust());
    }
}
