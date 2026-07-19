//! Host ops backing `runtime:http` (the `serve((req) => res)` shape), routed
//! through the [`HttpServerProvider`]. `http_serve` is gated on
//! `Capability::NetListen` (binding a listening socket, like `runtime:net`
//! `listen`) — the security boundary is the op (D7). `http_next_request`,
//! `http_body_read`, and `http_respond` operate by server/request id (the
//! authorized `serve` produced the id), so they need no capability — like
//! `fetch_body_read` and the `net_*` read/write ops. All ops are async unless
//! noted.
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
    ByteStream, HttpServerBody, HttpServerProvider, HttpServerResponse, ProviderError, SocketInfo,
};
use futures_channel::mpsc;
use futures_util::{SinkExt, StreamExt};

use crate::Result;

/// Bounded response-body channel capacity — same rationale as the `fetch`
/// request-body buffer: the pump awaits each push, so at most this many chunks
/// sit between the guest `ReadableStream` and the provider.
const RESPONSE_BODY_BUFFER: usize = 8;

type BodyItem = std::result::Result<Vec<u8>, ProviderError>;

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
    let resp_id_gen = Rc::new(Cell::new(1u64));

    let h = http.clone();
    engine.register_op(
        OpDecl::r#async("http_serve", move |args| {
            let h = h.clone();
            let host = arg_str(&args, 0);
            let port = arg_u16(&args, 1);
            Box::pin(async move {
                let (id, info) = require(&h)?.serve(host, port).await.map_err(map_err)?;
                Ok(server_value(id, &info))
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
    engine.register_op(OpDecl::r#async("http_next_request", move |args| {
        let h = h.clone();
        let bodies = bodies_for_next.clone();
        let id = arg_u64(&args, 0);
        Box::pin(async move {
            let reqs = require(&h)?
                .next_requests(id, MAX_BATCH)
                .await
                .map_err(map_err)?;
            if reqs.is_empty() {
                return Ok(Value::Null); // server closed
            }
            // A flat array to avoid recursive v8::Array allocations.
            // Format: [requestId, method, url, hasBody, numHeaders, name1, val1, ...]
            let mut flat = Vec::new();
            for (rid, req) in reqs {
                let has_body = !req.body.is_empty();
                if has_body {
                    bodies.borrow_mut().insert(rid, into_stream(req.body));
                }
                flat.push(Value::Number(rid as f64));
                flat.push(Value::String(req.method));
                flat.push(Value::String(req.url));
                flat.push(Value::Bool(has_body));
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
    engine.register_op(OpDecl::r#async("http_body_read", move |args| {
        let bodies = bodies_for_read.clone();
        let rid = arg_u64(&args, 0);
        Box::pin(async move {
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
    // Args: [requestId, status, body, bodyStreamId, name0, value0, …] — `body`
    // is the buffered payload (string/bytes, or null), `bodyStreamId` the id
    // from http_response_body_new (or null) whose receiver becomes the stream.
    {
        let h = http.clone();
        let req_bodies = req_bodies.clone();
        let resp_receivers = resp_receivers;
        let resp_rids = resp_rids.clone();
        engine.register_op(OpDecl::r#async("http_respond", move |args| {
            let h = h.clone();
            let mut it = args.into_iter();
            let rid = it.next().and_then(|v| v.as_number()).unwrap_or(0.0) as u64;
            let status = it.next().and_then(|v| v.as_number()).unwrap_or(0.0) as u16;

            let buffered = match it.next() {
                Some(Value::String(s)) => Some(s.into_bytes()),
                Some(Value::Bytes(b)) => Some(b),
                Some(Value::Other(s)) => Some(s.into_bytes()),
                _ => None,
            };
            let stream_id = it.next().and_then(|v| v.as_number()).map(|n| n as u64);

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
            // reading the request body — drop it if the handler left it behind.
            if stream_id.is_none() {
                req_bodies.borrow_mut().remove(&rid);
            }

            let mut headers = Vec::new();
            while let (Some(name_val), Some(value_val)) = (it.next(), it.next()) {
                if let (Value::String(name), Value::String(value)) = (name_val, value_val) {
                    headers.push((name, value));
                }
            }

            Box::pin(async move {
                let response = HttpServerResponse {
                    status,
                    headers,
                    body,
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

    // ---- http_response_body_close ---------------------------------------------
    {
        let resp_senders = resp_senders;
        let req_bodies = req_bodies;
        engine.register_op(OpDecl::r#async("http_response_body_close", move |args| {
            let resp_senders = resp_senders.clone();
            let req_bodies = req_bodies.clone();
            let resp_rids = resp_rids.clone();
            let id = arg_u64(&args, 0);
            let err = args.get(1).and_then(Value::as_str).map(str::to_string);
            Box::pin(async move {
                // The response is over — drop the request's body stream too, if
                // the handler never finished (or started) draining it.
                if let Some(rid) = resp_rids.borrow_mut().remove(&id) {
                    req_bodies.borrow_mut().remove(&rid);
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

    engine.register_op(OpDecl::r#async("http_close", move |args| {
        let h = http.clone();
        let id = arg_u64(&args, 0);
        Box::pin(async move {
            require(&h)?.close(id).await.map_err(map_err)?;
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
