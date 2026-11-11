//! The outbound HTTP client (DECISIONS.md D198): every HTTP request either
//! binary makes — `fetch`, the telemetry exporter, `upgrade`, `esdev create`'s
//! registry lookups — goes through [`HttpClient`].
//!
//! The policy is here and the wire is in [`crate::h1_client`]: HTTP/1.1 and
//! the connection pool are ES-Runtime's own (D217), HTTP/2 is hyper's until
//! D217's phase 3, and DNS, TCP and Happy Eyeballs are [`crate::tcp_connect`].
//! TLS is tokio-rustls over the process trust store (D197).
//! What is written here is what a client decides rather than parses: which
//! redirects to follow and what to strip on the way, which content-codings to
//! decode, and how long a connection may take.

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use es_runtime_common::ErrorCode;
use es_runtime_providers::{ByteStream, ProviderError};
use futures_util::{Stream, StreamExt, TryStreamExt};
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt, Empty, Full, StreamBody};
use hyper::body::{Body as _, Frame};
use hyper::header::{self, HeaderMap, HeaderName, HeaderValue};
use hyper::{Method, StatusCode};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use tokio_rustls::rustls::pki_types::ServerName;
use url::Url;

use crate::h1_client::{Exchange, Origin, Pool, ResponseStream};
use crate::proxy::{BoxWire, Proxies, ProxyError};
use crate::{HostAllowlist, TrustStore};

/// The Fetch specification's redirect cap ("request's redirect count" is
/// compared against 20 in HTTP-redirect fetch).
pub const MAX_REDIRECTS: usize = 20;

/// The default `User-Agent`, matching `navigator.userAgent` so a server sees
/// the identity the guest reports. A request that sets its own overrides it.
pub const USER_AGENT: &str = concat!("ES-Runtime/", env!("CARGO_PKG_VERSION"));

/// How long establishing a connection — DNS, TCP and TLS together — may take
/// before it fails with `ERR_TIMED_OUT`.
///
/// Only the connect phase is bounded. Fetch defines no timeout, and a response
/// body may be long-lived by design (server-sent events, a log tail, a large
/// download); bounding the whole operation is the caller's call, through
/// `AbortSignal.timeout`. Waiting forever on a peer that never completes a
/// handshake is never anyone's intent.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// TCP keepalive on pooled connections, so a peer that vanished without a FIN
/// is noticed instead of leaving a dead socket to be handed to a later request.
const TCP_KEEPALIVE: Duration = Duration::from_secs(60);

/// The most a decoder hands over at once, so a highly compressible body is
/// expanded a bounded piece at a time rather than all at once.
const DECODED_CHUNK: usize = 64 * 1024;

/// A request body as the client sends it.
pub type RequestBody = UnsyncBoxBody<Bytes, Box<dyn std::error::Error + Send + Sync>>;

/// What a request carries, before the client adds its own headers.
pub enum Body {
    /// No body.
    Empty,
    /// A buffered body: sent with `Content-Length`, and resent when a `307` or
    /// `308` asks for it.
    Bytes(Bytes),
    /// A streamed body: sent chunked, and never resent — a redirect that would
    /// need it again is an error.
    Stream(ByteStream),
}

/// Whether redirects are followed or handed back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Redirects {
    /// Followed, up to [`MAX_REDIRECTS`].
    Follow,
    /// The redirect response is returned unfollowed.
    Manual,
}

/// One request through [`HttpClient::send`].
pub struct Outbound {
    /// The request method.
    pub method: Method,
    /// The absolute `http:` or `https:` URL.
    pub url: Url,
    /// Header name/value pairs, in order. Repeated names are all sent.
    pub headers: Vec<(String, String)>,
    /// The request body.
    pub body: Body,
    /// Whether redirects are followed.
    pub redirects: Redirects,
    /// Checked for the request and for every redirect hop before it is sent.
    pub allow: Option<Arc<HostAllowlist>>,
    /// Whether a content-coded response body is decoded (D210). `false` asks
    /// for the identity coding unless the request names its own, and returns
    /// the body and its headers as the server sent them.
    pub decompress: bool,
}

impl Outbound {
    /// A `GET` of `url` that follows redirects.
    pub fn get(url: Url) -> Self {
        Outbound {
            method: Method::GET,
            url,
            headers: Vec::new(),
            body: Body::Empty,
            redirects: Redirects::Follow,
            allow: None,
            decompress: true,
        }
    }
}

/// A response, headers in hand and the body still to come.
pub struct Inbound {
    /// The response status.
    pub status: StatusCode,
    /// The HTTP version the response arrived over.
    pub version: hyper::Version,
    /// The URL this response came from, after any redirects, fragment kept
    /// as Fetch's response URL list does not (the caller decides).
    pub url: Url,
    /// Whether at least one redirect was followed.
    pub redirected: bool,
    /// The response headers, minus `Content-Encoding` and `Content-Length`
    /// when the body was decoded.
    pub headers: HeaderMap,
    /// The body, decoded when its coding is one this client implements.
    pub body: ByteStream,
    /// Resolves to the trailer block once the body has ended — empty when
    /// there was none, or when the body was dropped unread.
    pub trailers: tokio::sync::oneshot::Receiver<Vec<(String, String)>>,
}

impl Inbound {
    /// The whole body, refusing one longer than `limit` bytes.
    pub async fn bytes(mut self, limit: usize) -> Result<Vec<u8>, ProviderError> {
        let mut all = Vec::new();
        while let Some(chunk) = self.body.next().await {
            let chunk = chunk?;
            if all.len() + chunk.len() > limit {
                return Err(ProviderError::Other(format!(
                    "the response is larger than {limit} bytes"
                )));
            }
            all.extend_from_slice(&chunk);
        }
        Ok(all)
    }
}

/// The outbound HTTP client. Cheap to clone: the pool is shared.
#[derive(Clone)]
pub struct HttpClient {
    connector: Connector,
    pool: Pool,
    /// What [`with_proxy`](Self::with_proxy) rebuilds the pool from.
    config: Arc<tokio_rustls::rustls::ClientConfig>,
    proxy_tls: TlsConnector,
    timeout: Duration,
    /// The outbound proxy (D205), consulted per target.
    proxies: Option<Arc<Proxies>>,
}

impl std::fmt::Debug for HttpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HttpClient")
    }
}

impl HttpClient {
    /// A client verifying HTTPS servers against `trust`.
    pub fn new(trust: &TrustStore) -> Self {
        Self::with_connect_timeout(trust, CONNECT_TIMEOUT)
    }

    /// A client for requests that bring their own TLS material (D201):
    /// `ca` trusted in addition to `trust`, and `identity` presented. Its pool
    /// is its own, so a connection that presented a client certificate is
    /// never reused for a request that did not ask for one.
    pub fn with_tls(
        trust: &TrustStore,
        ca: &[u8],
        identity: &es_runtime_providers::ClientIdentity,
    ) -> Result<Self, ProviderError> {
        let config = trust.client_config_for(ca, identity, &["h2", "http/1.1"])?;
        Ok(Self::from_config(
            Arc::new(config),
            proxy_tls(trust),
            CONNECT_TIMEOUT,
            None,
        ))
    }

    /// As [`new`](Self::new), with a different connect deadline.
    pub fn with_connect_timeout(trust: &TrustStore, timeout: Duration) -> Self {
        Self::from_config(
            Arc::new(trust.client_config(&["h2", "http/1.1"])),
            proxy_tls(trust),
            timeout,
            None,
        )
    }

    /// This client, sending through `proxies` (D205): a `http:` target is
    /// forwarded to the proxy, an `https:` one tunnelled with `CONNECT`. A
    /// fresh pool, since a pooled connection is direct or proxied for good.
    #[must_use]
    pub fn with_proxy(self, proxies: Option<Arc<Proxies>>) -> Self {
        Self::from_config(self.config, self.proxy_tls, self.timeout, proxies)
    }

