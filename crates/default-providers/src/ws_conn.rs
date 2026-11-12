//! One WebSocket connection on `es-runtime-http`'s framing (DECISIONS D217) —
//! the tokio half: the actor task that owns the stream, the opening handshake
//! a standalone WebSocket server answers, and the accept key.
//!
//! The actor is the one [`SystemWebSocket`](crate::SystemWebSocket) has always
//! had: a `select!` over the socket, the guest's commands and the overflow
//! signal. What it does on its own, without the guest: answers pings, echoes
//! the peer's close (code and reason) to complete the closing handshake (RFC 6455 §5.5.1),
//! and fails the connection — a close frame with the code §7.4.1 names, then
//! the socket dropped — on any protocol violation, which the guest sees as an
//! abnormal close (1006), as a browser's `WebSocket` reports it.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use es_runtime_http::h1::{Limits as HeadLimits, Parsed, parse_request};
use es_runtime_http::ws::{
    Decoder, Event, Limits, Opcode, Role, UpgradeRefusal, check_upgrade, encode_close, encode_frame,
};
use es_runtime_providers::{WsIncoming, WsMessage};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{Notify, mpsc};

/// RFC 6455 §7.4.2's "Try Again Later": a peer whose queue ran away.
const TRY_AGAIN_LATER: u16 = 1013;

/// A close with no status code is reported as 1005 (§7.1.5).
const NO_STATUS_RCVD: u16 = 1005;

/// The GUID §1.3 appends to the key before hashing.
const GUID: &[u8] = b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// `Sec-WebSocket-Accept` for a key (§4.2.2): base64 of SHA-1 of key + GUID.
pub(crate) fn accept_key(key: &[u8]) -> String {
    use base64::Engine as _;
    use sha1::Digest as _;
    let mut sha = sha1::Sha1::new();
    sha.update(key);
    sha.update(GUID);
    base64::engine::general_purpose::STANDARD.encode(sha.finalize())
}

/// An outbound command to a connection's actor.
pub(crate) enum Cmd {
    /// One message. Shared, so a broadcast hands every connection the same one.
    Send(Arc<WsMessage>),
    /// Begin the closing handshake.
    Close { code: Option<u16>, reason: String },
}

/// The bytes a message counts against the send queue.
pub(crate) fn message_len(message: &WsMessage) -> u64 {
    match message {
        WsMessage::Text(s) => s.len() as u64,
        WsMessage::Binary(b) => b.len() as u64,
    }
}

/// Masking keys for a client's frames (§5.3: fresh and unpredictable for
/// each), drawn from the OS in blocks rather than a syscall per frame.
struct Masks {
    pool: [u8; 4096],
    at: usize,
}

impl Masks {
    fn new() -> Self {
        Masks {
            pool: [0; 4096],
            at: 4096,
        }
    }

    fn next(&mut self) -> [u8; 4] {
        if self.at == self.pool.len() {
            // getrandom failing leaves the OS without entropy, which nothing
            // here could recover from; the runtime refuses to start then too.
            getrandom::fill(&mut self.pool).expect("OS entropy");
            self.at = 0;
        }
        let key = [
            self.pool[self.at],
            self.pool[self.at + 1],
            self.pool[self.at + 2],
            self.pool[self.at + 3],
        ];
        self.at += 4;
        key
    }
}

/// Frames for one side of a connection: masked when it is the client's.
struct Writer {
    role: Role,
    masks: Masks,
    out: Vec<u8>,
}

impl Writer {
    fn mask(&mut self) -> Option<[u8; 4]> {
        (self.role == Role::Client).then(|| self.masks.next())
    }

    fn message(&mut self, message: &WsMessage) {
        let mask = self.mask();
        match message {
            WsMessage::Text(s) => {
                encode_frame(&mut self.out, Opcode::Text, true, s.as_bytes(), mask)
            }
            WsMessage::Binary(b) => encode_frame(&mut self.out, Opcode::Binary, true, b, mask),
        }
    }

    fn pong(&mut self, payload: &[u8]) {
        let mask = self.mask();
        encode_frame(&mut self.out, Opcode::Pong, true, payload, mask);
    }

