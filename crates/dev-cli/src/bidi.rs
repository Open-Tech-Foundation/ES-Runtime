//! A WebDriver BiDi client: a session with a real browser, over the standard
//! protocol and nothing else (D99).
//!
//! **Two ways in, one protocol after.** Firefox serves BiDi itself: started
//! with `--remote-debugging-port`, it prints the WebSocket it listens on, and a
//! `session.new` over that socket is the session. Chrome, Chromium and Edge
//! serve only CDP, and their vendor's driver is what speaks BiDi for them: a
//! classic `POST /session` asking for `webSocketUrl` answers with the socket,
//! and the session already exists on it. Either way what follows is the same
//! JSON over the same socket, and nothing in this file after [`Session::start`]
//! knows which browser it is talking to.
//!
//! **Port 0, read back.** Both the browser and the drivers accept port 0 and
//! print the port they took, so no port is guessed and no two runs collide. The
//! line they print is the only vendor-specific text read here.
//!
//! **Nothing is fetched and nothing is kept.** The executables are the ones
//! [`crate::browser`] found; Firefox gets a fresh profile directory, removed
//! when the session ends, so a run neither reads a developer's profile nor
//! leaves one behind.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;

use crate::browser::{Browser, Launch};

/// How long a browser or driver may take to say where it is listening, and a
/// session to be granted. A cold browser start on a loaded CI machine is
/// seconds, not tens of them; past this something is wrong, and waiting
/// longer only delays saying so.
const STARTUP: Duration = Duration::from_secs(30);

/// A message the browser sent that was not an answer to a command.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub method: String,
    pub params: Value,
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>;

/// One BiDi connection: commands matched to their answers by id, and
/// everything else delivered in order as an [`Event`].
pub struct Client {
    outgoing: mpsc::UnboundedSender<Message>,
    pending: Pending,
    /// Taken once, by whoever routes events — so the client itself can be
    /// shared by everything sending commands.
    events: std::sync::Mutex<Option<mpsc::UnboundedReceiver<Event>>>,
    next: AtomicU64,
    /// Set once the connection has ended, before the commands waiting on it
    /// are failed — so a command sent afterwards fails too, rather than waiting
    /// for an answer nothing will send.
    closed: Arc<std::sync::atomic::AtomicBool>,
}

impl Client {
    /// Connects to a BiDi WebSocket on loopback.
    pub async fn connect(url: &str) -> Result<Client, String> {
        let address = url
            .strip_prefix("ws://")
            .and_then(|rest| rest.split('/').next())
            .ok_or_else(|| format!("not a WebDriver BiDi address: {url}"))?;
        let stream = TcpStream::connect(address)
            .await
            .map_err(|err| format!("cannot reach {url}: {err}"))?;
        let (socket, _) = tokio_tungstenite::client_async(url, stream)
            .await
            .map_err(|err| format!("WebDriver BiDi handshake with {url} failed: {err}"))?;
        let (mut sink, mut stream) = socket.split();

        let (outgoing, mut queue) = mpsc::unbounded_channel::<Message>();
        tokio::spawn(async move {
            while let Some(message) = queue.recv().await {
                if sink.send(message).await.is_err() {
                    break;
                }
            }
        });

        let pending: Pending = Arc::default();
        let (deliver, events) = mpsc::unbounded_channel();
        let answers = Arc::clone(&pending);
        let closed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let ended = Arc::clone(&closed);
        tokio::spawn(async move {
            while let Some(Ok(message)) = stream.next().await {
                let Message::Text(text) = message else {
                    continue;
                };
                let Ok(value) = serde_json::from_str::<Value>(&text) else {
                    continue;
                };
                match route(value) {
                    Routed::Answer(id, result) => {
                        if let Some(waiting) = answers.lock().await.remove(&id) {
                            let _ = waiting.send(result);
                        }
                    }
                    Routed::Event(event) => {
                        let _ = deliver.send(event);
                    }
                    Routed::Neither => {}
                }
            }
            // The socket is gone: every command still waiting will never be
            // answered, and says so rather than hanging the run.
            ended.store(true, Ordering::SeqCst);
            for (_, waiting) in answers.lock().await.drain() {
                let _ = waiting.send(Err("the browser closed the connection".to_string()));
            }
        });

        Ok(Client {
            outgoing,
            pending,
            events: std::sync::Mutex::new(Some(events)),
            next: AtomicU64::new(1),
            closed,
        })
    }

