//! Host ops backing `runtime:archive` (DECISIONS.md D174).
//!
//! Bytes in and out need no capability: encoding, listing and decoding are
//! pure computation over a `Uint8Array` the guest already holds. Reading a
//! `file()` source is gated on `FileRead`; `archive_extract` writes through the
//! [`FileSystem`] provider and is gated on `FileWrite` with the destination
//! folder as its target (so a path-scoped grant sees the folder it names).
//!
//! A file source is never read into a single buffer. It is scanned through the
//! jailed [`SyncFileSystem`], entries are streamed in bounded chunks, and
//! extraction writes each chunk before the producer can read the next. A
//! provider without a synchronous filesystem falls back to reading the file
//! once, which is the best that seam can do.
//!
//! Every read is validated by [`crate::archive`] before the guest sees an
//! entry. Extraction decodes into a temporary directory first and only copies
//! files into the destination after the complete pass succeeds.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use es_runtime_common::{Capability, ExceptionClass, IntoException};
use es_runtime_engine::{Engine, OpDecl, OpError, Value};
use es_runtime_providers::{FileSystem, ProviderError, SyncFileSystem, TaskSpawner};
use futures_channel::mpsc;
use futures_util::StreamExt;

use crate::Result;
use crate::archive::{
    self, Compression, EntryMeta, ExtractEvent, Format, InputEntry, Limits, MetaEvent, Source,
    StreamEvent,
};

struct ArchiveHandle {
    source: Source,
    format: Format,
    limits: Limits,
}

struct StreamHandle {
    rx: mpsc::Receiver<StreamEvent>,
}

struct MetaHandle {
    rx: mpsc::Receiver<MetaEvent>,
}

