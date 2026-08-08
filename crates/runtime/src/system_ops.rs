//! Host ops backing `runtime:system` — child processes (DECISIONS D37), routed
//! through the [`CommandProvider`].
//!
//! `system_spawn` is gated on [`Capability::Run`](es_runtime_common::Capability::Run);
//! the security boundary is the op, not the JS module (D7). Everything else
//! addresses a child by the id that spawn returned — and that id is proof of an
//! authorized spawn only for the agent the spawn happened in. The provider is
//! shared across every agent in the process and its ids are sequential, so each
//! of these ops checks ownership (D50, [`crate::handles`]) before it acts;
//! otherwise a worker holding no capability at all could write to, read from
//! and kill its parent's children by naming small integers.
//!
//! Note what is *not* here: nothing merges the host environment into a child.
//! The spec carries the child's complete environment, and a guest that wants to
//! inherit reads the environment through the `Env`-gated `runtime:process` ops
//! and passes it along. So `Run` alone can start a program but cannot hand it
//! the host's secrets, and the two grants compose instead of overlapping.

use std::sync::Arc;

use es_runtime_common::{Capability, ErrorCode, ExceptionClass, IntoException};
use es_runtime_engine::{Engine, OpDecl, OpError, Value};
use es_runtime_providers::{
    ChildStatus, ChildStream, CommandProvider, CommandSpec, ProviderError, Signal, Stdio,
};

use crate::Result;
use crate::handles::Handles;

pub(crate) fn install(
    engine: &mut dyn Engine,
    commands: Option<Arc<dyn CommandProvider>>,
) -> Result<()> {
    // The children this agent started.
    let children = Handles::new("child process");

    let c = commands.clone();
    let owned = children.clone();
    engine.register_op(
        OpDecl::r#async("system_spawn", move |args| {
            let c = c.clone();
            let owned = owned.clone();
            let spec = parse_spec(args.first());
            Box::pin(async move {
                let spec = spec?;
                let (id, pid) = require(&c)?.spawn(spec).await.map_err(map_err)?;
                Ok(Value::Object(vec![
                    ("id".to_string(), Value::Number(owned.own(id) as f64)),
                    ("pid".to_string(), Value::Number(pid as f64)),
                ]))
            })
        })
        .requires(Capability::Run),
    )?;

    let c = commands.clone();
    let owned = children.clone();
    engine.register_op(OpDecl::r#async("system_read", move |args| {
        let c = c.clone();
        let owned = owned.clone();
        let id = arg_u64(&args, 0);
        let stream = match args.get(1).and_then(Value::as_str) {
            Some("stderr") => ChildStream::Stderr,
            _ => ChildStream::Stdout,
        };
        Box::pin(async move {
            match require(&c)?
                .read(owned.check(id)?, stream)
                .await
                .map_err(map_err)?
            {
                Some(bytes) => Ok(Value::Bytes(bytes)),
                None => Ok(Value::Null),
            }
        })
    }))?;

    let c = commands.clone();
    let owned = children.clone();
    engine.register_op(OpDecl::r#async("system_write", move |args| {
        let c = c.clone();
        let owned = owned.clone();
        let id = arg_u64(&args, 0);
        let data = args
            .get(1)
            .and_then(Value::as_bytes)
            .map(<[u8]>::to_vec)
            .unwrap_or_default();
        Box::pin(async move {
            require(&c)?
                .write(owned.check(id)?, data)
                .await
                .map_err(map_err)?;
            Ok(Value::Undefined)
        })
    }))?;

    let c = commands.clone();
    let owned = children.clone();
    engine.register_op(OpDecl::r#async("system_stdin_close", move |args| {
        let c = c.clone();
        let owned = owned.clone();
        let id = arg_u64(&args, 0);
        Box::pin(async move {
            require(&c)?
                .close_stdin(owned.check(id)?)
                .await
                .map_err(map_err)?;
            Ok(Value::Undefined)
        })
    }))?;

    let c = commands.clone();
    let owned = children.clone();
    engine.register_op(OpDecl::r#async("system_wait", move |args| {
        let c = c.clone();
        let owned = owned.clone();
        let id = arg_u64(&args, 0);
        Box::pin(async move {
            let status = require(&c)?.wait(owned.check(id)?).await.map_err(map_err)?;
            Ok(status_value(&status))
        })
    }))?;

    let c = commands.clone();
    let owned = children.clone();
    engine.register_op(OpDecl::r#async("system_kill", move |args| {
        let c = c.clone();
        let owned = owned.clone();
        let id = arg_u64(&args, 0);
        let signal = parse_signal(args.get(1));
        Box::pin(async move {
            require(&c)?
                .kill(owned.check(id)?, signal?)
                .await
                .map_err(map_err)?;
            Ok(Value::Undefined)
        })
    }))?;

    let owned = children;
    engine.register_op(OpDecl::r#async("system_close", move |args| {
        let c = commands.clone();
        let owned = owned.clone();
        let id = arg_u64(&args, 0);
        Box::pin(async move {
            require(&c)?
                .close(owned.check_and_release(id)?)
                .await
                .map_err(map_err)?;
            Ok(Value::Undefined)
        })
    }))?;

    Ok(())
}

