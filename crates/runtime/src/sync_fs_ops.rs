//! Synchronous host ops backing `runtime:wasi`'s file calls, routed through the
//! [`SyncFileSystem`] provider.
//!
//! These are the only **sync** I/O ops in the runtime, and they exist for one
//! reason: WASI's syscalls are synchronous, so a guest calling `fd_read` has no
//! way to await (see the [`SyncFileSystem`] docs). They block the runtime's
//! thread for the duration of the call.
//!
//! The gating is identical to `runtime:fs`: reads need `Capability::FileRead`,
//! mutations need `Capability::FileWrite`, and the provider confines every path
//! to its root jail. A WASI guest therefore passes exactly the same two checks —
//! the capability, then the jail — as any other file access. With no provider
//! installed, every op fails cleanly and `runtime:wasi` reports `ENOTCAPABLE`.

use std::sync::Arc;

use es_runtime_common::{Capability, ExceptionClass, IntoException};
use es_runtime_engine::{Engine, OpDecl, OpError, Value};
use es_runtime_providers::{
    DirEntry, FileStat, ProviderError, SyncFileSystem, SyncOpenOptions, SyncWhence,
};

use crate::Result;

/// Registers the synchronous filesystem ops, capturing the (optional) provider.
pub(crate) fn install(engine: &mut dyn Engine, fs: Option<Arc<dyn SyncFileSystem>>) -> Result<()> {
    // Opening is split in two so each half carries the right gate — `requires`
    // takes a single capability, and a read-only open must not demand
    // `FileWrite`. Which one the guest calls is decided by the requested mode.
    //
    // Enforcement stays per-operation regardless of how the handle was obtained:
    // reading bytes goes through the `FileRead`-gated `sync_fs_read` and writing
    // through the `FileWrite`-gated `sync_fs_write`, so a handle opened under one
    // capability cannot be used to do the other's work.
    let f = fs.clone();
    engine.register_op(
        OpDecl::sync("sync_fs_open", move |args| {
            let fs = require(&f)?;
            let options = open_options(&args, 1);
            if options.write || options.create || options.create_new || options.truncate {
                return Err(OpError::type_error(
                    "a mutating open must go through sync_fs_open_write",
                ));
            }
            let fd = fs.open(&arg_str(&args, 0), options).map_err(map_err)?;
            Ok(Value::Number(f64::from(fd)))
        })
        .requires(Capability::FileRead),
    )?;

    let f = fs.clone();
    engine.register_op(
        OpDecl::sync("sync_fs_open_write", move |args| {
            let fs = require(&f)?;
            let fd = fs
                .open(&arg_str(&args, 0), open_options(&args, 1))
                .map_err(map_err)?;
            Ok(Value::Number(f64::from(fd)))
        })
        .requires(Capability::FileWrite),
    )?;

    let f = fs.clone();
    engine.register_op(
        OpDecl::sync("sync_fs_read", move |args| {
            let fs = require(&f)?;
            let fd = arg_u32(&args, 0);
            let len = arg_u32(&args, 1) as usize;
            let mut buf = vec![0u8; len];
            let n = fs.read(fd, &mut buf).map_err(map_err)?;
            buf.truncate(n);
            Ok(Value::Bytes(buf))
        })
        .requires(Capability::FileRead),
    )?;

    let f = fs.clone();
    engine.register_op(
        OpDecl::sync("sync_fs_write", move |args| {
            let fs = require(&f)?;
            let fd = arg_u32(&args, 0);
            let data = match args.get(1) {
                Some(Value::Bytes(b)) => b.clone(),
                _ => return Err(OpError::type_error("sync_fs_write expects bytes")),
            };
            let n = fs.write(fd, &data).map_err(map_err)?;
            Ok(Value::Number(n as f64))
        })
        .requires(Capability::FileWrite),
    )?;

    let f = fs.clone();
    engine.register_op(
        OpDecl::sync("sync_fs_seek", move |args| {
            let fs = require(&f)?;
            let fd = arg_u32(&args, 0);
            let offset = args.get(1).and_then(Value::as_number).unwrap_or(0.0) as i64;
            let whence = match args.get(2).and_then(Value::as_number).unwrap_or(0.0) as u32 {
                1 => SyncWhence::Current,
                2 => SyncWhence::End,
                _ => SyncWhence::Start,
            };
            let pos = fs.seek(fd, offset, whence).map_err(map_err)?;
            Ok(Value::Number(pos as f64))
        })
        .requires(Capability::FileRead),
    )?;

    // Closing releases a handle the guest already holds; it grants nothing and
    // touches no path, so it carries no capability of its own.
    let f = fs.clone();
    engine.register_op(OpDecl::sync("sync_fs_close", move |args| {
        let fs = require(&f)?;
        fs.close(arg_u32(&args, 0)).map_err(map_err)?;
        Ok(Value::Undefined)
    }))?;

    let f = fs.clone();
    engine.register_op(
        OpDecl::sync("sync_fs_fstat", move |args| {
            let fs = require(&f)?;
            let stat = fs.fstat(arg_u32(&args, 0)).map_err(map_err)?;
            Ok(stat_value(&stat))
        })
        .requires(Capability::FileRead),
    )?;

    let f = fs.clone();
    engine.register_op(
        OpDecl::sync("sync_fs_stat", move |args| {
            let fs = require(&f)?;
            let stat = fs.stat(&arg_str(&args, 0)).map_err(map_err)?;
            Ok(stat_value(&stat))
        })
        .requires(Capability::FileRead),
    )?;

    let f = fs.clone();
    engine.register_op(
        OpDecl::sync("sync_fs_readdir", move |args| {
            let fs = require(&f)?;
            let entries = fs.read_dir(&arg_str(&args, 0)).map_err(map_err)?;
            Ok(Value::Array(entries.iter().map(entry_value).collect()))
        })
        .requires(Capability::FileRead),
    )?;

    let f = fs.clone();
    engine.register_op(
        OpDecl::sync("sync_fs_mkdir", move |args| {
            let fs = require(&f)?;
            fs.mkdir(&arg_str(&args, 0)).map_err(map_err)?;
            Ok(Value::Undefined)
        })
        .requires(Capability::FileWrite),
    )?;

    let f = fs.clone();
    engine.register_op(
        OpDecl::sync("sync_fs_remove_file", move |args| {
            let fs = require(&f)?;
            fs.remove_file(&arg_str(&args, 0)).map_err(map_err)?;
            Ok(Value::Undefined)
        })
        .requires(Capability::FileWrite),
    )?;

    let f = fs.clone();
    engine.register_op(
        OpDecl::sync("sync_fs_remove_dir", move |args| {
            let fs = require(&f)?;
            fs.remove_dir(&arg_str(&args, 0)).map_err(map_err)?;
            Ok(Value::Undefined)
        })
        .requires(Capability::FileWrite),
    )?;

    engine.register_op(
        OpDecl::sync("sync_fs_rename", move |args| {
            let fs = require(&fs)?;
            fs.rename(&arg_str(&args, 0), &arg_str(&args, 1))
                .map_err(map_err)?;
            Ok(Value::Undefined)
        })
        .requires(Capability::FileWrite),
    )?;

    Ok(())
}