/// Registers the `runtime:archive` ops.
pub(crate) fn install(
    engine: &mut dyn Engine,
    fs: Option<Arc<dyn FileSystem>>,
    sync_fs: Option<Arc<dyn SyncFileSystem>>,
    spawner: Option<Arc<dyn TaskSpawner>>,
) -> Result<()> {
    let archives: Rc<RefCell<HashMap<u64, ArchiveHandle>>> = Rc::new(RefCell::new(HashMap::new()));
    let streams: Rc<RefCell<HashMap<u64, StreamHandle>>> = Rc::new(RefCell::new(HashMap::new()));
    let metas: Rc<RefCell<HashMap<u64, MetaHandle>>> = Rc::new(RefCell::new(HashMap::new()));
    let next_archive = Rc::new(RefCell::new(0_u64));
    let next_stream = Rc::new(RefCell::new(0_u64));
    let next_meta = Rc::new(RefCell::new(0_u64));

    engine.register_op(OpDecl::sync("archive_build", |mut args| {
        let entries = parse_entries(args.get_mut(0))?;
        let format = parse_format(args.get(1))?;
        let compress = parse_compression(args.get(2))?;
        let limits = parse_limits(args.get(3))?;
        let bytes = archive::build(entries, format, compress, &limits).map_err(op_error)?;
        Ok(Value::Bytes(bytes))
    }))?;

    let (reg, ids) = (archives.clone(), next_archive.clone());
    engine.register_op(OpDecl::r#async("archive_open_bytes", move |args| {
        let parsed: std::result::Result<(Source, Limits), OpError> = (|| {
            let bytes = bytes_arg(&args, 0, "source")?;
            let limits = parse_limits(args.get(1))?;
            Ok((Source::bytes(bytes), limits))
        })();
        let (reg, ids) = (reg.clone(), ids.clone());
        Box::pin(async move {
            let (source, limits) = parsed?;
            let format = archive::sniff(match &source {
                Source::Bytes(bytes) => bytes,
                Source::File(_) => unreachable!("open_bytes only receives bytes"),
            });
            let id = mint(&ids);
            reg.borrow_mut().insert(
                id,
                ArchiveHandle {
                    source,
                    format,
                    limits,
                },
            );
            Ok(open_value(id, format))
        })
    }))?;

    let (reg, ids, async_fs, sync_fs) = (
        archives.clone(),
        next_archive.clone(),
        fs.clone(),
        sync_fs.clone(),
    );
    engine.register_op(
        OpDecl::r#async("archive_open_file", move |args| {
            let parsed: std::result::Result<(String, Limits), OpError> = (|| {
                let path = string_arg(&args, 0, "path")?;
                let limits = parse_limits(args.get(1))?;
                Ok((path, limits))
            })();
            let (reg, ids) = (reg.clone(), ids.clone());
            let (async_fs, sync_fs) = (async_fs.clone(), sync_fs.clone());
            Box::pin(async move {
                let (path, limits) = parsed?;
                let source = match sync_fs {
                    Some(sync_fs) => Source::file(sync_fs, path).map_err(map_err)?,
                    None => {
                        let fs = require(&async_fs)?;
                        Source::bytes(fs.read(path).await.map_err(map_err)?)
                    }
                };
                let format = archive::sniff(&source.read_prefix(8).map_err(op_error)?);
                let id = mint(&ids);
                reg.borrow_mut().insert(
                    id,
                    ArchiveHandle {
                        source,
                        format,
                        limits,
                    },
                );
                Ok(open_value(id, format))
            })
        })
        .requires(Capability::FileRead)
        .target_arg(0),
    )?;

    let (reg, s) = (archives.clone(), spawner.clone());
    engine.register_op(OpDecl::r#async("archive_source_bytes", move |args| {
        let parsed: std::result::Result<Source, OpError> = (|| {
            let id = number_arg(&args, 0, "handle")?;
            Ok(lookup(&reg, id)?.source.clone())
        })();
        let spawner = s.clone();
        Box::pin(async move {
            let source = parsed?;
            let work = source.clone();
            let bytes = offload(spawner, move || archive::source_bytes(&work))
                .await?
                .map_err(op_error)?;
            Ok(Value::Bytes(bytes))
        })
    }))?;

    let (reg, s) = (archives.clone(), spawner.clone());
    engine.register_op(OpDecl::r#async("archive_read_selected", move |args| {
        let parsed: std::result::Result<(Source, Format, Limits, BTreeSet<String>), OpError> =
            (|| {
                let id = number_arg(&args, 0, "handle")?;
                let paths = paths_arg(&args, 1)?;
                let handle = lookup(&reg, id)?;
                Ok((handle.source, handle.format, handle.limits, paths))
            })();
        let spawner = s.clone();
        Box::pin(async move {
            let (source, format, limits, paths) = parsed?;
            let work = source.clone();
            let entries = offload(spawner, move || {
                archive::read_selected_source(&work, format, &paths, &limits)
            })
            .await?
            .map_err(op_error)?;
            Ok(Value::Array(
                entries
                    .into_iter()
                    .map(|entry| {
                        Value::Object(vec![
                            ("path".to_string(), Value::String(entry.meta.path)),
                            ("data".to_string(), Value::Bytes(entry.data)),
                        ])
                    })
                    .collect(),
            ))
        })
    }))?;

    let (reg, stream_registry, ids) = (archives.clone(), streams.clone(), next_stream.clone());
    engine.register_op(OpDecl::sync("archive_stream_open", move |args| {
        let id = number_arg(&args, 0, "handle")?;
        let path = string_arg(&args, 1, "entry")?;
        let handle = lookup(&reg, id)?;
        let stream_id = mint(&ids);
        let (tx, rx) = mpsc::channel(2);
        archive::spawn_entry_stream(
            handle.source.clone(),
            handle.format,
            path,
            handle.limits,
            tx,
        );
        stream_registry
            .borrow_mut()
            .insert(stream_id, StreamHandle { rx });
        Ok(Value::Number(stream_id as f64))
    }))?;

    let stream_next_registry = streams.clone();
    engine.register_op(OpDecl::r#async("archive_stream_next", move |args| {
        let parsed: std::result::Result<(u64, StreamHandle), OpError> = (|| {
            let id = number_arg(&args, 0, "stream")?;
            let state = stream_next_registry
                .borrow_mut()
                .remove(&id)
                .ok_or_else(|| {
                    OpError::new(
                        ExceptionClass::TypeError,
                        "this archive stream has already ended",
                    )
                })?;
            Ok((id, state))
        })();
        let stream_next_registry = stream_next_registry.clone();
        Box::pin(async move {
            let (id, mut state) = parsed?;
            let event = state.rx.next().await;
            match event {
                Some(StreamEvent::Data(data)) => {
                    stream_next_registry.borrow_mut().insert(id, state);
                    Ok(Value::Object(vec![
                        ("data".to_string(), Value::Bytes(data)),
                        ("done".to_string(), Value::Bool(false)),
                    ]))
                }
                Some(StreamEvent::Done) => Ok(Value::Object(vec![
                    ("data".to_string(), Value::Bytes(Vec::new())),
                    ("done".to_string(), Value::Bool(true)),
                ])),
                Some(StreamEvent::Error(error)) => Err(op_error(error)),
                None => Err(OpError::new(
                    ExceptionClass::Error,
                    "the archive stream ended unexpectedly",
                )),
            }
        })
    }))?;

    let streams = streams.clone();
    engine.register_op(OpDecl::sync("archive_stream_close", move |args| {
        let id = number_arg(&args, 0, "stream")?;
        streams.borrow_mut().remove(&id);
        Ok(Value::Undefined)
    }))?;

    let (reg, s) = (archives.clone(), spawner.clone());
    engine.register_op(OpDecl::r#async("archive_entries", move |args| {
        let parsed: std::result::Result<(Source, Limits), OpError> = (|| {
            let id = number_arg(&args, 0, "handle")?;
            let handle = lookup(&reg, id)?;
            Ok((handle.source, handle.limits))
        })();
        let spawner = s.clone();
        Box::pin(async move {
            let (source, limits) = parsed?;
            let work = source.clone();
            let listed = offload(spawner, move || archive::list_source(&work, &limits)).await?;
            let (_, entries) = listed.map_err(op_error)?;
            Ok(Value::Array(entries.iter().map(meta_value).collect()))
        })
    }))?;

    let (reg, meta_registry, ids) = (archives.clone(), metas.clone(), next_meta.clone());
    engine.register_op(OpDecl::sync("archive_entries_open", move |args| {
        let id = number_arg(&args, 0, "handle")?;
        let handle = lookup(&reg, id)?;
        let stream_id = mint(&ids);
        let (tx, rx) = mpsc::channel(2);
        archive::spawn_meta_stream(handle.source.clone(), handle.format, handle.limits, tx);
        meta_registry
            .borrow_mut()
            .insert(stream_id, MetaHandle { rx });
        Ok(Value::Number(stream_id as f64))
    }))?;

    let meta_next_registry = metas.clone();
    engine.register_op(OpDecl::r#async("archive_entries_next", move |args| {
        let parsed: std::result::Result<(u64, MetaHandle), OpError> = (|| {
            let id = number_arg(&args, 0, "stream")?;
            let state = meta_next_registry.borrow_mut().remove(&id).ok_or_else(|| {
                OpError::new(
                    ExceptionClass::TypeError,
                    "this archive listing has already ended",
                )
            })?;
            Ok((id, state))
        })();
        let meta_next_registry = meta_next_registry.clone();
        Box::pin(async move {
            let (id, mut state) = parsed?;
            match state.rx.next().await {
                Some(MetaEvent::Entry(meta)) => {
                    meta_next_registry.borrow_mut().insert(id, state);
                    Ok(Value::Object(vec![
                        ("entry".to_string(), meta_value(&meta)),
                        ("done".to_string(), Value::Bool(false)),
                    ]))
                }
                Some(MetaEvent::Done) => Ok(Value::Object(vec![
                    ("entry".to_string(), Value::Null),
                    ("done".to_string(), Value::Bool(true)),
                ])),
                Some(MetaEvent::Error(error)) => Err(op_error(error)),
                None => Err(OpError::new(
                    ExceptionClass::Error,
                    "the archive listing ended unexpectedly",
                )),
            }
        })
    }))?;

    let metas = metas.clone();
    engine.register_op(OpDecl::sync("archive_entries_close", move |args| {
        let id = number_arg(&args, 0, "stream")?;
        metas.borrow_mut().remove(&id);
        Ok(Value::Undefined)
    }))?;

    let reg = archives.clone();
    engine.register_op(OpDecl::sync("archive_close", move |args| {
        let id = number_arg(&args, 0, "handle")?;
        reg.borrow_mut().remove(&id);
        Ok(Value::Undefined)
    }))?;

    let (reg, s, f) = (archives.clone(), spawner, fs);
    engine.register_op(
        OpDecl::r#async("archive_extract", move |args| {
            let parsed: std::result::Result<(Source, String, Limits), OpError> = (|| {
                let id = number_arg(&args, 0, "handle")?;
                let directory = string_arg(&args, 1, "destination")?;
                let handle = lookup(&reg, id)?;
                let limits = match args.get(2) {
                    None | Some(Value::Undefined) => handle.limits,
                    Some(value) => parse_limits(Some(value))?,
                };
                Ok((handle.source, directory, limits))
            })();
            let (spawner, fs) = (s.clone(), f.clone());
            Box::pin(async move {
                let (source, directory, limits) = parsed?;
                let work = source.clone();
                let listed = offload(spawner.clone(), move || {
                    archive::list_source(&work, &limits)
                })
                .await?;
                let (format, metas) = listed.map_err(op_error)?;

                let fs = require(&fs)?;

                for path in extraction_paths(&directory, &metas) {
                    ensure_no_symlink(&fs, &path).await?;
                }
                let destination_existed = directory.is_empty()
                    || directory == "."
                    || fs.exists(directory.clone()).await.map_err(map_err)?;
                if !directory.is_empty() && directory != "." {
                    fs.mkdir(directory.clone(), true).await.map_err(map_err)?;
                }
                let stage = match fs
                    .make_temp_dir(directory.clone(), ".esrun-archive-".to_string())
                    .await
                    .map_err(map_err)
                {
                    Ok(stage) => stage,
                    Err(error) => {
                        if !destination_existed && !directory.is_empty() && directory != "." {
                            let _ = fs.remove(directory, false).await;
                        }
                        return Err(error);
                    }
                };

                let staged = stage_extract(&fs, source, format, limits, &stage).await;
                let result = match staged {
                    Ok(metas) => {
                        async {
                            // Check the paths from the bytes actually staged,
                            // since a file source can be changed in place.
                            for path in extraction_paths(&directory, &metas) {
                                ensure_no_symlink(&fs, &path).await?;
                            }
                            commit_stage(&fs, &stage, &directory, &metas).await
                        }
                        .await
                    }
                    Err(error) => Err(error),
                };

                let cleanup = fs.remove(stage, true).await.map_err(map_err);
                if result.is_err()
                    && !destination_existed
                    && !directory.is_empty()
                    && directory != "."
                {
                    let _ = fs.remove(directory, false).await;
                }
                cleanup?;
                result
            })
        })
        .requires(Capability::FileWrite)
        .target_arg(1),
    )?;

    Ok(())
}

