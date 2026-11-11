//! The HTTP client's HTTP/2 (DECISIONS D217, phase 3): one task per
//! connection drives `es-runtime-http`'s connection state machine, and
//! requests reach it as commands. The pool ([`crate::h1_client`]) decides
//! which connection a request goes on; this module carries it there.
//!
//! A connection is handed out only once the server's first SETTINGS has
//! arrived, and only while it has a stream free under the server's
//! `SETTINGS_MAX_CONCURRENT_STREAMS` ([`H2Conn::try_reserve`]): past it, the
//! pool opens another connection rather than queueing on a full one.
//!
//! Flow control is tied to the reader: a response body's window is given back
//! as the caller takes each frame, so a slow reader holds at most a window per
//! stream. A request body is pulled only as the server's windows let it out.
//!
//! A request the server never processed — refused with `REFUSED_STREAM`, or
//! above the last stream a GOAWAY covered — fails as retryable: RFC 9113 §8.7
//! makes it safe to send again on another connection, whatever its method.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use es_runtime_common::ErrorCode as Code;
use es_runtime_http::h2::ErrorCode;
use es_runtime_http::h2::conn::{Config, Connection, Event};
use es_runtime_providers::{ByteStream, ProviderError};
use futures_util::future::BoxFuture;
use futures_util::stream::{FuturesUnordered, StreamExt};
use hyper::body::Frame;
use hyper::header::{HeaderMap, HeaderName, HeaderValue};
use hyper::{Method, Response, StatusCode, Version};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};

use crate::h1_client::{BoxError, Exchange, ResponseStream};
use crate::http_client::{Body, Conn};

/// How long a connection with no open stream is kept.
const IDLE: Duration = Duration::from_secs(90);

/// How long the server has to send its first SETTINGS after the preface.
const SETTINGS_WAIT: Duration = Duration::from_secs(10);

type FrameResult = Result<Frame<Bytes>, BoxError>;

/// Why a request failed, and whether the server is known not to have
/// processed it.
pub(crate) struct H2Failure {
    pub(crate) error: ProviderError,
    /// Never processed: safe to send again elsewhere (RFC 9113 §8.7).
    pub(crate) retry: bool,
}

fn reset_error(message: impl Into<String>) -> ProviderError {
    ProviderError::Coded {
        code: Code::ConnectionReset,
        message: message.into(),
    }
}

struct Shared {
    /// Streams reserved or open.
    active: AtomicU32,
    /// The server's `SETTINGS_MAX_CONCURRENT_STREAMS`.
    max: AtomicU32,
    /// The connection takes no new streams (ended, failed or going away).
    closed: AtomicBool,
}

/// A handle on one HTTP/2 connection; clones share it.
#[derive(Clone)]
pub(crate) struct H2Conn {
    cmd: mpsc::UnboundedSender<Command>,
    shared: Arc<Shared>,
}

