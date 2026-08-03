//! OS-backed [`HttpServerProvider`] — a hyper HTTP/1.1 + HTTP/2 server for
//! `runtime:http`.
//!
//! Each accepted connection is served on its own spawned task. hyper parses the
//! request and, for each one, hands `(request, oneshot)` to a per-server channel
//! and `await`s the oneshot for the response; the runtime drains that channel via
//! [`next_request`](HttpServerProvider::next_request) and completes each request
//! with [`respond`](HttpServerProvider::respond). This handoff lets hyper run
//! across the reactor's threads while the single-threaded JS isolate produces
//! responses one at a time. Bodies **stream** in both directions: the inbound
//! `Incoming` body crosses as an [`HttpServerBody::Stream`] the guest pulls
//! chunk-by-chunk (hyper keeps feeding it while the connection task awaits the
//! response oneshot), and a streamed response body is written with chunked
//! transfer-encoding as the guest produces it.
//!
//! Each request also carries a "was the response delivered?" half that
//! [`request_disconnected`](HttpServerProvider::request_disconnected) reads,
//! which is what backs the guest's `request.signal`: hyper dropping the service
//! future — what a vanished client does to it — drops that half unsent, and the
//! watcher reads the drop as the disconnect.
//!
//! The protocol version is the client's choice, not the listener's: every
//! connection is served through hyper-util's version-detecting builder, so an
//! HTTP/1.1 client and an HTTP/2 client are both answered on the same port by
//! the same handler. Over TLS the choice is ALPN (`serve` advertises `h2` and
//! `http/1.1` unless the guest narrows it); on a cleartext port it is the HTTP/2
//! connection preface, which is prior-knowledge h2c. Nothing above the socket
//! changes with the version — the handoff already answers requests out of order,
//! which is what HTTP/2 multiplexing needs.

use std::collections::HashMap;
use std::convert::Infallible;
use std::future::Future;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use es_runtime_providers::{
    BoxFuture, HttpServeOptions, HttpServerBody, HttpServerProvider, HttpServerRequest,
    HttpServerResponse, HttpTimeouts, ProviderError, SocketInfo,
};
use futures_util::StreamExt;
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt, Full, StreamBody};
use hyper::body::{Body as _, Frame, Incoming};
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use hyper_util::server::conn::auto;
use tokio::net::TcpListener;
use tokio::sync::{Notify, mpsc, oneshot, watch};
use tokio::task::AbortHandle;

use crate::accept_backoff::AcceptBackoff;
use crate::first_byte::FirstByteTimeout;

/// The response body handed to hyper: buffered (`Full`) or streamed
/// (`StreamBody`), erased behind one type so both share the service signature.
/// Unsync because the guest's [`ByteStream`](es_runtime_providers::ByteStream)
/// is `Send` but not `Sync` — hyper only needs `Send` here.
type OutBody = UnsyncBoxBody<Bytes, ProviderError>;

/// One inbound request, the channel that carries its response back to hyper, and
/// the half that reports whether the peer outlived the wait for that response.
///
/// The `delivered` receiver is the disconnect signal, read backwards: the
/// service future *sends* on it once it has the guest's response in hand, and
/// dropping it without sending is what a dropped connection looks like. So
/// `Ok(())` means delivered and `Err(_)` means the client went away — no
/// polling, and nothing to clean up on either path.
type Pending = (
    HttpServerRequest,
    oneshot::Sender<HttpServerResponse>,
    oneshot::Receiver<()>,
);

/// Per-server shutdown handle, kept in a side map that `next_request` never
/// removes — so `close` can stop a server even while a `next_request` await has
/// the request receiver checked out. Aborting the acceptor stops new
/// connections; the notify wakes the parked `next_request` so it returns `None`.
struct Control {
    acceptor: AbortHandle,
    shutdown: Arc<Notify>,
    /// Level-triggered "begin a graceful shutdown" flag the live connection
    /// tasks watch. A `Notify` would not do: it is edge-triggered, so a
    /// connection accepted a moment before the signal could miss it and then
    /// hold the process open until its client happened to hang up.
    draining: watch::Sender<bool>,
}

/// Live connection accounting, shared by every server in one registry.
///
/// A response is *handed to* hyper by `respond`, which is not the same as
/// written to the socket — hyper needs to be polled again for that. So a caller
/// draining for shutdown cannot ask "are all responses sent?"; it asks "are all
/// connections finished?", which is this.
#[derive(Clone, Default)]
struct LiveConnections {
    count: Arc<AtomicU64>,
    idle: Arc<Notify>,
}

impl LiveConnections {
    fn enter(&self) {
        self.count.fetch_add(1, Ordering::SeqCst);
    }

    fn leave(&self) {
        if self.count.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.idle.notify_waiters();
        }
    }

    /// Resolves once no connection is live. Returns `false` if `grace` ran out
    /// first, so a caller can report that it gave up rather than drained.
    async fn wait_idle(&self, grace: std::time::Duration) -> bool {
        let deadline = tokio::time::Instant::now() + grace;
        loop {
            if self.count.load(Ordering::SeqCst) == 0 {
                return true;
            }
            // Register before re-checking, so a `leave` that lands between the
            // two is not a missed wakeup.
            let idle = self.idle.notified();
            if self.count.load(Ordering::SeqCst) == 0 {
                return true;
            }
            if tokio::time::timeout_at(deadline, idle).await.is_err() {
                return self.count.load(Ordering::SeqCst) == 0;
            }
        }
    }
}

/// An [`HttpServerProvider`] over a hyper HTTP/1.1 + HTTP/2 server. The `Arc`s are cloned
/// into each returned future so the futures stay `'static`.
#[derive(Clone, Default)]
pub struct SystemHttpServer {
    requests: Arc<Mutex<HashMap<u64, mpsc::Receiver<Pending>>>>,
    controls: Arc<Mutex<HashMap<u64, Control>>>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<HttpServerResponse>>>>,
    /// Per-request "was the response delivered?" halves, read by
    /// [`request_disconnected`](HttpServerProvider::request_disconnected). An
    /// entry is taken by the first watcher; a request nobody watches has its
    /// entry dropped with the server.
    delivered: Arc<Mutex<HashMap<u64, oneshot::Receiver<()>>>>,
    live: LiveConnections,
    next_id: Arc<AtomicU64>,
    /// Addresses `serve` may bind (`--allow-listen=<addresses>`). `None` ⇒ any.
    allow_listen: Option<Arc<crate::HostAllowlist>>,
}

impl SystemHttpServer {
    /// Builds an empty server registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Restricts `serve` to `allow` — `esrun --allow-listen=<addresses>` (D38).
    /// The same list `runtime:net` `listen` consults: `listen` is one
    /// capability whichever API claims the port.
    #[must_use]
    pub fn with_listen_allowlist(mut self, allow: crate::HostAllowlist) -> Self {
        self.allow_listen = Some(Arc::new(allow));
        self
    }

    fn id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Stops every live server from accepting, and returns how many there were.
    ///
    /// This is the first half of a graceful shutdown: new connections stop, but
    /// requests already handed to the guest are untouched — their response
    /// channels stay open, so a handler still in flight can finish and reply.
    /// The caller keeps driving until that work drains.
    ///
    /// The count is what tells a caller whether a drain is worth waiting for at
    /// all: zero means there was never a server, so there is nothing in flight
    /// to protect and no reason to delay exiting.
    pub fn shutdown_all(&self) -> usize {
        let controls: Vec<_> = self
            .controls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain()
            .collect();
        for (id, control) in &controls {
            // Dropping the request receiver ends idle keep-alive connections;
            // aborting the acceptor stops new ones; the notify wakes a parked
            // `next_requests` so the guest's accept loop finishes; and the
            // drain flag puts every live connection into hyper's graceful
            // shutdown, which answers what is in flight and then closes.
            self.requests
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(id);
            control.acceptor.abort();
            control.shutdown.notify_waiters();
            let _ = control.draining.send(true);
        }
        controls.len()
    }