struct CurrentEntry {
    path: String,
    directory: bool,
    wrote: bool,
}

/// Decodes an archive into a private directory. Nothing under the requested
/// destination is changed until the codec has consumed and validated every
/// entry, including ZIP checksums and the gzip trailer.
async fn stage_extract(
    fs: &Arc<dyn FileSystem>,
    source: Source,
    format: Format,
    limits: Limits,
    stage: &str,
) -> std::result::Result<Vec<EntryMeta>, OpError> {
    let (tx, mut rx) = mpsc::channel(2);
    archive::spawn_extract(source, format, limits, tx);

    let mut parents = HashSet::new();
    let mut metas = Vec::new();
    let mut current: Option<CurrentEntry> = None;
    while let Some(event) = rx.next().await {
        match event {
            ExtractEvent::Entry(meta) => {
                let path = join(stage, &meta.path);
                metas.push(meta.clone());
                if meta.directory {
                    fs.mkdir(path.clone(), true).await.map_err(map_err)?;
                    current = Some(CurrentEntry {
                        path,
                        directory: true,
                        wrote: false,
                    });
                } else {
                    if let Some((parent, _)) = meta.path.rsplit_once('/') {
                        let parent = join(stage, parent);
                        if parents.insert(parent.clone()) {
                            fs.mkdir(parent, true).await.map_err(map_err)?;
                        }
                    }
                    let wrote = if meta.size == 0 {
                        fs.write(path.clone(), Vec::new(), false)
                            .await
                            .map_err(map_err)?;
                        true
                    } else {
                        false
                    };
                    current = Some(CurrentEntry {
                        path,
                        directory: false,
                        wrote,
                    });
                }
            }
            ExtractEvent::Data(data) => {
                if let Some(entry) = &mut current
                    && !entry.directory
                {
                    fs.write(entry.path.clone(), data, entry.wrote)
                        .await
                        .map_err(map_err)?;
                    entry.wrote = true;
                }
            }
            ExtractEvent::EndEntry => {
                current = None;
            }
            ExtractEvent::Done => break,
            ExtractEvent::Error(error) => return Err(op_error(error)),
        }
    }
    if current.is_some() {
        return Err(OpError::new(
            ExceptionClass::Error,
            "archive extraction ended in the middle of an entry",
        ));
    }
    Ok(metas)
}

