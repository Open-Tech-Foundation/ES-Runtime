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
//!
//! The UDP ops ([`install_datagram`], D58) are the one place two capabilities
//! meet on one resource: `net_bind_datagram` is gated on `NetListen` and
//! `net_datagram_send`/`net_datagram_connect` on `Net`, because a datagram
//! socket is a server and a client at once.

use std::sync::Arc;

use es_runtime_common::{Capability, ErrorCode, ExceptionClass, IntoException};
use es_runtime_engine::{Engine, OpDecl, OpError, Value};
use es_runtime_providers::{
    ConnectOptions, DatagramOptions, ListenOptions, NetProvider, ProviderError, SocketInfo,
};

use crate::Result;
use crate::handles::Handles;

pub(crate) fn install(engine: &mut dyn Engine, net: Option<Arc<dyn NetProvider>>) -> Result<()> {
    // This agent's sockets and listeners. Separate registries because they are
    // separate namespaces in the provider: a socket id and a listener id may
    // collide, and `accept` on a socket is not a request worth honouring.
    let sockets = Handles::new("socket");
    let listeners = Handles::new("listener");
    let datagrams = Handles::new("datagram socket");

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
                // Extra trust anchors (PEM). Passed inline by the guest like
                // the server-side cert and key, so it needs no capability of
                // its own — and it can only ever make verification accept
                // *more* certificates, never skip it.
                ca: arg_bytes(&args, 5),
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
        let ca = arg_bytes(&args, 3);
        Box::pin(async move {
            let id = owned.check(id)?;
            let (new_id, info) = require(&n)?
                .start_tls(id, server_name, alpn, ca)
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

    // Checked but **not** released: a program holds a handful of listeners, not
    // one per unit of work, so retaining a closed one costs nothing measurable —
    // and it keeps teardown graceful, since an `accept()` that races the close
    // gets the provider's "this listener is done" rather than a refusal aimed at
    // a different mistake. The high-cardinality handles (sockets, requests,
    // children) are the ones that must give their ids back.
    let owned = listeners;
    let n = net.clone();
    engine.register_op(OpDecl::r#async("net_close_listener", move |args| {
        let n = n.clone();
        let owned = owned.clone();
        let id = arg_u64(&args, 0);
        Box::pin(async move {
            require(&n)?
                .close_listener(owned.check(id)?)
                .await
                .map_err(map_err)?;
            Ok(Value::Undefined)
        })
    }))?;

    install_datagram(engine, net, datagrams)?;

    Ok(())
}

