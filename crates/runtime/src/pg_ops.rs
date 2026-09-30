//! Host ops behind `runtime:db`'s built-in PostgreSQL driver (DECISIONS.md
//! D147), over the [`crate::postgres`] engine.
//!
//! **Authority.** Connecting is the only gated op, and it is gated on `Net`
//! exactly as `runtime:net`'s `connect` is: the engine reaches the server
//! through the agent's own [`NetProvider`], so the host allowlist, TLS roots
//! and socket ownership apply unchanged. Every op after that carries an
//! ownership check on the connection id (D50) and no capability.
//!
//! **The hot path is one op.** `pg_query` prepares if it must, binds, executes
//! and returns the first batch; a result that fits in it is finished by then.
//! The pool stays in JavaScript (D56), where acquiring and releasing cost no
//! op at all.
//!
//! **Failures resolve rather than throw.** A server error carries a dozen
//! fields an application reads — the SQLSTATE, the constraint, the hint — and
//! the driver builds its `DbError` from them, so an op answers
//! `{ error: { kind, message, fields } }` and leaves the throwing to the
//! driver. What throws is only what is not the database's to say: a refused
//! capability, an id that is not this agent's.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use es_runtime_common::{Capability, ErrorCode, ExceptionClass, IntoException};
use es_runtime_engine::{Engine, OpDecl, OpError, Value};
use es_runtime_providers::{Entropy, NetProvider};

use crate::Result;
use crate::handles::Handles;
use crate::postgres::{
    Aside, Batch, CancelTarget, Connection, Event, Failure, Options, SslMode, Started,
};

/// Connections by id. A slot is empty while an op has the connection out: a
/// connection is one conversation, and the driver never runs two at once on
/// it, so finding one checked out is a bug worth naming rather than a queue.
type Slots = Arc<Mutex<HashMap<u64, Option<Box<Connection>>>>>;

/// What stays reachable about a connection while an op has it checked out: a
/// cancel is sent *during* the query it cancels, a `LISTEN` is written *under*
/// the read loop that holds the connection, and closing a subscribed
/// connection has to reach a socket that loop is blocked reading.
struct Side {
    socket: u64,
    cancel: CancelTarget,
    /// Given over to a read loop (`pg_listen_next`): the only state in which
    /// writing a command underneath the connection is sound, because the loop
    /// is what reads the reply.
    subscribed: bool,
}

type Sides = Arc<Mutex<HashMap<u64, Side>>>;

/// Connects in flight: ticket → the socket opened so far, if any.
type Connecting = Arc<Mutex<HashMap<u64, Arc<Mutex<Option<u64>>>>>>;

