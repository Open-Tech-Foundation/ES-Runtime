//! The built-in MongoDB driver's connections and pool (DECISIONS D186).
//!
//! Rust owns the conversation: the TLS connect, the `hello` handshake, SCRAM
//! authentication, `OP_MSG` framing, and the pool of connections to the server.
//! JavaScript (`runtime:db`) owns values — encoding commands and decoding
//! replies with `BSON` — the connection string, and the collection API. A
//! command is one op: the pool picks a connection inside it.
//!
//! It reaches the server through the agent's [`NetProvider`], so the host
//! allowlist, the TLS roots and socket ownership apply as they do to
//! `runtime:net`.

mod pool;
pub(crate) mod sdam;
pub(crate) mod select;
#[cfg(test)]
mod spec_tests;
mod topology;
mod wire;

use std::sync::Arc;

use es_runtime_bson::{Document, Element, Writer, kind};
use es_runtime_common::ErrorCode;
use es_runtime_providers::{ConnectOptions, Entropy, NetProvider, ProviderError};

use crate::scram::{Hash, Scram};

pub(crate) use topology::{Selector, Topology};
pub(crate) use wire::Sequence;

/// MongoDB 7.0's wire version: the oldest server this driver speaks to.
const MIN_WIRE_VERSION: i32 = 21;

/// The fewest SCRAM iterations a server may ask for, as the authentication
/// specification requires.
const MIN_SCRAM_ITERATIONS: u32 = 4096;

/// Everything a connection needs, decided by the driver in JavaScript.
#[derive(Clone)]
pub(crate) struct Options {
    pub host: String,
    pub port: u16,
    pub tls: bool,
    /// Extra trust anchors, PEM.
    pub ca: Vec<u8>,
    pub user: Option<String>,
    /// The password as given, for SCRAM-SHA-1's digest.
    pub password: String,
    /// The password normalized (NFKC), for SCRAM-SHA-256.
    pub password_prepared: String,
    pub auth_source: String,
    /// `SCRAM-SHA-1`, `SCRAM-SHA-256`, or `None` to take what the server offers.
    pub mechanism: Option<String>,
    pub app_name: Option<String>,
    pub max_pool: usize,
}

/// Why an operation failed.
#[derive(Debug)]
pub(crate) enum Failure {
    /// The server answered the handshake or authentication with an error.
    Server {
        code: i32,
        code_name: String,
        message: String,
    },
    Lost(String),
    Auth(String),
    Unsupported(String),
    /// The pool was closed.
    Closed,
    /// The command was stopped by a cancel.
    Cancelled,
    /// No server can take the operation now; the message says why.
    Selection(String),
    /// The provider refused to connect — an address outside the allowlist.
    Refused(ProviderError),
}

/// What `hello` said that the driver acts on.
#[derive(Clone)]
pub(crate) struct Server {
    /// The whole `hello` reply.
    pub hello: Vec<u8>,
}

pub(crate) struct Connection {
    net: Arc<dyn NetProvider>,
    socket: u64,
    inbox: wire::Inbox,
    next_request: i32,
    max_message: usize,
    fatal: Option<String>,
}

