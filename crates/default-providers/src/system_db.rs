//! The embedded SQL engine behind `runtime:db`'s `sqlite:` scheme — an
//! [`EmbeddedDb`] over `turso_core` (DECISIONS.md D56).
//!
//! Two things make this more than a wrapper.
//!
//! **The engine gets a jailed VFS, not the filesystem.** `turso_core` opens
//! more files than it is given: a write-ahead log, a shared-memory index, and
//! whatever a future format adds beside them. Handing it the path and letting
//! it open the rest would put those files outside the root jail (D25) and the
//! `--allow-read`/`--allow-write` scopes (D38) — inside the directory, but
//! reached by a route nothing checked. So the engine's `IO` is
//! [`JailedVfs`], which resolves **every** open through the same
//! [`SystemFileSystem`](crate::SystemFileSystem) that backs `runtime:fs`. No filename has to be
//! guessed, because the engine has to ask.
//!
//! **Rows leave as bytes.** [`fetch`](EmbeddedDb::fetch) encodes a run of rows
//! into one buffer in the layout `EmbeddedDb` documents — Postgres's `DataRow`
//! body — so the JS decoder written for the wire protocols is the decoder here
//! too. Nothing is marshaled per value, and a batch stops on a **byte** budget,
//! so a table of wide rows costs the same per fetch as a table of narrow ones.
//!
//! The engine's work is CPU and disk with no reactor of its own — `step()`
//! drives its own I/O to completion — so every call runs on
//! `tokio::task::spawn_blocking`, the same place the rest of this crate puts
//! work that would otherwise sit on the loop.

use std::collections::HashMap;
use std::num::NonZero;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use es_runtime_common::ErrorCode;
use es_runtime_providers::{
    BoxFuture, DbColumn, DbCursor, DbParams, DbValue, EmbeddedDb, EmbeddedDbOptions, ExecuteResult,
    ProviderError, RowBatch,
};
use turso_core::types::Text;
use turso_core::{
    Clock, Completion, Connection, Database, DatabaseOpts, EncryptionKey, EncryptionOpts, File, IO,
    LimboError, MemoryIO, MonotonicInstant, NonNan, Numeric, OpenFlags, OpenOptions, PlatformIO,
    SqliteDialect, Statement, StepResult, TempStore, Value as TursoValue, WallClockInstant,
};

use crate::path_allowlist::Access;
use crate::system_fs::SystemFileSystem;

/// The per-value type tags of the row encoding (see [`EmbeddedDb`]). Carried
/// because SQLite declares types on columns and stores them on values: a column
/// declared `INTEGER` may hand back text in one row and null in the next, so
/// the column descriptor cannot answer what a given value is.
const TAG_INTEGER: u8 = 1;
const TAG_REAL: u8 = 2;
const TAG_TEXT: u8 = 3;
const TAG_BLOB: u8 = 4;

/// A `turso_core` [`IO`] whose every path goes through the runtime's filesystem
/// jail before the real backend sees it.
///
/// Everything except opening and removing is delegated untouched: the platform
/// backend's completion model, its WAL coordination, and its syscalls are what
/// make the engine correct, and reimplementing any of it here would be a way to
/// get it wrong. What is *not* delegated is the choice of which file — that is
/// the entire reason this type exists.
struct JailedVfs {
    inner: PlatformIO,
    fs: Arc<SystemFileSystem>,
    /// Which half of the filesystem grant an open is judged against. A
    /// read-only database resolves under the read roots and a writable one
    /// under the write roots, so `--allow-read=./data` alone opens a database
    /// for reading and refuses to open it for writing — the same split
    /// `runtime:fs` applies to a file.
    access: Access,
}

impl JailedVfs {
    fn resolve(&self, path: &str, access: Access) -> turso_core::Result<String> {
        let real = self
            .fs
            .jailed(path, access)
            .map_err(|e| LimboError::InvalidArgument(provider_message(&e)))?;
        real.to_str()
            .map(str::to_string)
            .ok_or_else(|| LimboError::InvalidArgument(format!("path is not UTF-8: {path}")))
    }
}

impl Clock for JailedVfs {
    fn current_time_monotonic(&self) -> MonotonicInstant {
        self.inner.current_time_monotonic()
    }

    fn current_time_wall_clock(&self) -> WallClockInstant {
        self.inner.current_time_wall_clock()
    }
}

impl IO for JailedVfs {
    fn open_file(
        &self,
        path: &str,
        flags: OpenFlags,
        direct: bool,
    ) -> turso_core::Result<Arc<dyn File>> {
        let real = self.resolve(path, self.access)?;
        self.inner.open_file(&real, flags, direct)
    }

    fn remove_file(&self, path: &str) -> turso_core::Result<()> {
        // Removal is a mutation whatever the database was opened as, so it is
        // judged against the write scope even on a read-only connection —
        // which then simply cannot reach it.
        let real = self.resolve(path, Access::Write)?;
        self.inner.remove_file(&real)
    }