    fn close(&mut self, code: Option<u16>, reason: &str) {
        let mask = self.mask();
        encode_close(&mut self.out, code, reason, mask);
    }

    async fn flush<W: AsyncWrite + Unpin>(&mut self, wr: &mut W) -> std::io::Result<()> {
        if self.out.is_empty() {
            return Ok(());
        }
        let written = wr.write_all(&self.out).await;
        self.out.clear();
        written?;
        wr.flush().await
    }
}

/// The channels and counters an actor shares with its [`WsSlot`](crate).
pub(crate) struct Actor {
    pub(crate) inbound_tx: mpsc::Sender<WsIncoming>,
    pub(crate) cmd_rx: mpsc::Receiver<Cmd>,
    /// Bytes accepted by `send` and not yet written.
    pub(crate) buffered: Arc<AtomicU64>,
    /// Rung when `buffered` passed its bound.
    pub(crate) overflow: Arc<Notify>,
}

/// Runs one connection until it ends. `leftover` is whatever was read past
/// the opening handshake: the first frames.
pub(crate) async fn run<S>(io: S, leftover: Vec<u8>, role: Role, actor: Actor)
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let Actor {
        inbound_tx,
        mut cmd_rx,
        buffered,
        overflow,
    } = actor;
    let (mut rd, mut wr) = tokio::io::split(io);
    let mut decoder = Decoder::new(role, Limits::default());
    let mut writer = Writer {
        role,
        masks: Masks::new(),
        out: Vec::with_capacity(4096),
    };
    let mut buf = leftover;
    let mut close_sent = false;
    // Frames that came with the handshake are read before anything else.
    let mut pending_read = !buf.is_empty();
    loop {
        if pending_read {
            pending_read = false;
            match read_frames(&mut decoder, &mut buf, &mut writer, &inbound_tx, close_sent).await {
                Flow::Continue => {}
                Flow::Close => {
                    let _ = writer.flush(&mut wr).await;
                    if role == Role::Server {
                        // The server closes the TCP connection first (§7.1.1).
                        let _ = wr.shutdown().await;
                    }
                    return;
                }
                Flow::Fail => {
                    let _ = writer.flush(&mut wr).await;
                    return;
                }
            }
            if writer.flush(&mut wr).await.is_err() {
                return;
            }
        }
        tokio::select! {
            // A peer so far behind that its queue passed the bound: a close
            // frame rather than a dropped connection, so both ends learn why.
            () = overflow.notified() => {
                tracing::debug!(
                    target: "runtime::websocket",
                    buffered = buffered.load(Ordering::Relaxed),
                    "closing: the send queue for this peer passed its bound",
                );
                writer.close(Some(TRY_AGAIN_LATER), "send queue overflowed");
                let _ = writer.flush(&mut wr).await;
                return;
            },
            n = async { buf.reserve(16 * 1024); rd.read_buf(&mut buf).await } => match n {
                // The stream ended without a closing handshake: dropping
                // `inbound_tx` makes the next `recv` resolve `None` (1006).
                Ok(0) | Err(_) => return,
                Ok(_) => pending_read = true,
            },
            cmd = cmd_rx.recv() => {
                // A burst of queued sends goes out as one write.
                let mut fed = 0u64;
                let mut cmd = cmd;
                loop {
                    match cmd {
                        Some(Cmd::Send(message)) => {
                            fed += message_len(&message);
                            // §5.5.1: no data frame after our close frame.
                            if !close_sent {
                                writer.message(&message);
                            }
                        }
                        Some(Cmd::Close { code, reason }) => {
                            if !close_sent {
                                writer.close(code, &reason);
                                close_sent = true;
                            }
                            // Keep reading for the peer's answer.
                        }
                        None => {
                            // The runtime dropped the socket.
                            let _ = writer.flush(&mut wr).await;
                            return;
                        }
                    }
                    match cmd_rx.try_recv() {
                        Ok(next) => cmd = Some(next),
                        Err(_) => break,
                    }
                }
                let flushed = writer.flush(&mut wr).await;
                // Off the queue once it has reached the transport.
                buffered.fetch_sub(fed, Ordering::Relaxed);
                if flushed.is_err() {
                    return;
                }
            },
        }
    }
}