/// A stream reserved on a connection, to be used for one request. Dropped
/// unused, it frees its slot.
pub(crate) struct Reservation {
    conn: H2Conn,
    used: bool,
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if !self.used {
            self.conn.shared.active.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

enum Command {
    Open {
        fields: Vec<(Vec<u8>, Vec<u8>)>,
        body: Body,
        reply: oneshot::Sender<Result<Head, H2Failure>>,
        /// Dropped by the caller when it stops caring — before the response,
        /// or by dropping the body — which resets the stream.
        cancel: oneshot::Receiver<()>,
    },
    Release(u32, usize),
}

struct Head {
    stream: u32,
    status: u16,
    headers: HeaderMap,
    body: mpsc::UnboundedReceiver<FrameResult>,
}

impl H2Conn {
    /// Starts HTTP/2 on `conn` (ALPN chose it) and returns once the server's
    /// first SETTINGS has arrived, so its limits are known.
    pub(crate) async fn start(conn: Conn) -> Result<H2Conn, ProviderError> {
        let (cmd, rx) = mpsc::unbounded_channel();
        let shared = Arc::new(Shared {
            active: AtomicU32::new(0),
            max: AtomicU32::new(0),
            closed: AtomicBool::new(false),
        });
        let (ready_tx, ready_rx) = oneshot::channel();
        let driver = Driver {
            conn: Connection::client(Config::default()),
            streams: HashMap::new(),
            wakes: FuturesUnordered::new(),
            shared: shared.clone(),
            ready: Some(ready_tx),
        };
        tokio::spawn(driver.run(conn, rx));
        match tokio::time::timeout(SETTINGS_WAIT, ready_rx).await {
            Ok(Ok(())) => Ok(H2Conn { cmd, shared }),
            Ok(Err(_)) => Err(reset_error(
                "request failed: the connection closed before HTTP/2 began",
            )),
            Err(_) => Err(ProviderError::Coded {
                code: Code::TimedOut,
                message: "request failed: the server sent no HTTP/2 SETTINGS".to_string(),
            }),
        }
    }

    /// Whether the connection takes no new streams.
    pub(crate) fn is_closed(&self) -> bool {
        self.shared.closed.load(Ordering::Acquire) || self.cmd.is_closed()
    }

    /// A stream on this connection, if it has one free.
    pub(crate) fn try_reserve(&self) -> Option<Reservation> {
        if self.is_closed() {
            return None;
        }
        let max = self.shared.max.load(Ordering::Acquire);
        self.shared
            .active
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < max).then_some(n + 1)
            })
            .ok()?;
        Some(Reservation {
            conn: self.clone(),
            used: false,
        })
    }
}

impl Reservation {
    /// Sends the request on the reserved stream; resolves with the response
    /// head, its body to follow.
    pub(crate) async fn send(
        mut self,
        exchange: &Exchange<'_>,
        secure: bool,
        body: Body,
    ) -> Result<Response<ResponseStream>, H2Failure> {
        let fields = request_fields(exchange, secure, &body);
        let (reply, reply_rx) = oneshot::channel();
        let (cancel_tx, cancel) = oneshot::channel();
        self.used = true; // the driver frees the slot from here on
        let cmd = self.conn.cmd.clone();
        if cmd
            .send(Command::Open {
                fields,
                body,
                reply,
                cancel,
            })
            .is_err()
        {
            self.conn.shared.active.fetch_sub(1, Ordering::AcqRel);
            return Err(H2Failure {
                error: reset_error("request failed: the connection closed"),
                retry: true,
            });
        }
        // A driver that ended with the command still queued never opened it.
        let head = reply_rx.await.map_err(|_| H2Failure {
            error: reset_error("request failed: the connection closed"),
            retry: true,
        })??;
        let status = StatusCode::from_u16(head.status).map_err(|_| H2Failure {
            error: ProviderError::Other("request failed: invalid status code".to_string()),
            retry: false,
        })?;
        let body = H2Body {
            rx: head.body,
            stream: head.stream,
            cmd,
            _cancel: cancel_tx,
        };
        let mut response = Response::new(http_body_util::BodyExt::boxed_unsync(body));
        *response.status_mut() = status;
        *response.version_mut() = Version::HTTP_2;
        *response.headers_mut() = head.headers;
        Ok(response)
    }
}

/// The request's field section as HTTP/2 sends it (RFC 9113 §8.3.1): the
/// pseudo-header fields, then the headers without those that describe an
/// HTTP/1.1 connection, and the body's length when it has one.
fn request_fields(exchange: &Exchange<'_>, secure: bool, body: &Body) -> Vec<(Vec<u8>, Vec<u8>)> {
    let path = if exchange.target.starts_with('/') || exchange.target == "*" {
        exchange.target
    } else {
        "/"
    };
    let mut fields = vec![
        (
            b":method".to_vec(),
            exchange.method.as_str().as_bytes().to_vec(),
        ),
        (
            b":scheme".to_vec(),
            if secure { &b"https"[..] } else { &b"http"[..] }.to_vec(),
        ),
        (b":authority".to_vec(), exchange.host.as_bytes().to_vec()),
        (b":path".to_vec(), path.as_bytes().to_vec()),
    ];
    let mut has_length = false;
    for (name, value) in exchange.headers {
        match name.as_str() {
            "host" | "connection" | "keep-alive" | "proxy-connection" | "transfer-encoding"
            | "upgrade" => continue,
            "te" if value.as_bytes() != b"trailers" => continue,
            "content-length" => has_length = true,
            _ => {}
        }
        fields.push((name.as_str().as_bytes().to_vec(), value.as_bytes().to_vec()));
    }
    if !has_length {
        match body {
            Body::Bytes(b) => {
                fields.push((b"content-length".to_vec(), b.len().to_string().into_bytes()))
            }
            Body::Empty
                if matches!(*exchange.method, Method::POST | Method::PUT | Method::PATCH) =>
            {
                fields.push((b"content-length".to_vec(), b"0".to_vec()));
            }
            _ => {}
        }
    }
    fields
}

