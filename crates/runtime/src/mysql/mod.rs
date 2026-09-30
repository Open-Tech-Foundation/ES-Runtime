//! The built-in MySQL (and MariaDB) driver's connection (DECISIONS.md D147).
//!
//! Rust owns the conversation: the protocol-10 handshake and its TLS upgrade,
//! the authentication plugins, the statement cache, `COM_STMT_EXECUTE`, and
//! transcoding binary (and text) rows into the row layout D56 fixed.
//! JavaScript (`runtime:db`) owns values — encoding the parameter block,
//! decoding columns — the connection string, and the pool.
//!
//! It reaches the server through the agent's [`NetProvider`], so the host
//! allowlist, the TLS roots and socket ownership apply as they do to
//! `runtime:net`.

mod auth;
mod wire;

use std::collections::HashMap;
use std::sync::Arc;

use es_runtime_common::ErrorCode;
use es_runtime_providers::{ConnectOptions, Entropy, NetProvider, ProviderError};

use crate::postgres::SslMode;
use wire::{Fields, Inbox, Out};

// Capability flags (`CLIENT_*`) this driver asks for.
const LONG_PASSWORD: u32 = 0x1;
const FOUND_ROWS: u32 = 0x2;
const LONG_FLAG: u32 = 0x4;
const CONNECT_WITH_DB: u32 = 0x8;
const PROTOCOL_41: u32 = 0x200;
const SSL: u32 = 0x800;
const TRANSACTIONS: u32 = 0x2000;
const SECURE_CONNECTION: u32 = 0x8000;
const MULTI_STATEMENTS: u32 = 0x1_0000;
const MULTI_RESULTS: u32 = 0x2_0000;
const PS_MULTI_RESULTS: u32 = 0x4_0000;
const PLUGIN_AUTH: u32 = 0x8_0000;
const PLUGIN_AUTH_LENENC_CLIENT_DATA: u32 = 0x20_0000;
const DEPRECATE_EOF: u32 = 0x100_0000;

/// Server status: more result sets follow this one.
const STATUS_MORE_RESULTS: u16 = 0x8;

const COM_QUIT: u8 = 0x01;
const COM_QUERY: u8 = 0x03;
const COM_PING: u8 = 0x0e;
const COM_STMT_PREPARE: u8 = 0x16;
const COM_STMT_EXECUTE: u8 = 0x17;
const COM_STMT_CLOSE: u8 = 0x19;

/// `ER_UNSUPPORTED_PS`: this statement cannot be prepared — run it as text.
const ER_UNSUPPORTED_PS: u16 = 1295;

/// Everything a connection needs, decided by the driver in JavaScript.
#[derive(Clone)]
pub(crate) struct Options {
    pub host: String,
    pub port: u16,
    pub sslmode: SslMode,
    pub ca: Vec<u8>,
    pub user: String,
    pub password: String,
    pub database: Option<String>,
    /// The server's RSA public key (PEM), for full authentication without TLS.
    pub server_public_key: Option<String>,
    /// Ask the server for its key instead — opt-in, since it arrives over the
    /// connection it protects.
    pub allow_public_key_retrieval: bool,
    pub cache_limit: usize,
    pub statement_timeout_ms: Option<u64>,
}

/// Why an operation failed.
pub(crate) enum Failure {
    /// An `ERR` packet; the connection is fine.
    Server {
        code: u16,
        sqlstate: String,
        message: String,
    },
    Lost(String),
    Auth(String),
    Busy(String),
    Unsupported(String),
    /// What the caller asked for does not fit the statement (a parameter count).
    Mismatch(String),
    /// The provider refused to connect — an address outside the allowlist.
    Refused(ProviderError),
}

/// What a `ColumnDefinition41` says, of the parts a decoder needs.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Column {
    pub name: String,
    pub kind: u8,
    pub flags: u16,
    pub charset: u16,
}

struct Prepared {
    id: u32,
    params: u16,
    /// Stable while cached: the driver keys its row class on it.
    key: u64,
    /// The columns the last execution reported; every execution sends them
    /// again, and the ones sent then are the ones that are true then.
    columns: Option<Arc<Vec<Column>>>,
    last_used: u64,
}

/// A batch of rows in the shared layout.
pub(crate) struct Batch {
    pub bytes: Vec<u8>,
    pub rows: u32,
    pub done: bool,
}

/// What a statement answered.
pub(crate) enum Started {
    /// An `OK`: no result set.
    Done { affected: u64, last_insert_id: u64 },
    /// A result set, and its first batch.
    Rows {
        /// The row-class key: a cached statement's, or 0 for the text protocol.
        statement: u64,
        /// The columns, when they are not the ones last reported for `statement`.
        fresh: Option<Arc<Vec<Column>>>,
        /// Whether rows are in the binary encoding (prepared) or text.
        binary: bool,
        first: Batch,
    },
}

/// The open result set a connection is in the middle of.
struct Stream {
    layout: Vec<i8>,
    binary: bool,
}

struct Greeting {
    version: String,
    connection_id: u32,
    scramble: Vec<u8>,
    capabilities: u32,
    charset: u8,
    plugin: String,
}

enum Head {
    Ok {
        affected: u64,
        last_insert_id: u64,
        status: u16,
    },
    Columns(Vec<Column>),
}

