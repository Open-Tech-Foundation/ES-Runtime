//! `--inspect` — the Chrome DevTools Protocol endpoint, and the thread that
//! serves it (DECISIONS.md D59).
//!
//! The engine speaks CDP as JSON text over an
//! [`InspectorTransport`](es_runtime_cli_common::InspectorTransport); everything that makes
//! that text reach Chrome, VS Code or any other CDP client lives here, in the
//! development binary, and nowhere else. `esrun` has no flag that could ask for
//! it and — unless the build was told otherwise — no code that could answer.
//!
//! ## Why a thread of its own
//!
//! Because a paused program is a stopped one. When V8 stops at a breakpoint it
//! hands the isolate's thread to the embedder and asks it not to come back until
//! the debugger says so, so *nothing* on that thread runs meanwhile — no timers,
//! no I/O completions, and certainly no socket read that would deliver the
//! `Debugger.resume` we are waiting for. The socket therefore lives on another
//! thread with its own runtime, and the two halves meet at a pair of queues: a
//! [`std::sync::mpsc`] the isolate's thread may block on (that block *is* the
//! pause) and a tokio channel the server thread drains.
//!
//! ## What it exposes
//!
//! The three endpoints a CDP client looks for: `/json/version`, `/json/list`
//! (the target it should attach to) and the WebSocket itself. Bound to loopback
//! unless told otherwise, and one debugger at a time — a second connection is
//! refused rather than quietly interleaved with the first.

use std::net::{IpAddr, SocketAddr, TcpListener};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::task::Waker;

use es_runtime_cli_common::InspectorTransport;
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
use tokio_tungstenite::tungstenite::protocol::Role;

/// What `--inspect[=<addr>]` / `--inspect-brk[=<addr>]` parsed to.
#[derive(Debug, Clone)]
pub struct InspectConfig {
    /// Where to listen. Defaults to `127.0.0.1:9229`, the port every CDP client
    /// probes first.
    pub address: SocketAddr,
    /// Hold the program before its first statement until a debugger attaches
    /// and releases it — `--inspect-brk`.
    pub wait: bool,
}

impl InspectConfig {
    /// The default endpoint: loopback, port 9229.
    pub fn default_address() -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], 9229))
    }
}

/// What the isolate's thread and the server thread share.
struct Shared {
    /// Messages from the client, in the order they arrived. Read only by the
    /// isolate's thread, which blocks on it while paused; the `Mutex` is what
    /// makes a `!Sync` receiver shareable, not contention management.
    incoming: Mutex<Receiver<Incoming>>,
    /// The same queue's sending half, cloned into each connection.
    sender: Mutex<Sender<Incoming>>,
    /// Where a reply goes, while a client is attached.
    outgoing: Mutex<Option<tokio::sync::mpsc::UnboundedSender<String>>>,
    /// Set when a client attaches, cleared when the engine has noticed and
    /// connected a fresh session to it.
    connected: AtomicBool,
    /// Whether a debugger is attached right now, so a second is refused.
    attached: AtomicBool,
    /// The driver's waker. Without it, a message arriving while the event loop
    /// is parked waits for whatever wakes the loop next — which on an idle
    /// server is the next request, i.e. possibly never.
    waker: Mutex<Option<Waker>>,
}

/// One thing that happened on the socket, in order.
enum Incoming {
    /// A CDP message from the client.
    Message(String),
    /// The client went away. Delivered as an item rather than by dropping the
    /// sender because the server keeps its sender for the *next* client — and a
    /// paused program blocked on this queue would otherwise never be told that
    /// the debugger it is waiting for is gone.
    Disconnected,
}

impl Shared {
    fn wake(&self) {
        if let Some(waker) = self
            .waker
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            waker.wake_by_ref();
        }
    }
}

/// The isolate-side half: what the engine holds.
struct Transport {
    shared: Arc<Shared>,
}

