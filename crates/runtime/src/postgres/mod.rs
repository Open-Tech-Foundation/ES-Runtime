//! The built-in PostgreSQL driver's connection (DECISIONS.md D147).
//!
//! Rust owns the conversation: TLS negotiation, authentication, the statement
//! cache, the extended query protocol, and gathering `DataRow`s into a batch in
//! the row layout D56 fixed. JavaScript (`runtime:db`) owns what is about
//! values — the connection string, the startup parameters, encoding
//! parameters, decoding columns — and the pool.
//!
//! It reaches the server through the agent's [`NetProvider`], so the host
//! allowlist, the TLS roots and socket ownership apply exactly as they do to
//! `runtime:net`, and it holds no authority `runtime:net` would not.

mod scram;
mod wire;

use std::collections::HashMap;
use std::sync::Arc;

use es_runtime_providers::{ConnectOptions, NetProvider, ProviderError};

use wire::{Fields, Inbox, Message, Out, back};

/// SQLSTATEs that mean a cached plan is stale rather than that the statement
/// failed: the table changed under it (`0A000`), or the server no longer has
/// it (`26000`), or it has one of the name already (`42P05`) after a reset.
const STALE_PLAN: [&str; 3] = ["0A000", "26000", "42P05"];

/// Whether and how to ask for TLS.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SslMode {
    Disable,
    Prefer,
    Require,
}

/// Everything a connection needs, decided by the driver in JavaScript.
pub(crate) struct Options {
    pub host: String,
    pub port: u16,
    pub sslmode: SslMode,
    /// Extra trust anchors (PEM); empty ⇒ the provider's roots.
    pub ca: Vec<u8>,
    /// The startup packet's parameters, `user` and `database` among them.
    pub params: Vec<(String, String)>,
    /// Already normalized (NFKC) by the driver.
    pub password: Option<String>,
    /// How many prepared statements to keep; `0` prepares every query anew.
    pub cache_limit: usize,
    /// Type OIDs whose columns are asked for in binary.
    pub binary_oids: Vec<u32>,
    /// Random bytes for the SCRAM nonce.
    pub nonce: [u8; 18],
}

/// Why an operation failed.
pub(crate) enum Failure {
    /// The server answered with an `ErrorResponse`; the connection is fine.
    Server(Vec<(u8, String)>),
    /// The connection is gone, and every later call says so.
    Lost(String),
    /// Authentication was refused or could not be attempted.
    Auth(String),
    /// A result set is open and only its reader can finish it.
    Busy(String),
    /// What was asked cannot be done here.
    Unsupported(String),
}

/// A statement's result columns.
pub(crate) struct Columns {
    pub names: Vec<String>,
    pub oids: Vec<u32>,
}

struct Prepared {
    /// Stable for as long as the statement is cached: the driver keys its row
    /// class on it rather than receiving the columns again every query.
    id: u64,
    name: String,
    columns: Option<Arc<Columns>>,
    formats: Vec<i16>,
    last_used: u64,
}

/// One batch of a result set.
pub(crate) struct Batch {
    /// `DataRow` frames, back to back, from each length onward.
    pub bytes: Vec<u8>,
    pub rows: u32,
    pub done: bool,
    /// The `CommandComplete` tag once `done`: `SELECT 3`, `INSERT 0 1`.
    pub tag: String,
}

/// What a query started.
pub(crate) struct Started {
    pub statement: u64,
    /// The columns and their formats, when the driver has not seen this
    /// statement before; `None` in `columns` means it returns no rows.
    pub fresh: Option<(Option<Arc<Columns>>, Vec<i16>)>,
    /// Statements dropped from the cache, whose row classes can go too.
    pub evicted: Vec<u64>,
    pub first: Batch,
}

/// What the server said on the side while an operation ran.
#[derive(Default)]
pub(crate) struct Aside {
    pub notices: Vec<Vec<(u8, String)>>,
    pub parameters: Vec<(String, String)>,
}

