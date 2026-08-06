//! Host ops behind the `Worker` global and the worker's own global scope.
//!
//! Two halves of one seam. The **parent** half (`worker_spawn`, `worker_post`,
//! `worker_recv`, `worker_terminate`) is routed through the [`WorkerHost`]
//! provider; the **worker** half (`worker_scope_info`, `worker_self_post`,
//! `worker_self_recv`, `worker_self_close`) through [`WorkerScope`], which is
//! installed only on a runtime that *is* a worker.
//!
//! Only `worker_spawn` is capability-checked, on [`Capability::Worker`]. The
//! rest operate by an id a checked spawn produced, so they need no capability
//! of their own — the same reasoning as `net_read` and `ws_send`. The worker
//! half needs none at all: possessing a `WorkerScope` *is* the authority, and
//! the only agent given one is the worker it belongs to.
//!
//! Messages cross as bytes in the engine's structured-clone format. The object
//! graph is flattened on the JS side, before the op, because an op argument is
//! a marshaled [`Value`] and could not carry one — see `engine::serialize`.

use std::sync::Arc;

use es_runtime_common::{Capability, CapabilitySet, ErrorCode, ExceptionClass, IntoException};
use es_runtime_engine::{Engine, OpDecl, OpError, Value};
use es_runtime_providers::{ProviderError, WorkerHost, WorkerIncoming, WorkerScope, WorkerSpec};

use crate::Result;

pub(crate) fn install(
    engine: &mut dyn Engine,
    workers: Option<Arc<dyn WorkerHost>>,
    scope: Option<Arc<dyn WorkerScope>>,
    capabilities: std::rc::Rc<std::cell::Cell<CapabilitySet>>,
    loader: crate::module_ops::LoaderSlot,
    entry: crate::EntrySlot,
) -> Result<()> {
    install_entry_reader(engine, loader)?;
    install_base(engine, entry)?;
    install_parent(engine, workers, capabilities)?;
    install_scope(engine, scope)
}

/// `worker_read_entry(url) -> { specifier, source }`: resolve a worker's entry
/// module and read it, through the loader, on the **parent's** authority.
///
/// Gated on [`Capability::FileSystem`], the same grant an `import` needs, and
/// running the loader means it obeys the same root jail and import policy — so
/// a worker URL can never reach somewhere an import could not. Doing it here
/// rather than on the worker's thread is what lets a spawn that cannot find its
/// script fail visibly, instead of inside a thread that already exists.
fn install_entry_reader(
    engine: &mut dyn Engine,
    loader: crate::module_ops::LoaderSlot,
) -> Result<()> {
    engine.register_op(
        OpDecl::r#async("worker_read_entry", move |args| {
            let url = arg_str(&args, 0);
            let loader = loader.borrow().clone();
            Box::pin(async move {
                let Some(loader) = loader else {
                    return Err(OpError::new(
                        ExceptionClass::TypeError,
                        format!(
                            "cannot start a worker from {url:?}: module loading is not \
                             permitted in this runtime"
                        ),
                    ));
                };
                // Resolved against itself: a worker URL is absolute by the time
                // it gets here, because the prelude resolves it with the realm's
                // own `URL` first.
                let specifier = loader.resolve(&url, &url).await.map_err(map_err)?;
                let source = loader.load(&specifier).await.map_err(map_err)?;
                let source = match source {
                    es_runtime_providers::ModuleSource::Text(text) => text,
                    // A worker entry is a module of source text. Wasm and JSON
                    // are modules too, but neither has a top level that can run
                    // a worker, so pointing one at them is a mistake worth
                    // naming rather than a graph to build.
                    _ => {
                        return Err(OpError::new(
                            ExceptionClass::TypeError,
                            format!("a worker entry must be a JavaScript module: {specifier}"),
                        ));
                    }
                };
                Ok(Value::Object(vec![
                    ("specifier".to_string(), Value::String(specifier)),
                    ("source".to_string(), Value::String(source)),
                ]))
            })
        })
        .requires(Capability::FileSystem),
    )?;
    Ok(())
}

