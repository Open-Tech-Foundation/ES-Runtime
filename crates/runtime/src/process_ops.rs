//! Host ops backing `runtime:process` (DECISIONS D24): environment, arguments,
//! working directory, platform, exit, and permission introspection. The ones
//! reading process *state* — env, args, cwd — are gated on
//! [`Capability::Env`](es_runtime_common::Capability::Env); the security
//! boundary is the op, not the JS module (D7, D38) — and dispatch to the
//! injected [`Process`] provider. `process_exit` records the code and halts
//! execution via the engine's interrupt handle; the embedder reads the recorded
//! code to learn that exit (not an error) stopped the run.
//!
//! The signal ops alongside them are gated on
//! [`Capability::Signals`](es_runtime_common::Capability::Signals) instead: a
//! watch suppresses the signal's default action, so it is the privilege to
//! decline to die on request rather than a read of process state.
//!
//! **Ungated here** (D38): `process_platform`/`process_arch` return
//! `std::env::consts` values — compile-time constants of the binary the guest is
//! already running, not host state — and `process_permissions_denied` reports
//! only what the guest could discover anyway by calling ops and catching the
//! denials. Gating those would make `runtime:process` unimportable under
//! `--deny-env`, which D26 forbids.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use es_runtime_common::{Capability, CapabilitySet, ErrorCode, ExceptionClass, IntoException};
use es_runtime_engine::{Engine, InterruptHandle, OpDecl, OpError, Value};
use es_runtime_providers::{Process, Signal, Signals};

use crate::Result;

/// Registers the `runtime:process` ops, capturing the (optional) [`Process`]
/// provider, the engine's interrupt handle (for `exit`), and the runtime's live
/// view of its own capability set (for `permissions`).
pub(crate) fn install(
    engine: &mut dyn Engine,
    process: Option<Arc<dyn Process>>,
    signals: Option<Arc<dyn Signals>>,
    interrupt: InterruptHandle,
    capabilities: Rc<Cell<CapabilitySet>>,
    is_worker: bool,
) -> Result<()> {
    // Ungated, unlike `process_env` beside it, and safe for one reason: it can
    // only report what a *parent handed this worker* — values that parent could
    // already read and chose to pass on. On the agent driving the process it is
    // always null. Reading data someone handed you is not an authority, which
    // is why `new Worker(url, { env })` works without granting `env`.
    let p = process.clone();
    engine.register_op(OpDecl::sync("process_env_provided", move |_args| {
        let proc = require(&p)?;
        Ok(match proc.provided_env() {
            Some(pairs) => Value::Array(
                pairs
                    .into_iter()
                    .map(|(k, v)| Value::Array(vec![Value::String(k), Value::String(v)]))
                    .collect(),
            ),
            None => Value::Null,
        })
    }))?;

    let p = process.clone();
    engine.register_op(
        OpDecl::sync("process_env", move |_args| {
            let proc = require(&p)?;
            let pairs = proc
                .env()
                .into_iter()
                .map(|(k, v)| Value::Array(vec![Value::String(k), Value::String(v)]))
                .collect();
            Ok(Value::Array(pairs))
        })
        .requires(Capability::Env),
    )?;

    let p = process.clone();
    engine.register_op(
        OpDecl::sync("process_args", move |_args| {
            let proc = require(&p)?;
            Ok(Value::Array(
                proc.args().into_iter().map(Value::String).collect(),
            ))
        })
        .requires(Capability::Env),
    )?;

    let p = process.clone();
    engine.register_op(
        OpDecl::sync("process_cwd", move |_args| {
            let proc = require(&p)?;
            proc.cwd()
                .map(Value::String)
                .map_err(|e| OpError::new(ExceptionClass::Error, e.to_string()))
        })
        .requires(Capability::Env),
    )?;

    // Ungated (D38): the host OS and CPU architecture are properties of the
    // binary the guest is already executing, not of the host's state. They are
    // also read at module-evaluation time by `runtime:process`, so gating them
    // would make the module unimportable under `--deny-env` (D26).
    let p = process.clone();
    engine.register_op(OpDecl::sync("process_platform", move |_args| {
        let proc = require(&p)?;
        Ok(Value::String(proc.platform()))
    }))?;

    let p = process.clone();
    engine.register_op(OpDecl::sync("process_concurrency", move |_args| {
        // Ungated, like platform/arch above, and with a working default when no
        // Process provider is installed: `navigator.hardwareConcurrency` is
        // defined to be a number, so reporting 1 is better than throwing.
        Ok(Value::Number(
            p.as_ref().map_or(1, |proc| proc.hardware_concurrency()) as f64,
        ))
    }))?;

    let p = process.clone();
    engine.register_op(OpDecl::sync("process_arch", move |_args| {
        let proc = require(&p)?;
        Ok(Value::String(proc.arch()))
    }))?;

    // Permission introspection (D38). Ungated, and deliberately so: it reveals
    // only what the guest could learn by calling each op and catching the
    // denial, and code that must ask "may I?" before trying is exactly the code
    // running under the tightest policy.
    engine.register_op(OpDecl::sync("process_permissions_denied", move |_args| {
        Ok(Value::Array(
            capabilities
                .get()
                .denied_names()
                .into_iter()
                .map(|name| Value::String(name.to_string()))
                .collect(),
        ))
    }))?;

    // Ungated (D38): stopping is the guest's own control flow, not authority over
    // the host — code that can run can always stop running, and the exit code is
    // the one thing it may already say on the way out. Node and Deno gate it no
    // further either. Gating it on `Env` (as this once did) meant `--deny-env`
    // denied the unrelated ability to exit cleanly.
    engine.register_op(OpDecl::sync("process_exit", move |args| {
        let proc = require(&process)?;
        let code = args.first().and_then(Value::as_number).unwrap_or(0.0) as i32;
        proc.exit(code);
        // Halt execution immediately (like Node's process.exit). The embedder
        // reads the recorded code and treats the resulting termination as a
        // clean exit rather than an error.
        interrupt.terminate();
        Ok(Value::Undefined)
    }))?;

    install_signals(engine, signals, is_worker)?;
    Ok(())
}

