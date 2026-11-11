//! OS-backed [`WebSocketProvider`] — the default transport for the `WebSocket`
//! global (DECISIONS D29), over `tokio-tungstenite` (RFC 6455 framing).
//!
//! Each connection is owned by a **spawned actor task** that holds the split
//! `WebSocketStream` and runs a `select!` loop: it forwards inbound text/binary
//! frames to a channel the `ws_recv` op drains, answers ping with pong itself,
//! and applies `ws_send`/`ws_close` commands sent over a second channel. This is
//! the same shape as [`SystemNet`](crate::SystemNet): the I/O is driven by the
//! reactor via the task, so the ops just send/recv on channels while the runtime
//! ticks — no owned loop in the runtime (D4).
//!
//! TLS for `wss:` reuses the rustls / `tokio-rustls` stack from `runtime:net`
//! (the `aws-lc-rs` provider, `webpki-roots` trust anchors, DECISIONS D28): we
//! complete the TLS handshake ourselves. The client's opening handshake — an
//! HTTP/1.1 request and its `101` — is ours too (D217), on `es-runtime-http`'s
//! codec; tungstenite is handed the established stream and does the framing.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use es_runtime_providers::{
    BoxFuture, ProviderError, SocketInfo, UpgradedIo, WebSocketInfo, WebSocketProvider, WsIncoming,
    WsMessage, WsServeOptions,
};
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
#[cfg(test)]
use tokio::net::TcpStream;
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore, mpsc};
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tracing::Instrument;

use crate::accept_backoff::AcceptBackoff;
use crate::checkout::Checkout;
use crate::peer_limit::{PeerLimit, PeerSlot};
use crate::proxy::BoxWire;
use es_runtime_common::ErrorCode;
use tokio_util::compat::FuturesAsyncReadCompatExt;

/// RFC 6455 §7.4.2's "Try Again Later" — the honest answer to a peer whose
/// queue has run away: the server is not refusing the client, it is refusing to
/// hold any more for it, and reconnecting is a reasonable thing to do next.
const WS_TRY_AGAIN_LATER: u16 = 1013;

/// The send-queue bound a client `connect` gets, since no server's options
/// apply to one — a slow server is the same problem from the other end.
///
/// On by default, unlike the connection caps, because this number does not
/// depend on anything the deployment knows: 8 MiB *undrained* is already a peer
/// several messages behind on a large payload, or thousands behind on a small
/// one, and the alternative to a bound is the host holding it until the process
/// dies.
const DEFAULT_MAX_BUFFERED: u64 = WsServeOptions::DEFAULT_MAX_BUFFERED_AMOUNT;

/// A close with no peer status code maps to 1005 ("no status received").
const NO_STATUS_RCVD: u16 = 1005;

/// An outbound command to a connection actor (a `ws_send` or `ws_close`).
enum Cmd {
    Send(Message),
    Close { code: Option<u16>, reason: String },
}

/// A connection's channel ends. `inbound_rx` is taken out during a `recv` and
/// restored after (one outstanding recv per socket, like [`SystemNet`] reads);
/// `cmd_tx` is cloned to send a command to the actor.
struct WsSlot {
    inbound_rx: Option<mpsc::Receiver<WsIncoming>>,
    cmd_tx: mpsc::Sender<Cmd>,
    /// Bytes accepted by `send` and not yet written to the sink.
    ///
    /// `send()` is fire-and-forget by design — the WebSocket API has no way to
    /// report a full buffer — so writing faster than a peer reads does not stall
    /// the guest; it parks one host-side send per message. This is the count
    /// that makes that visible, and the one [`max_buffered`](Self::max_buffered)
    /// bounds.
    buffered: Arc<AtomicU64>,
    /// The most that may be outstanding before the connection is closed, or
    /// `None` for no bound.
    max_buffered: Option<u64>,
    /// Rung when the bound is passed. A separate signal rather than a `Cmd`,
    /// because the command channel is exactly what is full at that moment —
    /// queueing the close behind the backlog it is meant to end would park with
    /// everything else.
    overflow: Arc<Notify>,
}

/// What [`WsSlot::queue`] decided about one outbound message.
enum Queued {
    /// Counted against the connection's queue; send it and, if that fails,
    /// return `bytes` to `buffered`.
    Accepted {
        tx: mpsc::Sender<Cmd>,
        bytes: u64,
        buffered: Arc<AtomicU64>,
    },
    /// Past the bound. The connection is being closed; drop the message.
    Overflowed,
}

impl WsSlot {
    /// Counts `msg` against this connection's queue, or rings the overflow
    /// signal if it would pass the bound.
    ///
    /// The reservation happens *before* the message is handed to the command
    /// channel, because the parked send is the thing being bounded: past the
    /// channel's 16 slots each `send()` leaves a pending future holding a
    /// message, and counting only what the channel accepted would count none of
    /// them.
    fn queue(&self, msg: &Message) -> Queued {
        let bytes = msg.len() as u64;
        if let Some(max) = self.max_buffered {
            // `fetch_update` rather than load-then-add: two concurrent sends
            // that each read the same value below the bound would both be
            // admitted, and the bound would be a suggestion.
            let admitted =
                self.buffered
                    .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |held| {
                        (held.saturating_add(bytes) <= max).then(|| held + bytes)
                    });
            if admitted.is_err() {
                // `notify_one`, not `notify_waiters`: the actor is only
                // registered on this signal while it is parked in its `select!`,
                // and the moment the queue overflows is exactly the moment it is
                // busy draining. `notify_one` leaves a permit behind, so the
                // next park sees it instead of missing it.
                self.overflow.notify_one();
                return Queued::Overflowed;
            }
        } else {
            self.buffered.fetch_add(bytes, Ordering::Relaxed);
        }
        Queued::Accepted {
            tx: self.cmd_tx.clone(),
            bytes,
            buffered: self.buffered.clone(),
        }
    }
}

/// A bound server's queue of accepted (connection id, info), drained by `accept`.
type AcceptRx = mpsc::Receiver<(u64, WebSocketInfo)>;

/// A [`WebSocketProvider`] over real `tokio-tungstenite` connections. The `Arc`s
/// are cloned into each returned future so the futures stay `'static`.
#[derive(Clone, Default)]
pub struct SystemWebSocket {
    conns: Arc<Mutex<HashMap<u64, WsSlot>>>,
    servers: Arc<Mutex<HashMap<u64, AcceptRx>>>,
    /// The accept task per server, kept so `close_server` can abort it — the
    /// same handle [`SystemNet`](crate::SystemNet) keeps for a listener, and for
    /// both of its reasons.
    ///
    /// Dropping the receiver is not a signal the acceptor can see. It parks in
    /// two places — on a connection permit, then on `accept` — and observes the
    /// closure only *after* an accept returns, so at a full cap no arriving
    /// connection can wake it and the listening socket stays bound for as long
    /// as the connections live. For WebSockets that is indefinitely.
    ///
    /// Aborting also drops the task's sender, which is the only thing that can
    /// resolve an `accept` already parked on the channel: it holds the receiver
    /// checked out, so closing cannot reach it through the registry.
    server_tasks: Arc<Mutex<HashMap<u64, tokio::task::JoinHandle<()>>>>,
    next_id: Arc<AtomicU64>,
    /// The process trust store (D197): the bundled Mozilla roots unless
    /// `--tls-ca` replaced them.
    trust: crate::TrustStore,
    /// Addresses `connect` may reach (`--allow-net=<hosts>`). `None` ⇒ any.
    allow: Option<Arc<crate::HostAllowlist>>,
    /// The outbound proxy (`--proxy`, D205) a client `connect` tunnels
    /// through. `None` ⇒ direct.
    proxies: Option<Arc<crate::Proxies>>,
}

impl SystemWebSocket {
    /// Builds an empty connection registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Restricts `connect` to `allow` — `esrun --allow-net=<hosts>` (D38). The
    /// same list `fetch` and `runtime:net` consult: `net` is one capability, and
    /// which provider serves which API is not something a policy should care
    /// about.
    #[must_use]
    pub fn with_allowlist(mut self, allow: crate::HostAllowlist) -> Self {
        self.allow = Some(Arc::new(allow));
        self
    }

    /// Tunnels client connections through `proxies` (D205) — `ws:` and `wss:`
    /// alike, with `CONNECT`, since an upgrade through a forwarding proxy is
    /// not reliable.
    #[must_use]
    pub fn with_proxy(mut self, proxies: Option<Arc<crate::Proxies>>) -> Self {
        self.proxies = proxies;
        self
    }

    /// Verifies `wss:` servers against `trust` — the process store
    /// `--tls-ca` describes (D197) — instead of the bundled Mozilla roots.
    #[must_use]
    pub fn with_trust(mut self, trust: crate::TrustStore) -> Self {
        self.trust = trust;
        self
    }

    fn id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// A TLS client connector for `wss:`, verifying against the process
    /// store. No ALPN is offered: the WebSocket upgrade rides plain HTTP/1.1.
    fn tls_connector(&self) -> TlsConnector {
        TlsConnector::from(Arc::new(self.trust.client_config(&[])))
    }

