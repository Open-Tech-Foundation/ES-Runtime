//! Host ops backing the pure-JS prelude — the "world-touching" parts that the
//! prelude calls through `globalThis.__ops` (ARCHITECTURE.md §4).
//!
//! Each op captures the provider it needs, so the prelude itself stays free of
//! host concerns. Pure-computation ops (encoding, URL — later increments) need
//! no provider; the ops here are the provider-backed ones for Phase 4.

use std::sync::Arc;

use es_runtime_engine::{Engine, OpDecl, Value};
use es_runtime_providers::{Clock, Console, ConsoleLevel};

use crate::{HostProviders, Result};

/// Registers every built-in host op on `engine`.
pub(crate) fn install(engine: &mut dyn Engine, providers: &HostProviders) -> Result<()> {
    install_console(engine, providers.console())?;
    install_performance(engine, providers.clock())?;
    // Pure-computation ops (no provider): URL parsing, UTF-8 transcoding,
    // base64.
    crate::url_ops::install(engine)?;
    crate::encoding_ops::install(engine)?;
    crate::base64_ops::install(engine)?;
    // Networking ops, capability-gated on Net.
    crate::fetch_ops::install(engine, providers.net())?;
    // WebCrypto ops, backed by the Entropy provider + RustCrypto.
    crate::crypto_ops::install(engine, providers.entropy())?;
    // Elliptic-curve WebCrypto (ECDSA/ECDH), also Entropy-backed for key gen.
    crate::ec_ops::install(engine, providers.entropy())?;
    // RSA WebCrypto (PKCS1-v1_5 / PSS / OAEP), Entropy-backed for key gen/salt.
    crate::rsa_ops::install(engine, providers.entropy())?;
    // runtime:process ops, gated on Env; `exit` halts via the interrupt handle.
    let interrupt = engine.interrupt_handle();
    crate::process_ops::install(engine, providers.process(), interrupt)?;
    // runtime:fs ops, gated on FileRead / FileWrite, jailed by the provider.
    crate::fs_ops::install(engine, providers.file_system())?;
    // runtime:net ops: connect (Net), listen (NetListen); read/write/accept by id.
    crate::net_ops::install(engine, providers.net_provider())?;
    // runtime:http ops: serve (NetListen); next_request/body_read/respond by id.
    crate::http_ops::install(engine, providers.http_server())?;
    // WebSocket global ops: connect (Net); send/recv/close by id (DECISIONS D29).
    crate::ws_ops::install(engine, providers.web_socket())?;
    crate::parsers_ops::install(engine)?;
    Ok(())
}

/// `__ops.console(level, message)` → the [`Console`] sink. The prelude formats
/// arguments into `message`; the level is one of debug/info/log/warn/error.
fn install_console(engine: &mut dyn Engine, console: Arc<dyn Console>) -> Result<()> {
    engine.register_op(OpDecl::sync("console", move |args| {
        let level = match args.first().and_then(Value::as_str) {
            Some("debug") => ConsoleLevel::Debug,
            Some("info") => ConsoleLevel::Info,
            Some("warn") => ConsoleLevel::Warn,
            Some("error") => ConsoleLevel::Error,
            _ => ConsoleLevel::Log,
        };
        let message = args.get(1).and_then(Value::as_str).unwrap_or("");
        console.write(level, message);
        Ok(Value::Undefined)
    }))?;
    Ok(())
}

/// `__ops.now()` → monotonic ms with sub-ms (µs) precision (`performance.now`);
/// `__ops.time_origin()` → the wall-clock ms captured at construction
/// (`performance.timeOrigin`).
fn install_performance(engine: &mut dyn Engine, clock: Arc<dyn Clock>) -> Result<()> {
    let now_clock = clock.clone();
    engine.register_op(OpDecl::sync("now", move |_args| {
        Ok(Value::Number(now_clock.monotonic_micros() as f64 / 1_000.0))
    }))?;

    // timeOrigin is fixed at construction.
    let origin = clock.wall_ms() as f64;
    engine.register_op(OpDecl::sync("time_origin", move |_args| {
        Ok(Value::Number(origin))
    }))?;
    Ok(())
}