impl InspectorTransport for Transport {
    fn try_recv(&self) -> Option<String> {
        let queue = self
            .shared
            .incoming
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        loop {
            match queue.try_recv() {
                Ok(Incoming::Message(message)) => return Some(message),
                // Nothing for the engine to do about it: the session stays until
                // a new client replaces it.
                Ok(Incoming::Disconnected) => {}
                Err(_) => return None,
            }
        }
    }

    fn recv_blocking(&self) -> Option<String> {
        let queue = self
            .shared
            .incoming
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        match queue.recv() {
            Ok(Incoming::Message(message)) => Some(message),
            // The debugger disconnected while we were paused. Saying so is what
            // lets the engine resume rather than stay stopped for ever.
            Ok(Incoming::Disconnected) | Err(_) => None,
        }
    }

    fn send(&self, message: &str) {
        if let Some(sender) = self
            .shared
            .outgoing
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            let _ = sender.send(message.to_string());
        }
    }

    fn take_new_connection(&self) -> bool {
        self.shared.connected.swap(false, Ordering::SeqCst)
    }

    fn set_waker(&self, waker: Waker) {
        *self.shared.waker.lock().unwrap_or_else(|e| e.into_inner()) = Some(waker);
    }
}

/// Everything the `/json/list` answer needs about the program being debugged.
struct Target {
    /// The path component of the WebSocket URL; a client is expected to use the
    /// one `/json/list` gave it.
    id: String,
    /// What the client shows in its target list.
    title: String,
    /// The entry module's URL.
    url: String,
    address: SocketAddr,
}

/// Binds the endpoint and starts serving it, returning the transport to hand the
/// runtime.
///
/// Binding happens **here**, on the calling thread, so a port already in use is
/// an error before the program runs rather than a silent failure on a thread
/// nobody is watching.
pub fn start(config: &InspectConfig, entry: &str) -> Result<Rc<dyn InspectorTransport>, String> {
    let listener = bind(config.address)
        .map_err(|e| format!("cannot listen for a debugger on {}: {e}", config.address))?;
    let address = listener
        .local_addr()
        .map_err(|e| format!("cannot read the debugger port: {e}"))?;
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("cannot prepare the debugger port: {e}"))?;

    let target = Arc::new(Target {
        id: session_id(),
        title: entry.to_string(),
        url: entry.to_string(),
        address,
    });

    let (sender, receiver) = std::sync::mpsc::channel();
    let shared = Arc::new(Shared {
        incoming: Mutex::new(receiver),
        sender: Mutex::new(sender),
        outgoing: Mutex::new(None),
        connected: AtomicBool::new(false),
        attached: AtomicBool::new(false),
        waker: Mutex::new(None),
    });

    // A whole runtime for one listener is not extravagant here: it must keep
    // running while the isolate's thread is stopped inside V8, which rules out
    // sharing the one the program is driven on.
    let server_shared = shared.clone();
    let server_target = target.clone();
    std::thread::Builder::new()
        .name("es-inspector".to_string())
        .spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            runtime.block_on(serve(listener, server_shared, server_target));
        })
        .map_err(|e| format!("cannot start the debugger thread: {e}"))?;

    if !config.address.ip().is_loopback() {
        eprintln!(
            "warning: the debugger is listening on {address}, which is not loopback — anyone \
             who can reach it can run code in this process, whatever it was denied."
        );
    }
    eprintln!("Debugger listening on ws://{address}/{}", target.id);
    if config.wait {
        eprintln!("Waiting for the debugger to attach…");
    }

    Ok(Rc::new(Transport { shared }))
}