    /// Spawns the actor task owning `ws` and returns its channel ends. Generic
    /// over the stream so a plain `TcpStream` or a TLS stream drives the same
    /// machinery.
    ///
    /// `permit` is a server's connection slot, moved into the task so it is
    /// released when the connection actually ends rather than when the handshake
    /// finishes. Holding it only until the connection is *established* would
    /// make the cap bound the handshake rate and nothing else, which is the
    /// opposite of what a WebSocket server needs — the connections it holds are
    /// the long-lived ones. `None` for a client `connect`, which no server's
    /// budget applies to.
    fn spawn<S>(
        ws: WebSocketStream<S>,
        permit: Option<OwnedSemaphorePermit>,
        peer_slot: Option<PeerSlot>,
        max_buffered: Option<u64>,
    ) -> WsSlot
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        let (mut sink, mut stream) = ws.split();
        let (inbound_tx, inbound_rx) = mpsc::channel::<WsIncoming>(16);
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<Cmd>(16);
        let buffered = Arc::new(AtomicU64::new(0));
        let overflow = Arc::new(Notify::new());
        let actor_buffered = buffered.clone();
        let actor_overflow = overflow.clone();

        tokio::spawn(async move {
            // Released when this task ends, whichever way it ends — the peer's
            // close, a broken stream, the guest dropping the socket, or a panic.
            let _permit = permit;
            let _peer_slot = peer_slot;
            loop {
                tokio::select! {
                    // A peer so far behind that the queue for it passed the
                    // bound. Answered with a close frame rather than a dropped
                    // connection, so the peer and the guest both learn why:
                    // 1013 is "try again later", which is exactly the situation.
                    () = actor_overflow.notified() => {
                        tracing::debug!(
                            target: "runtime::websocket",
                            buffered = actor_buffered.load(Ordering::Relaxed),
                            "closing: the send queue for this peer passed its bound",
                        );
                        let _ = sink.send(Message::Close(Some(CloseFrame {
                            code: CloseCode::from(WS_TRY_AGAIN_LATER),
                            reason: "send queue overflowed".into(),
                        }))).await;
                        break;
                    },
                    msg = stream.next() => match msg {
                        Some(Ok(Message::Text(t))) => {
                            if inbound_tx.send(WsIncoming::Text(t.as_str().to_string())).await.is_err() {
                                break; // consumer gone
                            }
                        }
                        Some(Ok(Message::Binary(b))) => {
                            if inbound_tx.send(WsIncoming::Binary(b.to_vec())).await.is_err() {
                                break;
                            }
                        }
                        // Control frames stay in the host (the IDL has no ping event).
                        Some(Ok(Message::Ping(p))) => {
                            let _ = sink.send(Message::Pong(p)).await;
                        }
                        Some(Ok(Message::Pong(_))) | Some(Ok(Message::Frame(_))) => {}
                        Some(Ok(Message::Close(frame))) => {
                            let (code, reason) = match frame {
                                Some(cf) => (u16::from(cf.code), cf.reason.as_str().to_string()),
                                None => (NO_STATUS_RCVD, String::new()),
                            };
                            let _ = inbound_tx.send(WsIncoming::Close { code, reason }).await;
                            let _ = sink.send(Message::Close(None)).await; // complete the handshake
                            break;
                        }
                        // Stream error or end without a close: drop inbound_tx so
                        // the next `recv` resolves `None` (an abnormal close, 1006).
                        Some(Err(_)) | None => break,
                    },
                    cmd = cmd_rx.recv() => {
                        // Coalesce a burst of queued sends (e.g. a fan-out that
                        // enqueued one frame per broadcast) into the sink with a
                        // single flush — one socket write for the whole batch
                        // instead of a flush per frame.
                        let mut closing = None;
                        // Bytes this batch takes off the queue, returned to the
                        // count once they have reached the transport — which is
                        // the flush below, not the feed.
                        let mut fed = 0u64;
                        match cmd {
                            Some(Cmd::Send(m)) => {
                                fed += m.len() as u64;
                                if sink.feed(m).await.is_err() { break; }
                            }
                            Some(Cmd::Close { code, reason }) => closing = Some((code, reason)),
                            None => break, // the runtime dropped the socket
                        }
                        let mut broken = false;
                        if closing.is_none() {
                            loop {
                                match cmd_rx.try_recv() {
                                    Ok(Cmd::Send(m)) => {
                                        fed += m.len() as u64;
                                        if sink.feed(m).await.is_err() { broken = true; break; }
                                    }
                                    Ok(Cmd::Close { code, reason }) => {
                                        closing = Some((code, reason));
                                        break;
                                    }
                                    Err(_) => break, // drained (or disconnected)
                                }
                            }
                        }
                        if !broken && sink.flush().await.is_err() { broken = true; }
                        actor_buffered.fetch_sub(fed, Ordering::Relaxed);
                        if let Some((code, reason)) = closing {
                            let frame = code.map(|c| CloseFrame {
                                code: CloseCode::from(c),
                                reason: reason.into(),
                            });
                            let _ = sink.send(Message::Close(frame)).await;
                            // Keep looping to receive the peer's close acknowledgement.
                        }
                        if broken { break; }
                    },
                }
            }
        });

        WsSlot {
            inbound_rx: Some(inbound_rx),
            cmd_tx,
            buffered,
            max_buffered,
            overflow,
        }
    }
}

fn err(e: impl ToString) -> ProviderError {
    ProviderError::Other(e.to_string())
}

fn coded(code: ErrorCode, message: String) -> ProviderError {
    ProviderError::Coded { code, message }
}

/// A tungstenite frame for one outbound message (text ⇒ Text, bytes ⇒ Binary).
fn into_message(message: WsMessage) -> Message {
    match message {
        WsMessage::Text(s) => Message::Text(s.into()),
        WsMessage::Binary(b) => Message::Binary(b.into()),
    }
}

/// TCP keepalive on a client connection, as `fetch` has it.
const TCP_KEEPALIVE: std::time::Duration = std::time::Duration::from_secs(60);

/// The opening handshake, client side (RFC 6455 §4.1): the upgrade request,
/// and a `101` checked the way the RFC says a client must — `Upgrade:
/// websocket`, `Connection: upgrade`, the accept value derived from our key, a
/// subprotocol only from those offered, and no extension we did not offer.
/// Whatever the server sent after its head is handed on with the stream.
async fn client_handshake(
    mut wire: BoxWire,
    url: &url::Url,
    protocols: &[String],
) -> Result<
    (
        WebSocketStream<crate::prefixed::Prefixed<BoxWire>>,
        WebSocketInfo,
    ),
    ProviderError,