    fn supports_shared_wal_coordination(&self) -> bool {
        self.inner.supports_shared_wal_coordination()
    }

    fn step(&self) -> turso_core::Result<()> {
        self.inner.step()
    }

    fn cancel(&self, c: &[Completion]) -> turso_core::Result<()> {
        self.inner.cancel(c)
    }

    fn drain_completions(&self, completions: &[Completion]) -> turso_core::Result<()> {
        self.inner.drain_completions(completions)
    }

    fn wait_for_completion(&self, c: Completion) -> turso_core::Result<()> {
        self.inner.wait_for_completion(c)
    }
}

/// One open database: the connection, and the VFS and handle keeping it alive.
struct ConnEntry {
    conn: Arc<Connection>,
    _db: Arc<Database>,
    _io: Arc<dyn IO>,
}

/// One open result set, positioned but not yet read.
struct CursorEntry {
    stmt: Statement,
    columns: usize,
    /// Set once the statement reports `Done`, so a fetch after exhaustion
    /// answers "no more rows" instead of stepping a finished program.
    done: bool,
}

/// An [`EmbeddedDb`] over `turso_core`, jailed to the filesystem provider it is
/// built with.
pub struct SystemEmbeddedDb {
    fs: Arc<SystemFileSystem>,
    conns: Arc<Mutex<HashMap<u64, Arc<ConnEntry>>>>,
    cursors: Arc<Mutex<HashMap<u64, Arc<Mutex<CursorEntry>>>>>,
    next_id: Arc<AtomicU64>,
}

impl SystemEmbeddedDb {
    /// Builds an engine that resolves every path through `fs`.
    ///
    /// Taking the *same* [`SystemFileSystem`] the runtime gave `runtime:fs`, rather
    /// than a root of its own, is what makes `--allow-read` and `--allow-write`
    /// mean the same thing for a database as for a file. A second jail
    /// configured separately would be a second policy to keep in step.
    pub fn new(fs: Arc<SystemFileSystem>) -> Self {
        Self {
            fs,
            conns: Arc::new(Mutex::new(HashMap::new())),
            cursors: Arc::new(Mutex::new(HashMap::new())),
            next_id: Arc::new(AtomicU64::new(1)),
        }
    }

    fn id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    fn conn(&self, id: u64) -> Result<Arc<ConnEntry>, ProviderError> {
        self.conns
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .ok_or_else(|| coded(ErrorCode::NotFound, "the database connection is closed"))
    }

    fn cursor(&self, id: u64) -> Result<Arc<Mutex<CursorEntry>>, ProviderError> {
        self.cursors
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .ok_or_else(|| coded(ErrorCode::NotFound, "the cursor is closed"))
    }
}

impl EmbeddedDb for SystemEmbeddedDb {
    fn open(&self, path: String, opts: EmbeddedDbOptions) -> BoxFuture<Result<u64, ProviderError>> {
        let fs = self.fs.clone();
        let conns = self.conns.clone();
        let id = self.id();
        Box::pin(async move {
            let entry = blocking(move || {
                if opts.in_memory {
                    return open_in_memory(&opts);
                }
                let access = if opts.read_only {
                    Access::Read
                } else {
                    Access::Write
                };
                // Resolved here as well as inside the VFS, because the engine
                // does not put every path through its own `IO`: it stats the
                // database's filesystem directly to decide how to coordinate a
                // shared WAL. A relative path would be resolved against the
                // process's working directory for that one call — which is not
                // where the jail put it. Handing over an absolute path makes
                // the two agree, and the VFS still authorizes every open,
                // including the ones nobody named.
                let resolved = fs.jailed(&path, access)?;
                let path = resolved
                    .to_str()
                    .ok_or_else(|| other("the database path is not UTF-8"))?
                    .to_string();
                let io: Arc<dyn IO> = Arc::new(JailedVfs {
                    inner: PlatformIO {},
                    fs,
                    access,
                });
                let flags = if opts.read_only {
                    OpenFlags::ReadOnly
                } else {
                    OpenFlags::Create
                };
                let key = opts.hex_key.clone();
                let encryption = opts.hex_key.map(|hexkey| EncryptionOpts {
                    // The cipher is the caller's when given and a 256-bit AEAD
                    // otherwise. Defaulting rather than requiring it keeps the
                    // guest API `{ key }`, which is the whole option most
                    // callers want.
                    cipher: opts.cipher.unwrap_or_else(|| "aes256gcm".to_string()),
                    hexkey,
                });
                let mut open = OpenOptions::new(Arc::new(SqliteDialect)).flags(flags);
                if let Some(encryption) = encryption {
                    // The engine refuses an encrypted open unless the feature
                    // is switched on for the database as well as configured on
                    // it — upstream still calls encryption experimental, and
                    // says so through this flag. Turning it on exactly when a
                    // key was given keeps the guest API `{ key }` rather than
                    // `{ key, andAlsoMeanIt: true }`.
                    open = open
                        .encryption(encryption)
                        .db_opts(DatabaseOpts::default().with_encryption(true));
                }
                let db = Database::open(io.clone(), &path, open).map_err(engine_error)?;
                // The key is given twice on purpose. `OpenOptions::encryption`
                // tells the *database* which cipher its pages use; the
                // connection needs the key itself, because the pager that
                // writes a page lives there. Opening with only the first
                // succeeds and then writes plaintext pages into a database
                // marked encrypted — which fails on the next open, not this
                // one, with a decryption error for page 1.
                let conn = match &key {
                    Some(key) => db
                        .connect_with_encryption(Some(
                            EncryptionKey::from_hex_string(key).map_err(engine_error)?,
                        ))
                        .map_err(engine_error)?,
                    None => db.connect().map_err(engine_error)?,
                };
                confine_temp_storage(&conn);
                Ok(ConnEntry {
                    conn,
                    _db: db,
                    _io: io,
                })
            })
            .await?;
            conns.lock().unwrap().insert(id, Arc::new(entry));
            Ok(id)
        })
    }