pub(crate) fn install(
    engine: &mut dyn Engine,
    net: Option<Arc<dyn NetProvider>>,
    entropy: Arc<dyn Entropy>,
    inventory: &crate::handles::Inventory,
) -> Result<()> {
    let owned = inventory.track(Handles::new("PostgreSQL connection"));
    let slots: Slots = Arc::new(Mutex::new(HashMap::new()));
    let next_id = Arc::new(AtomicU64::new(1));
    // Connects in flight, by the ticket the driver chose, each with the socket
    // it has opened so far. Per agent, like everything here: one agent cannot
    // abort another's connect by guessing its ticket.
    let connecting: Connecting = Arc::new(Mutex::new(HashMap::new()));
    let sides: Sides = Arc::new(Mutex::new(HashMap::new()));

    // Args: [host, port, sslmode, ca (bytes, empty for none), params (flat
    // [key, value, …]), password (string or null), cacheLimit, binaryOids,
    // ticket].
    {
        let net = net.clone();
        let owned = owned.clone();
        let slots = slots.clone();
        let connecting = connecting.clone();
        let sides = sides.clone();
        engine.register_op(
            OpDecl::r#async("pg_connect", move |args| {
                let net = net.clone();
                let owned = owned.clone();
                let slots = slots.clone();
                let next_id = next_id.clone();
                let connecting = connecting.clone();
                let sides = sides.clone();
                let ticket = arg_u64(&args, 8);
                let mut nonce = [0u8; 18];
                let filled = entropy.fill(&mut nonce);
                let options = Options {
                    host: arg_str(&args, 0),
                    port: arg_u64(&args, 1) as u16,
                    sslmode: match arg_str(&args, 2).as_str() {
                        "disable" => SslMode::Disable,
                        "require" => SslMode::Require,
                        _ => SslMode::Prefer,
                    },
                    ca: args
                        .get(3)
                        .and_then(Value::as_bytes)
                        .unwrap_or(&[])
                        .to_vec(),
                    params: pairs(args.get(4)),
                    password: args.get(5).and_then(Value::as_str).map(str::to_string),
                    cache_limit: arg_u64(&args, 6) as usize,
                    binary_oids: numbers(args.get(7)),
                    nonce,
                };
                Box::pin(async move {
                    filled.map_err(|e| OpError::new(ExceptionClass::Error, e.to_string()))?;
                    let net = require(&net)?;
                    let mut cancel = CancelTarget {
                        host: options.host.clone(),
                        port: options.port,
                        sslmode: options.sslmode,
                        ca: options.ca.clone(),
                        process_id: 0,
                        secret_key: 0,
                    };
                    let opened = Arc::new(Mutex::new(None));
                    connecting.lock().unwrap().insert(ticket, opened.clone());
                    let result = Connection::open(net, options, &opened).await;
                    connecting.lock().unwrap().remove(&ticket);
                    match result {
                        Ok(connection) => {
                            let id = next_id.fetch_add(1, Ordering::Relaxed);
                            let status = connection.status;
                            let process = connection.process_id;
                            cancel.process_id = connection.process_id;
                            cancel.secret_key = connection.secret_key;
                            sides.lock().unwrap().insert(
                                id,
                                Side {
                                    socket: connection.socket(),
                                    cancel,
                                    subscribed: false,
                                },
                            );
                            let mut connection = Box::new(connection);
                            let aside = connection.take_aside();
                            slots.lock().unwrap().insert(id, Some(connection));
                            let mut out = vec![
                                ("id".to_string(), Value::Number(owned.own(id) as f64)),
                                ("processId".to_string(), Value::Number(process as f64)),
                                ("status".to_string(), status_value(status)),
                            ];
                            push_aside(&mut out, aside);
                            Ok(Value::Object(out))
                        }
                        Err(failure) => answer(failure, Aside::default()),
                    }
                })
            })
            .requires(Capability::Net),
        )?;
    }

    // Args: [ticket]. Gives up on a connect still in progress by closing the
    // socket it opened: a server that accepted and went silent would otherwise
    // hold the handshake — and the event loop — open for good.
    {
        let net = net.clone();
        let connecting = connecting.clone();
        engine.register_op(OpDecl::r#async("pg_abort_connect", move |args| {
            let net = net.clone();
            let ticket = arg_u64(&args, 0);
            let socket = connecting
                .lock()
                .unwrap()
                .get(&ticket)
                .and_then(|opened| *opened.lock().unwrap());
            Box::pin(async move {
                if let (Some(socket), Some(net)) = (socket, net) {
                    let _ = net.close(socket).await;
                }
                Ok(Value::Undefined)
            })
        }))?;
    }

    // Args: [id, sql, params (the Bind parameter section, bytes), maxBytes,
    // discard]. Resolves to the statement id, its columns when fresh, the first
    // batch, and whatever the server said on the side.
    {
        let owned = owned.clone();
        let slots = slots.clone();
        engine.register_op(OpDecl::r#async("pg_query", move |args| {
            let owned = owned.clone();
            let slots = slots.clone();
            let id = arg_u64(&args, 0);
            let sql = arg_str(&args, 1);
            let params = args
                .get(2)
                .and_then(Value::as_bytes)
                .unwrap_or(&[])
                .to_vec();
            let max_bytes = arg_u64(&args, 3) as usize;
            let discard = matches!(args.get(4), Some(Value::Bool(true)));
            Box::pin(async move {
                let id = owned.check(id)?;
                let mut connection = checkout(&slots, id)?;
                let result = connection.query(&sql, &params, max_bytes, discard).await;
                let status = connection.status;
                let aside = connection.take_aside();
                checkin(&slots, id, connection);
                Ok(match result {
                    Ok(started) => started_value(started, status, aside),
                    Err(failure) => failure_value(failure, aside),
                })
            })
        }))?;
    }

    // Args: [id, maxBytes]. The next batch of an open result set.
    {
        let owned = owned.clone();
        let slots = slots.clone();
        engine.register_op(OpDecl::r#async("pg_fetch", move |args| {
            let owned = owned.clone();
            let slots = slots.clone();
            let id = arg_u64(&args, 0);
            let max_bytes = arg_u64(&args, 1) as usize;
            Box::pin(async move {
                let id = owned.check(id)?;
                let mut connection = checkout(&slots, id)?;
                let result = connection.fetch(max_bytes).await;
                let status = connection.status;
                let aside = connection.take_aside();
                checkin(&slots, id, connection);
                Ok(match result {
                    Ok(batch) => {
                        let mut out = batch_fields(batch);
                        out.push(("status".to_string(), status_value(status)));
                        push_aside(&mut out, aside);
                        Value::Object(out)
                    }
                    Err(failure) => failure_value(failure, aside),
                })
            })
        }))?;
    }

    // Args: [id]. Reads and drops the rest of an open result set — a caller
    // that stopped early — so the connection can take the next exchange.
    {
        let owned = owned.clone();
        let slots = slots.clone();
        engine.register_op(OpDecl::r#async("pg_finish", move |args| {
            let owned = owned.clone();
            let slots = slots.clone();
            let id = arg_u64(&args, 0);
            Box::pin(async move {
                let id = owned.check(id)?;
                let mut connection = checkout(&slots, id)?;
                let result = connection.finish().await;
                let status = connection.status;
                let aside = connection.take_aside();
                checkin(&slots, id, connection);
                Ok(match result {
                    Ok(()) => {
                        let mut out = vec![("status".to_string(), status_value(status))];
                        push_aside(&mut out, aside);
                        Value::Object(out)
                    }
                    Err(failure) => failure_value(failure, aside),
                })
            })
        }))?;
    }

    // Args: [id, sql]. A script, through the simple query protocol: each
    // statement's command tag, rows discarded.
    {
        let owned = owned.clone();
        let slots = slots.clone();
        engine.register_op(OpDecl::r#async("pg_script", move |args| {
            let owned = owned.clone();
            let slots = slots.clone();
            let id = arg_u64(&args, 0);
            let sql = arg_str(&args, 1);
            Box::pin(async move {
                let id = owned.check(id)?;
                let mut connection = checkout(&slots, id)?;
                let result = connection.script(&sql).await;
                let status = connection.status;
                let aside = connection.take_aside();
                checkin(&slots, id, connection);
                Ok(match result {
                    Ok((tags, _evicted)) => {
                        let mut out = vec![
                            (
                                "tags".to_string(),
                                Value::Array(tags.into_iter().map(Value::String).collect()),
                            ),
                            ("status".to_string(), status_value(status)),
                        ];
                        push_aside(&mut out, aside);
                        Value::Object(out)
                    }
                    Err(failure) => failure_value(failure, aside),
                })
            })
        }))?;
    }

    // Args: [id]. Asks the server to cancel what the connection is running, on
    // a connection of its own. Needs no capability beyond the id: it reaches
    // the server this agent already connected to, the same way.
    {
        let owned = owned.clone();
        let sides = sides.clone();
        let net = net.clone();
        engine.register_op(OpDecl::r#async("pg_cancel", move |args| {
            let owned = owned.clone();
            let sides = sides.clone();
            let net = net.clone();
            let id = arg_u64(&args, 0);
            Box::pin(async move {
                let id = owned.check(id)?;
                let target = sides
                    .lock()
                    .unwrap()
                    .get(&id)
                    .map(|side| side.cancel.clone());
                let (Some(target), Some(net)) = (target, net) else {
                    return Ok(Value::Undefined);
                };
                if target.process_id == 0 {
                    return Ok(Value::Undefined);
                }
                match crate::postgres::cancel(net, target).await {
                    Ok(()) => Ok(Value::Undefined),
                    Err(failure) => answer(failure, Aside::default()),
                }
            })
        }))?;
    }

    // Args: [id]. The read loop of a subscribed connection: the next events.
    // Marks the connection subscribed synchronously, before any command can be
    // written underneath it.
    {
        let owned = owned.clone();
        let slots = slots.clone();
        let sides = sides.clone();
        engine.register_op(OpDecl::r#async("pg_listen_next", move |args| {
            let owned = owned.clone();
            let slots = slots.clone();
            let id = arg_u64(&args, 0);
            if let Ok(checked) = owned.check(id)
                && let Some(side) = sides.lock().unwrap().get_mut(&checked)
            {
                side.subscribed = true;
            }
            Box::pin(async move {
                let id = owned.check(id)?;
                let mut connection = checkout(&slots, id)?;
                let result = connection.listen_next().await;
                let status = connection.status;
                let aside = connection.take_aside();
                checkin(&slots, id, connection);
                Ok(match result {
                    Ok(events) => {
                        let mut out = vec![
                            (
                                "events".to_string(),
                                Value::Array(events.into_iter().map(event_value).collect()),
                            ),
                            ("status".to_string(), status_value(status)),
                        ];
                        push_aside(&mut out, aside);
                        Value::Object(out)
                    }
                    Err(failure) => failure_value(failure, aside),
                })
            })
        }))?;
    }

    // Args: [id, sql]. Writes a simple query underneath a subscribed
    // connection's read loop, which reads the reply. Refused on any other
    // connection: nothing would read the answer, and the next exchange would.
    {
        let owned = owned.clone();
        let sides = sides.clone();
        let net = net.clone();
        engine.register_op(OpDecl::r#async("pg_send", move |args| {
            let owned = owned.clone();
            let sides = sides.clone();
            let net = net.clone();
            let id = arg_u64(&args, 0);
            let sql = arg_str(&args, 1);
            Box::pin(async move {
                let id = owned.check(id)?;
                let socket = match sides.lock().unwrap().get(&id) {
                    Some(side) if side.subscribed => side.socket,
                    _ => {
                        return Err(OpError::new(
                            ExceptionClass::Error,
                            "only a subscribed connection takes commands underneath its read loop",
                        )
                        .with_code(ErrorCode::Io));
                    }
                };
                let net = require(&net)?;
                Ok(
                    match net
                        .write(socket, crate::postgres::wire::simple_query_bytes(&sql))
                        .await
                    {
                        Ok(()) => Value::Undefined,
                        Err(e) => failure_value(Failure::Lost(e.to_string()), Aside::default()),
                    },
                )
            })
        }))?;
    }

    // Args: [id]. Says goodbye and closes the socket; the id is given up.
    {
        let owned = owned.clone();
        let slots = slots.clone();
        engine.register_op(OpDecl::r#async("pg_close", move |args| {
            let owned = owned.clone();
            let slots = slots.clone();
            let id = arg_u64(&args, 0);
            let net = net.clone();
            let sides = sides.clone();
            Box::pin(async move {
                let id = owned.check_and_release(id)?;
                let side = sides.lock().unwrap().remove(&id);
                let slot = slots.lock().unwrap().remove(&id);
                match slot {
                    Some(Some(mut connection)) => connection.close().await,
                    // Checked out: a read loop is blocked on the socket. Closing
                    // the socket is what ends it; the loop's op then finds its
                    // slot gone and drops the connection.
                    Some(None) => {
                        if let (Some(side), Some(net)) = (side, net) {
                            let _ = net.close(side.socket).await;
                        }
                    }
                    None => {}
                }
                Ok(Value::Undefined)
            })
        }))?;
    }

    Ok(())
}