> {
    use base64::Engine as _;
    use es_runtime_http::h1::{
        Framing, Limits, ParsedResponse, encode_request_head, parse_response,
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut nonce = [0u8; 16];
    getrandom::fill(&mut nonce).map_err(|e| err(format!("WebSocket key: {e}")))?;
    let key = base64::engine::general_purpose::STANDARD.encode(nonce);
    let target = match url.query() {
        Some(q) => format!("{}?{q}", url.path()),
        None => url.path().to_string(),
    };
    let host = url.host_str().unwrap_or_default();
    let authority = match url.port() {
        Some(p) => format!("{host}:{p}"),
        None => host.to_string(),
    };
    let offered = protocols.join(", ");
    let mut fields: Vec<(&[u8], &[u8])> = vec![
        (b"upgrade", b"websocket"),
        (b"connection", b"Upgrade"),
        (b"sec-websocket-key", key.as_bytes()),
        (b"sec-websocket-version", b"13"),
    ];
    if !protocols.is_empty() {
        fields.push((b"sec-websocket-protocol", offered.as_bytes()));
    }
    let mut head = Vec::with_capacity(256);
    encode_request_head(
        &mut head,
        b"GET",
        target.as_bytes(),
        authority.as_bytes(),
        fields,
        Framing::NoBody,
    )
    .map_err(|e| err(format!("WebSocket handshake: {e}")))?;
    wire.write_all(&head)
        .await
        .map_err(|e| coded(ErrorCode::from_io_kind(e.kind()), e.to_string()))?;

    let limits = Limits::default();
    let mut buf = Vec::with_capacity(1024);
    let mut parsed = Vec::new();
    let response = loop {
        match parse_response(&buf, &limits, &mut parsed, false) {
            Ok(ParsedResponse::Complete(r)) if r.is_interim() => {
                buf.drain(..r.len);
                continue;
            }
            Ok(ParsedResponse::Complete(r)) => break r,
            Ok(ParsedResponse::Partial) => {}
            Err(e) => return Err(err(format!("WebSocket handshake: {e}"))),
        }
        buf.reserve(4096);
        let n = wire
            .read_buf(&mut buf)
            .await
            .map_err(|e| coded(ErrorCode::from_io_kind(e.kind()), e.to_string()))?;
        if n == 0 {
            return Err(coded(
                ErrorCode::ConnectionReset,
                "the connection closed during the WebSocket handshake".to_string(),
            ));
        }
    };
    let field = |name: &[u8]| {
        parsed
            .iter()
            .find(|f| f.name.of(&buf).eq_ignore_ascii_case(name))
            .map(|f| f.value.of(&buf))
    };
    if response.status != 101 {
        return Err(err(format!(
            "the server answered the WebSocket handshake with HTTP {}",
            response.status
        )));
    }
    let refuse = |why: &str| Err(err(format!("WebSocket handshake refused: {why}")));
    if !field(b"upgrade").is_some_and(|v| v.eq_ignore_ascii_case(b"websocket")) {
        return refuse("no `Upgrade: websocket` in the 101");
    }
    let upgrade_token = field(b"connection").is_some_and(|v| {
        v.split(|&b| b == b',')
            .any(|t| es_runtime_http::fields::trim_ows(t).eq_ignore_ascii_case(b"upgrade"))
    });
    if !upgrade_token {
        return refuse("no `Connection: upgrade` in the 101");
    }
    let expected = tokio_tungstenite::tungstenite::handshake::derive_accept_key(key.as_bytes());
    if field(b"sec-websocket-accept") != Some(expected.as_bytes()) {
        return refuse("`Sec-WebSocket-Accept` does not match the key sent");
    }
    let protocol = field(b"sec-websocket-protocol")
        .map(|v| String::from_utf8_lossy(v).into_owned())
        .unwrap_or_default();
    if !protocol.is_empty() && !protocols.contains(&protocol) {
        return refuse("the server chose a subprotocol that was not offered");
    }
    let extensions = field(b"sec-websocket-extensions")
        .map(|v| String::from_utf8_lossy(v).into_owned())
        .unwrap_or_default();
    if !extensions.is_empty() {
        return refuse("the server chose an extension that was not offered");
    }
    let leftover = buf.split_off(response.len);
    let stream = WebSocketStream::from_raw_socket(
        crate::prefixed::Prefixed::new(wire, leftover),
        tokio_tungstenite::tungstenite::protocol::Role::Client,
        None,
    )
    .await;
    Ok((
        stream,
        WebSocketInfo {
            protocol,
            extensions,
        },
    ))
}

impl WebSocketProvider for SystemWebSocket {
    fn connect(
        &self,
        url: String,
        protocols: Vec<String>,
    ) -> BoxFuture<Result<(u64, WebSocketInfo), ProviderError>> {
        let this = self.clone();
        Box::pin(async move {
            let parsed = url::Url::parse(&url).map_err(err)?;
            let secure = parsed.scheme() == "wss";
            let host = parsed
                .host_str()
                .ok_or_else(|| err("WebSocket URL has no host"))?
                .to_string();
            let port = parsed
                .port_or_known_default()
                .ok_or_else(|| err("WebSocket URL has no port"))?;
            // Before the upgrade request is built and before anything is sent.
            if let Some(allow) = &this.allow {
                allow.check(&host, port, "WebSocket connect")?;
            }

            // Reaching the server — directly or through the proxy — and its
            // TLS, within the same deadline `fetch` has (D198).
            let target = host
                .trim_start_matches('[')
                .trim_end_matches(']')
                .to_string();
            let dial = async {
                let proxy = this
                    .proxies
                    .as_ref()
                    .and_then(|p| p.for_target(parsed.scheme(), &target, port));
                let wire: BoxWire = match proxy {
                    Some(server) => server
                        .tunnel(&this.tls_connector(), &target, port)
                        .await
                        .map_err(|e| coded(e.code, e.message))?,
                    None => {
                        // The same connector `fetch` has: the system resolver,
                        // Happy Eyeballs, no Nagle, TCP keepalive.
                        let tcp = crate::tcp_connect::connect(&target, port, TCP_KEEPALIVE)
                            .await
                            .map_err(|e| coded(e.code, e.message))?;
                        Box::new(tcp)
                    }
                };
                if !secure {
                    return Ok::<BoxWire, ProviderError>(wire);
                }
                let server_name = ServerName::try_from(target.clone())
                    .map_err(|_| coded(ErrorCode::Tls, "invalid TLS server name".to_string()))?;
                let tls = this
                    .tls_connector()
                    .connect(server_name, wire)
                    .await
                    .map_err(|e| coded(ErrorCode::Tls, format!("TLS: {e}")))?;
                Ok(Box::new(tls))
            };
            let wire = tokio::time::timeout(crate::http_client::CONNECT_TIMEOUT, dial)
                .await
                .map_err(|_| {
                    coded(
                        ErrorCode::TimedOut,
                        format!(
                            "no connection within {}s",
                            crate::http_client::CONNECT_TIMEOUT.as_secs()
                        ),
                    )
                })??;

            let id = this.id();
            let (stream, info) = client_handshake(wire, &parsed, &protocols).await?;
            let slot = SystemWebSocket::spawn(stream, None, None, Some(DEFAULT_MAX_BUFFERED));
            this.conns.lock().unwrap().insert(id, slot);
            Ok((id, info))
        })
    }

    fn send(&self, id: u64, message: WsMessage) -> BoxFuture<Result<(), ProviderError>> {
        let conns = self.conns.clone();
        Box::pin(async move {
            let msg = into_message(message);
            let queued = {
                let guard = conns.lock().unwrap();
                match guard.get(&id) {
                    Some(slot) => slot.queue(&msg),
                    None => return Err(err("WebSocket is closed")),
                }
            };
            match queued {
                // Over the bound: the connection is on its way out, so the
                // message is dropped rather than queued behind the backlog that
                // caused it. Reported as success — `send()` is fire-and-forget,
                // and the guest learns what happened from the close event, not
                // from a rejection it never awaited.
                Queued::Overflowed => Ok(()),
                Queued::Accepted {
                    tx,
                    bytes,
                    buffered,
                } => {
                    match tx.send(Cmd::Send(msg)).await {
                        Ok(()) => Ok(()),
                        Err(_) => {
                            // The actor is gone, so nothing will decrement this.
                            buffered.fetch_sub(bytes, Ordering::Relaxed);
                            Err(err("WebSocket is closed"))
                        }
                    }
                }
            }
        })
    }

    fn broadcast(&self, ids: Vec<u64>, message: WsMessage) -> BoxFuture<Result<(), ProviderError>> {
        let conns = self.conns.clone();
        Box::pin(async move {
            // Build the frame once and snapshot the live senders, then enqueue to
            // every connection concurrently — the `Message` is refcounted (Bytes/
            // Utf8Bytes), so each clone is O(1), and a slow connection awaits its
            // own channel without blocking the others (no head-of-line stall).
            let msg = into_message(message);
            // Each connection is admitted or refused on its own count: a fan-out
            // is a list of sends, and the one peer that has stopped reading is
            // the one that should be closed for it.
            let queued: Vec<_> = {
                let guard = conns.lock().unwrap();
                ids.iter()
                    .filter_map(|id| guard.get(id))
                    .map(|slot| slot.queue(&msg))
                    .collect()
            };
            let sends = queued.into_iter().filter_map(|queued| match queued {
                Queued::Overflowed => None,
                Queued::Accepted {
                    tx,
                    bytes,
                    buffered,
                } => Some({
                    let msg = msg.clone();
                    async move {
                        // Dropped sockets are skipped; their bytes are returned.
                        if tx.send(Cmd::Send(msg)).await.is_err() {
                            buffered.fetch_sub(bytes, Ordering::Relaxed);
                        }
                    }
                }),
            });
            futures_util::future::join_all(sends).await;
            Ok(())
        })
    }

    fn adopt(&self, io: UpgradedIo) -> BoxFuture<Result<u64, ProviderError>> {
        let this = self.clone();
        Box::pin(async move {
            // The handshake was answered by the HTTP server, so there is nothing
            // to negotiate here — frames start at the first byte, in the
            // **server** role (the side that does not mask).
            //
            // `compat` back the other way: the seam speaks futures-io and
            // tungstenite speaks tokio's traits (D55).
            let stream = WebSocketStream::from_raw_socket(
                io.compat(),
                tokio_tungstenite::tungstenite::protocol::Role::Server,
                None,
            )
            .await;
            let id = this.next_id.fetch_add(1, Ordering::Relaxed) + 1;
            // No server permit and no peer slot: an upgraded connection was
            // already counted by the HTTP server that accepted it, and counting
            // it twice would make one connection spend two budgets.
            let slot = SystemWebSocket::spawn(stream, None, None, Some(DEFAULT_MAX_BUFFERED));
            this.conns.lock().unwrap().insert(id, slot);
            Ok(id)
        })
    }

    fn recv(&self, id: u64) -> BoxFuture<Result<Option<WsIncoming>, ProviderError>> {
        let conns = self.conns.clone();
        Box::pin(async move {
            // Guarded so an abandoned receive cannot take the inbound channel
            // with it and leave every later receive reporting end-of-stream on a
            // live connection (see `checkout`).
            let mut rx = match conns
                .lock()
                .unwrap()
                .get_mut(&id)
                .and_then(|s| s.inbound_rx.take())
            {
                Some(rx) => {
                    let back = conns.clone();
                    Checkout::new(rx, move |rx| {
                        if let Some(slot) = back.lock().unwrap().get_mut(&id) {
                            slot.inbound_rx = Some(rx);
                        }
                    })
                }
                None => return Ok(None), // closed or already ended
            };
            match rx.get_mut().recv().await {
                // A close frame is the last thing a connection delivers: the
                // actor sends it and ends. So the entry goes now, with the
                // frame, rather than on a later `recv` that sees the channel
                // closed — the guest's pump stops at the close event and never
                // makes that call, which left one entry behind for every
                // connection the peer closed.
                Some(item @ WsIncoming::Close { .. }) => {
                    rx.keep_out();
                    conns.lock().unwrap().remove(&id);
                    Ok(Some(item))
                }
                Some(item) => Ok(Some(item)),
                // The actor ended (clean close already delivered, or abnormal):
                // drop the registry entry and signal end-of-stream.
                None => {
                    rx.keep_out();
                    conns.lock().unwrap().remove(&id);
                    Ok(None)
                }
            }
        })
    }

    fn close(
        &self,
        id: u64,
        code: Option<u16>,
        reason: String,
    ) -> BoxFuture<Result<(), ProviderError>> {
        let conns = self.conns.clone();
        Box::pin(async move {
            let tx = conns.lock().unwrap().get(&id).map(|s| s.cmd_tx.clone());
            if let Some(tx) = tx {
                let _ = tx.send(Cmd::Close { code, reason }).await;
            }
            Ok(())
        })
    }

    fn serve(
        &self,
        options: WsServeOptions,
    ) -> BoxFuture<Result<(u64, SocketInfo), ProviderError>> {
        let this = self.clone();
        Box::pin(async move {
            // Built before the bind, so a bad certificate fails `serve` rather
            // than every connection after the port is open.
            let site = match options.tls {
                Some(tls) => Some(crate::tls::ServerTls::new(
                    crate::tls::ServerTlsMaterial {
                        cert: tls.cert,
                        key: tls.key,
                        client_ca: tls.client_ca,
                        crl: tls.crl,
                        request_cert: tls.request_cert,
                    },
                    &tls.alpn,
                )?),
                None => None,
            };
            let listener = TcpListener::bind((options.host.as_str(), options.port))
                .await
                .map_err(err)?;
            let local = listener.local_addr().ok();
            let (tx, rx) = mpsc::channel::<(u64, WebSocketInfo)>(64);
            let conns = this.conns.clone();
            let next_id = this.next_id.clone();
            let handshake_timeout = options.timeouts.handshake;
            // One permit per connection this server may hold, taken *before*
            // `accept` and released when the connection ends. At the cap the
            // acceptor simply stops accepting: excess connections wait in the
            // kernel's backlog and are refused by the OS once that fills, so
            // nothing is spent on a connection this server will not serve. Same
            // mechanism as the HTTP server (D45).
            let slots = options
                .max_connections
                .map(|max| Arc::new(Semaphore::new(max)));
            // The other half of the cap: whose connections they are. Taken after
            // `accept`, because that is when the peer is known, and a refusal
            // closes rather than waits — see [`crate::peer_limit`].
            let per_peer = PeerLimit::new(options.max_connections_per_ip);
            let max_buffered = options.max_buffered_amount;
            // Accept loop: each TCP connection's WS handshake runs in its own task
            // so a slow handshake never blocks the next accept; on success the
            // connection registers in the shared `conns` map and is queued for
            // `accept`.
            let task = tokio::spawn(async move {
                // Errors from `accept` are retried, never fatal — see
                // [`AcceptBackoff`](crate::accept_backoff). Breaking here would
                // end the server while the port stayed bound, on an error that
                // says nothing about the listening socket.
                let mut backoff = AcceptBackoff::new();
                loop {
                    // Held across the accept, then handed to the connection it
                    // admits. `acquire_owned` only fails on a closed semaphore
                    // and nothing closes this one, so a failure would mean the
                    // cap had silently stopped applying — end the loop rather
                    // than serve unbounded.
                    let permit = match &slots {
                        None => None,
                        Some(slots) => match slots.clone().acquire_owned().await {
                            Ok(permit) => Some(permit),
                            Err(_) => break,
                        },
                    };
                    // The peer is kept, not discarded: it is the only thing that
                    // makes a failed handshake attributable to a client.
                    let (tcp, peer) = match listener.accept().await {
                        Ok(accepted) => {
                            backoff.reset();
                            accepted
                        }
                        Err(e) => {
                            let delay = backoff.next_delay();
                            tracing::warn!(
                                target: "runtime::websocket",
                                error = %e,
                                backoff_ms = delay.as_millis() as u64,
                                "accept failed; retrying",
                            );
                            tokio::time::sleep(delay).await;
                            continue;
                        }
                    };
                    if tx.is_closed() {
                        break; // server closed (accept rx dropped)
                    }
                    // Before the handshake, the task and the span: a refusal
                    // here costs a close, and the descriptor comes straight
                    // back.
                    let peer_slot = match &per_peer {
                        None => None,
                        Some(per_peer) => match per_peer.take(peer.ip()) {
                            Some(slot) => Some(slot),
                            None => {
                                tracing::debug!(
                                    target: "runtime::websocket",
                                    peer = %peer,
                                    "refused: this address is at its connection limit",
                                );
                                continue;
                            }
                        },
                    };
                    let _ = tcp.set_nodelay(true);
                    let tx = tx.clone();
                    let conns = conns.clone();
                    let next_id = next_id.clone();
                    let site = site.clone();
                    // One span per connection, at `debug`, so the handshake
                    // failure below and anything logged after it are
                    // attributable to a peer. See the same shape in
                    // [`system_http`](crate::system_http).
                    let span = tracing::debug_span!(
                        target: "runtime::websocket",
                        "connection",
                        peer = %peer,
                    );
                    tokio::spawn(
                        async move {
                            // Moved in so the slot is released whichever way
                            // this connection ends, including a failed
                            // handshake — and, on success, handed to the actor
                            // task so it lives as long as the connection does.
                            let permit = permit;
                            let peer_slot = peer_slot;
                            // TLS first on a `wss:` server, then the opening
                            // handshake; one deadline covers both. A slot taken
                            // above goes with whichever way this ends.
                            let establish = async move {
                                match site {
                                    None => {
                                        let ws = tokio_tungstenite::accept_async(tcp)
                                            .await
                                            .map_err(|e| e.to_string())?;
                                        Ok::<_, String>(SystemWebSocket::spawn(
                                            ws,
                                            permit,
                                            peer_slot,
                                            max_buffered,
                                        ))
                                    }
                                    Some(site) => {
                                        let tls = site
                                            .acceptor()
                                            .accept(tcp)
                                            .await
                                            .map_err(|e| format!("tls handshake: {e}"))?;
                                        let ws = tokio_tungstenite::accept_async(tls)
                                            .await
                                            .map_err(|e| e.to_string())?;
                                        Ok(SystemWebSocket::spawn(
                                            ws,
                                            permit,
                                            peer_slot,
                                            max_buffered,
                                        ))
                                    }
                                }
                            };
                            // A peer that opens a connection and never sends its
                            // upgrade request is the cheapest hold there is: one
                            // syscall to the peer, a task and a descriptor to us,
                            // and the handshake waits for that request forever.
                            let handshake = match handshake_timeout {
                                None => Some(establish.await),
                                Some(limit) => match tokio::time::timeout(limit, establish).await {
                                    Ok(done) => Some(done),
                                    Err(_) => {
                                        tracing::debug!(
                                            target: "runtime::websocket",
                                            timeout_ms = limit.as_millis() as u64,
                                            "handshake timed out; closing the connection",
                                        );
                                        None
                                    }
                                },
                            };
                            let slot = match handshake {
                                Some(Ok(slot)) => slot,
                                // A plain HTTP request to a WebSocket port, a
                                // failed TLS handshake, a missing
                                // `Sec-WebSocket-Key`. Debug, not warn: it is
                                // peer-driven, so warning would let any client
                                // set this server's log volume.
                                Some(Err(e)) => {
                                    tracing::debug!(
                                        target: "runtime::websocket",
                                        error = %e,
                                        "websocket handshake failed",
                                    );
                                    return;
                                }
                                // Already logged, above.
                                None => return,
                            };
                            let id = next_id.fetch_add(1, Ordering::Relaxed) + 1;
                            conns.lock().unwrap().insert(id, slot);
                            if tx.send((id, WebSocketInfo::default())).await.is_err() {
                                conns.lock().unwrap().remove(&id); // server gone before accept
                            }
                        }
                        .instrument(span),
                    );
                }
            });
            let server_id = this.id();
            this.servers.lock().unwrap().insert(server_id, rx);
            this.server_tasks.lock().unwrap().insert(server_id, task);
            let info = SocketInfo {
                local_address: local.map(|a| a.ip().to_string()).unwrap_or_default(),
                local_port: local.map(|a| a.port()).unwrap_or(0),
                ..Default::default()
            };
            Ok((server_id, info))
        })
    }

    fn accept(&self, id: u64) -> BoxFuture<Result<Option<(u64, WebSocketInfo)>, ProviderError>> {
        let servers = self.servers.clone();
        Box::pin(async move {
            // Guarded: an abandoned accept would take the server's channel with
            // it and every later accept would report the server closed.
            let mut rx = match servers.lock().unwrap().remove(&id) {
                Some(rx) => {
                    let back = servers.clone();
                    Checkout::new(rx, move |rx| {
                        back.lock().unwrap().insert(id, rx);
                    })
                }
                None => return Ok(None), // server closed
            };
            let conn = rx.get_mut().recv().await;
            drop(rx); // back to the registry — keep accepting
            Ok(conn)
        })
    }

    fn close_server(&self, id: u64) -> BoxFuture<Result<(), ProviderError>> {
        let servers = self.servers.clone();
        let tasks = self.server_tasks.clone();
        Box::pin(async move {
            servers.lock().unwrap().remove(&id);
            // Aborting is what actually stops this server: it drops the
            // listening socket (so the port comes back now rather than whenever
            // a slot next frees) and drops the task's sender (so an `accept`
            // already parked on the channel resolves to `None` instead of
            // waiting for a connection that can no longer arrive).
            //
            // Connections already accepted are untouched — they are separate
            // actor tasks in `conns`, and the trait promises they keep working
            // until individually closed.
            if let Some(task) = tasks.lock().unwrap().remove(&id) {
                task.abort();
            }
            Ok(())
        })
    }
}

/// The client's opening handshake, against a server that answers by hand.
#[cfg(test)]
mod handshake_tests {
    use super::*;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// A one-connection server: reads the request head, then writes whatever
    /// `answer` makes of the key it was sent.
    async fn answering(answer: fn(&str) -> Vec<u8>) -> u16 {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                if s.read(&mut byte).await.unwrap_or(0) == 0 {
                    return;
                }
                head.push(byte[0]);
            }
            let text = String::from_utf8_lossy(&head).to_string();
            let key = text
                .lines()
                .find_map(|l| l.strip_prefix("sec-websocket-key: "))
                .unwrap_or_default()
                .to_string();
            s.write_all(&answer(&key)).await.unwrap();
            tokio::time::sleep(Duration::from_millis(300)).await;
        });
        port
    }

    fn accept(key: &str) -> String {
        tokio_tungstenite::tungstenite::handshake::derive_accept_key(key.as_bytes())
    }

    async fn connect(port: u16, protocols: &[&str]) -> Result<(u64, WebSocketInfo), ProviderError> {
        SystemWebSocket::default()
            .connect(
                format!("ws://127.0.0.1:{port}/chat?room=1"),
                protocols.iter().map(|p| p.to_string()).collect(),
            )
            .await
    }

    #[tokio::test]
    async fn a_good_101_connects_and_bytes_after_it_are_the_first_frame() {
        let port = answering(|key| {
            let mut out = format!(
                "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\nSec-WebSocket-Protocol: chat\r\n\r\n",
                accept(key)
            )
            .into_bytes();
            // An unmasked text frame "hi", sent in the same write as the head.
            out.extend_from_slice(&[0x81, 0x02, b'h', b'i']);
            out
        })
        .await;
        let ws = SystemWebSocket::default();
        let (id, info) = ws
            .connect(
                format!("ws://127.0.0.1:{port}/chat?room=1"),
                vec!["chat".into(), "v2".into()],
            )
            .await
            .unwrap();
        assert_eq!(info.protocol, "chat");
        let first = ws.recv(id).await.unwrap();
        assert!(matches!(first, Some(WsIncoming::Text(ref t)) if t == "hi"));
    }

    #[tokio::test]
    async fn a_101_that_breaks_the_handshake_is_refused() {
        for (answer, why) in [
            (
                (|_: &str| {
                    b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: wrong\r\n\r\n".to_vec()
                }) as fn(&str) -> Vec<u8>,
                "Accept",
            ),
            (
                |key: &str| {
                    format!("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\nSec-WebSocket-Protocol: other\r\n\r\n", accept(key)).into_bytes()
                },
                "subprotocol",
            ),
            (
                |key: &str| {
                    format!("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\nSec-WebSocket-Extensions: permessage-deflate\r\n\r\n", accept(key)).into_bytes()
                },
                "extension",
            ),
            (
                |key: &str| {
                    format!("HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n", accept(key)).into_bytes()
                },
                "Upgrade",
            ),
            (
                |_: &str| b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_vec(),
                "HTTP 404",
            ),
        ] {
            let port = answering(answer).await;
            let Err(e) = connect(port, &["chat"]).await else {
                panic!("a broken handshake ({why}) was accepted");
            };
            let e = e.to_string();
            assert!(e.contains(why), "expected {why:?} in: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use es_runtime_providers::WsTimeouts;
    use futures_util::{SinkExt, StreamExt};
    use tokio::net::TcpListener;

    /// `serve()` on an ephemeral loopback port with the default posture.
    pub(super) async fn bound(sys: &SystemWebSocket) -> (u64, SocketInfo) {
        bound_with(sys, WsTimeouts::default(), None).await
    }

    /// `serve()` on an ephemeral loopback port with a chosen posture.
    pub(super) async fn bound_with(
        sys: &SystemWebSocket,
        timeouts: WsTimeouts,
        max_connections: Option<usize>,
    ) -> (u64, SocketInfo) {
        sys.serve(WsServeOptions {
            host: "127.0.0.1".to_string(),
            tls: None,
            port: 0,
            timeouts,
            max_connections,
            max_connections_per_ip: None,
            max_buffered_amount: Some(DEFAULT_MAX_BUFFERED),
        })
        .await
        .unwrap()
    }

    /// A minimal echo server: accept the WebSocket, bounce every data frame, and
    /// let tungstenite drive the closing handshake when the peer closes.
    async fn echo<S>(mut ws: WebSocketStream<S>)
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    {
        while let Some(Ok(msg)) = ws.next().await {
            if msg.is_text() || msg.is_binary() {
                let _ = ws.send(msg).await;
            }
        }
    }

    // A full plaintext round-trip over loopback: text + binary echo and a clean
    // closing handshake (the peer's 1000/"bye" comes back via recv).
    #[tokio::test]
    async fn ws_echoes_text_and_binary_then_closes_cleanly() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            echo(tokio_tungstenite::accept_async(tcp).await.unwrap()).await;
        });

        let client = SystemWebSocket::new();
        let (id, info) = client
            .connect(format!("ws://127.0.0.1:{port}/"), vec![])
            .await
            .unwrap();
        assert_eq!(info.protocol, "");

        client
            .send(id, WsMessage::Text("hello".to_string()))
            .await
            .unwrap();
        match client.recv(id).await.unwrap() {
            Some(WsIncoming::Text(t)) => assert_eq!(t, "hello"),
            _ => panic!("expected a text echo"),
        }

        client
            .send(id, WsMessage::Binary(vec![1, 2, 3]))
            .await
            .unwrap();
        match client.recv(id).await.unwrap() {
            Some(WsIncoming::Binary(b)) => assert_eq!(b, vec![1, 2, 3]),
            _ => panic!("expected a binary echo"),
        }

        client
            .close(id, Some(1000), "bye".to_string())
            .await
            .unwrap();
        match client.recv(id).await.unwrap() {
            Some(WsIncoming::Close { code, reason }) => {
                assert_eq!(code, 1000);
                assert_eq!(reason, "bye");
            }
            _ => panic!("expected a clean close"),
        }
        server.await.unwrap();
    }

    // D197: `wss:` verifies against the process store. A server its private
    // CA signed is refused by the bundled roots and by a store holding a
    // different CA.
    #[tokio::test]
    async fn wss_refuses_a_server_the_trust_store_does_not_hold() {
        use crate::trust::test_pki::{Ca, tls_server};
        let ca = Ca::new("Private CA");
        let other = Ca::new("Other CA");
        for client in [
            SystemWebSocket::new(),
            SystemWebSocket::new().with_trust(other.trust()),
        ] {
            let (port, server) = tls_server(ca.leaf(&["localhost"]), &[], b"").await;
            let result = client
                .connect(format!("wss://localhost:{port}/"), vec![])
                .await;
            server.abort();
            assert!(result.is_err(), "an untrusted wss: server was accepted");
        }
    }

    // The same round-trip over `wss:` against a hermetic self-signed TLS server,
    // reusing the rustls stack from D28 (aws-lc-rs provider, test-injected roots).
    #[tokio::test]
    async fn wss_echoes_over_tls() {
        use tokio_rustls::TlsAcceptor;
        use tokio_rustls::rustls::ServerConfig;
        use tokio_rustls::rustls::crypto::aws_lc_rs;
        use tokio_rustls::rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};

        let ck = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
        let cert = ck.cert.der().clone();
        let key = PrivateKeyDer::from(PrivatePkcs8KeyDer::from(ck.signing_key.serialize_der()));

        let server_cfg =
            ServerConfig::builder_with_provider(Arc::new(aws_lc_rs::default_provider()))
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_no_client_auth()
                .with_single_cert(vec![cert.clone()], key)
                .unwrap();
        let acceptor = TlsAcceptor::from(Arc::new(server_cfg));

        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let tls = acceptor.accept(tcp).await.unwrap();
            echo(tokio_tungstenite::accept_async(tls).await.unwrap()).await;
        });

        // Trust only the self-signed cert; SNI "localhost" matches it.
        let client = SystemWebSocket::new()
            .with_trust(crate::TrustStore::from_certificates([cert]).unwrap());
        let (id, _info) = client
            .connect(format!("wss://localhost:{port}/"), vec![])
            .await
            .unwrap();
        client
            .send(id, WsMessage::Text("secure".to_string()))
            .await
            .unwrap();
        match client.recv(id).await.unwrap() {
            Some(WsIncoming::Text(t)) => assert_eq!(t, "secure"),
            _ => panic!("expected a tls text echo"),
        }
        client.close(id, Some(1000), String::new()).await.unwrap();
        let _ = client.recv(id).await;
        server.await.unwrap();
    }

    // Abandoning a receive must not take the connection's inbound channel with
    // it — see `system_http`'s cancel_safety_tests for why an embedder can do
    // this at all. Before the checkout guard, every later receive on a live
    // connection reported end-of-stream.
    #[tokio::test]
    async fn an_abandoned_recv_leaves_the_connection_receiving() {
        let sys = SystemWebSocket::new();
        let (server_id, info) = bound(&sys).await;
        let port = info.local_port;
        let (cid, _) = sys
            .connect(format!("ws://127.0.0.1:{port}/"), vec![])
            .await
            .unwrap();
        let (conn_id, _) = sys.accept(server_id).await.unwrap().expect("a connection");

        // Nothing has been sent yet, so this parks — then give up on it.
        let abandoned =
            tokio::time::timeout(std::time::Duration::from_millis(100), sys.recv(conn_id)).await;
        assert!(abandoned.is_err(), "the receive parked, as this test needs");

        sys.send(cid, WsMessage::Text("after".to_string()))
            .await
            .unwrap();
        let got = tokio::time::timeout(std::time::Duration::from_secs(10), sys.recv(conn_id))
            .await
            .expect("the inbound channel survived the abandoned receive")
            .unwrap();
        match got {
            Some(WsIncoming::Text(t)) => assert_eq!(t, "after"),
            _ => panic!("expected the text message sent after the abandoned receive"),
        }
        sys.close(cid, Some(1000), String::new()).await.unwrap();
    }

    // The third accept loop with the same invariant as the HTTP server's and
    // `runtime:net`'s: it keeps looping. It used to leave on the first error,
    // which ended the server while the port stayed bound. The errno is not
    // provokable in-process (the retry policy is unit-tested in
    // `accept_backoff`), so what is asserted is that connections abandoned on
    // arrival leave the server able to accept the next real one.
    #[tokio::test]
    async fn the_accept_loop_survives_connections_abandoned_on_arrival() {
        let sys = SystemWebSocket::new();
        let (server_id, info) = bound(&sys).await;
        let port = info.local_port;

        // TCP connections that never attempt the WebSocket handshake.
        for _ in 0..32 {
            drop(TcpStream::connect(("127.0.0.1", port)).await.unwrap());
        }

        let (cid, _) = sys
            .connect(format!("ws://127.0.0.1:{port}/"), vec![])
            .await
            .expect("a real client still connects after the burst");
        let accepted =
            tokio::time::timeout(std::time::Duration::from_secs(10), sys.accept(server_id))
                .await
                .expect("the accept loop is still accepting")
                .unwrap();
        assert!(accepted.is_some(), "the connection after the burst arrives");
        sys.close(cid, Some(1000), String::new()).await.unwrap();
    }

    // The server side: serve → accept → the accepted connection echoes back
    // (text + binary) over the same send/recv/close used by client sockets.
    #[tokio::test]
    async fn ws_server_accepts_and_echoes() {
        let sys = SystemWebSocket::new();
        let (server_id, info) = bound(&sys).await;
        let port = info.local_port;

        // A client connects to our own server.
        let (cid, _) = sys
            .connect(format!("ws://127.0.0.1:{port}/"), vec![])
            .await
            .unwrap();

        // Accept it, then echo whatever the client sends.
        let (conn_id, _) = sys.accept(server_id).await.unwrap().expect("a connection");

        sys.send(cid, WsMessage::Text("ping".to_string()))
            .await
            .unwrap();
        match sys.recv(conn_id).await.unwrap() {
            Some(WsIncoming::Text(t)) => {
                assert_eq!(t, "ping");
                sys.send(conn_id, WsMessage::Text(t.to_uppercase()))
                    .await
                    .unwrap();
            }
            _ => panic!("server expected a text frame"),
        }
        match sys.recv(cid).await.unwrap() {
            Some(WsIncoming::Text(t)) => assert_eq!(t, "PING"),
            _ => panic!("client expected the echo"),
        }

        sys.send(cid, WsMessage::Binary(vec![9, 8, 7]))
            .await
            .unwrap();
        match sys.recv(conn_id).await.unwrap() {
            Some(WsIncoming::Binary(b)) => {
                assert_eq!(b, vec![9, 8, 7]);
                sys.send(conn_id, WsMessage::Binary(b)).await.unwrap();
            }
            _ => panic!("server expected a binary frame"),
        }
        match sys.recv(cid).await.unwrap() {
            Some(WsIncoming::Binary(b)) => assert_eq!(b, vec![9, 8, 7]),
            _ => panic!("client expected the binary echo"),
        }

        sys.close(cid, Some(1000), "bye".to_string()).await.unwrap();
        match sys.recv(conn_id).await.unwrap() {
            Some(WsIncoming::Close { code, reason }) => {
                assert_eq!(code, 1000);
                assert_eq!(reason, "bye");
            }
            _ => panic!("server expected the close handshake"),
        }
        sys.close_server(server_id).await.unwrap();
    }

    // Delivering the peer's close frees the connection's entry, on both ends.
    // The guest's pump stops at the close event and never calls `recv` again,
    // so an entry left for that call is never freed — one per closed
    // connection, for as long as the server runs.
    #[tokio::test]
    async fn delivering_a_close_frees_the_connection() {
        let sys = SystemWebSocket::new();
        let (server_id, info) = bound(&sys).await;
        let (cid, _) = sys
            .connect(format!("ws://127.0.0.1:{}/", info.local_port), vec![])
            .await
            .unwrap();
        let (conn_id, _) = sys.accept(server_id).await.unwrap().expect("a connection");

        sys.close(cid, Some(1000), "bye".to_string()).await.unwrap();
        assert!(matches!(
            sys.recv(conn_id).await.unwrap(),
            Some(WsIncoming::Close { code: 1000, .. })
        ));
        // The side that closed sees its handshake complete as the end of the
        // stream, which frees its entry the way it always has.
        assert!(matches!(
            sys.recv(cid).await.unwrap(),
            None | Some(WsIncoming::Close { .. })
        ));
        assert!(sys.conns.lock().unwrap().is_empty());

        // Afterwards the id reads as ended, as it always did once freed.
        assert!(sys.recv(conn_id).await.unwrap().is_none());
        sys.close(conn_id, None, String::new()).await.unwrap();
        sys.close_server(server_id).await.unwrap();
    }

    // broadcast() fans one message out to many connections in a single call —
    // both accepted clients receive it.
    #[tokio::test]
    async fn ws_server_broadcast_reaches_all() {
        let sys = SystemWebSocket::new();
        let (server_id, info) = bound(&sys).await;
        let port = info.local_port;

        let (c1, _) = sys
            .connect(format!("ws://127.0.0.1:{port}/"), vec![])
            .await
            .unwrap();
        let (c2, _) = sys
            .connect(format!("ws://127.0.0.1:{port}/"), vec![])
            .await
            .unwrap();
        let (s1, _) = sys.accept(server_id).await.unwrap().expect("conn 1");
        let (s2, _) = sys.accept(server_id).await.unwrap().expect("conn 2");

        sys.broadcast(vec![s1, s2], WsMessage::Text("hi-all".to_string()))
            .await
            .unwrap();

        for c in [c1, c2] {
            match sys.recv(c).await.unwrap() {
                Some(WsIncoming::Text(t)) => assert_eq!(t, "hi-all"),
                _ => panic!("each client should receive the broadcast"),
            }
        }
        sys.close_server(server_id).await.unwrap();
    }

    // A server that drops without a closing handshake: recv resolves `None`, the
    // signal the prelude turns into an abnormal close (1006).
    #[tokio::test]
    async fn ws_abnormal_close_when_server_vanishes() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let _ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
            // Drop the connection immediately — no close frame.
        });

        let client = SystemWebSocket::new();
        let (id, _) = client
            .connect(format!("ws://127.0.0.1:{port}/"), vec![])
            .await
            .unwrap();
        server.await.unwrap();
        assert!(client.recv(id).await.unwrap().is_none());
    }

    // --- The outbound proxy (D205) ------------------------------------------

    /// A one-connection WebSocket echo server on loopback, plain or TLS.
    async fn echo_once(leaf: Option<crate::trust::test_pki::Leaf>) -> u16 {
        use tokio_rustls::rustls::ServerConfig;
        use tokio_rustls::rustls::crypto::aws_lc_rs;
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            async fn echo<S>(stream: S)
            where
                S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
            {
                let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
                if let Some(Ok(message)) = ws.next().await {
                    let _ = ws.send(message).await;
                }
            }
            match leaf {
                None => echo(tcp).await,
                Some(leaf) => {
                    let config = ServerConfig::builder_with_provider(Arc::new(
                        aws_lc_rs::default_provider(),
                    ))
                    .with_safe_default_protocol_versions()
                    .unwrap()
                    .with_no_client_auth()
                    .with_single_cert(leaf.chain, leaf.key)
                    .unwrap();
                    let tls = tokio_rustls::TlsAcceptor::from(Arc::new(config))
                        .accept(tcp)
                        .await
                        .unwrap();
                    echo(tls).await;
                }
            }
        });
        port
    }

    fn proxied(proxy_port: u16, no_proxy: &[&str]) -> Option<Arc<crate::Proxies>> {
        let list: Vec<String> = no_proxy.iter().map(|s| s.to_string()).collect();
        Some(
            crate::Proxies::from_flags(&format!("http://u:p@127.0.0.1:{proxy_port}"), &list)
                .unwrap()
                .shared(),
        )
    }

    async fn round_trip(client: &SystemWebSocket, url: String) {
        let (id, _) = client.connect(url, vec![]).await.unwrap();
        client
            .send(id, WsMessage::Text("through".to_string()))
            .await
            .unwrap();
        match client.recv(id).await.unwrap() {
            Some(WsIncoming::Text(t)) => assert_eq!(t, "through"),
            _ => panic!("expected the echo"),
        }
    }

    #[tokio::test]
    async fn a_ws_client_tunnels_through_the_proxy_with_credentials() {
        use crate::proxy::test_proxy::{self, Mode};
        let port = echo_once(None).await;
        let proxy = test_proxy::start(Mode::Auth("Basic dTpw")).await;
        let client = SystemWebSocket::new().with_proxy(proxied(proxy.port, &[]));
        round_trip(&client, format!("ws://chat.test:{port}/room")).await;
        let heads = proxy.heads();
        assert!(
            heads[0].starts_with(&format!("CONNECT chat.test:{port} HTTP/1.1\r\n")),
            "{heads:?}"
        );
    }

    #[tokio::test]
    async fn a_wss_client_verifies_the_server_inside_the_tunnel() {
        use crate::proxy::test_proxy::{self, Mode};
        use crate::trust::test_pki::Ca;
        let ca = Ca::new("WS Proxy CA");
        let port = echo_once(Some(ca.leaf(&["chat.test"]))).await;
        let proxy = test_proxy::start(Mode::Auth("Basic dTpw")).await;
        let client = SystemWebSocket::new()
            .with_trust(ca.trust())
            .with_proxy(proxied(proxy.port, &[]));
        round_trip(&client, format!("wss://chat.test:{port}/")).await;
        assert_eq!(proxy.heads().len(), 1);
    }

    #[tokio::test]
    async fn a_ws_exception_goes_direct() {
        use crate::proxy::test_proxy::{self, Mode};
        let proxy = test_proxy::start(Mode::Open).await;
        let client = SystemWebSocket::new().with_proxy(proxied(proxy.port, &["chat.test"]));
        // Direct, `chat.test` does not resolve: proof it did not go through.
        let Err(error) = client
            .connect("ws://chat.test:9/".to_string(), vec![])
            .await
        else {
            panic!("an exception was proxied");
        };
        assert!(error.code().is_some(), "an uncoded connect error: {error}");
        assert!(proxy.heads().is_empty());
        // Loopback always goes direct, whatever the list says.
        let port = echo_once(None).await;
        let client = SystemWebSocket::new().with_proxy(proxied(proxy.port, &[]));
        round_trip(&client, format!("ws://127.0.0.1:{port}/")).await;
        assert!(proxy.heads().is_empty());
    }

    #[tokio::test]
    async fn a_refused_ws_tunnel_is_err_proxy_tunnel() {
        use crate::proxy::test_proxy::{self, Mode};
        let proxy = test_proxy::start(Mode::Refuse).await;
        let client = SystemWebSocket::new().with_proxy(proxied(proxy.port, &[]));
        let Err(error) = client.connect("wss://chat.test/".to_string(), vec![]).await else {
            panic!("a refused tunnel connected");
        };
        assert_eq!(
            error.code(),
            Some(es_runtime_common::ErrorCode::ProxyTunnel),
            "{error}"
        );
        assert!(!error.to_string().contains("u:p"), "{error}");
    }

    #[tokio::test]
    async fn connect_refuses_a_url_outside_the_allowlist() {
        // Same list `fetch` and `runtime:net` consult — one capability, one
        // policy, whichever API the guest reached for. Nothing is listening on
        // the refused address: the check precedes the socket.
        let client = SystemWebSocket::new()
            .with_allowlist(crate::HostAllowlist::parse(["chat.example.com:443"]).unwrap());
        let err = client
            .connect("ws://evil.test:8080/".to_string(), vec![])
            .await
            .err()
            .expect("a URL outside the list must be refused");
        assert_eq!(
            err.code(),
            Some(es_runtime_common::ErrorCode::PermissionDenied)
        );
        assert!(err.to_string().contains("evil.test:8080"), "{err}");
    }

    #[tokio::test]
    async fn connect_permits_a_url_on_the_allowlist() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
            echo(ws).await;
        });
        let client = SystemWebSocket::new()
            .with_allowlist(crate::HostAllowlist::parse([format!("127.0.0.1:{port}")]).unwrap());
        let (id, _) = client
            .connect(format!("ws://127.0.0.1:{port}/"), vec![])
            .await
            .expect("the allowed address connects");
        client.close(id, None, String::new()).await.unwrap();
        server.await.unwrap();
    }
}