enum Flow {
    Continue,
    /// The closing handshake is complete (the peer's close answered, or ours
    /// acknowledged).
    Close,
    /// The peer broke the protocol, or the guest is gone.
    Fail,
}

/// Decodes every whole frame in `buf` and acts on it.
async fn read_frames(
    decoder: &mut Decoder,
    buf: &mut Vec<u8>,
    writer: &mut Writer,
    inbound_tx: &mpsc::Sender<WsIncoming>,
    close_sent: bool,
) -> Flow {
    let mut at = 0;
    let flow = loop {
        match decoder.decode(&buf[at..]) {
            Ok(None) => break Flow::Continue,
            Ok(Some((event, used))) => {
                at += used;
                let Some(event) = event else { continue };
                match event {
                    Event::Text(t) => {
                        if inbound_tx.send(WsIncoming::Text(t)).await.is_err() {
                            break Flow::Fail; // consumer gone
                        }
                    }
                    Event::Binary(b) => {
                        if inbound_tx.send(WsIncoming::Binary(b)).await.is_err() {
                            break Flow::Fail;
                        }
                    }
                    // Control frames stay in the host (the IDL has no ping
                    // event).
                    Event::Ping(payload) => {
                        if !close_sent {
                            writer.pong(&payload);
                        }
                    }
                    Event::Pong(_) => {}
                    Event::Close { code, reason } => {
                        if !close_sent {
                            // Completes the handshake, echoing the code and
                            // reason, so both ends report the same close.
                            writer.close(code, &reason);
                        }
                        let _ = inbound_tx
                            .send(WsIncoming::Close {
                                code: code.unwrap_or(NO_STATUS_RCVD),
                                reason,
                            })
                            .await;
                        break Flow::Close;
                    }
                }
            }
            Err(e) => {
                tracing::debug!(target: "runtime::websocket", error = %e, "failing the connection");
                writer.close(Some(e.code), e.why);
                break Flow::Fail;
            }
        }
    };
    buf.drain(..at);
    flow
}

/// The opening handshake as a standalone WebSocket server answers it
/// (RFC 6455 §4.2): reads the request head, checks it, and writes the `101`
/// — or `400`/`426` and an error. Returns what was read past the head.
pub(crate) async fn server_handshake<S>(io: &mut S) -> Result<Vec<u8>, String>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let limits = HeadLimits::default();
    let mut buf = Vec::with_capacity(1024);
    let mut fields = Vec::new();
    let head = loop {
        match parse_request(&buf, &limits, &mut fields) {
            Ok(Parsed::Complete(head)) => break head,
            Ok(Parsed::Partial) => {}
            Err(e) => {
                let _ = io.write_all(&refusal(e.status(), "")).await;
                return Err(format!("handshake: {e:?}"));
            }
        }
        buf.reserve(4096);
        match io.read_buf(&mut buf).await {
            Ok(0) => return Err("the connection closed during the handshake".into()),
            Ok(_) => {}
            Err(e) => return Err(format!("handshake: {e}")),
        }
    };
    let field = |name: &[u8]| {
        fields
            .iter()
            .find(|f| f.name.of(&buf).eq_ignore_ascii_case(name))
            .map(|f| f.value.of(&buf))
    };
    let key = match check_upgrade(head.method.of(&buf), field) {
        Ok(key) => key,
        Err(UpgradeRefusal::Version) => {
            let _ = io
                .write_all(&refusal(426, "sec-websocket-version: 13\r\n"))
                .await;
            return Err("handshake: a Sec-WebSocket-Version other than 13".into());
        }
        Err(why) => {
            let _ = io.write_all(&refusal(400, "")).await;
            return Err(format!("handshake: {why:?}"));
        }
    };
    let response = format!(
        "HTTP/1.1 101 Switching Protocols\r\nupgrade: websocket\r\nconnection: Upgrade\r\nsec-websocket-accept: {}\r\n\r\n",
        accept_key(key)
    );
    io.write_all(response.as_bytes())
        .await
        .map_err(|e| format!("handshake: {e}"))?;
    Ok(buf.split_off(head.len))
}

