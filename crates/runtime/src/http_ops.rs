//! Host ops backing `runtime:http` (the `serve((req) => res)` shape), routed
//! through the [`HttpServerProvider`]. `http_serve` is gated on
//! `Capability::NetListen` (binding a listening socket, like `runtime:net`
//! `listen`) — the security boundary is the op (D7). `http_next_request`,
//! `http_body_read`, and `http_respond` operate by server/request id, so they
//! need no capability of their own — but they do check that the id is **this
//! agent's** (D50, [`crate::handles`]). The provider is shared across agents
//! and mints ids from one counter, so without that check a worker holding no
//! capability could answer its parent's in-flight requests: `http_respond(2,
//! …)` reaching a real request is a response hijack, not a guess that fails.
//! All ops are async unless noted.
//!
//! Bodies stream in both directions (mirroring the `fetch` ops):
//!
//! **Request** bodies: `http_next_request` stashes each request's body stream
//! under the request id and returns flat metadata (with `hasBody`); the prelude
//! pulls chunks via `http_body_read` (one chunk per call, `null` at end) into a
//! JS `ReadableStream`.
//!
//! **Response** bodies: a buffered body crosses inline on `http_respond`. For a
//! `ReadableStream` body the prelude first calls `http_response_body_new`
//! (allocating a bounded channel — NetListen-gated, since it mints a resource
//! not derived from an authorized id), passes the id to `http_respond` (whose
//! [`HttpServerResponse`] then carries the receiver as its body stream), and
//! pumps chunks through `http_response_body_push` — each push awaits the
//! bounded sender, giving download backpressure — ending with
//! `http_response_body_close`. Push/close are id-scoped, so ungated like
//! `http_respond`.
//!
//! A request body the handler never drained is dropped once its response can no
//! longer echo it: on a buffered `http_respond` immediately, or at
//! `http_response_body_close` for a streamed response (a streaming handler may
//! still be pumping the request body into the response — the proxy/echo shape).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use es_runtime_common::{Capability, ErrorCode, ExceptionClass, IntoException};
use es_runtime_engine::{Engine, OpDecl, OpError, Value};
use es_runtime_providers::{
    ByteStream, HttpServeOptions, HttpServerBody, HttpServerProvider, HttpServerResponse,
    HttpServerTls, HttpTimeouts, ProviderError, SocketInfo,
};
use futures_channel::mpsc;
use futures_util::{SinkExt, StreamExt};

use crate::Result;
use crate::handles::Handles;

/// Bounded response-body channel capacity — same rationale as the `fetch`
/// request-body buffer: the pump awaits each push, so at most this many chunks
/// sit between the guest `ReadableStream` and the provider.
const RESPONSE_BODY_BUFFER: usize = 8;

type BodyItem = std::result::Result<Vec<u8>, ProviderError>;
type Trailers = Vec<(String, String)>;

/// Trailer name/value pairs from a flat `[name, value, …]` JS array.
fn trailer_pairs(items: Vec<Value>) -> Trailers {
    let mut out = Vec::with_capacity(items.len() / 2);
    let mut it = items.into_iter();
    while let (Some(Value::String(name)), Some(Value::String(value))) = (it.next(), it.next()) {
        out.push((name, value));
    }
    out
}

