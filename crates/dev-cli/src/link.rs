//! The link between `esdev start` and the servers it runs (DECISIONS D207).
//!
//! One connection per child, from the child to the leader, over a local
//! socket — a Unix socket in a directory only this user can enter, or a named
//! pipe on Windows — opened with a one-time token. Then JSON, one message per
//! line, in both directions:
//!
//! | From | Message | |
//! | --- | --- | --- |
//! | child | `{"type":"hello","version":1,"token":…,"pid":…}` | first, or the leader hangs up |
//! | child | `{"type":"ready","port":…}` | a `serve()` bound |
//! | child | `{"type":"error","context":…,"name":…,"message":…,"stack":…}` | an unhandled error |
//! | child | `{"type":"crash","message":…}` | the run failed and the child is exiting |
//! | leader | `{"type":"stop"}` | drain and exit, as `SIGTERM` would |
//!
//! A closed connection means the other side is gone: the leader learns a
//! child died, and a child whose leader died drains and exits rather than
//! outliving it. Nothing here runs under `esrun`.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

/// The protocol's version, sent in `hello`. A leader refuses another.
pub const VERSION: u64 = 1;

/// The flag naming the endpoint: `--dev-link=<endpoint>`.
pub const FLAG: &str = "--dev-link";

/// The variable carrying the token. An environment variable rather than a
/// flag because a command line is readable by every user on the machine, and
/// a process's environment only by its owner.
pub const TOKEN_VAR: &str = "ESDEV_LINK_TOKEN";

/// How long a connection has to say `hello` before the leader hangs up.
const HELLO_WITHIN: Duration = Duration::from_secs(5);

/// The longest line read: a stack is long, an endless line is an attack.
const LINE_LIMIT: usize = 256 * 1024;

/// What a child said, as the leader hears it.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// `serve()` bound this port.
    Ready { pid: u32, port: u16 },
    /// An unhandled error, as the overlay shows it.
    Error { pid: u32, text: String },
    /// The run failed; the child is exiting.
    Crash { pid: u32, message: String },
    /// The connection closed: the child exited, or never will again speak.
    Gone { pid: u32 },
}

trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}

/// The leader's half: the endpoint children connect to, what they said, and a
/// way to ask one to stop.
pub struct Leader {
    endpoint: String,
    token: String,
    events: mpsc::UnboundedReceiver<Event>,
    /// The `stop` sender for each connected child, by pid.
    stops: Arc<Mutex<std::collections::HashMap<u32, mpsc::UnboundedSender<()>>>>,
    /// The private directory the socket lives in, removed on drop (Unix).
    #[cfg(unix)]
    dir: std::path::PathBuf,
}

