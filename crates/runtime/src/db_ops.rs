//! Host ops backing `runtime:db`'s embedded backends (DECISIONS.md D56), routed
//! through the [`EmbeddedDb`] provider.
//!
//! Networked backends have no ops of their own: a Postgres or MySQL driver is
//! JS over `runtime:net`, and reaches the host through `net_connect` and its
//! siblings. What lives here is the one case that cannot be written in JS —
//! an engine running in this process.
//!
//! **Capabilities.** Opening a database is a filesystem access and is gated as
//! one: [`FileRead`](Capability::FileRead) to read, and
//! [`FileWrite`](Capability::FileWrite) as well to write. That is why opening
//! is two ops rather than one with a flag — a capability an op *might* need is
//! not a gate, so the read-only open is its own op and demands only what it
//! uses. `runtime:db` adds no capability of its own: it reaches nowhere
//! `runtime:fs` and `runtime:net` cannot already reach, and
//! `--allow-read=./data` scopes a database exactly as it scopes a file.
//!
//! Everything after the open — query, fetch, execute, close — carries an
//! **ownership check** instead ([`crate::handles`], D50) and no capability: the
//! open was authorized, and the ids are sequential across a shared provider, so
//! without the check a worker holding nothing could read another agent's result
//! sets by naming small integers.
//!
//! **Values cross as bytes, both ways.** Rows come back in the layout
//! [`EmbeddedDb`] documents, and parameters go out in the same tagged encoding.
//! A JS array of parameters would have to marshal per value and could not carry
//! an `i64` at all, since [`Value`] has no bigint; one buffer written with a
//! `DataView` carries both exactly.

use std::sync::Arc;

use es_runtime_common::{Capability, ErrorCode, ExceptionClass, IntoException};
use es_runtime_engine::{Engine, OpDecl, OpError, Value};
use es_runtime_providers::{
    DbColumn, DbParams, DbValue, EmbeddedDb, EmbeddedDbOptions, ProviderError,
};

use crate::Result;
use crate::handles::Handles;

/// The per-value type tags shared with the provider's row encoding.
const TAG_INTEGER: u8 = 1;
const TAG_REAL: u8 = 2;
const TAG_TEXT: u8 = 3;
const TAG_BLOB: u8 = 4;

pub(crate) fn install(engine: &mut dyn Engine, db: Option<Arc<dyn EmbeddedDb>>) -> Result<()> {
    // Connections and cursors are separate namespaces in the provider, so they
    // are separate registries here: a cursor id and a connection id may
    // collide, and fetching from a connection is not a request worth honouring.
    let conns = Handles::new("database");
    let cursors = Handles::new("cursor");

    for (name, read_only) in [("db_open", false), ("db_open_read_only", true)] {
        let d = db.clone();
        let owned = conns.clone();
        let mut op = OpDecl::r#async(name, move |args| {
            let d = d.clone();
            let owned = owned.clone();
            let path = arg_str(&args, 0);
            let hex_key = arg_str(&args, 1);
            let cipher = arg_str(&args, 2);
            let opts = EmbeddedDbOptions {
                read_only,
                hex_key: (!hex_key.is_empty()).then_some(hex_key),
                cipher: (!cipher.is_empty()).then_some(cipher),
            };
            Box::pin(async move {
                let id = require(&d)?.open(path, opts).await.map_err(map_err)?;
                Ok(Value::Number(owned.own(id) as f64))
            })
        })
        .requires(Capability::FileRead);
        if !read_only {
            op = op.requires(Capability::FileWrite);
        }
        engine.register_op(op)?;
    }

    let d = db.clone();
    let owned_conns = conns.clone();
    let owned_cursors = cursors.clone();
    engine.register_op(OpDecl::r#async("db_query", move |args| {
        let d = d.clone();
        let owned_conns = owned_conns.clone();
        let owned_cursors = owned_cursors.clone();
        let id = arg_u64(&args, 0);
        let sql = arg_str(&args, 1);
        let params = decode_params(args.get(2).and_then(Value::as_bytes).unwrap_or(&[]));
        Box::pin(async move {
            let id = owned_conns.check(id)?;
            let cursor = require(&d)?
                .query(id, sql, params?)
                .await
                .map_err(map_err)?;
            Ok(Value::Object(vec![
                (
                    "id".to_string(),
                    Value::Number(owned_cursors.own(cursor.id) as f64),
                ),
                ("columns".to_string(), columns_value(&cursor.columns)),
            ]))
        })
    }))?;

    let d = db.clone();
    let owned = cursors.clone();
    engine.register_op(OpDecl::r#async("db_fetch", move |args| {
        let d = d.clone();
        let owned = owned.clone();
        let id = arg_u64(&args, 0);
        let max_bytes = arg_u64(&args, 1) as usize;
        Box::pin(async move {
            let id = owned.check(id)?;
            let batch = require(&d)?.fetch(id, max_bytes).await.map_err(map_err)?;
            Ok(Value::Object(vec![
                ("bytes".to_string(), Value::Bytes(batch.bytes)),
                ("rows".to_string(), Value::Number(batch.rows as f64)),
                ("done".to_string(), Value::Bool(batch.done)),
            ]))
        })
    }))?;

    let d = db.clone();
    let owned = conns.clone();
    engine.register_op(OpDecl::r#async("db_execute", move |args| {
        let d = d.clone();
        let owned = owned.clone();
        let id = arg_u64(&args, 0);
        let sql = arg_str(&args, 1);
        let params = decode_params(args.get(2).and_then(Value::as_bytes).unwrap_or(&[]));
        Box::pin(async move {
            let id = owned.check(id)?;
            let result = require(&d)?
                .execute(id, sql, params?)
                .await
                .map_err(map_err)?;
            Ok(Value::Object(vec![
                ("changes".to_string(), Value::Number(result.changes as f64)),
                (
                    "lastInsertRowid".to_string(),
                    match result.last_insert_rowid {
                        Some(id) => Value::Number(id as f64),
                        None => Value::Null,
                    },
                ),
            ]))
        })
    }))?;

    let d = db.clone();
    let owned = cursors;
    engine.register_op(OpDecl::r#async("db_close_cursor", move |args| {
        let d = d.clone();
        let owned = owned.clone();
        let id = arg_u64(&args, 0);
        Box::pin(async move {
            require(&d)?
                .close_cursor(owned.check_and_release(id)?)
                .await
                .map_err(map_err)?;
            Ok(Value::Undefined)
        })
    }))?;

    let owned = conns;
    engine.register_op(OpDecl::r#async("db_close", move |args| {
        let d = db.clone();
        let owned = owned.clone();
        let id = arg_u64(&args, 0);
        Box::pin(async move {
            require(&d)?
                .close(owned.check_and_release(id)?)
                .await
                .map_err(map_err)?;
            Ok(Value::Undefined)
        })
    }))?;

    Ok(())
}