fn checkout(slots: &Slots, id: u64) -> std::result::Result<Box<Connection>, OpError> {
    let mut slots = slots.lock().unwrap();
    match slots.get_mut(&id) {
        Some(slot) => slot.take().ok_or_else(|| {
            OpError::new(
                ExceptionClass::Error,
                "this PostgreSQL connection is already running an operation",
            )
            .with_code(ErrorCode::Io)
        }),
        None => Err(
            OpError::new(ExceptionClass::Error, "the connection is closed")
                .with_code(ErrorCode::Io),
        ),
    }
}

fn checkin(slots: &Slots, id: u64, connection: Box<Connection>) {
    if let Some(slot) = slots.lock().unwrap().get_mut(&id) {
        *slot = Some(connection);
    }
}

fn started_value(started: Started, status: u8, aside: Aside) -> Value {
    let mut out = vec![(
        "statement".to_string(),
        Value::Number(started.statement as f64),
    )];
    if let Some((columns, formats)) = started.fresh {
        let (names, oids) = match &columns {
            Some(c) => (
                c.names.iter().cloned().map(Value::String).collect(),
                c.oids.iter().map(|&o| Value::Number(o as f64)).collect(),
            ),
            None => (Vec::new(), Vec::new()),
        };
        out.push(("hasRows".to_string(), Value::Bool(columns.is_some())));
        out.push(("names".to_string(), Value::Array(names)));
        out.push(("oids".to_string(), Value::Array(oids)));
        out.push((
            "formats".to_string(),
            Value::Array(
                formats
                    .into_iter()
                    .map(|f| Value::Number(f as f64))
                    .collect(),
            ),
        ));
    }
    if !started.evicted.is_empty() {
        out.push((
            "evicted".to_string(),
            Value::Array(
                started
                    .evicted
                    .into_iter()
                    .map(|id| Value::Number(id as f64))
                    .collect(),
            ),
        ));
    }
    out.extend(batch_fields(started.first));
    out.push(("status".to_string(), status_value(status)));
    push_aside(&mut out, aside);
    Value::Object(out)
}