    /// Resolves once every connection has finished, or after `grace`; `true` if
    /// they drained, `false` if the deadline ran out first.
    ///
    /// Pairs with [`shutdown_all`](Self::shutdown_all). Waiting for the guest's
    /// work to finish is not enough on its own: the guest hands a response to
    /// [`respond`](HttpServerProvider::respond) and moves on, and hyper has to be
    /// polled again to put the bytes on the socket. Exiting between those two
    /// points is exactly how an in-flight request turns into an empty reply.
    pub async fn wait_for_idle(&self, grace: std::time::Duration) -> bool {
        self.live.wait_idle(grace).await
    }
}

fn err(e: impl ToString) -> ProviderError {
    ProviderError::Other(e.to_string())
}

fn info_of(local: Option<SocketAddr>) -> SocketInfo {
    SocketInfo {
        remote_address: String::new(),
        remote_port: 0,
        local_address: local.map(|a| a.ip().to_string()).unwrap_or_default(),
        local_port: local.map(|a| a.port()).unwrap_or(0),
        alpn: None,
    }
}

/// Turns a parsed hyper request into the [`HttpServerRequest`] handoff shape
/// without touching the body: the `Incoming` body crosses as a chunk stream the
/// guest pulls (hyper feeds it while the connection task awaits the response).
/// The absolute URL is reconstructed from the listener's scheme plus the request
/// authority — the `Host` header, or on HTTP/2 the `:authority` pseudo-header
/// hyper puts in the URI — falling back to the bound address.
fn to_server_request(req: Request<Incoming>, origin: &str) -> HttpServerRequest {
    let method = req.method().to_string();
    // `origin` is "<scheme>://<bound address>", the fallback when a request
    // names no authority. The authority replaces only the host part — the scheme
    // is the listener's, and a client cannot talk this into claiming https:.
    let (scheme, authority) = origin.split_once("://").unwrap_or(("http", origin));
    // HTTP/2 has no `Host` header: the client sends `:authority`, which hyper
    // parses into the URI. Reading only the header would leave every h2 request
    // reporting the bound address as its host.
    let host = req
        .headers()
        .get(hyper::header::HOST)
        .and_then(|v| v.to_str().ok())
        .filter(|h| !h.is_empty())
        .or_else(|| req.uri().authority().map(|a| a.as_str()))
        .unwrap_or(authority);
    let path = req
        .uri()
        .path_and_query()
        .map(|p| p.as_str())
        .unwrap_or("/");
    let url = format!("{scheme}://{host}{path}");
    let headers = req
        .headers()
        .iter()
        .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();
    let incoming = req.into_body();
    let body = if incoming.is_end_stream() {
        HttpServerBody::Empty
    } else {
        HttpServerBody::Stream(Box::pin(incoming.into_data_stream().map(|item| {
            item.map(|bytes| bytes.to_vec())
                .map_err(|e| ProviderError::Other(e.to_string()))
        })))
    };
    HttpServerRequest {
        method,
        url,
        headers,
        body,
    }
}

/// Builds the hyper response from the guest's [`HttpServerResponse`]. A buffered
/// body goes out as `Full` (hyper sets `Content-Length`); a streamed body goes
/// out as a `StreamBody` (hyper uses chunked transfer-encoding), and a chunk
/// `Err` aborts the connection mid-body — the only honest signal once the status
/// line is on the wire. Guest-supplied framing headers are dropped either way to
/// avoid conflicting with what hyper computes.
fn build_response(resp: HttpServerResponse) -> Response<OutBody> {
    let status = StatusCode::from_u16(resp.status).unwrap_or(StatusCode::OK);
    let mut builder = Response::builder().status(status);
    for (name, value) in &resp.headers {
        let lower = name.to_ascii_lowercase();
        if lower == "content-length" || lower == "transfer-encoding" {
            continue;
        }
        builder = builder.header(name, value);
    }
    let body: OutBody = match resp.body {
        HttpServerBody::Empty => buffered(Bytes::new()),
        HttpServerBody::Bytes(b) => buffered(Bytes::from(b)),
        HttpServerBody::Stream(s) => BodyExt::boxed_unsync(StreamBody::new(
            s.map(|item| item.map(|chunk| Frame::data(Bytes::from(chunk)))),
        )),
    };
    builder
        .body(body)
        .unwrap_or_else(|_| status_only(StatusCode::INTERNAL_SERVER_ERROR))
}

/// A fully-buffered [`OutBody`] (`Infallible` widened to the shared error type).
fn buffered(bytes: Bytes) -> OutBody {
    Full::new(bytes)
        .map_err(|never: Infallible| match never {})
        .boxed_unsync()
}

fn status_only(status: StatusCode) -> Response<OutBody> {
    Response::builder()
        .status(status)
        .body(buffered(Bytes::new()))
        .expect("status-only response is always valid")
}

/// Serves one accepted connection to completion, honouring the drain signal.
///
/// Awaits `fut` under an optional deadline, logging and yielding `None` if it
/// expires. `None` for the duration means no deadline, so a disabled timeout
/// costs a branch rather than a timer.
async fn with_timeout<F: Future>(
    limit: Option<std::time::Duration>,
    fut: F,
    what: &'static str,
) -> Option<F::Output> {
    match limit {
        None => Some(fut.await),
        Some(limit) => match tokio::time::timeout(limit, fut).await {
            Ok(out) => Some(out),
            Err(_) => {
                // Debug, not warn: on a public port this is a scanner or a
                // half-open connection, not an operator's problem, and warning
                // per connection would hand any peer a log-flooding lever.
                tracing::debug!(
                    target: "runtime::http",
                    timeout_ms = limit.as_millis() as u64,
                    "{what} timed out; closing the connection",
                );
                None
            }
        },
    }
}

/// Generic over the transport so a plain `TcpStream` and a TLS stream share one
/// path: everything above the socket — the service, the response handoff, the
/// graceful shutdown — is identical, and duplicating it per scheme is how the
/// two quietly drift apart.
async fn serve_connection<S>(
    io: S,
    tx: mpsc::Sender<Pending>,
    origin: String,
    mut drain_rx: watch::Receiver<bool>,
    timeouts: HttpTimeouts,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let service = service_fn(move |req: Request<Incoming>| {
        let tx = tx.clone();
        let origin = origin.clone();
        async move {
            let server_req = to_server_request(req, &origin);
            let (rtx, rrx) = oneshot::channel();
            let (dtx, drx) = oneshot::channel();
            if tx.send((server_req, rtx, drx)).await.is_err() {
                // Server closed: the request channel is gone.
                return Ok::<_, Infallible>(status_only(StatusCode::SERVICE_UNAVAILABLE));
            }
            match rrx.await {
                Ok(resp) => {
                    // Say "delivered" before building, so a handler awaiting the
                    // disconnect signal learns the request completed. If this
                    // future is instead dropped — which is what a vanished
                    // client does to it — `dtx` drops unsent and the watcher
                    // sees the disconnect.
                    let _ = dtx.send(());
                    Ok(build_response(resp))
                }
                // Guest dropped the request without responding.
                Err(_) => {
                    let _ = dtx.send(());
                    Ok(status_only(StatusCode::INTERNAL_SERVER_ERROR))
                }
            }
        }
    });

    // One builder for both versions: it reads the start of the stream and picks
    // HTTP/2 when the connection preface is there, HTTP/1.1 otherwise. That is
    // the whole of h2c support on a cleartext port, and over TLS it agrees with
    // whatever ALPN already negotiated (an `h2` client sends the preface first).
    let mut builder = auto::Builder::new(TokioExecutor::new());
    // Every timeout below needs a timer, and hyper is emphatic about it in both
    // directions: a *default* timeout with no timer is silently dropped (with a
    // `warn!` nobody reads — which is how this server ran without hyper's own
    // 30s header timeout), while an *explicitly configured* one panics. So the
    // timer goes on both builders before anything else touches them.
    builder.http1().timer(TokioTimer::new());
    builder.http2().timer(TokioTimer::new());
    // Passing `None` disables, which is exactly what `None` means here.
    builder.http1().header_read_timeout(timeouts.header_read);
    if let Some(interval) = timeouts.h2_keep_alive {
        builder
            .http2()
            .keep_alive_interval(interval)
            .keep_alive_timeout(interval);
    }
    // Explicit rather than inherited: an HTTP/2 peer can open streams far faster
    // than a single-threaded isolate answers them, and every open stream holds a
    // queued `Pending` plus its body channel. Capping concurrency bounds what one
    // connection can make the server hold, and leaves the request channel's
    // buffer for spreading across connections rather than one client filling it.
    builder.http2().max_concurrent_streams(256);
    // The version-detection read happens inside the connection future, before
    // any hyper timer exists, so the deadline on it has to come from under the
    // socket — see [`FirstByteTimeout`](crate::first_byte).
    let io = FirstByteTimeout::new(io, timeouts.handshake);
    let conn = builder.serve_connection(TokioIo::new(io), service);
    tokio::pin!(conn);
    // Race the connection against the drain signal. On shutdown,
    // `graceful_shutdown` stops reading new requests but still finishes the one
    // in flight and writes its response — the difference between draining and
    // dropping. Awaiting the connection afterwards is what makes the response
    // actually reach the socket.
    tokio::select! {
        _ = conn.as_mut() => {}
        // Wrapped so the borrow `wait_for` hands back — a read guard — is
        // dropped at the end of the statement, rather than living across the
        // await below and making this task non-`Send`.
        _ = async {
            let _ = drain_rx.wait_for(|draining| *draining).await;
        } => {
            conn.as_mut().graceful_shutdown();
            let _ = conn.await;
        }
    }
}