    fn from_config(
        config: Arc<tokio_rustls::rustls::ClientConfig>,
        proxy_tls: TlsConnector,
        timeout: Duration,
        proxies: Option<Arc<Proxies>>,
    ) -> Self {
        let tls = TlsConnector::from(config.clone());
        let connector = Connector {
            tls,
            timeout,
            proxies: proxies.clone(),
            proxy_tls: proxy_tls.clone(),
        };
        HttpClient {
            connector,
            pool: Pool::default(),
            config,
            proxy_tls,
            timeout,
            proxies,
        }
    }

    /// The `Proxy-Authorization` a forwarded request to `url` carries: set
    /// when `url` is `http:`, goes through a proxy, and the proxy URL had
    /// credentials. A tunnelled request carries it on the `CONNECT` instead.
    fn forward_authorization(&self, url: &Url) -> Option<HeaderValue> {
        let proxies = self.proxies.as_ref()?;
        if url.scheme() != "http" {
            return None;
        }
        let port = url.port_or_known_default()?;
        let server = proxies.for_target("http", url.host_str()?, port)?;
        HeaderValue::from_str(server.authorization()?).ok()
    }

    /// Sends `request`, following redirects as it asks, and resolves once the
    /// final response's headers arrive.
    pub async fn send(&self, request: Outbound) -> Result<Inbound, ProviderError> {
        let Outbound {
            mut method,
            mut url,
            headers,
            body,
            redirects,
            allow,
            decompress,
        } = request;
        let mut headers = header_map(&headers)?;
        let mut body = body;
        let mut hops = 0usize;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(ProviderError::Other(format!(
                "a {}: URL cannot be fetched over HTTP",
                url.scheme()
            )));
        }
        loop {
            if let Some(allow) = &allow {
                let action = if hops == 0 { "fetch" } else { "fetch redirect" };
                allow.check_url(&url, action)?;
            }
            let (this_body, resend) = match body {
                Body::Empty => (Body::Empty, Body::Empty),
                Body::Bytes(bytes) => (Body::Bytes(bytes.clone()), Body::Bytes(bytes)),
                Body::Stream(stream) => (Body::Stream(stream), Body::Empty),
            };
            let streamed = matches!(this_body, Body::Stream(_));
            // Per hop: a redirect may move between a proxied and a direct
            // target, or between schemes.
            let hop_headers = match self.forward_authorization(&url) {
                Some(auth) => {
                    let mut with = headers.clone();
                    with.insert(header::PROXY_AUTHORIZATION, auth);
                    std::borrow::Cow::Owned(with)
                }
                None => std::borrow::Cow::Borrowed(&headers),
            };
            let response = self
                .exchange(
                    &method,
                    &url,
                    request_headers(&hop_headers, decompress),
                    this_body,
                )
                .await?;
            let status = response.status();
            let location = response.headers().get(header::LOCATION).cloned();
            let follow = redirects == Redirects::Follow && is_redirect(status);
            let Some(location) = location.filter(|_| follow) else {
                return Ok(inbound(&method, url, hops > 0, response, decompress));
            };
            if hops == MAX_REDIRECTS {
                return Err(ProviderError::Coded {
                    code: ErrorCode::TooManyRedirects,
                    message: format!("too many redirects (more than {MAX_REDIRECTS})"),
                });
            }
            let next = next_url(&url, &location)?;
            // Fetch, HTTP-redirect fetch step 12: a 303 becomes GET (HEAD
            // stays HEAD), and so does a 301 or 302 for a POST.
            let to_get = (status == StatusCode::SEE_OTHER && method != Method::HEAD)
                || (matches!(status, StatusCode::MOVED_PERMANENTLY | StatusCode::FOUND)
                    && method == Method::POST);
            if to_get {
                method = Method::GET;
                body = Body::Empty;
                for name in REQUEST_BODY_HEADERS {
                    headers.remove(name);
                }
            } else if streamed {
                // Step 11: a body that was streamed is gone, and resending an
                // empty one in its place would be a different request.
                return Err(ProviderError::Other(
                    "a redirect asked for the request body again, and a streamed body \
                     cannot be sent twice"
                        .to_string(),
                ));
            } else {
                body = resend;
            }
            if !same_origin(&url, &next) {
                for name in CROSS_ORIGIN_HEADERS {
                    headers.remove(name);
                }
            }
            drop(response);
            url = next;
            hops += 1;
        }
    }
}

/// Headers that describe a request body, removed with it (Fetch's
/// "request-body-header name").
const REQUEST_BODY_HEADERS: [HeaderName; 5] = [
    header::CONTENT_ENCODING,
    header::CONTENT_LANGUAGE,
    header::CONTENT_LOCATION,
    header::CONTENT_TYPE,
    header::CONTENT_LENGTH,
];

/// Credentials that never follow a redirect to another origin.
const CROSS_ORIGIN_HEADERS: [HeaderName; 5] = [
    header::AUTHORIZATION,
    header::COOKIE,
    HeaderName::from_static("cookie2"),
    header::PROXY_AUTHORIZATION,
    header::WWW_AUTHENTICATE,
];

impl HttpClient {
    /// One request to `url`'s origin, on a pooled or new connection.
    async fn exchange(
        &self,
        method: &Method,
        url: &Url,
        headers: HeaderMap,
        body: Body,
    ) -> Result<hyper::Response<ResponseStream>, ProviderError> {
        let secure = url.scheme() == "https";
        let host = url
            .host_str()
            .ok_or_else(|| ProviderError::Other("the URL has no host".to_string()))?;
        let port = url
            .port_or_known_default()
            .unwrap_or(if secure { 443 } else { 80 });
        let bare_host = host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_string();
        // A plain `http:` request through a proxy is forwarded: the proxy is
        // sent the absolute URL. Anything else is sent in origin form.
        let forwarded = !secure
            && self
                .proxies
                .as_ref()
                .is_some_and(|p| p.for_target("http", &bare_host, port).is_some());
        let mut target_url = url.clone();
        target_url.set_fragment(None);
        let origin_form = match target_url.query() {
            Some(q) => format!("{}?{q}", target_url.path()),
            None => target_url.path().to_string(),
        };
        let target = if forwarded {
            target_url.as_str().to_string()
        } else {
            origin_form
        };
        let authority = match url.port() {
            Some(p) => format!("{host}:{p}"),
            None => host.to_string(),
        };
        let origin = Origin {
            secure,
            host: bare_host.clone(),
            port,
        };
        let connector = self.connector.clone();
        crate::h1_client::send(
            &self.pool,
            &origin,
            Exchange {
                method,
                target: &target,
                host: &authority,
                headers: &headers,
            },
            body,
            || {
                let connector = connector.clone();
                let host = bare_host.clone();
                async move {
                    connector
                        .connect(secure, host, port)
                        .await
                        .map_err(|e| ProviderError::Coded {
                            code: e.code,
                            message: format!("request failed: {}", e.message),
                        })
                }
            },
        )
        .await
    }
}

fn is_redirect(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::MOVED_PERMANENTLY
            | StatusCode::FOUND
            | StatusCode::SEE_OTHER
            | StatusCode::TEMPORARY_REDIRECT
            | StatusCode::PERMANENT_REDIRECT
    )
}

fn same_origin(a: &Url, b: &Url) -> bool {
    a.scheme() == b.scheme()
        && a.host_str() == b.host_str()
        && a.port_or_known_default() == b.port_or_known_default()
}

/// The URL a `Location` names, resolved against the current one.
fn next_url(current: &Url, location: &HeaderValue) -> Result<Url, ProviderError> {
    let invalid = || ProviderError::Other("a redirect's Location is not a valid URL".to_string());
    let location = location.to_str().map_err(|_| invalid())?;
    let mut next = current.join(location).map_err(|_| invalid())?;
    if !matches!(next.scheme(), "http" | "https") {
        return Err(ProviderError::Other(format!(
            "a redirect to a {}: URL is not followed",
            next.scheme()
        )));
    }
    // Fetch, step 6: a Location without a fragment keeps the request's.
    if next.fragment().is_none() {
        next.set_fragment(current.fragment());
    }
    Ok(next)
}