/// `worker_base() -> string`: the entry module's URL, which a relative
/// `new Worker("./w.js")` resolves against — the base a browser would take from
/// the document.
///
/// Ungated: it reports a URL the guest already has, since `import.meta.url` on
/// the entry module is the same string.
///
/// A relative specifier resolving against the *entry* rather than the calling
/// module is the one place this differs from a browser. Reaching the caller's
/// URL would mean walking a stack trace, which is guesswork; the precise form
/// is `new Worker(new URL("./w.js", import.meta.url))`, which is also what
/// Vite, webpack and Deno all recommend writing.
fn install_base(engine: &mut dyn Engine, entry: crate::EntrySlot) -> Result<()> {
    engine.register_op(OpDecl::sync("worker_base", move |_args| {
        Ok(Value::String(entry.borrow().clone().unwrap_or_default()))
    }))?;
    Ok(())
}

// ---- the parent's half ------------------------------------------------------

fn install_parent(
    engine: &mut dyn Engine,
    workers: Option<Arc<dyn WorkerHost>>,
    capabilities: std::rc::Rc<std::cell::Cell<CapabilitySet>>,
) -> Result<()> {
    let host = workers.clone();
    engine.register_op(
        OpDecl::r#async("worker_spawn", move |args| {
            let host = host.clone();
            let specifier = arg_str(&args, 0);
            let source = arg_str(&args, 1);
            let name = arg_str(&args, 2);
            let requested = arg_str_vec(&args, 3);
            // `null` — the ordinary case — means "read the host environment,
            // if you were granted it". A list of pairs is one the parent built
            // out of values it already held.
            let env = arg_pairs(&args, 4);
            // Read *now*, on the calling thread, rather than inside the future:
            // this is the spawning agent's own grant, and it is the ceiling on
            // what the child can be given.
            let parent = capabilities.get();
            Box::pin(async move {
                let spec = WorkerSpec {
                    specifier,
                    source,
                    name,
                    env,
                    capabilities: child_capabilities(&requested, parent),
                    // Module loading, and nothing else. The child's static graph
                    // is loaded with the parent's authority to read it (which
                    // the parent had, and used, to read the entry source above),
                    // then narrowed before a line of the worker runs.
                    load_capabilities: load_capabilities(parent),
                    limits: worker_limits(),
                };
                let id = require(&host)?.spawn(spec).await.map_err(map_err)?;
                Ok(Value::Number(id as f64))
            })
        })
        .requires(Capability::Worker),
    )?;

    let host = workers.clone();
    engine.register_op(OpDecl::r#async("worker_post", move |args| {
        let host = host.clone();
        let id = arg_u64(&args, 0);
        let message = arg_bytes(&args, 1);
        Box::pin(async move {
            require(&host)?.post(id, message).await.map_err(map_err)?;
            Ok(Value::Undefined)
        })
    }))?;

    let host = workers.clone();
    engine.register_op(OpDecl::r#async("worker_recv", move |args| {
        let host = host.clone();
        let id = arg_u64(&args, 0);
        Box::pin(async move {
            let event = require(&host)?.recv(id).await.map_err(map_err)?;
            Ok(match event {
                // `null` ends the prelude's pump: the worker is gone and its
                // queue is drained.
                None => Value::Null,
                Some(WorkerIncoming::Message(bytes)) => envelope("message", Value::Bytes(bytes)),
                Some(WorkerIncoming::Error { message }) => {
                    envelope("error", Value::String(message))
                }
                Some(WorkerIncoming::Closed) => envelope("close", Value::Undefined),
            })
        })
    }))?;

    let host = workers;
    engine.register_op(OpDecl::r#async("worker_terminate", move |args| {
        let host = host.clone();
        let id = arg_u64(&args, 0);
        Box::pin(async move {
            // Terminating a worker that is already gone is a no-op, not an
            // error — `terminate()` is idempotent by specification.
            if let Some(host) = host.as_ref() {
                host.terminate(id).await.map_err(map_err)?;
            }
            Ok(Value::Undefined)
        })
    }))?;
    Ok(())
}

