//! tokio/reqwest-backed [`NetTransport`] (DECISIONS.md D20).

use std::sync::{Arc, OnceLock};

use es_runtime_common::ErrorCode;
use futures_util::StreamExt;

use es_runtime_providers::{
    BoxFuture, ByteStream, HttpRequest, HttpResponse, NetTransport, ProviderError, RedirectMode,
    RequestBody,
};

use crate::HostAllowlist;

/// The Fetch specification's redirect cap ("request's redirect count" is
/// compared against 20 in HTTP-redirect fetch).
const MAX_REDIRECTS: usize = 20;

/// The default `User-Agent`, matching `navigator.userAgent` so a server sees the
/// same identity the guest reports. A request that sets its own `user-agent`
/// header overrides this.
const USER_AGENT: &str = concat!("ES-Runtime/", env!("CARGO_PKG_VERSION"));

/// How long establishing the connection (DNS + TCP + TLS) may take before the
/// attempt fails with `ERR_TIMED_OUT`.
///
/// This deliberately bounds only the *connect* phase. A cap on the whole request
/// would be wrong here: Fetch defines no timeout, and a response body may be
/// long-lived by design — server-sent events, a log tail, a large download —
/// so a total deadline would break correct programs. Bounding the whole
/// operation is the caller's call, and Fetch already gives them the tool:
/// `fetch(url, { signal: AbortSignal.timeout(ms) })`. What is *never* the
/// caller's intent is waiting forever on a peer that never completes a
/// handshake, which is what an unbounded connect does.
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// TCP keepalive on pooled connections, so a peer that disappears without a FIN
/// is detected instead of leaving a dead socket to be handed to a later request.
const TCP_KEEPALIVE: std::time::Duration = std::time::Duration::from_secs(60);

/// A [`NetTransport`] backed by `reqwest` with rustls TLS (no OpenSSL). HTTP/1.1
/// and HTTP/2; response bodies stream.
///
/// Content-codings are negotiated and decoded here (Fetch's "decode" step):
/// `Accept-Encoding: gzip, br, deflate` goes out, and a response in any of those
/// codings arrives decoded with `Content-Encoding`/`Content-Length` dropped from
/// the headers, so they cannot describe bytes the guest never sees. Decoding
/// keys off the response's `Content-Encoding` rather than off who asked, so a
/// server that compresses unbidden is still handled. A coding this client does
/// not implement (`zstd`, say) passes through untouched, headers intact — the
/// honest answer, since pretending to have decoded it would be a lie.
///
/// The clients are built on the **first fetch** that needs one, not at
/// construction: client build-out (TLS config, root-store loading) costs startup
/// milliseconds that scripts which never fetch shouldn't pay. A build failure
/// surfaces as a [`ProviderError`] from that first fetch.
///
/// reqwest's redirect policy is a property of the client rather than of a
/// request, so the two [`RedirectMode`]s get one client each; a program that
/// only ever uses the default never pays to build the other.
pub struct ReqwestTransport {
    following: OnceLock<Result<reqwest::Client, String>>,
    manual: OnceLock<Result<reqwest::Client, String>>,
    /// Addresses `fetch` may reach. `None` ⇒ any (`esrun --allow-net` without a
    /// list, or an embedder that scopes nothing).
    allow: Option<Arc<HostAllowlist>>,
}

/// A redirect refused by the allowlist, boxed into reqwest's redirect error so
/// [`classify_reqwest`] can tell it from an exhausted redirect budget.
#[derive(Debug)]
struct DeniedRedirect(String);

impl std::fmt::Display for DeniedRedirect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for DeniedRedirect {}

impl ReqwestTransport {
    /// Builds a transport. Infallible today (the clients are built lazily); the
    /// `Result` is kept so a future eager validation can fail here.
    pub fn new() -> Result<Self, ProviderError> {
        Ok(ReqwestTransport {
            following: OnceLock::new(),
            manual: OnceLock::new(),
            allow: None,
        })
    }

    /// Restricts `fetch` to `allow` — the provider-side half of
    /// `esrun --allow-net=<hosts>` (D38).
    ///
    /// **Every redirect hop is checked, not just the request the guest wrote.**
    /// reqwest's redirect policy is a client-level property, so without this a
    /// `302` from an allowed host to a denied one would be followed
    /// transparently and the guest would receive its body — the allowlist
    /// enforced at the front door and nowhere else. A refused hop fails the
    /// request rather than returning the redirect, since a program that asked
    /// to follow redirects has no way to interpret one.
    #[must_use]
    pub fn with_allowlist(mut self, allow: HostAllowlist) -> Self {
        self.allow = Some(Arc::new(allow));
        self
    }

