//! tokio/reqwest-backed [`NetTransport`] (DECISIONS.md D20).

use std::sync::OnceLock;

use futures_util::StreamExt;

use es_runtime_providers::{
    BoxFuture, ByteStream, HttpRequest, HttpResponse, NetTransport, ProviderError,
};

/// A [`NetTransport`] backed by `reqwest` with rustls TLS (no OpenSSL). HTTP/1.1
/// and HTTP/2; response bodies stream.
///
/// The client is built on the **first fetch**, not at construction: client
/// build-out (TLS config, root-store loading) costs startup milliseconds that
/// scripts which never fetch shouldn't pay. A build failure surfaces as a
/// [`ProviderError`] from that first fetch.
pub struct ReqwestTransport {
    client: OnceLock<Result<reqwest::Client, String>>,
}

impl ReqwestTransport {
    /// Builds a transport. Infallible today (the client is built lazily); the
    /// `Result` is kept so a future eager validation can fail here.
    pub fn new() -> Result<Self, ProviderError> {
        Ok(ReqwestTransport {
            client: OnceLock::new(),
        })
    }

    /// The shared client, built on first use (cheap to clone: an `Arc` inside).
    fn client(&self) -> Result<reqwest::Client, ProviderError> {
        self.client
            .get_or_init(|| {
                reqwest::Client::builder()
                    .build()
                    .map_err(|e| format!("http client: {e}"))
            })
            .clone()
            .map_err(ProviderError::Other)
    }
}

impl NetTransport for ReqwestTransport {
    fn fetch(&self, request: HttpRequest) -> BoxFuture<Result<HttpResponse, ProviderError>> {
        let client = self.client();
        Box::pin(async move {
            let client = client?;
            let method = reqwest::Method::from_bytes(request.method.as_bytes())
                .map_err(|e| ProviderError::Other(format!("method: {e}")))?;
            let mut builder = client.request(method, &request.url);
            for (name, value) in &request.headers {
                builder = builder.header(name, value);
            }
            if let Some(body) = request.body {
                builder = builder.body(body);
            }

            let response = builder
                .send()
                .await
                .map_err(|e| ProviderError::Other(format!("request failed: {e}")))?;

            let status = response.status().as_u16();
            let status_text = response
                .status()
                .canonical_reason()
                .unwrap_or("")
                .to_string();
            let url = response.url().to_string();
            let headers = response
                .headers()
                .iter()
                .map(|(name, value)| {
                    (
                        name.as_str().to_string(),
                        value.to_str().unwrap_or("").to_string(),
                    )
                })
                .collect();

            let body: ByteStream = Box::pin(response.bytes_stream().map(|chunk| {
                chunk
                    .map(|bytes| bytes.to_vec())
                    .map_err(|e| ProviderError::Other(format!("body: {e}")))
            }));

            Ok(HttpResponse {
                status,
                status_text,
                url,
                headers,
                body,
            })
        })
    }
}