    /// Sends a command and waits for its answer. A BiDi error comes back as
    /// its `error` code and `message`, prefixed with the command that failed.
    pub async fn command(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (answer, answered) = oneshot::channel();
        self.pending.lock().await.insert(id, answer);
        // Checked after the command is registered: either the reader fails it
        // when it drains, or it had already finished draining and this sees
        // the flag it set first.
        if self.closed.load(Ordering::SeqCst) {
            self.pending.lock().await.remove(&id);
            return Err(format!("{method}: the browser closed the connection"));
        }
        let text = json!({ "id": id, "method": method, "params": params }).to_string();
        if self.outgoing.send(Message::text(text)).is_err() {
            self.pending.lock().await.remove(&id);
            return Err(format!("{method}: the browser closed the connection"));
        }
        match answered.await {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(err)) => Err(format!("{method}: {err}")),
            Err(_) => Err(format!("{method}: the browser closed the connection")),
        }
    }

    /// Every event the connection will deliver, in order, ending when it
    /// closes. There is one stream, so only the first call gets it.
    pub fn take_events(&self) -> Option<mpsc::UnboundedReceiver<Event>> {
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }
}

enum Routed {
    Answer(u64, Result<Value, String>),
    Event(Event),
    Neither,
}

/// Sorts one incoming message. `type` says which it is; an answer without one
/// (older drivers omit it on success) is recognised by carrying an `id`.
fn route(mut value: Value) -> Routed {
    let kind = value
        .get("type")
        .and_then(Value::as_str)
        .map(str::to_string);
    if kind.as_deref() == Some("event") {
        let method = value
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let params = value
            .get_mut("params")
            .map(Value::take)
            .unwrap_or(Value::Null);
        return Routed::Event(Event { method, params });
    }
    let Some(id) = value.get("id").and_then(Value::as_u64) else {
        return Routed::Neither;
    };
    if kind.as_deref() == Some("error") || value.get("error").is_some() {
        let code = value
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        let message = value.get("message").and_then(Value::as_str).unwrap_or("");
        let text = if message.is_empty() {
            code.to_string()
        } else {
            format!("{code}: {message}")
        };
        return Routed::Answer(id, Err(text));
    }
    let result = value
        .get_mut("result")
        .map(Value::take)
        .unwrap_or(Value::Null);
    Routed::Answer(id, Ok(result))
}

/// A running browser with a BiDi session on it, and whatever has to be torn
/// down when it ends.
pub struct Session {
    pub client: Arc<Client>,
    /// The browser itself (Firefox) or the driver that started it.
    process: Child,
    /// A driver's classic session — its address, id, and the browser's
    /// process — ended over HTTP so the driver closes the browser it started.
    classic: Option<(String, String, Option<u32>)>,
    /// Firefox's throwaway profile.
    profile: Option<PathBuf>,
}

impl Session {
    /// Starts the browser and opens a session on it. `headless` is the
    /// default; `false` shows the window, for watching a test.
    pub async fn start(launch: &Launch, headless: bool) -> Result<Session, String> {
        let started = async {
            match &launch.driver {
                None => start_direct(launch, headless).await,
                Some(driver) => start_driver(launch, driver, headless).await,
            }
        };
        tokio::time::timeout(STARTUP, started).await.map_err(|_| {
            format!(
                "{} did not start a WebDriver BiDi session within {}s",
                launch.browser,
                STARTUP.as_secs()
            )
        })?
    }