/// Binds the listening socket, asking for the port back if it is lingering.
///
/// `SO_REUSEADDR` is what makes `esdev --watch --inspect` work at all: a restart
/// binds the same port while the previous process's accepted connections are
/// still in `TIME_WAIT`, which the kernel refuses without it — and a debugger
/// attached across a restart guarantees there are such connections.
#[cfg(unix)]
fn bind(address: SocketAddr) -> std::io::Result<TcpListener> {
    use rustix::net::{AddressFamily, SocketType, sockopt};

    let family = if address.is_ipv4() {
        AddressFamily::INET
    } else {
        AddressFamily::INET6
    };
    let socket = rustix::net::socket(family, SocketType::STREAM, None)?;
    sockopt::set_socket_reuseaddr(&socket, true)?;
    rustix::net::bind(&socket, &address)?;
    // The same backlog std uses. Nothing here is a load-bearing listener: the
    // clients are one developer's debugger and its discovery requests.
    rustix::net::listen(&socket, 128)?;
    Ok(TcpListener::from(socket))
}

#[cfg(not(unix))]
fn bind(address: SocketAddr) -> std::io::Result<TcpListener> {
    // Windows has no equivalent worth setting: its `SO_REUSEADDR` lets two live
    // sockets share a port rather than reclaiming a lingering one, and a port
    // left behind by an exited process is already rebindable there.
    TcpListener::bind(address)
}

/// Accepts connections for as long as the process lives.
async fn serve(listener: TcpListener, shared: Arc<Shared>, target: Arc<Target>) {
    let Ok(listener) = tokio::net::TcpListener::from_std(listener) else {
        return;
    };
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            continue;
        };
        // Each connection on its own task: a client that opens the WebSocket and
        // holds it must not stop another from reading `/json/list`, which is how
        // most of them discover the first.
        tokio::spawn(handle(stream, shared.clone(), target.clone()));
    }
}

/// Serves one connection: a JSON discovery request, or the debugger itself.
async fn handle(mut stream: TcpStream, shared: Arc<Shared>, target: Arc<Target>) {
    let Some(head) = read_head(&mut stream).await else {
        return;
    };
    let path = request_path(&head).unwrap_or_default();

    if let Some(key) = websocket_key(&head) {
        if path.trim_start_matches('/') != target.id {
            let _ = respond(&mut stream, "404 Not Found", "text/plain", "no such target").await;
            return;
        }
        // One debugger at a time. Two sessions on one isolate is a coherent idea
        // — V8 supports it — but two clients stepping the same program is not,
        // and refusing is clearer than interleaving.
        if shared.attached.swap(true, Ordering::SeqCst) {
            let _ = respond(
                &mut stream,
                "409 Conflict",
                "text/plain",
                "a debugger is already attached",
            )
            .await;
            return;
        }
        session(stream, &shared, &key).await;
        shared.attached.store(false, Ordering::SeqCst);
        return;
    }

    let (status, body) = match path.as_str() {
        "/json/version" => (
            "200 OK",
            // `esdev`, not the runtime: this names the binary the client is
            // attached to, and the two are versioned separately.
            format!(
                r#"{{"Browser":"esdev/{}","Protocol-Version":"1.3"}}"#,
                env!("CARGO_PKG_VERSION")
            ),
        ),
        "/json" | "/json/list" => ("200 OK", target.as_json()),
        _ => ("404 Not Found", "[]".to_string()),
    };
    let _ = respond(
        &mut stream,
        status,
        "application/json; charset=UTF-8",
        &body,
    )
    .await;
}