/// Copies staged files into the destination after decoding has completed.
async fn commit_stage(
    fs: &Arc<dyn FileSystem>,
    stage: &str,
    destination: &str,
    metas: &[EntryMeta],
) -> std::result::Result<Value, OpError> {
    let mut files = 0_u64;
    let mut directories = 0_u64;
    let mut parents = HashSet::new();
    for meta in metas {
        let target = join(destination, &meta.path);
        if meta.directory {
            fs.mkdir(target, true).await.map_err(map_err)?;
            directories += 1;
        } else {
            if let Some((parent, _)) = meta.path.rsplit_once('/') {
                let parent = join(destination, parent);
                if parents.insert(parent.clone()) {
                    fs.mkdir(parent, true).await.map_err(map_err)?;
                }
            }
            fs.copy(join(stage, &meta.path), target)
                .await
                .map_err(map_err)?;
            files += 1;
        }
    }
    Ok(Value::Object(vec![
        ("files".to_string(), Value::Number(files as f64)),
        ("directories".to_string(), Value::Number(directories as f64)),
    ]))
}

fn mint(ids: &Rc<RefCell<u64>>) -> u64 {
    let mut id = ids.borrow_mut();
    *id += 1;
    *id
}

fn lookup(
    archives: &Rc<RefCell<HashMap<u64, ArchiveHandle>>>,
    id: u64,
) -> std::result::Result<ArchiveHandle, OpError> {
    let archives = archives.borrow();
    let handle = archives.get(&id).ok_or_else(|| {
        OpError::new(
            ExceptionClass::TypeError,
            "this archive has already been closed",
        )
    })?;
    Ok(ArchiveHandle {
        source: handle.source.clone(),
        format: handle.format,
        limits: handle.limits,
    })
}