impl HttpServerProvider for SystemHttpServer {
    fn serve(
        &self,
        options: HttpServeOptions,
    ) -> BoxFuture<Result<(u64, SocketInfo), ProviderError>> {
        let this = self.clone();
        Box::pin(async move {
            // Before the acceptor and before the port is claimed: a denied bind
            // must leave nothing behind.
            if let Some(allow) = &this.allow_listen {
                allow.check(&options.host, options.port, "serve")?;
            }
            // Build the TLS acceptor before binding: a bad cert should fail the
            // `serve` call, not each connection after the port is already open.
            let tls = match &options.tls {
                Some(tls) => Some(crate::tls::server_acceptor(&tls.cert, &tls.key, &tls.alpn)?),
                None => None,
            };
            let listener = TcpListener::bind((options.host.as_str(), options.port))
                .await
                .map_err(err)?;
            let local = listener.local_addr().ok();
            let authority = local.map(|a| a.to_string()).unwrap_or_default();
            // The scheme the guest sees in `request.url`. A TLS listener serves
            // https:, and reporting http: there would misdescribe the request to
            // any handler that builds an absolute URL from it.
            let scheme = if tls.is_some() { "https" } else { "http" };
            let origin = format!("{scheme}://{authority}");
            // Roomy buffer so many connections can have a request queued for the
            // consumer to drain in one batch (see `next_requests`), rather than
            // stalling on backpressure between crossings.
            let (tx, rx) = mpsc::channel::<Pending>(1024);
            let (draining, drain_rx) = watch::channel(false);
            let live = this.live.clone();
            let timeouts = options.timeouts;

            let acceptor = tokio::spawn(async move {
                // Errors from `accept` are retried, never fatal — see
                // [`AcceptBackoff`](crate::accept_backoff) for why, and for
                // what the wait between attempts is protecting.
                let mut backoff = AcceptBackoff::new();
                loop {
                    let stream = match listener.accept().await {
                        Ok((stream, _peer)) => {
                            backoff.reset();
                            stream
                        }
                        Err(e) => {
                            let delay = backoff.next_delay();
                            tracing::warn!(
                                target: "runtime::http",
                                error = %e,
                                backoff_ms = delay.as_millis() as u64,
                                "accept failed; retrying",
                            );
                            tokio::time::sleep(delay).await;
                            continue;
                        }
                    };
                    let _ = stream.set_nodelay(true);
                    let tx = tx.clone();
                    let origin = origin.clone();
                    let live = live.clone();
                    let drain_rx = drain_rx.clone();
                    let tls = tls.clone();
                    live.enter();
                    tokio::spawn(async move {
                        match tls {
                            // A failed handshake ends this connection only. It is
                            // an ordinary event on a public port — a scanner, a
                            // client with no shared cipher — and must never take
                            // the acceptor down with it.
                            Some(acceptor) => {
                                // A handshake that never finishes is the same
                                // hold on a task and a descriptor as one that
                                // never starts, and rustls will wait for the
                                // peer's next flight indefinitely.
                                let handshake = with_timeout(
                                    timeouts.handshake,
                                    acceptor.accept(stream),
                                    "tls handshake",
                                )
                                .await;
                                if let Some(Ok(stream)) = handshake {
                                    serve_connection(stream, tx, origin, drain_rx, timeouts).await;
                                }
                            }
                            None => {
                                serve_connection(stream, tx, origin, drain_rx, timeouts).await;
                            }
                        }
                        live.leave();
                    });
                }
            })
            .abort_handle();

            let id = this.id();
            this.requests.lock().unwrap().insert(id, rx);
            this.controls.lock().unwrap().insert(
                id,
                Control {
                    acceptor,
                    shutdown: Arc::new(Notify::new()),
                    draining,
                },
            );
            Ok((id, info_of(local)))
        })
    }

    fn next_requests(
        &self,
        id: u64,
        max: usize,
    ) -> BoxFuture<Result<Vec<(u64, HttpServerRequest)>, ProviderError>> {
        let this = self.clone();
        Box::pin(async move {
            // Take the receiver out so no lock is held across the await, then
            // reinsert to keep serving (mirrors SystemNet::accept). The shutdown
            // signal lives in a side map `close` can still reach meanwhile.
            let mut rx = match this.requests.lock().unwrap().remove(&id) {
                Some(rx) => rx,
                None => return Ok(Vec::new()), // closed
            };
            let shutdown = this
                .controls
                .lock()
                .unwrap()
                .get(&id)
                .map(|c| c.shutdown.clone());
            // Await the first request (parking until one arrives or close fires)…
            let first = match shutdown {
                Some(notify) => tokio::select! {
                    biased;
                    () = notify.notified() => None, // close() asked us to stop
                    r = rx.recv() => r,
                },
                None => rx.recv().await,
            };
            let mut batch = Vec::new();
            if let Some(pending) = first {
                batch.push(pending);
                // …then drain whatever else is already queued, without parking,
                // up to `max` — this is the amortization: one await, many
                // requests handed to the single-threaded consumer per crossing.
                while batch.len() < max {
                    match rx.try_recv() {
                        Ok(pending) => batch.push(pending),
                        Err(_) => break, // empty (or disconnected) — stop draining
                    }
                }
            }
            this.requests.lock().unwrap().insert(id, rx);

            // Assign a request id to each and stash its response sender. (Empty
            // batch ⇒ closed/shutting down.)
            let mut out = Vec::with_capacity(batch.len());
            if !batch.is_empty() {
                let mut pending = this.pending.lock().unwrap();
                let mut delivered = this.delivered.lock().unwrap();
                for (req, sender, disconnect) in batch {
                    let rid = this.id();
                    pending.insert(rid, sender);
                    delivered.insert(rid, disconnect);
                    out.push((rid, req));
                }
            }
            Ok(out)
        })
    }

    fn respond(
        &self,
        request_id: u64,
        response: HttpServerResponse,
    ) -> BoxFuture<Result<(), ProviderError>> {
        let pending = self.pending.clone();
        Box::pin(async move {
            if let Some(sender) = pending.lock().unwrap().remove(&request_id) {
                let _ = sender.send(response); // client may have gone away
            }
            Ok(())
        })
    }