fn require(
    fs: &Option<Arc<dyn SyncFileSystem>>,
) -> std::result::Result<Arc<dyn SyncFileSystem>, OpError> {
    fs.clone().ok_or_else(|| {
        OpError::new(
            ExceptionClass::Error,
            "no synchronous filesystem is available in this runtime",
        )
    })
}

fn map_err(e: ProviderError) -> OpError {
    OpError::new(e.exception_class(), e.exception_message()).with_code_opt(e.code())
}

fn arg_str(args: &[Value], i: usize) -> String {
    args.get(i)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn arg_u32(args: &[Value], i: usize) -> u32 {
    let n = args.get(i).and_then(Value::as_number).unwrap_or(0.0);
    if n.is_finite() && n >= 0.0 {
        n as u32
    } else {
        0
    }
}

fn arg_bool(args: &[Value], i: usize, key: &str) -> bool {
    match args.get(i) {
        Some(Value::Object(pairs)) => pairs
            .iter()
            .find(|(k, _)| k == key)
            .is_some_and(|(_, v)| matches!(v, Value::Bool(true))),
        _ => false,
    }
}

fn open_options(args: &[Value], i: usize) -> SyncOpenOptions {
    SyncOpenOptions {
        read: arg_bool(args, i, "read"),
        write: arg_bool(args, i, "write"),
        create: arg_bool(args, i, "create"),
        create_new: arg_bool(args, i, "createNew"),
        truncate: arg_bool(args, i, "truncate"),
        append: arg_bool(args, i, "append"),
        directory: arg_bool(args, i, "directory"),
    }
}

fn stat_value(s: &FileStat) -> Value {
    Value::Object(vec![
        ("size".into(), Value::Number(s.size as f64)),
        ("isFile".into(), Value::Bool(s.is_file)),
        ("isDir".into(), Value::Bool(s.is_dir)),
        ("isSymlink".into(), Value::Bool(s.is_symlink)),
        (
            "mtimeMs".into(),
            s.mtime_ms.map_or(Value::Null, Value::Number),
        ),
    ])
}

fn entry_value(e: &DirEntry) -> Value {
    Value::Object(vec![
        ("name".into(), Value::String(e.name.clone())),
        ("isFile".into(), Value::Bool(e.is_file)),
        ("isDir".into(), Value::Bool(e.is_dir)),
        ("isSymlink".into(), Value::Bool(e.is_symlink)),
    ])
}