    /// Ends the session and stops the browser. Best effort by design: a
    /// browser that already died has nothing left to close, and the process
    /// is killed either way.
    pub async fn end(mut self) {
        match self.classic.take() {
            Some((address, id, browser_pid)) => {
                let deleted = tokio::time::timeout(
                    Duration::from_secs(10),
                    http(&address, "DELETE", &format!("/session/{id}"), None),
                )
                .await;
                let closed = matches!(deleted, Ok(Ok((status, _))) if (200..300).contains(&status));
                // A driver that could not close its browser — because it
                // died, or hung — leaves it running with nothing to stop it.
                if !closed && let Some(pid) = browser_pid {
                    crate::watch::end(pid);
                }
                // A driver stays up after its session ends; it is ours to stop.
                let _ = self.process.kill().await;
            }
            None => {
                let _ = tokio::time::timeout(
                    Duration::from_secs(10),
                    self.client.command("browser.close", json!({})),
                )
                .await;
                if tokio::time::timeout(Duration::from_secs(5), self.process.wait())
                    .await
                    .is_err()
                {
                    let _ = self.process.kill().await;
                }
            }
        }
        if let Some(profile) = self.profile.take() {
            let _ = std::fs::remove_dir_all(profile);
        }
    }
}

/// Firefox: the browser is the BiDi server.
async fn start_direct(launch: &Launch, headless: bool) -> Result<Session, String> {
    let profile = std::env::temp_dir().join(format!(
        "esdev-browser-{}-{}",
        std::process::id(),
        PROFILES.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&profile)
        .map_err(|err| format!("cannot create a browser profile: {err}"))?;
    let mut command = Command::new(&launch.binary);
    if headless {
        command.arg("--headless");
    }
    command
        .arg("--no-remote")
        .arg("--profile")
        .arg(&profile)
        .args(["--remote-debugging-port", "0", "about:blank"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut process = command
        .spawn()
        .map_err(|err| format!("cannot start {}: {err}", launch.binary.display()))?;
    let output = process.stderr.take().expect("stderr is piped");
    let Some(url) = watch_for(output, listening_url).await else {
        let _ = std::fs::remove_dir_all(&profile);
        return Err(format!(
            "{} exited without serving WebDriver BiDi",
            launch.browser
        ));
    };
    let client = Client::connect(&format!("{url}/session")).await?;
    client
        .command("session.new", json!({ "capabilities": {} }))
        .await?;
    Ok(Session {
        client: Arc::new(client),
        process,
        classic: None,
        profile: Some(profile),
    })
}

static PROFILES: AtomicU64 = AtomicU64::new(0);

/// Chrome, Chromium, Edge: the vendor's driver starts the browser and serves
/// BiDi on its behalf.
async fn start_driver(
    launch: &Launch,
    driver: &std::path::Path,
    headless: bool,
) -> Result<Session, String> {
    let mut process = Command::new(driver)
        .arg("--port=0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|err| format!("cannot start {}: {err}", driver.display()))?;
    let output = process.stdout.take().expect("stdout is piped");
    let port = watch_for(output, driver_port)
        .await
        .ok_or_else(|| format!("{} exited without saying its port", driver.display()))?;
    let address = format!("127.0.0.1:{port}");
    let body = new_session_body(launch, headless);
    let (status, answer) = http(&address, "POST", "/session", Some(&body)).await?;
    let granted = new_session_answer(status, &answer).map_err(|err| {
        format!(
            "{} refused a session for {}: {err}",
            driver.display(),
            launch.binary.display()
        )
    })?;
    let client = Client::connect(&granted.url).await?;
    Ok(Session {
        client: Arc::new(client),
        process,
        classic: Some((address, granted.id, granted.browser_pid)),
        profile: None,
    })
}

/// Reads a child's output line by line until `find` recognises one. The rest
/// of the output keeps being drained, so a chatty browser never blocks on a
/// full pipe.
async fn watch_for<R, T>(output: R, find: fn(&str) -> Option<T>) -> Option<T>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    let mut lines = BufReader::new(output).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if let Some(found) = find(&line) {
            tokio::spawn(async move { while let Ok(Some(_)) = lines.next_line().await {} });
            return Some(found);
        }
    }
    None
}

/// `WebDriver BiDi listening on ws://127.0.0.1:42287` — Firefox's line.
fn listening_url(line: &str) -> Option<String> {
    line.trim()
        .strip_prefix("WebDriver BiDi listening on ")
        .filter(|url| url.starts_with("ws://"))
        .map(|url| url.trim_end_matches('/').to_string())
}

/// `ChromeDriver was started successfully on port 32943.` — and msedgedriver's
/// `Microsoft Edge WebDriver was started successfully on port …`.
fn driver_port(line: &str) -> Option<u16> {
    let (_, rest) = line.split_once("started successfully on port ")?;
    rest.trim().trim_end_matches('.').parse().ok()
}

/// The classic new-session request, asking for the BiDi socket.
fn new_session_body(launch: &Launch, headless: bool) -> String {
    let mut args = vec!["--no-first-run", "--no-default-browser-check"];
    if headless {
        args.push("--headless=new");
    }
    let options = json!({ "binary": launch.binary, "args": args });
    let vendor = match launch.browser {
        Browser::Edge => "ms:edgeOptions",
        _ => "goog:chromeOptions",
    };
    json!({
        "capabilities": {
            "alwaysMatch": { "webSocketUrl": true, vendor: options }
        }
    })
    .to_string()
}

/// The session id and BiDi socket out of a new-session answer, or the driver's
/// own reason for refusing — which is where a version mismatch the browser
/// would not state ends up.
fn new_session_answer(status: u16, body: &str) -> Result<Granted, String> {
    let value: Value =
        serde_json::from_str(body).map_err(|_| format!("HTTP {status}: {}", body.trim()))?;
    let value = value.get("value").unwrap_or(&Value::Null);
    if let Some(error) = value.get("error").and_then(Value::as_str) {
        let message = value.get("message").and_then(Value::as_str).unwrap_or("");
        // Chrome's messages carry a stack trace after the first line.
        let first = message.lines().next().unwrap_or("");
        return Err(format!("{error}: {first}"));
    }
    let id = value
        .get("sessionId")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("HTTP {status}: no session id in {}", body.trim()))?;
    let url = value
        .pointer("/capabilities/webSocketUrl")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            "the driver granted a session without a WebDriver BiDi socket \
             (`webSocketUrl`); it is too old to speak BiDi"
                .to_string()
        })?;
    // The browser's own process, which the vendors report under their prefix
    // (`goog:processID`). Kept to stop the browser if its driver cannot.
    let browser_pid = value
        .get("capabilities")
        .and_then(Value::as_object)
        .and_then(|capabilities| {
            capabilities
                .iter()
                .find(|(key, _)| key.ends_with(":processID"))
                .and_then(|(_, pid)| pid.as_u64())
        })
        .and_then(|pid| u32::try_from(pid).ok());
    Ok(Granted {
        id: id.to_string(),
        url: url.to_string(),
        browser_pid,
    })
}