fn open_value(id: u64, format: Format) -> Value {
    Value::Object(vec![
        ("handle".to_string(), Value::Number(id as f64)),
        (
            "format".to_string(),
            Value::String(format.as_str().to_string()),
        ),
    ])
}

fn meta_value(meta: &EntryMeta) -> Value {
    Value::Object(vec![
        ("path".to_string(), Value::String(meta.path.clone())),
        ("directory".to_string(), Value::Bool(meta.directory)),
        ("size".to_string(), Value::Number(meta.size as f64)),
        (
            "lastModified".to_string(),
            match meta.modified_ms {
                Some(ms) => Value::Number(ms as f64),
                None => Value::Null,
            },
        ),
    ])
}

/// Every path under `directory` that an extraction could write or write
/// through, including the destination itself and each file. Reading through an
/// existing symlink here is what the write would do later, so it is refused
/// now; a symlink at a file's own path must not be followed either.
fn extraction_paths(directory: &str, metas: &[EntryMeta]) -> Vec<String> {
    let mut paths = BTreeSet::new();
    if !directory.is_empty() && directory != "." {
        paths.insert(directory.to_string());
    }
    for meta in metas {
        let components: Vec<&str> = meta.path.split('/').collect();
        for count in 1..=components.len() {
            paths.insert(join(directory, &components[..count].join("/")));
        }
    }
    paths.into_iter().collect()
}

async fn ensure_no_symlink(
    fs: &Arc<dyn FileSystem>,
    path: &str,
) -> std::result::Result<(), OpError> {
    if fs.read_link(path.to_string()).await.is_ok() {
        return Err(OpError::new(
            ExceptionClass::Error,
            format!("the extraction destination {path:?} is a symbolic link"),
        )
        .with_code(es_runtime_common::ErrorCode::ArchiveUnsafePath));
    }
    Ok(())
}

/// Joins an archive-relative path onto the extraction folder without ever
/// letting the result escape it. `path` is already validated by the codec, so
/// it is relative and has no `..`; this only handles the folder's own form.
fn join(directory: &str, path: &str) -> String {
    if directory.is_empty() || directory == "." {
        path.to_string()
    } else if directory.ends_with('/') {
        format!("{directory}{path}")
    } else {
        format!("{directory}/{path}")
    }
}

