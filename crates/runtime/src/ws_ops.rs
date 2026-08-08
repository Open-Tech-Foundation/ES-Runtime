//! Host ops backing the `WebSocket` global (DECISIONS D29), routed through the
//! [`WebSocketProvider`]. `ws_connect` is gated on `Capability::Net` — the same
//! boundary as `fetch` / `runtime:net` `connect` (D7). `ws_send`, `ws_recv`, and
//! `ws_close` operate by socket id, so they need no capability of their own —
//! but they do check the id is **this agent's** (D50, [`crate::handles`]): the
//! provider is shared across agents and its ids are sequential, so an unchecked
//! `ws_send` would let a worker with no capability write frames onto its
//! parent's connections. All ops are async.
//! `ws_recv` returns a tagged object the prelude pump dispatches as a
//! `MessageEvent`/`CloseEvent`, or `null` for an abnormal close.

use std::sync::Arc;

use es_runtime_common::{Capability, ErrorCode, ExceptionClass, IntoException};
use es_runtime_engine::{Engine, OpDecl, OpError, Value};
use es_runtime_providers::{
    HttpServerProvider, ProviderError, SocketInfo, WebSocketProvider, WsIncoming, WsMessage,
    WsServeOptions, WsTimeouts,
};

use crate::Result;
use crate::handles::Handles;