fn header_map(pairs: &[(String, String)]) -> Result<HeaderMap, ProviderError> {
    let mut map = HeaderMap::with_capacity(pairs.len());
    for (name, value) in pairs {
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| ProviderError::Other(format!("invalid header name: {name}")))?;
        let value = HeaderValue::from_str(value)
            .map_err(|_| ProviderError::Other(format!("invalid value for header {name}")))?;
        map.append(name, value);
    }
    Ok(map)
}

/// One hop's request: the caller's headers plus the client's defaults.
/// A request body as hyper's HTTP/2 client takes it.
pub(crate) fn request_body(body: Body) -> RequestBody {
    match body {
        Body::Empty => Empty::new().map_err(|never| match never {}).boxed_unsync(),
        Body::Bytes(bytes) => Full::new(bytes)
            .map_err(|never| match never {})
            .boxed_unsync(),
        Body::Stream(stream) => {
            let frames = stream.map(|chunk| {
                chunk
                    .map(|bytes| Frame::data(Bytes::from(bytes)))
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
            });
            StreamBody::new(frames).boxed_unsync()
        }
    }
}

/// The request's headers with the ones this client adds.
fn request_headers(headers: &HeaderMap, decompress: bool) -> HeaderMap {
    let mut out = headers.clone();
    if !out.contains_key(header::USER_AGENT) {
        out.insert(header::USER_AGENT, HeaderValue::from_static(USER_AGENT));
    }
    // A range of a compressed representation is not a range of the resource,
    // so a ranged request asks for the identity coding (Fetch, HTTP-network-or-
    // cache fetch). Saying nothing would not do: a request without
    // `Accept-Encoding` accepts any coding (RFC 9110 §12.5.3). Nor does a
    // request that will not decode invite a coding. A request that names its
    // own codings keeps them.
    if !out.contains_key(header::ACCEPT_ENCODING) {
        let codings = if !decompress || out.contains_key(header::RANGE) {
            "identity"
        } else {
            "gzip, br, deflate"
        };
        out.insert(header::ACCEPT_ENCODING, HeaderValue::from_static(codings));
    }
    // Trailers are welcome. A conformant HTTP/1.1 server sends a trailer
    // section only to a client that asked for one (RFC 9110 §10.1.4), and
    // `trailers` is the one value HTTP/2 allows (RFC 9113 §8.2.2).
    out.insert(header::TE, HeaderValue::from_static("trailers"));
    out
}

/// The content-codings this client decodes.
#[derive(Clone, Copy)]
enum Coding {
    Gzip,
    Brotli,
    Deflate,
}

/// The coding to decode `headers`' body with, if it is one coding this client
/// implements. A list of codings, or one it does not know, passes through.
fn coding(method: &Method, status: StatusCode, headers: &HeaderMap) -> Option<Coding> {
    if *method == Method::HEAD
        || status == StatusCode::NO_CONTENT
        || status == StatusCode::NOT_MODIFIED
    {
        return None;
    }
    let mut values = headers.get_all(header::CONTENT_ENCODING).iter();
    let value = values.next()?;
    if values.next().is_some() {
        return None;
    }
    match value.to_str().ok()?.trim().to_ascii_lowercase().as_str() {
        "gzip" | "x-gzip" => Some(Coding::Gzip),
        "br" => Some(Coding::Brotli),
        "deflate" => Some(Coding::Deflate),
        _ => None,
    }
}

fn inbound(
    method: &Method,
    url: Url,
    redirected: bool,
    response: hyper::Response<ResponseStream>,
    decompress: bool,
) -> Inbound {
    let (parts, incoming) = response.into_parts();
    let mut headers = parts.headers;
    let (trailer_tx, trailers) = tokio::sync::oneshot::channel();
    let raw = frames(incoming, trailer_tx);
    let coding = coding(method, parts.status, &headers).filter(|_| decompress);
    let body: ByteStream = match coding {
        Some(coding) => {
            headers.remove(header::CONTENT_ENCODING);
            headers.remove(header::CONTENT_LENGTH);
            decoded(raw, coding)
        }
        None => Box::pin(raw.map(|chunk| chunk.map(|bytes| bytes.to_vec()).map_err(body_error))),
    };
    Inbound {
        status: parts.status,
        version: parts.version,
        url,
        redirected,
        headers,
        body,
        trailers,
    }
}

/// The body's data, with its trailer block handed to `trailer_tx` when it
/// arrives. The sender is dropped when the body ends, so the receiver settles
/// either way.
fn frames(
    mut incoming: ResponseStream,
    trailer_tx: tokio::sync::oneshot::Sender<Vec<(String, String)>>,
) -> impl Stream<Item = std::io::Result<Bytes>> + Send + Unpin + 'static {
    let mut trailer_tx = Some(trailer_tx);
    futures_util::stream::poll_fn(move |cx| {
        loop {
            return match Pin::new(&mut incoming).poll_frame(cx) {
                Poll::Pending => Poll::Pending,
                Poll::Ready(None) => {
                    trailer_tx.take();
                    Poll::Ready(None)
                }
                Poll::Ready(Some(Err(e))) => Poll::Ready(Some(Err(std::io::Error::other(e)))),
                Poll::Ready(Some(Ok(frame))) => match frame.into_data() {
                    Ok(bytes) => Poll::Ready(Some(Ok(bytes))),
                    Err(frame) => {
                        if let Ok(map) = frame.into_trailers()
                            && let Some(tx) = trailer_tx.take()
                        {
                            let _ = tx.send(pairs(&map));
                        }
                        continue;
                    }
                },
            };
        }
    })
}

/// `raw` decoded with `coding`, at most [`DECODED_CHUNK`] bytes at a time. An
/// empty body stays empty rather than failing as a truncated stream.
fn decoded(
    raw: impl Stream<Item = std::io::Result<Bytes>> + Send + Unpin + 'static,
    coding: Coding,
) -> ByteStream {
    use async_compression::tokio::bufread::{BrotliDecoder, GzipDecoder, ZlibDecoder};
    use tokio_util::io::{ReaderStream, StreamReader};

    type Chunks = Pin<Box<dyn Stream<Item = std::io::Result<Bytes>> + Send>>;
    let stream = futures_util::stream::once(async move {
        let mut raw = raw;
        let Some(first) = raw.next().await else {
            return Box::pin(futures_util::stream::empty()) as Chunks;
        };
        let reader = StreamReader::new(futures_util::stream::once(async { first }).chain(raw));
        match coding {
            Coding::Gzip => Box::pin(ReaderStream::with_capacity(
                GzipDecoder::new(reader),
                DECODED_CHUNK,
            )) as Chunks,
            Coding::Brotli => Box::pin(ReaderStream::with_capacity(
                BrotliDecoder::new(reader),
                DECODED_CHUNK,
            )),
            Coding::Deflate => Box::pin(ReaderStream::with_capacity(
                ZlibDecoder::new(reader),
                DECODED_CHUNK,
            )),
        }
    })
    .flatten();
    Box::pin(stream.map_ok(|bytes| bytes.to_vec()).map_err(body_error))
}

fn body_error(e: std::io::Error) -> ProviderError {
    ProviderError::Other(format!("body: {e}"))
}