pub(crate) struct Connection {
    net: Arc<dyn NetProvider>,
    socket: u64,
    inbox: Inbox,
    statements: HashMap<String, Prepared>,
    next_statement: u64,
    tick: u64,
    cache_limit: usize,
    binary_oids: Vec<u32>,
    pub process_id: i32,
    pub secret_key: i32,
    /// `ReadyForQuery`'s transaction status: `I`, `T` or `E`.
    pub status: u8,
    /// The first transport failure, latched: nothing later on this socket can
    /// be trusted to start on a message boundary.
    fatal: Option<String>,
    /// A result set is open and unread.
    streaming: bool,
    aside: Aside,
}

impl Connection {
    /// Opens a connection and completes the handshake. A failure closes the
    /// socket before it returns.
    ///
    /// `opened` is told the socket's id as soon as there is one, so a caller
    /// that gives up waiting — a connect timeout — can close it, which is what
    /// ends a handshake with a server that accepted and then went silent.
    pub(crate) async fn open(
        net: Arc<dyn NetProvider>,
        options: Options,
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
            .map_err(|e| Failure::Lost(provider_message(&e)))?;
        *opened.lock().unwrap() = Some(socket);
        let mut connection = Connection {
            net,
            socket,
            inbox: Inbox::default(),
            statements: HashMap::new(),
            next_statement: 0,
            tick: 0,
            cache_limit: options.cache_limit,
            binary_oids: options.binary_oids.clone(),
            process_id: 0,
            secret_key: 0,
            status: b'I',
            fatal: None,
            streaming: false,
            aside: Aside::default(),
        };
        match connection.handshake(&options, opened).await {
            Ok(()) => Ok(connection),
            Err(failure) => {
                let _ = connection.net.close(connection.socket).await;
                Err(failure)
            }
        }
    }

    async fn handshake(
        &mut self,
        options: &Options,
        opened: &std::sync::Mutex<Option<u64>>,
    ) -> Result<(), Failure> {
        if options.sslmode != SslMode::Disable {
            // libpq's sequence: ask in plaintext, and only then become TLS.
            let mut out = Out::default();
            wire::ssl_request(&mut out);
            self.write(out).await?;
            let answer = loop {
                if let Some(byte) = self.inbox.byte() {
                    break byte;
                }
                self.fill().await?;
            };
            if answer == b'S' {
                let (socket, _) = self
                    .net
                    .start_tls(
                        self.socket,
                        options.host.clone(),
                        Vec::new(),
                        options.ca.clone(),
                    )
                    .await
                    .map_err(|e| self.die(provider_message(&e)))?;
                self.socket = socket;
                *opened.lock().unwrap() = Some(socket);
            } else if options.sslmode == SslMode::Require {
                return Err(Failure::Unsupported(format!(
                    "the server refused TLS and sslmode is 'require' (it answered {:?})",
                    answer as char
                )));
            }
        }

        let params: Vec<(&str, &str)> = options
            .params
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let mut out = Out::default();
        wire::startup(&mut out, &params);
        self.write(out).await?;
        self.authenticate(options).await
    }