/// A classic session a driver granted.
#[derive(Debug, PartialEq)]
struct Granted {
    id: String,
    /// The BiDi socket.
    url: String,
    browser_pid: Option<u32>,
}

/// One HTTP/1.1 request on loopback, read to the end. The classic protocol is
/// needed twice per run — to open the session and to end it — which does not
/// earn an HTTP client.
async fn http(
    address: &str,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> Result<(u16, String), String> {
    let mut stream = TcpStream::connect(address)
        .await
        .map_err(|err| format!("cannot reach the driver at {address}: {err}"))?;
    let body = body.unwrap_or("");
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: {address}\r\n\
         Content-Type: application/json; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|err| format!("cannot write to the driver: {err}"))?;
    // Read until the response is complete by its own framing, not until the
    // connection closes: chromedriver keeps it open whatever `Connection`
    // said, and a read to EOF waits for ever.
    let mut response = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        if complete(&response) {
            break;
        }
        let read = stream
            .read(&mut buffer)
            .await
            .map_err(|err| format!("cannot read from the driver: {err}"))?;
        if read == 0 {
            break;
        }
        response.extend_from_slice(&buffer[..read]);
    }
    parse_response(&response)
}

/// Whether `bytes` hold a whole HTTP/1.1 response: its head, and then the body
/// its `Content-Length` promised or a chunked body's last chunk. A response
/// with neither ends when the connection does.
fn complete(bytes: &[u8]) -> bool {
    let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") else {
        return false;
    };
    let head = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
    let body = &bytes[end + 4..];
    for line in head.lines() {
        if let Some(length) = line.strip_prefix("content-length:") {
            return length
                .trim()
                .parse::<usize>()
                .is_ok_and(|length| body.len() >= length);
        }
        if line.starts_with("transfer-encoding:") && line.contains("chunked") {
            return body.ends_with(b"0\r\n\r\n");
        }
    }
    false
}