/// What a worker is granted: the names it asked for, intersected with what the
/// spawning agent itself holds.
///
/// Deny-by-default, and narrowing only. A worker starts from nothing, is given
/// exactly what `new Worker(url, { permissions: [...] })` names, and can never
/// be handed something its parent does not hold — so no chain of spawns widens
/// what the original grant allowed. (Deno differs here: it clones the parent's
/// permissions unmodified.)
fn child_capabilities(requested: &[String], parent: CapabilitySet) -> CapabilitySet {
    let mut child = CapabilitySet::none();
    for name in requested {
        if let Some(capability) = Capability::from_flag_name(name)
            && parent.contains(capability)
        {
            child.grant(capability);
        }
    }
    // The ungated capabilities — clock, entropy, timers, task spawn — back
    // `Date.now()`, `crypto` and `setTimeout`. No op gates them, so withholding
    // them would deny nothing while making a worker look more confined than it
    // is; a worker without `setTimeout` would also be a strange thing to hand
    // someone. They ride along, still bounded by the parent's set.
    for capability in [
        Capability::Clock,
        Capability::Entropy,
        Capability::Timers,
        Capability::TaskSpawn,
    ] {
        if parent.contains(capability) {
            child.grant(capability);
        }
    }
    child
}

/// The set the worker's static import graph is loaded under: the parent's
/// module-loading authority and nothing else.
fn load_capabilities(parent: CapabilitySet) -> CapabilitySet {
    let mut set = CapabilitySet::none();
    for capability in [Capability::FileSystem, Capability::FileRead] {
        if parent.contains(capability) {
            set.grant(capability);
        }
    }
    set
}

/// A worker agent's isolate limits.
fn worker_limits() -> es_runtime_common::Limits {
    es_runtime_common::Limits::default()
        // A worker owns its thread, so parking it in `Atomics.wait` is what the
        // call is for — unlike on the agent driving the loop, where it is a
        // hang. This is the ECMAScript agent record's [[CanBlock]], and the
        // same split HTML makes between window and worker agents.
        .with_can_block(true)
}

// ---- the worker's half ------------------------------------------------------

fn install_scope(engine: &mut dyn Engine, scope: Option<Arc<dyn WorkerScope>>) -> Result<()> {
    // Sync, and the prelude's test for "am I a worker": `null` on the agent
    // driving the process, an object inside a worker. There is deliberately no
    // `isMainThread` — HTML distinguishes the two by the shape of the global
    // scope, and so do Deno and Bun.
    let s = scope.clone();
    engine.register_op(OpDecl::sync("worker_scope_info", move |_args| {
        Ok(match s.as_ref() {
            None => Value::Null,
            Some(scope) => Value::Object(vec![
                ("name".to_string(), Value::String(scope.name())),
                ("url".to_string(), Value::String(scope.url())),
            ]),
        })
    }))?;

    let s = scope.clone();
    engine.register_op(OpDecl::r#async("worker_self_post", move |args| {
        let s = s.clone();
        let message = arg_bytes(&args, 0);
        Box::pin(async move {
            require_scope(&s)?.post(message).await.map_err(map_err)?;
            Ok(Value::Undefined)
        })
    }))?;

    let s = scope.clone();
    engine.register_op(OpDecl::r#async("worker_self_recv", move |args| {
        let s = s.clone();
        let _ = args;
        Box::pin(async move {
            let message = require_scope(&s)?.recv().await.map_err(map_err)?;
            Ok(match message {
                Some(bytes) => Value::Bytes(bytes),
                None => Value::Null,
            })
        })
    }))?;

    let s = scope.clone();
    engine.register_op(OpDecl::sync("worker_self_close", move |_args| {
        if let Some(scope) = s.as_ref() {
            scope.close();
        }
        Ok(Value::Undefined)
    }))?;

    // `worker_self_fail(message)`: a failure this worker's own listeners did not
    // claim. Reported to the parent and fatal to this agent — see
    // `WorkerScope::report_error`.
    //
    // The prelude calls it for the one route the host cannot see: an exception
    // thrown by an event listener, which `dispatchEvent` catches and hands to
    // `reportError` rather than letting escape. A throw inside `onmessage` is
    // exactly that, and it is the failure a worker is most likely to have.
    //
    // Sync, because it must take effect within the tick that produced it: the
    // drive loop reads the flag it sets as soon as this tick ends.
    let s = scope;
    engine.register_op(OpDecl::sync("worker_self_fail", move |args| {
        let message = arg_str(&args, 0);
        // Not an error when there is no scope: on the agent driving the process
        // the prelude never installs the hook that calls this, and an embedder
        // without workers has no parent for a report to reach.
        if let Some(scope) = s.as_ref() {
            scope.report_error(message);
        }
        Ok(Value::Undefined)
    }))?;
    Ok(())
}

// ---- MessagePort ------------------------------------------------------------

