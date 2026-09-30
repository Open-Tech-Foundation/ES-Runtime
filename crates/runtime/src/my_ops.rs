//! Host ops behind `runtime:db`'s built-in MySQL driver (DECISIONS.md D147),
//! over the [`crate::mysql`] engine.
//!
//! The same arrangement as the PostgreSQL ops (`pg_ops`): connecting is gated
//! on `Net` and goes through the agent's own [`NetProvider`]; every op after it
//! carries an ownership check on the connection id (D50); a query is one op;
//! and failures resolve to `{ error }` for the driver to turn into a `DbError`,
//! except a refusal by the provider, which throws as `runtime:net`'s does.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use es_runtime_common::{Capability, ErrorCode, ExceptionClass, IntoException};
use es_runtime_engine::{Engine, OpDecl, OpError, Value};
use es_runtime_providers::{Entropy, NetProvider};

use crate::Result;
use crate::db_conns::Registry;
use crate::handles::Handles;
use crate::mysql::{Batch, Column, Connection, Failure, Options, Started};
use crate::postgres::SslMode;

/// What stays reachable about a connection while an op has it checked out: a
/// cancel runs *during* the statement it stops, and closing has to reach a
/// socket an op may be blocked reading.
struct Side {
    socket: u64,
    /// How this connection was opened — what `KILL QUERY`'s own connection
    /// needs to reach the same server as the same user.
    options: Options,
    connection_id: u32,
}

type Sides = Arc<Mutex<HashMap<u64, Side>>>;