impl Drop for Leader {
    fn drop(&mut self) {
        #[cfg(unix)]
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Leader {
    /// Opens the endpoint and starts accepting. Must run inside a tokio
    /// runtime.
    pub fn bind() -> Result<Leader, String> {
        let token = random_hex(16)?;
        let (tx, events) = mpsc::unbounded_channel();
        let stops = Arc::new(Mutex::new(std::collections::HashMap::new()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            let dir = std::env::temp_dir().join(format!("esdev-link-{}", random_hex(8)?));
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(&dir)
                .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
            let path = dir.join("link.sock");
            let listener = tokio::net::UnixListener::bind(&path)
                .map_err(|e| format!("cannot listen on {}: {e}", path.display()))?;
            let accept = (token.clone(), tx, stops.clone());
            tokio::spawn(async move {
                while let Ok((stream, _)) = listener.accept().await {
                    let (token, tx, stops) = accept.clone();
                    tokio::spawn(serve_child(Box::new(stream), token, tx, stops));
                }
            });
            Ok(Leader {
                endpoint: path.display().to_string(),
                token,
                events,
                stops,
                dir,
            })
        }
        #[cfg(windows)]
        {
            use tokio::net::windows::named_pipe::ServerOptions;
            let name = format!(r"\\.\pipe\esdev-link-{}", random_hex(16)?);
            let first = ServerOptions::new()
                .first_pipe_instance(true)
                .reject_remote_clients(true)
                .create(&name)
                .map_err(|e| format!("cannot open {name}: {e}"))?;
            let accept = (token.clone(), tx, stops.clone(), name.clone());
            tokio::spawn(async move {
                let (token, tx, stops, name) = accept;
                let mut server = first;
                loop {
                    if server.connect().await.is_err() {
                        return;
                    }
                    let connected = server;
                    server = match ServerOptions::new()
                        .reject_remote_clients(true)
                        .create(&name)
                    {
                        Ok(next) => next,
                        Err(_) => return,
                    };
                    tokio::spawn(serve_child(
                        Box::new(connected),
                        token.clone(),
                        tx.clone(),
                        stops.clone(),
                    ));
                }
            });
            Ok(Leader {
                endpoint: name,
                token,
                events,
                stops,
            })
        }
    }

    /// The endpoint and token a child is started with.
    pub fn child_args(&self) -> (String, (&'static str, String)) {
        (
            format!("{FLAG}={}", self.endpoint),
            (TOKEN_VAR, self.token.clone()),
        )
    }

    /// The next thing a child said.
    pub async fn next(&mut self) -> Option<Event> {
        self.events.recv().await
    }

    /// Asks the child `pid` to drain and exit. `false` when it has no link,
    /// so the caller falls back to a signal.
    pub fn stop(&self, pid: u32) -> bool {
        self.stops
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&pid)
            .is_some_and(|tx| tx.send(()).is_ok())
    }
}

/// One child's connection: `hello` within the deadline, then its messages
/// until it closes, with `stop` written whenever the leader asks.
async fn serve_child(
    stream: Box<dyn Stream>,
    token: String,
    events: mpsc::UnboundedSender<Event>,
    stops: Arc<Mutex<std::collections::HashMap<u32, mpsc::UnboundedSender<()>>>>,
) {
    let (read, mut write) = tokio::io::split(stream);
    let mut lines = BufReader::new(read);
    let Ok(Some(hello)) = tokio::time::timeout(HELLO_WITHIN, read_message(&mut lines)).await else {
        return;
    };
    let pid = match (
        hello["type"].as_str(),
        hello["version"].as_u64(),
        hello["token"].as_str(),
        hello["pid"].as_u64(),
    ) {
        (Some("hello"), Some(VERSION), Some(given), Some(pid)) if same(given, &token) => pid as u32,
        _ => return,
    };
    let (stop_tx, mut stop_rx) = mpsc::unbounded_channel();
    stops
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(pid, stop_tx);
    loop {
        tokio::select! {
            message = read_message(&mut lines) => {
                let Some(message) = message else { break };
                if let Some(event) = event_of(pid, &message)
                    && events.send(event).is_err()
                {
                    break;
                }
            }
            Some(()) = stop_rx.recv() => {
                if write.write_all(b"{\"type\":\"stop\"}\n").await.is_err() {
                    break;
                }
                let _ = write.flush().await;
            }
        }
    }
    stops.lock().unwrap_or_else(|e| e.into_inner()).remove(&pid);
    let _ = events.send(Event::Gone { pid });
}

/// A child message as an [`Event`]; `None` for one this leader does not know,
/// so a newer child's extra messages are ignored rather than fatal.
fn event_of(pid: u32, message: &Value) -> Option<Event> {
    let text = |key: &str| message[key].as_str().unwrap_or("").to_string();
    match message["type"].as_str()? {
        "ready" => Some(Event::Ready {
            pid,
            port: u16::try_from(message["port"].as_u64()?).ok()?,
        }),
        "error" => {
            let stack = text("stack");
            let body = if stack.is_empty() {
                format!("{}: {}", text("name"), text("message"))
            } else {
                stack
            };
            let text = match message["context"].as_str() {
                Some(context) => format!("{context}\n{body}"),
                None => body,
            };
            Some(Event::Error { pid, text })
        }
        "crash" => Some(Event::Crash {
            pid,
            message: text("message"),
        }),
        _ => None,
    }
}

/// One JSON line, or `None` at the end of the stream or on anything that is
/// not one.
async fn read_message<R: AsyncBufReadExt + Unpin>(lines: &mut R) -> Option<Value> {
    let mut line = Vec::new();
    loop {
        let buf = lines.fill_buf().await.ok()?;
        if buf.is_empty() {
            return None;
        }
        match buf.iter().position(|&b| b == b'\n') {
            Some(i) => {
                line.extend_from_slice(&buf[..i]);
                lines.consume(i + 1);
                break;
            }
            None => {
                let n = buf.len();
                line.extend_from_slice(buf);
                lines.consume(n);
            }
        }
        if line.len() > LINE_LIMIT {
            return None;
        }
    }
    serde_json::from_slice(&line).ok()
}

/// Compares the token without stopping at the first difference.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

fn random_hex(bytes: usize) -> Result<String, String> {
    use es_runtime_providers::Entropy;
    let mut buf = vec![0u8; bytes];
    es_runtime_default_providers::OsEntropy
        .fill(&mut buf)
        .map_err(|e| format!("no randomness for the link: {e}"))?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

/// The child's half: connects, says `hello`, and then reports — `ready` as
/// `serve()` binds, `error` as the console hears one — and drains on `stop` or
/// when the leader goes away.
pub struct Child {
    tx: mpsc::UnboundedSender<String>,
    flushed: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
}

impl Child {
    /// Starts the link to `endpoint` with the token from the environment.
    /// The connection is made by [`Child::run`], once the run has started.
    pub fn new() -> (
        Arc<Child>,
        mpsc::UnboundedReceiver<String>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let (tx, rx) = mpsc::unbounded_channel();
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        (
            Arc::new(Child {
                tx,
                flushed: Mutex::new(Some(done_rx)),
            }),
            rx,
            done_tx,
        )
    }

    /// Queues an `error` message.
    pub fn error(&self, report: &es_runtime_providers::ErrorReport<'_>) {
        let _ = self.tx.send(
            json!({
                "type": "error",
                "context": report.context,
                "name": report.name,
                "message": report.message,
                "stack": report.stack,
            })
            .to_string(),
        );
    }

    /// Sends `crash` and waits, briefly, for it to be written — the process
    /// is about to exit, and a message still queued would be lost with it.
    pub fn crash(&self, message: &str) {
        let _ = self
            .tx
            .send(json!({ "type": "crash", "message": message }).to_string());
        // Closing the queue makes the writer finish and signal.
        let rx = self
            .flushed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(rx) = rx {
            let deadline = std::time::Instant::now() + Duration::from_millis(500);
            let mut rx = rx;
            while std::time::Instant::now() < deadline {
                if !matches!(
                    rx.try_recv(),
                    Err(tokio::sync::oneshot::error::TryRecvError::Empty)
                ) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
}

/// Drives the child's link: connect, `hello`, then forward queued messages and
/// bound ports, and drain on `stop` or a closed connection. Spawned from the
/// run's `on_start`.
pub async fn run_child(
    endpoint: String,
    token: String,
    mut queued: mpsc::UnboundedReceiver<String>,
    flushed: tokio::sync::oneshot::Sender<()>,
    started: es_runtime_cli_common::run::Started,
) {
    let Some(stream) = connect(&endpoint).await else {
        eprintln!("esdev: cannot reach esdev start at {endpoint}; running without it");
        return;
    };
    let (read, mut write) = tokio::io::split(stream);
    let mut lines = BufReader::new(read);
    let hello = json!({
        "type": "hello",
        "version": VERSION,
        "token": token,
        "pid": std::process::id(),
    });
    if write
        .write_all(format!("{hello}\n").as_bytes())
        .await
        .is_err()
    {
        return;
    }
    let drain = |reason: &str| {
        es_runtime_cli_common::shutdown::drain(
            started.bin,
            reason,
            &started.http,
            &started.interrupt,
            started.grace,
            143,
        );
    };
    let mut announced = 0usize;
    let mut flushed = Some(flushed);
    loop {
        tokio::select! {
            () = started.http.port_bound() => {
                let ports = started.http.bound_ports();
                for port in &ports[announced.min(ports.len())..] {
                    let line = format!("{}\n", json!({ "type": "ready", "port": port }));
                    if write.write_all(line.as_bytes()).await.is_err() {
                        return;
                    }
                }
                announced = ports.len();
                let _ = write.flush().await;
            }
            message = queued.recv() => {
                let Some(message) = message else { break };
                if write.write_all(format!("{message}\n").as_bytes()).await.is_err() {
                    break;
                }
                let _ = write.flush().await;
                if message.contains("\"type\":\"crash\"")
                    && let Some(flushed) = flushed.take()
                {
                    let _ = flushed.send(());
                }
            }
            message = read_message(&mut lines) => {
                match message {
                    Some(message) if message["type"] == "stop" => drain("esdev start asked it to stop"),
                    Some(_) => {}
                    // The leader is gone: nobody will restart, stop or show
                    // this process again, so it must not outlive it.
                    None => {
                        drain("esdev start went away");
                        return;
                    }
                }
            }
        }
    }
}

async fn connect(endpoint: &str) -> Option<Box<dyn Stream>> {
    #[cfg(unix)]
    {
        tokio::net::UnixStream::connect(endpoint)
            .await
            .ok()
            .map(|s| Box::new(s) as Box<dyn Stream>)
    }
    #[cfg(windows)]
    {
        use tokio::net::windows::named_pipe::ClientOptions;
        for _ in 0..50 {
            match ClientOptions::new().open(endpoint) {
                Ok(client) => return Some(Box::new(client)),
                // Every instance busy: the leader is between accepts.
                Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
            }
        }
        None
    }
}

/// The console a linked child runs with: everything written as before, and an
/// unhandled error also sent to the leader.
pub struct LinkConsole {
    pub inner: Arc<dyn es_runtime_providers::Console>,
    pub link: Arc<Child>,
}

impl es_runtime_providers::Console for LinkConsole {
    fn write(&self, level: es_runtime_providers::ConsoleLevel, message: &str) {
        self.inner.write(level, message);
    }

    fn report(&self, report: &es_runtime_providers::ErrorReport<'_>) {
        self.inner.report(report);
        self.link.error(report);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_become_events() {
        let error = json!({
            "type": "error", "context": "runtime:http: GET /a failed",
            "name": "RangeError", "message": "x", "stack": "RangeError: x\n    at f",
        });
        assert_eq!(
            event_of(7, &error),
            Some(Event::Error {
                pid: 7,
                text: "runtime:http: GET /a failed\nRangeError: x\n    at f".into()
            })
        );
        assert_eq!(
            event_of(7, &json!({"type": "ready", "port": 5173})),
            Some(Event::Ready { pid: 7, port: 5173 })
        );
        assert_eq!(event_of(7, &json!({"type": "ready", "port": 70000})), None);
        assert_eq!(event_of(7, &json!({"type": "from-the-future"})), None);
    }

    #[test]
    fn the_token_compares_whole() {
        assert!(same("abc", "abc"));
        assert!(!same("abc", "abd"));
        assert!(!same("abc", "abcd"));
    }

    #[tokio::test]
    async fn a_child_without_the_token_is_hung_up_on() {
        let mut leader = Leader::bind().unwrap();
        let stream = connect(&leader.endpoint).await.unwrap();
        let (read, mut write) = tokio::io::split(stream);
        write
            .write_all(b"{\"type\":\"hello\",\"version\":1,\"token\":\"wrong\",\"pid\":1}\n")
            .await
            .unwrap();
        write
            .write_all(b"{\"type\":\"ready\",\"port\":1}\n")
            .await
            .unwrap();
        let mut lines = BufReader::new(read);
        assert!(read_message(&mut lines).await.is_none(), "not hung up on");
        let nothing = tokio::time::timeout(Duration::from_millis(100), leader.next()).await;
        assert!(nothing.is_err(), "an unauthenticated message got through");
    }

    #[tokio::test]
    async fn a_linked_child_is_heard_and_can_be_stopped() {
        let mut leader = Leader::bind().unwrap();
        let stream = connect(&leader.endpoint).await.unwrap();
        let (read, mut write) = tokio::io::split(stream);
        let hello = json!({"type":"hello","version":1,"token": leader.token.clone(),"pid":42});
        write
            .write_all(format!("{hello}\n").as_bytes())
            .await
            .unwrap();
        write
            .write_all(b"{\"type\":\"ready\",\"port\":8080}\n")
            .await
            .unwrap();
        assert_eq!(
            leader.next().await,
            Some(Event::Ready {
                pid: 42,
                port: 8080
            })
        );
        assert!(leader.stop(42));
        let mut lines = BufReader::new(read);
        assert_eq!(read_message(&mut lines).await, Some(json!({"type":"stop"})));
        drop(write);
        drop(lines);
        assert_eq!(leader.next().await, Some(Event::Gone { pid: 42 }));
        assert!(!leader.stop(42), "a gone child is still stoppable");
    }
}