/// A response body, fed by the connection's task; each frame taken gives
/// its bytes of window back.
struct H2Body {
    rx: mpsc::UnboundedReceiver<FrameResult>,
    stream: u32,
    cmd: mpsc::UnboundedSender<Command>,
    /// Dropped with the body: if the response is not finished, the stream
    /// is reset.
    _cancel: oneshot::Sender<()>,
}

impl hyper::body::Body for H2Body {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, BoxError>>> {
        let polled = self.rx.poll_recv(cx);
        if let Poll::Ready(Some(Ok(frame))) = &polled
            && let Some(data) = frame.data_ref()
            && !data.is_empty()
        {
            let _ = self.cmd.send(Command::Release(self.stream, data.len()));
        }
        polled
    }
}

/// One request's state on the connection.
struct Req {
    /// Where the response head goes, until it has arrived.
    reply: Option<oneshot::Sender<Result<Head, H2Failure>>>,
    /// Where the body goes, until it has ended.
    body_tx: Option<mpsc::UnboundedSender<FrameResult>>,
    /// Request body bytes waiting for window.
    pending: Vec<u8>,
    /// The request body ends with `pending`.
    end: bool,
    /// A streamed request body, parked between chunks.
    upload: Option<ByteStream>,
    /// The request has gone out whole.
    sent: bool,
}

enum Wake {
    Read(std::io::Result<usize>),
    Cmd(Option<Command>),
    Chunk(u32, Option<Result<Vec<u8>, ProviderError>>, ByteStream),
    Cancelled(u32),
    Idle,
}

struct Driver {
    conn: Connection,
    streams: HashMap<u32, Req>,
    wakes: FuturesUnordered<BoxFuture<'static, Wake>>,
    shared: Arc<Shared>,
    ready: Option<oneshot::Sender<()>>,
}

fn pull(stream: u32, mut body: ByteStream) -> BoxFuture<'static, Wake> {
    Box::pin(async move {
        let item = body.next().await;
        Wake::Chunk(stream, item, body)
    })
}

async fn sleep_or_never(idle: bool) {
    if idle {
        tokio::time::sleep(IDLE).await;
    } else {
        std::future::pending::<()>().await;
    }
}

