//! Host ops backing `fetch` (SPEC §2.9), routed through the [`NetTransport`]
//! provider (capability-gated on `Capability::Net`).
//!
//! **Response** bodies stream rather than buffer: `fetch` performs the request
//! and stashes the body stream under an id, returning the response metadata;
//! `fetch_body_read` pulls the next chunk for that id (the prelude's `Response`
//! drives it from a `ReadableStream`).
//!
//! **Request** bodies stream too. When the guest passes a `ReadableStream` body,
//! the prelude:
//!   1. calls `fetch_request_body_new` to allocate a body-stream id (this creates
//!      a bounded channel — sender kept here, receiver handed to the request);
//!   2. calls `fetch` with that id, so the [`HttpRequest`] carries a
//!      [`RequestBody::Stream`] reading from the channel; and
//!   3. concurrently pumps the guest stream into `fetch_request_body_push`
//!      (one chunk per call — the bounded channel gives upload backpressure),
//!      ending with `fetch_request_body_close`.
//!
//! The request is sent with chunked transfer-encoding and never fully buffered.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use es_runtime_common::{Capability, ExceptionClass, IntoException};
use es_runtime_engine::{Engine, OpDecl, OpError, Value};
use es_runtime_providers::{
    ByteStream, HttpRequest, NetTransport, ProviderError, RedirectMode, RequestBody,
};
use futures_channel::{mpsc, oneshot};
use futures_util::future::{Either, select};
use futures_util::{SinkExt, StreamExt};

use crate::Result;

/// Bounded request-body channel capacity. The guest pump awaits each push, so at
/// most this many chunks (plus the one in flight) are buffered between the guest
/// `ReadableStream` and the transport — the backpressure that keeps a large
/// streamed upload from materializing in memory.
const REQUEST_BODY_BUFFER: usize = 8;

type ReqBodyItem = std::result::Result<Vec<u8>, ProviderError>;