/// Status and body out of a whole HTTP/1.1 response, chunked or not.
fn parse_response(bytes: &[u8]) -> Result<(u16, String), String> {
    let text = String::from_utf8_lossy(bytes);
    let (head, body) = text
        .split_once("\r\n\r\n")
        .ok_or("the driver sent an incomplete HTTP response")?;
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .ok_or("the driver sent no HTTP status")?;
    let chunked = head.lines().any(|line| {
        let lower = line.to_ascii_lowercase();
        lower.starts_with("transfer-encoding:") && lower.contains("chunked")
    });
    let body = if chunked {
        dechunk(body)?
    } else {
        body.to_string()
    };
    Ok((status, body))
}

fn dechunk(mut rest: &str) -> Result<String, String> {
    let mut body = String::new();
    loop {
        let (size, after) = rest
            .split_once("\r\n")
            .ok_or("the driver sent a truncated chunk")?;
        let size = usize::from_str_radix(size.split(';').next().unwrap_or("").trim(), 16)
            .map_err(|_| "the driver sent a malformed chunk size")?;
        if size == 0 {
            return Ok(body);
        }
        let chunk = after
            .get(..size)
            .ok_or("the driver sent a truncated chunk")?;
        body.push_str(chunk);
        rest = after
            .get(size..)
            .and_then(|tail| tail.strip_prefix("\r\n"))
            .ok_or("the driver sent a truncated chunk")?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    /// A BiDi server that answers from a script: for each command received,
    /// `respond` returns the messages to send back, in order.
    async fn mock(respond: fn(&Value) -> Vec<Value>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            let mut socket = tokio_tungstenite::accept_async(stream)
                .await
                .expect("upgrade");
            while let Some(Ok(Message::Text(text))) = socket.next().await {
                let command: Value = serde_json::from_str(&text).expect("json");
                if command["method"] == "test.hangUp" {
                    return;
                }
                for reply in respond(&command) {
                    socket
                        .send(Message::text(reply.to_string()))
                        .await
                        .expect("send");
                }
            }
        });
        format!("ws://{address}/session")
    }

    #[tokio::test]
    async fn a_command_gets_its_own_answer() {
        let url = mock(|command| {
            vec![json!({
                "type": "success",
                "id": command["id"],
                "result": { "echo": command["params"]["value"] },
            })]
        })
        .await;
        let client = Client::connect(&url).await.expect("connect");
        let (a, b) = tokio::join!(
            client.command("test.echo", json!({ "value": "a" })),
            client.command("test.echo", json!({ "value": "b" })),
        );
        assert_eq!(a.unwrap(), json!({ "echo": "a" }));
        assert_eq!(b.unwrap(), json!({ "echo": "b" }));
    }

    #[tokio::test]
    async fn answers_out_of_order_still_reach_their_commands() {
        // The first command is answered only after the second arrives, and
        // then second-first.
        let url = mock(|command| {
            if command["params"]["value"] == "first" {
                return vec![];
            }
            vec![
                json!({ "type": "success", "id": command["id"], "result": "second" }),
                json!({ "type": "success", "id": 1, "result": "first" }),
            ]
        })
        .await;
        let client = Client::connect(&url).await.expect("connect");
        let (first, second) = tokio::join!(
            client.command("test.echo", json!({ "value": "first" })),
            async {
                tokio::task::yield_now().await;
                client
                    .command("test.echo", json!({ "value": "second" }))
                    .await
            },
        );
        assert_eq!(first.unwrap(), json!("first"));
        assert_eq!(second.unwrap(), json!("second"));
    }

    #[tokio::test]
    async fn an_error_names_the_command_and_the_reason() {
        let url = mock(|command| {
            vec![json!({
                "type": "error",
                "id": command["id"],
                "error": "no such frame",
                "message": "Browsing context with id 42 not found",
            })]
        })
        .await;
        let client = Client::connect(&url).await.expect("connect");
        let err = client
            .command("browsingContext.navigate", json!({}))
            .await
            .unwrap_err();
        assert_eq!(
            err,
            "browsingContext.navigate: no such frame: Browsing context with id 42 not found"
        );
    }

    #[tokio::test]
    async fn events_arrive_in_order_and_apart_from_answers() {
        let url = mock(|command| {
            vec![
                json!({ "type": "event", "method": "log.entryAdded", "params": { "n": 1 } }),
                json!({ "type": "event", "method": "script.message", "params": { "n": 2 } }),
                json!({ "type": "success", "id": command["id"], "result": {} }),
            ]
        })
        .await;
        let client = Client::connect(&url).await.expect("connect");
        let mut events = client.take_events().expect("the event stream");
        assert!(client.take_events().is_none(), "there is one stream");
        client
            .command("session.subscribe", json!({}))
            .await
            .expect("answered");
        let first = events.recv().await.expect("an event");
        let second = events.recv().await.expect("an event");
        assert_eq!(first.method, "log.entryAdded");
        assert_eq!(first.params, json!({ "n": 1 }));
        assert_eq!(second.method, "script.message");
    }

    #[tokio::test]
    async fn a_closed_connection_fails_what_was_waiting_instead_of_hanging() {
        let url = mock(|_| vec![]).await;
        let client = Client::connect(&url).await.expect("connect");
        let mut events = client.take_events().expect("the event stream");
        let (waiting, _) = tokio::join!(
            client.command("test.neverAnswered", json!({})),
            client.command("test.hangUp", json!({})),
        );
        assert_eq!(
            waiting.unwrap_err(),
            "test.neverAnswered: the browser closed the connection"
        );
        assert_eq!(events.recv().await, None);
    }

    #[tokio::test]
    async fn a_command_after_the_connection_closed_fails_at_once() {
        let url = mock(|_| vec![]).await;
        let client = Client::connect(&url).await.expect("connect");
        let mut events = client.take_events().expect("the event stream");
        let _ = client.command("test.hangUp", json!({})).await;
        assert_eq!(events.recv().await, None);
        let late = tokio::time::timeout(
            Duration::from_secs(5),
            client.command("browser.removeUserContext", json!({})),
        )
        .await
        .expect("a command on a closed connection must not wait");
        assert_eq!(
            late.unwrap_err(),
            "browser.removeUserContext: the browser closed the connection"
        );
    }

    #[test]
    fn an_answer_without_a_type_is_still_an_answer() {
        let Routed::Answer(7, Ok(result)) = route(json!({ "id": 7, "result": { "ok": true } }))
        else {
            panic!("not routed as an answer");
        };
        assert_eq!(result, json!({ "ok": true }));
        assert!(matches!(route(json!({ "hello": 1 })), Routed::Neither));
    }

    #[test]
    fn reads_where_firefox_and_the_drivers_listen() {
        assert_eq!(
            listening_url("WebDriver BiDi listening on ws://127.0.0.1:42287"),
            Some("ws://127.0.0.1:42287".to_string())
        );
        assert_eq!(listening_url("*** You are running in headless mode."), None);
        assert_eq!(
            driver_port("ChromeDriver was started successfully on port 32943."),
            Some(32943)
        );
        assert_eq!(
            driver_port("Microsoft Edge WebDriver was started successfully on port 9515."),
            Some(9515)
        );
        assert_eq!(driver_port("Only local connections are allowed."), None);
    }

    #[test]
    fn the_new_session_request_asks_for_bidi_in_the_vendors_words() {
        let chrome = Launch {
            browser: Browser::Chromium,
            binary: "/usr/bin/chromium".into(),
            version: Some(131),
            driver: Some("/usr/bin/chromedriver".into()),
        };
        let body: Value = serde_json::from_str(&new_session_body(&chrome, true)).unwrap();
        let always = &body["capabilities"]["alwaysMatch"];
        assert_eq!(always["webSocketUrl"], json!(true));
        assert_eq!(
            always["goog:chromeOptions"]["binary"],
            json!("/usr/bin/chromium")
        );
        let args = always["goog:chromeOptions"]["args"].as_array().unwrap();
        assert!(args.contains(&json!("--headless=new")));

        let edge = Launch {
            browser: Browser::Edge,
            ..chrome
        };
        let body: Value = serde_json::from_str(&new_session_body(&edge, false)).unwrap();
        let options = &body["capabilities"]["alwaysMatch"]["ms:edgeOptions"];
        assert!(
            !options["args"]
                .as_array()
                .unwrap()
                .contains(&json!("--headless=new"))
        );
    }

    #[test]
    fn a_new_session_answer_gives_the_socket_or_the_drivers_reason() {
        let granted = r#"{"value":{"sessionId":"abc","capabilities":{"webSocketUrl":"ws://127.0.0.1:9222/session/abc","goog:processID":4242}}}"#;
        assert_eq!(
            new_session_answer(200, granted),
            Ok(Granted {
                id: "abc".to_string(),
                url: "ws://127.0.0.1:9222/session/abc".to_string(),
                browser_pid: Some(4242),
            })
        );
        let refused = r#"{"value":{"error":"session not created","message":"session not created: This version of ChromeDriver only supports Chrome version 131\nCurrent browser version is 120.0.6099.71\nStacktrace:\n#0 0x55d…"}}"#;
        let err = new_session_answer(500, refused).unwrap_err();
        assert_eq!(
            err,
            "session not created: session not created: This version of ChromeDriver only supports Chrome version 131"
        );
        let old = r#"{"value":{"sessionId":"abc","capabilities":{}}}"#;
        assert!(
            new_session_answer(200, old)
                .unwrap_err()
                .contains("too old to speak BiDi")
        );
        assert!(
            new_session_answer(502, "Bad Gateway")
                .unwrap_err()
                .contains("HTTP 502")
        );
    }

    #[test]
    fn a_response_is_read_whether_or_not_it_is_chunked() {
        let plain = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}";
        assert_eq!(parse_response(plain), Ok((200, "{}".to_string())));
        let chunked =
            b"HTTP/1.1 500 Internal\r\nTransfer-Encoding: chunked\r\n\r\n4\r\n{\"a\"\r\n3\r\n:1}\r\n0\r\n\r\n";
        assert_eq!(parse_response(chunked), Ok((500, "{\"a\":1}".to_string())));
        assert!(parse_response(b"HTTP/1.1 200 OK\r\n").is_err());
        let truncated = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nff\r\nshort";
        assert!(parse_response(truncated).is_err());
    }

    #[test]
    fn a_response_is_complete_by_its_framing_not_by_the_connection_closing() {
        assert!(!complete(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n"));
        assert!(!complete(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{"));
        assert!(complete(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}"));
        assert!(complete(b"HTTP/1.1 200 OK\r\ncontent-length:0\r\n\r\n"));
        let chunked = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n";
        assert!(!complete(chunked));
        assert!(complete(&[&chunked[..], b"0\r\n\r\n"].concat()));
        // Neither: only the connection closing says it is over.
        assert!(!complete(b"HTTP/1.0 200 OK\r\n\r\n{}"));
    }

    /// The real thing, against every browser this machine can drive — both
    /// ways in, when it has a Firefox and a driven one. On a machine with none
    /// it checks nothing and says so, since which browsers a CI runner carries
    /// is not this crate's to decide; the stubbed tests above always run.
    #[tokio::test]
    async fn drives_every_real_browser_that_is_installed() {
        let mut driven = 0;
        for browser in crate::browser::ORDER {
            let Ok(launch) = crate::browser::find(browser, &crate::browser::System) else {
                continue;
            };
            let session = Session::start(&launch, true)
                .await
                .unwrap_or_else(|err| panic!("{browser}: {err}"));
            let tree = session
                .client
                .command("browsingContext.getTree", json!({}))
                .await
                .unwrap_or_else(|err| panic!("{browser}: {err}"));
            assert!(
                tree["contexts"]
                    .as_array()
                    .is_some_and(|all| !all.is_empty()),
                "{browser}: {tree}"
            );
            session.end().await;
            driven += 1;
        }
        if driven == 0 {
            eprintln!("no browser can be driven here; the real-browser check did not run");
        }
    }
}