    fn request_disconnected(&self, request_id: u64) -> BoxFuture<bool> {
        // Take the receiver out rather than borrowing it: no lock may be held
        // across the await, and only one watcher exists per request.
        let watch = self.delivered.lock().unwrap().remove(&request_id);
        Box::pin(async move {
            match watch {
                // Err means the service future was dropped without ever getting
                // a response to hand over — the client is gone.
                Some(rx) => rx.await.is_err(),
                // Unknown id: already responded to, or never ours.
                None => false,
            }
        })
    }

    fn close(&self, id: u64) -> BoxFuture<Result<(), ProviderError>> {
        let this = self.clone();
        Box::pin(async move {
            // Drop the request receiver (idle keep-alive conns will end), abort
            // the acceptor (stop new conns), and wake any parked next_request.
            this.requests.lock().unwrap().remove(&id);
            if let Some(ctrl) = this.controls.lock().unwrap().remove(&id) {
                ctrl.acceptor.abort();
                ctrl.shutdown.notify_waiters();
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use es_runtime_providers::HttpServerTls;
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpStream;

    /// Sends a bare `GET /` on a fresh connection and returns it, so a test can
    /// decide when (and whether) to hang up.
    async fn request_on_new_conn(port: u16) -> TcpStream {
        let mut sock = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        sock.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")
            .await
            .unwrap();
        sock
    }

    async fn bound() -> (SystemHttpServer, u64, u16) {
        bound_with(None).await
    }

    pub(super) async fn bound_with(tls: Option<HttpServerTls>) -> (SystemHttpServer, u64, u16) {
        bound_with_timeouts(tls, HttpTimeouts::default()).await
    }

    /// Most tests want the shipping defaults; the timeout tests want values
    /// short enough to wait for, which is the only reason this is a parameter.
    pub(super) async fn bound_with_timeouts(
        tls: Option<HttpServerTls>,
        timeouts: HttpTimeouts,
    ) -> (SystemHttpServer, u64, u16) {
        let http = SystemHttpServer::new();
        let (id, info) = http
            .serve(HttpServeOptions {
                host: "127.0.0.1".into(),
                port: 0,
                tls,
                timeouts,
            })
            .await
            .unwrap();
        (http, id, info.local_port)
    }

    /// The accept loop must keep looping. It used to leave on the first error
    /// from `accept` — which killed the server while the port stayed bound, and
    /// said nothing. The errno that motivates the fix (`ECONNABORTED`,
    /// `EMFILE`) cannot be provoked on demand from in-process test code, so the
    /// retry policy itself is covered by the unit tests in
    /// [`accept_backoff`](crate::accept_backoff); what this holds down is that
    /// a stream of connections opened and abandoned on arrival leaves the
    /// listener still serving afterwards.
    #[tokio::test]
    async fn the_acceptor_keeps_serving_after_a_burst_of_abandoned_connections() {
        let (http, id, port) = bound().await;

        for _ in 0..64 {
            drop(TcpStream::connect(("127.0.0.1", port)).await.unwrap());
        }

        let _sock = request_on_new_conn(port).await;
        let reqs = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            http.next_requests(id, 8),
        )
        .await
        .expect("the acceptor is still accepting")
        .unwrap();
        assert_eq!(reqs.len(), 1, "the request after the burst still arrives");
    }

    /// The signal a handler's `request.signal` is built on: a client that goes
    /// away before its response was handed over must be reported as a
    /// disconnect, not left waiting forever.
    #[tokio::test]
    async fn request_disconnected_reports_a_client_that_hung_up() {
        let (http, id, port) = bound().await;
        let sock = request_on_new_conn(port).await;
        let reqs = http.next_requests(id, 8).await.unwrap();
        let rid = reqs[0].0;

        drop(sock); // hang up without ever reading a response
        let gone = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            http.request_disconnected(rid),
        )
        .await
        .expect("must settle rather than hang");
        assert!(gone, "a dropped connection is a disconnect");
    }

    /// The other half of the contract: a request that was answered resolves
    /// `false`. If it did not settle here, every served request would leak a
    /// pending op and hold a driven loop open.
    #[tokio::test]
    async fn request_disconnected_resolves_false_once_answered() {
        let (http, id, port) = bound().await;
        let _sock = request_on_new_conn(port).await;
        let reqs = http.next_requests(id, 8).await.unwrap();
        let rid = reqs[0].0;

        let watch = tokio::spawn({
            let http = http.clone();
            async move { http.request_disconnected(rid).await }
        });
        http.respond(
            rid,
            HttpServerResponse {
                status: 200,
                headers: vec![],
                body: HttpServerBody::Bytes(b"ok".to_vec()),
            },
        )
        .await
        .unwrap();

        let gone = tokio::time::timeout(std::time::Duration::from_secs(5), watch)
            .await
            .expect("must settle rather than hang")
            .unwrap();
        assert!(!gone, "an answered request is not a disconnect");
    }

    /// An id that was never handed out (or was already watched) answers `false`
    /// immediately rather than parking on nothing.
    #[tokio::test]
    async fn request_disconnected_is_false_for_an_unknown_id() {
        let (http, _id, _port) = bound().await;
        assert!(!http.request_disconnected(9_999).await);
    }
    #[tokio::test]
    async fn serve_refuses_a_bind_outside_the_allowlist() {
        // The check precedes the TLS acceptor and the bind, so a refused
        // `serve` claims no port and leaves nothing behind. The list names the
        // interface rather than a port, so the allowed half can bind port 0 and
        // never race another test for a number.
        let http = SystemHttpServer::new()
            .with_listen_allowlist(crate::HostAllowlist::parse(["127.0.0.1"]).unwrap());
        let err = http
            .serve(HttpServeOptions {
                host: "0.0.0.0".to_string(),
                port: 0,
                tls: None,
                timeouts: HttpTimeouts::default(),
            })
            .await
            .err()
            .expect("a bind outside the list must be refused");
        assert!(err.to_string().contains("0.0.0.0:0"), "{err}");
        // ...and the allowed address still binds: `listen` is one capability,
        // whichever API claims the port.
        let (id, _) = http
            .serve(HttpServeOptions {
                host: "127.0.0.1".to_string(),
                port: 0,
                tls: None,
                timeouts: HttpTimeouts::default(),
            })
            .await
            .expect("the allowed address binds");
        http.close(id).await.unwrap();
    }
}

#[cfg(test)]
mod tls_tests {
    use super::*;
    use es_runtime_providers::HttpServerTls;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_rustls::TlsConnector;
    use tokio_rustls::rustls::crypto::aws_lc_rs;
    use tokio_rustls::rustls::pki_types::{CertificateDer, ServerName};
    use tokio_rustls::rustls::{ClientConfig, RootCertStore};

    /// A throwaway cert for `localhost`: (PEM cert, PEM key, DER cert). The DER
    /// copy is what the test client trusts, so nothing depends on the host's
    /// certificate store.
    pub(super) fn self_signed() -> (Vec<u8>, Vec<u8>, CertificateDer<'static>) {
        let ck = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
        (
            ck.cert.pem().into_bytes(),
            ck.signing_key.serialize_pem().into_bytes(),
            ck.cert.der().clone(),
        )
    }

    pub(super) fn tls_options(cert: Vec<u8>, key: Vec<u8>) -> HttpServerTls {
        HttpServerTls {
            cert,
            key,
            alpn: vec!["http/1.1".to_string()],
        }
    }

    /// A real handshake against the real listener, then a real HTTP request over
    /// it — the only way to know TLS termination is wired to the same request
    /// path as plain HTTP rather than to a parallel one that has drifted.
    #[tokio::test]
    async fn a_tls_listener_terminates_and_serves() {
        let (cert_pem, key_pem, cert_der) = self_signed();
        let (http, id, port) = super::tests::bound_with(Some(tls_options(cert_pem, key_pem))).await;

        // A client trusting only this cert.
        let mut roots = RootCertStore::empty();
        roots.add(cert_der).unwrap();
        let config = ClientConfig::builder_with_provider(Arc::new(aws_lc_rs::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let connector = TlsConnector::from(Arc::new(config));

        let request = tokio::spawn(async move {
            let tcp = tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .unwrap();
            let name = ServerName::try_from("localhost").unwrap();
            let mut tls = connector.connect(name, tcp).await.expect("handshake");
            tls.write_all(b"GET /x HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
            let mut got = Vec::new();
            tls.read_to_end(&mut got).await.unwrap();
            String::from_utf8_lossy(&got).into_owned()
        });

        let reqs = http.next_requests(id, 8).await.unwrap();
        let (rid, req) = reqs.into_iter().next().expect("one request");
        // The listener's scheme, not the client's Host header, decides this.
        assert_eq!(req.url, "https://localhost/x");

        http.respond(
            rid,
            HttpServerResponse {
                status: 200,
                headers: vec![],
                body: HttpServerBody::Bytes(b"secure".to_vec()),
            },
        )
        .await
        .unwrap();

        let response = request.await.unwrap();
        assert!(response.contains("200 OK"), "{response}");
        assert!(response.contains("secure"), "{response}");
    }

    /// A bad cert must fail `serve` itself. Binding the port first and then
    /// rejecting every handshake would look like a working server that nothing
    /// can talk to.
    #[tokio::test]
    async fn an_unusable_certificate_fails_the_bind() {
        let (_, key_pem, _) = self_signed();
        let http = SystemHttpServer::new();
        let result = http
            .serve(HttpServeOptions {
                host: "127.0.0.1".into(),
                port: 0,
                tls: Some(tls_options(b"not a certificate".to_vec(), key_pem)),
                timeouts: HttpTimeouts::default(),
            })
            .await;
        assert!(result.is_err(), "an unparseable cert must not bind");
    }

    /// A failed handshake ends that connection only. On a public port this is an
    /// ordinary event — a scanner, a plain-HTTP client — and taking the acceptor
    /// down with it would be a one-packet denial of service.
    #[tokio::test]
    async fn a_failed_handshake_does_not_stop_the_server() {
        let (cert_pem, key_pem, cert_der) = self_signed();
        let (http, id, port) = super::tests::bound_with(Some(tls_options(cert_pem, key_pem))).await;

        // Plain HTTP at a TLS port: garbage to the handshake.
        {
            let mut tcp = tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .unwrap();
            let _ = tcp.write_all(b"GET / HTTP/1.1\r\n\r\n").await;
        }

        // The server must still serve a proper TLS client afterwards.
        let mut roots = RootCertStore::empty();
        roots.add(cert_der).unwrap();
        let config = ClientConfig::builder_with_provider(Arc::new(aws_lc_rs::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let connector = TlsConnector::from(Arc::new(config));
        let request = tokio::spawn(async move {
            let tcp = tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .unwrap();
            let name = ServerName::try_from("localhost").unwrap();
            let mut tls = connector.connect(name, tcp).await.expect("handshake");
            tls.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
            let mut got = Vec::new();
            tls.read_to_end(&mut got).await.unwrap();
            String::from_utf8_lossy(&got).into_owned()
        });

        let reqs = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            http.next_requests(id, 8),
        )
        .await
        .expect("the acceptor survived the bad handshake")
        .unwrap();
        let (rid, _) = reqs.into_iter().next().expect("one request");
        http.respond(
            rid,
            HttpServerResponse {
                status: 200,
                headers: vec![],
                body: HttpServerBody::Bytes(b"still here".to_vec()),
            },
        )
        .await
        .unwrap();
        assert!(request.await.unwrap().contains("still here"));
    }
}

#[cfg(test)]
mod http2_tests {
    use super::*;
    use es_runtime_providers::HttpServerTls;

    /// Answers `rid` with a 200 carrying `body`.
    async fn ok(http: &SystemHttpServer, rid: u64, body: &str) {
        http.respond(
            rid,
            HttpServerResponse {
                status: 200,
                headers: vec![],
                body: HttpServerBody::Bytes(body.as_bytes().to_vec()),
            },
        )
        .await
        .unwrap();
    }

    /// A cleartext port serves an HTTP/2 client that opens with the connection
    /// preface (prior knowledge, h2c) — the version detection is the accepted
    /// stream's own bytes, with no ALPN to consult and no upgrade dance.
    #[tokio::test]
    async fn serves_h2c_on_a_cleartext_port_by_prior_knowledge() {
        let (http, id, port) = super::tests::bound_with(None).await;
        let client = reqwest::Client::builder()
            .http2_prior_knowledge()
            .build()
            .unwrap();

        let request = tokio::spawn(async move {
            let resp = client
                .get(format!("http://127.0.0.1:{port}/h2c"))
                .send()
                .await
                .expect("an h2c client is answered");
            (resp.version(), resp.text().await.unwrap())
        });

        let reqs = http.next_requests(id, 8).await.unwrap();
        let (rid, req) = reqs.into_iter().next().expect("one request");
        // HTTP/2 sends `:authority`, not a `Host` header. Reading only the
        // header would leave every h2 request claiming the bound address.
        assert_eq!(req.url, format!("http://127.0.0.1:{port}/h2c"));
        ok(&http, rid, "over-h2c").await;

        let (version, body) = request.await.unwrap();
        assert_eq!(version, reqwest::Version::HTTP_2);
        assert_eq!(body, "over-h2c");
    }

    /// The point of multiplexing: two requests in flight on **one** connection,
    /// answered out of order. On HTTP/1.1 the second response could not be
    /// written before the first; here it is, and each stream gets its own body.
    #[tokio::test]
    async fn multiplexes_concurrent_streams_on_one_connection() {
        let (http, id, port) = super::tests::bound_with(None).await;
        // One client, so both requests share a pooled connection.
        let client = reqwest::Client::builder()
            .http2_prior_knowledge()
            .build()
            .unwrap();

        let get = |path: &'static str| {
            let client = client.clone();
            tokio::spawn(async move {
                client
                    .get(format!("http://127.0.0.1:{port}/{path}"))
                    .send()
                    .await
                    .unwrap()
                    .text()
                    .await
                    .unwrap()
            })
        };
        let first = get("first");
        let second = get("second");

        // Collect both before answering either: on HTTP/1.1 this deadlocks
        // unless the client opens a second connection, because the second
        // request is not written until the first response is read.
        let mut pending = Vec::new();
        while pending.len() < 2 {
            pending.extend(http.next_requests(id, 8).await.unwrap());
        }
        // Answer in reverse arrival order — the stream, not the connection,
        // decides which response belongs to which request.
        for (rid, req) in pending.into_iter().rev() {
            let body = if req.url.ends_with("/first") {
                "one"
            } else {
                "two"
            };
            ok(&http, rid, body).await;
        }

        assert_eq!(first.await.unwrap(), "one");
        assert_eq!(second.await.unwrap(), "two");
    }

    /// Over TLS the version is ALPN's answer. The listener offers `h2` first, so
    /// an h2-capable client gets HTTP/2 without the guest asking for anything.
    #[tokio::test]
    async fn negotiates_h2_over_tls_alpn() {
        let ck = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
        let cert_pem = ck.cert.pem().into_bytes();
        let key_pem = ck.signing_key.serialize_pem().into_bytes();
        let (http, id, port) = super::tests::bound_with(Some(HttpServerTls {
            cert: cert_pem.clone(),
            key: key_pem,
            // What `runtime:http` `serve` sends when the guest names no `alpn`.
            alpn: vec!["h2".to_string(), "http/1.1".to_string()],
        }))
        .await;

        // Trusts this cert and nothing else, so the test never reads the host's
        // certificate store.
        let client = reqwest::Client::builder()
            .add_root_certificate(reqwest::Certificate::from_pem(&cert_pem).unwrap())
            .resolve("localhost", ([127, 0, 0, 1], port).into())
            .build()
            .unwrap();

        let request = tokio::spawn(async move {
            let resp = client
                .get(format!("https://localhost:{port}/tls"))
                .send()
                .await
                .expect("the handshake negotiates a protocol both sides speak");
            (resp.version(), resp.text().await.unwrap())
        });

        let reqs = http.next_requests(id, 8).await.unwrap();
        let (rid, req) = reqs.into_iter().next().expect("one request");
        assert_eq!(req.url, format!("https://localhost:{port}/tls"));
        ok(&http, rid, "secure-h2").await;

        let (version, body) = request.await.unwrap();
        assert_eq!(version, reqwest::Version::HTTP_2);
        assert_eq!(body, "secure-h2");
    }

    /// One port, both versions, in one test: an HTTP/1.1 client written by hand
    /// and an HTTP/2 client on the same listener. Version detection is per
    /// connection, so this is the shape that would break if it were per
    /// listener — and it is the shape every existing deployment is in the moment
    /// h2 is advertised.
    #[tokio::test]
    async fn one_listener_serves_an_http1_and_an_http2_client() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let (http, id, port) = super::tests::bound_with(None).await;

        let h1 = tokio::spawn(async move {
            let mut sock = tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .unwrap();
            sock.write_all(b"GET /one HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
            let mut got = String::new();
            sock.read_to_string(&mut got).await.unwrap();
            got
        });
        let client = reqwest::Client::builder()
            .http2_prior_knowledge()
            .build()
            .unwrap();
        let h2 = tokio::spawn(async move {
            let resp = client
                .get(format!("http://127.0.0.1:{port}/two"))
                .send()
                .await
                .unwrap();
            (resp.version(), resp.text().await.unwrap())
        });

        let mut pending = Vec::new();
        while pending.len() < 2 {
            pending.extend(http.next_requests(id, 8).await.unwrap());
        }
        for (rid, req) in pending {
            let body = if req.url.ends_with("/one") {
                "v1"
            } else {
                "v2"
            };
            ok(&http, rid, body).await;
        }

        let one = h1.await.unwrap();
        assert!(one.contains("HTTP/1.1 200"), "{one}");
        assert!(one.contains("v1"), "{one}");
        let (version, two) = h2.await.unwrap();
        assert_eq!(version, reqwest::Version::HTTP_2);
        assert_eq!(two, "v2");
    }

    /// A streamed response body over HTTP/2. There is no chunked transfer-
    /// encoding here — the version frames the body itself, and
    /// `Transfer-Encoding` is forbidden on it — so this is the path that would
    /// silently produce a malformed response if the HTTP/1.1 framing leaked
    /// through.
    #[tokio::test]
    async fn streams_a_response_body_over_h2() {
        let (http, id, port) = super::tests::bound_with(None).await;
        let client = reqwest::Client::builder()
            .http2_prior_knowledge()
            .build()
            .unwrap();

        let request = tokio::spawn(async move {
            let resp = client
                .get(format!("http://127.0.0.1:{port}/stream"))
                .send()
                .await
                .unwrap();
            let te = resp.headers().get("transfer-encoding").is_some();
            let len = resp.headers().get("content-length").is_some();
            (resp.text().await.unwrap(), te, len)
        });

        let reqs = http.next_requests(id, 8).await.unwrap();
        let (rid, _) = reqs.into_iter().next().expect("one request");
        let chunks = futures_util::stream::iter(
            (0..8).map(|i| Ok::<_, ProviderError>(format!("chunk-{i};").into_bytes())),
        );
        http.respond(
            rid,
            HttpServerResponse {
                status: 200,
                headers: vec![],
                body: HttpServerBody::Stream(Box::pin(chunks)),
            },
        )
        .await
        .unwrap();

        let (body, transfer_encoding, content_length) = request.await.unwrap();
        assert_eq!(
            body,
            "chunk-0;chunk-1;chunk-2;chunk-3;chunk-4;chunk-5;chunk-6;chunk-7;"
        );
        assert!(!transfer_encoding, "HTTP/2 forbids Transfer-Encoding");
        // Length is unknown up front for a streamed body on either version.
        assert!(!content_length, "a streamed body has no Content-Length");
    }

    /// The inbound half: a request body arrives as a stream the consumer pulls,
    /// and on HTTP/2 that is DATA frames under per-stream flow control rather
    /// than a chunked HTTP/1.1 body. The handoff is the same either way, which
    /// is what this pins.
    #[tokio::test]
    async fn reads_a_streamed_request_body_over_h2() {
        let (http, id, port) = super::tests::bound_with(None).await;
        let client = reqwest::Client::builder()
            .http2_prior_knowledge()
            .build()
            .unwrap();

        // Big enough to span many DATA frames and exhaust the initial stream
        // window, so the client only finishes if WINDOW_UPDATEs flow back.
        let upload = "u".repeat(256 * 1024);
        let expected = upload.len();
        let request = tokio::spawn(async move {
            client
                .post(format!("http://127.0.0.1:{port}/upload"))
                .body(upload)
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap()
        });

        let reqs = http.next_requests(id, 8).await.unwrap();
        let (rid, req) = reqs.into_iter().next().expect("one request");
        assert_eq!(req.method, "POST");
        let mut seen = 0usize;
        match req.body {
            HttpServerBody::Stream(mut chunks) => {
                while let Some(chunk) = chunks.next().await {
                    seen += chunk.expect("a chunk, not an error").len();
                }
            }
            _ => panic!("a request with a body must cross as a stream"),
        }
        assert_eq!(seen, expected, "every uploaded byte reached the consumer");
        ok(&http, rid, "read-it-all").await;

        assert_eq!(request.await.unwrap(), "read-it-all");
    }

    /// Method, status, and ordinary headers survive the HPACK round trip — the
    /// header representation is entirely different on HTTP/2, so "it worked on
    /// HTTP/1.1" proves nothing about it.
    #[tokio::test]
    async fn carries_method_status_and_headers_over_h2() {
        let (http, id, port) = super::tests::bound_with(None).await;
        let client = reqwest::Client::builder()
            .http2_prior_knowledge()
            .build()
            .unwrap();

        let request = tokio::spawn(async move {
            let resp = client
                .patch(format!("http://127.0.0.1:{port}/thing?q=1"))
                .header("x-request-note", "from-client")
                .send()
                .await
                .unwrap();
            let note = resp
                .headers()
                .get("x-response-note")
                .map(|v| v.to_str().unwrap().to_string());
            (resp.status().as_u16(), note)
        });

        let reqs = http.next_requests(id, 8).await.unwrap();
        let (rid, req) = reqs.into_iter().next().expect("one request");
        assert_eq!(req.method, "PATCH");
        assert!(req.url.ends_with("/thing?q=1"), "{}", req.url);
        assert!(
            req.headers
                .iter()
                .any(|(k, v)| k == "x-request-note" && v == "from-client"),
            "{:?}",
            req.headers
        );
        http.respond(
            rid,
            HttpServerResponse {
                status: 201,
                headers: vec![("x-response-note".into(), "from-server".into())],
                body: HttpServerBody::Bytes(b"created".to_vec()),
            },
        )
        .await
        .unwrap();

        let (status, note) = request.await.unwrap();
        assert_eq!(status, 201);
        assert_eq!(note.as_deref(), Some("from-server"));
    }

    /// HTTP/2 forbids connection-specific header fields, and a handler written
    /// against HTTP/1.1 habits will set them. The response must still be valid
    /// rather than a stream the client resets — so this pins the behaviour we
    /// rely on, whichever layer provides it.
    #[tokio::test]
    async fn connection_specific_headers_do_not_break_an_h2_response() {
        let (http, id, port) = super::tests::bound_with(None).await;
        let client = reqwest::Client::builder()
            .http2_prior_knowledge()
            .build()
            .unwrap();

        let request = tokio::spawn(async move {
            let resp = client
                .get(format!("http://127.0.0.1:{port}/legacy"))
                .send()
                .await
                .expect("the response must be valid HTTP/2");
            let leaked: Vec<String> = ["connection", "keep-alive", "transfer-encoding", "upgrade"]
                .iter()
                .filter(|h| resp.headers().contains_key(**h))
                .map(|h| (*h).to_string())
                .collect();
            (resp.status().as_u16(), leaked, resp.text().await.unwrap())
        });

        let reqs = http.next_requests(id, 8).await.unwrap();
        let (rid, _) = reqs.into_iter().next().expect("one request");
        http.respond(
            rid,
            HttpServerResponse {
                status: 200,
                headers: vec![
                    ("connection".into(), "keep-alive".into()),
                    ("keep-alive".into(), "timeout=5".into()),
                    ("transfer-encoding".into(), "chunked".into()),
                    ("content-length".into(), "999".into()),
                    ("x-kept".into(), "yes".into()),
                ],
                body: HttpServerBody::Bytes(b"legacy-ok".to_vec()),
            },
        )
        .await
        .unwrap();

        let (status, leaked, body) = request.await.unwrap();
        assert_eq!(status, 200);
        assert!(
            leaked.is_empty(),
            "these must not reach an h2 client: {leaked:?}"
        );
        assert_eq!(body, "legacy-ok");
    }

    /// A response larger than HTTP/2's initial 64 KiB stream window only
    /// completes if flow control is honoured in both directions. A server that
    /// wrote the body and stopped would hang here rather than fail loudly.
    #[tokio::test]
    async fn a_response_larger_than_the_flow_control_window_completes() {
        let (http, id, port) = super::tests::bound_with(None).await;
        let client = reqwest::Client::builder()
            .http2_prior_knowledge()
            .build()
            .unwrap();

        let request = tokio::spawn(async move {
            client
                .get(format!("http://127.0.0.1:{port}/big"))
                .send()
                .await
                .unwrap()
                .bytes()
                .await
                .unwrap()
                .len()
        });

        const SIZE: usize = 1024 * 1024; // 16× the default stream window
        let reqs = http.next_requests(id, 8).await.unwrap();
        let (rid, _) = reqs.into_iter().next().expect("one request");
        http.respond(
            rid,
            HttpServerResponse {
                status: 200,
                headers: vec![],
                body: HttpServerBody::Bytes(vec![b'z'; SIZE]),
            },
        )
        .await
        .unwrap();

        let got = tokio::time::timeout(std::time::Duration::from_secs(30), request)
            .await
            .expect("a windowed body must drain, not stall")
            .unwrap();
        assert_eq!(got, SIZE);
    }

    /// The disconnect signal on HTTP/2: a client that resets its stream (here,
    /// by being dropped mid-flight) must be reported, or a handler awaiting
    /// `request.signal` waits for a peer that is already gone. The stream reset
    /// is a different event from the HTTP/1.1 connection close this mirrors.
    #[tokio::test]
    async fn a_client_that_abandons_an_h2_stream_is_reported_as_a_disconnect() {
        let (http, id, port) = super::tests::bound_with(None).await;
        let client = reqwest::Client::builder()
            .http2_prior_knowledge()
            .build()
            .unwrap();

        let request = tokio::spawn(async move {
            let _ = client
                .get(format!("http://127.0.0.1:{port}/gone"))
                .send()
                .await;
        });
        let reqs = http.next_requests(id, 8).await.unwrap();
        let (rid, _) = reqs.into_iter().next().expect("one request");

        request.abort(); // drops the client, and with it the connection
        let gone = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            http.request_disconnected(rid),
        )
        .await
        .expect("must settle rather than hang");
        assert!(gone, "an abandoned h2 stream is a disconnect");
    }

    /// A request whose response can no longer arrive — here because the whole
    /// server registry went away while it was in flight — ends as a 500 on
    /// HTTP/2 as it does on HTTP/1.1. The alternative is a client left holding
    /// an open stream forever, which is the failure that looks like a hang
    /// rather than an error.
    ///
    /// (Dropping the returned `HttpServerRequest` alone would *not* do this: the
    /// response sender is stashed in the provider's pending map when the request
    /// is handed out, not carried by the request itself.)
    #[tokio::test]
    async fn a_request_that_can_no_longer_be_answered_becomes_a_500() {
        let (http, id, port) = super::tests::bound_with(None).await;
        let client = reqwest::Client::builder()
            .http2_prior_knowledge()
            .build()
            .unwrap();

        let request = tokio::spawn(async move {
            client
                .get(format!("http://127.0.0.1:{port}/dropped"))
                .send()
                .await
                .unwrap()
                .status()
                .as_u16()
        });

        let reqs = http.next_requests(id, 8).await.unwrap();
        let (_rid, _req) = reqs.into_iter().next().expect("one request");
        // The last handle to the registry, and with it the stashed response
        // sender: the connection task sees the send half drop.
        drop(http);

        let status = tokio::time::timeout(std::time::Duration::from_secs(10), request)
            .await
            .expect("the client must be answered, not left hanging")
            .unwrap();
        assert_eq!(status, 500);
    }

    /// Graceful shutdown over HTTP/2: an in-flight request still gets its
    /// response written after the drain starts. hyper's graceful shutdown is
    /// version-specific machinery (GOAWAY here, not a closed keep-alive), so it
    /// needs its own coverage.
    #[tokio::test]
    async fn a_drain_finishes_an_inflight_h2_request() {
        let (http, id, port) = super::tests::bound_with(None).await;
        let client = reqwest::Client::builder()
            .http2_prior_knowledge()
            .build()
            .unwrap();

        let request = tokio::spawn(async move {
            client
                .get(format!("http://127.0.0.1:{port}/slow"))
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap()
        });
        let reqs = http.next_requests(id, 8).await.unwrap();
        let (rid, _) = reqs.into_iter().next().expect("one request");

        // Drain *first*, then answer: the response has to survive the shutdown.
        assert_eq!(http.shutdown_all(), 1);
        ok(&http, rid, "answered-while-draining").await;

        assert_eq!(request.await.unwrap(), "answered-while-draining");
        assert!(
            http.wait_for_idle(std::time::Duration::from_secs(10)).await,
            "the connection must finish and the server go idle"
        );
        http.close(id).await.unwrap();
    }

    /// A guest that narrows `alpn` to HTTP/1.1 gets HTTP/1.1, even from a client
    /// that would have preferred h2 — the option is the escape hatch, so it has
    /// to actually decide the outcome.
    #[tokio::test]
    async fn an_alpn_of_http1_only_keeps_a_capable_client_on_http1() {
        let ck = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
        let cert_pem = ck.cert.pem().into_bytes();
        let key_pem = ck.signing_key.serialize_pem().into_bytes();
        let (http, id, port) = super::tests::bound_with(Some(HttpServerTls {
            cert: cert_pem.clone(),
            key: key_pem,
            alpn: vec!["http/1.1".to_string()],
        }))
        .await;

        let client = reqwest::Client::builder()
            .add_root_certificate(reqwest::Certificate::from_pem(&cert_pem).unwrap())
            .resolve("localhost", ([127, 0, 0, 1], port).into())
            .build()
            .unwrap();

        let request = tokio::spawn(async move {
            let resp = client
                .get(format!("https://localhost:{port}/one-one"))
                .send()
                .await
                .expect("a narrowed ALPN still serves");
            (resp.version(), resp.text().await.unwrap())
        });

        let reqs = http.next_requests(id, 8).await.unwrap();
        let (rid, _) = reqs.into_iter().next().expect("one request");
        ok(&http, rid, "plain").await;

        let (version, body) = request.await.unwrap();
        assert_eq!(version, reqwest::Version::HTTP_11);
        assert_eq!(body, "plain");
    }
}

/// Timeouts, from the outside: a client that stalls at each stage in turn must
/// end up disconnected, and one that is genuinely working must not.
///
/// The intervals here are milliseconds rather than the shipping seconds so a
/// test can wait for them. What is being checked is that each stage *has* a
/// deadline and that it is the configured one — the values themselves are
/// [`HttpTimeouts::default`]'s business.
#[cfg(test)]
mod timeout_tests {
    use super::tests::bound_with_timeouts;
    use super::*;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    /// Short enough to wait for, long enough that a loaded CI box does not trip
    /// it by accident on a connection that is genuinely progressing.
    const SHORT: Duration = Duration::from_millis(200);
    /// How long a test waits for a close that should already have happened.
    const GRACE: Duration = Duration::from_secs(10);

    /// Everything off, so each test can switch on exactly the one it is about.
    const OFF: HttpTimeouts = HttpTimeouts {
        handshake: None,
        header_read: None,
        h2_keep_alive: None,
    };

    /// Reads until the peer closes, returning `false` if it has not within
    /// `grace`. Bytes the server sends first (hyper answers a header timeout
    /// with a 408 before hanging up) are read past rather than treated as an
    /// answer — what is being waited for is the close.
    async fn closed_within(sock: &mut TcpStream, grace: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + grace;
        let mut buf = [0u8; 1024];
        loop {
            match tokio::time::timeout_at(deadline, sock.read(&mut buf)).await {
                Err(_) => return false,    // still open when time ran out
                Ok(Ok(0)) => return true,  // clean EOF
                Ok(Err(_)) => return true, // reset
                Ok(Ok(_)) => continue,     // said something; keep waiting
            }
        }
    }

    /// A peer that completes the TCP handshake and then says nothing never
    /// reaches hyper — the version-detecting read is what it is stalling — so
    /// no timeout hyper owns can reach it. It is also the cheapest possible way
    /// to hold a descriptor: one `connect` and silence.
    #[tokio::test]
    async fn a_connection_that_never_speaks_is_closed() {
        let timeouts = HttpTimeouts {
            handshake: Some(SHORT),
            ..OFF
        };
        let (_http, _id, port) = bound_with_timeouts(None, timeouts).await;

        let mut sock = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        assert!(
            closed_within(&mut sock, GRACE).await,
            "a silent connection must not be held forever"
        );
    }

    /// The same stall one stage later: TLS, where rustls will wait for the
    /// peer's next handshake flight indefinitely.
    #[tokio::test]
    async fn a_tls_client_that_never_starts_the_handshake_is_closed() {
        let (cert_pem, key_pem, _) = super::tls_tests::self_signed();
        let timeouts = HttpTimeouts {
            handshake: Some(SHORT),
            ..OFF
        };
        let tls = super::tls_tests::tls_options(cert_pem, key_pem);
        let (_http, _id, port) = bound_with_timeouts(Some(tls), timeouts).await;

        let mut sock = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        assert!(
            closed_within(&mut sock, GRACE).await,
            "a connection that never sends a ClientHello must not be held forever"
        );
    }

    /// Slowloris: a request head that starts and then dribbles. The first byte
    /// disarms the handshake deadline, so this is `header_read`'s job alone —
    /// which is why the handshake timeout is off here.
    #[tokio::test]
    async fn a_request_head_that_never_finishes_is_closed() {
        let timeouts = HttpTimeouts {
            header_read: Some(SHORT),
            ..OFF
        };
        let (_http, _id, port) = bound_with_timeouts(None, timeouts).await;

        let mut sock = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        // A request line and one header, and never the blank line that ends it.
        sock.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n")
            .await
            .unwrap();
        assert!(
            closed_within(&mut sock, GRACE).await,
            "an unfinished request head must not be held forever"
        );
    }

    /// The same timer's other half, and the one with a visible consequence: on
    /// HTTP/1.1, waiting for the *next* request on a kept-alive connection is
    /// waiting for a request head, so an idle connection is closed after
    /// `header_read` and a client that wants more opens a new one.
    #[tokio::test]
    async fn an_idle_keep_alive_connection_is_closed() {
        let timeouts = HttpTimeouts {
            header_read: Some(SHORT),
            ..OFF
        };
        let (http, id, port) = bound_with_timeouts(None, timeouts).await;

        let mut sock = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        sock.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")
            .await
            .unwrap();
        let reqs = http.next_requests(id, 8).await.unwrap();
        let (rid, _) = reqs.into_iter().next().expect("one request");
        http.respond(
            rid,
            HttpServerResponse {
                status: 200,
                headers: vec![],
                body: HttpServerBody::Bytes(b"done".to_vec()),
            },
        )
        .await
        .unwrap();

        // The connection was used, answered, and is now idle — not stalled.
        assert!(
            closed_within(&mut sock, GRACE).await,
            "an idle keep-alive connection must not be held forever"
        );
    }

    /// HTTP/2 has no idle timeout to fall back on — its connections are meant
    /// to be long-lived — so a peer that stops answering is found by probing.
    /// This client opens properly and then ignores everything, PINGs included,
    /// which is what a vanished peer looks like from the server's side when no
    /// FIN ever arrives.
    #[tokio::test]
    async fn an_http2_peer_that_stops_answering_pings_is_dropped() {
        let timeouts = HttpTimeouts {
            h2_keep_alive: Some(SHORT),
            ..OFF
        };
        let (_http, _id, port) = bound_with_timeouts(None, timeouts).await;

        let mut sock = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        sock.write_all(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n")
            .await
            .unwrap();
        // An empty SETTINGS frame: length 0, type 0x4, no flags, stream 0.
        sock.write_all(&[0, 0, 0, 4, 0, 0, 0, 0, 0]).await.unwrap();

        assert!(
            closed_within(&mut sock, GRACE).await,
            "an h2 peer that never ACKs a PING must not hold its streams forever"
        );
    }

    /// The regression that matters most. These deadlines exist to bound
    /// connections that are *not* making progress, and a response that streams
    /// for longer than `header_read` is making progress — a live feed, a slow
    /// query, a large download. If this ever fails, the timeouts have started
    /// cutting off working traffic, which is far worse than the hold they
    /// prevent.
    #[tokio::test]
    async fn a_response_that_streams_past_the_header_timeout_completes() {
        let timeouts = HttpTimeouts {
            header_read: Some(SHORT),
            handshake: Some(SHORT),
            h2_keep_alive: Some(SHORT),
        };
        let (http, id, port) = bound_with_timeouts(None, timeouts).await;

        let request = tokio::spawn(async move {
            let mut sock = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
            sock.write_all(b"GET /slow HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
            let mut got = Vec::new();
            sock.read_to_end(&mut got).await.unwrap();
            String::from_utf8_lossy(&got).into_owned()
        });

        let reqs = http.next_requests(id, 8).await.unwrap();
        let (rid, _) = reqs.into_iter().next().expect("one request");
        // Each chunk lands after the header timeout would have fired, and the
        // body runs well past it in total.
        let chunks = futures_util::stream::unfold(0u8, |i| async move {
            if i == 3 {
                return None;
            }
            tokio::time::sleep(SHORT * 2).await;
            Some((
                Ok::<_, ProviderError>(format!("part{i};").into_bytes()),
                i + 1,
            ))
        });
        http.respond(
            rid,
            HttpServerResponse {
                status: 200,
                headers: vec![],
                body: HttpServerBody::Stream(Box::pin(chunks)),
            },
        )
        .await
        .unwrap();

        let response = request.await.unwrap();
        assert!(response.contains("200 OK"), "{response}");
        // Chunk-framed, so the parts arrive with their lengths between them.
        for part in ["part0;", "part1;", "part2;"] {
            assert!(
                response.contains(part),
                "a slow but progressing response must complete: {part} missing from {response}"
            );
        }
        assert!(
            response.trim_end().ends_with('0'),
            "the terminal chunk must arrive, not a cut-off connection: {response}"
        );
    }

    /// Off means off. A guest that knows its deployment — a private port behind
    /// a proxy that already does this — can turn each one off and get the old
    /// unbounded behaviour back.
    #[tokio::test]
    async fn a_disabled_timeout_never_fires() {
        let (_http, _id, port) = bound_with_timeouts(None, OFF).await;

        let mut sock = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        assert!(
            !closed_within(&mut sock, SHORT * 5).await,
            "a silent connection must survive when the timeouts are disabled"
        );
    }
}