pub(crate) struct Connection {
    net: Arc<dyn NetProvider>,
    socket: u64,
    inbox: Inbox,
    capabilities: u32,
    pub server_version: String,
    /// This connection's id on the server — what `KILL QUERY` names.
    pub connection_id: u32,
    /// Server status flags from the last `OK` or `EOF`.
    pub status: u16,
    statements: HashMap<String, Prepared>,
    /// Evicted statements, closed ahead of the next command (they get no answer).
    closing: Vec<u32>,
    /// Row-class keys of evicted statements, for the driver to forget too.
    evicted: Vec<u64>,
    cache_limit: usize,
    tick: u64,
    next_key: u64,
    fatal: Option<String>,
    stream: Option<Stream>,
}

impl Connection {
    /// Opens a connection and completes the handshake; a failure closes the
    /// socket before it returns. `opened` learns the socket id as soon as there
    /// is one, so a connect timeout can close it.
    pub(crate) async fn open(
        net: Arc<dyn NetProvider>,
        entropy: Arc<dyn Entropy>,
        options: &Options,
        opened: &std::sync::Mutex<Option<u64>>,
    ) -> Result<Connection, Failure> {
        let (socket, _) = net
            .connect(
                options.host.clone(),
                options.port,
                ConnectOptions {
                    secure: false,
                    ca: Vec::new(),
                    sni: None,
                    alpn: Vec::new(),
                },
            )
            .await
            .map_err(connect_failure)?;
        *opened.lock().unwrap() = Some(socket);
        let mut connection = Connection {
            net,
            socket,
            inbox: Inbox::default(),
            capabilities: 0,
            server_version: String::new(),
            connection_id: 0,
            status: 0,
            statements: HashMap::new(),
            closing: Vec::new(),
            evicted: Vec::new(),
            cache_limit: options.cache_limit,
            tick: 0,
            next_key: 0,
            fatal: None,
            stream: None,
        };
        match connection
            .handshake(entropy.as_ref(), options, opened)
            .await
        {
            Ok(()) => Ok(connection),
            Err(failure) => {
                let _ = connection.net.close(connection.socket).await;
                Err(failure)
            }
        }
    }

    async fn handshake(
        &mut self,
        entropy: &dyn Entropy,
        options: &Options,
        opened: &std::sync::Mutex<Option<u64>>,
    ) -> Result<(), Failure> {
        // Through `packet`, so a server that hangs up before greeting is a lost
        // connection rather than a bare error.
        let greeting = self.packet().await?;
        if greeting.first() == Some(&0xff) {
            return Err(read_error(&greeting).map_err(|e| self.die(e))?);
        }
        let hello = read_greeting(&greeting).map_err(|e| match e {
            GreetingError::Protocol(p) => Failure::Unsupported(format!(
                "the server speaks handshake protocol {p}; this driver speaks 10"
            )),
            GreetingError::Malformed(m) => self.die(m),
        })?;
        self.server_version = hello.version.clone();
        self.connection_id = hello.connection_id;

        let mut capabilities = LONG_PASSWORD
            | FOUND_ROWS
            | LONG_FLAG
            | PROTOCOL_41
            | TRANSACTIONS
            | SECURE_CONNECTION
            | MULTI_STATEMENTS
            | MULTI_RESULTS
            | PS_MULTI_RESULTS
            | PLUGIN_AUTH
            | PLUGIN_AUTH_LENENC_CLIENT_DATA
            | DEPRECATE_EOF;
        let database = options.database.as_deref().filter(|d| !d.is_empty());
        if database.is_some() {
            capabilities |= CONNECT_WITH_DB;
        }
        capabilities &= hello.capabilities | CONNECT_WITH_DB;
        if hello.capabilities & PROTOCOL_41 == 0 {
            return Err(Failure::Unsupported(
                "the server does not speak protocol 4.1, which MySQL has since 4.1".into(),
            ));
        }
        // utf8mb4 in the server's preferred collation: MySQL 8's
        // utf8mb4_0900_ai_ci when offered, utf8mb4_general_ci otherwise.
        let charset = if hello.charset == 255 { 255 } else { 45 };

        let wants_tls = options.sslmode != SslMode::Disable;
        let mut tls = false;
        if wants_tls && hello.capabilities & SSL != 0 {
            capabilities |= SSL;
            // The start of a handshake response, cut short: the server reads
            // this much, then expects a TLS handshake.
            let seq = self.inbox.seq.wrapping_add(1);
            let mut request = Out::new(32);
            request
                .u32(capabilities)
                .u32(0x0100_0000)
                .u8(charset)
                .zeros(23);
            self.write(request.finish(seq)).await?;
            let (socket, _) = self
                .net
                .start_tls(self.socket, options.host.clone(), Vec::new(), options.ca.clone())
                .await
                .map_err(|e| {
                    Failure::Lost(format!(
                        "TLS with the server failed: {e}. A stock MySQL server presents a certificate it signed itself, which cannot be verified — pass the server's authority as sslRootCert, or sslmode: \"disable\" to connect without TLS"
                    ))
                })?;
            self.socket = socket;
            *opened.lock().unwrap() = Some(socket);
            self.inbox = Inbox::default();
            self.inbox.seq = seq;
            tls = true;
        } else if options.sslmode == SslMode::Require {
            return Err(Failure::Unsupported(
                "the server does not offer TLS and sslmode is 'require'".into(),
            ));
        }
        self.capabilities = capabilities;

        let plugin = if hello.plugin.is_empty() {
            "mysql_native_password".to_string()
        } else {
            hello.plugin.clone()
        };
        let mut response = Out::new(128);
        response
            .u32(capabilities)
            .u32(0x0100_0000)
            .u8(charset)
            .zeros(23)
            .cstr(&options.user)
            .lenenc_bytes(&scramble_for(&plugin, &options.password, &hello.scramble)?);
        if capabilities & CONNECT_WITH_DB != 0 {
            response.cstr(database.unwrap_or(""));
        }
        response.cstr(&plugin);
        let seq = self.inbox.seq.wrapping_add(1);
        self.write(response.finish(seq)).await?;
        self.authenticate(entropy, plugin, hello.scramble, tls, options)
            .await?;

        // The session in UTC, so a TIMESTAMP arrives as the instant it is.
        let mut init = "SET time_zone = '+00:00'".to_string();
        if let Some(ms) = options.statement_timeout_ms.filter(|ms| *ms > 0) {
            if self.server_version.contains("MariaDB") {
                init.push_str(&format!(", max_statement_time = {}", ms as f64 / 1000.0));
            } else {
                init.push_str(&format!(", max_execution_time = {ms}"));
            }
        }
        self.simple(&init).await
    }

