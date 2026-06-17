//! Host ops backing `runtime:fs` (SPEC §11, DECISIONS D25), routed through the
//! [`FileSystem`] provider. Reads are gated on `Capability::FileRead` and
//! mutations on `Capability::FileWrite` — the security boundary is the op (D7) —
//! and the provider confines every path to its root jail. All ops are async
//! because file I/O is blocking work the driver offloads. `stat`/`readDir`
//! return JSON strings the prelude `JSON.parse`s (like `fetch`).

use std::sync::Arc;

use es_runtime_common::{Capability, ExceptionClass, IntoException};
use es_runtime_engine::{Engine, OpDecl, OpError, Value};
use es_runtime_providers::{DirEntry, FileStat, FileSystem, GlobScanOptions, ProviderError};

use crate::Result;

/// Registers the `runtime:fs` ops, capturing the (optional) [`FileSystem`].
pub(crate) fn install(engine: &mut dyn Engine, fs: Option<Arc<dyn FileSystem>>) -> Result<()> {
    let f = fs.clone();
    engine.register_op(
        OpDecl::r#async("fs_read", move |args| {
            let f = f.clone();
            let path = arg_str(&args, 0);
            Box::pin(async move {
                let bytes = require(&f)?.read(path).await.map_err(map_err)?;
                Ok(Value::Bytes(bytes))
            })
        })
        .requires(Capability::FileRead),
    )?;

    let f = fs.clone();
    engine.register_op(
        OpDecl::r#async("fs_stat", move |args| {
            let f = f.clone();
            let path = arg_str(&args, 0);
            Box::pin(async move {
                let s = require(&f)?.stat(path).await.map_err(map_err)?;
                Ok(stat_value(&s))
            })
        })
        .requires(Capability::FileRead),
    )?;

    let f = fs.clone();
    engine.register_op(
        OpDecl::r#async("fs_exists", move |args| {
            let f = f.clone();
            let path = arg_str(&args, 0);
            Box::pin(async move {
                let exists = require(&f)?.exists(path).await.map_err(map_err)?;
                Ok(Value::Bool(exists))
            })
        })
        .requires(Capability::FileRead),
    )?;

    let f = fs.clone();
    engine.register_op(
        OpDecl::r#async("fs_read_dir", move |args| {
            let f = f.clone();
            let path = arg_str(&args, 0);
            Box::pin(async move {
                let entries = require(&f)?.read_dir(path).await.map_err(map_err)?;
                Ok(dir_value(&entries))
            })
        })
        .requires(Capability::FileRead),
    )?;

    let f = fs.clone();
    engine.register_op(
        OpDecl::r#async("fs_write", move |mut args| {
            let f = f.clone();
            let path = arg_str(&args, 0);
            let append = arg_bool(&args, 2);
            // Move the payload out instead of borrowing + `to_vec`: a Uint8Array
            // arrives as `Value::Bytes` (marshal's one copy) and we take that Vec;
            // a string arrives as `Value::String` and we take its already-UTF-8
            // bytes — so writing a string costs no JS-side `TextEncoder` buffer
            // and no second copy here.
            let data = args
                .get_mut(1)
                .map(|v| std::mem::replace(v, Value::Undefined))
                .and_then(Value::into_bytes)
                .unwrap_or_default();
            Box::pin(async move {
                let n = require(&f)?
                    .write(path, data, append)
                    .await
                    .map_err(map_err)?;
                Ok(Value::Number(n as f64))
            })
        })
        .requires(Capability::FileWrite),
    )?;

    let f = fs.clone();
    engine.register_op(
        OpDecl::r#async("fs_mkdir", move |args| {
            let f = f.clone();
            let path = arg_str(&args, 0);
            let recursive = arg_bool(&args, 1);
            Box::pin(async move {
                require(&f)?.mkdir(path, recursive).await.map_err(map_err)?;
                Ok(Value::Undefined)
            })
        })
        .requires(Capability::FileWrite),
    )?;

    let f = fs.clone();
    engine.register_op(
        OpDecl::r#async("fs_remove", move |args| {
            let f = f.clone();
            let path = arg_str(&args, 0);
            let recursive = arg_bool(&args, 1);
            Box::pin(async move {
                require(&f)?
                    .remove(path, recursive)
                    .await
                    .map_err(map_err)?;
                Ok(Value::Undefined)
            })
        })
        .requires(Capability::FileWrite),
    )?;

    // glob match is pure pattern matching — no capability (still needs a
    // provider, which holds the matcher implementation).
    let f = fs.clone();
    engine.register_op(OpDecl::sync("glob_match", move |args| {
        let pattern = arg_str(&args, 0);
        let path = arg_str(&args, 1);
        Ok(Value::Bool(
            require(&f)?.glob_match(&pattern, &path).map_err(map_err)?,
        ))
    }))?;

    let f = fs.clone();
    engine.register_op(
        OpDecl::r#async("glob_scan", move |args| {
            let f = f.clone();
            let base = arg_str(&args, 0);
            let pattern = arg_str(&args, 1);
            let opts = GlobScanOptions {
                dot: arg_bool(&args, 2),
                absolute: arg_bool(&args, 3),
                only_files: arg_bool(&args, 4),
                follow_symlinks: arg_bool(&args, 5),
            };
            Box::pin(async move {
                let paths = require(&f)?
                    .glob_scan(base, pattern, opts)
                    .await
                    .map_err(map_err)?;
                Ok(strings_value(&paths))
            })
        })
        .requires(Capability::FileRead),
    )?;

    engine.register_op(
        OpDecl::r#async("fs_rename", move |args| {
            let f = fs.clone();
            let from = arg_str(&args, 0);
            let to = arg_str(&args, 1);
            Box::pin(async move {
                require(&f)?.rename(from, to).await.map_err(map_err)?;
                Ok(Value::Undefined)
            })
        })
        .requires(Capability::FileWrite),
    )?;

    Ok(())
}

fn arg_str(args: &[Value], i: usize) -> String {
    args.get(i)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn arg_bool(args: &[Value], i: usize) -> bool {
    matches!(args.get(i), Some(Value::Bool(true)))
}

fn require(fs: &Option<Arc<dyn FileSystem>>) -> std::result::Result<Arc<dyn FileSystem>, OpError> {
    fs.clone().ok_or_else(|| {
        OpError::new(
            ExceptionClass::Error,
            "filesystem is unavailable (no FileSystem provider configured)",
        )
    })
}

fn map_err(e: ProviderError) -> OpError {
    OpError::new(e.exception_class(), e.exception_message())
}

fn stat_value(s: &FileStat) -> Value {
    Value::Object(vec![
        ("size".to_string(), Value::Number(s.size as f64)),
        ("isFile".to_string(), Value::Bool(s.is_file)),
        ("isDir".to_string(), Value::Bool(s.is_dir)),
        ("isSymlink".to_string(), Value::Bool(s.is_symlink)),
        ("mtimeMs".to_string(), s.mtime_ms.map(|m| Value::Number(m as f64)).unwrap_or(Value::Null)),
    ])
}

fn strings_value(items: &[String]) -> Value {
    Value::Array(items.iter().map(|s| Value::String(s.clone())).collect())
}

fn dir_value(entries: &[DirEntry]) -> Value {
    Value::Array(entries.iter().map(|e| {
        Value::Object(vec![
            ("name".to_string(), Value::String(e.name.clone())),
            ("isFile".to_string(), Value::Bool(e.is_file)),
            ("isDir".to_string(), Value::Bool(e.is_dir)),
            ("isSymlink".to_string(), Value::Bool(e.is_symlink)),
        ])
    }).collect())
}