    /// The shared client for `mode`, built on first use (cheap to clone: an
    /// `Arc` inside).
    fn client(&self, mode: RedirectMode) -> Result<reqwest::Client, ProviderError> {
        let (cell, policy) = match mode {
            RedirectMode::Follow => (&self.following, self.follow_policy()),
            RedirectMode::Manual => (&self.manual, reqwest::redirect::Policy::none()),
        };
        cell.get_or_init(|| {
            reqwest::Client::builder()
                .redirect(policy)
                .user_agent(USER_AGENT)
                .connect_timeout(CONNECT_TIMEOUT)
                .tcp_keepalive(TCP_KEEPALIVE)
                .build()
                .map_err(|e| format!("http client: {e}"))
        })
        .clone()
        .map_err(ProviderError::Other)
    }

    /// The redirect policy for [`RedirectMode::Follow`]: the Fetch cap alone
    /// when nothing is scoped, and the cap plus a per-hop allowlist check when
    /// it is.
    fn follow_policy(&self) -> reqwest::redirect::Policy {
        let Some(allow) = self.allow.clone() else {
            return reqwest::redirect::Policy::limited(MAX_REDIRECTS);
        };
        reqwest::redirect::Policy::custom(move |attempt| {
            if attempt.previous().len() >= MAX_REDIRECTS {
                return attempt.error(TooManyRedirects);
            }
            match allow.check_url(attempt.url(), "fetch redirect") {
                Ok(()) => attempt.follow(),
                Err(e) => attempt.error(DeniedRedirect(e.to_string())),
            }
        })
    }
}

/// The redirect budget, as an error type, so the custom policy above reports
/// exhaustion exactly as `Policy::limited` would.
#[derive(Debug)]
struct TooManyRedirects;

impl std::fmt::Display for TooManyRedirects {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "too many redirects (more than {MAX_REDIRECTS})")
    }
}

