//! Host ops backing the `WebSocket` global (DECISIONS D29), routed through the
//! [`WebSocketProvider`]. `ws_connect` is gated on `Capability::Net` — the same
//! boundary as `fetch` / `runtime:net` `connect` (D7). `ws_send`, `ws_recv`, and
//! `ws_close` operate by socket id (the connect that produced the id was already
//! authorized), so they need no capability — like `net_read`. All ops are async.
//! `ws_recv` returns a tagged object the prelude pump dispatches as a
//! `MessageEvent`/`CloseEvent`, or `null` for an abnormal close.

use std::sync::Arc;

use es_runtime_common::{Capability, ExceptionClass, IntoException};
use es_runtime_engine::{Engine, OpDecl, OpError, Value};
use es_runtime_providers::{ProviderError, SocketInfo, WebSocketProvider, WsIncoming, WsMessage};

use crate::Result;

pub(crate) fn install(
    engine: &mut dyn Engine,
    ws: Option<Arc<dyn WebSocketProvider>>,
) -> Result<()> {
    let w = ws.clone();
    engine.register_op(
        OpDecl::r#async("ws_connect", move |args| {
            let w = w.clone();
            let url = arg_str(&args, 0);
            let protocols = arg_str_vec(&args, 1);
            Box::pin(async move {
                let (id, info) = require(&w)?
                    .connect(url, protocols)
                    .await
                    .map_err(map_err)?;
                Ok(Value::Object(vec![
                    ("id".to_string(), Value::Number(id as f64)),
                    ("protocol".to_string(), Value::String(info.protocol)),
                    ("extensions".to_string(), Value::String(info.extensions)),
                ]))
            })
        })
        .requires(Capability::Net),
    )?;

    let w = ws.clone();
    engine.register_op(OpDecl::r#async("ws_send", move |args| {
        let w = w.clone();
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
            require(&w)?.send(id, message).await.map_err(map_err)?;
            Ok(Value::Undefined)
        })
    }))?;

    let w = ws.clone();
    engine.register_op(OpDecl::r#async("ws_recv", move |args| {
        let w = w.clone();
        let id = arg_u64(&args, 0);
        Box::pin(async move {
            match require(&w)?.recv(id).await.map_err(map_err)? {
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
    engine.register_op(OpDecl::r#async("ws_close", move |args| {
        let w = w.clone();
        let id = arg_u64(&args, 0);
        // `close()` with no code sends a bare close frame (code ⇒ None).
        let code = match args.get(1) {
            Some(Value::Number(n)) => Some(*n as u16),
            _ => None,
        };
        let reason = arg_str(&args, 2);
        Box::pin(async move {
            require(&w)?
                .close(id, code, reason)
                .await
                .map_err(map_err)?;
            Ok(Value::Undefined)
        })
    }))?;

    // Server side (`runtime:websocket` `serve()`): bind is gated on NetListen;
    // accept returns a connection id driven by the same ws_send/ws_recv/ws_close.
    let w = ws.clone();
    engine.register_op(
        OpDecl::r#async("ws_serve", move |args| {
            let w = w.clone();
            let host = arg_str(&args, 0);
            let port = arg_u16(&args, 1);
            Box::pin(async move {
                let (id, info) = require(&w)?.serve(host, port).await.map_err(map_err)?;
                Ok(server_value(id, &info))
            })
        })
        .requires(Capability::NetListen),
    )?;

    let w = ws.clone();
    engine.register_op(OpDecl::r#async("ws_accept", move |args| {
        let w = w.clone();
        let id = arg_u64(&args, 0);
        Box::pin(async move {
            match require(&w)?.accept(id).await.map_err(map_err)? {
                Some((cid, info)) => Ok(Value::Object(vec![
                    ("id".to_string(), Value::Number(cid as f64)),
                    ("protocol".to_string(), Value::String(info.protocol)),
                    ("extensions".to_string(), Value::String(info.extensions)),
                ])),
                None => Ok(Value::Null),
            }
        })
    }))?;

    engine.register_op(OpDecl::r#async("ws_close_server", move |args| {
        let w = ws.clone();
        let id = arg_u64(&args, 0);
        Box::pin(async move {
            require(&w)?.close_server(id).await.map_err(map_err)?;
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
    })
}

fn map_err(e: ProviderError) -> OpError {
    OpError::new(e.exception_class(), e.exception_message())
}