impl Connection {
    /// Opens a connection and completes the handshake, authenticating when
    /// `authenticate` is set (a monitoring connection does not); a failure
    /// closes the socket before it returns. `opened` learns the socket id as soon
    /// as there is one, so a timeout or a cancel can close it.
    pub(crate) async fn open(
        net: Arc<dyn NetProvider>,
        entropy: &dyn Entropy,
        options: &Options,
        opened: &std::sync::Mutex<Option<u64>>,
        authenticate: bool,
    ) -> Result<(Connection, Server), Failure> {
        let (socket, _) = net
            .connect(
                options.host.clone(),
                options.port,
                ConnectOptions {
                    secure: options.tls,
                    ca: options.ca.clone(),
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
            inbox: wire::Inbox::default(),
            next_request: 0,
            max_message: wire::DEFAULT_MAX_MESSAGE,
            fatal: None,
        };
        match connection.handshake(entropy, options, authenticate).await {
            Ok(server) => Ok((connection, server)),
            Err(failure) => {
                connection.close().await;
                Err(failure)
            }
        }
    }

    pub(crate) fn socket(&self) -> u64 {
        self.socket
    }

    /// Whether the connection can carry another command.
    pub(crate) fn healthy(&self) -> bool {
        self.fatal.is_none()
    }

    /// Checks the server again on an open connection: its `hello` reply.
    pub(crate) async fn hello(&mut self) -> Result<Vec<u8>, Failure> {
        self.command(&plain_hello()?, None).await
    }

    async fn handshake(
        &mut self,
        entropy: &dyn Entropy,
        options: &Options,
        authenticate: bool,
    ) -> Result<Server, Failure> {
        let hello = self
            .command(&hello_command(options, authenticate)?, None)
            .await?;
        let doc = Document::new(&hello)
            .map_err(|e| Failure::Lost(format!("the server's hello is malformed: {e}")))?;
        check_ok(&doc)?;
        let wire_version = int(&doc, "maxWireVersion").unwrap_or(0);
        if wire_version < MIN_WIRE_VERSION {
            return Err(Failure::Unsupported(format!(
                "the server speaks wire version {wire_version}; this driver needs MongoDB 7.0 or later (wire version {MIN_WIRE_VERSION})"
            )));
        }
        if let Some(max) = int(&doc, "maxMessageSizeBytes").and_then(|n| usize::try_from(n).ok()) {
            self.max_message = max;
        }
        if let (true, Some(user)) = (authenticate, &options.user) {
            let offered = strings(&doc, "saslSupportedMechs");
            let mechanism = match options.mechanism.as_deref() {
                Some(m) => m.to_string(),
                // The stronger one when the server offers it, as the
                // authentication specification negotiates.
                None if offered.is_empty() || offered.iter().any(|m| m == "SCRAM-SHA-256") => {
                    "SCRAM-SHA-256".into()
                }
                None => "SCRAM-SHA-1".into(),
            };
            self.authenticate(entropy, options, user, &mechanism)
                .await?;
        }
        Ok(Server { hello })
    }

    async fn authenticate(
        &mut self,
        entropy: &dyn Entropy,
        options: &Options,
        user: &str,
        mechanism: &str,
    ) -> Result<(), Failure> {
        let (hash, password) = match mechanism {
            "SCRAM-SHA-256" => (Hash::Sha256, options.password_prepared.clone()),
            // SCRAM-SHA-1's password is MongoDB's digest of the user and password.
            "SCRAM-SHA-1" => (
                Hash::Sha1,
                md5_hex(&format!("{user}:mongo:{}", options.password)),
            ),
            other => {
                return Err(Failure::Unsupported(format!(
                    "the {other} mechanism is not supported"
                )));
            }
        };
        let mut nonce = [0u8; 24];
        entropy
            .fill(&mut nonce)
            .map_err(|_| Failure::Auth("the entropy provider failed".into()))?;
        let mut scram = Scram::new(hash, MIN_SCRAM_ITERATIONS, &nonce, user);

        let source = options.auth_source.as_str();
        let start = self
            .sasl(&sasl_command(
                "saslStart",
                None,
                Some(mechanism),
                scram.initial().as_bytes(),
                source,
            )?)
            .await?;
        let server_first = start.payload;
        let client_final = scram
            .respond(&password, &String::from_utf8_lossy(&server_first))
            .map_err(Failure::Auth)?;
        let mut step = self
            .sasl(&sasl_command(
                "saslContinue",
                Some(start.conversation),
                None,
                client_final.as_bytes(),
                source,
            )?)
            .await?;
        scram
            .verify(&String::from_utf8_lossy(&step.payload))
            .map_err(Failure::Auth)?;
        // A server that did not take `skipEmptyExchange` wants one more, empty,
        // round before it says it is done.
        if !step.done {
            step = self
                .sasl(&sasl_command(
                    "saslContinue",
                    Some(start.conversation),
                    None,
                    b"",
                    source,
                )?)
                .await?;
            if !step.done {
                return Err(Failure::Auth(
                    "the server did not finish the SCRAM exchange".into(),
                ));
            }
        }
        Ok(())
    }

    async fn sasl(&mut self, command: &[u8]) -> Result<Sasl, Failure> {
        let reply = self.command(command, None).await?;
        let doc = Document::new(&reply)
            .map_err(|e| Failure::Lost(format!("a SASL reply is malformed: {e}")))?;
        if let Err(Failure::Server { message, .. }) = check_ok(&doc) {
            return Err(Failure::Auth(message));
        }
        let payload = match doc.get("payload") {
            Ok(Some(Element::Binary { bytes, .. })) => bytes.to_vec(),
            _ => Vec::new(),
        };
        Ok(Sasl {
            conversation: int(&doc, "conversationId").unwrap_or(0),
            payload,
            done: matches!(doc.get("done"), Ok(Some(Element::Boolean(true)))),
        })
    }

    /// Sends one command and returns the reply document, whatever its `ok`.
    pub(crate) async fn command(
        &mut self,
        body: &[u8],
        sequence: Option<&Sequence>,
    ) -> Result<Vec<u8>, Failure> {
        if let Some(fatal) = &self.fatal {
            return Err(Failure::Lost(fatal.clone()));
        }
        self.next_request = self.next_request.wrapping_add(1);
        let request = self.next_request;
        let message = wire::message(request, body, sequence).map_err(Failure::Unsupported)?;
        if message.len() > self.max_message {
            return Err(Failure::Unsupported(format!(
                "a command of {} bytes is larger than the server's {}-byte limit",
                message.len(),
                self.max_message
            )));
        }
        if let Err(e) = self.net.write(self.socket, message).await {
            return Err(self.die(format!("writing to the server failed: {e}")));
        }
        loop {
            match self.inbox.take(self.max_message) {
                Ok(Some(message)) => {
                    return wire::reply(&message, request).map_err(|e| self.die(e));
                }
                Ok(None) => {}
                Err(e) => return Err(self.die(e)),
            }
            match self.net.read(self.socket).await {
                Ok(Some(chunk)) => self.inbox.push(&chunk),
                Ok(None) => return Err(self.die("the server closed the connection".into())),
                Err(e) => return Err(self.die(format!("reading from the server failed: {e}"))),
            }
        }
    }

    pub(crate) async fn close(&mut self) {
        let _ = self.net.close(self.socket).await;
        self.fatal
            .get_or_insert_with(|| "the connection is closed".into());
    }

    fn die(&mut self, detail: String) -> Failure {
        let message = self
            .fatal
            .get_or_insert_with(|| format!("the connection to the server was lost: {detail}"))
            .clone();
        Failure::Lost(message)
    }
}

struct Sasl {
    conversation: i32,
    payload: Vec<u8>,
    done: bool,
}

/// The first `hello`: the client's metadata, and, when it will authenticate,
/// a request for the mechanisms this user may use.
fn hello_command(options: &Options, authenticate: bool) -> Result<Vec<u8>, Failure> {
    let bad = |e: es_runtime_bson::Error| Failure::Unsupported(e.to_string());
    let mut w = Writer::new();
    w.start_document().map_err(bad)?;
    w.key(kind::INT32, "hello").map_err(bad)?;
    w.int32(1);
    w.key(kind::DOCUMENT, "client").map_err(bad)?;
    w.start_document().map_err(bad)?;
    if let Some(app) = &options.app_name {
        w.key(kind::DOCUMENT, "application").map_err(bad)?;
        w.start_document().map_err(bad)?;
        w.key(kind::STRING, "name").map_err(bad)?;
        w.string(app).map_err(bad)?;
        w.end_document().map_err(bad)?;
    }
    w.key(kind::DOCUMENT, "driver").map_err(bad)?;
    w.start_document().map_err(bad)?;
    w.key(kind::STRING, "name").map_err(bad)?;
    w.string("esrun").map_err(bad)?;
    w.key(kind::STRING, "version").map_err(bad)?;
    w.string(env!("CARGO_PKG_VERSION")).map_err(bad)?;
    w.end_document().map_err(bad)?;
    w.key(kind::DOCUMENT, "os").map_err(bad)?;
    w.start_document().map_err(bad)?;
    w.key(kind::STRING, "type").map_err(bad)?;
    w.string(os_type()).map_err(bad)?;
    w.key(kind::STRING, "architecture").map_err(bad)?;
    w.string(std::env::consts::ARCH).map_err(bad)?;
    w.end_document().map_err(bad)?;
    w.key(kind::STRING, "platform").map_err(bad)?;
    w.string(concat!("esrun ", env!("CARGO_PKG_VERSION")))
        .map_err(bad)?;
    w.end_document().map_err(bad)?;
    if let (true, Some(user), None) = (authenticate, &options.user, &options.mechanism) {
        w.key(kind::STRING, "saslSupportedMechs").map_err(bad)?;
        w.string(&format!("{}.{user}", options.auth_source))
            .map_err(bad)?;
    }
    w.key(kind::STRING, "$db").map_err(bad)?;
    w.string("admin").map_err(bad)?;
    w.end_document().map_err(bad)?;
    w.finish().map_err(bad)
}

/// A later `hello`, without the handshake's client metadata.
fn plain_hello() -> Result<Vec<u8>, Failure> {
    let bad = |e: es_runtime_bson::Error| Failure::Unsupported(e.to_string());
    let mut w = Writer::new();
    w.start_document().map_err(bad)?;
    w.key(kind::INT32, "hello").map_err(bad)?;
    w.int32(1);
    w.key(kind::STRING, "$db").map_err(bad)?;
    w.string("admin").map_err(bad)?;
    w.end_document().map_err(bad)?;
    w.finish().map_err(bad)
}

/// The handshake specification's `os.type` names.
fn os_type() -> &'static str {
    match std::env::consts::OS {
        "linux" => "Linux",
        "macos" => "Darwin",
        "windows" => "Windows",
        other => other,
    }
}