impl Driver {
    async fn run(mut self, mut io: Conn, mut rx: mpsc::UnboundedReceiver<Command>) {
        let mut buf: Vec<u8> = Vec::with_capacity(16 * 1024);
        loop {
            let n = self.conn.output().len();
            if n > 0 {
                let written = io.write_all(self.conn.output()).await;
                self.conn.consume_output(n);
                if written.is_err() || io.flush().await.is_err() {
                    break;
                }
            }
            if self.conn.is_failed() {
                break;
            }
            if self.shared.closed.load(Ordering::Acquire) && self.streams.is_empty() {
                break;
            }
            let idle = self.streams.is_empty();
            let wake = tokio::select! {
                n = async { buf.reserve(16 * 1024); io.read_buf(&mut buf).await } => Wake::Read(n),
                c = rx.recv() => Wake::Cmd(c),
                Some(w) = self.wakes.next() => w,
                () = sleep_or_never(idle) => Wake::Idle,
            };
            match wake {
                Wake::Read(Ok(0) | Err(_)) => break,
                Wake::Read(Ok(_)) => match self.conn.receive(&buf) {
                    Ok(used) => {
                        buf.drain(..used);
                    }
                    Err(e) => {
                        tracing::debug!(target: "runtime::fetch", error = %e, "HTTP/2 connection ended with an error");
                        // GOAWAY is queued: write it, then end.
                        let n = self.conn.output().len();
                        let _ = io.write_all(self.conn.output()).await;
                        self.conn.consume_output(n);
                        break;
                    }
                },
                // Every handle is gone: no request can come any more.
                Wake::Cmd(None) => {
                    self.shared.closed.store(true, Ordering::Release);
                    if self.streams.is_empty() {
                        self.conn.go_away(ErrorCode::NoError);
                    }
                }
                Wake::Cmd(Some(Command::Open {
                    fields,
                    body,
                    reply,
                    cancel,
                })) => self.open(fields, body, reply, cancel),
                Wake::Cmd(Some(Command::Release(stream, n))) => self.conn.release(stream, n),
                Wake::Chunk(stream, item, upload) => self.chunk(stream, item, upload),
                Wake::Cancelled(stream) => {
                    if self.streams.contains_key(&stream) {
                        self.conn.reset(stream, ErrorCode::Cancel);
                        self.remove(stream);
                    }
                }
                Wake::Idle => {
                    // Closed first, so no reservation lands on a connection
                    // about to go.
                    self.shared.closed.store(true, Ordering::Release);
                    self.conn.go_away(ErrorCode::NoError);
                }
            }
            // The server's limit, as its latest SETTINGS states it; the
            // connection is handed out once the first has arrived.
            if self.conn.peer_settings_received() {
                self.shared
                    .max
                    .store(self.conn.peer_max_concurrent_streams(), Ordering::Release);
                if let Some(ready) = self.ready.take() {
                    let _ = ready.send(());
                }
            }
            while let Some(event) = self.conn.poll_event() {
                self.event(event);
            }
        }
        // However it ended: no new streams, and every open one fails.
        self.shared.closed.store(true, Ordering::Release);
        let ids: Vec<u32> = self.streams.keys().copied().collect();
        for id in ids {
            self.fail(
                id,
                reset_error("request failed: the connection closed before the response completed"),
                false,
            );
        }
        // Commands that arrived too late were never opened.
        rx.close();
        while let Ok(Command::Open { reply, .. }) = rx.try_recv() {
            self.shared.active.fetch_sub(1, Ordering::AcqRel);
            let _ = reply.send(Err(H2Failure {
                error: reset_error("request failed: the connection closed"),
                retry: true,
            }));
        }
    }

    fn open(
        &mut self,
        fields: Vec<(Vec<u8>, Vec<u8>)>,
        body: Body,
        reply: oneshot::Sender<Result<Head, H2Failure>>,
        cancel: oneshot::Receiver<()>,
    ) {
        let end = matches!(body, Body::Empty);
        let pairs = fields.iter().map(|(n, v)| (n.as_slice(), v.as_slice()));
        let stream = match self.conn.open_stream(pairs, end) {
            Ok(stream) => stream,
            Err(e) => {
                self.shared.active.fetch_sub(1, Ordering::AcqRel);
                let _ = reply.send(Err(H2Failure {
                    error: reset_error(format!("request failed: {e}")),
                    retry: true,
                }));
                return;
            }
        };
        let mut req = Req {
            reply: Some(reply),
            body_tx: None,
            pending: Vec::new(),
            end: false,
            upload: None,
            sent: end,
        };
        match body {
            Body::Empty => {}
            Body::Bytes(bytes) => {
                req.pending = bytes.to_vec();
                req.end = true;
            }
            Body::Stream(upload) => self.wakes.push(pull(stream, upload)),
        }
        self.streams.insert(stream, req);
        self.wakes.push(Box::pin(async move {
            let _ = cancel.await;
            Wake::Cancelled(stream)
        }));
        self.flush(stream);
    }