impl std::error::Error for TooManyRedirects {}

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
    // A redirect the allowlist refused. Checked before the generic redirect
    // arm below: both arrive as `is_redirect()`, and "the chain left the
    // addresses you allowed" is a different fact from "the chain was too long".
    let mut hop: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(&e);
    while let Some(err) = hop {
        if let Some(denied) = err.downcast_ref::<DeniedRedirect>() {
            return ProviderError::Coded {
                code: ErrorCode::PermissionDenied,
                message: denied.0.clone(),
            };
        }
        hop = std::error::Error::source(err);
    }
    // The redirect policy gave up: the chain exceeded MAX_REDIRECTS.
    if e.is_redirect() {
        return ProviderError::Coded {
            code: ErrorCode::TooManyRedirects,
            message: format!("too many redirects (more than {MAX_REDIRECTS})"),
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
        let client = self.client(request.redirect);
        // The request the guest wrote. Checked here, before the connection, so
        // a denial costs no DNS lookup and leaks no traffic; the redirect
        // policy covers every hop after it.
        let front_door = self.allow.clone().map(|allow| {
            let url = url::Url::parse(&request.url);
            match url {
                Ok(url) => allow.check_url(&url, "fetch"),
                // An unparseable URL never reaches here (the guest's `Request`
                // parsed it), but if it did there would be no host to judge.
                Err(e) => Err(ProviderError::Other(format!("url: {e}"))),
            }
        });
        Box::pin(async move {
            front_door.transpose()?;
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
            // reqwest reports the *final* URL, so a redirect shows up as a URL
            // that differs from the one asked for. Both sides are compared as
            // parsed `Url`s rather than strings, so reqwest's own normalization
            // (`http://h` → `http://h/`) is not mistaken for a redirect.
            let redirected = reqwest::Url::parse(&request.url)
                .is_ok_and(|requested| &requested != response.url());
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
                redirected,
                headers,
                body,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HostAllowlist;
    use es_runtime_providers::RedirectMode;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn request(url: &str) -> HttpRequest {
        HttpRequest {
            method: "GET".to_string(),
            url: url.to_string(),
            headers: vec![],
            body: RequestBody::Empty,
            redirect: RedirectMode::Follow,
        }
    }

    /// A one-shot HTTP server that answers with `response` and then closes.
    /// Returns its port and the task, so a test can await the handoff.
    async fn one_shot(response: &'static str) -> (u16, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 1024];
            let _ = sock.read(&mut buf).await;
            let _ = sock.write_all(response.as_bytes()).await;
            let _ = sock.flush().await;
        });
        (port, task)
    }

    #[tokio::test]
    async fn fetch_refuses_a_host_outside_the_allowlist() {
        // Refused at the front door: no DNS lookup, no packet, nothing to
        // observe on the wire — which is why `evil.test` needing not to exist
        // is not a problem for this test.
        let transport = ReqwestTransport::new()
            .unwrap()
            .with_allowlist(HostAllowlist::parse(["api.example.com"]).unwrap());
        let err = transport
            .fetch(request("https://evil.test/steal"))
            .await
            .err()
            .expect("a host outside the list must be refused");
        assert_eq!(err.code(), Some(ErrorCode::PermissionDenied));
        assert!(err.to_string().contains("evil.test:443"), "{err}");
    }

    #[tokio::test]
    async fn fetch_reaches_a_host_on_the_allowlist() {
        let (port, server) = one_shot("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhi").await;
        let transport = ReqwestTransport::new()
            .unwrap()
            .with_allowlist(HostAllowlist::parse([format!("127.0.0.1:{port}")]).unwrap());
        let response = transport
            .fetch(request(&format!("http://127.0.0.1:{port}/")))
            .await
            .expect("the allowed host is reachable");
        assert_eq!(response.status, 200);
        server.await.unwrap();
    }

    /// The reason scoping `net` is more than a flag: reqwest follows redirects
    /// on a **client-level** policy, so a 302 from an allowed host to a denied
    /// one would otherwise be followed transparently and the guest handed the
    /// denied host's body — the allowlist enforced at the front door and
    /// nowhere else.
    #[tokio::test]
    async fn a_redirect_to_a_denied_host_is_refused_mid_chain() {
        let (port, server) = one_shot(
            "HTTP/1.1 302 Found\r\nLocation: http://evil.test/steal\r\nContent-Length: 0\r\n\r\n",
        )
        .await;
        let transport = ReqwestTransport::new()
            .unwrap()
            .with_allowlist(HostAllowlist::parse([format!("127.0.0.1:{port}")]).unwrap());
        let err = transport
            .fetch(request(&format!("http://127.0.0.1:{port}/")))
            .await
            .err()
            .expect("the redirect hop must be refused");
        // Reported as the scoped denial it is, not as an exhausted redirect
        // budget: both arrive from reqwest as redirect errors.
        assert_eq!(err.code(), Some(ErrorCode::PermissionDenied));
        assert!(err.to_string().contains("evil.test"), "{err}");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn a_redirect_within_the_allowlist_is_followed() {
        let (target, target_task) =
            one_shot("HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\ndone").await;
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let hop = listener.local_addr().unwrap().port();
        let hop_task = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 1024];
            let _ = sock.read(&mut buf).await;
            let response = format!(
                "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{target}/\r\nContent-Length: 0\r\n\r\n"
            );
            let _ = sock.write_all(response.as_bytes()).await;
            let _ = sock.flush().await;
        });

        let transport = ReqwestTransport::new().unwrap().with_allowlist(
            HostAllowlist::parse([format!("127.0.0.1:{hop}"), format!("127.0.0.1:{target}")])
                .unwrap(),
        );
        let response = transport
            .fetch(request(&format!("http://127.0.0.1:{hop}/")))
            .await
            .expect("both hops are allowed");
        assert_eq!(response.status, 200);
        assert!(response.redirected);
        hop_task.await.unwrap();
        target_task.await.unwrap();
    }

    #[tokio::test]
    async fn an_unscoped_transport_reaches_anything() {
        // The default is unchanged: no allowlist, no check.
        let (port, server) = one_shot("HTTP/1.1 204 No Content\r\n\r\n").await;
        let transport = ReqwestTransport::new().unwrap();
        let response = transport
            .fetch(request(&format!("http://127.0.0.1:{port}/")))
            .await
            .expect("nothing is scoped");
        assert_eq!(response.status, 204);
        server.await.unwrap();
    }
}