fn sasl_command(
    name: &str,
    conversation: Option<i32>,
    mechanism: Option<&str>,
    payload: &[u8],
    source: &str,
) -> Result<Vec<u8>, Failure> {
    let bad = |e: es_runtime_bson::Error| Failure::Unsupported(e.to_string());
    let mut w = Writer::new();
    w.start_document().map_err(bad)?;
    w.key(kind::INT32, name).map_err(bad)?;
    w.int32(1);
    if let Some(conversation) = conversation {
        w.key(kind::INT32, "conversationId").map_err(bad)?;
        w.int32(conversation);
    }
    if let Some(mechanism) = mechanism {
        w.key(kind::STRING, "mechanism").map_err(bad)?;
        w.string(mechanism).map_err(bad)?;
    }
    w.key(kind::BINARY, "payload").map_err(bad)?;
    w.binary(0, payload).map_err(bad)?;
    if mechanism.is_some() {
        w.key(kind::DOCUMENT, "options").map_err(bad)?;
        w.start_document().map_err(bad)?;
        w.key(kind::BOOLEAN, "skipEmptyExchange").map_err(bad)?;
        w.boolean(true);
        w.end_document().map_err(bad)?;
    }
    w.key(kind::STRING, "$db").map_err(bad)?;
    w.string(source).map_err(bad)?;
    w.end_document().map_err(bad)?;
    w.finish().map_err(bad)
}