    /// Answers the server until it says the login succeeded, or refuses it.
    async fn authenticate(
        &mut self,
        entropy: &dyn Entropy,
        mut plugin: String,
        mut scramble: Vec<u8>,
        tls: bool,
        options: &Options,
    ) -> Result<(), Failure> {
        loop {
            let packet = self.packet().await?;
            match packet.first() {
                Some(0x00) => {
                    self.status = read_ok(&packet).map_err(|e| self.die(e))?.2;
                    return Ok(());
                }
                Some(0xff) => return Err(read_error(&packet).map_err(|e| self.die(e))?),
                Some(0xfe) => {
                    // AuthSwitchRequest: another plugin, and a fresh scramble.
                    let mut fields = Fields::new(&packet, 1);
                    plugin = fields.cstr().map_err(|e| self.die(e))?;
                    let data = fields.rest();
                    scramble = match data.split_last() {
                        Some((0, head)) => head.to_vec(),
                        _ => data.to_vec(),
                    };
                    let reply = scramble_for(&plugin, &options.password, &scramble)?;
                    self.reply(&reply).await?;
                }
                Some(0x01) => {
                    // AuthMoreData, which only caching_sha2_password sends.
                    let data = &packet[1..];
                    if data == [3] {
                        continue; // fast authentication succeeded; OK follows
                    }
                    if data == [4] {
                        // Full authentication: the server wants the password.
                        let reply = if tls {
                            auth::password_bytes(&options.password)
                        } else if let Some(pem) = &options.server_public_key {
                            auth::encrypt_password(entropy, &options.password, &scramble, pem)
                                .map_err(Failure::Auth)?
                        } else if options.allow_public_key_retrieval {
                            vec![2] // ask for the key; the caller accepted the trade
                        } else {
                            return Err(Failure::Auth(
                                "the server needs the password itself (caching_sha2_password full authentication), and this connection has no TLS to send it over. Connect with TLS, give the server's key as serverPublicKey, or — on a network you trust — allowPublicKeyRetrieval: true".into(),
                            ));
                        };
                        self.reply(&reply).await?;
                        continue;
                    }
                    let pem = String::from_utf8_lossy(data).into_owned();
                    if pem.contains("BEGIN PUBLIC KEY") {
                        let reply =
                            auth::encrypt_password(entropy, &options.password, &scramble, &pem)
                                .map_err(Failure::Auth)?;
                        self.reply(&reply).await?;
                        continue;
                    }
                    return Err(Failure::Auth(format!(
                        "the {plugin} plugin sent something this driver does not understand"
                    )));
                }
                other => {
                    return Err(Failure::Auth(format!(
                        "unexpected packet 0x{:x} during authentication",
                        other.copied().unwrap_or(0)
                    )));
                }
            }
        }
    }

    /// One more packet in a conversation the server started.
    async fn reply(&mut self, payload: &[u8]) -> Result<(), Failure> {
        let mut out = Out::new(payload.len());
        out.bytes(payload);
        let seq = self.inbox.seq.wrapping_add(1);
        self.write(out.finish(seq)).await
    }

    /// Row-class keys of statements evicted since the last call.
    pub(crate) fn take_evicted(&mut self) -> Vec<u64> {
        std::mem::take(&mut self.evicted)
    }

    /// The connection's socket, once the handshake is done.
    pub(crate) fn socket(&self) -> u64 {
        self.socket
    }

