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
    BoxFuture, HttpServerBody, HttpServerProvider, HttpServerRequest, HttpServerResponse,
    ProviderError, SocketInfo,
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
use tokio::sync::{Notify, mpsc, oneshot};
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
    next_id: Arc<AtomicU64>,
}

impl SystemHttpServer {
    /// Builds an empty server registry.
    pub fn new() -> Self {
        Self::default()
    }

    fn id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed) + 1
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
/// The absolute URL is reconstructed from the `Host` header (or `authority`
/// fallback — the bound address).
fn to_server_request(req: Request<Incoming>, authority: &str) -> HttpServerRequest {
    let method = req.method().to_string();
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
    let url = format!("http://{host}{path}");
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

impl HttpServerProvider for SystemHttpServer {
    fn serve(
        &self,
        host: String,
        port: u16,
    ) -> BoxFuture<Result<(u64, SocketInfo), ProviderError>> {
        let this = self.clone();
        Box::pin(async move {
            let listener = TcpListener::bind((host.as_str(), port))
                .await
                .map_err(err)?;
            let local = listener.local_addr().ok();
            let authority = local.map(|a| a.to_string()).unwrap_or_default();
            // Roomy buffer so many connections can have a request queued for the
            // consumer to drain in one batch (see `next_requests`), rather than
            // stalling on backpressure between crossings.
            let (tx, rx) = mpsc::channel::<Pending>(1024);

            let acceptor = tokio::spawn(async move {
                while let Ok((stream, _peer)) = listener.accept().await {
                    let _ = stream.set_nodelay(true);
                    let io = TokioIo::new(stream);
                    let tx = tx.clone();
                    let authority = authority.clone();
                    tokio::spawn(async move {
                        let service = service_fn(move |req: Request<Incoming>| {
                            let tx = tx.clone();
                            let authority = authority.clone();
                            async move {
                                let server_req = to_server_request(req, &authority);
                                let (rtx, rrx) = oneshot::channel();
                                let (dtx, drx) = oneshot::channel();
                                if tx.send((server_req, rtx, drx)).await.is_err() {
                                    // Server closed: the request channel is gone.
                                    return Ok::<_, Infallible>(status_only(
                                        StatusCode::SERVICE_UNAVAILABLE,
                                    ));
                                }
                                match rrx.await {
                                    Ok(resp) => {
                                        // Say "delivered" before building, so a
                                        // handler awaiting the disconnect signal
                                        // learns the request completed. If this
                                        // future is instead dropped — which is
                                        // what a vanished client does to it —
                                        // `dtx` drops unsent and the watcher
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
                        let _ = http1::Builder::new().serve_connection(io, service).await;
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
        let http = SystemHttpServer::new();
        let (id, info) = http.serve("127.0.0.1".into(), 0).await.unwrap();
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
}
