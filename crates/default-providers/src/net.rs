//! The default [`NetTransport`]: `fetch` over the outbound HTTP client
//! (DECISIONS.md D198).

use std::sync::{Arc, OnceLock};

use es_runtime_providers::{
    BoxFuture, HttpRequest, HttpResponse, NetTransport, ProviderError, RedirectMode, RequestBody,
};

use crate::HostAllowlist;
use crate::http_client::{Body, HttpClient, Outbound, Redirects};

/// A [`NetTransport`] over [`HttpClient`]: HTTP/1.1 and HTTP/2, pooled
/// connections, streamed bodies both ways.
///
/// Content-codings are negotiated and decoded by the client (Fetch's "decode"
/// step): `Accept-Encoding: gzip, br, deflate` goes out, and a response in any
/// of those codings arrives decoded with `Content-Encoding`/`Content-Length`
/// dropped from the headers, so they cannot describe bytes the guest never
/// sees. A coding the client does not implement (`zstd`, say) passes through
/// untouched, headers intact.
///
/// The client is built on the **first fetch**, not at construction: building
/// it assembles a TLS configuration, which costs startup time a script that
/// never fetches should not pay.
pub struct HttpTransport {
    client: OnceLock<HttpClient>,
    /// Addresses `fetch` may reach. `None` ⇒ any (`esrun --allow-net` without a
    /// list, or an embedder that scopes nothing).
    allow: Option<Arc<HostAllowlist>>,
    /// The process trust store (D197).
    trust: crate::TrustStore,
}

impl HttpTransport {
    /// Builds a transport. Infallible today (the client is built lazily); the
    /// `Result` is kept so a future eager validation can fail here.
    pub fn new() -> Result<Self, ProviderError> {
        Ok(HttpTransport {
            client: OnceLock::new(),
            allow: None,
            trust: crate::TrustStore::bundled(),
        })
    }

    /// Restricts `fetch` to `allow` — the provider-side half of
    /// `esrun --allow-net=<hosts>` (D38).
    ///
    /// **Every redirect hop is checked, not just the request the guest wrote.**
    /// Without that a `302` from an allowed host to a denied one would be
    /// followed transparently and the guest would receive its body. A refused
    /// hop fails the request rather than returning the redirect, since a
    /// program that asked to follow redirects has no way to interpret one.
    #[must_use]
    pub fn with_allowlist(mut self, allow: HostAllowlist) -> Self {
        self.allow = Some(Arc::new(allow));
        self
    }

    /// Verifies HTTPS servers against `trust` — the process store `--tls-ca`
    /// describes (D197) — instead of the bundled Mozilla roots.
    #[must_use]
    pub fn with_trust(mut self, trust: crate::TrustStore) -> Self {
        self.trust = trust;
        self
    }

    fn client(&self) -> HttpClient {
        self.client
            .get_or_init(|| HttpClient::new(&self.trust))
            .clone()
    }
}