// ---- arguments --------------------------------------------------------------

fn bytes_arg(args: &[Value], index: usize, what: &str) -> std::result::Result<Vec<u8>, OpError> {
    args.get(index)
        .and_then(Value::as_bytes)
        .map(<[u8]>::to_vec)
        .ok_or_else(|| OpError::type_error(format!("an archive {what} must be a Uint8Array")))
}

fn string_arg(args: &[Value], index: usize, what: &str) -> std::result::Result<String, OpError> {
    args.get(index)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| OpError::type_error(format!("an archive {what} must be a string")))
}

fn number_arg(args: &[Value], index: usize, what: &str) -> std::result::Result<u64, OpError> {
    match args.get(index) {
        Some(Value::Number(number)) if number.is_finite() && *number >= 0.0 => Ok(*number as u64),
        _ => Err(OpError::type_error(format!(
            "an archive {what} must be a number"
        ))),
    }
}

fn paths_arg(args: &[Value], index: usize) -> std::result::Result<BTreeSet<String>, OpError> {
    let Some(Value::Array(items)) = args.get(index) else {
        return Err(OpError::type_error(
            "the selected archive entries must be an array",
        ));
    };
    items
        .iter()
        .map(|item| {
            item.as_str().map(str::to_string).ok_or_else(|| {
                OpError::type_error("a selected archive entry must be a path string")
            })
        })
        .collect()
}

fn parse_format(value: Option<&Value>) -> std::result::Result<Format, OpError> {
    match value.and_then(Value::as_str).unwrap_or("tar") {
        "tar" => Ok(Format::Tar),
        "zip" => Ok(Format::Zip),
        other => Err(OpError::type_error(format!(
            "unknown archive format {other:?}; expected \"tar\" or \"zip\""
        ))),
    }
}

fn parse_compression(value: Option<&Value>) -> std::result::Result<Compression, OpError> {
    match value.and_then(Value::as_str).unwrap_or("none") {
        "none" => Ok(Compression::None),
        "gzip" => Ok(Compression::Gzip),
        "deflate" => Ok(Compression::Deflate),
        other => Err(OpError::type_error(format!(
            "unknown archive compression {other:?}; expected \"none\", \"gzip\" or \"deflate\""
        ))),
    }
}

fn parse_entries(value: Option<&mut Value>) -> std::result::Result<Vec<InputEntry>, OpError> {
    let Some(Value::Array(items)) = value else {
        return Err(OpError::type_error("archive entries must be an array"));
    };
    items
        .iter()
        .map(|item| {
            let Value::Object(fields) = item else {
                return Err(OpError::type_error("an archive entry must be an object"));
            };
            let mut path = None;
            let mut data = None;
            let mut modified = None;
            for (name, value) in fields {
                match name.as_str() {
                    "path" => {
                        path = value.as_str().map(str::to_string);
                    }
                    "data" => match value {
                        Value::Null => {}
                        Value::Bytes(bytes) => data = Some(bytes.clone()),
                        _ => {
                            return Err(OpError::type_error(
                                "an archive entry's data must be a Uint8Array",
                            ));
                        }
                    },
                    "lastModified" => match value {
                        Value::Null => {}
                        Value::Number(ms) if ms.is_finite() && *ms >= 0.0 => {
                            modified = Some(*ms as u64);
                        }
                        _ => {
                            return Err(OpError::type_error(
                                "an archive entry's lastModified must be a non-negative number",
                            ));
                        }
                    },
                    _ => {}
                }
            }
            let Some(path) = path else {
                return Err(OpError::type_error(
                    "an archive entry's path must be a string",
                ));
            };
            Ok(InputEntry {
                path,
                data,
                modified_ms: modified,
            })
        })
        .collect()
}

fn parse_limits(value: Option<&Value>) -> std::result::Result<Limits, OpError> {
    let mut limits = Limits::default();
    let Some(Value::Object(fields)) = value else {
        if value.is_some_and(|value| !matches!(value, Value::Undefined)) {
            return Err(OpError::type_error("archive options must be an object"));
        }
        return Ok(limits);
    };
    for (name, value) in fields {
        match name.as_str() {
            "maxEntries" => {
                let number = number_value(value, "maxEntries")?;
                limits.max_entries = number as usize;
            }
            "maxBytes" => {
                limits.max_bytes = number_value(value, "maxBytes")?;
            }
            _ => {}
        }
    }
    Ok(limits)
}