/// A WebSocket port that answers nothing gives an operator no way to tell a
/// client sending the wrong handshake from a server that is not being called.
/// This event is the difference.
#[cfg(test)]
mod tracing_tests {
    use super::*;
    use crate::trace_capture;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn a_failed_handshake_is_logged_with_its_peer() {
        trace_capture::install();
        let ws = SystemWebSocket::default();
        let (sid, addr) = super::tests::bound(&ws).await;

        // A plain HTTP request at a WebSocket port: no `Upgrade`, no
        // `Sec-WebSocket-Key` — tungstenite refuses it.
        let mut tcp = tokio::net::TcpStream::connect(("127.0.0.1", addr.local_port))
            .await
            .unwrap();
        let peer = tcp.local_addr().unwrap();
        tcp.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();

        let mine = trace_capture::wait_for(
            &["websocket handshake failed", &format!("peer={peer}")],
            std::time::Duration::from_secs(5),
        )
        .await;
        assert!(
            !mine.is_empty(),
            "the handshake failure must be logged against the peer that caused it; saw: {:?}",
            trace_capture::lines_containing(&["websocket handshake failed"]),
        );
        let line = &mine[0];
        assert!(
            line.contains("[DEBUG] runtime::websocket"),
            "peer-driven failures log at debug on the websocket target: {line}",
        );
        assert!(
            line.contains("error="),
            "the reason is the whole point of the event: {line}",
        );
        ws.close_server(sid).await.unwrap();
    }
}