    async fn authenticate(&mut self, options: &Options) -> Result<(), Failure> {
        let mut exchange: Option<scram::Scram> = None;
        let password = || {
            options.password.as_deref().ok_or_else(|| {
                Failure::Auth("the server asked for a password and none was given".into())
            })
        };
        loop {
            let message = self.message().await?;
            match message.tag {
                back::AUTHENTICATION => {
                    let parsed = read_authentication(self.inbox.slice(message.body));
                    let (kind, rest) = parsed.map_err(|e| self.die(e))?;
                    match kind {
                        0 => {}
                        3 => {
                            let mut body = password()?.as_bytes().to_vec();
                            body.push(0);
                            let mut out = Out::default();
                            wire::password(&mut out, &body);
                            self.write(out).await?;
                        }
                        10 => {
                            let offered: Vec<String> = rest
                                .split(|&b| b == 0)
                                .filter(|m| !m.is_empty())
                                .map(|m| String::from_utf8_lossy(m).into_owned())
                                .collect();
                            if !offered.iter().any(|m| m == "SCRAM-SHA-256") {
                                let list = if offered.is_empty() {
                                    "no".to_string()
                                } else {
                                    offered.join(", ")
                                };
                                return Err(Failure::Auth(format!(
                                    "the server offered {list} authentication; this driver speaks SCRAM-SHA-256"
                                )));
                            }
                            password()?;
                            let session = scram::Scram::new(&options.nonce, "");
                            let mut out = Out::default();
                            wire::sasl_initial(
                                &mut out,
                                "SCRAM-SHA-256",
                                session.initial().as_bytes(),
                            );
                            exchange = Some(session);
                            self.write(out).await?;
                        }
                        11 => {
                            let server_first = String::from_utf8_lossy(&rest).into_owned();
                            let session = exchange.as_mut().ok_or_else(|| {
                                Failure::Auth(
                                    "the server continued a SASL exchange that never began".into(),
                                )
                            })?;
                            let reply = session
                                .respond(password()?, &server_first)
                                .map_err(Failure::Auth)?;
                            let mut out = Out::default();
                            wire::password(&mut out, reply.as_bytes());
                            self.write(out).await?;
                        }
                        12 => {
                            let server_final = String::from_utf8_lossy(&rest).into_owned();
                            exchange
                                .as_ref()
                                .ok_or_else(|| {
                                    Failure::Auth(
                                        "the server finished a SASL exchange that never began"
                                            .into(),
                                    )
                                })?
                                .verify(&server_final)
                                .map_err(Failure::Auth)?;
                        }
                        5 => {
                            return Err(Failure::Auth(
                                "the server asked for md5 authentication, which this driver does not implement — configure the server for scram-sha-256 (the default since PostgreSQL 14)".into(),
                            ));
                        }
                        other => {
                            return Err(Failure::Auth(format!(
                                "unsupported authentication request {other}"
                            )));
                        }
                    }
                }
                back::BACKEND_KEY_DATA => {
                    let parsed = read_backend_key(self.inbox.slice(message.body));
                    let (pid, key) = parsed.map_err(|e| self.die(e))?;
                    self.process_id = pid;
                    self.secret_key = key;
                }
                back::ERROR_RESPONSE => {
                    let fields = self.server_fields(message)?;
                    return Err(Failure::Server(fields));
                }
                back::READY_FOR_QUERY => {
                    self.ready(message)?;
                    return Ok(());
                }
                _ => self.observe(message)?,
            }
        }
    }

    /// Takes what the server said on the side since the last call.
    pub(crate) fn take_aside(&mut self) -> Aside {
        std::mem::take(&mut self.aside)
    }

    /// Runs `text` with `params` (the `Bind` parameter section in wire layout)
    /// and returns the first batch of up to about `max_bytes` of rows.
    ///
    /// With `discard`, rows are counted and dropped: `execute()`, which wants
    /// only the command tag.
    pub(crate) async fn query(
        &mut self,
        text: &str,
        params: &[u8],
        max_bytes: usize,
        discard: bool,
    ) -> Result<Started, Failure> {
        self.check()?;
        match self.start(text, params, max_bytes, discard).await {
            Err(Failure::Server(fields)) if is_stale_plan(&fields) => {
                // Retried exactly once, and only for this: the error was followed
                // by a drain to ReadyForQuery, so the connection is clean. The
                // whole cache goes, since whatever invalidated one plan — a
                // migration, a DISCARD — rarely stopped at one.
                let mut evicted: Vec<u64> = self.statements.drain().map(|(_, p)| p.id).collect();
                let mut started = self.start(text, params, max_bytes, discard).await?;
                evicted.append(&mut started.evicted);
                started.evicted = evicted;
                Ok(started)
            }
            other => other,
        }
    }

    /// The next batch of an open result set, or an empty finished one.
    pub(crate) async fn fetch(&mut self, max_bytes: usize) -> Result<Batch, Failure> {
        if let Some(message) = &self.fatal {
            return Err(Failure::Lost(message.clone()));
        }
        if !self.streaming {
            return Ok(Batch {
                bytes: Vec::new(),
                rows: 0,
                done: true,
                tag: String::new(),
            });
        }
        self.batch(max_bytes, false).await
    }

    /// Reads and drops the rest of an open result set, so the connection can
    /// take the next exchange.
    pub(crate) async fn finish(&mut self) -> Result<(), Failure> {
        while self.streaming {
            self.batch(usize::MAX, true).await?;
        }
        Ok(())
    }

    /// Says goodbye, if the connection is still there, and closes the socket.
    pub(crate) async fn close(&mut self) {
        if self.fatal.is_none() {
            let mut out = Out::default();
            wire::terminate(&mut out);
            let _ = self.write(out).await;
        }
        self.fatal
            .get_or_insert_with(|| "the connection is closed".into());
        let _ = self.net.close(self.socket).await;
    }