pub(crate) fn install(
    engine: &mut dyn Engine,
    http: Option<Arc<dyn HttpServerProvider>>,
) -> Result<()> {
    // Inbound request-body streams, keyed by request id: `http_next_request`
    // inserts, `http_body_read` pulls chunks, response completion drops leftovers.
    let req_bodies: Rc<RefCell<HashMap<u64, ByteStream>>> = Rc::new(RefCell::new(HashMap::new()));

    // Streaming response bodies, keyed by body-stream id.
    // `http_response_body_new` creates the channel (storing both ends);
    // `http_respond` takes the receiver as the response body;
    // `http_response_body_push`/`_close` drive the sender. The rid map lets
    // `_close` drop the request's un-drained body stream (see module docs).
    let resp_senders: Rc<RefCell<HashMap<u64, mpsc::Sender<BodyItem>>>> =
        Rc::new(RefCell::new(HashMap::new()));
    let resp_receivers: Rc<RefCell<HashMap<u64, mpsc::Receiver<BodyItem>>>> =
        Rc::new(RefCell::new(HashMap::new()));
    let resp_rids: Rc<RefCell<HashMap<u64, u64>>> = Rc::new(RefCell::new(HashMap::new()));
    // Trailers for a *streamed* response are not known when the head goes out —
    // that is what trailers are for — so `http_respond` parks a sender here,
    // keyed by body-stream id, and `http_response_body_close` delivers them.
    let resp_trailers: Rc<RefCell<HashMap<u64, futures_channel::oneshot::Sender<Trailers>>>> =
        Rc::new(RefCell::new(HashMap::new()));
    let resp_id_gen = Rc::new(Cell::new(1u64));

    // The servers this agent bound, and the requests it has been handed. Both
    // come from the shared provider's id space; these are the halves of it this
    // agent may name. A request is released when it is answered — an id past
    // that names a response already on the wire.
    let servers = Handles::new("HTTP server");
    let requests = Handles::new("HTTP request");

    let h = http.clone();
    let owned = servers.clone();
    engine.register_op(
        // Args: [host, port, cert?, key?, handshakeMs?, headerReadMs?,
        // h2KeepAliveMs?, maxConnections?, alpn0, alpn1, …]. A cert *and* key (both non-empty)
        // turn on TLS termination; the cert/key travel inline because reading a
        // file is the filesystem's privilege, so serving HTTPS needs no grant
        // beyond NetListen. Each timeout is `null` for the provider default or
        // `0` to disable (see [`arg_timeout`]); the ALPN list stays last so it
        // can keep taking the whole tail.
        OpDecl::r#async("http_serve", move |args| {
            let h = h.clone();
            let owned = owned.clone();
            let host = arg_str(&args, 0);
            let port = arg_u16(&args, 1);
            let cert = arg_bytes(&args, 2);
            let key = arg_bytes(&args, 3);
            // The defaults live in the provider, not here and not in JS: the
            // prelude sends `null` for "the guest said nothing", so there is
            // one copy of the numbers to keep true.
            let fallback = HttpTimeouts::default();
            let timeouts = HttpTimeouts {
                handshake: arg_timeout(&args, 4, fallback.handshake),
                header_read: arg_timeout(&args, 5, fallback.header_read),
                h2_keep_alive: arg_timeout(&args, 6, fallback.h2_keep_alive),
            };
            // `null`/absent ⇒ no limit, which is the default: the right
            // number follows from a deployment's descriptor budget, and a cap
            // guessed here would throttle real traffic silently.
            let max_connections = args
                .get(7)
                .and_then(Value::as_number)
                .filter(|n| *n >= 1.0 && n.is_finite())
                .map(|n| n as usize);
            // `SO_REUSEPORT`: several processes sharing one listening port.
            // Ahead of the ALPN tail, which must stay last.
            let reuse_port = matches!(args.get(8), Some(Value::Bool(true)));
            let alpn: Vec<String> = args
                .iter()
                .skip(9)
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect();
            Box::pin(async move {
                let tls = match (cert, key) {
                    (Some(cert), Some(key)) if !cert.is_empty() && !key.is_empty() => {
                        Some(HttpServerTls {
                            cert,
                            key,
                            // The server speaks both versions, so it offers
                            // both, h2 first — ALPN order is the server's
                            // preference. A guest that wants one of them only
                            // says so with an explicit `alpn`.
                            alpn: if alpn.is_empty() {
                                vec!["h2".to_string(), "http/1.1".to_string()]
                            } else {
                                alpn
                            },
                        })
                    }
                    _ => None,
                };
                let options = HttpServeOptions {
                    host,
                    port,
                    tls,
                    timeouts,
                    max_connections,
                    reuse_port,
                };
                let (id, info) = require(&h)?.serve(options).await.map_err(map_err)?;
                Ok(server_value(owned.own(id), &info))
            })
        })
        .requires(Capability::NetListen),
    )?;

    // How many already-queued requests one `http_next_request` crossing may
    // drain. Amortizes the op dispatch + promise resolution + microtask
    // checkpoint over a batch; bounded so responses still flush promptly.
    const MAX_BATCH: usize = 64;

    let h = http.clone();
    let bodies_for_next = req_bodies.clone();
    let owned_servers = servers.clone();
    let owned_requests = requests.clone();
    engine.register_op(OpDecl::r#async("http_next_request", move |args| {
        let h = h.clone();
        let bodies = bodies_for_next.clone();
        let owned_servers = owned_servers.clone();
        let owned_requests = owned_requests.clone();
        let id = arg_u64(&args, 0);
        Box::pin(async move {
            let id = owned_servers.check(id)?;
            let reqs = require(&h)?
                .next_requests(id, MAX_BATCH)
                .await
                .map_err(map_err)?;
            if reqs.is_empty() {
                return Ok(Value::Null); // server closed
            }
            // A flat array to avoid recursive v8::Array allocations.
            // Format: [requestId, method, url, hasBody, peerHost, peerPort,
            // numHeaders, name1, val1, ...]
            let mut flat = Vec::new();
            for (rid, req) in reqs {
                let has_body = !req.body.is_empty();
                if has_body {
                    bodies.borrow_mut().insert(rid, into_stream(req.body));
                }
                flat.push(Value::Number(owned_requests.own(rid) as f64));
                flat.push(Value::String(req.method));
                flat.push(Value::String(req.url));
                flat.push(Value::Bool(has_body));
                // The socket peer, for the handler's second argument. Empty
                // when the provider has no peer to report, which the prelude
                // turns into a null `remoteAddr` rather than an address-shaped
                // object full of blanks.
                flat.push(Value::String(req.remote_address));
                flat.push(Value::Number(req.remote_port as f64));
                flat.push(Value::Number(req.headers.len() as f64));
                for (n, v) in req.headers {
                    flat.push(Value::String(n));
                    flat.push(Value::String(v));
                }
            }
            Ok(Value::Array(flat))
        })
    }))?;

    let bodies_for_read = req_bodies.clone();
    let owned = requests.clone();
    engine.register_op(OpDecl::r#async("http_body_read", move |args| {
        let bodies = bodies_for_read.clone();
        let owned = owned.clone();
        let rid = arg_u64(&args, 0);
        Box::pin(async move {
            let rid = owned.check(rid)?;
            // Take the stream out so no RefCell borrow is held across the await.
            let stream = bodies.borrow_mut().remove(&rid);
            let Some(mut stream) = stream else {
                return Ok(Value::Null); // unknown id, drained, or dropped
            };
            match stream.next().await {
                Some(Ok(chunk)) => {
                    bodies.borrow_mut().insert(rid, stream);
                    Ok(Value::Bytes(chunk))
                }
                Some(Err(e)) => Err(OpError::new(e.exception_class(), e.exception_message())
                    .with_code_opt(e.code())),
                None => Ok(Value::Null), // end of stream; not reinserted
            }
        })
    }))?;

    // ---- http_response_body_new ---------------------------------------------
    {
        let resp_senders = resp_senders.clone();
        let resp_receivers = resp_receivers.clone();
        engine.register_op(
            OpDecl::sync("http_response_body_new", move |_args| {
                let id = resp_id_gen.get();
                resp_id_gen.set(id + 1);
                let (tx, rx) = mpsc::channel::<BodyItem>(RESPONSE_BODY_BUFFER);
                resp_senders.borrow_mut().insert(id, tx);
                resp_receivers.borrow_mut().insert(id, rx);
                Ok(Value::Number(id as f64))
            })
            .requires(Capability::NetListen),
        )?;
    }

    // ---- http_respond ---------------------------------------------------------
    // Args: [requestId, status, body, bodyStreamId, trailers, name0, value0, …]
    // — `body` is the buffered payload (string/bytes, or null), `bodyStreamId`
    // the id from http_response_body_new (or null) whose receiver becomes the
    // stream. `trailers` is `null` for none, a flat `[name, value, …]` array for
    // trailers already in hand, or `true` for "they will arrive with the body's
    // close" — the streamed case, where their values depend on the body that has
    // not been produced yet.
    {
        let h = http.clone();
        let req_bodies = req_bodies.clone();
        let resp_receivers = resp_receivers;
        let resp_rids = resp_rids.clone();
        let resp_trailers = resp_trailers.clone();
        let owned = requests.clone();
        engine.register_op(OpDecl::r#async("http_respond", move |args| {
            let h = h.clone();
            let resp_trailers_for_respond = resp_trailers.clone();
            let mut it = args.into_iter();
            let rid = it.next().and_then(|v| v.as_number()).unwrap_or(0.0) as u64;
            let status = it.next().and_then(|v| v.as_number()).unwrap_or(0.0) as u16;

            // Checked here, in the synchronous half, so a refused id never
            // reaches the registries below it. Given up further down rather than
            // in one step with the check: a *streamed* response may still be
            // echoing the request body, so the rid outlives the respond that
            // started it and is released with the body instead.
            let rid = match owned.check(rid) {
                Ok(rid) => rid,
                Err(e) => return Box::pin(std::future::ready(Err(e))),
            };

            let buffered = match it.next() {
                Some(Value::String(s)) => Some(s.into_bytes()),
                Some(Value::Bytes(b)) => Some(b),
                Some(Value::Other(s)) => Some(s.into_bytes()),
                _ => None,
            };
            let stream_id = it.next().and_then(|v| v.as_number()).map(|n| n as u64);
            let trailers_arg = it.next();

            let body = match (buffered, stream_id) {
                (_, Some(sid)) => match resp_receivers.borrow_mut().remove(&sid) {
                    Some(rx) => {
                        // The streamed response may still be echoing the request
                        // body; defer its cleanup to http_response_body_close.
                        resp_rids.borrow_mut().insert(sid, rid);
                        HttpServerBody::Stream(Box::pin(rx))
                    }
                    None => HttpServerBody::Empty, // id already consumed
                },
                (Some(bytes), None) => HttpServerBody::Bytes(bytes),
                (None, None) => HttpServerBody::Empty,
            };
            // A buffered response is fully materialized, so nothing can still be
            // reading the request body — drop it if the handler left it behind,
            // and with it the request itself.
            if stream_id.is_none() {
                req_bodies.borrow_mut().remove(&rid);
                owned.release(rid);
            }

            let mut headers = Vec::new();
            while let (Some(name_val), Some(value_val)) = (it.next(), it.next()) {
                if let (Value::String(name), Value::String(value)) = (name_val, value_val) {
                    headers.push((name, value));
                }
            }

            let trailers: Option<es_runtime_providers::BoxFuture<Trailers>> = match trailers_arg {
                // Already in hand: a buffered body, so the guest could await
                // them before responding.
                Some(Value::Array(items)) => {
                    let pairs = trailer_pairs(items);
                    Some(Box::pin(std::future::ready(pairs)))
                }
                // Promised: park a sender for `http_response_body_close`. If it
                // is dropped instead — the guest closed without trailers, or the
                // response was torn down — the receiver resolves to none, which
                // is exactly "no trailers".
                Some(Value::Bool(true)) => {
                    let (tx, rx) = futures_channel::oneshot::channel::<Trailers>();
                    if let Some(sid) = stream_id {
                        resp_trailers_for_respond.borrow_mut().insert(sid, tx);
                    }
                    Some(Box::pin(async move { rx.await.unwrap_or_default() }))
                }
                _ => None,
            };

            Box::pin(async move {
                let response = HttpServerResponse {
                    status,
                    headers,
                    body,
                    trailers,
                };
                require(&h)?.respond(rid, response).await.map_err(map_err)?;
                Ok(Value::Undefined)
            })
        }))?;
    }

    // ---- http_response_body_push ---------------------------------------------
    {
        let resp_senders = resp_senders.clone();
        engine.register_op(OpDecl::r#async("http_response_body_push", move |args| {
            let resp_senders = resp_senders.clone();
            let id = arg_u64(&args, 0);
            let chunk = args.get(1).and_then(Value::as_bytes).map(<[u8]>::to_vec);
            Box::pin(async move {
                // Take the sender out so no borrow is held across the send await;
                // the guest pump is sequential per id, so there is no contention.
                let Some(mut tx) = resp_senders.borrow_mut().remove(&id) else {
                    return Ok(Value::Bool(false)); // unknown/closed id
                };
                let chunk = chunk.unwrap_or_default();
                match tx.send(Ok(chunk)).await {
                    // Accepted (channel had room or the provider drained one):
                    // put the sender back for the next chunk.
                    Ok(()) => {
                        resp_senders.borrow_mut().insert(id, tx);
                        Ok(Value::Bool(true))
                    }
                    // Receiver gone (client disconnected) — stop pumping.
                    Err(_) => Ok(Value::Bool(false)),
                }
            })
        }))?;
    }

    // ---- http_response_body_close --------------------------------------------
    // Args: [bodyStreamId, error?, trailers?] — `trailers` is a flat
    // `[name, value, …]` array when the guest promised them at respond time.
    {
        let resp_senders = resp_senders;
        let req_bodies = req_bodies;
        let resp_trailers_for_close = resp_trailers;
        let owned_requests_for_close = requests.clone();
        engine.register_op(OpDecl::r#async("http_response_body_close", move |args| {
            let resp_senders = resp_senders.clone();
            let req_bodies = req_bodies.clone();
            let resp_rids = resp_rids.clone();
            let owned_requests_for_close = owned_requests_for_close.clone();
            let resp_trailers = resp_trailers_for_close.clone();
            let id = arg_u64(&args, 0);
            let err = args.get(1).and_then(Value::as_str).map(str::to_string);
            // Trailers the guest promised at `http_respond` time, now that the
            // body they describe has been produced.
            let trailers = match args.get(2) {
                Some(Value::Array(items)) => Some(trailer_pairs(items.clone())),
                _ => None,
            };
            Box::pin(async move {
                // Deliver (or drop) the promised trailers before the body ends,
                // so the provider's trailer future is already settled by the
                // time it is polled. Dropping the sender means "none", which is
                // what a close with no trailers should look like.
                if let Some(tx) = resp_trailers.borrow_mut().remove(&id)
                    && let Some(trailers) = trailers
                {
                    let _ = tx.send(trailers);
                }
                // The response is over — drop the request's body stream too, if
                // the handler never finished (or started) draining it, and give
                // up the request id, which `http_respond` held onto for exactly
                // this moment.
                if let Some(rid) = resp_rids.borrow_mut().remove(&id) {
                    req_bodies.borrow_mut().remove(&rid);
                    owned_requests_for_close.release(rid);
                }
                // Take the sender out (dropping the borrow before any await);
                // letting it drop ends the response body cleanly at its next
                // `None`.
                let tx = resp_senders.borrow_mut().remove(&id);
                if let (Some(mut tx), Some(err)) = (tx, err) {
                    // Surface a guest-side stream error to the provider as an
                    // aborting item; best-effort (the receiver may be gone).
                    let _ = tx.send(Err(ProviderError::Other(err))).await;
                }
                Ok(Value::Null)
            })
        }))?;
    }

    // ---- http_request_disconnected -------------------------------------------
    // Resolves `true` if request `requestId`'s client went away before its
    // response was handed over, `false` once the response was delivered. Backs
    // the handler's `request.signal`. It always settles, so awaiting it cannot
    // hold the driven loop open past the request it belongs to.
    {
        let h = http.clone();
        let owned = requests;
        engine.register_op(OpDecl::r#async("http_request_disconnected", move |args| {
            let h = h.clone();
            let owned = owned.clone();
            let rid = arg_u64(&args, 0);
            Box::pin(async move {
                let rid = owned.check(rid)?;
                Ok(Value::Bool(require(&h)?.request_disconnected(rid).await))
            })
        }))?;
    }

    // Checked, not released — see `net_ops`' note on `net_close_listener`: a
    // server id is low-cardinality, and keeping it lets a `next_request` that
    // races the stop end the way it always did.
    let owned = servers;
    engine.register_op(OpDecl::r#async("http_close", move |args| {
        let h = http.clone();
        let owned = owned.clone();
        let id = arg_u64(&args, 0);
        Box::pin(async move {
            require(&h)?
                .close(owned.check(id)?)
                .await
                .map_err(map_err)?;
            Ok(Value::Undefined)
        })
    }))?;

    Ok(())
}