/// The two bounds a WebSocket server holds on connections that are not yet, or
/// no longer, doing anything useful. Both are the same mechanisms the HTTP
/// server uses (D43, D45), against real sockets on loopback.
#[cfg(test)]
mod bounds_tests {
    use super::tests::{bound, bound_with};
    use super::*;
    use es_runtime_providers::WsTimeouts;
    use std::time::Duration;

    /// Waits until `sock` is closed by the peer, up to `grace`; `false` if it
    /// was still open when time ran out.
    async fn closed_within(sock: &mut TcpStream, grace: Duration) -> bool {
        let mut buf = [0u8; 1];
        match tokio::time::timeout(grace, tokio::io::AsyncReadExt::read(sock, &mut buf)).await {
            Ok(Ok(0)) => true,  // clean EOF — the server hung up
            Ok(Err(_)) => true, // reset
            _ => false,         // still open, or it sent us something
        }
    }

    /// A peer that opens a connection and never sends its upgrade request is the
    /// cheapest hold there is — one syscall to it, a task and a descriptor to
    /// us — and tungstenite waits for that request indefinitely.
    #[tokio::test]
    async fn a_connection_that_never_sends_a_handshake_is_closed() {
        let sys = SystemWebSocket::new();
        let (server_id, info) = bound_with(
            &sys,
            WsTimeouts {
                handshake: Some(Duration::from_millis(300)),
            },
            None,
        )
        .await;

        let mut sock = TcpStream::connect(("127.0.0.1", info.local_port))
            .await
            .unwrap();
        assert!(
            closed_within(&mut sock, Duration::from_secs(5)).await,
            "a connection that never starts its handshake must be closed",
        );
        sys.close_server(server_id).await.unwrap();
    }

