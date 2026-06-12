//! Host ops backing `TextEncoder`/`TextDecoder` (SPEC §2.3). UTF-8 transcoding
//! in Rust + V8's native string conversion is faster than the pure-JS
//! code-point loop for non-trivial inputs.

use es_runtime_common::ExceptionClass;
use es_runtime_engine::{Engine, OpDecl, OpError, Value};

use crate::Result;

/// Registers `utf8_encode` / `utf8_decode`.
pub(crate) fn install(engine: &mut dyn Engine) -> Result<()> {
    // `arg 0` arrives already transcoded UTF-16 → UTF-8 by V8 (lone surrogates
    // become U+FFFD), which is exactly TextEncoder semantics — so the op is just
    // "hand the bytes back".
    engine.register_op(OpDecl::sync("utf8_encode", |args| {
        let s = args.first().and_then(Value::as_str).unwrap_or("");
        Ok(Value::Bytes(s.as_bytes().to_vec()))
    }))?;

    // `(bytes, fatal, ignoreBOM)` → string. V8 builds the JS string natively.
    engine.register_op(OpDecl::sync("utf8_decode", |args| {
        let bytes = args.first().and_then(Value::as_bytes).unwrap_or(&[]);
        let fatal = matches!(args.get(1), Some(Value::Bool(true)));
        let ignore_bom = matches!(args.get(2), Some(Value::Bool(true)));
        let body = if !ignore_bom && bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
            &bytes[3..]
        } else {
            bytes
        };
        if fatal {
            match std::str::from_utf8(body) {
                Ok(s) => Ok(Value::String(s.to_string())),
                Err(_) => Err(OpError::new(ExceptionClass::TypeError, "invalid UTF-8")),
            }
        } else {
            Ok(Value::String(String::from_utf8_lossy(body).into_owned()))
        }
    }))?;
    Ok(())
}