/// Reads the spawn spec out of the single object argument. The JS module has
/// already validated and normalized it; this is the marshaling, plus the
/// refusals that must not depend on JS having behaved (an op is reachable from
/// `__ops` directly).
fn parse_spec(value: Option<&Value>) -> std::result::Result<CommandSpec, OpError> {
    let fields = match value {
        Some(Value::Object(fields)) => fields,
        _ => return Err(type_error("a spawn spec object is required")),
    };
    let get = |name: &str| fields.iter().find(|(k, _)| k == name).map(|(_, v)| v);

    let program = get("program")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if program.is_empty() {
        return Err(type_error("a program name is required"));
    }
    let cwd = get("cwd")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    Ok(CommandSpec {
        program,
        args: string_list(get("args")),
        cwd,
        env: env_pairs(get("env")),
        stdin: stdio(get("stdin")),
        stdout: stdio(get("stdout")),
        stderr: stdio(get("stderr")),
    })
}

fn string_list(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

/// `[[name, value], …]` — the same shape `process_env` hands back, so the guest
/// can pass an inherited environment straight through.
fn env_pairs(value: Option<&Value>) -> Vec<(String, String)> {
    let items = match value {
        Some(Value::Array(items)) => items,
        _ => return Vec::new(),
    };
    items
        .iter()
        .filter_map(|pair| match pair {
            Value::Array(kv) if kv.len() == 2 => {
                Some((kv[0].as_str()?.to_string(), kv[1].as_str()?.to_string()))
            }
            _ => None,
        })
        .collect()
}

fn stdio(value: Option<&Value>) -> Stdio {
    match value.and_then(Value::as_str) {
        Some("piped") => Stdio::Piped,
        Some("inherit") => Stdio::Inherit,
        _ => Stdio::Null,
    }
}

/// A signal name, or a `TypeError` naming what was passed. An unknown name is
/// never silently downgraded to `SIGTERM`: killing with the wrong signal is a
/// different act from the one the caller asked for.
fn parse_signal(value: Option<&Value>) -> std::result::Result<Signal, OpError> {
    let name = value.and_then(Value::as_str).unwrap_or("");
    Signal::from_name(name)
        .ok_or_else(|| type_error(&format!("'{name}' is not a signal name this runtime knows")))
}

fn status_value(status: &ChildStatus) -> Value {
    Value::Object(vec![
        ("success".to_string(), Value::Bool(status.success)),
        (
            "code".to_string(),
            status
                .code
                .map(|c| Value::Number(c as f64))
                .unwrap_or(Value::Null),
        ),
        (
            "signal".to_string(),
            status
                .signal
                .clone()
                .map(Value::String)
                .unwrap_or(Value::Null),
        ),
    ])
}

fn arg_u64(args: &[Value], i: usize) -> u64 {
    args.get(i).and_then(Value::as_number).unwrap_or(0.0) as u64
}

fn type_error(message: &str) -> OpError {
    OpError::new(ExceptionClass::TypeError, message.to_string())
}

fn require(
    commands: &Option<Arc<dyn CommandProvider>>,
) -> std::result::Result<Arc<dyn CommandProvider>, OpError> {
    commands.clone().ok_or_else(|| {
        OpError::new(
            ExceptionClass::Error,
            "child processes are unavailable (no CommandProvider configured)",
        )
        .with_code(ErrorCode::ProviderUnavailable)
    })
}

fn map_err(e: ProviderError) -> OpError {
    OpError::new(e.exception_class(), e.exception_message()).with_code_opt(e.code())
}