    fn chunk(
        &mut self,
        stream: u32,
        item: Option<Result<Vec<u8>, ProviderError>>,
        upload: ByteStream,
    ) {
        let Some(req) = self.streams.get_mut(&stream) else {
            return;
        };
        match item {
            Some(Ok(chunk)) => {
                req.pending = chunk;
                req.upload = Some(upload);
            }
            None => req.end = true,
            Some(Err(e)) => {
                // The upload failed: the request is abandoned.
                self.conn.reset(stream, ErrorCode::Cancel);
                self.fail(stream, e, false);
                return;
            }
        }
        self.flush(stream);
    }

    /// Sends what fits of the request body; pulls the next chunk once the
    /// last is out.
    fn flush(&mut self, stream: u32) {
        let Some(req) = self.streams.get_mut(&stream) else {
            return;
        };
        if req.sent || (req.pending.is_empty() && req.upload.is_none() && !req.end) {
            return;
        }
        match self.conn.send_data(stream, &req.pending, req.end) {
            Ok(n) => {
                req.pending.drain(..n);
            }
            Err(_) => {
                // The stream closed for sending (the server answered and
                // reset it): nothing more to send.
                req.sent = true;
                req.upload = None;
                self.maybe_done(stream);
                return;
            }
        }
        if !req.pending.is_empty() {
            return; // waits for window
        }
        if let Some(upload) = req.upload.take() {
            self.wakes.push(pull(stream, upload));
        } else if req.end {
            req.sent = true;
            self.maybe_done(stream);
        }
    }

    fn event(&mut self, event: Event) {
        match event {
            Event::Headers {
                stream,
                pseudo,
                fields,
                end_stream,
            } => {
                let status = pseudo.status.unwrap_or(0);
                if status < 200 {
                    return; // interim: the final response follows
                }
                let Some(req) = self.streams.get_mut(&stream) else {
                    return;
                };
                let Some(reply) = req.reply.take() else {
                    return;
                };
                let mut headers = HeaderMap::with_capacity(fields.len());
                for (name, value) in fields {
                    if let (Ok(name), Ok(value)) = (
                        HeaderName::from_bytes(&name),
                        HeaderValue::from_bytes(&value),
                    ) {
                        headers.append(name, value);
                    }
                }
                let (tx, rx) = mpsc::unbounded_channel();
                if !end_stream {
                    req.body_tx = Some(tx);
                }
                let head = Head {
                    stream,
                    status,
                    headers,
                    body: rx,
                };
                if reply.send(Ok(head)).is_err() {
                    // The caller gave up meanwhile.
                    self.conn.reset(stream, ErrorCode::Cancel);
                    self.remove(stream);
                    return;
                }
                self.maybe_done(stream);
            }
            Event::Data {
                stream,
                data,
                end_stream,
            } => {
                let n = data.len();
                let delivered = self
                    .streams
                    .get(&stream)
                    .and_then(|r| r.body_tx.as_ref())
                    .is_some_and(|tx| {
                        n == 0 || tx.send(Ok(Frame::data(Bytes::from(data)))).is_ok()
                    });
                if !delivered && n > 0 {
                    // Nobody reads it: its window comes back now.
                    self.conn.release(stream, n);
                }
                if end_stream {
                    self.body_ended(stream);
                }
            }
            Event::Trailers { stream, fields } => {
                if let Some(tx) = self.streams.get(&stream).and_then(|r| r.body_tx.as_ref()) {
                    let mut map = HeaderMap::with_capacity(fields.len());
                    for (name, value) in fields {
                        if let (Ok(name), Ok(value)) = (
                            HeaderName::from_bytes(&name),
                            HeaderValue::from_bytes(&value),
                        ) {
                            map.append(name, value);
                        }
                    }
                    let _ = tx.send(Ok(Frame::trailers(map)));
                }
                self.body_ended(stream);
            }
            Event::Reset { stream, code } => {
                let retry = code == ErrorCode::RefusedStream;
                self.fail(
                    stream,
                    reset_error(format!(
                        "request failed: the server reset the stream ({code:?})"
                    )),
                    retry,
                );
            }
            Event::SendCapacity { stream } => {
                let ids: Vec<u32> = if stream == 0 {
                    self.streams.keys().copied().collect()
                } else {
                    vec![stream]
                };
                for id in ids {
                    self.flush(id);
                }
            }
            Event::GoAway { last_stream, .. } => {
                self.shared.closed.store(true, Ordering::Release);
                // Streams above `last_stream` were not processed (§6.8).
                let unprocessed: Vec<u32> = self
                    .streams
                    .keys()
                    .copied()
                    .filter(|&id| id > last_stream)
                    .collect();
                for id in unprocessed {
                    self.conn.reset(id, ErrorCode::Cancel);
                    self.fail(
                        id,
                        reset_error("request failed: the server is going away"),
                        true,
                    );
                }
            }
        }
    }