    async fn start(
        &mut self,
        text: &str,
        params: &[u8],
        max_bytes: usize,
        discard: bool,
    ) -> Result<Started, Failure> {
        let mut evicted = Vec::new();
        self.tick += 1;
        let (statement, fresh) = match self.statements.get_mut(text) {
            Some(cached) => {
                cached.last_used = self.tick;
                (cached.id, None)
            }
            None => {
                let prepared = self.prepare(text, &mut evicted).await?;
                let fresh = (prepared.columns.clone(), prepared.formats.clone());
                let id = prepared.id;
                if self.cache_limit > 0 {
                    self.statements.insert(text.to_string(), prepared);
                }
                (id, Some(fresh))
            }
        };

        let mut out = Out::with_capacity(64 + params.len());
        {
            let (name, formats) = match self.statements.get(text) {
                Some(p) => (p.name.as_str(), p.formats.as_slice()),
                // Caching off: the unnamed statement, just prepared.
                None => ("", fresh.as_ref().map(|f| f.1.as_slice()).unwrap_or(&[])),
            };
            wire::bind(&mut out, name, params, formats);
        }
        wire::execute(&mut out);
        wire::sync(&mut out);
        self.write(out).await?;

        loop {
            let message = self.message().await?;
            match message.tag {
                back::BIND_COMPLETE => break,
                back::ERROR_RESPONSE => {
                    let fields = self.server_fields(message)?;
                    self.drain_to_ready().await?;
                    return Err(Failure::Server(fields));
                }
                _ => self.observe(message)?,
            }
        }
        self.streaming = true;
        let first = self.batch(max_bytes, discard).await?;
        Ok(Started {
            statement,
            fresh,
            evicted,
            first,
        })
    }

    /// Prepares `text`, learning its shape: the result formats go in `Bind`, so
    /// they have to be known before it, which only a statement `Describe` can
    /// say. One extra round trip the first time a statement is seen.
    async fn prepare(&mut self, text: &str, evicted: &mut Vec<u64>) -> Result<Prepared, Failure> {
        let mut out = Out::default();
        if self.cache_limit > 0 {
            while self.statements.len() >= self.cache_limit {
                let Some(oldest) = self
                    .statements
                    .iter()
                    .min_by_key(|(_, p)| p.last_used)
                    .map(|(k, _)| k.clone())
                else {
                    break;
                };
                let gone = self
                    .statements
                    .remove(&oldest)
                    .expect("the key was just found");
                wire::close_statement(&mut out, &gone.name);
                evicted.push(gone.id);
            }
        }
        self.next_statement += 1;
        let id = self.next_statement;
        let name = if self.cache_limit > 0 {
            format!("esrun_s{id}")
        } else {
            String::new()
        };
        wire::parse(&mut out, &name, text);
        wire::describe_statement(&mut out, &name);
        wire::sync(&mut out);
        self.write(out).await?;

        let mut columns = None;
        let mut failure = None;
        loop {
            let message = self.message().await?;
            match message.tag {
                back::PARSE_COMPLETE | back::PARAMETER_DESCRIPTION | back::CLOSE_COMPLETE => {}
                back::ROW_DESCRIPTION => {
                    let parsed = read_row_description(self.inbox.slice(message.body));
                    columns = Some(Arc::new(parsed.map_err(|e| self.die(e))?));
                }
                back::NO_DATA => columns = None,
                back::ERROR_RESPONSE => failure = Some(self.server_fields(message)?),
                back::READY_FOR_QUERY => {
                    self.ready(message)?;
                    break;
                }
                _ => self.observe(message)?,
            }
        }
        if let Some(fields) = failure {
            return Err(Failure::Server(fields));
        }
        // Binary where it is cheaper to read than text, and text where it is
        // not. An all-text statement sends no format list at all.
        let mut formats: Vec<i16> = columns
            .as_ref()
            .map(|c| {
                c.oids
                    .iter()
                    .map(|oid| i16::from(self.binary_oids.contains(oid)))
                    .collect()
            })
            .unwrap_or_default();
        if !formats.contains(&1) {
            formats.clear();
        }
        Ok(Prepared {
            id,
            name,
            columns,
            formats,
            last_used: self.tick,
        })
    }