pub(crate) fn install(
    engine: &mut dyn Engine,
    ws: Option<Arc<dyn WebSocketProvider>>,
    http: Option<Arc<dyn HttpServerProvider>>,
    requests: Handles,
) -> Result<()> {
    // This agent's connections (dialled or accepted) and its bound servers.
    let connections = Handles::new("WebSocket");
    let servers = Handles::new("WebSocket server");

    let w = ws.clone();
    let owned = connections.clone();
    engine.register_op(
        OpDecl::r#async("ws_connect", move |args| {
            let w = w.clone();
            let owned = owned.clone();
            let url = arg_str(&args, 0);
            let protocols = arg_str_vec(&args, 1);
            Box::pin(async move {
                let (id, info) = require(&w)?
                    .connect(url, protocols)
                    .await
                    .map_err(map_err)?;
                Ok(Value::Object(vec![
                    ("id".to_string(), Value::Number(owned.own(id) as f64)),
                    ("protocol".to_string(), Value::String(info.protocol)),
                    ("extensions".to_string(), Value::String(info.extensions)),
                ]))
            })
        })
        .requires(Capability::Net),
    )?;

    // The HTTP-upgrade path (D55): `runtime:websocket`'s `upgradeWebSocket`
    // hands a request over to this module. Two providers, joined here rather
    // than at either seam — the HTTP server surrenders the connection, the
    // WebSocket provider adopts it, and neither has to know the other exists.
    //
    // No capability of its own: the request being upgraded arrived on a server
    // the guest already bound under `NetListen`, and taking that same connection
    // over reaches nothing new. What it does need is the *request* to be this
    // agent's, which `http_ops` recorded when it handed the id out (D50).
    let w = ws.clone();
    let h = http.clone();
    let owned = connections.clone();
    let owned_requests = requests;
    engine.register_op(OpDecl::r#async("ws_upgrade", move |args| {
        let w = w.clone();
        let h = h.clone();
        let owned = owned.clone();
        let owned_requests = owned_requests.clone();
        let rid = arg_u64(&args, 0);
        Box::pin(async move {
            let rid = owned_requests.check(rid)?;
            let http = h.clone().ok_or_else(|| {
                OpError::new(
                    ExceptionClass::Error,
                    "HTTP serving is unavailable (no HttpServerProvider configured)",
                )
                .with_code(ErrorCode::ProviderUnavailable)
            })?;
            // Resolves once the guest's `101` is on the wire — so this op is
            // started before the handler returns and settles after it.
            let io = http.upgrade(rid).await.map_err(map_err)?;
            let id = require(&w)?.adopt(io).await.map_err(map_err)?;
            Ok(Value::Number(owned.own(id) as f64))
        })
    }))?;

    let w = ws.clone();
    let owned = connections.clone();
    engine.register_op(OpDecl::r#async("ws_send", move |args| {
        let w = w.clone();
        let owned = owned.clone();
        let id = arg_u64(&args, 0);
        // A JS string ⇒ text frame; bytes (ArrayBuffer/typed-array) ⇒ binary.
        let message = match args.get(1) {
            Some(Value::String(s)) => WsMessage::Text(s.clone()),
            other => WsMessage::Binary(
                other
                    .and_then(Value::as_bytes)
                    .map(<[u8]>::to_vec)
                    .unwrap_or_default(),
            ),
        };
        Box::pin(async move {
            require(&w)?
                .send(owned.check(id)?, message)
                .await
                .map_err(map_err)?;
            Ok(Value::Undefined)
        })
    }))?;

    // Batched fan-out: one op crossing + one payload marshal for a message sent
    // to many connections (the `runtime:websocket` `broadcast()`); no capability
    // (operates on already-accepted/connected ids, like ws_send).
    let w = ws.clone();
    let owned = connections.clone();
    engine.register_op(OpDecl::r#async("ws_broadcast", move |args| {
        let w = w.clone();
        let owned = owned.clone();
        let ids = arg_u64_vec(&args, 0);
        let message = match args.get(1) {
            Some(Value::String(s)) => WsMessage::Text(s.clone()),
            other => WsMessage::Binary(
                other
                    .and_then(Value::as_bytes)
                    .map(<[u8]>::to_vec)
                    .unwrap_or_default(),
            ),
        };
        Box::pin(async move {
            // Every id in the fan-out, not just the first: a broadcast is a
            // list of sends, and one foreign entry is one foreign send.
            let ids = ids
                .into_iter()
                .map(|id| owned.check(id))
                .collect::<std::result::Result<Vec<_>, _>>()?;
            require(&w)?
                .broadcast(ids, message)
                .await
                .map_err(map_err)?;
            Ok(Value::Undefined)
        })
    }))?;

    let w = ws.clone();
    let owned = connections.clone();
    engine.register_op(OpDecl::r#async("ws_recv", move |args| {
        let w = w.clone();
        let owned = owned.clone();
        let id = arg_u64(&args, 0);
        Box::pin(async move {
            match require(&w)?.recv(owned.check(id)?).await.map_err(map_err)? {
                Some(WsIncoming::Text(s)) => Ok(frame("text", Value::String(s))),
                Some(WsIncoming::Binary(b)) => Ok(frame("binary", Value::Bytes(b))),
                Some(WsIncoming::Close { code, reason }) => Ok(Value::Object(vec![
                    ("type".to_string(), Value::String("close".to_string())),
                    ("code".to_string(), Value::Number(code as f64)),
                    ("reason".to_string(), Value::String(reason)),
                ])),
                None => Ok(Value::Null),
            }
        })
    }))?;

    let w = ws.clone();
    let owned = connections.clone();
    engine.register_op(OpDecl::r#async("ws_close", move |args| {
        let w = w.clone();
        let owned = owned.clone();
        let id = arg_u64(&args, 0);
        // `close()` with no code sends a bare close frame (code ⇒ None).
        let code = match args.get(1) {
            Some(Value::Number(n)) => Some(*n as u16),
            _ => None,
        };
        let reason = arg_str(&args, 2);
        Box::pin(async move {
            // Checked, not released. Closing a WebSocket does not end the id:
            // the prelude's drain keeps calling `ws_recv` afterwards, because a
            // local close is a *request* to close and `closed` settles on the
            // peer's answering frame. There is no single op that means "this
            // connection is over", and a connection is long-lived by design —
            // so this kind is retained rather than released, unlike a request
            // or a socket, whose ends are exact.
            require(&w)?
                .close(owned.check(id)?, code, reason)
                .await
                .map_err(map_err)?;
            Ok(Value::Undefined)
        })
    }))?;

    // Server side (`runtime:websocket` `serve()`): bind is gated on NetListen;
    // accept returns a connection id driven by the same ws_send/ws_recv/ws_close.
    let w = ws.clone();
    let owned = servers.clone();
    engine.register_op(
        OpDecl::r#async("ws_serve", move |args| {
            let w = w.clone();
            let owned = owned.clone();
            let host = arg_str(&args, 0);
            let port = arg_u16(&args, 1);
            // The defaults live in the provider, not here and not in JS: the
            // prelude sends `null` for "the guest said nothing", so there is one
            // copy of the number to keep true. Same crossing as `http_serve`.
            let timeouts = WsTimeouts {
                handshake: arg_timeout(&args, 2, WsTimeouts::default().handshake),
            };
            // `null`/absent ⇒ no limit, which is the default: the right number
            // follows from a deployment's descriptor budget, and a cap guessed
            // here would throttle real traffic silently.
            let max_connections = args
                .get(3)
                .and_then(Value::as_number)
                .filter(|n| *n >= 1.0 && n.is_finite())
                .map(|n| n as usize);
            // Also `null`/absent ⇒ no limit, and for a sharper reason: the count
            // is per address, so a deployment behind a proxy or a NAT would see
            // all of its traffic as one peer.
            let max_connections_per_ip = args
                .get(4)
                .and_then(Value::as_number)
                .filter(|n| *n >= 1.0 && n.is_finite())
                .map(|n| n as usize);
            // Unlike the caps above, absent means the provider's default rather
            // than "no bound": a queue nobody bounds is a peer that can spend
            // the host's memory, and the right number here does not depend on
            // the deployment. `0` is the guest turning it off, the same
            // spelling the timeouts use.
            let max_buffered_amount = match args.get(5).and_then(Value::as_number) {
                None => Some(WsServeOptions::DEFAULT_MAX_BUFFERED_AMOUNT),
                Some(n) if n <= 0.0 || !n.is_finite() => None,
                Some(n) => Some(n as u64),
            };
            Box::pin(async move {
                let (id, info) = require(&w)?
                    .serve(WsServeOptions {
                        host,
                        port,
                        timeouts,
                        max_connections,
                        max_connections_per_ip,
                        max_buffered_amount,
                    })
                    .await
                    .map_err(map_err)?;
                Ok(server_value(owned.own(id), &info))
            })
        })
        .requires(Capability::NetListen),
    )?;

    let w = ws.clone();
    let owned_servers = servers.clone();
    let owned_connections = connections;
    engine.register_op(OpDecl::r#async("ws_accept", move |args| {
        let w = w.clone();
        let owned_servers = owned_servers.clone();
        let owned_connections = owned_connections.clone();
        let id = arg_u64(&args, 0);
        Box::pin(async move {
            let id = owned_servers.check(id)?;
            match require(&w)?.accept(id).await.map_err(map_err)? {
                Some((cid, info)) => Ok(Value::Object(vec![
                    (
                        "id".to_string(),
                        Value::Number(owned_connections.own(cid) as f64),
                    ),
                    ("protocol".to_string(), Value::String(info.protocol)),
                    ("extensions".to_string(), Value::String(info.extensions)),
                ])),
                None => Ok(Value::Null),
            }
        })
    }))?;

    let owned = servers;
    engine.register_op(OpDecl::r#async("ws_close_server", move |args| {
        let w = ws.clone();
        let owned = owned.clone();
        let id = arg_u64(&args, 0);
        Box::pin(async move {
            require(&w)?
                .close_server(owned.check(id)?)
                .await
                .map_err(map_err)?;
            Ok(Value::Undefined)
        })
    }))?;

    Ok(())
}