/// Registers the signal ops, gated on [`Capability::Signals`] — a separate grant
/// from `Env` because watching a signal suppresses its default action, which is
/// the privilege to decline to die on request, not a read of process state.
fn install_signals(
    engine: &mut dyn Engine,
    signals: Option<Arc<dyn Signals>>,
    is_worker: bool,
) -> Result<()> {
    // A worker agent does not watch signals. A signal is delivered to the
    // *process*, and watching one suppresses the default action — so a worker
    // that took `SIGTERM` would be deciding, from a thread the program may not
    // even know is running, whether the whole process declines to die. Process
    // control belongs to the agent that owns the process. Node reaches the same
    // conclusion: `process.on('SIGTERM')` in a worker thread does nothing.
    let signals = if is_worker { None } else { signals };
    let s = signals.clone();
    engine.register_op(
        OpDecl::sync("signal_available", move |_args| {
            let signals = require_signals(&s, is_worker)?;
            Ok(Value::Array(
                signals
                    .available()
                    .into_iter()
                    .map(|sig| Value::String(sig.name().to_string()))
                    .collect(),
            ))
        })
        .requires(Capability::Signals),
    )?;

    let s = signals.clone();
    engine.register_op(
        OpDecl::sync("signal_watch", move |args| {
            let signals = require_signals(&s, is_worker)?;
            signals.watch(parse_signal(&args)?).map_err(|e| {
                OpError::new(e.exception_class(), e.exception_message()).with_code_opt(e.code())
            })?;
            Ok(Value::Undefined)
        })
        .requires(Capability::Signals),
    )?;

    let s = signals.clone();
    engine.register_op(
        OpDecl::sync("signal_unwatch", move |args| {
            require_signals(&s, is_worker)?.unwatch(parse_signal(&args)?);
            Ok(Value::Undefined)
        })
        .requires(Capability::Signals),
    )?;

    // Resolves with the name of the next watched signal to arrive, or `null`
    // once nothing is watched — so the guest's pump is released rather than
    // holding the driven loop open forever.
    engine.register_op(
        OpDecl::r#async("signal_next", move |_args| {
            let signals = signals.clone();
            Box::pin(async move {
                let signals = require_signals(&signals, is_worker)?;
                Ok(match signals.next().await {
                    Some(sig) => Value::String(sig.name().to_string()),
                    None => Value::Null,
                })
            })
        })
        .requires(Capability::Signals),
    )?;
    Ok(())
}

/// Reads a signal name from the op's first argument. An unknown name is a
/// `TypeError` naming it, never a silently ignored registration.
fn parse_signal(args: &[Value]) -> std::result::Result<Signal, OpError> {
    let name = args.first().and_then(Value::as_str).unwrap_or("");
    Signal::from_name(name).ok_or_else(|| {
        OpError::new(
            ExceptionClass::TypeError,
            format!("'{name}' is not a signal name this runtime knows"),
        )
    })
}

fn require_signals(
    signals: &Option<Arc<dyn Signals>>,
    is_worker: bool,
) -> std::result::Result<Arc<dyn Signals>, OpError> {
    signals.clone().ok_or_else(|| {
        OpError::new(
            ExceptionClass::Error,
            if is_worker {
                "signals cannot be watched from a worker: a signal is delivered \
                 to the process, and watching one suppresses the default action, \
                 so that belongs to the agent that owns the process"
            } else {
                "signals are unavailable (no Signals provider configured)"
            },
        )
        .with_code(ErrorCode::ProviderUnavailable)
    })
}

fn require(process: &Option<Arc<dyn Process>>) -> std::result::Result<Arc<dyn Process>, OpError> {
    process.clone().ok_or_else(|| {
        OpError::new(
            ExceptionClass::Error,
            "process info is unavailable (no Process provider configured)",
        )
        .with_code(ErrorCode::ProviderUnavailable)
    })
}