    /// Gathers rows until the batch holds `max_bytes` or the statement ends.
    async fn batch(&mut self, max_bytes: usize, discard: bool) -> Result<Batch, Failure> {
        let mut bytes = if discard {
            Vec::new()
        } else {
            Vec::with_capacity(self.inbox.buffered().min(max_bytes))
        };
        let mut rows = 0u32;
        loop {
            let message = self.message().await?;
            match message.tag {
                back::DATA_ROW => {
                    rows += 1;
                    if !discard {
                        // A DataRow frame is already the shared row encoding:
                        // copied as-is, never transcoded (D56).
                        bytes.extend_from_slice(self.inbox.slice(message.framed));
                        if bytes.len() >= max_bytes {
                            return Ok(Batch {
                                bytes,
                                rows,
                                done: false,
                                tag: String::new(),
                            });
                        }
                    }
                }
                back::COMMAND_COMPLETE | back::EMPTY_QUERY => {
                    let tag = if message.tag == back::COMMAND_COMPLETE {
                        let parsed = Fields::new(self.inbox.slice(message.body)).cstr();
                        parsed.map_err(|e| self.die(e))?
                    } else {
                        String::new()
                    };
                    self.drain_to_ready().await?;
                    self.streaming = false;
                    return Ok(Batch {
                        bytes,
                        rows,
                        done: true,
                        tag,
                    });
                }
                back::ERROR_RESPONSE => {
                    let fields = self.server_fields(message)?;
                    self.drain_to_ready().await?;
                    self.streaming = false;
                    return Err(Failure::Server(fields));
                }
                back::PORTAL_SUSPENDED => {
                    return Ok(Batch {
                        bytes,
                        rows,
                        done: false,
                        tag: String::new(),
                    });
                }
                _ => self.observe(message)?,
            }
        }
    }

    /// Reads and discards up to `ReadyForQuery`, so the connection is reusable.
    /// An error while draining is not thrown: the caller is already unwinding.
    async fn drain_to_ready(&mut self) -> Result<(), Failure> {
        loop {
            let message = self.message().await?;
            match message.tag {
                back::READY_FOR_QUERY => return self.ready(message),
                back::ERROR_RESPONSE | back::DATA_ROW | back::COMMAND_COMPLETE => {}
                _ => self.observe(message)?,
            }
        }
    }

    fn ready(&mut self, message: Message) -> Result<(), Failure> {
        let parsed = Fields::new(self.inbox.slice(message.body)).u8();
        self.status = parsed.map_err(|e| self.die(e))?;
        Ok(())
    }

    /// What the server may send between any two messages of an exchange.
    fn observe(&mut self, message: Message) -> Result<(), Failure> {
        match message.tag {
            back::PARAMETER_STATUS => {
                let parsed = read_parameter(self.inbox.slice(message.body));
                let pair = parsed.map_err(|e| self.die(e))?;
                self.aside.parameters.push(pair);
            }
            back::NOTICE_RESPONSE => {
                let fields = self.server_fields(message)?;
                self.aside.notices.push(fields);
            }
            // Notifications arrive only on a subscribed connection, which is
            // the next phase (D147); anything else is not ours to act on.
            back::NOTIFICATION => {}
            _ => {}
        }
        Ok(())
    }

    fn server_fields(&mut self, message: Message) -> Result<Vec<(u8, String)>, Failure> {
        let parsed = read_server_message(self.inbox.slice(message.body));
        parsed.map_err(|e| self.die(e))
    }

    fn check(&self) -> Result<(), Failure> {
        if let Some(message) = &self.fatal {
            return Err(Failure::Lost(message.clone()));
        }
        if self.streaming {
            return Err(Failure::Busy(
                "this connection is streaming a result set — finish it (await rows.toArray(), or let the for-await end), or run the second query on another connection".into(),
            ));
        }
        Ok(())
    }

    async fn write(&mut self, out: Out) -> Result<(), Failure> {
        if let Some(message) = &self.fatal {
            return Err(Failure::Lost(message.clone()));
        }
        self.net
            .write(self.socket, out.into_bytes())
            .await
            .map_err(|e| self.die(provider_message(&e)))
    }