impl NetTransport for HttpTransport {
    fn fetch(&self, request: HttpRequest) -> BoxFuture<Result<HttpResponse, ProviderError>> {
        let client = self.client();
        let allow = self.allow.clone();
        Box::pin(async move {
            // An unparseable URL never reaches here (the guest's `Request`
            // parsed it), but if it did there would be nothing to send.
            let url = url::Url::parse(&request.url)
                .map_err(|e| ProviderError::Other(format!("url: {e}")))?;
            let method = hyper::Method::from_bytes(request.method.as_bytes())
                .map_err(|e| ProviderError::Other(format!("method: {e}")))?;
            let response = client
                .send(Outbound {
                    method,
                    url,
                    headers: request.headers,
                    body: match request.body {
                        RequestBody::Empty => Body::Empty,
                        RequestBody::Bytes(bytes) => Body::Bytes(bytes.into()),
                        RequestBody::Stream(stream) => Body::Stream(stream),
                    },
                    redirects: match request.redirect {
                        RedirectMode::Follow => Redirects::Follow,
                        RedirectMode::Manual => Redirects::Manual,
                    },
                    allow,
                })
                .await?;
            let mut url = response.url;
            // Fetch serializes a response's URL without its fragment.
            url.set_fragment(None);
            let headers = response
                .headers
                .iter()
                .map(|(name, value)| {
                    (
                        name.as_str().to_string(),
                        value.to_str().unwrap_or("").to_string(),
                    )
                })
                .collect();
            let trailers = response.trailers;
            Ok(HttpResponse {
                status: response.status.as_u16(),
                status_text: response.status.canonical_reason().unwrap_or("").to_string(),
                url: url.to_string(),
                redirected: response.redirected,
                headers,
                body: response.body,
                // Resolves when the body ends: either with what the peer sent,
                // or empty once the sender is dropped with the stream — so a
                // body nobody reads settles this rather than hanging on it.
                trailers: Some(Box::pin(async move { trailers.await.unwrap_or_default() })),
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HostAllowlist;
    use es_runtime_common::ErrorCode;
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

    const OK: &[u8] = b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok";

    /// `fetch` over TLS to a server a private CA signed, verifying against
    /// `trust`. `Ok(status)` or the error code.
    async fn fetch_tls(
        trust: crate::TrustStore,
        leaf: crate::trust::test_pki::Leaf,
        host: &str,
    ) -> Result<u16, Option<ErrorCode>> {
        let (port, server) = crate::trust::test_pki::tls_server(leaf, &["http/1.1"], OK).await;
        let transport = HttpTransport::new().unwrap().with_trust(trust);
        let result = transport
            .fetch(request(&format!("https://{host}:{port}/")))
            .await;
        server.abort();
        result.map(|response| response.status).map_err(|e| e.code())
    }

    // D197: `fetch` verifies against the process store, never the operating
    // system's. A private CA the store does not hold is
    // refused; the same server verifies once the store holds it.
    #[tokio::test]
    async fn fetch_refuses_a_server_the_trust_store_does_not_hold() {
        let ca = crate::trust::test_pki::Ca::new("Private CA");
        let refused = fetch_tls(
            crate::TrustStore::bundled(),
            ca.leaf(&["localhost"]),
            "localhost",
        )
        .await;
        assert_eq!(refused, Err(Some(ErrorCode::Tls)));
        let trusted = fetch_tls(ca.trust(), ca.leaf(&["localhost"]), "localhost").await;
        assert_eq!(trusted, Ok(200));
    }

    // Trusting a CA is not trusting every name it signs for: the hostname
    // check still applies.
    #[tokio::test]
    async fn fetch_checks_the_hostname_against_a_trusted_ca() {
        let ca = crate::trust::test_pki::Ca::new("Private CA");
        let wrong_name = fetch_tls(ca.trust(), ca.leaf(&["other.test"]), "localhost").await;
        assert_eq!(wrong_name, Err(Some(ErrorCode::Tls)));
    }

    // A leaf from another CA is refused even when one CA is trusted: a store
    // `--tls-ca=<file>` built trusts that file and nothing else.
    #[tokio::test]
    async fn fetch_refuses_a_server_signed_by_another_ca() {
        let trusted = crate::trust::test_pki::Ca::new("Trusted CA");
        let other = crate::trust::test_pki::Ca::new("Other CA");
        let result = fetch_tls(trusted.trust(), other.leaf(&["localhost"]), "localhost").await;
        assert_eq!(result, Err(Some(ErrorCode::Tls)));
    }

    #[tokio::test]
    async fn fetch_refuses_an_expired_server_certificate() {
        let ca = crate::trust::test_pki::Ca::new("Private CA");
        let expired = fetch_tls(
            ca.trust(),
            ca.leaf_valid(&["localhost"], 2001, 2002),
            "localhost",
        )
        .await;
        assert_eq!(expired, Err(Some(ErrorCode::Tls)));
        let future = fetch_tls(
            ca.trust(),
            ca.leaf_valid(&["localhost"], 2080, 2090),
            "localhost",
        )
        .await;
        assert_eq!(future, Err(Some(ErrorCode::Tls)));
    }

    // The client's TLS config offers `h2`, so a server that speaks only h2
    // negotiates it.
    #[tokio::test]
    async fn fetch_still_offers_h2_over_alpn() {
        let ca = crate::trust::test_pki::Ca::new("Private CA");
        let (port, server) =
            crate::trust::test_pki::tls_server(ca.leaf(&["localhost"]), &["h2"], b"").await;
        let transport = HttpTransport::new().unwrap().with_trust(ca.trust());
        let _ = transport
            .fetch(request(&format!("https://localhost:{port}/")))
            .await;
        assert_eq!(server.await.unwrap(), Some(Some(b"h2".to_vec())));
    }

    /// A name that cannot resolve is `ERR_DNS`, not the catch-all `ERR_IO`.
    ///
    /// The classifier returns early on any io error whose kind is not `Other`,
    /// and a resolver failure carries a kind with no stable name — which maps
    /// to the catch-all `Io`. Returning *that* immediately meant the
    /// "lookup"/"dns" text check below it never ran, so every DNS failure was
    /// reported as a generic I/O error.
    ///
    /// `.invalid` is reserved by RFC 2606 precisely so it can never resolve, so
    /// this needs no network to be deterministic.
    #[tokio::test]
    async fn a_name_that_cannot_resolve_is_reported_as_dns() {
        let transport = HttpTransport::new().unwrap();
        let err = transport
            .fetch(request("http://no.such.host.invalid/"))
            .await
            .err()
            .expect("an unresolvable name must fail");
        assert_eq!(err.code(), Some(ErrorCode::Dns), "{err}");
    }

    /// …while a refusal from a host that *did* resolve keeps its own code, so
    /// the fallback did not swallow the more specific classifications.
    #[tokio::test]
    async fn a_refused_connection_is_still_connection_refused() {
        let transport = HttpTransport::new().unwrap();
        // Port 1 on loopback: resolves instantly, refuses instantly.
        let err = transport
            .fetch(request("http://127.0.0.1:1/"))
            .await
            .err()
            .expect("a refused connection must fail");
        assert_eq!(err.code(), Some(ErrorCode::ConnectionRefused), "{err}");
    }

    #[tokio::test]
    async fn fetch_refuses_a_host_outside_the_allowlist() {
        // Refused at the front door: no DNS lookup, no packet, nothing to
        // observe on the wire — which is why `evil.test` needing not to exist
        // is not a problem for this test.
        let transport = HttpTransport::new()
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
        let transport = HttpTransport::new()
            .unwrap()
            .with_allowlist(HostAllowlist::parse([format!("127.0.0.1:{port}")]).unwrap());
        let response = transport
            .fetch(request(&format!("http://127.0.0.1:{port}/")))
            .await
            .expect("the allowed host is reachable");
        assert_eq!(response.status, 200);
        server.await.unwrap();
    }

    /// The reason scoping `net` is more than a flag: a client follows redirects
    /// on its own, so a 302 from an allowed host to a denied one would
    /// otherwise be followed transparently and the guest handed the
    /// denied host's body — the allowlist enforced at the front door and
    /// nowhere else.
    #[tokio::test]
    async fn a_redirect_to_a_denied_host_is_refused_mid_chain() {
        let (port, server) = one_shot(
            "HTTP/1.1 302 Found\r\nLocation: http://evil.test/steal\r\nContent-Length: 0\r\n\r\n",
        )
        .await;
        let transport = HttpTransport::new()
            .unwrap()
            .with_allowlist(HostAllowlist::parse([format!("127.0.0.1:{port}")]).unwrap());
        let err = transport
            .fetch(request(&format!("http://127.0.0.1:{port}/")))
            .await
            .err()
            .expect("the redirect hop must be refused");
        // Reported as the scoped denial it is, not as an exhausted redirect
        // budget.
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

        let transport = HttpTransport::new().unwrap().with_allowlist(
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
        let transport = HttpTransport::new().unwrap();
        let response = transport
            .fetch(request(&format!("http://127.0.0.1:{port}/")))
            .await
            .expect("nothing is scoped");
        assert_eq!(response.status, 204);
        server.await.unwrap();
    }
}