    /// And the timeout must not reach an *established* connection. A WebSocket
    /// that has said nothing since its handshake is idle, not stalled — closing
    /// it is the application's decision, not the transport's.
    #[tokio::test]
    async fn an_established_connection_outlives_the_handshake_timeout() {
        let sys = SystemWebSocket::new();
        let (server_id, info) = bound_with(
            &sys,
            WsTimeouts {
                handshake: Some(Duration::from_millis(200)),
            },
            None,
        )
        .await;
        let port = info.local_port;

        let (cid, _) = sys
            .connect(format!("ws://127.0.0.1:{port}/"), vec![])
            .await
            .unwrap();
        let accepted = tokio::time::timeout(Duration::from_secs(10), sys.accept(server_id))
            .await
            .expect("accepted")
            .unwrap()
            .expect("a connection");

        // Well past the handshake deadline, saying nothing the whole time.
        tokio::time::sleep(Duration::from_millis(800)).await;

        sys.send(cid, WsMessage::Text("still here".into()))
            .await
            .unwrap();
        let got = tokio::time::timeout(Duration::from_secs(5), sys.recv(accepted.0))
            .await
            .expect("the idle connection is still open")
            .unwrap();
        assert!(
            matches!(got, Some(WsIncoming::Text(t)) if t == "still here"),
            "an established connection must not be reaped by the handshake timeout",
        );
        sys.close_server(server_id).await.unwrap();
    }

