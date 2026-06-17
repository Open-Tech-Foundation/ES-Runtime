//! Host ops backing `runtime:net` (SPEC §12), routed through the [`NetProvider`].
//! `net_connect` is gated on `Capability::Net` and `net_listen` on
//! `Capability::NetListen` — the security boundary is the op (D7). Reads, writes,
//! accepts, and closes operate by socket/listener id (the connect/listen that
//! produced the id was already authorized), so they need no capability — like
//! `fetch_body_read`. All ops are async. `connect`/`listen`/`accept` return JSON
//! the prelude `JSON.parse`s; `read` returns bytes or null (EOF/closed).

use std::sync::Arc;

use es_runtime_common::{Capability, ExceptionClass, IntoException};
use es_runtime_engine::{Engine, OpDecl, OpError, Value};
use es_runtime_providers::{ConnectOptions, NetProvider, ProviderError, SocketInfo};

use crate::Result;

pub(crate) fn install(engine: &mut dyn Engine, net: Option<Arc<dyn NetProvider>>) -> Result<()> {
    let n = net.clone();
    engine.register_op(
        OpDecl::r#async("net_connect", move |args| {
            let n = n.clone();
            let host = arg_str(&args, 0);
            let port = arg_u16(&args, 1);
            // (secure, sni, alpn) mirror the WinterTC SocketOptions (D28).
            let sni = arg_str(&args, 3);
            let opts = ConnectOptions {
                secure: arg_bool(&args, 2),
                sni: (!sni.is_empty()).then_some(sni),
                alpn: arg_str_vec(&args, 4),
            };
            Box::pin(async move {
                let (id, info) = require(&n)?
                    .connect(host, port, opts)
                    .await
                    .map_err(map_err)?;
                Ok(socket_value(id, &info))
            })
        })
        .requires(Capability::Net),
    )?;

    let n = net.clone();
    engine.register_op(OpDecl::r#async("net_read", move |args| {
        let n = n.clone();
        let id = arg_u64(&args, 0);
        Box::pin(async move {
            match require(&n)?.read(id).await.map_err(map_err)? {
                Some(bytes) => Ok(Value::Bytes(bytes)),
                None => Ok(Value::Null),
            }
        })
    }))?;

    let n = net.clone();
    engine.register_op(OpDecl::r#async("net_write", move |args| {
        let n = n.clone();
        let id = arg_u64(&args, 0);
        let data = args
            .get(1)
            .and_then(Value::as_bytes)
            .map(<[u8]>::to_vec)
            .unwrap_or_default();
        Box::pin(async move {
            require(&n)?.write(id, data).await.map_err(map_err)?;
            Ok(Value::Undefined)
        })
    }))?;

    let n = net.clone();
    engine.register_op(OpDecl::r#async("net_shutdown", move |args| {
        let n = n.clone();
        let id = arg_u64(&args, 0);
        Box::pin(async move {
            require(&n)?.shutdown(id).await.map_err(map_err)?;
            Ok(Value::Undefined)
        })
    }))?;

    let n = net.clone();
    engine.register_op(OpDecl::r#async("net_close", move |args| {
        let n = n.clone();
        let id = arg_u64(&args, 0);
        Box::pin(async move {
            require(&n)?.close(id).await.map_err(map_err)?;
            Ok(Value::Undefined)
        })
    }))?;

    let n = net.clone();
    engine.register_op(
        OpDecl::r#async("net_listen", move |args| {
            let n = n.clone();
            let host = arg_str(&args, 0);
            let port = arg_u16(&args, 1);
            Box::pin(async move {
                let (id, info) = require(&n)?.listen(host, port).await.map_err(map_err)?;
                Ok(socket_value(id, &info))
            })
        })
        .requires(Capability::NetListen),
    )?;

    let n = net.clone();
    engine.register_op(OpDecl::r#async("net_accept", move |args| {
        let n = n.clone();
        let id = arg_u64(&args, 0);
        Box::pin(async move {
            match require(&n)?.accept(id).await.map_err(map_err)? {
                Some((sid, info)) => Ok(socket_value(sid, &info)),
                None => Ok(Value::Null),
            }
        })
    }))?;

    engine.register_op(OpDecl::r#async("net_close_listener", move |args| {
        let n = net.clone();
        let id = arg_u64(&args, 0);
        Box::pin(async move {
            require(&n)?.close_listener(id).await.map_err(map_err)?;
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

fn arg_bool(args: &[Value], i: usize) -> bool {
    matches!(args.get(i), Some(Value::Bool(true)))
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
    net: &Option<Arc<dyn NetProvider>>,
) -> std::result::Result<Arc<dyn NetProvider>, OpError> {
    net.clone().ok_or_else(|| {
        OpError::new(
            ExceptionClass::Error,
            "networking is unavailable (no NetProvider configured)",
        )
    })
}

fn map_err(e: ProviderError) -> OpError {
    OpError::new(e.exception_class(), e.exception_message())
}

fn socket_value(id: u64, info: &SocketInfo) -> Value {
    Value::Object(vec![
        ("id".to_string(), Value::Number(id as f64)),
        (
            "remoteAddress".to_string(),
            Value::String(info.remote_address.clone()),
        ),
        (
            "remotePort".to_string(),
            Value::Number(info.remote_port as f64),
        ),
        (
            "localAddress".to_string(),
            Value::String(info.local_address.clone()),
        ),
        (
            "localPort".to_string(),
            Value::Number(info.local_port as f64),
        ),
        (
            "alpn".to_string(),
            info.alpn.clone().map(Value::String).unwrap_or(Value::Null),
        ),
    ])
}