/// A `{ id, hostname, port }` envelope for a bound server's address.
fn server_value(id: u64, info: &SocketInfo) -> Value {
    Value::Object(vec![
        ("id".to_string(), Value::Number(id as f64)),
        (
            "hostname".to_string(),
            Value::String(info.local_address.clone()),
        ),
        ("port".to_string(), Value::Number(info.local_port as f64)),
    ])
}

/// A `{ type, data }` envelope for an inbound text/binary message.
fn frame(kind: &str, data: Value) -> Value {
    Value::Object(vec![
        ("type".to_string(), Value::String(kind.to_string())),
        ("data".to_string(), data),
    ])
}

fn arg_str(args: &[Value], i: usize) -> String {
    args.get(i)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn arg_u64(args: &[Value], i: usize) -> u64 {
    args.get(i).and_then(Value::as_number).unwrap_or(0.0) as u64
}

fn arg_u16(args: &[Value], i: usize) -> u16 {
    args.get(i).and_then(Value::as_number).unwrap_or(0.0) as u16
}

/// A timeout in milliseconds from the prelude. Absent (the guest said nothing)
/// means `default`; `0` or a non-finite value means the guest turned it off.
/// Identical to `http_ops`' reading of the same crossing, deliberately — a
/// `timeouts` object should mean the same thing in both modules.
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

/// Collects a JS number array argument as `u64`s (non-numbers skipped).
fn arg_u64_vec(args: &[Value], i: usize) -> Vec<u64> {
    match args.get(i) {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_number().map(|n| n as u64))
            .collect(),
        _ => Vec::new(),
    }
}

/// Collects a JS string array argument (non-strings skipped); `[]` if absent.
fn arg_str_vec(args: &[Value], i: usize) -> Vec<String> {
    match args.get(i) {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

fn require(
    ws: &Option<Arc<dyn WebSocketProvider>>,
) -> std::result::Result<Arc<dyn WebSocketProvider>, OpError> {
    ws.clone().ok_or_else(|| {
        OpError::new(
            ExceptionClass::Error,
            "WebSocket is unavailable (no WebSocketProvider configured)",
        )
        .with_code(ErrorCode::ProviderUnavailable)
    })
}

fn map_err(e: ProviderError) -> OpError {
    OpError::new(e.exception_class(), e.exception_message()).with_code_opt(e.code())
}