/// Runs one debugger session until the client goes away.
async fn session(mut stream: TcpStream, shared: &Arc<Shared>, key: &str) {
    let accept = derive_accept_key(key.as_bytes());
    let handshake = format!(
        "HTTP/1.1 101 Switching Protocols\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Accept: {accept}\r\n\r\n"
    );
    if stream.write_all(handshake.as_bytes()).await.is_err() {
        return;
    }
    let socket = WebSocketStream::from_raw_socket(stream, Role::Server, None).await;
    let (mut sink, mut messages) = socket.split();

    let (outgoing, mut replies) = tokio::sync::mpsc::unbounded_channel::<String>();
    *shared.outgoing.lock().unwrap_or_else(|e| e.into_inner()) = Some(outgoing);
    // Announced before anything is read, so the engine has connected a session
    // to this client by the time its first message is dispatched.
    shared.connected.store(true, Ordering::SeqCst);
    shared.wake();

    loop {
        tokio::select! {
            incoming = messages.next() => match incoming {
                Some(Ok(Message::Text(text))) => {
                    let sent = shared
                        .sender
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .send(Incoming::Message(text.as_str().to_string()));
                    if sent.is_err() {
                        break;
                    }
                    // The program may be parked with nothing pending — an idle
                    // server is the ordinary case — and this is the only thing
                    // that will get it to look.
                    shared.wake();
                }
                // A CDP client sends text and nothing else; a ping is answered
                // by the library beneath us.
                Some(Ok(_)) => {}
                Some(Err(_)) | None => break,
            },
            reply = replies.recv() => match reply {
                Some(text) => {
                    if sink.send(Message::Text(text.into())).await.is_err() {
                        break;
                    }
                }
                None => break,
            },
        }
    }

    *shared.outgoing.lock().unwrap_or_else(|e| e.into_inner()) = None;
    let _ = shared
        .sender
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .send(Incoming::Disconnected);
    // A program stopped at a breakpoint is blocked on that queue: this is what
    // tells it the debugger is gone, and it resumes rather than waiting for a
    // client that will never answer.
    shared.wake();
}

/// Reads an HTTP request head, up to and including the blank line.
///
/// Deliberately stops there rather than reading whatever else is buffered: on a
/// WebSocket upgrade the bytes after the head are already the client's first
/// frames, and consuming them here would lose them.
pub async fn read_head(stream: &mut TcpStream) -> Option<String> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        match stream.read_exact(&mut byte).await {
            Ok(_) => head.push(byte[0]),
            Err(_) => return None,
        }
        // A request head this long is not one of ours.
        if head.len() > 8192 {
            return None;
        }
    }
    String::from_utf8(head).ok()
}

/// The path from a request head's first line.
pub fn request_path(head: &str) -> Option<String> {
    let mut parts = head.lines().next()?.split_whitespace();
    let _method = parts.next()?;
    Some(parts.next()?.to_string())
}

/// The `Sec-WebSocket-Key` of an upgrade request, or `None` if this is an
/// ordinary GET.
fn websocket_key(head: &str) -> Option<String> {
    let mut upgrading = false;
    let mut key = None;
    for line in head.lines().skip(1) {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match name.trim().to_ascii_lowercase().as_str() {
            "upgrade" if value.eq_ignore_ascii_case("websocket") => upgrading = true,
            "sec-websocket-key" => key = Some(value.to_string()),
            _ => {}
        }
    }
    if upgrading { key } else { None }
}

pub async fn respond(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &str,
) -> std::io::Result<()> {
    let response = format!(
        "HTTP/1.1 {status}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.flush().await
}

impl Target {
    /// The `/json/list` answer: one target, described the way a CDP client
    /// expects a non-browser one to be.
    fn as_json(&self) -> String {
        let ws = format!("{}/{}", self.address, self.id);
        format!(
            r#"[{{"description":"es-runtime instance","devtoolsFrontendUrl":"devtools://devtools/bundled/js_app.html?experiments=true&v8only=true&ws={ws}","id":"{id}","title":"{title}","type":"node","url":"{url}","webSocketDebuggerUrl":"ws://{ws}"}}]"#,
            id = escape(&self.id),
            title = escape(&self.title),
            url = escape(&self.url),
        )
    }
}

/// Escapes a string for embedding in the hand-built JSON above. A path is the
/// only thing that reaches it, and on Windows a path is full of backslashes.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// A UUID-shaped id for this session's WebSocket path.
///
/// It is not a secret and is not treated as one — the endpoint's protection is
/// that it is bound to loopback, and the warning above says what happens when it
/// is not. This exists because clients expect the shape.
fn session_id() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    // The counter is what keeps two ids in one process apart: a clock read is
    // not guaranteed to have moved between them.
    static MINTED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let serial = MINTED.fetch_add(1, Ordering::Relaxed);
    let mut state = (now as u64)
        ^ (u64::from(std::process::id()) << 32)
        ^ serial.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    let mut next = || {
        // SplitMix64: a few lines, no dependency, and plenty for a name.
        state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    };
    let (high, low) = (next(), next());
    format!(
        "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
        high >> 32,
        (high >> 16) & 0xffff,
        high & 0xfff,
        0x8000 | (low >> 52),
        low & 0xffff_ffff_ffff
    )
}