    fn body_ended(&mut self, stream: u32) {
        if let Some(req) = self.streams.get_mut(&stream) {
            req.body_tx = None;
        }
        self.maybe_done(stream);
    }

    /// Fails `stream` with `error`: its caller, if still waiting for the
    /// head, or its body's reader.
    fn fail(&mut self, stream: u32, error: ProviderError, retry: bool) {
        let Some(mut req) = self.streams.remove(&stream) else {
            return;
        };
        self.shared.active.fetch_sub(1, Ordering::AcqRel);
        if let Some(reply) = req.reply.take() {
            let _ = reply.send(Err(H2Failure { error, retry }));
        } else if let Some(tx) = req.body_tx.take() {
            let _ = tx.send(Err(Box::new(error)));
        }
    }

    /// Done once the response has been handed over whole and the request has
    /// gone out whole.
    fn maybe_done(&mut self, stream: u32) {
        if self
            .streams
            .get(&stream)
            .is_some_and(|r| r.reply.is_none() && r.body_tx.is_none() && r.sent)
        {
            self.remove(stream);
        }
    }

    fn remove(&mut self, stream: u32) {
        if self.streams.remove(&stream).is_some() {
            self.shared.active.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

/// `fetch` over HTTP/2 against our own server driver on TLS: how many
/// connections a burst of requests makes, that the server's stream limit is
/// kept, and that requests the server never processed are sent again.
#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use es_runtime_http::h2::conn::Config;
    use es_runtime_providers::{HttpServerBody, HttpServerResponse};
    use futures_util::StreamExt;
    use hyper::{Method, StatusCode, Version};
    use tokio::net::TcpListener;
    use tokio::sync::{mpsc, watch};
    use tokio_rustls::TlsAcceptor;
    use tokio_rustls::rustls::ServerConfig;
    use tokio_rustls::rustls::crypto::aws_lc_rs;
    use url::Url;

    use crate::h2_conn::{ConnContext, serve};
    use crate::http_client::{Body, HttpClient, Outbound};
    use crate::system_http::Pending;
    use crate::trust::test_pki::Ca;

    struct Server {
        port: u16,
        ca: Ca,
        /// TCP connections accepted.
        connections: Arc<AtomicUsize>,
        /// The most requests the guest held at once on any one connection.
        peak_per_connection: Arc<AtomicUsize>,
        /// Requests the guest saw.
        requests: Arc<AtomicUsize>,
    }

    /// A TLS server speaking `alpn`, answering each request after `hold`.
    async fn server(config: Config, hold: Duration, alpn: &[&str]) -> Server {
        server_draining(config, hold, alpn, 0).await
    }

    /// The same, with its first `drain` connections going away as soon as
    /// they start — what a server restarting does to the connections it has.
    async fn server_draining(
        config: Config,
        hold: Duration,
        alpn: &[&str],
        drain: usize,
    ) -> Server {
        let ca = Ca::new("H2 Client CA");
        let leaf = ca.leaf(&["localhost"]);
        let mut tls = ServerConfig::builder_with_provider(Arc::new(aws_lc_rs::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(leaf.chain, leaf.key)
            .unwrap();
        tls.alpn_protocols = alpn.iter().map(|p| p.as_bytes().to_vec()).collect();
        let acceptor = TlsAcceptor::from(Arc::new(tls));
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let connections = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let requests = Arc::new(AtomicUsize::new(0));
        let (c, p, r) = (connections.clone(), peak.clone(), requests.clone());
        let h2 = alpn.contains(&"h2");
        tokio::spawn(async move {
            loop {
                let Ok((tcp, _)) = listener.accept().await else {
                    return;
                };
                c.fetch_add(1, Ordering::SeqCst);
                let acceptor = acceptor.clone();
                let (p, r, c) = (p.clone(), r.clone(), c.clone());
                tokio::spawn(async move {
                    let Ok(tls) = acceptor.accept(tcp).await else {
                        return;
                    };
                    let (tx, mut rx) = mpsc::channel::<Pending>(1024);
                    let draining = c.load(Ordering::SeqCst) <= drain;
                    let (_drain, drain_rx) = watch::channel(draining);
                    let held = Arc::new(AtomicUsize::new(0));
                    tokio::spawn(async move {
                        while let Some((request, answer, _, _)) = rx.recv().await {
                            r.fetch_add(1, Ordering::SeqCst);
                            let now = held.fetch_add(1, Ordering::SeqCst) + 1;
                            p.fetch_max(now, Ordering::SeqCst);
                            let held = held.clone();
                            tokio::spawn(async move {
                                if let HttpServerBody::Stream(mut body) = request.body {
                                    while body.next().await.is_some() {}
                                }
                                tokio::time::sleep(hold).await;
                                held.fetch_sub(1, Ordering::SeqCst);
                                let _ = answer.send(HttpServerResponse {
                                    status: 200,
                                    headers: vec![("content-length".into(), "99".into())],
                                    body: HttpServerBody::Bytes(request.method.into_bytes()),
                                    trailers: None,
                                });
                            });
                        }
                    });
                    let ctx = ConnContext {
                        tx,
                        origin: Arc::from(format!("https://127.0.0.1:{port}").as_str()),
                        peer_host: Arc::from("127.0.0.1"),
                        peer_port: 0,
                        peer_certificate: None,
                        drain_rx,
                        keep_alive: None,
                        body_limit: None,
                        config,
                    };
                    if h2 {
                        serve(tls, Vec::new(), ctx).await;
                    } else {
                        let ctx = crate::h1_conn::ConnContext {
                            tx: ctx.tx,
                            origin: ctx.origin,
                            peer_host: ctx.peer_host,
                            peer_port: 0,
                            peer_certificate: None,
                            drain_rx: ctx.drain_rx,
                            header_read: None,
                            body_limit: None,
                            limits: es_runtime_http::h1::Limits::default(),
                        };
                        crate::h1_conn::serve(tls, Vec::new(), ctx).await;
                    }
                });
            }
        });
        Server {
            port,
            ca,
            connections,
            peak_per_connection: peak,
            requests,
        }
    }

    async fn burst(
        client: &HttpClient,
        port: u16,
        n: usize,
    ) -> Vec<(StatusCode, Version, Vec<u8>)> {
        let url = Url::parse(&format!("https://localhost:{port}/")).unwrap();
        let requests = (0..n).map(|_| {
            let client = client.clone();
            let url = url.clone();
            async move {
                let response = client.send(Outbound::get(url)).await.unwrap();
                let mut body = Vec::new();
                let mut stream = response.body;
                while let Some(chunk) = stream.next().await {
                    body.extend(chunk.unwrap());
                }
                (response.status, response.version, body)
            }
        });
        futures_util::future::join_all(requests).await
    }

    /// Gap 1: concurrent first requests to a new origin wait for the one
    /// connect in flight and share it, rather than each opening its own.
    #[tokio::test]
    async fn a_burst_to_a_new_origin_shares_one_connection() {
        let s = server(Config::default(), Duration::from_millis(50), &["h2"]).await;
        let client = HttpClient::new(&s.ca.trust());
        let answers = burst(&client, s.port, 40).await;
        assert!(
            answers
                .iter()
                .all(|(st, v, b)| *st == StatusCode::OK && *v == Version::HTTP_2 && b == b"GET")
        );
        assert_eq!(
            s.connections.load(Ordering::SeqCst),
            1,
            "one connection for the burst"
        );
        assert_eq!(
            s.peak_per_connection.load(Ordering::SeqCst),
            40,
            "all on it at once"
        );
        // And the next wave reuses it.
        burst(&client, s.port, 10).await;
        assert_eq!(s.connections.load(Ordering::SeqCst), 1);
    }

    /// Gap 2: the server's SETTINGS_MAX_CONCURRENT_STREAMS is kept; past it,
    /// another connection opens instead of queueing on a full one.
    #[tokio::test]
    async fn the_servers_stream_limit_is_kept_with_more_connections() {
        let config = Config {
            max_concurrent_streams: 4,
            ..Config::default()
        };
        let s = server(config, Duration::from_millis(100), &["h2"]).await;
        let client = HttpClient::new(&s.ca.trust());
        let answers = burst(&client, s.port, 10).await;
        assert!(answers.iter().all(|(st, ..)| *st == StatusCode::OK));
        assert!(
            s.peak_per_connection.load(Ordering::SeqCst) <= 4,
            "a connection carried more streams than its server allows"
        );
        assert_eq!(
            s.connections.load(Ordering::SeqCst),
            3,
            "10 requests, 4 per connection"
        );
    }

    /// An origin that turns out to be HTTP/1.1 waits only for its first
    /// connect, then connects in parallel — concurrency there *is* more
    /// connections — and is known from then on.
    #[tokio::test]
    async fn an_http1_origin_still_gets_parallel_connections() {
        let s = server(Config::default(), Duration::from_millis(50), &["http/1.1"]).await;
        let client = HttpClient::new(&s.ca.trust());
        let answers = burst(&client, s.port, 8).await;
        assert!(
            answers
                .iter()
                .all(|(st, v, _)| *st == StatusCode::OK && *v == Version::HTTP_11)
        );
        assert!(
            s.connections.load(Ordering::SeqCst) >= 2,
            "HTTP/1.1 requests must not queue on one connection"
        );
    }

    /// A response to HEAD states the length a GET would have had and carries
    /// no body; it is not a short body.
    #[tokio::test]
    async fn a_head_response_with_a_length_is_not_a_short_body() {
        let s = server(Config::default(), Duration::ZERO, &["h2"]).await;
        let client = HttpClient::new(&s.ca.trust());
        let url = Url::parse(&format!("https://localhost:{}/", s.port)).unwrap();
        let mut head = Outbound::get(url);
        head.method = Method::HEAD;
        let response = client.send(head).await.unwrap();
        assert_eq!(response.status, StatusCode::OK);
        let mut body = response.body;
        assert!(body.next().await.is_none(), "no body, and no error");
    }

    /// A server going away before it took the request: the request was never
    /// processed (RFC 9113 §8.7), so even a POST is sent again, once, on a
    /// fresh connection.
    #[tokio::test]
    async fn a_request_the_server_never_processed_is_sent_again() {
        let s = server_draining(Config::default(), Duration::ZERO, &["h2"], 1).await;
        let client = HttpClient::new(&s.ca.trust());
        let url = Url::parse(&format!("https://localhost:{}/", s.port)).unwrap();
        let mut post = Outbound::get(url);
        post.method = Method::POST;
        post.body = Body::Bytes(b"once".to_vec().into());
        let response = client.send(post).await.unwrap();
        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(s.connections.load(Ordering::SeqCst), 2);
        assert_eq!(
            s.requests.load(Ordering::SeqCst),
            1,
            "processed exactly once"
        );
    }

    /// A buffered upload goes out with its length, under the server's windows.
    #[tokio::test]
    async fn an_upload_larger_than_the_window_arrives_whole() {
        let s = server(Config::default(), Duration::ZERO, &["h2"]).await;
        let client = HttpClient::new(&s.ca.trust());
        let url = Url::parse(&format!("https://localhost:{}/", s.port)).unwrap();
        let mut post = Outbound::get(url);
        post.method = Method::POST;
        post.body = Body::Bytes(vec![7u8; 3 << 20].into());
        let response = client.send(post).await.unwrap();
        assert_eq!(response.status, StatusCode::OK);
        let mut text = Vec::new();
        let mut body = response.body;
        while let Some(chunk) = body.next().await {
            text.extend(chunk.unwrap());
        }
        assert_eq!(text, b"POST");
        assert_eq!(s.requests.load(Ordering::SeqCst), 1);
    }
}