/// `ok: 1`, or the server's error.
fn check_ok(doc: &Document<'_>) -> Result<(), Failure> {
    let ok = match doc.get("ok") {
        Ok(Some(Element::Double(n))) => n == 1.0,
        Ok(Some(Element::Int32(n))) => n == 1,
        Ok(Some(Element::Int64(n))) => n == 1,
        Ok(Some(Element::Boolean(b))) => b,
        _ => false,
    };
    if ok {
        return Ok(());
    }
    let text = |key| match doc.get(key) {
        Ok(Some(Element::String(s))) => s.to_string(),
        _ => String::new(),
    };
    Err(Failure::Server {
        code: int(doc, "code").unwrap_or(0),
        code_name: text("codeName"),
        message: text("errmsg"),
    })
}

/// An integer field, whichever width the server sent it as.
fn int(doc: &Document<'_>, key: &str) -> Option<i32> {
    match doc.get(key) {
        Ok(Some(Element::Int32(n))) => Some(n),
        Ok(Some(Element::Int64(n))) => i32::try_from(n).ok(),
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a server count sent as a double"
        )]
        Ok(Some(Element::Double(n))) => Some(n as i32),
        _ => None,
    }
}

fn strings(doc: &Document<'_>, key: &str) -> Vec<String> {
    match doc.get(key) {
        Ok(Some(Element::Array(list))) => list
            .iter()
            .filter_map(|e| match e {
                Ok((_, Element::String(s))) => Some(s.to_string()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn md5_hex(text: &str) -> String {
    use md5::Digest as _;
    es_runtime_bson::to_hex(&md5::Md5::digest(text.as_bytes()))
}

fn connect_failure(e: ProviderError) -> Failure {
    if e.code() == Some(ErrorCode::PermissionDenied) {
        Failure::Refused(e)
    } else {
        Failure::Lost(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(user: Option<&str>, mechanism: Option<&str>) -> Options {
        Options {
            host: "h".into(),
            port: 27017,
            tls: false,
            ca: Vec::new(),
            user: user.map(str::to_string),
            password: String::new(),
            password_prepared: String::new(),
            auth_source: "admin".into(),
            mechanism: mechanism.map(str::to_string),
            app_name: Some("app".into()),
            max_pool: 1,
        }
    }

    #[test]
    fn hello_names_the_driver_and_asks_for_mechanisms_only_when_it_will_negotiate() {
        let bytes = hello_command(&options(Some("u"), None), true).unwrap();
        let doc = Document::new(&bytes).unwrap();
        assert_eq!(doc.iter().next().unwrap().unwrap().0, "hello");
        assert_eq!(
            doc.get("saslSupportedMechs").unwrap(),
            Some(Element::String("admin.u"))
        );
        assert_eq!(doc.get("$db").unwrap(), Some(Element::String("admin")));
        let Some(Element::Document(client)) = doc.get("client").unwrap() else {
            panic!()
        };
        let Some(Element::Document(driver)) = client.get("driver").unwrap() else {
            panic!()
        };
        assert_eq!(driver.get("name").unwrap(), Some(Element::String("esrun")));

        let bytes = hello_command(&options(Some("u"), Some("SCRAM-SHA-1")), true).unwrap();
        assert_eq!(
            Document::new(&bytes)
                .unwrap()
                .get("saslSupportedMechs")
                .unwrap(),
            None
        );
        // A monitoring connection does not authenticate, so does not ask.
        let bytes = hello_command(&options(Some("u"), None), false).unwrap();
        assert_eq!(
            Document::new(&bytes)
                .unwrap()
                .get("saslSupportedMechs")
                .unwrap(),
            None
        );
    }

    #[test]
    fn ok_is_read_in_any_numeric_width_and_an_error_keeps_its_code() {
        let mut w = Writer::new();
        w.start_document().unwrap();
        w.key(kind::DOUBLE, "ok").unwrap();
        w.double(0.0);
        w.key(kind::STRING, "errmsg").unwrap();
        w.string("Authentication failed.").unwrap();
        w.key(kind::INT32, "code").unwrap();
        w.int32(18);
        w.key(kind::STRING, "codeName").unwrap();
        w.string("AuthenticationFailed").unwrap();
        w.end_document().unwrap();
        let bytes = w.finish().unwrap();
        match check_ok(&Document::new(&bytes).unwrap()) {
            Err(Failure::Server {
                code: 18,
                code_name,
                message,
            }) => {
                assert_eq!(code_name, "AuthenticationFailed");
                assert_eq!(message, "Authentication failed.");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_sha1_password_is_mongodbs_digest() {
        // The digest the authentication specification's example user has.
        assert_eq!(
            md5_hex("user:mongo:pencil"),
            "1c33006ec1ffd90f9cadcbcc0e118200"
        );
    }
}
