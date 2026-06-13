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
    // "hand the bytes back". The owned String's buffer becomes the returned
    // bytes (and ultimately the ArrayBuffer backing store) without a copy.
    engine.register_op(OpDecl::sync("utf8_encode", |args| {
        Ok(match args.into_iter().next() {
            Some(Value::String(s)) => Value::Bytes(s.into_bytes()),
            _ => Value::Bytes(Vec::new()),
        })
    }))?;

    // `(bytes, fatal, ignoreBOM)` → string. V8 builds the JS string natively.
    // The bytes are consumed: valid UTF-8 (the common case) converts in place
    // with no copy; only invalid input takes the lossy-replacement path.
    engine.register_op(OpDecl::sync("utf8_decode", |args| {
        let fatal = matches!(args.get(1), Some(Value::Bool(true)));
        let ignore_bom = matches!(args.get(2), Some(Value::Bool(true)));
        let mut bytes = match args.into_iter().next() {
            Some(Value::Bytes(b)) => b,
            _ => Vec::new(),
        };
        if !ignore_bom && bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
            bytes.drain(..3);
        }
        match String::from_utf8(bytes) {
            Ok(s) => Ok(Value::String(s)),
            Err(_) if fatal => Err(OpError::new(ExceptionClass::TypeError, "invalid UTF-8")),
            Err(e) => Ok(Value::String(
                String::from_utf8_lossy(e.as_bytes()).into_owned(),
            )),
        }
    }))?;
    Ok(())
}