    /// Runs `text` (prepared, or as text where MySQL will not prepare it) with
    /// `params` — the `COM_STMT_EXECUTE` parameter block the driver encoded —
    /// and returns what it answered: an `OK`, or the first batch of rows.
    /// With `discard`, rows are read and dropped: `execute()`.
    pub(crate) async fn query(
        &mut self,
        text: &str,
        params: &[u8],
        param_count: usize,
        max_bytes: usize,
        discard: bool,
    ) -> Result<Started, Failure> {
        self.check()?;
        let (head, binary, statement) = self.run(text, params, param_count).await?;
        let columns = match head {
            Head::Ok {
                affected,
                last_insert_id,
                status,
            } => {
                self.status = status;
                if status & STATUS_MORE_RESULTS != 0 {
                    self.discard_results().await?;
                }
                return Ok(Started::Done {
                    affected,
                    last_insert_id,
                });
            }
            Head::Columns(columns) => columns,
        };
        let layout = if binary {
            columns.iter().map(|c| width_of(c.kind)).collect()
        } else {
            vec![0; columns.len()]
        };
        // The row class is keyed on the statement; report the columns only
        // when they are not the ones this statement last reported.
        let (key, fresh) = match statement.and_then(|t| self.statements.get_mut(&t)) {
            Some(prepared) if prepared.columns.as_deref() == Some(&columns) => (prepared.key, None),
            Some(prepared) => {
                let columns = Arc::new(columns);
                prepared.columns = Some(columns.clone());
                (prepared.key, Some(columns))
            }
            None => (0, Some(Arc::new(columns))),
        };
        self.stream = Some(Stream { layout, binary });
        let first = if discard {
            while self.stream.is_some() {
                self.batch(usize::MAX, true).await?;
            }
            Batch {
                bytes: Vec::new(),
                rows: 0,
                done: true,
            }
        } else {
            self.batch(max_bytes, false).await?
        };
        Ok(Started::Rows {
            statement: key,
            fresh,
            binary,
            first,
        })
    }

    /// The next batch of an open result set, or an empty finished one.
    pub(crate) async fn fetch(&mut self, max_bytes: usize) -> Result<Batch, Failure> {
        if let Some(message) = &self.fatal {
            return Err(Failure::Lost(message.clone()));
        }
        if self.stream.is_none() {
            return Ok(Batch {
                bytes: Vec::new(),
                rows: 0,
                done: true,
            });
        }
        self.batch(max_bytes, false).await
    }

    /// Reads and drops the rest of an open result set.
    pub(crate) async fn finish(&mut self) -> Result<(), Failure> {
        while self.stream.is_some() {
            self.batch(usize::MAX, true).await?;
        }
        Ok(())
    }

    /// Runs a script — several statements — through the text protocol,
    /// returning each statement's affected rows and insert id. MySQL does not
    /// wrap a script in a transaction, and DDL commits implicitly.
    pub(crate) async fn script(&mut self, sql: &str) -> Result<Vec<(u64, u64)>, Failure> {
        self.check()?;
        let mut out = Out::new(sql.len() + 1);
        out.u8(COM_QUERY).bytes(sql.as_bytes());
        self.send(out).await?;
        let mut results = Vec::new();
        let mut head = self.head().await?;
        loop {
            let status = match head {
                Head::Ok {
                    affected,
                    last_insert_id,
                    status,
                } => {
                    results.push((affected, last_insert_id));
                    status
                }
                Head::Columns(_) => {
                    let status = self.skip_rows().await?;
                    results.push((0, 0));
                    status
                }
            };
            self.status = status;
            if status & STATUS_MORE_RESULTS == 0 {
                return Ok(results);
            }
            head = self.head().await?;
        }
    }

    /// Asks the server whether it is still there.
    pub(crate) async fn ping(&mut self) -> Result<(), Failure> {
        self.check()?;
        let mut out = Out::new(1);
        out.u8(COM_PING);
        self.write(out.finish(0)).await?;
        let packet = self.packet().await?;
        if packet.first() == Some(&0xff) {
            return Err(read_error(&packet).map_err(|e| self.die(e))?);
        }
        Ok(())
    }

    /// Says goodbye, if the connection is still there, and closes the socket.
    pub(crate) async fn close(&mut self) {
        if self.fatal.is_none() {
            let mut out = Out::new(1);
            out.u8(COM_QUIT);
            let _ = self.write(out.finish(0)).await;
        }
        self.fatal
            .get_or_insert_with(|| "the connection is closed".into());
        let _ = self.net.close(self.socket).await;
    }

    /// One statement through the text protocol, any rows dropped.
    async fn simple(&mut self, sql: &str) -> Result<(), Failure> {
        let mut out = Out::new(sql.len() + 1);
        out.u8(COM_QUERY).bytes(sql.as_bytes());
        self.send(out).await?;
        match self.head().await? {
            Head::Ok { status, .. } => {
                self.status = status;
                if status & STATUS_MORE_RESULTS != 0 {
                    self.discard_results().await?;
                }
            }
            Head::Columns(_) => {
                self.status = self.skip_rows().await?;
                if self.status & STATUS_MORE_RESULTS != 0 {
                    self.discard_results().await?;
                }
            }
        }
        Ok(())
    }