fn number_value(value: &Value, what: &str) -> std::result::Result<u64, OpError> {
    match value {
        Value::Number(number) if number.is_finite() && *number >= 0.0 => {
            Ok(number.min(u64::MAX as f64) as u64)
        }
        _ => Err(OpError::type_error(format!(
            "the archive {what} limit must be a non-negative number"
        ))),
    }
}

fn require(fs: &Option<Arc<dyn FileSystem>>) -> std::result::Result<Arc<dyn FileSystem>, OpError> {
    fs.clone().ok_or_else(|| {
        OpError::new(
            ExceptionClass::Error,
            "filesystem is unavailable (no FileSystem provider configured)",
        )
        .with_code(es_runtime_common::ErrorCode::ProviderUnavailable)
    })
}

fn map_err(error: ProviderError) -> OpError {
    OpError::new(error.exception_class(), error.exception_message()).with_code_opt(error.code())
}

fn op_error(error: archive::Error) -> OpError {
    OpError::new(error.class(), error.to_string()).with_code_opt(error.code())
}

/// Runs `work` on the spawner, or inline without one.
async fn offload<T: Send + 'static>(
    spawner: Option<Arc<dyn TaskSpawner>>,
    work: impl FnOnce() -> T + Send + 'static,
) -> std::result::Result<T, OpError> {
    let Some(spawner) = spawner else {
        return Ok(work());
    };
    let (tx, rx) = futures_channel::oneshot::channel();
    spawner
        .spawn_blocking(Box::new(move || {
            let _ = tx.send(work());
        }))
        .await;
    rx.await
        .map_err(|_| OpError::new(ExceptionClass::Error, "the archive work did not complete"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(value: &str) -> Value {
        Value::String(value.to_string())
    }

    #[test]
    fn joins_relative_paths() {
        assert_eq!(join("out", "a/b"), "out/a/b");
        assert_eq!(join("out/", "a/b"), "out/a/b");
        assert_eq!(join(".", "a/b"), "a/b");
        assert_eq!(join("", "a/b"), "a/b");
    }

    #[test]
    fn extraction_paths_cover_every_parent() {
        let metas = vec![
            EntryMeta {
                path: "a/b.txt".to_string(),
                directory: false,
                size: 1,
                modified_ms: None,
            },
            EntryMeta {
                path: "c/d".to_string(),
                directory: true,
                size: 0,
                modified_ms: None,
            },
        ];
        assert_eq!(
            extraction_paths("out", &metas),
            ["out", "out/a", "out/a/b.txt", "out/c", "out/c/d"]
        );
    }

    #[test]
    fn limits_default_and_parse() {
        assert_eq!(parse_limits(None).unwrap().max_entries, 100_000);
        let limits = parse_limits(Some(&Value::Object(vec![
            ("maxEntries".to_string(), Value::Number(3.0)),
            ("maxBytes".to_string(), Value::Number(10.0)),
        ])))
        .unwrap();
        assert_eq!(limits.max_entries, 3);
        assert_eq!(limits.max_bytes, 10);
        assert!(parse_limits(Some(&s("nope"))).is_err());
    }

    #[test]
    fn formats_are_validated() {
        assert_eq!(parse_format(Some(&s("tar"))).unwrap(), Format::Tar);
        assert_eq!(parse_format(Some(&s("zip"))).unwrap(), Format::Zip);
        assert_eq!(parse_format(None).unwrap(), Format::Tar);
        assert!(parse_format(Some(&s("rar"))).is_err());
    }

    #[test]
    fn compression_is_validated() {
        assert_eq!(
            parse_compression(Some(&s("gzip"))).unwrap(),
            Compression::Gzip
        );
        assert_eq!(
            parse_compression(Some(&s("deflate"))).unwrap(),
            Compression::Deflate
        );
        assert_eq!(
            parse_compression(Some(&s("none"))).unwrap(),
            Compression::None
        );
        assert!(parse_compression(Some(&s("brotli"))).is_err());
    }
}