/// Ops behind `MessagePort`, routed through the [`PortHub`].
///
/// Ungated, like `BroadcastChannel`: a port carries no authority, and an agent
/// only ever receives one from something that already held it. With no hub the
/// prelude keeps its agent-local ports, which is all an embedder without
/// workers could use anyway.
pub(crate) fn install_ports(
    engine: &mut dyn Engine,
    hub: Option<Arc<dyn es_runtime_providers::PortHub>>,
) -> Result<()> {
    let present = hub.is_some();
    engine.register_op(OpDecl::sync("port_available", move |_args| {
        Ok(Value::Bool(present))
    }))?;

    // Sync: `new MessageChannel()` is synchronous in the specification, and
    // allocating two queues has nothing to await.
    let h = hub.clone();
    engine.register_op(OpDecl::sync("port_create", move |_args| {
        let (a, b) = require_ports(&h)?.create().map_err(map_err)?;
        Ok(Value::Array(vec![
            Value::Number(a as f64),
            Value::Number(b as f64),
        ]))
    }))?;

    // Sync too, and for a stronger reason: `postMessage` must be ordered with
    // respect to later ones, which an async op's scheduling would not
    // guarantee.
    let h = hub.clone();
    engine.register_op(OpDecl::sync("port_post", move |args| {
        let id = arg_u64(&args, 0);
        let message = arg_bytes(&args, 1);
        require_ports(&h)?.post(id, message).map_err(map_err)?;
        Ok(Value::Undefined)
    }))?;

    let h = hub.clone();
    engine.register_op(
        OpDecl::r#async("port_recv", move |args| {
            let h = h.clone();
            let id = arg_u64(&args, 0);
            Box::pin(async move {
                Ok(match require_ports(&h)?.recv(id).await.map_err(map_err)? {
                    Some(bytes) => Value::Bytes(bytes),
                    None => Value::Null,
                })
            })
        })
        // Unref'd: an open port must not, by itself, keep the process running.
        // `new MessageChannel()` with an `onmessage` is ordinary code, and
        // before ports were host-backed it exited cleanly; a pump that counted
        // as work would turn every such program into one that hangs.
        //
        // Nothing is lost by it. A port whose peer is in this agent can only
        // receive if this agent runs code, so waiting on it with the loop
        // otherwise idle is provably futile. A port whose peer is in *another*
        // agent is held open by that agent instead — the worker keeps the
        // process alive, and this pump resolves whenever it posts.
        .unref(),
    )?;

    let h = hub.clone();
    engine.register_op(OpDecl::sync("port_detach", move |args| {
        let id = arg_u64(&args, 0);
        if let Some(hub) = h.as_ref() {
            hub.detach_reader(id);
        }
        Ok(Value::Undefined)
    }))?;

    let h = hub;
    engine.register_op(OpDecl::sync("port_close", move |args| {
        let id = arg_u64(&args, 0);
        if let Some(hub) = h.as_ref() {
            hub.close(id);
        }
        Ok(Value::Undefined)
    }))?;
    Ok(())
}

fn require_ports(
    hub: &Option<Arc<dyn es_runtime_providers::PortHub>>,
) -> std::result::Result<Arc<dyn es_runtime_providers::PortHub>, OpError> {
    hub.clone().ok_or_else(|| {
        OpError::new(
            ExceptionClass::Error,
            "MessagePort cannot be transferred (no PortHub configured)",
        )
        .with_code(ErrorCode::ProviderUnavailable)
    })
}

// ---- BroadcastChannel -------------------------------------------------------