/// Registers the `fetch` family of ops, sharing the body registries.
pub(crate) fn install(engine: &mut dyn Engine, net: Arc<dyn NetTransport>) -> Result<()> {
    // Active response-body streams, keyed by id: `fetch` inserts, `fetch_body_read`
    // drains.
    let bodies: Rc<RefCell<HashMap<u64, ByteStream>>> = Rc::new(RefCell::new(HashMap::new()));
    let resp_id_gen = Rc::new(Cell::new(1u64));

    // Streaming request bodies, keyed by id. `fetch_request_body_new` creates the
    // channel (storing both ends); `fetch` takes the receiver to build the request
    // stream; `fetch_request_body_push`/`_close` drive the sender.
    let req_senders: Rc<RefCell<HashMap<u64, mpsc::Sender<ReqBodyItem>>>> =
        Rc::new(RefCell::new(HashMap::new()));
    let req_receivers: Rc<RefCell<HashMap<u64, mpsc::Receiver<ReqBodyItem>>>> =
        Rc::new(RefCell::new(HashMap::new()));
    let req_id_gen = Rc::new(Cell::new(1u64));

    // Abort channels, keyed by id. `fetch_abort_new` allocates the pair before
    // the request starts (so the guest holds a handle it can fire at any time);
    // `fetch` takes the receiver and races it against the transport future, and
    // `fetch_abort` fires the sender. Losing the race *drops* the transport
    // future, which is what actually tears the connection down.
    let abort_txs: Rc<RefCell<HashMap<u64, oneshot::Sender<()>>>> =
        Rc::new(RefCell::new(HashMap::new()));
    let abort_rxs: Rc<RefCell<HashMap<u64, oneshot::Receiver<()>>>> =
        Rc::new(RefCell::new(HashMap::new()));
    let abort_id_gen = Rc::new(Cell::new(1u64));

    // ---- fetch_abort_new ----------------------------------------------------
    {
        let abort_txs = abort_txs.clone();
        let abort_rxs = abort_rxs.clone();
        let abort_id_gen = abort_id_gen.clone();
        engine.register_op(
            OpDecl::sync("fetch_abort_new", move |_args| {
                let id = abort_id_gen.get();
                abort_id_gen.set(id + 1);
                let (tx, rx) = oneshot::channel::<()>();
                abort_txs.borrow_mut().insert(id, tx);
                abort_rxs.borrow_mut().insert(id, rx);
                Ok(Value::Number(id as f64))
            })
            .requires(Capability::Net),
        )?;
    }

    // ---- fetch_abort --------------------------------------------------------
    {
        let abort_txs = abort_txs.clone();
        let abort_rxs = abort_rxs.clone();
        engine.register_op(
            OpDecl::sync("fetch_abort", move |args| {
                let id = args.first().and_then(Value::as_number).unwrap_or(0.0) as u64;
                // Firing the sender wakes the `select` in `fetch`. Dropping the
                // receiver too covers the case where the request already
                // finished (or never started) — both registries end up clean.
                if let Some(tx) = abort_txs.borrow_mut().remove(&id) {
                    let _ = tx.send(());
                }
                abort_rxs.borrow_mut().remove(&id);
                Ok(Value::Null)
            })
            .requires(Capability::Net),
        )?;
    }

    // ---- fetch_request_body_new --------------------------------------------
    {
        let req_senders = req_senders.clone();
        let req_receivers = req_receivers.clone();
        let req_id_gen = req_id_gen.clone();
        engine.register_op(
            OpDecl::sync("fetch_request_body_new", move |_args| {
                let id = req_id_gen.get();
                req_id_gen.set(id + 1);
                let (tx, rx) = mpsc::channel::<ReqBodyItem>(REQUEST_BODY_BUFFER);
                req_senders.borrow_mut().insert(id, tx);
                req_receivers.borrow_mut().insert(id, rx);
                Ok(Value::Number(id as f64))
            })
            .requires(Capability::Net),
        )?;
    }

    // ---- fetch --------------------------------------------------------------
    {
        let net = net;
        let bodies = bodies.clone();
        let req_receivers = req_receivers.clone();
        let abort_txs = abort_txs.clone();
        let abort_rxs = abort_rxs.clone();
        engine.register_op(
            OpDecl::r#async("fetch", move |args| {
                let net = net.clone();
                let bodies = bodies.clone();
                let resp_id_gen = resp_id_gen.clone();
                let abort_txs = abort_txs.clone();
                // Resolve a streaming request body (if any) synchronously, before
                // the await, so the receiver is owned by the request. Same for the
                // abort receiver, so an abort fired before the first poll still
                // lands on this request.
                let request = parse_request(&args, &req_receivers);
                let abort_id = args.get(4).and_then(Value::as_number).map(|n| n as u64);
                let abort_rx = abort_id.and_then(|id| abort_rxs.borrow_mut().remove(&id));
                Box::pin(async move {
                    let result = match abort_rx {
                        // Race the transport against the abort signal. If the
                        // abort wins, the transport future is dropped here —
                        // that is the cancellation, not a flag anyone polls.
                        Some(rx) => match select(net.fetch(request), rx).await {
                            Either::Left((response, _)) => response,
                            Either::Right((_, _)) => {
                                if let Some(id) = abort_id {
                                    abort_txs.borrow_mut().remove(&id);
                                }
                                return Err(OpError::new(
                                    ExceptionClass::DomException("AbortError"),
                                    "The operation was aborted.",
                                ));
                            }
                        },
                        None => net.fetch(request).await,
                    };
                    // The request is settled either way; retire its abort handle.
                    if let Some(id) = abort_id {
                        abort_txs.borrow_mut().remove(&id);
                    }
                    let response = result.map_err(|e| {
                        OpError::new(e.exception_class(), e.exception_message())
                            .with_code_opt(e.code())
                    })?;
                    let id = resp_id_gen.get();
                    resp_id_gen.set(id + 1);
                    bodies.borrow_mut().insert(id, response.body);
                    Ok(response_value(
                        response.status,
                        &response.status_text,
                        &response.url,
                        response.redirected,
                        &response.headers,
                        id,
                    ))
                })
            })
            .requires(Capability::Net),
        )?;
    }

    // ---- __fetch_inflight (test-only diagnostic) ----------------------------
    // Returns the live entry counts of the three body registries so soak/leak
    // tests can assert they drain to zero between requests. Not compiled into
    // release builds — purely an internal observability hook.
    #[cfg(test)]
    {
        let bodies_d = bodies.clone();
        let senders_d = req_senders.clone();
        let receivers_d = req_receivers.clone();
        engine.register_op(OpDecl::sync("__fetch_inflight", move |_args| {
            Ok(Value::Array(vec![
                Value::Number(bodies_d.borrow().len() as f64),
                Value::Number(senders_d.borrow().len() as f64),
                Value::Number(receivers_d.borrow().len() as f64),
            ]))
        }))?;
    }

    // ---- fetch_body_read ----------------------------------------------------
    {
        let bodies = bodies.clone();
        engine.register_op(OpDecl::r#async("fetch_body_read", move |args| {
            let bodies = bodies.clone();
            let id = args.first().and_then(Value::as_number).unwrap_or(0.0) as u64;
            Box::pin(async move {
                // Take the stream out so no RefCell borrow is held across the await.
                let stream = bodies.borrow_mut().remove(&id);
                let Some(mut stream) = stream else {
                    return Ok(Value::Null); // unknown id or already drained
                };
                match stream.next().await {
                    Some(Ok(chunk)) => {
                        bodies.borrow_mut().insert(id, stream);
                        Ok(Value::Bytes(chunk))
                    }
                    Some(Err(e)) => Err(OpError::new(e.exception_class(), e.exception_message())
                        .with_code_opt(e.code())),
                    None => Ok(Value::Null), // end of stream; not reinserted
                }
            })
        }))?;
    }

    // ---- fetch_body_cancel --------------------------------------------------
    // Drops a response body stream without draining it — the guest cancelled or
    // aborted mid-body. Dropping the stream closes the connection and keeps the
    // registry from retaining an entry nobody will ever read.
    {
        let bodies = bodies.clone();
        engine.register_op(
            OpDecl::sync("fetch_body_cancel", move |args| {
                let id = args.first().and_then(Value::as_number).unwrap_or(0.0) as u64;
                bodies.borrow_mut().remove(&id);
                Ok(Value::Null)
            })
            .requires(Capability::Net),
        )?;
    }

    // ---- fetch_request_body_push -------------------------------------------
    {
        let req_senders = req_senders.clone();
        engine.register_op(
            OpDecl::r#async("fetch_request_body_push", move |args| {
                let req_senders = req_senders.clone();
                let id = args.first().and_then(Value::as_number).unwrap_or(0.0) as u64;
                let chunk = args.get(1).and_then(Value::as_bytes).map(<[u8]>::to_vec);
                Box::pin(async move {
                    // Take the sender out so no borrow is held across the send await;
                    // the guest pump is sequential per id, so there is no contention.
                    let Some(mut tx) = req_senders.borrow_mut().remove(&id) else {
                        return Ok(Value::Bool(false)); // unknown/closed id
                    };
                    let chunk = chunk.unwrap_or_default();
                    match tx.send(Ok(chunk)).await {
                        // Accepted (channel had room or the transport drained one):
                        // put the sender back for the next chunk.
                        Ok(()) => {
                            req_senders.borrow_mut().insert(id, tx);
                            Ok(Value::Bool(true))
                        }
                        // Receiver gone (request finished or failed) — stop pumping.
                        Err(_) => Ok(Value::Bool(false)),
                    }
                })
            })
            .requires(Capability::Net),
        )?;
    }

    // ---- fetch_request_body_close ------------------------------------------
    {
        let req_senders = req_senders;
        engine.register_op(
            OpDecl::r#async("fetch_request_body_close", move |args| {
                let req_senders = req_senders.clone();
                let id = args.first().and_then(Value::as_number).unwrap_or(0.0) as u64;
                let err = args.get(1).and_then(Value::as_str).map(str::to_string);
                Box::pin(async move {
                    // Take the sender out (dropping the borrow before any await);
                    // letting it drop closes the channel → the request body stream
                    // ends cleanly at its next `None`.
                    let tx = req_senders.borrow_mut().remove(&id);
                    if let (Some(mut tx), Some(err)) = (tx, err) {
                        // Surface a guest-side stream error to the transport as an
                        // aborting item; best-effort (the receiver may be gone).
                        let _ = tx.send(Err(ProviderError::Other(err))).await;
                    }
                    Ok(Value::Null)
                })
            })
            .requires(Capability::Net),
        )?;
    }

    Ok(())
}

