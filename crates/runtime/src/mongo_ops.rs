//! Host ops behind `runtime:db`'s built-in MongoDB driver (DECISIONS D186),
//! over the [`crate::mongodb`] engine.
//!
//! The arrangement of the other drivers' ops: connecting is gated on `Net` and
//! goes through the agent's own [`NetProvider`]; every op after it carries an
//! ownership check on the client id (D50); a command is one op; and failures
//! resolve to `{ error }` for the driver to turn into a `DbError`, except a
//! refusal by the provider, which throws as `runtime:net`'s does. The id names
//! a pool, not a connection: the pool picks one inside each command.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use es_runtime_common::{Capability, ErrorCode, ExceptionClass, IntoException};
use es_runtime_engine::{Engine, OpDecl, OpError, Value};
use es_runtime_providers::{Entropy, NetProvider};

use crate::Result;
use crate::handles::Handles;
use crate::mongodb::{Connection, Failure, Options, Pool, Sequence};

type Pools = Arc<Mutex<HashMap<u64, Arc<Pool>>>>;
/// The socket a connect in flight has opened so far, by the driver's ticket.
type Connecting = Arc<Mutex<HashMap<u64, Arc<Mutex<Option<u64>>>>>>;

pub(crate) fn install(
    engine: &mut dyn Engine,
    net: Option<Arc<dyn NetProvider>>,
    entropy: Arc<dyn Entropy>,
    inventory: &crate::handles::Inventory,
) -> Result<()> {
    let owned = inventory.track(Handles::new("MongoDB client"));
    let pools: Pools = Arc::new(Mutex::new(HashMap::new()));
    let next_id = Arc::new(AtomicU64::new(1));
    let connecting: Connecting = Arc::new(Mutex::new(HashMap::new()));

    // Args: [host, port, tls, ca (bytes), user (or null), password,
    // passwordPrepared, authSource, mechanism (or null), appName (or null),
    // maxPoolSize, ticket]. Opens the first connection — which proves the
    // server and the credentials — and the pool around it.
    {
        let net = net.clone();
        let owned = owned.clone();
        let pools = pools.clone();
        let connecting = connecting.clone();
        engine.register_op(
            OpDecl::r#async("mongo_connect", move |args| {
                let net = net.clone();
                let entropy = entropy.clone();
                let owned = owned.clone();
                let pools = pools.clone();
                let next_id = next_id.clone();
                let connecting = connecting.clone();
                let options = Options {
                    host: arg_str(&args, 0),
                    port: u16::try_from(arg_u64(&args, 1)).unwrap_or(27017),
                    tls: matches!(args.get(2), Some(Value::Bool(true))),
                    ca: args
                        .get(3)
                        .and_then(Value::as_bytes)
                        .unwrap_or(&[])
                        .to_vec(),
                    user: args.get(4).and_then(Value::as_str).map(str::to_string),
                    password: arg_str(&args, 5),
                    password_prepared: arg_str(&args, 6),
                    auth_source: arg_str(&args, 7),
                    mechanism: args.get(8).and_then(Value::as_str).map(str::to_string),
                    app_name: args.get(9).and_then(Value::as_str).map(str::to_string),
                    max_pool: usize::try_from(arg_u64(&args, 10)).unwrap_or(1).max(1),
                };
                let ticket = arg_u64(&args, 11);
                Box::pin(async move {
                    let net = require(&net)?;
                    let opened = Arc::new(Mutex::new(None));
                    connecting.lock().unwrap().insert(ticket, opened.clone());
                    let result =
                        Connection::open(net.clone(), entropy.as_ref(), &options, &opened).await;
                    connecting.lock().unwrap().remove(&ticket);
                    match result {
                        Ok((connection, server)) => {
                            let id = next_id.fetch_add(1, Ordering::Relaxed);
                            let pool = Pool::new(net, entropy, options, connection);
                            pools.lock().unwrap().insert(id, Arc::new(pool));
                            Ok(Value::Object(vec![
                                ("id".to_string(), Value::Number(owned.own(id) as f64)),
                                ("hello".to_string(), Value::Bytes(server.hello)),
                                (
                                    "maxMessageSize".to_string(),
                                    Value::Number(server.max_message as f64),
                                ),
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
        let connecting = connecting.clone();
        engine.register_op(OpDecl::r#async("mongo_abort_connect", move |args| {
            let net = net.clone();
            let socket = connecting
                .lock()
                .unwrap()
                .get(&arg_u64(&args, 0))
                .and_then(|opened| *opened.lock().unwrap());
            Box::pin(async move {
                if let (Some(socket), Some(net)) = (socket, net) {
                    let _ = net.close(socket).await;
                }
                Ok(Value::Undefined)
            })
        }))?;
    }

    // Args: [id, body (a BSON command), ticket, sequence name (or null),
    // sequence documents (an array of BSON documents, or null)]. The reply
    // document's bytes, whatever its `ok`.
    {
        let owned = owned.clone();
        let pools = pools.clone();
        engine.register_op(OpDecl::r#async("mongo_command", move |args| {
            let owned = owned.clone();
            let pools = pools.clone();
            let id = arg_u64(&args, 0);
            let body = args
                .get(1)
                .and_then(Value::as_bytes)
                .unwrap_or(&[])
                .to_vec();
            let ticket = arg_u64(&args, 2);
            let sequence = match (args.get(3).and_then(Value::as_str), args.get(4)) {
                (Some(name), Some(Value::Array(documents))) => Some(Sequence {
                    name: name.to_string(),
                    documents: documents
                        .iter()
                        .filter_map(|d| d.as_bytes().map(<[u8]>::to_vec))
                        .collect(),
                }),
                _ => None,
            };
            Box::pin(async move {
                let pool = lookup(&owned, &pools, id)?;
                match pool.command(ticket, &body, sequence.as_ref()).await {
                    Ok(reply) => Ok(Value::Bytes(reply)),
                    Err(failure) => answer(failure),
                }
            })
        }))?;
    }

    // Args: [id, ticket]. Stops that command wherever it is (see `Pool::cancel`).
    {
        let owned = owned.clone();
        let pools = pools.clone();
        let net = net.clone();
        engine.register_op(OpDecl::r#async("mongo_cancel", move |args| {
            let owned = owned.clone();
            let pools = pools.clone();
            let net = net.clone();
            let id = arg_u64(&args, 0);
            let ticket = arg_u64(&args, 1);
            Box::pin(async move {
                let pool = lookup(&owned, &pools, id)?;
                if let (Some(socket), Some(net)) = (pool.cancel(ticket), net) {
                    let _ = net.close(socket).await;
                }
                Ok(Value::Undefined)
            })
        }))?;
    }

    // Args: [id]. Closes the pool and gives the id up.
    {
        engine.register_op(OpDecl::r#async("mongo_close", move |args| {
            let owned = owned.clone();
            let pools = pools.clone();
            let id = arg_u64(&args, 0);
            Box::pin(async move {
                let id = owned.check_and_release(id)?;
                let pool = pools.lock().unwrap().remove(&id);
                if let Some(pool) = pool {
                    pool.close().await;
                }
                Ok(Value::Undefined)
            })
        }))?;
    }

    Ok(())
}

fn lookup(owned: &Handles, pools: &Pools, id: u64) -> std::result::Result<Arc<Pool>, OpError> {
    let id = owned.check(id)?;
    pools.lock().unwrap().get(&id).cloned().ok_or_else(|| {
        OpError::new(ExceptionClass::Error, "the client is closed").with_code(ErrorCode::Io)
    })
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
            code_name,
            message,
        } => ("server", message, Some((code, code_name))),
        Failure::Lost(message) => ("lost", message, None),
        Failure::Auth(message) => ("auth", message, None),
        Failure::Unsupported(message) => ("unsupported", message, None),
        Failure::Closed => ("closed", "the client is closed".to_string(), None),
    };
    let mut error = vec![
        ("kind".to_string(), Value::String(kind.to_string())),
        ("message".to_string(), Value::String(message)),
    ];
    if let Some((code, code_name)) = server {
        error.push(("code".to_string(), Value::Number(f64::from(code))));
        error.push(("codeName".to_string(), Value::String(code_name)));
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

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "ids and sizes the driver chose"
)]
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