    /// Prepares `text` where MySQL can, running it as text where it cannot (a
    /// few statements — `USE`, some `SHOW`s — refuse the prepared protocol, and
    /// with no parameters the text protocol gets nothing wrong).
    async fn run(
        &mut self,
        text: &str,
        params: &[u8],
        param_count: usize,
    ) -> Result<(Head, bool, Option<String>), Failure> {
        let (id, expected, transient) = match self.prepare(text).await {
            Ok(prepared) => prepared,
            Err(Failure::Server { code, .. }) if code == ER_UNSUPPORTED_PS && param_count == 0 => {
                let mut out = Out::new(text.len() + 1);
                out.u8(COM_QUERY).bytes(text.as_bytes());
                self.send(out).await?;
                return Ok((self.head().await?, false, None));
            }
            Err(failure) => return Err(failure),
        };
        if param_count != expected as usize {
            if transient {
                self.closing.push(id);
            }
            let s = if expected == 1 { "" } else { "s" };
            return Err(Failure::Mismatch(format!(
                "the statement takes {expected} parameter{s} and was given {param_count}"
            )));
        }
        let mut out = Out::new(10 + params.len());
        out.u8(COM_STMT_EXECUTE).u32(id).u8(0).u32(1).bytes(params);
        self.send(out).await?;
        if transient {
            self.closing.push(id);
        }
        let head = self.head().await?;
        Ok((head, true, (!transient).then(|| text.to_string())))
    }

    /// The statement id for `text`, its parameter count, and whether it is
    /// transient (caching off: closed once run).
    async fn prepare(&mut self, text: &str) -> Result<(u32, u16, bool), Failure> {
        self.tick += 1;
        if let Some(cached) = self.statements.get_mut(text) {
            cached.last_used = self.tick;
            return Ok((cached.id, cached.params, false));
        }
        let mut out = Out::new(text.len() + 1);
        out.u8(COM_STMT_PREPARE).bytes(text.as_bytes());
        self.send(out).await?;
        let first = self.packet().await?;
        if first.first() == Some(&0xff) {
            return Err(read_error(&first).map_err(|e| self.die(e))?);
        }
        let parsed = read_prepare_ok(&first);
        let (id, columns, params) = parsed.map_err(|e| self.die(e))?;
        // Its parameter and column definitions are skipped: every execution
        // sends the columns again, and those are the ones that are true then.
        let eof = self.capabilities & DEPRECATE_EOF == 0;
        let skip = params as usize
            + usize::from(params > 0 && eof)
            + columns as usize
            + usize::from(columns > 0 && eof);
        for _ in 0..skip {
            self.packet().await?;
        }
        if self.cache_limit == 0 {
            return Ok((id, params, true));
        }
        while self.statements.len() >= self.cache_limit {
            let Some(oldest) = self
                .statements
                .iter()
                .min_by_key(|(_, p)| p.last_used)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            let gone = self.statements.remove(&oldest).expect("just found");
            self.closing.push(gone.id);
            self.evicted.push(gone.key);
        }
        self.next_key += 1;
        self.statements.insert(
            text.to_string(),
            Prepared {
                id,
                params,
                key: self.next_key,
                columns: None,
                last_used: self.tick,
            },
        );
        Ok((id, params, false))
    }

    /// Sends a command's payload, preceded by a `COM_STMT_CLOSE` packet for
    /// each statement the cache let go: those get no answer, so they cost
    /// nothing ahead of the next command and nothing is sent just for them.
    async fn send(&mut self, out: Out) -> Result<(), Failure> {
        let body = out.finish(0);
        if self.closing.is_empty() {
            return self.write(body).await;
        }
        let mut bytes = Vec::with_capacity(self.closing.len() * 9 + body.len());
        for id in self.closing.drain(..) {
            let mut close = Out::new(5);
            close.u8(COM_STMT_CLOSE).u32(id);
            bytes.extend_from_slice(&close.finish(0));
        }
        bytes.extend_from_slice(&body);
        self.write(bytes).await
    }

    /// The start of a response: an `OK`, an error, or a result set's columns.
    async fn head(&mut self) -> Result<Head, Failure> {
        let first = self.packet().await?;
        match first.first() {
            Some(0x00) => {
                let (affected, last_insert_id, status) =
                    read_ok(&first).map_err(|e| self.die(e))?;
                Ok(Head::Ok {
                    affected,
                    last_insert_id,
                    status,
                })
            }
            Some(0xff) => Err(read_error(&first).map_err(|e| self.die(e))?),
            Some(0xfb) => Err(Failure::Unsupported(
                "the server asked to read a local file, which this driver never does".into(),
            )),
            _ => {
                let count = Fields::new(&first, 0).count().map_err(|e| self.die(e))?;
                let mut columns = Vec::with_capacity(count as usize);
                for _ in 0..count {
                    let packet = self.packet().await?;
                    columns.push(read_column(&packet).map_err(|e| self.die(e))?);
                }
                if self.capabilities & DEPRECATE_EOF == 0 {
                    self.packet().await?;
                }
                Ok(Head::Columns(columns))
            }
        }
    }