/// Parses `--inspect[=<addr>]`'s value: a port, a host, or `host:port`.
pub fn parse_address(value: Option<&str>) -> Result<SocketAddr, String> {
    let default = InspectConfig::default_address();
    let Some(value) = value else {
        return Ok(default);
    };
    if let Ok(port) = value.parse::<u16>() {
        return Ok(SocketAddr::new(default.ip(), port));
    }
    if let Ok(address) = value.parse::<SocketAddr>() {
        return Ok(address);
    }
    if let Ok(ip) = value.parse::<IpAddr>() {
        return Ok(SocketAddr::new(ip, default.port()));
    }
    Err(format!(
        "{value} is not an address to listen on — write a port (--inspect=9229), an address \
         (--inspect=127.0.0.1) or both (--inspect=127.0.0.1:9229).\n\n\
         A host name is not accepted: a debugger port is bound, not resolved."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_defaults_to_loopback_9229() {
        let address = parse_address(None).expect("default");
        assert_eq!(address.to_string(), "127.0.0.1:9229");
    }

    #[test]
    fn a_bare_port_keeps_loopback() {
        let address = parse_address(Some("9300")).expect("port");
        assert_eq!(address.to_string(), "127.0.0.1:9300");
    }

    #[test]
    fn a_bare_address_keeps_the_default_port() {
        let address = parse_address(Some("0.0.0.0")).expect("address");
        assert_eq!(address.to_string(), "0.0.0.0:9229");
    }

    #[test]
    fn host_and_port_are_taken_as_written() {
        let address = parse_address(Some("0.0.0.0:9111")).expect("both");
        assert_eq!(address.to_string(), "0.0.0.0:9111");
    }

    #[test]
    fn a_host_name_is_refused_rather_than_resolved() {
        let error = parse_address(Some("localhost")).expect_err("refused");
        assert!(error.contains("not an address"), "{error}");
    }

    #[test]
    fn the_upgrade_key_is_read_only_from_an_upgrade() {
        let upgrade = "GET /x HTTP/1.1\r\nUpgrade: websocket\r\nSec-WebSocket-Key: abc\r\n\r\n";
        assert_eq!(websocket_key(upgrade).as_deref(), Some("abc"));
        let plain = "GET /json/list HTTP/1.1\r\nHost: localhost\r\n\r\n";
        assert_eq!(websocket_key(plain), None);
    }

    #[test]
    fn the_path_comes_from_the_request_line() {
        let head = "GET /json/list HTTP/1.1\r\nHost: localhost\r\n\r\n";
        assert_eq!(request_path(head).as_deref(), Some("/json/list"));
    }

    #[test]
    fn a_windows_path_survives_the_target_listing() {
        let target = Target {
            id: "id".to_string(),
            title: r"C:\app\server.ts".to_string(),
            url: r"C:\app\server.ts".to_string(),
            address: InspectConfig::default_address(),
        };
        let json = target.as_json();
        assert!(json.contains(r"C:\\app\\server.ts"), "{json}");
    }

    #[test]
    fn a_session_id_has_the_shape_clients_expect() {
        let id = session_id();
        let parts: Vec<&str> = id.split('-').collect();
        assert_eq!(
            parts.iter().map(|p| p.len()).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12],
            "{id}"
        );
        assert_ne!(id, session_id(), "two ids in one process must differ");
    }
}
