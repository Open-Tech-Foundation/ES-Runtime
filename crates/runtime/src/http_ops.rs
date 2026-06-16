//! Host ops backing `runtime:http` (the `serve((req) => res)` shape), routed
//! through the [`HttpServerProvider`]. `http_serve` is gated on
//! `Capability::NetListen` (binding a listening socket, like `runtime:net`
//! `listen`) — the security boundary is the op (D7). `http_next_request`,
//! `http_body_read`, and `http_respond` operate by server/request id (the
//! authorized `serve` produced the id), so they need no capability — like
//! `fetch_body_read` and the `net_*` read/write ops. All ops are async.
//!
//! Request bodies are buffered: `http_next_request` stashes the body bytes under
//! the request id and returns JSON metadata (with `hasBody`); the prelude reads
//! the bytes once via `http_body_read`. Responses are buffered too —
//! `http_respond` takes `[requestId, status, body, name0, value0, …]`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use es_runtime_common::{Capability, ExceptionClass, IntoException};
use es_runtime_engine::{Engine, OpDecl, OpError, Value};
use es_runtime_providers::{HttpServerProvider, HttpServerResponse, ProviderError, SocketInfo};

use crate::Result;

pub(crate) fn install(
    engine: &mut dyn Engine,
    http: Option<Arc<dyn HttpServerProvider>>,
) -> Result<()> {
    // Buffered request bodies, keyed by request id. The single-threaded ops
    // share this: `http_next_request` inserts, `http_body_read` drains.
    let bodies: Rc<RefCell<HashMap<u64, Vec<u8>>>> = Rc::new(RefCell::new(HashMap::new()));

    let h = http.clone();
    engine.register_op(
        OpDecl::r#async("http_serve", move |args| {
            let h = h.clone();
            let host = arg_str(&args, 0);
            let port = arg_u16(&args, 1);
            Box::pin(async move {
                let (id, info) = require(&h)?.serve(host, port).await.map_err(map_err)?;
                Ok(Value::String(server_json(id, &info)))
            })
        })
        .requires(Capability::NetListen),
    )?;

    // How many already-queued requests one `http_next_request` crossing may
    // drain. Amortizes the op dispatch + promise resolution + microtask
    // checkpoint over a batch; bounded so responses still flush promptly.
    const MAX_BATCH: usize = 64;

    let h = http.clone();
    let bodies_for_next = bodies.clone();
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
                    bodies.borrow_mut().insert(rid, req.body);
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

    let bodies_for_read = bodies;
    engine.register_op(OpDecl::r#async("http_body_read", move |args| {
        let bodies = bodies_for_read.clone();
        let rid = arg_u64(&args, 0);
        Box::pin(async move {
            match bodies.borrow_mut().remove(&rid) {
                Some(bytes) => Ok(Value::Bytes(bytes)),
                None => Ok(Value::Null),
            }
        })
    }))?;

    let h = http.clone();
    engine.register_op(OpDecl::r#async("http_respond", move |args| {
        let h = h.clone();
        let mut it = args.into_iter();
        let rid = it.next().and_then(|v| v.as_number()).unwrap_or(0.0) as u64;
        let status = it.next().and_then(|v| v.as_number()).unwrap_or(0.0) as u16;
        
        let body = match it.next() {
            Some(Value::String(s)) => s.into_bytes(),
            Some(Value::Bytes(b)) => b,
            Some(Value::Other(s)) => s.into_bytes(),
            _ => Vec::new(),
        };

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
    })
}

fn map_err(e: ProviderError) -> OpError {
    OpError::new(e.exception_class(), e.exception_message())
}

/// `{"id":N,"localAddress":"…","localPort":N}` for the prelude to `JSON.parse`.
fn server_json(id: u64, info: &SocketInfo) -> String {
    let mut out = format!("{{\"id\":{id},\"localAddress\":");
    push_json_string(&mut out, &info.local_address);
    out.push_str(&format!(",\"localPort\":{}}}", info.local_port));
    out
}

fn push_json_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}