pub(crate) fn install(
    engine: &mut dyn Engine,
    net: Option<Arc<dyn NetProvider>>,
    entropy: Arc<dyn Entropy>,
    inventory: &crate::handles::Inventory,
) -> Result<()> {
    let owned = inventory.track(Handles::new("MySQL connection"));
    let slots: Registry<Connection> = Registry::new();
    let sides: Sides = Arc::new(Mutex::new(HashMap::new()));

    // Args: [host, port, sslmode, ca (bytes), user, password, database (or
    // null), serverPublicKey (or null), allowPublicKeyRetrieval, cacheLimit,
    // statementTimeout (ms, or null), ticket].
    {
        let net = net.clone();
        let entropy = entropy.clone();
        let owned = owned.clone();
        let slots = slots.clone();
        let sides = sides.clone();
        engine.register_op(
            OpDecl::r#async("my_connect", move |args| {
                let net = net.clone();
                let entropy = entropy.clone();
                let owned = owned.clone();
                let slots = slots.clone();
                let sides = sides.clone();
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
                    user: arg_str(&args, 4),
                    password: arg_str(&args, 5),
                    database: args.get(6).and_then(Value::as_str).map(str::to_string),
                    server_public_key: args.get(7).and_then(Value::as_str).map(str::to_string),
                    allow_public_key_retrieval: matches!(args.get(8), Some(Value::Bool(true))),
                    cache_limit: arg_u64(&args, 9) as usize,
                    statement_timeout_ms: args.get(10).and_then(Value::as_number).map(|n| n as u64),
                };
                let ticket = arg_u64(&args, 11);
                Box::pin(async move {
                    let net = require(&net)?;
                    let opened = slots.connecting(ticket);
                    let result = Connection::open(net, entropy, &options, &opened).await;
                    slots.connected(ticket);
                    match result {
                        Ok(connection) => {
                            let version = connection.server_version.clone();
                            let connection_id = connection.connection_id;
                            let status = connection.status;
                            let socket = connection.socket();
                            let id = slots.insert(connection);
                            sides.lock().unwrap().insert(
                                id,
                                Side {
                                    socket,
                                    options,
                                    connection_id,
                                },
                            );
                            Ok(Value::Object(vec![
                                ("id".to_string(), Value::Number(owned.own(id) as f64)),
                                ("serverVersion".to_string(), Value::String(version)),
                                (
                                    "connectionId".to_string(),
                                    Value::Number(connection_id as f64),
                                ),
                                ("status".to_string(), Value::Number(status as f64)),
                            ]))
                        }
                        Err(failure) => answer(failure),
                    }
                })
            })
            .requires(Capability::Net),
        )?;
    }

    // Args: [ticket]. Gives up on a connect in progress by closing its socket.
    {
        let net = net.clone();
        let slots = slots.clone();
        engine.register_op(OpDecl::r#async("my_abort_connect", move |args| {
            let net = net.clone();
            let socket = slots.connecting_socket(arg_u64(&args, 0));
            Box::pin(async move {
                if let (Some(socket), Some(net)) = (socket, net) {
                    let _ = net.close(socket).await;
                }
                Ok(Value::Undefined)
            })
        }))?;
    }

    // Args: [id, sql, params (the COM_STMT_EXECUTE parameter block), count,
    // maxBytes, discard]. An OK's counts, or the first batch of rows with the
    // columns when they are not the ones this statement last reported.
    {
        let owned = owned.clone();
        let slots = slots.clone();
        engine.register_op(OpDecl::r#async("my_query", move |args| {
            let owned = owned.clone();
            let slots = slots.clone();
            let id = arg_u64(&args, 0);
            let sql = arg_str(&args, 1);
            let params = args
                .get(2)
                .and_then(Value::as_bytes)
                .unwrap_or(&[])
                .to_vec();
            let count = arg_u64(&args, 3) as usize;
            let max_bytes = arg_u64(&args, 4) as usize;
            let discard = matches!(args.get(5), Some(Value::Bool(true)));
            Box::pin(async move {
                let id = owned.check(id)?;
                let mut connection = slots.checkout(id)?;
                let result = connection
                    .query(&sql, &params, count, max_bytes, discard)
                    .await;
                let status = connection.status;
                let evicted = connection.take_evicted();
                slots.checkin(id, connection);
                match result {
                    Ok(started) => Ok(started_value(started, status, evicted)),
                    Err(failure) => answer(failure),
                }
            })
        }))?;
    }

    // Args: [id, maxBytes]. The next batch of an open result set.
    {
        let owned = owned.clone();
        let slots = slots.clone();
        engine.register_op(OpDecl::r#async("my_fetch", move |args| {
            let owned = owned.clone();
            let slots = slots.clone();
            let id = arg_u64(&args, 0);
            let max_bytes = arg_u64(&args, 1) as usize;
            Box::pin(async move {
                let id = owned.check(id)?;
                let mut connection = slots.checkout(id)?;
                let result = connection.fetch(max_bytes).await;
                let status = connection.status;
                slots.checkin(id, connection);
                match result {
                    Ok(batch) => {
                        let mut out = batch_fields(batch);
                        out.push(("status".to_string(), Value::Number(status as f64)));
                        Ok(Value::Object(out))
                    }
                    Err(failure) => answer(failure),
                }
            })
        }))?;
    }

    // Args: [id]. Reads and drops the rest of an open result set.
    {
        let owned = owned.clone();
        let slots = slots.clone();
        engine.register_op(OpDecl::r#async("my_finish", move |args| {
            let owned = owned.clone();
            let slots = slots.clone();
            let id = arg_u64(&args, 0);
            Box::pin(async move {
                let id = owned.check(id)?;
                let mut connection = slots.checkout(id)?;
                let result = connection.finish().await;
                let status = connection.status;
                slots.checkin(id, connection);
                match result {
                    Ok(()) => Ok(status_object(status)),
                    Err(failure) => answer(failure),
                }
            })
        }))?;
    }

    // Args: [id, sql]. A script through the text protocol: each statement's
    // affected rows and insert id.
    {
        let owned = owned.clone();
        let slots = slots.clone();
        engine.register_op(OpDecl::r#async("my_script", move |args| {
            let owned = owned.clone();
            let slots = slots.clone();
            let id = arg_u64(&args, 0);
            let sql = arg_str(&args, 1);
            Box::pin(async move {
                let id = owned.check(id)?;
                let mut connection = slots.checkout(id)?;
                let result = connection.script(&sql).await;
                let status = connection.status;
                slots.checkin(id, connection);
                match result {
                    Ok(results) => Ok(Value::Object(vec![
                        (
                            "results".to_string(),
                            Value::Array(
                                results
                                    .into_iter()
                                    .map(|(affected, last)| {
                                        Value::Array(vec![
                                            Value::Number(affected as f64),
                                            Value::Number(last as f64),
                                        ])
                                    })
                                    .collect(),
                            ),
                        ),
                        ("status".to_string(), Value::Number(status as f64)),
                    ])),
                    Err(failure) => answer(failure),
                }
            })
        }))?;
    }

    // Args: [id]. `COM_PING`.
    {
        let owned = owned.clone();
        let slots = slots.clone();
        engine.register_op(OpDecl::r#async("my_ping", move |args| {
            let owned = owned.clone();
            let slots = slots.clone();
            let id = arg_u64(&args, 0);
            Box::pin(async move {
                let id = owned.check(id)?;
                let mut connection = slots.checkout(id)?;
                let result = connection.ping().await;
                slots.checkin(id, connection);
                match result {
                    Ok(()) => Ok(Value::Undefined),
                    Err(failure) => answer(failure),
                }
            })
        }))?;
    }

    // Args: [id]. `KILL QUERY` on a second connection, as the same user. It
    // reaches the server this connection already reached, so it needs no
    // capability beyond the id.
    {
        let owned = owned.clone();
        let sides = sides.clone();
        let net = net.clone();
        let entropy = entropy.clone();
        engine.register_op(OpDecl::r#async("my_cancel", move |args| {
            let owned = owned.clone();
            let sides = sides.clone();
            let net = net.clone();
            let entropy = entropy.clone();
            let id = arg_u64(&args, 0);
            Box::pin(async move {
                let id = owned.check(id)?;
                let target = sides
                    .lock()
                    .unwrap()
                    .get(&id)
                    .map(|side| (side.options.clone(), side.connection_id));
                let (Some((options, connection_id)), Some(net)) = (target, net) else {
                    return Ok(Value::Undefined);
                };
                match crate::mysql::cancel(net, entropy, options, connection_id).await {
                    Ok(()) => Ok(Value::Undefined),
                    Err(failure) => answer(failure),
                }
            })
        }))?;
    }

    // Args: [id]. `COM_QUIT` and the socket closed; the id is given up. A
    // connection checked out by an op in flight has its socket closed under it.
    {
        let owned = owned.clone();
        let slots = slots.clone();
        let sides = sides.clone();
        engine.register_op(OpDecl::r#async("my_close", move |args| {
            let owned = owned.clone();
            let slots = slots.clone();
            let sides = sides.clone();
            let net = net.clone();
            let id = arg_u64(&args, 0);
            Box::pin(async move {
                let id = owned.check_and_release(id)?;
                let side = sides.lock().unwrap().remove(&id);
                match slots.remove(id) {
                    Some(Some(mut connection)) => connection.close().await,
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

fn started_value(started: Started, status: u16, evicted: Vec<u64>) -> Value {
    let mut value = started_fields(started, status);
    if !evicted.is_empty() {
        value.push((
            "evicted".to_string(),
            Value::Array(
                evicted
                    .into_iter()
                    .map(|k| Value::Number(k as f64))
                    .collect(),
            ),
        ));
    }
    Value::Object(value)
}

fn started_fields(started: Started, status: u16) -> Vec<(String, Value)> {
    match started {
        Started::Done {
            affected,
            last_insert_id,
        } => vec![
            ("affectedRows".to_string(), Value::Number(affected as f64)),
            (
                "lastInsertId".to_string(),
                Value::Number(last_insert_id as f64),
            ),
            ("status".to_string(), Value::Number(status as f64)),
        ],
        Started::Rows {
            statement,
            fresh,
            binary,
            first,
        } => {
            let mut out = vec![
                ("statement".to_string(), Value::Number(statement as f64)),
                ("binary".to_string(), Value::Bool(binary)),
            ];
            if let Some(columns) = fresh {
                out.push(("columns".to_string(), columns_value(&columns)));
            }
            out.extend(batch_fields(first));
            out.push(("status".to_string(), Value::Number(status as f64)));
            out
        }
    }
}

/// Columns as `[name, type, flags, charset]`, what a decoder is chosen by.
fn columns_value(columns: &[Column]) -> Value {
    Value::Array(
        columns
            .iter()
            .map(|c| {
                Value::Array(vec![
                    Value::String(c.name.clone()),
                    Value::Number(c.kind as f64),
                    Value::Number(c.flags as f64),
                    Value::Number(c.charset as f64),
                ])
            })
            .collect(),
    )
}

fn batch_fields(batch: Batch) -> Vec<(String, Value)> {
    vec![
        ("bytes".to_string(), Value::Bytes(batch.bytes)),
        ("rows".to_string(), Value::Number(batch.rows as f64)),
        ("done".to_string(), Value::Bool(batch.done)),
    ]
}

fn status_object(status: u16) -> Value {
    Value::Object(vec![("status".to_string(), Value::Number(status as f64))])
}

/// A refusal throws, as `runtime:net`'s does; the rest resolve to `{ error }`.
fn answer(failure: Failure) -> std::result::Result<Value, OpError> {
    let (kind, message, server) = match failure {
        Failure::Refused(e) => {
            return Err(
                OpError::new(e.exception_class(), e.exception_message()).with_code_opt(e.code())
            );
        }
        Failure::Server {
            code,
            sqlstate,
            message,
        } => ("server", message, Some((code, sqlstate))),
        Failure::Lost(message) => ("lost", message, None),
        Failure::Auth(message) => ("auth", message, None),
        Failure::Busy(message) => ("busy", message, None),
        Failure::Unsupported(message) => ("unsupported", message, None),
        Failure::Mismatch(message) => ("mismatch", message, None),
    };
    let mut error = vec![
        ("kind".to_string(), Value::String(kind.to_string())),
        ("message".to_string(), Value::String(message)),
    ];
    if let Some((code, sqlstate)) = server {
        error.push(("code".to_string(), Value::Number(code as f64)));
        error.push(("sqlstate".to_string(), Value::String(sqlstate)));
    }
    Ok(Value::Object(vec![(
        "error".to_string(),
        Value::Object(error),
    )]))
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