/// An event as `["n", processId, channel, payload]`, `["r"]` or `["e", fields]`.
fn event_value(event: Event) -> Value {
    match event {
        Event::Notification {
            process_id,
            channel,
            payload,
        } => Value::Array(vec![
            Value::String("n".into()),
            Value::Number(process_id as f64),
            Value::String(channel),
            Value::String(payload),
        ]),
        Event::Ready => Value::Array(vec![Value::String("r".into())]),
        Event::Failed(fields) => {
            Value::Array(vec![Value::String("e".into()), fields_value(fields)])
        }
    }
}

fn batch_fields(batch: Batch) -> Vec<(String, Value)> {
    vec![
        ("bytes".to_string(), Value::Bytes(batch.bytes)),
        ("rows".to_string(), Value::Number(batch.rows as f64)),
        ("done".to_string(), Value::Bool(batch.done)),
        ("tag".to_string(), Value::String(batch.tag)),
    ]
}

/// An op's answer to a failure: a refusal throws, as `runtime:net`'s does; the
/// rest resolve to `{ error }` for the driver to turn into a `DbError`.
fn answer(failure: Failure, aside: Aside) -> std::result::Result<Value, OpError> {
    match failure {
        Failure::Refused(e) => {
            Err(OpError::new(e.exception_class(), e.exception_message()).with_code_opt(e.code()))
        }
        other => Ok(failure_value(other, aside)),
    }
}