    fn query(
        &self,
        db: u64,
        sql: String,
        params: DbParams,
    ) -> BoxFuture<Result<DbCursor, ProviderError>> {
        let entry = self.conn(db);
        let cursors = self.cursors.clone();
        let id = self.id();
        Box::pin(async move {
            let entry = entry?;
            let (columns, cursor) = blocking(move || {
                let stmt = prepare(&entry.conn, &sql, params)?;
                let columns = (0..stmt.num_columns())
                    .map(|i| DbColumn {
                        name: stmt.get_column_name(i).into_owned(),
                        // Left unset rather than guessed: the engine reports a
                        // column's declared type only where the schema has
                        // one, and a synthetic column (an expression, a
                        // function) has none to report.
                        decl_type: None,
                    })
                    .collect::<Vec<_>>();
                let n = columns.len();
                Ok((
                    columns,
                    CursorEntry {
                        stmt,
                        columns: n,
                        done: false,
                    },
                ))
            })
            .await?;
            cursors
                .lock()
                .unwrap()
                .insert(id, Arc::new(Mutex::new(cursor)));
            Ok(DbCursor { id, columns })
        })
    }

    fn fetch(&self, cursor: u64, max_bytes: usize) -> BoxFuture<Result<RowBatch, ProviderError>> {
        let entry = self.cursor(cursor);
        Box::pin(async move {
            let entry = entry?;
            blocking(move || {
                let mut cur = entry.lock().unwrap();
                if cur.done {
                    return Ok(RowBatch {
                        bytes: Vec::new(),
                        rows: 0,
                        done: true,
                    });
                }
                let columns = cur.columns;
                let mut bytes = Vec::with_capacity(max_bytes.min(64 * 1024));
                let mut rows = 0u32;
                loop {
                    match cur.stmt.step().map_err(engine_error)? {
                        StepResult::Row => {
                            let row = cur
                                .stmt
                                .row()
                                .ok_or_else(|| other("the engine reported a row and had none"))?;
                            encode_row(&mut bytes, row.get_values(), columns);
                            rows += 1;
                            // Checked *after* the row is written, so a row
                            // wider than the budget still crosses whole: the
                            // bound shapes batches, it does not truncate
                            // values.
                            if bytes.len() >= max_bytes {
                                break;
                            }
                        }
                        StepResult::Done => {
                            cur.done = true;
                            break;
                        }
                        StepResult::Busy => return Err(busy()),
                        StepResult::Interrupt => {
                            return Err(coded(ErrorCode::Cancelled, "the query was interrupted"));
                        }
                        // The engine is asking to be driven, not answering.
                        StepResult::IO | StepResult::Yield => continue,
                    }
                }
                let done = cur.done;
                Ok(RowBatch { bytes, rows, done })
            })
            .await
        })
    }

    fn close_cursor(&self, cursor: u64) -> BoxFuture<Result<(), ProviderError>> {
        let removed = self.cursors.lock().unwrap().remove(&cursor);
        Box::pin(async move {
            // Idempotent: a cursor abandoned before exhaustion is ordinary, and
            // so is closing one twice.
            if let Some(entry) = removed {
                blocking(move || {
                    let _ = entry.lock().unwrap().stmt.reset();
                    Ok(())
                })
                .await?;
            }
            Ok(())
        })
    }

