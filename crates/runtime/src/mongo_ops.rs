//! Host ops behind `runtime:db`'s built-in MongoDB driver (DECISIONS D186),
//! over the [`crate::mongodb`] engine.
//!
//! The arrangement of the other drivers' ops: connecting is gated on `Net` and
//! goes through the agent's own [`NetProvider`]; every op after it carries an
//! ownership check on the client id (D50); a command is one op; and failures
//! resolve to `{ error }` for the driver to turn into a `DbError`, except a
//! refusal by the provider, which throws as `runtime:net`'s does. The id names
//! a topology: the servers it knows, a pool for each, and their monitors.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use es_runtime_common::{Capability, ErrorCode, ExceptionClass, IntoException};
use es_runtime_engine::{Engine, OpDecl, OpError, Value};
use es_runtime_providers::{Entropy, NetProvider};

use crate::Result;
use crate::handles::Handles;
use crate::mongodb::select::{Mode, Operation, ReadPreference};
use crate::mongodb::{Failure, Options, Selector, Sequence, Topology};

type Topologies = Arc<Mutex<HashMap<u64, Arc<Topology>>>>;

pub(crate) fn install(
    engine: &mut dyn Engine,
    net: Option<Arc<dyn NetProvider>>,
    entropy: Arc<dyn Entropy>,
    inventory: &crate::handles::Inventory,
) -> Result<()> {
    let owned = inventory.track(Handles::new("MongoDB client"));
    let topologies: Topologies = Arc::new(Mutex::new(HashMap::new()));
    let next_id = Arc::new(AtomicU64::new(1));
    // A topology still making its first checks, by the connect's ticket.
    let connecting: Topologies = Arc::new(Mutex::new(HashMap::new()));

    // Args: [SRV host, service name, srvMaxHosts]. `mongodb+srv://`'s lookups
    // (D188): resolves `{ hosts, txt }`, `txt` as [name, value] pairs, or
    // `{ error }` when the records cannot be used. A lookup the allowlist
    // refuses throws, as a connect would.
    {
        let net = net.clone();
        let entropy = entropy.clone();
        engine.register_op(
            OpDecl::r#async("mongo_seedlist", move |args| {
                let net = net.clone();
                let entropy = entropy.clone();
                let host = arg_str(&args, 0);
                let service = arg_str(&args, 1);
                let max_hosts = usize::try_from(arg_u64(&args, 2)).unwrap_or(usize::MAX);
                Box::pin(async move {
                    let net = net.ok_or_else(|| {
                        OpError::new(
                            ExceptionClass::Error,
                            "networking is unavailable (no NetProvider configured)",
                        )
                    })?;
                    let resolve = |q| net.resolve_dns(q);
                    let mut pick = |n: usize| {
                        let mut bytes = [0u8; 8];
                        // A failed source picks the first: the choice spreads
                        // load, and nothing depends on it being secret.
                        let _ = entropy.fill(&mut bytes);
                        usize::try_from(u64::from_ne_bytes(bytes) % n as u64).unwrap_or(0)
                    };
                    let found = crate::mongodb::seedlist::discover(
                        &resolve, &host, &service, max_hosts, &mut pick,
                    )
                    .await;
                    Ok(match found {
                        Ok(list) => Value::Object(vec![
                            (
                                "hosts".into(),
                                Value::Array(list.hosts.into_iter().map(Value::String).collect()),
                            ),
                            (
                                "txt".into(),
                                Value::Array(
                                    list.txt
                                        .into_iter()
                                        .map(|(k, v)| {
                                            Value::Array(vec![Value::String(k), Value::String(v)])
                                        })
                                        .collect(),
                                ),
                            ),
                        ]),
                        Err(crate::mongodb::seedlist::Refused::Denied(e)) => {
                            return Err(OpError::new(e.exception_class(), e.exception_message())
                                .with_code_opt(e.code()));
                        }
                        Err(refused) => Value::Object(vec![(
                            "error".into(),
                            Value::String(refused.to_string()),
                        )]),
                    })
                })
            })
            .requires(Capability::Net),
        )?;
    }

    // Args: [seeds (array of host:port), directConnection, replicaSet (or
    // null), tls, ca (bytes), user (or null), password, passwordPrepared,
    // authSource, mechanism (or null), appName (or null), maxPoolSize,
    // heartbeatFrequencyMS, localThresholdMS, ticket]. Checks every seed, and
    // every server they name, before it answers with the topology.
    {
        let net = net.clone();
        let owned = owned.clone();
        let topologies = topologies.clone();
        let connecting = connecting.clone();
        engine.register_op(
            OpDecl::r#async("mongo_connect", move |args| {
                let net = net.clone();
                let entropy = entropy.clone();
                let owned = owned.clone();
                let topologies = topologies.clone();
                let next_id = next_id.clone();
                let connecting = connecting.clone();
                let seeds: Vec<String> = match args.first() {
                    Some(Value::Array(list)) => list
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect(),
                    _ => Vec::new(),
                };
                let direct = matches!(args.get(1), Some(Value::Bool(true)));
                let set_name = args.get(2).and_then(Value::as_str).map(str::to_string);
                let options = Options {
                    host: String::new(),
                    port: 27017,
                    tls: matches!(args.get(3), Some(Value::Bool(true))),
                    ca: args
                        .get(4)
                        .and_then(Value::as_bytes)
                        .unwrap_or(&[])
                        .to_vec(),
                    user: args.get(5).and_then(Value::as_str).map(str::to_string),
                    password: arg_str(&args, 6),
                    password_prepared: arg_str(&args, 7),
                    auth_source: arg_str(&args, 8),
                    mechanism: args.get(9).and_then(Value::as_str).map(str::to_string),
                    app_name: args.get(10).and_then(Value::as_str).map(str::to_string),
                    max_pool: usize::try_from(arg_u64(&args, 11)).unwrap_or(1).max(1),
                };
                let heartbeat = i64::try_from(arg_u64(&args, 12)).unwrap_or(10_000);
                #[expect(clippy::cast_precision_loss, reason = "milliseconds")]
                let local_threshold = arg_u64(&args, 13) as f64;
                let ticket = arg_u64(&args, 14);
                Box::pin(async move {
                    let net = require(&net)?;
                    let topology = Arc::new(Topology::new(
                        net,
                        entropy,
                        options,
                        &seeds,
                        direct,
                        set_name,
                        heartbeat,
                        local_threshold,
                    ));
                    connecting.lock().unwrap().insert(ticket, topology.clone());
                    let checked = topology.check_all().await;
                    connecting.lock().unwrap().remove(&ticket);
                    if let Err(failure) = checked {
                        topology.close().await;
                        return answer(failure);
                    }
                    let id = next_id.fetch_add(1, Ordering::Relaxed);
                    let summary = topology.summary();
                    topologies.lock().unwrap().insert(id, topology);
                    Ok(Value::Object(vec![
                        ("id".to_string(), Value::Number(owned.own(id) as f64)),
                        ("topology".to_string(), summary),
                    ]))
                })
            })
            .requires(Capability::Net),
        )?;
    }

    // Args: [ticket]. Gives up on a connect in progress by closing its checks'
    // sockets.
    {
        let net = net.clone();
        engine.register_op(OpDecl::r#async("mongo_abort_connect", move |args| {
            let net = net.clone();
            let topology = connecting.lock().unwrap().get(&arg_u64(&args, 0)).cloned();
            Box::pin(async move {
                if let (Some(topology), Some(net)) = (topology, net) {
                    for socket in topology.checking_sockets() {
                        let _ = net.close(socket).await;
                    }
                }
                Ok(Value::Undefined)
            })
        }))?;
    }

    // Args: [id]. Checks every server now, and answers with the topology.
    {
        let owned = owned.clone();
        let topologies = topologies.clone();
        engine.register_op(OpDecl::r#async("mongo_check", move |args| {
            let owned = owned.clone();
            let topologies = topologies.clone();
            let id = arg_u64(&args, 0);
            Box::pin(async move {
                let topology = lookup(&owned, &topologies, id)?;
                match topology.check_all().await {
                    // A server outside the allowlist stays Unknown with that error.
                    Ok(()) | Err(Failure::Refused(_)) => Ok(topology.summary()),
                    Err(failure) => answer(failure),
                }
            })
        }))?;
    }

    // Args: [id, body (a BSON command), ticket, sequence name (or null),
    // sequence documents (or null), read preference mode, tag sets (or null),
    // maxStalenessSeconds (or null), "read" or "write", server (or null)].
    // `{ reply, server, recheck }`, or `{ error }`.
    {
        let owned = owned.clone();
        let topologies = topologies.clone();
        engine.register_op(OpDecl::r#async("mongo_command", move |args| {
            let owned = owned.clone();
            let topologies = topologies.clone();
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
            let selector = Selector {
                preference: ReadPreference {
                    mode: args
                        .get(5)
                        .and_then(Value::as_str)
                        .and_then(Mode::parse)
                        .unwrap_or(Mode::Primary),
                    tag_sets: tag_sets(args.get(6)),
                    #[expect(
                        clippy::cast_possible_truncation,
                        reason = "seconds the driver checked"
                    )]
                    max_staleness_seconds: args.get(7).and_then(Value::as_number).map(|n| n as i64),
                },
                operation: if args.get(8).and_then(Value::as_str) == Some("write") {
                    Operation::Write
                } else {
                    Operation::Read
                },
                server: args.get(9).and_then(Value::as_str).map(str::to_string),
            };
            Box::pin(async move {
                let topology = lookup(&owned, &topologies, id)?;
                match topology
                    .run(ticket, body, sequence.as_ref(), &selector)
                    .await
                {
                    Ok(ran) => Ok(Value::Object(vec![
                        ("reply".to_string(), Value::Bytes(ran.reply)),
                        ("server".to_string(), Value::String(ran.server)),
                        ("recheck".to_string(), Value::Bool(ran.recheck)),
                    ])),
                    Err(failure) => answer(failure),
                }
            })
        }))?;
    }

    // Args: [id, ticket]. Stops that command wherever it is.
    {
        let owned = owned.clone();
        let topologies = topologies.clone();
        let net = net.clone();
        engine.register_op(OpDecl::r#async("mongo_cancel", move |args| {
            let owned = owned.clone();
            let topologies = topologies.clone();
            let net = net.clone();
            let id = arg_u64(&args, 0);
            let ticket = arg_u64(&args, 1);
            Box::pin(async move {
                let topology = lookup(&owned, &topologies, id)?;
                if let (Some(socket), Some(net)) = (topology.cancel(ticket), net) {
                    let _ = net.close(socket).await;
                }
                Ok(Value::Undefined)
            })
        }))?;
    }

    // Args: [id]. Closes every pool and monitor and gives the id up.
    {
        engine.register_op(OpDecl::r#async("mongo_close", move |args| {
            let owned = owned.clone();
            let topologies = topologies.clone();
            let id = arg_u64(&args, 0);
            Box::pin(async move {
                let id = owned.check_and_release(id)?;
                let topology = topologies.lock().unwrap().remove(&id);
                if let Some(topology) = topology {
                    topology.close().await;
                }
                Ok(Value::Undefined)
            })
        }))?;
    }

    Ok(())
}

/// Tag sets as `[{ dc: "ny" }, {}]`.
fn tag_sets(value: Option<&Value>) -> Vec<Vec<(String, String)>> {
    let Some(Value::Array(sets)) = value else {
        return Vec::new();
    };
    sets.iter()
        .map(|set| match set {
            Value::Object(fields) => fields
                .iter()
                .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string())))
                .collect(),
            _ => Vec::new(),
        })
        .collect()
}

fn lookup(
    owned: &Handles,
    topologies: &Topologies,
    id: u64,
) -> std::result::Result<Arc<Topology>, OpError> {
    let id = owned.check(id)?;
    topologies.lock().unwrap().get(&id).cloned().ok_or_else(|| {
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
        Failure::Selection(message) => ("selection", message, None),
        Failure::Closed => ("closed", "the client is closed".to_string(), None),
        Failure::Cancelled => ("cancelled", "the operation was cancelled".to_string(), None),
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