fn refusal(status: u16, extra: &str) -> Vec<u8> {
    let reason = match status {
        426 => "Upgrade Required",
        431 => "Request Header Fields Too Large",
        505 => "HTTP Version Not Supported",
        _ => "Bad Request",
    };
    format!("HTTP/1.1 {status} {reason}\r\n{extra}connection: close\r\ncontent-length: 0\r\n\r\n")
        .into_bytes()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicU64;

    use es_runtime_http::ws::{Decoder, Event, Limits, Opcode, Role, encode_close, encode_frame};
    use es_runtime_providers::{WsIncoming, WsMessage};
    use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
    use tokio::sync::{Notify, mpsc};

    use super::{Actor, Cmd, accept_key, run, server_handshake};

    #[test]
    fn the_accept_key_is_rfc_6455s_example() {
        // §1.3.
        assert_eq!(
            accept_key(b"dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }

    const UPGRADE: &[u8] =
        b"GET /chat HTTP/1.1\r\nhost: x\r\nupgrade: websocket\r\nconnection: Upgrade\r\n\
        sec-websocket-key: dGhlIHNhbXBsZSBub25jZQ==\r\nsec-websocket-version: 13\r\n\r\n";

    struct Guest {
        inbound: mpsc::Receiver<WsIncoming>,
        cmd: mpsc::Sender<Cmd>,
    }

    /// The actor in `role` on one end of a pipe; the test is the peer.
    fn actor(io: DuplexStream, leftover: Vec<u8>, role: Role) -> Guest {
        let (inbound_tx, inbound) = mpsc::channel(16);
        let (cmd, cmd_rx) = mpsc::channel(16);
        let actor = Actor {
            inbound_tx,
            cmd_rx,
            buffered: Arc::new(AtomicU64::new(0)),
            overflow: Arc::new(Notify::new()),
        };
        tokio::spawn(run(io, leftover, role, actor));
        Guest { inbound, cmd }
    }

    fn masked(opcode: Opcode, payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        encode_frame(&mut out, opcode, true, payload, Some([1, 2, 3, 4]));
        out
    }

    /// Reads frames from the server side of `peer` until one event arrives.
    async fn next_event(peer: &mut DuplexStream, role: Role, buf: &mut Vec<u8>) -> Option<Event> {
        let mut d = Decoder::new(role, Limits::default());
        loop {
            if let Some((event, used)) = d.decode(buf).expect("a valid frame") {
                buf.drain(..used);
                if event.is_some() {
                    return event;
                }
                continue;
            }
            let mut chunk = [0u8; 4096];
            match peer.read(&mut chunk).await {
                Ok(0) | Err(_) => return None,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
        }
    }

    #[tokio::test]
    async fn frames_sent_with_the_handshake_are_not_lost() {
        let (mut server_io, mut peer) = tokio::io::duplex(64 * 1024);
        let mut hello = UPGRADE.to_vec();
        hello.extend(masked(Opcode::Text, b"early"));
        peer.write_all(&hello).await.unwrap();
        let leftover = server_handshake(&mut server_io).await.unwrap();
        let mut guest = actor(server_io, leftover, Role::Server);
        assert!(matches!(guest.inbound.recv().await, Some(WsIncoming::Text(t)) if t == "early"));
        let mut response = vec![0u8; 512];
        let n = peer.read(&mut response).await.unwrap();
        let response = String::from_utf8_lossy(&response[..n]);
        assert!(response.starts_with("HTTP/1.1 101 "), "{response}");
        assert!(response.contains("sec-websocket-accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo="));
        drop(guest.cmd);
    }

    #[tokio::test]
    async fn a_close_is_echoed_and_the_server_hangs_up() {
        let (io, mut peer) = tokio::io::duplex(64 * 1024);
        let mut guest = actor(io, Vec::new(), Role::Server);
        let mut close = Vec::new();
        encode_close(&mut close, Some(4001), "bye", Some([9, 9, 9, 9]));
        peer.write_all(&close).await.unwrap();
        assert!(matches!(
            guest.inbound.recv().await,
            Some(WsIncoming::Close { code: 4001, reason }) if reason == "bye"
        ));
        let mut buf = Vec::new();
        assert_eq!(
            next_event(&mut peer, Role::Client, &mut buf).await,
            Some(Event::Close {
                code: Some(4001),
                reason: "bye".into()
            })
        );
        assert_eq!(
            next_event(&mut peer, Role::Client, &mut buf).await,
            None,
            "then EOF"
        );
    }

    #[tokio::test]
    async fn a_protocol_violation_fails_the_connection_with_1002() {
        let (io, mut peer) = tokio::io::duplex(64 * 1024);
        let mut guest = actor(io, Vec::new(), Role::Server);
        // Unmasked, from a client: §5.1.
        let mut frame = Vec::new();
        encode_frame(&mut frame, Opcode::Text, true, b"x", None);
        peer.write_all(&frame).await.unwrap();
        let mut buf = Vec::new();
        assert!(matches!(
            next_event(&mut peer, Role::Client, &mut buf).await,
            Some(Event::Close {
                code: Some(1002),
                ..
            })
        ));
        // The guest sees the stream end with no close: an abnormal close.
        assert!(guest.inbound.recv().await.is_none());
    }

    #[tokio::test]
    async fn pings_are_answered_with_the_same_bytes() {
        let (io, mut peer) = tokio::io::duplex(64 * 1024);
        let _guest = actor(io, Vec::new(), Role::Server);
        peer.write_all(&masked(Opcode::Ping, b"are you there"))
            .await
            .unwrap();
        let mut buf = Vec::new();
        assert_eq!(
            next_event(&mut peer, Role::Client, &mut buf).await,
            Some(Event::Pong(b"are you there".to_vec()))
        );
    }

    #[tokio::test]
    async fn a_clients_frames_are_masked_and_a_servers_are_not() {
        for role in [Role::Client, Role::Server] {
            let (io, mut peer) = tokio::io::duplex(64 * 1024);
            let guest = actor(io, Vec::new(), role);
            guest
                .cmd
                .send(Cmd::Send(Arc::new(WsMessage::Binary(vec![5; 300]))))
                .await
                .unwrap();
            let peer_role = if role == Role::Client {
                Role::Server
            } else {
                Role::Client
            };
            let mut buf = Vec::new();
            // The peer's decoder refuses a frame masked the wrong way.
            assert_eq!(
                next_event(&mut peer, peer_role, &mut buf).await,
                Some(Event::Binary(vec![5; 300])),
                "{role:?}"
            );
        }
    }

    #[tokio::test]
    async fn the_standalone_handshake_refuses_what_is_not_a_websocket_request() {
        let cases: [(&[u8], &str); 3] = [
            (b"GET / HTTP/1.1\r\nhost: x\r\n\r\n", "HTTP/1.1 400 "),
            (
                b"GET / HTTP/1.1\r\nhost: x\r\nupgrade: websocket\r\nconnection: upgrade\r\n\
                  sec-websocket-key: dGhlIHNhbXBsZSBub25jZQ==\r\nsec-websocket-version: 8\r\n\r\n",
                "HTTP/1.1 426 ",
            ),
            (b"GET / HTTP/1.0\r\n\r\n", "HTTP/1.1 505 "),
        ];
        for (request, status) in cases {
            let (mut server_io, mut peer) = tokio::io::duplex(64 * 1024);
            peer.write_all(request).await.unwrap();
            assert!(server_handshake(&mut server_io).await.is_err());
            drop(server_io);
            let mut response = String::new();
            peer.read_to_string(&mut response).await.unwrap();
            assert!(response.starts_with(status), "{status}: {response}");
            if status.contains("426") {
                assert!(response.contains("sec-websocket-version: 13"));
            }
        }
    }
}