    fn execute(
        &self,
        db: u64,
        sql: String,
        params: DbParams,
    ) -> BoxFuture<Result<ExecuteResult, ProviderError>> {
        let entry = self.conn(db);
        Box::pin(async move {
            let entry = entry?;
            blocking(move || {
                let mut stmt = prepare(&entry.conn, &sql, params)?;
                loop {
                    match stmt.step().map_err(engine_error)? {
                        // A statement run for its effect may still produce
                        // rows — `INSERT … RETURNING`, or a `SELECT` someone
                        // ran through `execute`. Stepping past them is what
                        // makes the effect happen; dropping them is what the
                        // caller asked for by not asking for rows.
                        StepResult::Row | StepResult::IO | StepResult::Yield => continue,
                        StepResult::Done => break,
                        StepResult::Busy => return Err(busy()),
                        StepResult::Interrupt => {
                            return Err(coded(
                                ErrorCode::Cancelled,
                                "the statement was interrupted",
                            ));
                        }
                    }
                }
                Ok(ExecuteResult {
                    changes: entry.conn.changes().max(0) as u64,
                    last_insert_rowid: match entry.conn.last_insert_rowid() {
                        0 => None,
                        id => Some(id),
                    },
                })
            })
            .await
        })
    }

    fn close(&self, db: u64) -> BoxFuture<Result<(), ProviderError>> {
        let removed = self.conns.lock().unwrap().remove(&db);
        Box::pin(async move {
            if let Some(entry) = removed {
                blocking(move || entry.conn.close().map_err(engine_error)).await?;
            }
            Ok(())
        })
    }
}

/// Opens a database that exists only in memory.
///
/// The engine's storage is decided by **which `IO` it is handed**, and by
/// nothing else — `Database::open` uses the one it is given, and the path→IO
/// mapping lives in a convenience constructor we cannot use because we supply
/// our own VFS. So handing it the jailed VFS with a `:memory:` path produces a
/// *file* called `:memory:`, while `is_in_memory_db()` cheerfully reports
/// true. The dispatch is ours to make, and this is where it is made.
///
/// Nothing here touches the filesystem, so nothing here consults the jail.
fn open_in_memory(opts: &EmbeddedDbOptions) -> Result<ConnEntry, ProviderError> {
    let io: Arc<dyn IO> = Arc::new(MemoryIO::new());
    let mut open = OpenOptions::new(Arc::new(SqliteDialect)).flags(OpenFlags::Create);
    if let Some(hexkey) = opts.hex_key.clone() {
        open = open
            .encryption(EncryptionOpts {
                cipher: opts
                    .cipher
                    .clone()
                    .unwrap_or_else(|| "aes256gcm".to_string()),
                hexkey,
            })
            .db_opts(DatabaseOpts::default().with_encryption(true));
    }
    // `:memory:` is still the path, because the engine reads it back through
    // `is_in_memory_db()` and bypasses its connection registry on it — two
    // behaviours that should agree with the storage rather than contradict it.
    let db = Database::open(io.clone(), ":memory:", open).map_err(engine_error)?;
    let conn = match &opts.hex_key {
        Some(key) => db
            .connect_with_encryption(Some(
                EncryptionKey::from_hex_string(key).map_err(engine_error)?,
            ))
            .map_err(engine_error)?,
        None => db.connect().map_err(engine_error)?,
    };
    confine_temp_storage(&conn);
    Ok(ConnEntry {
        conn,
        _db: db,
        _io: io,
    })
}

/// Points the engine's scratch space at memory rather than at the OS temp
/// directory.
///
/// Not a tuning choice — a jail one. Some statements (`DROP TABLE`, a large
/// sort) ask the engine for a temp file, and it takes one from
/// `tempfile::tempdir()`: `/tmp/.tmpXXXX/tursodb_temp_file`, which is outside
/// the root jail by construction. Our VFS refuses it, correctly, and the
/// statement fails. Redirecting the path into the jail would mean recognising
/// the engine's own temp naming and leaving scratch files in the user's
/// project; memory needs neither.
///
/// The cost is real and worth stating: work that would have spilled to disk is
/// now bounded by memory instead. That is the same bound `runtime:fs` already
/// puts on temp files by refusing to use the OS temp directory (D25), so the
/// two agree rather than one of them being an exception.
fn confine_temp_storage(conn: &Arc<Connection>) {
    conn.set_temp_store(TempStore::Memory);
}

/// Prepares `sql` on `conn` and binds `params`.
///
/// Named parameters are resolved by the engine rather than by rewriting the SQL
/// a layer up: the statement already knows where its names are, and finding
/// them anywhere else would mean parsing SQL twice.
fn prepare(
    conn: &Arc<Connection>,
    sql: &str,
    params: DbParams,
) -> Result<Statement, ProviderError> {
    let mut stmt = conn.prepare(sql).map_err(engine_error)?;
    for (i, value) in params.positional.into_iter().enumerate() {
        let index = NonZero::new(i + 1).expect("index is 1-based");
        stmt.bind_at(index, to_engine_value(value)?)
            .map_err(engine_error)?;
    }
    for (name, value) in params.named {
        let index = named_index(&stmt, &name).ok_or_else(|| {
            // Named rather than ignored: a misspelled parameter binds nothing
            // and the statement then runs against NULL, which is a wrong answer
            // rather than an error.
            other(&format!("the statement has no parameter named {name:?}"))
        })?;
        stmt.bind_at(index, to_engine_value(value)?)
            .map_err(engine_error)?;
    }
    Ok(stmt)
}