/// Widens a non-empty [`HttpServerBody`] into the one shape `http_body_read`
/// pulls from — buffered provider bytes become a one-chunk stream.
fn into_stream(body: HttpServerBody) -> ByteStream {
    match body {
        HttpServerBody::Stream(s) => s,
        HttpServerBody::Bytes(b) => Box::pin(futures_util::stream::iter([Ok(b)])),
        HttpServerBody::Empty => Box::pin(futures_util::stream::empty()),
    }
}

fn arg_str(args: &[Value], i: usize) -> String {
    args.get(i)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn arg_u16(args: &[Value], i: usize) -> u16 {
    args.get(i).and_then(Value::as_number).unwrap_or(0.0) as u16
}

/// Bytes at `i`, accepting either a typed array or a string (PEM is text, and
/// reading a cert file gives either shape depending on how the guest read it).
fn arg_bytes(args: &[Value], i: usize) -> Option<Vec<u8>> {
    match args.get(i) {
        Some(Value::Bytes(b)) => Some(b.clone()),
        Some(Value::String(s)) => Some(s.as_bytes().to_vec()),
        _ => None,
    }
}

/// One timeout from the prelude, in the three states the guest can express:
/// nothing said (`null`/absent) keeps `default`, `0` disables it, and a positive
/// number is that many milliseconds. The prelude has already rejected anything
/// that is not one of those, so a stray value here falls back rather than
/// failing a bind.
fn arg_timeout(
    args: &[Value],
    i: usize,
    default: Option<std::time::Duration>,
) -> Option<std::time::Duration> {
    match args.get(i).and_then(Value::as_number) {
        None => default,
        Some(ms) if ms <= 0.0 || !ms.is_finite() => None,
        Some(ms) => Some(std::time::Duration::from_millis(ms as u64)),
    }
}

fn arg_u64(args: &[Value], i: usize) -> u64 {
    args.get(i).and_then(Value::as_number).unwrap_or(0.0) as u64
}

fn require(
    http: &Option<Arc<dyn HttpServerProvider>>,
) -> std::result::Result<Arc<dyn HttpServerProvider>, OpError> {
    http.clone().ok_or_else(|| {
        OpError::new(
            ExceptionClass::Error,
            "HTTP serving is unavailable (no HttpServerProvider configured)",
        )
        .with_code(ErrorCode::ProviderUnavailable)
    })
}

fn map_err(e: ProviderError) -> OpError {
    OpError::new(e.exception_class(), e.exception_message()).with_code_opt(e.code())
}

fn server_value(id: u64, info: &SocketInfo) -> Value {
    Value::Object(vec![
        ("id".to_string(), Value::Number(id as f64)),
        (
            "localAddress".to_string(),
            Value::String(info.local_address.clone()),
        ),
        (
            "localPort".to_string(),
            Value::Number(info.local_port as f64),
        ),
    ])
}