/// Ops behind `BroadcastChannel`, routed through the [`BroadcastHub`].
///
/// Ungated, as the interface always has been: a channel reaches only agents
/// this runtime started, and carries a payload the sender could already build.
/// With no hub installed the prelude keeps its agent-local delivery, which is
/// what an embedder gets when it has no workers either.
pub(crate) fn install_broadcast(
    engine: &mut dyn Engine,
    hub: Option<Arc<dyn es_runtime_providers::BroadcastHub>>,
) -> Result<()> {
    // Sync, and the prelude's test for whether cross-agent delivery exists.
    let present = hub.is_some();
    engine.register_op(OpDecl::sync("broadcast_available", move |_args| {
        Ok(Value::Bool(present))
    }))?;

    // Sync, like `port_create` and for the same reason: `new BroadcastChannel()`
    // joins the channel as it is constructed, so there must be no window in
    // which a channel misses what the next line posts.
    let h = hub.clone();
    engine.register_op(OpDecl::sync("broadcast_subscribe", move |args| {
        let name = arg_str(&args, 0);
        let id = require_hub(&h)?.subscribe(name).map_err(map_err)?;
        Ok(Value::Number(id as f64))
    }))?;

    let h = hub.clone();
    engine.register_op(OpDecl::r#async("broadcast_publish", move |args| {
        let h = h.clone();
        let id = arg_u64(&args, 0);
        let message = arg_bytes(&args, 1);
        Box::pin(async move {
            require_hub(&h)?
                .publish(id, message)
                .await
                .map_err(map_err)?;
            Ok(Value::Undefined)
        })
    }))?;

    let h = hub.clone();
    engine.register_op(
        OpDecl::r#async("broadcast_recv_next", move |args| {
            let h = h.clone();
            let _ = args;
            Box::pin(async move {
                // One receive for the agent, not one per channel: the order
                // messages come back in *is* the delivery order, and the id
                // says which channel each is for.
                Ok(match require_hub(&h)?.recv_next().await.map_err(map_err)? {
                    Some((id, bytes)) => envelope_id(id, Value::Bytes(bytes)),
                    None => Value::Null,
                })
            })
        })
        // Unref'd, for the same reason as `port_recv`: an open BroadcastChannel
        // is not a reason for the process to keep running. A live worker that
        // might still post to it is.
        .unref(),
    )?;

    let h = hub;
    engine.register_op(OpDecl::r#async("broadcast_close", move |args| {
        let h = h.clone();
        let id = arg_u64(&args, 0);
        Box::pin(async move {
            if let Some(hub) = h.as_ref() {
                hub.close(id).await.map_err(map_err)?;
            }
            Ok(Value::Undefined)
        })
    }))?;
    Ok(())
}

fn require_hub(
    hub: &Option<Arc<dyn es_runtime_providers::BroadcastHub>>,
) -> std::result::Result<Arc<dyn es_runtime_providers::BroadcastHub>, OpError> {
    hub.clone().ok_or_else(|| {
        OpError::new(
            ExceptionClass::Error,
            "BroadcastChannel cannot reach other agents (no BroadcastHub configured)",
        )
        .with_code(ErrorCode::ProviderUnavailable)
    })
}

// ---- shared -----------------------------------------------------------------

/// A `{ id, data }` envelope: which subscription a broadcast is for.
fn envelope_id(id: u64, data: Value) -> Value {
    Value::Object(vec![
        ("id".to_string(), Value::Number(id as f64)),
        ("data".to_string(), data),
    ])
}

/// A `{ type, data }` envelope the prelude's pump dispatches on.
fn envelope(kind: &str, data: Value) -> Value {
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

fn arg_bytes(args: &[Value], i: usize) -> Vec<u8> {
    args.get(i)
        .and_then(Value::as_bytes)
        .map(<[u8]>::to_vec)
        .unwrap_or_default()
}

/// Reads `[[name, value], …]` — the shape `new Worker(url, { env })` sends.
/// Absent or null is `None`; an empty list is a worker handed no environment.
fn arg_pairs(args: &[Value], i: usize) -> Option<Vec<(String, String)>> {
    let Some(Value::Array(items)) = args.get(i) else {
        return None;
    };
    Some(
        items
            .iter()
            .filter_map(|item| match item {
                Value::Array(pair) if pair.len() == 2 => {
                    Some((pair[0].as_str()?.to_string(), pair[1].as_str()?.to_string()))
                }
                _ => None,
            })
            .collect(),
    )
}

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
    host: &Option<Arc<dyn WorkerHost>>,
) -> std::result::Result<Arc<dyn WorkerHost>, OpError> {
    host.clone().ok_or_else(|| {
        OpError::new(
            ExceptionClass::Error,
            "Worker is unavailable (no WorkerHost configured)",
        )
        .with_code(ErrorCode::ProviderUnavailable)
    })
}

fn require_scope(
    scope: &Option<Arc<dyn WorkerScope>>,
) -> std::result::Result<Arc<dyn WorkerScope>, OpError> {
    scope.clone().ok_or_else(|| {
        OpError::new(ExceptionClass::Error, "not running inside a worker")
            .with_code(ErrorCode::ProviderUnavailable)
    })
}

fn map_err(e: ProviderError) -> OpError {
    OpError::new(e.exception_class(), e.exception_message()).with_code_opt(e.code())
}