    async fn fill(&mut self) -> Result<(), Failure> {
        match self.net.read(self.socket).await {
            Ok(Some(chunk)) => {
                self.inbox.push(&chunk);
                Ok(())
            }
            Ok(None) => Err(self.die("the server closed the connection".into())),
            Err(e) => Err(self.die(provider_message(&e))),
        }
    }

    async fn message(&mut self) -> Result<Message, Failure> {
        loop {
            match self.inbox.next() {
                Ok(Some(message)) => return Ok(message),
                Ok(None) => self.fill().await?,
                Err(e) => return Err(self.die(e)),
            }
        }
    }

    /// Latches the first transport failure. Nothing is streaming any more,
    /// whatever a result set believes.
    fn die(&mut self, detail: String) -> Failure {
        let message = self
            .fatal
            .get_or_insert_with(|| format!("the connection to the server was lost: {detail}"))
            .clone();
        self.streaming = false;
        Failure::Lost(message)
    }
}

/// An `Authentication` message: its kind, and whatever follows it.
fn read_authentication(body: &[u8]) -> Result<(i32, Vec<u8>), String> {
    let mut fields = Fields::new(body);
    let kind = fields.i32()?;
    Ok((kind, fields.rest().to_vec()))
}

fn read_backend_key(body: &[u8]) -> Result<(i32, i32), String> {
    let mut fields = Fields::new(body);
    Ok((fields.i32()?, fields.i32()?))
}

fn read_parameter(body: &[u8]) -> Result<(String, String), String> {
    let mut fields = Fields::new(body);
    Ok((fields.cstr()?, fields.cstr()?))
}

/// The fields of an `ErrorResponse` or `NoticeResponse`, by their one-byte
/// codes (`S`everity, `C`ode, `M`essage, `D`etail, `H`int, …).
fn read_server_message(body: &[u8]) -> Result<Vec<(u8, String)>, String> {
    let mut fields = Fields::new(body);
    let mut out = Vec::new();
    loop {
        let kind = match fields.u8() {
            Ok(0) => break,
            Ok(kind) => kind,
            // Ran off the end: what was read is what there is.
            Err(_) => break,
        };
        out.push((kind, fields.cstr()?));
    }
    Ok(out)
}

fn read_row_description(body: &[u8]) -> Result<Columns, String> {
    let mut fields = Fields::new(body);
    let count = fields.i16()?.max(0) as usize;
    let mut names = Vec::with_capacity(count);
    let mut oids = Vec::with_capacity(count);
    for _ in 0..count {
        names.push(fields.cstr()?);
        fields.i32()?; // table OID
        fields.i16()?; // column number
        oids.push(fields.i32()? as u32);
        fields.i16()?; // type size
        fields.i32()?; // type modifier
        fields.i16()?; // format
    }
    Ok(Columns { names, oids })
}

fn is_stale_plan(fields: &[(u8, String)]) -> bool {
    fields
        .iter()
        .any(|(kind, value)| *kind == b'C' && STALE_PLAN.contains(&value.as_str()))
}

fn provider_message(e: &ProviderError) -> String {
    e.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_row_description_yields_names_and_type_oids() {
        let mut body = vec![0, 2];
        for (name, oid) in [("a", 23i32), ("b", 25)] {
            body.extend_from_slice(name.as_bytes());
            body.push(0);
            body.extend_from_slice(&0i32.to_be_bytes());
            body.extend_from_slice(&0i16.to_be_bytes());
            body.extend_from_slice(&oid.to_be_bytes());
            body.extend_from_slice(&4i16.to_be_bytes());
            body.extend_from_slice(&(-1i32).to_be_bytes());
            body.extend_from_slice(&0i16.to_be_bytes());
        }
        let columns = read_row_description(&body).unwrap();
        assert_eq!(columns.names, ["a", "b"]);
        assert_eq!(columns.oids, [23, 25]);
    }

    #[test]
    fn only_plan_invalidation_codes_count_as_stale() {
        assert!(is_stale_plan(&[(b'C', "0A000".into())]));
        assert!(is_stale_plan(&[(b'M', "x".into()), (b'C', "26000".into())]));
        assert!(!is_stale_plan(&[(b'C', "23505".into())]));
    }
}