fn failure_value(failure: Failure, aside: Aside) -> Value {
    let (kind, message, fields) = match failure {
        Failure::Server(fields) => {
            let message = fields
                .iter()
                .find(|(k, _)| *k == b'M')
                .map(|(_, m)| m.clone())
                .unwrap_or_default();
            ("server", message, Some(fields))
        }
        Failure::Lost(message) => ("lost", message, None),
        Failure::Auth(message) => ("auth", message, None),
        Failure::Busy(message) => ("busy", message, None),
        Failure::Unsupported(message) => ("unsupported", message, None),
        Failure::Refused(e) => ("lost", e.exception_message(), None),
    };
    let mut error = vec![
        ("kind".to_string(), Value::String(kind.to_string())),
        ("message".to_string(), Value::String(message)),
    ];
    if let Some(fields) = fields {
        error.push(("fields".to_string(), fields_value(fields)));
    }
    let mut out = vec![("error".to_string(), Value::Object(error))];
    push_aside(&mut out, aside);
    Value::Object(out)
}

/// A server message's fields, keyed by their one-character codes.
fn fields_value(fields: Vec<(u8, String)>) -> Value {
    Value::Object(
        fields
            .into_iter()
            .map(|(k, v)| ((k as char).to_string(), Value::String(v)))
            .collect(),
    )
}

/// Notices and parameter changes, only when there are any: most results carry
/// neither, and an empty array is still an allocation on the other side.
fn push_aside(out: &mut Vec<(String, Value)>, aside: Aside) {
    if !aside.notices.is_empty() {
        out.push((
            "notices".to_string(),
            Value::Array(aside.notices.into_iter().map(fields_value).collect()),
        ));
    }
    if !aside.parameters.is_empty() {
        out.push((
            "parameters".to_string(),
            Value::Array(
                aside
                    .parameters
                    .into_iter()
                    .map(|(k, v)| Value::Array(vec![Value::String(k), Value::String(v)]))
                    .collect(),
            ),
        ));
    }
}

fn status_value(status: u8) -> Value {
    Value::String((status as char).to_string())
}

fn pairs(value: Option<&Value>) -> Vec<(String, String)> {
    let Some(Value::Array(items)) = value else {
        return Vec::new();
    };
    items
        .chunks(2)
        .filter_map(|pair| match pair {
            [Value::String(k), Value::String(v)] => Some((k.clone(), v.clone())),
            _ => None,
        })
        .collect()
}

fn numbers(value: Option<&Value>) -> Vec<u32> {
    let Some(Value::Array(items)) = value else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(Value::as_number)
        .map(|n| n as u32)
        .collect()
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