    /// Rows until the batch holds `max_bytes` or the result set ends.
    async fn batch(&mut self, max_bytes: usize, discard: bool) -> Result<Batch, Failure> {
        let Some(stream) = self.stream.take() else {
            return Ok(Batch {
                bytes: Vec::new(),
                rows: 0,
                done: true,
            });
        };
        let mut bytes = if discard {
            Vec::new()
        } else {
            Vec::with_capacity(self.inbox.buffered().min(max_bytes))
        };
        let mut rows = 0u32;
        loop {
            let payload = match self.inbox.next() {
                Some(payload) => payload,
                None => {
                    self.fill().await?;
                    continue;
                }
            };
            let packet = self.inbox.bytes(payload);
            let lead = packet.first().copied();
            if lead == Some(0xfe) && packet.len() < wire::MAX_PAYLOAD {
                let status = if self.capabilities & DEPRECATE_EOF != 0 {
                    read_ok(packet).map(|r| r.2)
                } else {
                    Fields::new(packet, 3).u16()
                };
                self.status = status.map_err(|e| self.die(e))?;
                if self.status & STATUS_MORE_RESULTS != 0 {
                    self.discard_results().await?;
                }
                return Ok(Batch {
                    bytes,
                    rows,
                    done: true,
                });
            }
            if lead == Some(0xff) {
                let error = read_error(packet);
                return Err(error.map_err(|e| self.die(e))?);
            }
            rows += 1;
            if !discard {
                let appended = if stream.binary {
                    append_binary_row(&mut bytes, packet, &stream.layout)
                } else {
                    append_text_row(&mut bytes, packet, stream.layout.len())
                };
                appended.map_err(|e| self.die(e))?;
                if bytes.len() >= max_bytes {
                    self.stream = Some(stream);
                    return Ok(Batch {
                        bytes,
                        rows,
                        done: false,
                    });
                }
            }
        }
    }

    /// Reads a result set's rows to its end, returning the final status.
    async fn skip_rows(&mut self) -> Result<u16, Failure> {
        loop {
            let packet = self.packet().await?;
            match packet.first() {
                Some(0xff) => return Err(read_error(&packet).map_err(|e| self.die(e))?),
                Some(0xfe) if packet.len() < wire::MAX_PAYLOAD => {
                    let status = if self.capabilities & DEPRECATE_EOF != 0 {
                        read_ok(&packet).map(|r| r.2)
                    } else {
                        Fields::new(&packet, 3).u16()
                    };
                    return status.map_err(|e| self.die(e));
                }
                _ => {}
            }
        }
    }

    /// Reads and drops any further result sets — a `CALL` answers with its
    /// procedure's results and then its own status — which must come off the
    /// wire before anything else can be asked.
    async fn discard_results(&mut self) -> Result<(), Failure> {
        loop {
            let status = match self.head().await? {
                Head::Ok { status, .. } => status,
                Head::Columns(_) => self.skip_rows().await?,
            };
            self.status = status;
            if status & STATUS_MORE_RESULTS == 0 {
                return Ok(());
            }
        }
    }

    fn check(&self) -> Result<(), Failure> {
        if let Some(message) = &self.fatal {
            return Err(Failure::Lost(message.clone()));
        }
        if self.stream.is_some() {
            return Err(Failure::Busy(
                "this connection is streaming a result set — finish it (await rows.toArray(), or let the for-await end), or run the second query on another connection".into(),
            ));
        }
        Ok(())
    }

    async fn write(&mut self, bytes: Vec<u8>) -> Result<(), Failure> {
        if let Some(message) = &self.fatal {
            return Err(Failure::Lost(message.clone()));
        }
        self.net
            .write(self.socket, bytes)
            .await
            .map_err(|e| self.die(e.to_string()))
    }

    async fn fill(&mut self) -> Result<(), Failure> {
        match self.net.read(self.socket).await {
            Ok(Some(chunk)) => {
                self.inbox.push(&chunk);
                Ok(())
            }
            Ok(None) => Err(self.die("the connection closed while a packet was in flight".into())),
            Err(e) => Err(self.die(e.to_string())),
        }
    }

    /// The next payload, as an owned copy: the control paths, where a packet is
    /// small and read once. Rows are read in place by [`batch`](Self::batch).
    async fn packet(&mut self) -> Result<Vec<u8>, Failure> {
        loop {
            if let Some(payload) = self.inbox.next() {
                return Ok(self.inbox.bytes(payload).to_vec());
            }
            self.fill().await?;
        }
    }

    fn die(&mut self, detail: String) -> Failure {
        let message = self
            .fatal
            .get_or_insert_with(|| format!("the connection to the server was lost: {detail}"))
            .clone();
        self.stream = None;
        Failure::Lost(message)
    }
}

/// Stops the statement connection `connection_id` is running: MySQL has no
/// cancel message on the connection itself, so a second connection, as the
/// same user, runs `KILL QUERY`. The statement fails with
/// `ER_QUERY_INTERRUPTED` and the connection stays usable.
pub(crate) async fn cancel(
    net: Arc<dyn NetProvider>,
    entropy: Arc<dyn Entropy>,
    mut options: Options,
    connection_id: u32,
) -> Result<(), Failure> {
    options.cache_limit = 0;
    let opened = std::sync::Mutex::new(None);
    let mut killer = Connection::open(net, entropy, &options, &opened).await?;
    let result = killer.simple(&format!("KILL QUERY {connection_id}")).await;
    killer.close().await;
    result
}

