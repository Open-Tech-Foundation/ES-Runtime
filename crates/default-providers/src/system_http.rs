//! OS-backed [`HttpServerProvider`] — a hyper HTTP/1.1 server for `runtime:http`.
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
//! watcher reads the drop as the disconnect. TLS is not supported yet.

use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use es_runtime_providers::{
    BoxFuture, HttpServeOptions, HttpServerBody, HttpServerProvider, HttpServerRequest,
    HttpServerResponse, ProviderError, SocketInfo,
};
use futures_util::StreamExt;
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt, Full, StreamBody};
use hyper::body::{Body as _, Frame, Incoming};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tokio::sync::{Notify, mpsc, oneshot, watch};
use tokio::task::AbortHandle;

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

/// An [`HttpServerProvider`] over a hyper HTTP/1.1 server. The `Arc`s are cloned
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
/// The absolute URL is reconstructed from the listener's scheme plus the `Host`
/// header (falling back to the bound address).
fn to_server_request(req: Request<Incoming>, origin: &str) -> HttpServerRequest {
    let method = req.method().to_string();
    // `origin` is "<scheme>://<bound address>", the fallback when a request
    // carries no Host. A Host header replaces only the authority — the scheme
    // is the listener's, and a client cannot talk this into claiming https:.
    let (scheme, authority) = origin.split_once("://").unwrap_or(("http", origin));
    let host = req
        .headers()
        .get(hyper::header::HOST)
        .and_then(|v| v.to_str().ok())
        .filter(|h| !h.is_empty())
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
/// Generic over the transport so a plain `TcpStream` and a TLS stream share one
/// path: everything above the socket — the service, the response handoff, the
/// graceful shutdown — is identical, and duplicating it per scheme is how the
/// two quietly drift apart.
async fn serve_connection<S>(
    io: S,
    tx: mpsc::Sender<Pending>,
    origin: String,
    mut drain_rx: watch::Receiver<bool>,
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

    let conn = http1::Builder::new().serve_connection(TokioIo::new(io), service);
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

            let acceptor = tokio::spawn(async move {
                while let Ok((stream, _peer)) = listener.accept().await {
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
                                if let Ok(stream) = acceptor.accept(stream).await {
                                    serve_connection(stream, tx, origin, drain_rx).await;
                                }
                            }
                            None => serve_connection(stream, tx, origin, drain_rx).await,
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
        let http = SystemHttpServer::new();
        let (id, info) = http
            .serve(HttpServeOptions {
                host: "127.0.0.1".into(),
                port: 0,
                tls,
            })
            .await
            .unwrap();
        (http, id, info.local_port)
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
    fn self_signed() -> (Vec<u8>, Vec<u8>, CertificateDer<'static>) {
        let ck = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
        (
            ck.cert.pem().into_bytes(),
            ck.signing_key.serialize_pem().into_bytes(),
            ck.cert.der().clone(),
        )
    }

    fn tls_options(cert: Vec<u8>, key: Vec<u8>) -> HttpServerTls {
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