/// Parses the `fetch` op arguments and resolves any streaming request body.
///
/// Layout: `[method, url, body?, bodyStreamId?, abortId?, follow, name0, value0,
/// …]` — `body` is the buffered bytes (or null), `bodyStreamId` is the id from
/// `fetch_request_body_new` (or null) whose receiver becomes the request stream,
/// `abortId` is the id from `fetch_abort_new` (or null), and `follow` is whether
/// the transport should follow redirects. `abortId` is read by the caller, not
/// here.
///
/// Only Fetch's `"follow"` and `"manual"` reach the transport: `"error"` is a
/// rule about the response, so the prelude asks for `"manual"` and rejects on a
/// redirect status itself (see `RedirectMode`).
fn parse_request(
    args: &[Value],
    req_receivers: &Rc<RefCell<HashMap<u64, mpsc::Receiver<ReqBodyItem>>>>,
) -> HttpRequest {
    let method = args
        .first()
        .and_then(Value::as_str)
        .unwrap_or("GET")
        .to_string();
    let url = args
        .get(1)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    let body = if let Some(stream_id) = args.get(3).and_then(Value::as_number) {
        // Streaming body: take the receiver allocated by fetch_request_body_new.
        match req_receivers.borrow_mut().remove(&(stream_id as u64)) {
            Some(rx) => RequestBody::Stream(Box::pin(rx)),
            None => RequestBody::Empty, // id already consumed; treat as no body
        }
    } else if let Some(bytes) = args.get(2).and_then(Value::as_bytes) {
        RequestBody::Bytes(bytes.to_vec())
    } else {
        RequestBody::Empty
    };

    // Absent (an older caller) means the previous behaviour: follow.
    let redirect = match args.get(5) {
        Some(Value::Bool(false)) => RedirectMode::Manual,
        _ => RedirectMode::Follow,
    };

    let mut headers = Vec::new();
    let mut i = 6;
    while i + 1 < args.len() {
        if let (Some(name), Some(value)) = (args[i].as_str(), args[i + 1].as_str()) {
            headers.push((name.to_string(), value.to_string()));
        }
        i += 2;
    }
    HttpRequest {
        method,
        url,
        headers,
        body,
        redirect,
    }
}

fn response_value(
    status: u16,
    status_text: &str,
    url: &str,
    redirected: bool,
    headers: &[(String, String)],
    body_id: u64,
) -> Value {
    Value::Object(vec![
        ("status".to_string(), Value::Number(status as f64)),
        (
            "statusText".to_string(),
            Value::String(status_text.to_string()),
        ),
        ("url".to_string(), Value::String(url.to_string())),
        ("redirected".to_string(), Value::Bool(redirected)),
        ("bodyId".to_string(), Value::Number(body_id as f64)),
        (
            "headers".to_string(),
            Value::Array(
                headers
                    .iter()
                    .map(|(n, v)| {
                        Value::Array(vec![
                            Value::String(n.to_string()),
                            Value::String(v.to_string()),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}