fn connect_failure(e: ProviderError) -> Failure {
    if e.code() == Some(ErrorCode::PermissionDenied) {
        Failure::Refused(e)
    } else {
        Failure::Lost(e.to_string())
    }
}

/// The authentication response `plugin` computes for this password.
fn scramble_for(plugin: &str, password: &str, scramble: &[u8]) -> Result<Vec<u8>, Failure> {
    match plugin {
        "caching_sha2_password" => Ok(auth::caching_sha2(password, scramble)),
        "mysql_native_password" => Ok(auth::native_password(password, scramble)),
        "mysql_clear_password" => Err(Failure::Auth(
            "the server asked for the password in clear text (mysql_clear_password), which this driver never sends".into(),
        )),
        other => Err(Failure::Auth(format!(
            "the server asked for the {other} authentication plugin, which this driver does not speak"
        ))),
    }
}

enum GreetingError {
    Protocol(u8),
    Malformed(String),
}

/// The server's Initial Handshake (protocol 10).
fn read_greeting(packet: &[u8]) -> Result<Greeting, GreetingError> {
    let mut p = Fields::new(packet, 0);
    let protocol = p.u8().map_err(GreetingError::Malformed)?;
    if protocol != 10 {
        return Err(GreetingError::Protocol(protocol));
    }
    (|| {
        let version = p.cstr()?;
        let connection_id = p.u32()?;
        let part1 = p.bytes_of(8)?.to_vec();
        p.u8()?; // filler
        let mut capabilities = p.u16()? as u32;
        let charset = p.u8()?;
        p.u16()?; // status
        capabilities |= (p.u16()? as u32) << 16;
        let data_length = p.u8()? as usize;
        p.bytes_of(10)?; // reserved
        let mut part2: &[u8] = &[];
        if capabilities & SECURE_CONNECTION != 0 {
            part2 = p.bytes_of(13.max(data_length.saturating_sub(8)))?;
            // NUL-terminated; the scramble is the twenty bytes before it.
            if part2.last() == Some(&0) {
                part2 = &part2[..part2.len() - 1];
            }
        }
        let plugin = if capabilities & PLUGIN_AUTH != 0 && !p.done() {
            p.cstr()?
        } else {
            String::new()
        };
        let mut scramble = part1;
        scramble.extend_from_slice(part2);
        Ok(Greeting {
            version,
            connection_id,
            scramble,
            capabilities,
            charset,
            plugin,
        })
    })()
    .map_err(GreetingError::Malformed)
}

/// An `OK` packet: affected rows, last insert id, status flags.
fn read_ok(packet: &[u8]) -> Result<(u64, u64, u16), String> {
    let mut p = Fields::new(packet, 1);
    let affected = p.count()?;
    let last_insert_id = p.count()?;
    let status = if p.done() { 0 } else { p.u16()? };
    Ok((affected, last_insert_id, status))
}

/// An `ERR` packet: code, SQLSTATE and message.
fn read_error(packet: &[u8]) -> Result<Failure, String> {
    let mut p = Fields::new(packet, 1);
    let code = p.u16()?;
    let mut sqlstate = "HY000".to_string();
    if packet.get(p.at) == Some(&b'#') {
        p.u8()?;
        sqlstate = String::from_utf8_lossy(p.bytes_of(5)?).into_owned();
    }
    let message = String::from_utf8_lossy(p.rest()).into_owned();
    Ok(Failure::Server {
        code,
        sqlstate,
        message,
    })
}

/// `COM_STMT_PREPARE_OK`: statement id, column count, parameter count.
fn read_prepare_ok(packet: &[u8]) -> Result<(u32, u16, u16), String> {
    let mut p = Fields::new(packet, 1);
    Ok((p.u32()?, p.u16()?, p.u16()?))
}

/// A `ColumnDefinition41`, of the parts a decoder needs.
fn read_column(packet: &[u8]) -> Result<Column, String> {
    let mut p = Fields::new(packet, 0);
    p.skip_lenenc()?; // catalog
    p.skip_lenenc()?; // schema
    p.skip_lenenc()?; // table alias
    p.skip_lenenc()?; // original table
    let name = p.lenenc_string()?;
    p.skip_lenenc()?; // original name
    p.count()?; // length of the fixed fields
    let charset = p.u16()?;
    p.u32()?; // display length
    let kind = p.u8()?;
    let flags = p.u16()?;
    Ok(Column {
        name,
        kind,
        flags,
        charset,
    })
}

/// How a column's value is laid out in a binary row: a fixed width, `-1` for
/// a one-byte length then that many bytes (dates and times), or `0` for a
/// length-encoded string.
fn width_of(kind: u8) -> i8 {
    match kind {
        0x01 => 1,                       // TINY
        0x02 | 0x0d => 2,                // SHORT, YEAR
        0x03 | 0x09 | 0x04 => 4,         // LONG, INT24, FLOAT
        0x08 | 0x05 => 8,                // LONGLONG, DOUBLE
        0x0a | 0x0c | 0x07 | 0x0b => -1, // DATE, DATETIME, TIMESTAMP, TIME
        _ => 0,
    }
}