fn pairs(map: &HeaderMap) -> Vec<(String, String)> {
    map.iter()
        .map(|(n, v)| (n.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
        .collect()
}

/// A failure to connect, with the code it is reported under.
#[derive(Debug)]
struct ConnectError {
    code: ErrorCode,
    message: String,
}

impl std::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ConnectError {}

impl From<ProxyError> for ConnectError {
    fn from(e: ProxyError) -> Self {
        ConnectError {
            code: e.code,
            message: e.message,
        }
    }
}

/// The connector to an `https://` proxy: the process trust store, no ALPN —
/// the proxy is spoken to over HTTP/1.1, whatever the target negotiates.
fn proxy_tls(trust: &TrustStore) -> TlsConnector {
    TlsConnector::from(Arc::new(trust.client_config(&[])))
}

/// Classifies a send failure with a stable guest-facing code where one can be
/// derived (SPEC §6 Phase 13). Never includes the URL: its credentials and
/// query are the program's secrets, and the message is for logs (D141).
pub(crate) fn classify_hyper(e: &hyper::Error) -> ProviderError {
    classify(e)
}

fn classify(e: &(dyn std::error::Error + 'static)) -> ProviderError {
    let message = format!("request failed: {}", chain(e));
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(e);
    // The server closed the connection before its response was complete. A
    // sentence and a code (D206), not the client library's chain of wrappers.
    while let Some(err) = source {
        if err
            .downcast_ref::<hyper::Error>()
            .is_some_and(hyper::Error::is_incomplete_message)
        {
            return ProviderError::Coded {
                code: ErrorCode::ConnectionReset,
                message: "the connection closed before the response completed".to_string(),
            };
        }
        source = err.source();
    }
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(e);
    let mut io_fallback = None;
    while let Some(err) = source {
        if let Some(connect) = err.downcast_ref::<ConnectError>() {
            return ProviderError::Coded {
                code: connect.code,
                message,
            };
        }
        if let Some(io) = err.downcast_ref::<std::io::Error>()
            && io.kind() != std::io::ErrorKind::Other
        {
            let code = ErrorCode::from_io_kind(io.kind());
            if code != ErrorCode::Io {
                return ProviderError::Coded { code, message };
            }
            io_fallback = Some(code);
        }
        source = err.source();
    }
    let lower = message.to_lowercase();
    let code = if lower.contains("reset") || lower.contains("broken pipe") {
        Some(ErrorCode::ConnectionReset)
    } else {
        io_fallback
    };
    match code {
        Some(code) => ProviderError::Coded { code, message },
        None => ProviderError::Other(message),
    }
}

/// `e` and its sources, joined, skipping a source whose text its parent
/// already repeats.
fn chain(e: &(dyn std::error::Error + 'static)) -> String {
    let mut text = e.to_string();
    let mut source = e.source();
    while let Some(err) = source {
        let next = err.to_string();
        if !text.contains(&next) {
            text.push_str(": ");
            text.push_str(&next);
        }
        source = err.source();
    }
    text
}

/// DNS and TCP through [`crate::tcp_connect`], TLS through the trust store,
/// all within one deadline.
#[derive(Clone)]
struct Connector {
    tls: TlsConnector,
    timeout: Duration,
    proxies: Option<Arc<Proxies>>,
    proxy_tls: TlsConnector,
}

impl Connector {
    /// A connection to `host:port`, TLS when `secure`, through the proxy when
    /// one applies — DNS, TCP and TLS within the one deadline.
    async fn connect(&self, secure: bool, host: String, port: u16) -> Result<Conn, ConnectError> {
        let tls = self.tls.clone();
        let proxies = self.proxies.clone();
        let proxy_tls = self.proxy_tls.clone();
        let connect = async move {
            let via = proxies
                .as_ref()
                .and_then(|p| p.for_target(if secure { "https" } else { "http" }, &host, port));
            if let Some(server) = via {
                if !secure {
                    // Forwarded: the request goes to the proxy in absolute form.
                    let wire = server.connect(&proxy_tls).await?;
                    return Ok(Conn::Proxied(Box::new(wire), false));
                }
                let wire = server.tunnel(&proxy_tls, &host, port).await?;
                let name = ServerName::try_from(host).map_err(|_| ConnectError {
                    code: ErrorCode::Tls,
                    message: "the host is not a name TLS can verify".to_string(),
                })?;
                let stream = tls.connect(name, wire).await.map_err(|e| ConnectError {
                    code: ErrorCode::Tls,
                    message: format!("TLS: {e}"),
                })?;
                let h2 = stream.get_ref().1.alpn_protocol() == Some(b"h2");
                let wire: BoxWire = Box::new(stream);
                return Ok(Conn::Proxied(Box::new(wire), h2));
            }
            let tcp = crate::tcp_connect::connect(&host, port, TCP_KEEPALIVE)
                .await
                .map_err(|e| ConnectError {
                    code: e.code,
                    message: e.message,
                })?;
            if !secure {
                return Ok(Conn::Plain(tcp));
            }
            let name = ServerName::try_from(host).map_err(|_| ConnectError {
                code: ErrorCode::Tls,
                message: "the host is not a name TLS can verify".to_string(),
            })?;
            let stream = tls.connect(name, tcp).await.map_err(|e| ConnectError {
                code: ErrorCode::Tls,
                message: format!("TLS: {e}"),
            })?;
            let h2 = stream.get_ref().1.alpn_protocol() == Some(b"h2");
            Ok(Conn::Tls(Box::new(stream), h2))
        };
        tokio::time::timeout(self.timeout, connect)
            .await
            .map_err(|_| ConnectError {
                code: ErrorCode::TimedOut,
                message: format!("no connection within {}s", self.timeout.as_secs()),
            })?
    }
}

/// A connection, plain or TLS, direct or through a proxy.
pub(crate) enum Conn {
    Plain(TcpStream),
    /// The stream, and whether ALPN chose HTTP/2. Boxed: a TLS session is a
    /// kilobyte, and the pool holds many plain connections too.
    Tls(Box<TlsStream<TcpStream>>, bool),
    /// Through the proxy (D205): the stream, and whether ALPN chose HTTP/2.
    /// Whether a request on it is forwarded or tunnelled is the request's
    /// scheme, decided in `exchange`.
    Proxied(Box<BoxWire>, bool),
}

impl Conn {
    /// Whether ALPN chose HTTP/2 for this connection.
    pub(crate) fn is_h2(&self) -> bool {
        matches!(self, Conn::Tls(_, true) | Conn::Proxied(_, true))
    }
}

impl AsyncRead for Conn {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Conn::Plain(io) => Pin::new(io).poll_read(cx, buf),
            Conn::Tls(io, _) => Pin::new(io.as_mut()).poll_read(cx, buf),
            Conn::Proxied(io, _) => Pin::new(io.as_mut()).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Conn {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match self.get_mut() {
            Conn::Plain(io) => Pin::new(io).poll_write(cx, buf),
            Conn::Tls(io, _) => Pin::new(io.as_mut()).poll_write(cx, buf),
            Conn::Proxied(io, _) => Pin::new(io.as_mut()).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Conn::Plain(io) => Pin::new(io).poll_flush(cx),
            Conn::Tls(io, _) => Pin::new(io.as_mut()).poll_flush(cx),
            Conn::Proxied(io, _) => Pin::new(io.as_mut()).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Conn::Plain(io) => Pin::new(io).poll_shutdown(cx),
            Conn::Tls(io, _) => Pin::new(io.as_mut()).poll_shutdown(cx),
            Conn::Proxied(io, _) => Pin::new(io.as_mut()).poll_shutdown(cx),
        }
    }

    fn is_write_vectored(&self) -> bool {
        match self {
            Conn::Plain(io) => io.is_write_vectored(),
            Conn::Tls(io, _) => io.is_write_vectored(),
            Conn::Proxied(io, _) => io.is_write_vectored(),
        }
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> Poll<std::io::Result<usize>> {
        match self.get_mut() {
            Conn::Plain(io) => Pin::new(io).poll_write_vectored(cx, bufs),
            Conn::Tls(io, _) => Pin::new(io.as_mut()).poll_write_vectored(cx, bufs),
            Conn::Proxied(io, _) => Pin::new(io.as_mut()).poll_write_vectored(cx, bufs),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

    /// A request as the test server received it.
    #[derive(Clone, Debug)]
    struct Seen {
        method: String,
        path: String,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    }

    impl Seen {
        fn header(&self, name: &str) -> Option<&str> {
            self.headers
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.as_str())
        }
    }

    type Responder = Arc<dyn Fn(&Seen) -> Vec<u8> + Send + Sync>;

    /// An HTTP/1.1 server on loopback that records every request, answers
    /// each with `respond`, and keeps connections alive. Returns its port,
    /// what it saw, and how many connections it accepted.
    struct Server {
        port: u16,
        seen: Arc<Mutex<Vec<Seen>>>,
        connections: Arc<AtomicUsize>,
    }

    impl Server {
        async fn start(respond: impl Fn(&Seen) -> Vec<u8> + Send + Sync + 'static) -> Server {
            let respond: Responder = Arc::new(respond);
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
                .await
                .unwrap();
            let port = listener.local_addr().unwrap().port();
            let seen = Arc::new(Mutex::new(Vec::new()));
            let connections = Arc::new(AtomicUsize::new(0));
            let (s, c) = (seen.clone(), connections.clone());
            tokio::spawn(async move {
                while let Ok((socket, _)) = listener.accept().await {
                    c.fetch_add(1, Ordering::SeqCst);
                    let (respond, seen) = (respond.clone(), s.clone());
                    tokio::spawn(async move {
                        let (read, mut write) = socket.into_split();
                        let mut read = BufReader::new(read);
                        while let Some(request) = read_request(&mut read).await {
                            let response = respond(&request);
                            seen.lock().unwrap().push(request);
                            if write.write_all(&response).await.is_err() {
                                return;
                            }
                        }
                    });
                }
            });
            Server {
                port,
                seen,
                connections,
            }
        }

        fn url(&self, path: &str) -> Url {
            Url::parse(&format!("http://127.0.0.1:{}{path}", self.port)).unwrap()
        }

        fn seen(&self) -> Vec<Seen> {
            self.seen.lock().unwrap().clone()
        }
    }

    async fn read_request(read: &mut BufReader<tokio::net::tcp::OwnedReadHalf>) -> Option<Seen> {
        let mut line = String::new();
        if read.read_line(&mut line).await.ok()? == 0 {
            return None;
        }
        let mut parts = line.split_whitespace();
        let method = parts.next()?.to_string();
        let path = parts.next()?.to_string();
        let mut headers = Vec::new();
        loop {
            line.clear();
            read.read_line(&mut line).await.ok()?;
            let trimmed = line.trim_end();
            if trimmed.is_empty() {
                break;
            }
            let (name, value) = trimmed.split_once(':')?;
            headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
        }
        let get = |name: &str| {
            headers
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.clone())
        };
        let mut body = Vec::new();
        if let Some(length) = get("content-length") {
            body.resize(length.parse().ok()?, 0);
            read.read_exact(&mut body).await.ok()?;
        } else if get("transfer-encoding").is_some_and(|v| v.contains("chunked")) {
            loop {
                line.clear();
                read.read_line(&mut line).await.ok()?;
                let size = usize::from_str_radix(line.trim(), 16).ok()?;
                let mut chunk = vec![0; size + 2];
                read.read_exact(&mut chunk).await.ok()?;
                if size == 0 {
                    break;
                }
                body.extend_from_slice(&chunk[..size]);
            }
        }
        Some(Seen {
            method,
            path,
            headers,
            body,
        })
    }

    fn reply(status: &str, headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
        let mut out = format!("HTTP/1.1 {status}\r\ncontent-length: {}\r\n", body.len());
        for (name, value) in headers {
            out.push_str(&format!("{name}: {value}\r\n"));
        }
        out.push_str("\r\n");
        let mut out = out.into_bytes();
        out.extend_from_slice(body);
        out
    }

    fn client() -> HttpClient {
        HttpClient::new(&TrustStore::bundled())
    }

    fn request(method: Method, url: Url) -> Outbound {
        Outbound {
            method,
            url,
            headers: Vec::new(),
            body: Body::Empty,
            redirects: Redirects::Follow,
            allow: None,
            decompress: true,
        }
    }

    async fn text(inbound: Inbound) -> String {
        String::from_utf8(inbound.bytes(usize::MAX).await.unwrap()).unwrap()
    }

    /// A server that redirects `/from` to `location` with `status`, and
    /// answers anything else `ok`.
    async fn redirecting(status: &'static str, location: String) -> Server {
        Server::start(move |seen| {
            if seen.path == "/from" {
                reply(status, &[("location", &location)], b"")
            } else {
                reply("200 OK", &[], b"ok")
            }
        })
        .await
    }

    const CREDENTIALS: [(&str, &str); 5] = [
        ("authorization", "Bearer secret"),
        ("cookie", "session=secret"),
        ("cookie2", "legacy=secret"),
        ("proxy-authorization", "Basic secret"),
        ("www-authenticate", "secret"),
    ];

    fn with_credentials(mut outbound: Outbound) -> Outbound {
        outbound.headers = CREDENTIALS
            .iter()
            .map(|(n, v)| (n.to_string(), v.to_string()))
            .chain([("x-kept".to_string(), "yes".to_string())])
            .collect();
        outbound
    }

    // Credentials never follow a redirect to another origin: a different port
    // is a different origin, and so is a different host on the same port.
    #[tokio::test]
    async fn a_cross_origin_redirect_drops_credentials() {
        let target = Server::start(|_| reply("200 OK", &[], b"there")).await;
        for location in [
            format!("http://127.0.0.1:{}/to", target.port),
            format!("http://localhost:{}/to", target.port),
        ] {
            let origin = redirecting("302 Found", location).await;
            let response = client()
                .send(with_credentials(request(Method::GET, origin.url("/from"))))
                .await
                .unwrap();
            assert_eq!(text(response).await, "there");
        }
        let seen = target.seen();
        assert_eq!(seen.len(), 2);
        for request in seen {
            for (name, _) in CREDENTIALS {
                assert_eq!(request.header(name), None, "{name} crossed origins");
            }
            assert_eq!(request.header("x-kept"), Some("yes"));
        }
    }

    #[tokio::test]
    async fn a_same_origin_redirect_keeps_credentials() {
        let server = redirecting("302 Found", "/to".to_string()).await;
        client()
            .send(with_credentials(request(Method::GET, server.url("/from"))))
            .await
            .unwrap();
        let to = server.seen().into_iter().find(|s| s.path == "/to").unwrap();
        for (name, value) in CREDENTIALS {
            assert_eq!(to.header(name), Some(value), "{name} was dropped");
        }
    }

    fn post_with_body(url: Url, body: Body) -> Outbound {
        let mut outbound = request(Method::POST, url);
        outbound.headers = vec![
            ("content-type".to_string(), "application/json".to_string()),
            ("content-language".to_string(), "en".to_string()),
            ("content-location".to_string(), "/x".to_string()),
        ];
        outbound.body = body;
        outbound
    }

    // Fetch, HTTP-redirect fetch step 12: a 303, and a 301/302 for a POST,
    // become a GET without the body or the headers that described it.
    #[tokio::test]
    async fn a_method_changing_redirect_drops_the_body_and_its_headers() {
        for status in ["301 Moved Permanently", "302 Found", "303 See Other"] {
            let server = redirecting(status, "/to".to_string()).await;
            client()
                .send(post_with_body(
                    server.url("/from"),
                    Body::Bytes("{}".into()),
                ))
                .await
                .unwrap();
            let to = server.seen().into_iter().find(|s| s.path == "/to").unwrap();
            assert_eq!(to.method, "GET", "{status}");
            assert!(to.body.is_empty(), "{status}: the body was resent");
            for name in [
                "content-type",
                "content-language",
                "content-location",
                "content-length",
                "content-encoding",
            ] {
                assert_eq!(to.header(name), None, "{status}: {name} survived");
            }
        }
    }

    #[tokio::test]
    async fn a_303_keeps_head_and_others_keep_their_method() {
        let server = redirecting("303 See Other", "/to".to_string()).await;
        client()
            .send(request(Method::HEAD, server.url("/from")))
            .await
            .unwrap();
        assert_eq!(server.seen()[1].method, "HEAD");

        let server = redirecting("302 Found", "/to".to_string()).await;
        let mut put = request(Method::PUT, server.url("/from"));
        put.body = Body::Bytes("data".into());
        client().send(put).await.unwrap();
        let to = &server.seen()[1];
        assert_eq!(
            (to.method.as_str(), to.body.as_slice()),
            ("PUT", &b"data"[..])
        );
    }

    #[tokio::test]
    async fn a_307_or_308_resends_the_method_and_a_buffered_body() {
        for status in ["307 Temporary Redirect", "308 Permanent Redirect"] {
            let server = redirecting(status, "/to".to_string()).await;
            client()
                .send(post_with_body(
                    server.url("/from"),
                    Body::Bytes("{}".into()),
                ))
                .await
                .unwrap();
            let to = server.seen().into_iter().find(|s| s.path == "/to").unwrap();
            assert_eq!(to.method, "POST", "{status}");
            assert_eq!(to.body, b"{}", "{status}");
            assert_eq!(to.header("content-type"), Some("application/json"));
        }
    }

    fn streamed(chunks: &[&'static [u8]]) -> Body {
        let chunks: Vec<_> = chunks.iter().map(|c| Ok(c.to_vec())).collect();
        Body::Stream(Box::pin(futures_util::stream::iter(chunks)))
    }

    // A streamed body is gone once sent; resending nothing in its place would
    // be a different request, so it is an error rather than a silent empty
    // POST — and the second request is never made.
    #[tokio::test]
    async fn a_streamed_body_is_never_resent() {
        for status in ["307 Temporary Redirect", "308 Permanent Redirect"] {
            let server = redirecting(status, "/to".to_string()).await;
            let err = client()
                .send(post_with_body(server.url("/from"), streamed(&[b"a", b"b"])))
                .await
                .err()
                .expect("a redirect that needs the body again must fail");
            assert!(err.to_string().contains("cannot be sent twice"), "{err}");
            assert_eq!(
                server.seen().len(),
                1,
                "{status}: the redirect was followed"
            );
            assert_eq!(server.seen()[0].body, b"ab");
        }
        // A 303 needs no body, so a streamed one is fine.
        let server = redirecting("303 See Other", "/to".to_string()).await;
        client()
            .send(post_with_body(server.url("/from"), streamed(&[b"a"])))
            .await
            .unwrap();
        assert_eq!(server.seen()[1].method, "GET");
    }

    /// A server whose `/n` redirects to `/n+1` until `/limit`.
    async fn chain(limit: usize) -> Server {
        Server::start(move |seen| {
            let n: usize = seen.path[1..].parse().unwrap_or(0);
            if n < limit {
                reply("302 Found", &[("location", &format!("/{}", n + 1))], b"")
            } else {
                reply("200 OK", &[], b"end")
            }
        })
        .await
    }

    #[tokio::test]
    async fn twenty_redirects_are_followed_and_the_twenty_first_is_not() {
        let server = chain(MAX_REDIRECTS).await;
        let response = client()
            .send(request(Method::GET, server.url("/0")))
            .await
            .unwrap();
        assert!(response.redirected);
        assert_eq!(text(response).await, "end");

        let server = chain(MAX_REDIRECTS + 1).await;
        let err = client()
            .send(request(Method::GET, server.url("/0")))
            .await
            .err()
            .unwrap();
        assert_eq!(err.code(), Some(ErrorCode::TooManyRedirects));
        assert_eq!(server.seen().len(), MAX_REDIRECTS + 1);
    }

    #[tokio::test]
    async fn a_location_resolves_and_inherits_the_fragment() {
        let server = Server::start(|seen| {
            if seen.path == "/a/b/from" {
                reply("302 Found", &[("location", "../to?x=1")], b"")
            } else {
                reply("200 OK", &[], b"")
            }
        })
        .await;
        let mut url = server.url("/a/b/from");
        url.set_fragment(Some("section"));
        let response = client().send(request(Method::GET, url)).await.unwrap();
        assert_eq!(response.url.path(), "/a/to");
        assert_eq!(response.url.query(), Some("x=1"));
        assert_eq!(response.url.fragment(), Some("section"));
        // The fragment is never sent.
        assert!(server.seen().iter().all(|s| !s.path.contains('#')));
    }

    // A server cannot send `fetch` somewhere that is not the web.
    #[tokio::test]
    async fn a_redirect_off_the_web_is_refused() {
        for location in [
            "file:///etc/passwd",
            "data:text/plain,hi",
            "ftp://x/",
            "javascript:1",
        ] {
            let server = redirecting("302 Found", location.to_string()).await;
            let err = client()
                .send(request(Method::GET, server.url("/from")))
                .await
                .err()
                .unwrap_or_else(|| panic!("{location} was followed"));
            assert!(
                err.to_string().contains("is not followed"),
                "{location}: {err}"
            );
        }
        let server = redirecting("302 Found", "http://[::1".to_string()).await;
        assert!(
            client()
                .send(request(Method::GET, server.url("/from")))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn a_redirect_without_location_or_in_manual_mode_is_returned() {
        let server = Server::start(|_| reply("302 Found", &[], b"no location")).await;
        let response = client()
            .send(request(Method::GET, server.url("/from")))
            .await
            .unwrap();
        assert_eq!(response.status, StatusCode::FOUND);
        assert!(!response.redirected);

        let server = redirecting("302 Found", "/to".to_string()).await;
        let mut manual = request(Method::GET, server.url("/from"));
        manual.redirects = Redirects::Manual;
        let response = client().send(manual).await.unwrap();
        assert_eq!(response.status, StatusCode::FOUND);
        assert_eq!(server.seen().len(), 1, "manual mode followed the redirect");
    }

    // Every hop is checked before it is sent: the denied server sees nothing.
    #[tokio::test]
    async fn every_redirect_hop_is_checked_against_the_allowlist() {
        let denied = Server::start(|_| reply("200 OK", &[], b"leaked")).await;
        let origin = redirecting("302 Found", format!("http://127.0.0.1:{}/x", denied.port)).await;
        let mut outbound = request(Method::GET, origin.url("/from"));
        outbound.allow = Some(Arc::new(
            HostAllowlist::parse([format!("127.0.0.1:{}", origin.port)]).unwrap(),
        ));
        let err = client().send(outbound).await.err().unwrap();
        assert_eq!(err.code(), Some(ErrorCode::PermissionDenied));
        assert!(denied.seen().is_empty(), "the denied host was contacted");
    }

    fn gzip(bytes: &[u8]) -> Vec<u8> {
        use std::io::Write;
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }

    fn zlib(bytes: &[u8]) -> Vec<u8> {
        use std::io::Write;
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }

    fn brotli(bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        brotli::BrotliCompress(&mut &bytes[..], &mut out, &Default::default()).unwrap();
        out
    }

    async fn served(coding: &'static str, body: Vec<u8>) -> Inbound {
        let server =
            Server::start(move |_| reply("200 OK", &[("content-encoding", coding)], &body)).await;
        client()
            .send(request(Method::GET, server.url("/")))
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn the_three_codings_are_decoded_and_their_headers_dropped() {
        let plain = b"hello, decoded world".repeat(50);
        for (coding, encoded) in [
            ("gzip", gzip(&plain)),
            ("x-gzip", gzip(&plain)),
            ("br", brotli(&plain)),
            ("deflate", zlib(&plain)),
            ("GZip", gzip(&plain)),
        ] {
            let response = served(coding, encoded).await;
            assert!(
                !response.headers.contains_key(header::CONTENT_ENCODING),
                "{coding}"
            );
            assert!(
                !response.headers.contains_key(header::CONTENT_LENGTH),
                "{coding}"
            );
            assert_eq!(response.bytes(usize::MAX).await.unwrap(), plain, "{coding}");
        }
    }

    // `decompress: false` (D210): the body is the bytes the server sent, and the
    // headers still describe them. The request asks for no coding unless it
    // named its own.
    #[tokio::test]
    async fn a_request_that_does_not_decompress_gets_the_wire_bytes() {
        let plain = b"stored compressed".repeat(40);
        let encoded = gzip(&plain);
        let length = encoded.len().to_string();
        let sent = encoded.clone();
        let server =
            Server::start(move |_| reply("200 OK", &[("content-encoding", "gzip")], &sent)).await;
        let mut raw = request(Method::GET, server.url("/"));
        raw.decompress = false;
        let response = client().send(raw).await.unwrap();
        assert_eq!(
            response.headers.get(header::CONTENT_ENCODING).unwrap(),
            "gzip"
        );
        assert_eq!(
            response.headers.get(header::CONTENT_LENGTH).unwrap(),
            length.as_str()
        );
        assert_eq!(response.bytes(usize::MAX).await.unwrap(), encoded);

        let mut own = request(Method::GET, server.url("/"));
        own.decompress = false;
        own.headers = vec![("accept-encoding".to_string(), "gzip".to_string())];
        let response = client().send(own).await.unwrap();
        assert_eq!(response.bytes(usize::MAX).await.unwrap(), encoded);

        let seen = server.seen();
        assert_eq!(seen[0].header("accept-encoding"), Some("identity"));
        assert_eq!(seen[1].header("accept-encoding"), Some("gzip"));
    }

    // Decoding something this client does not implement would be a lie; the
    // bytes and the header that describes them arrive together.
    #[tokio::test]
    async fn other_codings_pass_through_untouched() {
        for coding in ["zstd", "gzip, br", "compress", "identity"] {
            let response = served(coding, b"raw bytes".to_vec()).await;
            assert_eq!(
                response.headers.get(header::CONTENT_ENCODING).unwrap(),
                coding,
                "{coding}"
            );
            assert_eq!(response.bytes(usize::MAX).await.unwrap(), b"raw bytes");
        }
    }

    #[tokio::test]
    async fn bodies_with_nothing_to_decode_are_not_decoded() {
        let empty = served("gzip", Vec::new()).await;
        assert!(empty.bytes(usize::MAX).await.unwrap().is_empty());
        for (method, status) in [(Method::HEAD, "200 OK"), (Method::GET, "204 No Content")] {
            let server = Server::start(move |_| {
                format!("HTTP/1.1 {status}\r\ncontent-encoding: gzip\r\n\r\n").into_bytes()
            })
            .await;
            let response = client()
                .send(request(method.clone(), server.url("/")))
                .await
                .unwrap();
            assert!(
                response.bytes(usize::MAX).await.unwrap().is_empty(),
                "{method} {status}"
            );
        }
    }

    #[tokio::test]
    async fn a_corrupt_encoding_is_an_error_not_garbage() {
        let mut broken = gzip(b"some text that will not survive");
        let middle = broken.len() / 2;
        broken[middle] ^= 0xff;
        broken.truncate(broken.len() - 4);
        let response = served("gzip", broken).await;
        assert!(response.bytes(usize::MAX).await.is_err());
    }

    // A highly compressible body expands a bounded piece at a time: 16 MiB of
    // zeros is a few kilobytes of gzip, and no single chunk may be the whole.
    #[tokio::test]
    async fn decoding_expands_a_bounded_chunk_at_a_time() {
        use std::io::Write;
        let plain = vec![0u8; 16 * 1024 * 1024];
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
        encoder.write_all(&plain).unwrap();
        let encoded = encoder.finish().unwrap();
        assert!(encoded.len() < 64 * 1024);
        let mut response = served("gzip", encoded).await;
        let mut total = 0;
        while let Some(chunk) = response.body.next().await {
            let chunk = chunk.unwrap();
            assert!(chunk.len() <= DECODED_CHUNK, "a {} byte chunk", chunk.len());
            total += chunk.len();
        }
        assert_eq!(total, plain.len());
    }

    #[tokio::test]
    async fn the_client_sends_its_defaults_unless_told_otherwise() {
        let server = Server::start(|_| reply("200 OK", &[], b"")).await;
        client()
            .send(request(Method::GET, server.url("/")))
            .await
            .unwrap();
        let mut custom = request(Method::GET, server.url("/"));
        custom.headers = vec![
            ("user-agent".to_string(), "mine/1".to_string()),
            ("accept-encoding".to_string(), "identity".to_string()),
        ];
        client().send(custom).await.unwrap();
        let mut ranged = request(Method::GET, server.url("/"));
        ranged.headers = vec![("range".to_string(), "bytes=0-9".to_string())];
        client().send(ranged).await.unwrap();

        let seen = server.seen();
        assert_eq!(seen[0].header("user-agent"), Some(USER_AGENT));
        assert_eq!(seen[0].header("accept-encoding"), Some("gzip, br, deflate"));
        assert_eq!(seen[0].header("te"), Some("trailers"));
        assert_eq!(seen[1].header("user-agent"), Some("mine/1"));
        assert_eq!(seen[1].header("accept-encoding"), Some("identity"));
        assert_eq!(
            seen[2].header("accept-encoding"),
            Some("identity"),
            "a range must ask for the identity coding, not leave it open"
        );
    }

    #[tokio::test]
    async fn trailers_arrive_after_the_body() {
        let server = Server::start(|_| {
            b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\ntrailer: grpc-status\r\n\r\n\
              2\r\nok\r\n0\r\ngrpc-status: 0\r\n\r\n"
                .to_vec()
        })
        .await;
        let response = client()
            .send(request(Method::GET, server.url("/")))
            .await
            .unwrap();
        let trailers = response.trailers;
        let mut body = response.body;
        let mut bytes = Vec::new();
        while let Some(chunk) = body.next().await {
            bytes.extend(chunk.unwrap());
        }
        assert_eq!(bytes, b"ok");
        assert_eq!(
            trailers.await.unwrap(),
            [("grpc-status".to_string(), "0".to_string())]
        );
    }

    // A pooled connection is reused for the same origin, and only for it.
    #[tokio::test]
    async fn connections_are_pooled_per_origin() {
        let a = Server::start(|_| reply("200 OK", &[], b"a")).await;
        let b = Server::start(|_| reply("200 OK", &[], b"b")).await;
        let client = client();
        for _ in 0..3 {
            let response = client.send(request(Method::GET, a.url("/"))).await.unwrap();
            text(response).await;
        }
        let response = client.send(request(Method::GET, b.url("/"))).await.unwrap();
        text(response).await;
        assert_eq!(
            a.connections.load(Ordering::SeqCst),
            1,
            "three requests, one connection"
        );
        assert_eq!(b.connections.load(Ordering::SeqCst), 1);
        assert_eq!(a.seen().len(), 3);
    }

    // A server that accepts the TCP connection and never answers the TLS
    // handshake is the case the connect deadline exists for.
    #[tokio::test]
    async fn a_handshake_that_never_finishes_times_out() {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((socket, _)) = listener.accept().await {
                held.push(socket);
            }
        });
        let client =
            HttpClient::with_connect_timeout(&TrustStore::bundled(), Duration::from_millis(200));
        let url = Url::parse(&format!("https://localhost:{port}/")).unwrap();
        let started = std::time::Instant::now();
        let err = client.send(request(Method::GET, url)).await.err().unwrap();
        assert_eq!(err.code(), Some(ErrorCode::TimedOut), "{err}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn connect_failures_carry_their_code() {
        let refused = client()
            .send(request(
                Method::GET,
                Url::parse("http://127.0.0.1:1/").unwrap(),
            ))
            .await
            .err()
            .unwrap();
        assert_eq!(
            refused.code(),
            Some(ErrorCode::ConnectionRefused),
            "{refused}"
        );
        let dns = client()
            .send(request(
                Method::GET,
                Url::parse("http://no.such.host.invalid/").unwrap(),
            ))
            .await
            .err()
            .unwrap();
        assert_eq!(dns.code(), Some(ErrorCode::Dns), "{dns}");
    }

    // The URL's credentials and query are the program's secrets; an error is
    // text that ends up in logs (D141).
    #[tokio::test]
    async fn an_error_never_repeats_the_url() {
        let url = Url::parse("http://user:hunter2@127.0.0.1:1/path?token=s3cret").unwrap();
        let err = client()
            .send(request(Method::GET, url))
            .await
            .err()
            .unwrap();
        let text = err.to_string();
        for secret in ["hunter2", "s3cret", "user", "token"] {
            assert!(!text.contains(secret), "{secret} leaked: {text}");
        }
    }

    #[tokio::test]
    async fn only_web_urls_are_sent() {
        for url in [
            "ftp://example.com/",
            "file:///etc/passwd",
            "data:text/plain,x",
        ] {
            let err = client()
                .send(request(Method::GET, Url::parse(url).unwrap()))
                .await
                .err()
                .unwrap_or_else(|| panic!("{url} was sent"));
            assert!(
                err.to_string().contains("cannot be fetched"),
                "{url}: {err}"
            );
        }
    }

    #[tokio::test]
    async fn bytes_refuses_a_body_over_its_limit() {
        let server = Server::start(|_| reply("200 OK", &[], &[b'x'; 2048])).await;
        let response = client()
            .send(request(Method::GET, server.url("/")))
            .await
            .unwrap();
        assert!(response.bytes(1024).await.is_err());
    }

    #[test]
    fn origins_compare_scheme_host_and_port() {
        let u = |s: &str| Url::parse(s).unwrap();
        assert!(same_origin(&u("http://a/x"), &u("http://a:80/y")));
        assert!(!same_origin(&u("http://a/"), &u("https://a/")));
        assert!(!same_origin(&u("http://a/"), &u("http://b/")));
        assert!(!same_origin(&u("http://a/"), &u("http://a:8080/")));
        assert!(same_origin(&u("https://a/"), &u("https://a:443/")));
    }

    // --- The outbound proxy (D205) ------------------------------------------

    use crate::proxy::test_proxy::{self, Mode};
    use crate::trust::test_pki::{Ca, tls_server};

    /// An origin on loopback answering every request with `body`, recording
    /// each request head it received.
    async fn origin(body: &'static str) -> (u16, Arc<Mutex<Vec<String>>>) {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let heads = Arc::new(Mutex::new(Vec::new()));
        let seen = heads.clone();
        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let seen = seen.clone();
                tokio::spawn(async move {
                    let mut reader = BufReader::new(socket);
                    let mut head = String::new();
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                            return;
                        }
                        head.push_str(&line);
                        if line == "\r\n" {
                            break;
                        }
                    }
                    seen.lock().unwrap().push(head);
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = reader.get_mut().write_all(response.as_bytes()).await;
                });
            }
        });
        (port, heads)
    }

    fn via(url: &str, no_proxy: &[&str]) -> Option<Arc<Proxies>> {
        let list: Vec<String> = no_proxy.iter().map(|s| s.to_string()).collect();
        Some(Proxies::from_flags(url, &list).unwrap().shared())
    }

    async fn proxied_text(client: &HttpClient, request: Outbound) -> Result<String, ProviderError> {
        let response = client.send(request).await?;
        assert_eq!(response.status, StatusCode::OK);
        let bytes = response.bytes(1 << 20).await?;
        Ok(String::from_utf8(bytes).unwrap())
    }

    #[tokio::test]
    async fn an_http_target_is_forwarded_in_absolute_form_with_credentials() {
        let (port, origin_heads) = origin("forwarded").await;
        let proxy = test_proxy::start(Mode::Auth("Basic dTpw")).await;
        let client = client().with_proxy(via(&format!("http://u:p@127.0.0.1:{}", proxy.port), &[]));
        let url = Url::parse(&format!("http://api.test:{port}/a?b=1")).unwrap();
        let mut request = Outbound::get(url);
        // The program's own value is replaced, never passed on beside ours.
        request.headers = vec![("proxy-authorization".into(), "Basic forged".into())];
        assert_eq!(proxied_text(&client, request).await.unwrap(), "forwarded");
        let heads = proxy.heads();
        assert_eq!(heads.len(), 1, "{heads:?}");
        assert!(
            heads[0].starts_with(&format!("GET http://api.test:{port}/a?b=1 HTTP/1.1\r\n")),
            "{}",
            heads[0]
        );
        assert!(
            heads[0].contains("proxy-authorization: Basic dTpw\r\n"),
            "{}",
            heads[0]
        );
        assert!(!heads[0].contains("forged"), "{}", heads[0]);
        assert_eq!(origin_heads.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn an_https_target_is_tunnelled_and_verified_inside_it() {
        let ca = Ca::new("Proxy Test CA");
        let (port, server) = tls_server(
            ca.leaf(&["api.test"]),
            &["http/1.1"],
            b"HTTP/1.1 200 OK\r\ncontent-length: 8\r\nconnection: close\r\n\r\ntunneled",
        )
        .await;
        let proxy = test_proxy::start(Mode::Open).await;
        let client = HttpClient::new(&ca.trust())
            .with_proxy(via(&format!("http://127.0.0.1:{}", proxy.port), &[]));
        let url = Url::parse(&format!("https://api.test:{port}/")).unwrap();
        assert_eq!(
            proxied_text(&client, Outbound::get(url)).await.unwrap(),
            "tunneled"
        );
        assert!(
            server.await.unwrap().is_some(),
            "the target's handshake failed"
        );
        let heads = proxy.heads();
        assert!(heads[0].starts_with(&format!("CONNECT api.test:{port} HTTP/1.1\r\n")));
    }

    #[tokio::test]
    async fn an_https_proxy_is_verified_against_the_trust_store() {
        let ca = Ca::new("Proxy Test CA");
        let (port, _origin_heads) = origin("over tls").await;
        let proxy = test_proxy::start_tls(Mode::Open, ca.leaf(&["localhost"])).await;
        let url = Url::parse(&format!("http://api.test:{port}/")).unwrap();
        let trusted = HttpClient::new(&ca.trust())
            .with_proxy(via(&format!("https://localhost:{}", proxy.port), &[]));
        assert_eq!(
            proxied_text(&trusted, Outbound::get(url.clone()))
                .await
                .unwrap(),
            "over tls"
        );
        // The same proxy, not trusted: refused at its certificate.
        let untrusted = client().with_proxy(via(&format!("https://localhost:{}", proxy.port), &[]));
        let Err(error) = untrusted.send(Outbound::get(url)).await else {
            panic!("an untrusted proxy was used");
        };
        assert_eq!(error.code(), Some(ErrorCode::Tls), "{error}");
    }

    #[tokio::test]
    async fn an_exception_goes_direct_and_the_proxy_sees_nothing() {
        let proxy = test_proxy::start(Mode::Open).await;
        let client = client().with_proxy(via(
            &format!("http://127.0.0.1:{}", proxy.port),
            &["api.test"],
        ));
        // Direct, `api.test` does not resolve: proof it did not go through.
        let url = Url::parse("http://api.test:9/").unwrap();
        let Err(error) = client.send(Outbound::get(url)).await else {
            panic!("an exception was proxied");
        };
        assert_eq!(error.code(), Some(ErrorCode::Dns), "{error}");
        assert!(proxy.heads().is_empty());
    }

    #[tokio::test]
    async fn the_allowlist_still_judges_the_target_not_the_proxy() {
        let proxy = test_proxy::start(Mode::Open).await;
        let client = client().with_proxy(via(&format!("http://127.0.0.1:{}", proxy.port), &[]));
        let mut request = Outbound::get(Url::parse("http://denied.test:80/").unwrap());
        request.allow = Some(Arc::new(
            HostAllowlist::parse(&["allowed.test".to_string()]).unwrap(),
        ));
        let Err(error) = client.send(request).await else {
            panic!("a denied host was fetched");
        };
        assert_eq!(error.code(), Some(ErrorCode::PermissionDenied), "{error}");
        assert!(
            proxy.heads().is_empty(),
            "the proxy was asked for a denied host"
        );
    }

    #[tokio::test]
    async fn a_refused_tunnel_reaches_fetch_as_err_proxy_tunnel() {
        let proxy = test_proxy::start(Mode::Refuse).await;
        let client = client().with_proxy(via(&format!("http://127.0.0.1:{}", proxy.port), &[]));
        let Err(error) = client
            .send(Outbound::get(Url::parse("https://api.test/").unwrap()))
            .await
        else {
            panic!("a refused tunnel succeeded");
        };
        assert_eq!(error.code(), Some(ErrorCode::ProxyTunnel), "{error}");
        assert!(error.to_string().contains("403"), "{error}");
    }

    // D206: a response the server cut short is ERR_CONNECTION_RESET with a
    // sentence, not hyper's chain of wrappers.
    #[tokio::test]
    async fn a_response_cut_short_is_a_coded_reset() {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 1024];
            let _ = socket.read(&mut buf).await;
            // Nothing at all: the connection closes before a response.
        });
        let url = Url::parse(&format!("http://127.0.0.1:{port}/")).unwrap();
        let Err(error) = client().send(Outbound::get(url)).await else {
            panic!("an empty answer was a response");
        };
        assert_eq!(error.code(), Some(ErrorCode::ConnectionReset), "{error}");
        assert_eq!(
            error.to_string(),
            "the connection closed before the response completed"
        );
    }
}
