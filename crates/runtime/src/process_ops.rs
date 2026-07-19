//! Host ops backing `runtime:process` (DECISIONS D24): environment, arguments,
//! working directory, platform, and exit. All are gated on
//! [`Capability::Env`](es_runtime_common::Capability::Env) — the security
//! boundary is the op, not the JS module (D7) — and dispatch to the injected
//! [`Process`] provider. `process_exit` records the code and halts execution via
//! the engine's interrupt handle; the embedder reads the recorded code to learn
//! that exit (not an error) stopped the run.

use std::sync::Arc;

use es_runtime_common::{Capability, ErrorCode, ExceptionClass};
use es_runtime_engine::{Engine, InterruptHandle, OpDecl, OpError, Value};
use es_runtime_providers::Process;

use crate::Result;

/// Registers the `runtime:process` ops, capturing the (optional) [`Process`]
/// provider and the engine's interrupt handle (for `exit`).
pub(crate) fn install(
    engine: &mut dyn Engine,
    process: Option<Arc<dyn Process>>,
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
    Ok(())
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