/// Finds a named parameter's slot.
///
/// The engine keeps a name as it was written, sigil and all, while the seam
/// takes names without one — a JS object's keys are `{ label: 1 }`, not
/// `{ ":label": 1 }`. SQLite spells the same parameter three ways, so each is
/// tried; a name given *with* a sigil is honoured first, so a caller who knows
/// the SQL's exact spelling is not second-guessed.
fn named_index(stmt: &Statement, name: &str) -> Option<NonZero<usize>> {
    let params = stmt.parameters();
    params.index(name).or_else(|| {
        [':', '@', '$']
            .iter()
            .find_map(|sigil| params.index(format!("{sigil}{name}")))
    })
}

fn to_engine_value(value: DbValue) -> Result<TursoValue, ProviderError> {
    Ok(match value {
        DbValue::Null => TursoValue::Null,
        DbValue::Integer(i) => TursoValue::Numeric(Numeric::Integer(i)),
        // NaN binds as NULL, which is SQLite's own answer for it: the storage
        // format has no NaN, and the alternative — refusing the bind — would
        // make `x / 0` in JS unrepresentable rather than merely unordered.
        DbValue::Real(f) => match NonNan::new(f) {
            Some(f) => TursoValue::Numeric(Numeric::Float(f)),
            None => TursoValue::Null,
        },
        DbValue::Text(t) => TursoValue::Text(Text::new(t)),
        // Fallible because the engine's blob allocation is fallible: a bind
        // large enough to fail should say so rather than abort the process.
        DbValue::Blob(b) => {
            TursoValue::from_slice(&b).map_err(|_| other("the parameter is too large to bind"))?
        }
    })
}

/// Appends one row in the `EmbeddedDb` layout: a back-filled length, the column
/// count, then each column as a length and a tagged payload.
fn encode_row<'a>(out: &mut Vec<u8>, values: impl Iterator<Item = &'a TursoValue>, columns: usize) {
    let start = out.len();
    out.extend_from_slice(&0i32.to_be_bytes()); // back-filled below
    out.extend_from_slice(&(columns as i16).to_be_bytes());
    for value in values {
        encode_value(out, value);
    }
    let len = (out.len() - start) as i32;
    out[start..start + 4].copy_from_slice(&len.to_be_bytes());
}

fn encode_value(out: &mut Vec<u8>, value: &TursoValue) {
    match value {
        // NULL is the absence of a payload, so it carries no tag: a length of
        // -1 is the whole of it, exactly as on the wire.
        TursoValue::Null => out.extend_from_slice(&(-1i32).to_be_bytes()),
        TursoValue::Numeric(Numeric::Integer(i)) => {
            out.extend_from_slice(&9i32.to_be_bytes());
            out.push(TAG_INTEGER);
            out.extend_from_slice(&i.to_be_bytes());
        }
        TursoValue::Numeric(Numeric::Float(f)) => {
            out.extend_from_slice(&9i32.to_be_bytes());
            out.push(TAG_REAL);
            out.extend_from_slice(&f64::from(*f).to_be_bytes());
        }
        TursoValue::Text(t) => {
            let bytes = t.as_str().as_bytes();
            out.extend_from_slice(&((bytes.len() + 1) as i32).to_be_bytes());
            out.push(TAG_TEXT);
            out.extend_from_slice(bytes);
        }
        TursoValue::Blob(b) => {
            let bytes = b.as_slice();
            out.extend_from_slice(&((bytes.len() + 1) as i32).to_be_bytes());
            out.push(TAG_BLOB);
            out.extend_from_slice(bytes);
        }
    }
}

/// Runs `work` off the loop and flattens the join failure.
///
/// The engine drives its own I/O inside `step()`, so a call occupies its thread
/// until it finishes; on the loop's thread that would stall every other agent
/// for the length of a query.
async fn blocking<T, F>(work: F) -> Result<T, ProviderError>
where
    F: FnOnce() -> Result<T, ProviderError> + Send + 'static,
    T: Send + 'static,
{
    match tokio::task::spawn_blocking(work).await {
        Ok(result) => result,
        Err(e) => Err(other(&format!("the database task failed: {e}"))),
    }
}

fn engine_error(e: LimboError) -> ProviderError {
    // Carried as prose rather than classified here. The engine's error type is
    // not a code table — a constraint violation and a type mismatch are both
    // `InvalidArgument` with a message — so the classification a driver needs
    // is done where the backend's own vocabulary is known, in the JS driver
    // (D56). Turning one imprecise classification into another at this seam
    // would only lose the message on the way.
    ProviderError::Other(e.to_string())
}

fn busy() -> ProviderError {
    coded(
        ErrorCode::TimedOut,
        "the database is locked by another writer",
    )
}

