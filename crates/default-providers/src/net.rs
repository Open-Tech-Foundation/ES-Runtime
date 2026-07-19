//! tokio/reqwest-backed [`NetTransport`] (DECISIONS.md D20).

use std::sync::OnceLock;

use es_runtime_common::ErrorCode;
use futures_util::StreamExt;

use es_runtime_providers::{
    BoxFuture, ByteStream, HttpRequest, HttpResponse, NetTransport, ProviderError, RequestBody,
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

/// Classifies a reqwest failure with a stable guest-facing code where one can
/// be derived (SPEC §6 Phase 13): a source-chain io error carries its kind; a
/// reqwest timeout maps to `ERR_TIMED_OUT`; TLS/certificate failures to
/// `ERR_TLS`; name-resolution failures to `ERR_DNS`. Anything else stays an
/// uncoded provider error.
fn classify_reqwest(e: reqwest::Error) -> ProviderError {
    let message = format!("request failed: {e}");
    if e.is_timeout() {
        return ProviderError::Coded {
            code: ErrorCode::TimedOut,
            message,
        };
    }
    // Walk the source chain for the underlying io error / failure text.
    let mut source: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(&e);
    let mut chain_text = String::new();
    while let Some(err) = source {
        if let Some(io) = err.downcast_ref::<std::io::Error>()
            && io.kind() != std::io::ErrorKind::Other
        {
            return ProviderError::Coded {
                code: ErrorCode::from_io_kind(io.kind()),
                message,
            };
        }
        chain_text.push_str(&err.to_string());
        chain_text.push(' ');
        source = std::error::Error::source(err);
    }
    let lower = chain_text.to_lowercase();
    let code = if lower.contains("certificate") || lower.contains("tls") || lower.contains("ssl") {
        Some(ErrorCode::Tls)
    } else if lower.contains("lookup") || lower.contains("dns") {
        Some(ErrorCode::Dns)
    } else if lower.contains("refused") {
        Some(ErrorCode::ConnectionRefused)
    } else if lower.contains("reset") || lower.contains("broken pipe") {
        Some(ErrorCode::ConnectionReset)
    } else {
        None
    };
    match code {
        Some(code) => ProviderError::Coded { code, message },
        None => ProviderError::Other(message),
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
            match request.body {
                RequestBody::Empty => {}
                RequestBody::Bytes(body) => {
                    builder = builder.body(body);
                }
                RequestBody::Stream(stream) => {
                    // Chunked transfer-encoding: reqwest pulls from the stream as
                    // the connection drains, so the upload never fully buffers.
                    // `Bytes: From<Vec<u8>>`, so only the error needs adapting.
                    let body = stream.map(|chunk| chunk.map_err(std::io::Error::other));
                    builder = builder.body(reqwest::Body::wrap_stream(body));
                }
            }

            let response = builder.send().await.map_err(classify_reqwest)?;

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
