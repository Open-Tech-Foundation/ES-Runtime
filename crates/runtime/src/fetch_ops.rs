//! Host ops backing `fetch` (SPEC §2.9), routed through the [`NetTransport`]
//! provider (capability-gated on `Capability::Net`).
//!
//! Two ops cooperate so the response body can **stream** rather than buffer:
//! `fetch` performs the request and stashes the body stream under an id,
//! returning the response metadata as JSON; `fetch_body_read` pulls the next
//! chunk for that id (the prelude's `Response` drives it from a `ReadableStream`).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use es_runtime_common::{Capability, IntoException};
use es_runtime_engine::{Engine, OpDecl, OpError, Value};
use es_runtime_providers::{ByteStream, HttpRequest, NetTransport};
use futures_util::StreamExt;

use crate::Result;

/// Registers the `fetch` and `fetch_body_read` ops, sharing a body registry.
pub(crate) fn install(engine: &mut dyn Engine, net: Arc<dyn NetTransport>) -> Result<()> {
    // Active response-body streams, keyed by id. Shared (single-threaded) by the
    // two ops; the `fetch` op inserts, `fetch_body_read` drains.
    let bodies: Rc<RefCell<HashMap<u64, ByteStream>>> = Rc::new(RefCell::new(HashMap::new()));
    let next_id = Rc::new(Cell::new(1u64));

    let net_op = net;
    let bodies_for_fetch = bodies.clone();
    let id_gen = next_id;
    engine.register_op(
        OpDecl::r#async("fetch", move |args| {
            let net = net_op.clone();
            let bodies = bodies_for_fetch.clone();
            let id_gen = id_gen.clone();
            let request = parse_request(&args);
            Box::pin(async move {
                let response = net
                    .fetch(request)
                    .await
                    .map_err(|e| OpError::new(e.exception_class(), e.exception_message()))?;
                let id = id_gen.get();
                id_gen.set(id + 1);
                bodies.borrow_mut().insert(id, response.body);
                Ok(response_value(
                    response.status,
                    &response.status_text,
                    &response.url,
                    &response.headers,
                    id,
                ))
            })
        })
        .requires(Capability::Net),
    )?;

    let bodies_for_read = bodies;
    engine.register_op(OpDecl::r#async("fetch_body_read", move |args| {
        let bodies = bodies_for_read.clone();
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
                Some(Err(e)) => Err(OpError::new(e.exception_class(), e.exception_message())),
                None => Ok(Value::Null), // end of stream; not reinserted
            }
        })
    }))?;
    Ok(())
}

/// Parses the `fetch` op arguments: `[method, url, body?, name0, value0, …]`.
fn parse_request(args: &[Value]) -> HttpRequest {
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
    let body = args.get(2).and_then(Value::as_bytes).map(<[u8]>::to_vec);

    let mut headers = Vec::new();
    let mut i = 3;
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
    }
}

fn response_value(
    status: u16,
    status_text: &str,
    url: &str,
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