fn coded(code: ErrorCode, message: &str) -> ProviderError {
    ProviderError::Coded {
        code,
        message: message.to_string(),
    }
}

fn other(message: &str) -> ProviderError {
    ProviderError::Other(message.to_string())
}

/// The human half of a [`ProviderError`], for wrapping into an engine error on
/// the way *down* — the engine's own error type is the only shape its `IO` may
/// return, so a jail refusal has to travel as one and is unwrapped again above.
fn provider_message(e: &ProviderError) -> String {
    e.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PathAllowlist;

    /// A jailed engine rooted at a fresh directory.
    fn engine(name: &str) -> (std::path::PathBuf, SystemEmbeddedDb) {
        let root = std::env::temp_dir().join(format!("esrun-db-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let fs = Arc::new(SystemFileSystem::new(&root, &root));
        (root, SystemEmbeddedDb::new(fs))
    }

    fn opts() -> EmbeddedDbOptions {
        EmbeddedDbOptions::default()
    }

    async fn open(db: &SystemEmbeddedDb, path: &str) -> u64 {
        db.open(path.to_string(), opts()).await.unwrap()
    }

    async fn exec(db: &SystemEmbeddedDb, id: u64, sql: &str) -> ExecuteResult {
        db.execute(id, sql.to_string(), DbParams::default())
            .await
            .unwrap()
    }

    /// Collects every batch of a cursor, returning (bytes, row count).
    async fn drain(db: &SystemEmbeddedDb, cursor: u64, max_bytes: usize) -> (Vec<u8>, u32) {
        let mut bytes = Vec::new();
        let mut rows = 0;
        loop {
            let batch = db.fetch(cursor, max_bytes).await.unwrap();
            bytes.extend_from_slice(&batch.bytes);
            rows += batch.rows;
            if batch.done {
                return (bytes, rows);
            }
        }
    }

    #[tokio::test]
    async fn a_database_outside_the_jail_is_refused() {
        let (_root, db) = engine("escape");
        let err = db
            .open("../outside.db".to_string(), opts())
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("outside"),
            "expected a jail refusal naming the path, got: {err}"
        );
    }

    /// The engine opens files nobody asked for. If only the path the caller
    /// named were checked, the write-ahead log beside it would reach the disk
    /// through a route no scope had judged — the hole the jailed VFS exists to
    /// close.
    #[tokio::test]
    async fn every_file_the_engine_opens_goes_through_the_scope() {
        let root = std::env::temp_dir().join(format!("esrun-db-wal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::create_dir_all(root.join("other")).unwrap();
        let fs = Arc::new(
            SystemFileSystem::new(&root, &root)
                .with_read_allowlist(PathAllowlist::parse(["data"], &root).unwrap())
                .with_write_allowlist(PathAllowlist::parse(["data"], &root).unwrap()),
        );
        let db = SystemEmbeddedDb::new(fs);

        let id = open(&db, "data/app.db").await;
        exec(&db, id, "CREATE TABLE t (a INTEGER)").await;
        exec(&db, id, "INSERT INTO t VALUES (1)").await;
        db.close(id).await.unwrap();

        // The sidecars landed inside the scope, beside the database, rather
        // than anywhere the engine felt like putting them.
        let strays: Vec<_> = std::fs::read_dir(root.join("other"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert!(
            strays.is_empty(),
            "engine wrote outside the scope: {strays:?}"
        );

        // And a database the write scope does not cover cannot be opened for
        // writing at all.
        let err = db
            .open("other/nope.db".to_string(), opts())
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("is not an allowed path (write)"),
            "expected a scope refusal, got: {err}"
        );
    }

    #[tokio::test]
    async fn a_row_encodes_as_the_documented_layout() {
        let (_root, db) = engine("encode");
        let id = open(&db, "app.db").await;
        exec(
            &db,
            id,
            "CREATE TABLE t (i INTEGER, r REAL, s TEXT, b BLOB, n INTEGER)",
        )
        .await;
        exec(
            &db,
            id,
            "INSERT INTO t VALUES (7, 1.5, 'hi', x'0102', NULL)",
        )
        .await;

        let cursor = db
            .query(
                id,
                "SELECT i, r, s, b, n FROM t".to_string(),
                DbParams::default(),
            )
            .await
            .unwrap();
        assert_eq!(
            cursor
                .columns
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            ["i", "r", "s", "b", "n"]
        );
        let (bytes, rows) = drain(&db, cursor.id, 64 * 1024).await;
        assert_eq!(rows, 1);

        let mut expected = Vec::new();
        expected.extend_from_slice(&0i32.to_be_bytes()); // back-filled
        expected.extend_from_slice(&5i16.to_be_bytes());
        expected.extend_from_slice(&9i32.to_be_bytes());
        expected.push(TAG_INTEGER);
        expected.extend_from_slice(&7i64.to_be_bytes());
        expected.extend_from_slice(&9i32.to_be_bytes());
        expected.push(TAG_REAL);
        expected.extend_from_slice(&1.5f64.to_be_bytes());
        expected.extend_from_slice(&3i32.to_be_bytes());
        expected.push(TAG_TEXT);
        expected.extend_from_slice(b"hi");
        expected.extend_from_slice(&3i32.to_be_bytes());
        expected.push(TAG_BLOB);
        expected.extend_from_slice(&[1, 2]);
        expected.extend_from_slice(&(-1i32).to_be_bytes());
        let len = expected.len() as i32;
        expected[0..4].copy_from_slice(&len.to_be_bytes());

        assert_eq!(bytes, expected);
    }

    /// The batch bound is bytes, not rows — and it shapes batches rather than
    /// truncating them, so a row wider than the whole budget still crosses.
    #[tokio::test]
    async fn a_batch_is_bounded_by_bytes_and_never_splits_a_row() {
        let (_root, db) = engine("batching");
        let id = open(&db, "app.db").await;
        exec(&db, id, "CREATE TABLE t (s TEXT)").await;
        for _ in 0..8 {
            exec(&db, id, "INSERT INTO t VALUES (printf('%.*c', 100, 'x'))").await;
        }

        let cursor = db
            .query(id, "SELECT s FROM t".to_string(), DbParams::default())
            .await
            .unwrap();
        let first = db.fetch(cursor.id, 150).await.unwrap();
        assert_eq!(first.rows, 2, "a 150-byte budget holds two ~110-byte rows");
        assert!(!first.done);
        let (rest, more) = drain(&db, cursor.id, 150).await;
        assert_eq!(more, 6);
        assert!(!rest.is_empty());

        // A budget smaller than one row still yields that row, whole.
        let cursor = db
            .query(id, "SELECT s FROM t".to_string(), DbParams::default())
            .await
            .unwrap();
        let one = db.fetch(cursor.id, 1).await.unwrap();
        assert_eq!(one.rows, 1);
        assert!(one.bytes.len() > 100);
    }

    #[tokio::test]
    async fn parameters_bind_by_position_and_by_name() {
        let (_root, db) = engine("params");
        let id = open(&db, "app.db").await;
        exec(&db, id, "CREATE TABLE t (a INTEGER, b TEXT)").await;
        db.execute(
            id,
            "INSERT INTO t VALUES (?, :label)".to_string(),
            DbParams {
                positional: vec![DbValue::Integer(42)],
                named: vec![("label".to_string(), DbValue::Text("ok".to_string()))],
            },
        )
        .await
        .unwrap();

        let cursor = db
            .query(
                id,
                "SELECT b FROM t WHERE a = ?".to_string(),
                DbParams {
                    positional: vec![DbValue::Integer(42)],
                    named: Vec::new(),
                },
            )
            .await
            .unwrap();
        let (_bytes, rows) = drain(&db, cursor.id, 4096).await;
        assert_eq!(rows, 1);
    }

    /// A misspelled name binds nothing, and a statement run against an unbound
    /// parameter compares against NULL — a wrong answer rather than an error.
    #[tokio::test]
    async fn an_unknown_parameter_name_is_refused() {
        let (_root, db) = engine("bad-param");
        let id = open(&db, "app.db").await;
        exec(&db, id, "CREATE TABLE t (a INTEGER)").await;
        let err = db
            .execute(
                id,
                "INSERT INTO t VALUES (:value)".to_string(),
                DbParams {
                    positional: Vec::new(),
                    named: vec![("valeu".to_string(), DbValue::Integer(1))],
                },
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("valeu"), "got: {err}");
    }

    #[tokio::test]
    async fn a_read_only_database_cannot_be_written() {
        let (root, db) = engine("readonly");
        let id = open(&db, "app.db").await;
        exec(&db, id, "CREATE TABLE t (a INTEGER)").await;
        db.close(id).await.unwrap();
        assert!(root.join("app.db").exists());

        let ro = db
            .open(
                "app.db".to_string(),
                EmbeddedDbOptions {
                    read_only: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let err = db
            .execute(
                ro,
                "INSERT INTO t VALUES (1)".to_string(),
                DbParams::default(),
            )
            .await
            .unwrap_err();
        assert!(!err.to_string().is_empty());
    }

    #[tokio::test]
    async fn a_cursor_can_be_abandoned_before_it_is_exhausted() {
        let (_root, db) = engine("abandon");
        let id = open(&db, "app.db").await;
        exec(&db, id, "CREATE TABLE t (a INTEGER)").await;
        for _ in 0..50 {
            exec(&db, id, "INSERT INTO t VALUES (1)").await;
        }
        let cursor = db
            .query(id, "SELECT a FROM t".to_string(), DbParams::default())
            .await
            .unwrap();
        let batch = db.fetch(cursor.id, 16).await.unwrap();
        assert!(!batch.done);
        db.close_cursor(cursor.id).await.unwrap();
        // Idempotent, and a fetch afterwards is a closed-cursor error rather
        // than a step into a freed statement.
        db.close_cursor(cursor.id).await.unwrap();
        assert!(db.fetch(cursor.id, 16).await.is_err());
    }

    /// The engine asks for a scratch file for some statements, and takes it
    /// from the OS temp directory — which is outside the root jail by
    /// construction, so the VFS refuses it and the statement fails. Found by
    /// the conformance suite, which drops its table between checks.
    #[tokio::test]
    async fn a_statement_needing_scratch_space_does_not_reach_for_the_os_temp_dir() {
        let (root, db) = engine("temp-store");
        let id = open(&db, "app.db").await;
        exec(&db, id, "CREATE TABLE t (a INTEGER)").await;
        exec(&db, id, "INSERT INTO t VALUES (1)").await;
        // The statement that reaches for scratch space.
        db.execute(id, "DROP TABLE t".to_string(), DbParams::default())
            .await
            .expect("a drop must not need a file outside the jail");
        exec(&db, id, "CREATE TABLE t (a INTEGER)").await;
        db.close(id).await.unwrap();

        // Nothing scratch-shaped was left inside the jail either — the scratch
        // space is memory, not a file somewhere more convenient.
        let names: Vec<String> = std::fs::read_dir(&root)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            names.iter().all(|n| n.starts_with("app.db")),
            "scratch files were left behind: {names:?}"
        );
    }

    /// The engine picks its storage from the `IO` it is handed and from nothing
    /// else, so an in-memory database is one we hand `MemoryIO` — not one we
    /// name `:memory:`. Handing it the jailed VFS with that path produces a
    /// *file* called `:memory:` while `is_in_memory_db()` reports true, which
    /// is what this test exists to keep from coming back.
    #[tokio::test]
    async fn an_in_memory_database_touches_no_filesystem() {
        let (root, db) = engine("memory");
        let id = db
            .open(
                String::new(),
                EmbeddedDbOptions {
                    in_memory: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        exec(&db, id, "CREATE TABLE t (a INTEGER)").await;
        exec(&db, id, "INSERT INTO t VALUES (1)").await;
        let cursor = db
            .query(id, "SELECT a FROM t".to_string(), DbParams::default())
            .await
            .unwrap();
        let (_bytes, rows) = drain(&db, cursor.id, 4096).await;
        assert_eq!(rows, 1);

        let left: Vec<_> = std::fs::read_dir(&root)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert!(
            left.is_empty(),
            "an in-memory database wrote files: {left:?}"
        );

        // Each open is its own database. Nothing is shared by name, which is
        // why the named spelling is refused a layer up rather than silently
        // handing back a second empty one.
        let other = db
            .open(
                String::new(),
                EmbeddedDbOptions {
                    in_memory: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(
            db.query(other, "SELECT a FROM t".to_string(), DbParams::default())
                .await
                .is_err()
        );
        db.close(id).await.unwrap();
        db.close(other).await.unwrap();
    }

    /// A path arriving beside `in_memory` is ignored, not opened. The op above
    /// takes no path at all, so this is belt and braces — but the trait says
    /// "ignored", and a provider that quietly honoured it would turn an ungated
    /// op into a way to open any file.
    #[tokio::test]
    async fn an_in_memory_open_ignores_a_path_entirely() {
        let (root, db) = engine("memory-path");
        let id = db
            .open(
                "../../../etc/passwd".to_string(),
                EmbeddedDbOptions {
                    in_memory: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        exec(&db, id, "CREATE TABLE t (a INTEGER)").await;
        db.close(id).await.unwrap();
        assert!(std::fs::read_dir(&root).unwrap().next().is_none());
    }

    #[tokio::test]
    async fn an_encrypted_database_needs_its_key_to_reopen() {
        let (_root, db) = engine("encrypted");
        let key = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let with_key = || EmbeddedDbOptions {
            hex_key: Some(key.to_string()),
            ..Default::default()
        };
        let id = db.open("secret.db".to_string(), with_key()).await.unwrap();
        exec(&db, id, "CREATE TABLE t (a INTEGER)").await;
        exec(&db, id, "INSERT INTO t VALUES (1)").await;
        db.close(id).await.unwrap();

        let reopened = db.open("secret.db".to_string(), with_key()).await.unwrap();
        let cursor = db
            .query(reopened, "SELECT a FROM t".to_string(), DbParams::default())
            .await
            .unwrap();
        let (_bytes, rows) = drain(&db, cursor.id, 4096).await;
        assert_eq!(rows, 1);
        db.close(reopened).await.unwrap();

        // Without the key the pages are noise, not a database.
        let plain = db.open("secret.db".to_string(), opts()).await;
        let failed = match plain {
            Err(_) => true,
            Ok(id) => db
                .query(id, "SELECT a FROM t".to_string(), DbParams::default())
                .await
                .is_err(),
        };
        assert!(failed, "an encrypted database opened without its key");
    }
}