    /// Disabling it must actually disable it, not fall back to the default.
    #[tokio::test]
    async fn a_disabled_handshake_timeout_never_fires() {
        let sys = SystemWebSocket::new();
        let (server_id, info) = bound_with(&sys, WsTimeouts { handshake: None }, None).await;

        let mut sock = TcpStream::connect(("127.0.0.1", info.local_port))
            .await
            .unwrap();
        assert!(
            !closed_within(&mut sock, Duration::from_millis(700)).await,
            "with the timeout off, a silent connection must be left alone",
        );
        sys.close_server(server_id).await.unwrap();
    }

    /// The gap D47 recorded: `send()` is fire-and-forget, so writing faster
    /// than a peer reads never stalls the guest — it queues on the host, one
    /// pending send per message, bounded by nothing. A peer that stops reading
    /// a fan-out was a memory leak with a network interface.
    ///
    /// The peer here reads nothing at all, so everything sent to it backs up
    /// past the bound and the connection is closed rather than held.
    #[tokio::test]
    async fn a_peer_that_stops_reading_is_closed_rather_than_queued_for() {
        let sys = SystemWebSocket::new();
        // Small enough to reach with a handful of messages, and far past what
        // the 16-slot command channel absorbs.
        let (server_id, info) = sys
            .serve(WsServeOptions {
                host: "127.0.0.1".to_string(),
                tls: None,
                port: 0,
                timeouts: WsTimeouts::default(),
                max_connections: None,
                max_connections_per_ip: None,
                max_buffered_amount: Some(64 * 1024),
            })
            .await
            .unwrap();
        let port = info.local_port;

        // A raw socket that completes the handshake and then never reads: the
        // kernel's receive window fills, and everything after that is ours to
        // hold. `sys.connect` would drain, which is the opposite of the case.
        let (silent, _) = tokio_tungstenite::client_async(
            format!("ws://127.0.0.1:{port}/"),
            TcpStream::connect(("127.0.0.1", port)).await.unwrap(),
        )
        .await
        .unwrap();

        let (id, _) = tokio::time::timeout(Duration::from_secs(10), sys.accept(server_id))
            .await
            .expect("the connection is accepted")
            .unwrap()
            .expect("a connection");

        // 32 MiB in 64 KiB messages, *concurrently* — which is what a guest
        // doing `for (…) conn.send(chunk)` produces, since `send()` is not
        // awaited. Sending them one at a time would let the actor drain between
        // each and the queue would never build, which is the healthy case the
        // test below covers.
        let payload = vec![0u8; 64 * 1024];
        let sends = (0..512).map(|_| sys.send(id, WsMessage::Binary(payload.clone())));
        futures_util::future::join_all(sends).await;

        // The connection is gone: `recv` resolves rather than parking forever,
        // which is what it would do for a healthy but quiet peer.
        let ended = tokio::time::timeout(Duration::from_secs(10), sys.recv(id))
            .await
            .expect("an overflowing connection must be closed, not held");
        assert!(
            ended.is_err() || ended.unwrap().is_none(),
            "the connection outlived its send queue"
        );

        drop(silent);
        sys.close_server(server_id).await.unwrap();
    }

