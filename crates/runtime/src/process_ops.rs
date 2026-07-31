//! Host ops backing `runtime:process` (DECISIONS D24): environment, arguments,
//! working directory, platform, and exit. All are gated on
//! [`Capability::Env`](es_runtime_common::Capability::Env) — the security
//! boundary is the op, not the JS module (D7) — and dispatch to the injected
//! [`Process`] provider. `process_exit` records the code and halts execution via
//! the engine's interrupt handle; the embedder reads the recorded code to learn
//! that exit (not an error) stopped the run.
//!
//! The signal ops alongside them are gated on
//! [`Capability::Signals`](es_runtime_common::Capability::Signals) instead: a
//! watch suppresses the signal's default action, so it is the privilege to
//! decline to die on request rather than a read of process state.

use std::sync::Arc;

use es_runtime_common::{Capability, ErrorCode, ExceptionClass, IntoException};
use es_runtime_engine::{Engine, InterruptHandle, OpDecl, OpError, Value};
use es_runtime_providers::{Process, Signal, Signals};

use crate::Result;

/// Registers the `runtime:process` ops, capturing the (optional) [`Process`]
/// provider and the engine's interrupt handle (for `exit`).
pub(crate) fn install(
    engine: &mut dyn Engine,
    process: Option<Arc<dyn Process>>,
    signals: Option<Arc<dyn Signals>>,
    interrupt: InterruptHandle,
) -> Result<()> {
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

    let p = process.clone();
    engine.register_op(
        OpDecl::sync("process_platform", move |_args| {
            let proc = require(&p)?;
            Ok(Value::String(proc.platform()))
        })
        .requires(Capability::Env),
    )?;

    let p = process.clone();
    engine.register_op(
        OpDecl::sync("process_arch", move |_args| {
            let proc = require(&p)?;
            Ok(Value::String(proc.arch()))
        })
        .requires(Capability::Env),
    )?;

    engine.register_op(
        OpDecl::sync("process_exit", move |args| {
            let proc = require(&process)?;
            let code = args.first().and_then(Value::as_number).unwrap_or(0.0) as i32;
            proc.exit(code);
            // Halt execution immediately (like Node's process.exit). The embedder
            // reads the recorded code and treats the resulting termination as a
            // clean exit rather than an error.
            interrupt.terminate();
            Ok(Value::Undefined)
        })
        .requires(Capability::Env),
    )?;

    install_signals(engine, signals)?;
    Ok(())
}

/// Registers the signal ops, gated on [`Capability::Signals`] — a separate grant
/// from `Env` because watching a signal suppresses its default action, which is
/// the privilege to decline to die on request, not a read of process state.
fn install_signals(engine: &mut dyn Engine, signals: Option<Arc<dyn Signals>>) -> Result<()> {
    let s = signals.clone();
    engine.register_op(
        OpDecl::sync("signal_available", move |_args| {
            let signals = require_signals(&s)?;
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
            let signals = require_signals(&s)?;
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
            require_signals(&s)?.unwatch(parse_signal(&args)?);
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
                let signals = require_signals(&signals)?;
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
) -> std::result::Result<Arc<dyn Signals>, OpError> {
    signals.clone().ok_or_else(|| {
        OpError::new(
            ExceptionClass::Error,
            "signals are unavailable (no Signals provider configured)",
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