/// The UDP half (DECISIONS D58), in its own registry: a datagram socket is not
/// a stream socket, and an id from one namespace must not name a resource in
/// the other.
///
/// The gating is the decision worth reading twice. `bind` requires
/// `NetListen` — it takes a port, and a process holding a port is reachable,
/// ephemeral or not — while `send` requires `Net`, because a datagram leaving
/// this host is reaching out. A UDP socket is both things at once, so it is
/// checked against both grants rather than whichever one it was created under.
/// The consequence is deliberate: a program that only receives needs `listen`
/// alone, and one that sends needs `net` *and* `listen`, since it cannot send
/// without first holding a port that answers.
fn install_datagram(
    engine: &mut dyn Engine,
    net: Option<Arc<dyn NetProvider>>,
    datagrams: Handles,
) -> Result<()> {
    let n = net.clone();
    let owned = datagrams.clone();
    engine.register_op(
        OpDecl::r#async("net_bind_datagram", move |args| {
            let n = n.clone();
            let owned = owned.clone();
            let host = arg_str(&args, 0);
            let port = arg_u16(&args, 1);
            let opts = DatagramOptions {
                reuse_port: arg_bool(&args, 2),
                reuse_address: arg_bool(&args, 3),
                broadcast: arg_bool(&args, 4),
                // Absent ⇒ the OS default, which is a different thing from any
                // number this layer could pick.
                ttl: arg_opt_u32(&args, 5),
                multicast_ttl: arg_opt_u32(&args, 6),
                multicast_loopback: match args.get(7) {
                    Some(Value::Bool(on)) => Some(*on),
                    _ => None,
                },
            };
            Box::pin(async move {
                let (id, info) = require(&n)?
                    .bind_datagram(host, port, opts)
                    .await
                    .map_err(map_err)?;
                Ok(socket_value(owned.own(id), &info))
            })
        })
        .requires(Capability::NetListen),
    )?;

    // No capability of its own: receiving is what the bind was authorized for,
    // and the ownership check is what stops another agent naming this socket.
    let n = net.clone();
    let owned = datagrams.clone();
    engine.register_op(OpDecl::r#async("net_datagram_receive", move |args| {
        let n = n.clone();
        let owned = owned.clone();
        let id = arg_u64(&args, 0);
        Box::pin(async move {
            match require(&n)?
                .receive(owned.check(id)?)
                .await
                .map_err(map_err)?
            {
                Some(datagram) => Ok(Value::Object(vec![
                    ("data".to_string(), Value::Bytes(datagram.data)),
                    ("address".to_string(), Value::String(datagram.address)),
                    ("port".to_string(), Value::Number(datagram.port as f64)),
                ])),
                None => Ok(Value::Null),
            }
        })
    }))?;

    // `Net`, and checked on **every** send: the destination is an argument here
    // rather than a property of the socket, so a single grant at bind time
    // would authorize reaching anywhere for the socket's whole life.
    let n = net.clone();
    let owned = datagrams.clone();
    engine.register_op(
        OpDecl::r#async("net_datagram_send", move |args| {
            let n = n.clone();
            let owned = owned.clone();
            let id = arg_u64(&args, 0);
            let data = arg_bytes(&args, 1);
            let host = arg_str(&args, 2);
            let port = arg_u16(&args, 3);
            // An empty host is the guest saying "the connected peer" — the only
            // way to send without naming a destination, and the provider
            // refuses it on a socket that has none.
            let to = (!host.is_empty()).then_some((host, port));
            Box::pin(async move {
                let sent = require(&n)?
                    .send_to(owned.check(id)?, data, to)
                    .await
                    .map_err(map_err)?;
                Ok(Value::Number(sent as f64))
            })
        })
        .requires(Capability::Net),
    )?;

    let n = net.clone();
    let owned = datagrams.clone();
    engine.register_op(
        OpDecl::r#async("net_datagram_connect", move |args| {
            let n = n.clone();
            let owned = owned.clone();
            let id = arg_u64(&args, 0);
            let host = arg_str(&args, 1);
            let port = arg_u16(&args, 2);
            Box::pin(async move {
                let id = owned.check(id)?;
                let info = require(&n)?
                    .connect_datagram(id, host, port)
                    .await
                    .map_err(map_err)?;
                // The same id: fixing a peer changes where the socket sends,
                // not which socket it is.
                Ok(socket_value(id, &info))
            })
        })
        .requires(Capability::Net),
    )?;

    // Membership is `NetListen`: joining a group is a subscription to traffic
    // addressed to this host, which is the inbound half of the grant.
    let n = net.clone();
    let owned = datagrams.clone();
    engine.register_op(
        OpDecl::r#async("net_datagram_multicast", move |args| {
            let n = n.clone();
            let owned = owned.clone();
            let id = arg_u64(&args, 0);
            let group = arg_str(&args, 1);
            let interface = arg_str(&args, 2);
            let join = arg_bool(&args, 3);
            Box::pin(async move {
                require(&n)?
                    .set_multicast_membership(owned.check(id)?, group, interface, join)
                    .await
                    .map_err(map_err)?;
                Ok(Value::Undefined)
            })
        })
        .requires(Capability::NetListen),
    )?;

    let owned = datagrams;
    engine.register_op(OpDecl::r#async("net_datagram_close", move |args| {
        let n = net.clone();
        let owned = owned.clone();
        let id = arg_u64(&args, 0);
        Box::pin(async move {
            require(&n)?
                .close_datagram(owned.check_and_release(id)?)
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

/// A non-negative integer argument, or `None` when the guest said nothing —
/// which is how a socket option asks for the OS default rather than a number
/// chosen here.
fn arg_opt_u32(args: &[Value], i: usize) -> Option<u32> {
    args.get(i)
        .and_then(Value::as_number)
        .filter(|n| n.is_finite() && *n >= 0.0 && *n <= f64::from(u32::MAX))
        .map(|n| n as u32)
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