fn columns_value(columns: &[DbColumn]) -> Value {
    Value::Array(
        columns
            .iter()
            .map(|c| {
                Value::Object(vec![
                    ("name".to_string(), Value::String(c.name.clone())),
                    (
                        "declType".to_string(),
                        c.decl_type
                            .clone()
                            .map(Value::String)
                            .unwrap_or(Value::Null),
                    ),
                ])
            })
            .collect(),
    )
}

/// Reads the parameter buffer the JS side writes.
///
/// Malformed input is a bug in the module that wrote it, not guest data — but
/// it is still guest-reachable memory, so every read is bounds-checked and a
/// short buffer is an error rather than a panic.
fn decode_params(bytes: &[u8]) -> std::result::Result<DbParams, OpError> {
    if bytes.is_empty() {
        return Ok(DbParams::default());
    }
    let mut r = Reader { bytes, at: 0 };
    let positional_count = r.i16()? as usize;
    let mut positional = Vec::with_capacity(positional_count.min(64));
    for _ in 0..positional_count {
        positional.push(r.value()?);
    }
    let named_count = r.i16()? as usize;
    let mut named = Vec::with_capacity(named_count.min(64));
    for _ in 0..named_count {
        let len = r.i32()?;
        let name = String::from_utf8(r.take(len.max(0) as usize)?.to_vec())
            .map_err(|_| malformed("a parameter name is not UTF-8"))?;
        named.push((name, r.value()?));
    }
    Ok(DbParams { positional, named })
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> std::result::Result<&[u8], OpError> {
        let end = self
            .at
            .checked_add(n)
            .filter(|end| *end <= self.bytes.len())
            .ok_or_else(|| malformed("the parameter buffer ended mid-value"))?;
        let slice = &self.bytes[self.at..end];
        self.at = end;
        Ok(slice)
    }

    fn i16(&mut self) -> std::result::Result<i16, OpError> {
        Ok(i16::from_be_bytes(self.take(2)?.try_into().unwrap()))
    }

    fn i32(&mut self) -> std::result::Result<i32, OpError> {
        Ok(i32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn value(&mut self) -> std::result::Result<DbValue, OpError> {
        let len = self.i32()?;
        if len < 0 {
            return Ok(DbValue::Null);
        }
        let payload = self.take(len as usize)?;
        let (tag, body) = payload
            .split_first()
            .ok_or_else(|| malformed("a parameter carries no type tag"))?;
        match *tag {
            TAG_INTEGER => Ok(DbValue::Integer(i64::from_be_bytes(
                body.try_into()
                    .map_err(|_| malformed("an integer parameter is not 8 bytes"))?,
            ))),
            TAG_REAL => Ok(DbValue::Real(f64::from_be_bytes(
                body.try_into()
                    .map_err(|_| malformed("a real parameter is not 8 bytes"))?,
            ))),
            TAG_TEXT => Ok(DbValue::Text(
                String::from_utf8(body.to_vec())
                    .map_err(|_| malformed("a text parameter is not UTF-8"))?,
            )),
            TAG_BLOB => Ok(DbValue::Blob(body.to_vec())),
            other => Err(malformed(&format!("unknown parameter tag {other}"))),
        }
    }
}

fn malformed(message: &str) -> OpError {
    OpError::new(ExceptionClass::TypeError, message).with_code(ErrorCode::Io)
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

fn require(db: &Option<Arc<dyn EmbeddedDb>>) -> std::result::Result<Arc<dyn EmbeddedDb>, OpError> {
    db.clone().ok_or_else(|| {
        OpError::new(
            ExceptionClass::Error,
            "embedded databases are unavailable (no EmbeddedDb provider configured)",
        )
        .with_code(ErrorCode::ProviderUnavailable)
    })
}

fn map_err(e: ProviderError) -> OpError {
    OpError::new(e.exception_class(), e.exception_message()).with_code_opt(e.code())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a parameter buffer the way the JS side does.
    fn buffer(positional: &[(u8, &[u8])], named: &[(&str, u8, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(positional.len() as i16).to_be_bytes());
        for (tag, body) in positional {
            out.extend_from_slice(&((body.len() + 1) as i32).to_be_bytes());
            out.push(*tag);
            out.extend_from_slice(body);
        }
        out.extend_from_slice(&(named.len() as i16).to_be_bytes());
        for (name, tag, body) in named {
            out.extend_from_slice(&(name.len() as i32).to_be_bytes());
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(&((body.len() + 1) as i32).to_be_bytes());
            out.push(*tag);
            out.extend_from_slice(body);
        }
        out
    }

    #[test]
    fn an_empty_buffer_is_no_parameters() {
        let params = decode_params(&[]).unwrap();
        assert!(params.positional.is_empty() && params.named.is_empty());
    }

    #[test]
    fn every_tag_round_trips() {
        let bytes = buffer(
            &[
                (TAG_INTEGER, &i64::MAX.to_be_bytes()),
                (TAG_REAL, &1.5f64.to_be_bytes()),
                (TAG_TEXT, b"hi"),
                (TAG_BLOB, &[0, 255]),
            ],
            &[("label", TAG_TEXT, b"ok")],
        );
        let params = decode_params(&bytes).unwrap();
        assert_eq!(
            params.positional,
            vec![
                // The reason integers cross as eight bytes: this value is not
                // representable as a JS number, and a double would round it.
                DbValue::Integer(i64::MAX),
                DbValue::Real(1.5),
                DbValue::Text("hi".to_string()),
                DbValue::Blob(vec![0, 255]),
            ]
        );
        assert_eq!(
            params.named,
            vec![("label".to_string(), DbValue::Text("ok".to_string()))]
        );
    }

    #[test]
    fn a_negative_length_is_null_rather_than_a_huge_read() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&1i16.to_be_bytes());
        bytes.extend_from_slice(&(-1i32).to_be_bytes());
        bytes.extend_from_slice(&0i16.to_be_bytes());
        let params = decode_params(&bytes).unwrap();
        assert_eq!(params.positional, vec![DbValue::Null]);
    }

    /// Every one of these used to be a panic waiting for a caller that got the
    /// encoding wrong. They are errors: the buffer is written by a module, but
    /// it is built from guest values and reaches the host as guest bytes.
    #[test]
    fn a_malformed_buffer_is_an_error_and_never_a_panic() {
        let full = buffer(&[(TAG_TEXT, b"hello")], &[("a", TAG_BLOB, &[1])]);
        for cut in 1..full.len() {
            // Truncated anywhere: either a clean parse of a prefix that happens
            // to be well-formed, or an error. Never a panic, and never a read
            // past the end.
            let _ = decode_params(&full[..cut]);
        }

        // A count that promises more values than the buffer holds.
        let mut lying = Vec::new();
        lying.extend_from_slice(&999i16.to_be_bytes());
        assert!(decode_params(&lying).is_err());

        // A value whose payload carries no tag at all.
        let mut untagged = Vec::new();
        untagged.extend_from_slice(&1i16.to_be_bytes());
        untagged.extend_from_slice(&0i32.to_be_bytes());
        assert!(decode_params(&untagged).is_err());

        // A tag nothing answers to.
        assert!(decode_params(&buffer(&[(99, b"x")], &[])).is_err());

        // An integer that is not eight bytes: a driver bug that would otherwise
        // read whatever followed it.
        assert!(decode_params(&buffer(&[(TAG_INTEGER, &[1, 2, 3])], &[])).is_err());

        // Text that is not UTF-8, in a value and in a name.
        assert!(decode_params(&buffer(&[(TAG_TEXT, &[0xff, 0xfe])], &[])).is_err());
        assert!(decode_params(&buffer(&[], &[("\u{fffd}", TAG_TEXT, b"x")])).is_ok());
    }
}
