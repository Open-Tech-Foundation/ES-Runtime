//! Host ops backing `runtime:net` (SPEC §12), routed through the [`NetProvider`].
//! `net_connect` is gated on `Capability::Net` and `net_listen` on
//! `Capability::NetListen` — the security boundary is the op (D7). Reads, writes,
//! accepts, and closes operate by socket/listener id, so they carry no
//! capability of their own; what they carry instead is an **ownership check**
//! (D50, [`crate::handles`]): the id must be one *this agent* got back from a
//! checked `connect`/`listen`/`accept`. The provider is shared across agents
//! and its ids are sequential, so without that check a worker holding no
//! capability at all could read and write another agent's sockets by naming
//! small integers. All ops are async. `connect`/`listen`/`accept` return JSON
//! the prelude `JSON.parse`s; `read` returns bytes or null (EOF/closed).

use std::sync::Arc;

use es_runtime_common::{Capability, ErrorCode, ExceptionClass, IntoException};
use es_runtime_engine::{Engine, OpDecl, OpError, Value};
use es_runtime_providers::{ConnectOptions, ListenOptions, NetProvider, ProviderError, SocketInfo};

use crate::Result;
use crate::handles::Handles;

pub(crate) fn install(engine: &mut dyn Engine, net: Option<Arc<dyn NetProvider>>) -> Result<()> {
    // This agent's sockets and listeners. Separate registries because they are
    // separate namespaces in the provider: a socket id and a listener id may
    // collide, and `accept` on a socket is not a request worth honouring.
    let sockets = Handles::new("socket");
    let listeners = Handles::new("listener");

    let n = net.clone();
    let owned = sockets.clone();
    engine.register_op(
        OpDecl::r#async("net_connect", move |args| {
            let n = n.clone();
            let owned = owned.clone();
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
                Ok(socket_value(owned.own(id), &info))
            })
        })
        .requires(Capability::Net),
    )?;

    // Upgrades a plaintext "starttls" socket to TLS in place. Like read/write it
    // needs no capability — the original `connect` was already authorized (D7) —
    // but it does need the socket to be this agent's, and the upgrade replaces
    // the handle, so the old id is given up for the new one.
    let n = net.clone();
    let owned = sockets.clone();
    engine.register_op(OpDecl::r#async("net_start_tls", move |args| {
        let n = n.clone();
        let owned = owned.clone();
        let id = arg_u64(&args, 0);
        let server_name = arg_str(&args, 1);
        let alpn = arg_str_vec(&args, 2);
        Box::pin(async move {
            let id = owned.check(id)?;
            let (new_id, info) = require(&n)?
                .start_tls(id, server_name, alpn)
                .await
                .map_err(map_err)?;
            owned.release(id);
            Ok(socket_value(owned.own(new_id), &info))
        })
    }))?;

    let n = net.clone();
    let owned = sockets.clone();
    engine.register_op(OpDecl::r#async("net_read", move |args| {
        let n = n.clone();
        let owned = owned.clone();
        let id = arg_u64(&args, 0);
        Box::pin(async move {
            match require(&n)?.read(owned.check(id)?).await.map_err(map_err)? {
                Some(bytes) => Ok(Value::Bytes(bytes)),
                None => Ok(Value::Null),
            }
        })
    }))?;

    let n = net.clone();
    let owned = sockets.clone();
    engine.register_op(OpDecl::r#async("net_write", move |args| {
        let n = n.clone();
        let owned = owned.clone();
        let id = arg_u64(&args, 0);
        let data = args
            .get(1)
            .and_then(Value::as_bytes)
            .map(<[u8]>::to_vec)
            .unwrap_or_default();
        Box::pin(async move {
            require(&n)?
                .write(owned.check(id)?, data)
                .await
                .map_err(map_err)?;
            Ok(Value::Undefined)
        })
    }))?;

    let n = net.clone();
    let owned = sockets.clone();
    engine.register_op(OpDecl::r#async("net_shutdown", move |args| {
        let n = n.clone();
        let owned = owned.clone();
        let id = arg_u64(&args, 0);
        Box::pin(async move {
            require(&n)?
                .shutdown(owned.check(id)?)
                .await
                .map_err(map_err)?;
            Ok(Value::Undefined)
        })
    }))?;

    let n = net.clone();
    let owned = sockets.clone();
    engine.register_op(OpDecl::r#async("net_close", move |args| {
        let n = n.clone();
        let owned = owned.clone();
        let id = arg_u64(&args, 0);
        Box::pin(async move {
            require(&n)?
                .close(owned.check_and_release(id)?)
                .await
                .map_err(map_err)?;
            Ok(Value::Undefined)
        })
    }))?;

    let n = net.clone();
    let owned = listeners.clone();
    engine.register_op(
        OpDecl::r#async("net_listen", move |args| {
            let n = n.clone();
            let owned = owned.clone();
            let host = arg_str(&args, 0);
            let port = arg_u16(&args, 1);
            // (cert, key, alpn) carry server-side TLS termination (D28): empty
            // cert+key ⇒ plaintext. The PEM material is passed inline by the
            // guest, so no capability beyond NetListen is needed.
            let opts = ListenOptions {
                cert: arg_bytes(&args, 2),
                key: arg_bytes(&args, 3),
                alpn: arg_str_vec(&args, 4),
                // `SO_REUSEPORT`: several processes sharing one listening port.
                reuse_port: matches!(args.get(5), Some(Value::Bool(true))),
            };
            Box::pin(async move {
                let (id, info) = require(&n)?
                    .listen(host, port, opts)
                    .await
                    .map_err(map_err)?;
                Ok(socket_value(owned.own(id), &info))
            })
        })
        .requires(Capability::NetListen),
    )?;

    // An accepted connection is this agent's socket — the listener it came off
    // was authorized, and nobody else has been told the id.
    let n = net.clone();
    let owned_listeners = listeners.clone();
    let owned_sockets = sockets.clone();
    engine.register_op(OpDecl::r#async("net_accept", move |args| {
        let n = n.clone();
        let owned_listeners = owned_listeners.clone();
        let owned_sockets = owned_sockets.clone();
        let id = arg_u64(&args, 0);
        Box::pin(async move {
            let id = owned_listeners.check(id)?;
            match require(&n)?.accept(id).await.map_err(map_err)? {
                Some((sid, info)) => Ok(socket_value(owned_sockets.own(sid), &info)),
                None => Ok(Value::Null),
            }
        })
    }))?;

    let owned = listeners;
    engine.register_op(OpDecl::r#async("net_close_listener", move |args| {
        let n = net.clone();
        let owned = owned.clone();
        let id = arg_u64(&args, 0);
        Box::pin(async move {
            require(&n)?
                .close_listener(owned.check_and_release(id)?)
                .await
                .map_err(map_err)?;
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

/// Collects a byte argument (a JS `Uint8Array`); empty if absent or not bytes.
fn arg_bytes(args: &[Value], i: usize) -> Vec<u8> {
    args.get(i)
        .and_then(Value::as_bytes)
        .map(<[u8]>::to_vec)
        .unwrap_or_default()
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
        .with_code(ErrorCode::ProviderUnavailable)
    })
}

fn map_err(e: ProviderError) -> OpError {
    OpError::new(e.exception_class(), e.exception_message()).with_code_opt(e.code())
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
