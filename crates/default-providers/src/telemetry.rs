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

use crate::client::{Body, HttpClient, Outbound};

/// How long one export may take, connect to last byte. An export is
/// best-effort, and a sink that waits long on a wedged collector holds a
/// connection and a task for telemetry nobody is reading.
const EXPORT_TIMEOUT: Duration = Duration::from_secs(10);

/// Posts OTLP/JSON to an `OTEL_EXPORTER_OTLP_ENDPOINT`-style base URL.
pub struct OtlpHttpSink {
    client: HttpClient,
    /// The outbound proxy (`--proxy`, D205), kept so `with_trust` keeps it.
    proxies: Option<Arc<crate::Proxies>>,
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
            client: HttpClient::new(&crate::TrustStore::bundled()),
            proxies: None,
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
        self.client = HttpClient::new(trust).with_proxy(self.proxies.clone());
        self
    }

    /// Exports through `proxies` (D205), as `fetch` does.
    #[must_use]
    pub fn with_proxy(mut self, proxies: Option<Arc<crate::Proxies>>) -> Self {
        self.proxies = proxies.clone();
        self.client = self.client.with_proxy(proxies);
        self
    }
}

/// Posts `payload` to `url`, reading the answer to its end so the connection
/// goes back to the pool. `Err` describes what went wrong, without the body.
async fn post(client: &HttpClient, url: &str, payload: String) -> Result<(), String> {
    let parsed = url::Url::parse(url).map_err(|e| format!("{url}: {e}"))?;
    let mut request = Outbound::get(parsed);
    request.method = crate::client::types::Method::POST;
    request.headers = vec![("content-type".to_string(), "application/json".to_string())];
    request.body = Body::Bytes(payload.into());
    let response = client
        .send(request)
        .await
        .map_err(|e| format!("{url}: {e}"))?;
    let status = response.status;
    let _ = response.bytes(1024 * 1024).await;
    if status.is_success() {
        Ok(())
    } else {
        Err(format!("{url} returned {status}"))
    }
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
            let failure =
                match tokio::time::timeout(EXPORT_TIMEOUT, post(&client, &url, payload)).await {
                    Ok(Ok(())) => None,
                    Ok(Err(failure)) => Some(failure),
                    Err(_) => Some(format!(
                        "{url}: no answer within {}s",
                        EXPORT_TIMEOUT.as_secs()
                    )),
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
    use super::*;
    use crate::trust::test_pki::{Ca, tls_server};

    const OK: &[u8] = b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";

    /// Exports one payload to a TLS collector `ca` signed, through a sink
    /// trusting `trust`, and reports whether the collector's handshake
    /// completed.
    async fn export_to(ca: &Ca, trust: &crate::TrustStore) -> bool {
        let (port, server) = tls_server(ca.leaf(&["localhost"]), &["http/1.1"], OK).await;
        let sink = OtlpHttpSink::new(format!("https://localhost:{port}")).with_trust(trust);
        let _ = sink.export("traces", "{}".to_string()).await;
        sink.flush().await;
        server.await.unwrap().is_some()
    }

    // D197: an `https` collector is verified against the process store, like
    // every other TLS client.
    #[tokio::test]
    async fn an_https_collector_is_verified_against_the_trust_store() {
        let ca = Ca::new("Collector CA");
        assert!(
            export_to(&ca, &ca.trust()).await,
            "a trusted collector was refused"
        );
        assert!(
            !export_to(&ca, &crate::TrustStore::bundled()).await,
            "an untrusted collector was accepted"
        );
    }
}