    /// The other half: a peer that keeps up is never closed, however much goes
    /// through it. Without this the bound above would just be a message cap.
    #[tokio::test]
    async fn a_peer_that_keeps_reading_is_never_closed_for_the_bound() {
        let sys = SystemWebSocket::new();
        let (server_id, info) = sys
            .serve(WsServeOptions {
                host: "127.0.0.1".to_string(),
                tls: None,
                port: 0,
                timeouts: WsTimeouts::default(),
                max_connections: None,
                max_connections_per_ip: None,
                max_buffered_amount: Some(64 * 1024),
            })
            .await
            .unwrap();
        let port = info.local_port;

        let (client, _) = sys
            .connect(format!("ws://127.0.0.1:{port}/"), vec![])
            .await
            .unwrap();
        let (id, _) = tokio::time::timeout(Duration::from_secs(10), sys.accept(server_id))
            .await
            .expect("the connection is accepted")
            .unwrap()
            .expect("a connection");

        // 1 MiB — sixteen times the bound — through a peer that drains it.
        let payload = vec![7u8; 16 * 1024];
        for _ in 0..64 {
            sys.send(id, WsMessage::Binary(payload.clone()))
                .await
                .expect("a drained connection accepts every send");
            let got = tokio::time::timeout(Duration::from_secs(10), sys.recv(client))
                .await
                .expect("the peer keeps up")
                .unwrap();
            assert!(
                matches!(got, Some(WsIncoming::Binary(b)) if b.len() == 16 * 1024),
                "the peer read something other than the message"
            );
        }

        sys.close_server(server_id).await.unwrap();
    }

    /// A WebSocket connection is long-lived by design, so one peer holding
    /// every slot is not something churn takes back — the whole-server cap
    /// bounds what the deployment spends and says nothing about whose
    /// connections they are. Here the flooder is held to its share.
    ///
    /// The refusal is a **close**, unlike the whole-server cap's hold: the
    /// second connection's handshake fails rather than parking, which is
    /// exactly the difference this policy is asserting.
    #[tokio::test]
    async fn one_peer_cannot_hold_every_slot() {
        let sys = SystemWebSocket::new();
        let (server_id, info) = sys
            .serve(WsServeOptions {
                host: "127.0.0.1".to_string(),
                tls: None,
                port: 0,
                timeouts: WsTimeouts::default(),
                max_connections: Some(8),
                max_connections_per_ip: Some(1),
                max_buffered_amount: Some(DEFAULT_MAX_BUFFERED),
            })
            .await
            .unwrap();
        let port = info.local_port;

        let (first, _) = sys
            .connect(format!("ws://127.0.0.1:{port}/"), vec![])
            .await
            .unwrap();
        let accepted = tokio::time::timeout(Duration::from_secs(10), sys.accept(server_id))
            .await
            .expect("the peer's first connection is accepted")
            .unwrap()
            .expect("a connection");

        // The second from the same address is refused: the server closes it
        // before the handshake, so `connect` fails rather than parking.
        let refused = tokio::time::timeout(
            Duration::from_secs(10),
            sys.connect(format!("ws://127.0.0.1:{port}/"), vec![]),
        )
        .await
        .expect("a refused connection fails rather than hanging");
        assert!(
            refused.is_err(),
            "a second connection from a capped peer was served"
        );

        // …and the peer's share comes back when its connection ends. Both ends
        // are closed: the slot belongs to the *server's* actor task, which
        // lives until its stream does.
        let (accepted_id, _) = accepted;
        sys.close(first, Some(1000), String::new()).await.unwrap();
        sys.close(accepted_id, Some(1000), String::new())
            .await
            .unwrap();

        // Retried rather than raced: the slot is returned when the actor task
        // ends, which is a scheduling event, not something `close` awaits.
        let reconnected = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if sys
                    .connect(format!("ws://127.0.0.1:{port}/"), vec![])
                    .await
                    .is_ok()
                {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        assert!(reconnected.is_ok(), "a returned share was not reusable");

        sys.close_server(server_id).await.unwrap();
    }

    /// The cap holds a connection back rather than refusing it: over the limit
    /// nothing is accepted, and the moment a slot frees the waiting connection
    /// is served. That is the difference between a queue and a rejection.
    #[tokio::test]
    async fn a_connection_over_the_cap_waits_for_a_slot_rather_than_being_refused() {
        let sys = SystemWebSocket::new();
        let (server_id, info) = bound_with(&sys, WsTimeouts::default(), Some(1)).await;
        let port = info.local_port;

        // Fills the single slot, and holds it: a WebSocket connection is not
        // done until it closes.
        let (first, _) = sys
            .connect(format!("ws://127.0.0.1:{port}/"), vec![])
            .await
            .unwrap();
        let first_accepted = tokio::time::timeout(Duration::from_secs(10), sys.accept(server_id))
            .await
            .expect("the first connection is accepted")
            .unwrap()
            .expect("a connection");

        // The second completes its TCP connect — the kernel's backlog takes it —
        // but must not become an accepted WebSocket while the slot is held.
        let waiting = tokio::spawn({
            let sys = sys.clone();
            async move { sys.connect(format!("ws://127.0.0.1:{port}/"), vec![]).await }
        });
        let too_early =
            tokio::time::timeout(Duration::from_millis(700), sys.accept(server_id)).await;
        assert!(
            too_early.is_err(),
            "a connection over the cap must not be served while the cap is full",
        );

        // Free the slot. The waiting connection was held, not refused, so it is
        // served now rather than having been dropped.
        sys.close(first, Some(1000), String::new()).await.unwrap();
        drop(first_accepted);
        let second = tokio::time::timeout(Duration::from_secs(10), sys.accept(server_id))
            .await
            .expect("the held connection is served once a slot frees")
            .unwrap();
        assert!(
            second.is_some(),
            "the held connection arrives, not an error"
        );
        let _ = waiting.await;
        sys.close_server(server_id).await.unwrap();
    }

    /// A failed handshake must give its slot back. If it did not, the cap would
    /// count attempts rather than connections — and a peer that opens and
    /// abandons connections could exhaust it without ever holding one.
    #[tokio::test]
    async fn a_failed_handshake_returns_its_slot() {
        let sys = SystemWebSocket::new();
        let (server_id, info) = bound_with(&sys, WsTimeouts::default(), Some(1)).await;
        let port = info.local_port;

        // Plain HTTP at a WebSocket port, several times over: each takes the
        // single slot and must hand it straight back.
        for _ in 0..4 {
            let mut tcp = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
            let _ = tokio::io::AsyncWriteExt::write_all(
                &mut tcp,
                b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n",
            )
            .await;
            let _ = closed_within(&mut tcp, Duration::from_secs(5)).await;
        }

        let (cid, _) = tokio::time::timeout(
            Duration::from_secs(10),
            sys.connect(format!("ws://127.0.0.1:{port}/"), vec![]),
        )
        .await
        .expect("the slot was returned by the failed handshakes")
        .unwrap();
        let accepted = tokio::time::timeout(Duration::from_secs(10), sys.accept(server_id))
            .await
            .expect("a real connection is accepted afterwards")
            .unwrap();
        assert!(accepted.is_some());
        sys.close(cid, Some(1000), String::new()).await.unwrap();
        sys.close_server(server_id).await.unwrap();
    }

    /// Closing a server must release the listening socket. The acceptor parks in
    /// two places — on a permit and then on `accept` — and a dropped receiver is
    /// visible from neither, so nothing but aborting the task can end it while a
    /// full cap holds it. At the cap this is the difference between the port
    /// coming back and it staying bound for as long as the connections live,
    /// which for WebSockets is indefinitely.
    #[tokio::test]
    async fn closing_a_capped_server_releases_the_port() {
        let sys = SystemWebSocket::new();
        let (server_id, info) = bound_with(&sys, WsTimeouts::default(), Some(1)).await;
        let port = info.local_port;

        // Fill the one slot and keep it filled: the acceptor is now parked on
        // the semaphore, where no arriving connection can wake it.
        let (cid, _) = sys
            .connect(format!("ws://127.0.0.1:{port}/"), vec![])
            .await
            .unwrap();
        let _held = tokio::time::timeout(Duration::from_secs(10), sys.accept(server_id))
            .await
            .expect("the first connection is accepted")
            .unwrap()
            .expect("a connection");

        sys.close_server(server_id).await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;

        assert!(
            TcpStream::connect(("127.0.0.1", port)).await.is_err(),
            "the port must be released by close_server, not held until a slot frees",
        );
        let _ = cid;
    }

    /// And a guest already waiting in `for await (const ws of server)` must be
    /// let go. A parked `accept` holds the receiver checked out, so closing
    /// cannot reach it through the registry — only dropping the sender does,
    /// which is what aborting the acceptor achieves.
    #[tokio::test]
    async fn closing_a_server_resolves_a_parked_accept() {
        let sys = SystemWebSocket::new();
        let (server_id, _info) = bound(&sys).await;

        let parked = tokio::spawn({
            let sys = sys.clone();
            async move { sys.accept(server_id).await }
        });
        // Let it get as far as awaiting the channel.
        tokio::time::sleep(Duration::from_millis(200)).await;
        sys.close_server(server_id).await.unwrap();

        let got = tokio::time::timeout(Duration::from_secs(5), parked)
            .await
            .expect("a parked accept must not outlive the server that closed")
            .unwrap()
            .unwrap();
        assert!(got.is_none(), "a closed server's accept resolves to None");
    }
}