/// Transcodes one binary row (`0x00`, the NULL bitmap, the values) into the
/// shared layout — `length(4) columns(2)`, then per column `length(4)` (`-1`
/// for NULL) and the value's bytes, copied as they are.
fn append_binary_row(out: &mut Vec<u8>, src: &[u8], layout: &[i8]) -> Result<(), String> {
    let columns = layout.len();
    let row_start = out.len();
    out.reserve(src.len() + 6 + columns * 4);
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&(columns as i16).to_be_bytes());
    let bitmap = 1;
    let mut r = bitmap + (columns + 9) / 8;
    for (c, &width) in layout.iter().enumerate() {
        let bit = c + 2;
        let byte = *src.get(bitmap + bit / 8).ok_or_else(short_row)?;
        if byte & (1 << (bit % 8)) != 0 {
            out.extend_from_slice(&(-1i32).to_be_bytes());
            continue;
        }
        let size = if width > 0 {
            width as usize
        } else if width < 0 {
            let size = *src.get(r).ok_or_else(short_row)? as usize;
            r += 1;
            size
        } else {
            let mut fields = Fields::new(src, r);
            let size = fields.count()? as usize;
            r = fields.at;
            size
        };
        let value = src.get(r..r + size).ok_or_else(short_row)?;
        out.extend_from_slice(&(size as i32).to_be_bytes());
        out.extend_from_slice(value);
        r += size;
    }
    let length = (out.len() - row_start) as i32;
    out[row_start..row_start + 4].copy_from_slice(&length.to_be_bytes());
    Ok(())
}

/// Transcodes one text row — a length-encoded string per column, `0xFB` for
/// NULL — into the shared layout.
fn append_text_row(out: &mut Vec<u8>, src: &[u8], columns: usize) -> Result<(), String> {
    let row_start = out.len();
    out.reserve(src.len() + 6 + columns * 4);
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&(columns as i16).to_be_bytes());
    let mut fields = Fields::new(src, 0);
    for _ in 0..columns {
        match fields.lenenc()? {
            None => out.extend_from_slice(&(-1i32).to_be_bytes()),
            Some(size) => {
                let value = fields.bytes_of(size as usize)?;
                out.extend_from_slice(&(size as i32).to_be_bytes());
                out.extend_from_slice(value);
            }
        }
    }
    let length = (out.len() - row_start) as i32;
    out[row_start..row_start + 4].copy_from_slice(&length.to_be_bytes());
    Ok(())
}

fn short_row() -> String {
    "a row ended before its columns did".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row_values(bytes: &[u8]) -> Vec<Option<Vec<u8>>> {
        let columns = i16::from_be_bytes([bytes[4], bytes[5]]) as usize;
        let mut at = 6;
        let mut out = Vec::new();
        for _ in 0..columns {
            let size = i32::from_be_bytes(bytes[at..at + 4].try_into().unwrap());
            at += 4;
            if size < 0 {
                out.push(None);
            } else {
                out.push(Some(bytes[at..at + size as usize].to_vec()));
                at += size as usize;
            }
        }
        assert_eq!(
            i32::from_be_bytes(bytes[0..4].try_into().unwrap()) as usize,
            at
        );
        out
    }

    #[test]
    fn a_binary_row_is_transcoded_with_nulls_widths_and_strings() {
        // Columns: LONG (4), NULL string, DATE (-1 prefixed), VARCHAR "hi".
        let layout = [4, 0, -1, 0];
        // Bitmap: bit c+2 set for column 1 (NULL) → bit 3.
        let src = [
            0x00,
            0b0000_1000,
            7,
            0,
            0,
            0,
            4,
            0xe9,
            0x07,
            1,
            2,
            2,
            b'h',
            b'i',
        ];
        let mut out = Vec::new();
        append_binary_row(&mut out, &src, &layout).unwrap();
        assert_eq!(
            row_values(&out),
            [
                Some(vec![7, 0, 0, 0]),
                None,
                Some(vec![0xe9, 0x07, 1, 2]),
                Some(b"hi".to_vec())
            ]
        );
    }

    #[test]
    fn a_text_row_is_transcoded_with_its_nulls() {
        let src = [1, b'5', 0xfb, 2, b'o', b'k'];
        let mut out = Vec::new();
        append_text_row(&mut out, &src, 3).unwrap();
        assert_eq!(
            row_values(&out),
            [Some(b"5".to_vec()), None, Some(b"ok".to_vec())]
        );
    }

    #[test]
    fn a_short_row_is_an_error_not_a_panic() {
        let mut out = Vec::new();
        assert!(append_binary_row(&mut out, &[0x00, 0], &[4]).is_err());
        assert!(append_text_row(&mut out, &[5, b'a'], 1).is_err());
    }

    #[test]
    fn ok_and_error_packets_are_read() {
        assert_eq!(read_ok(&[0x00, 3, 9, 0x02, 0x00]).unwrap(), (3, 9, 2));
        let mut err = vec![0xff, 0x26, 0x04, b'#'];
        err.extend_from_slice(b"23000Duplicate");
        match read_error(&err).unwrap() {
            Failure::Server {
                code,
                sqlstate,
                message,
            } => {
                assert_eq!(
                    (code, sqlstate.as_str(), message.as_str()),
                    (1062, "23000", "Duplicate")
                );
            }
            _ => panic!("not a server error"),
        }
    }
}
