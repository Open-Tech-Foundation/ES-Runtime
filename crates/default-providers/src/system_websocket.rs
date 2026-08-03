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
//! complete the TLS handshake ourselves and hand the established stream to
//! `client_async`, so no TLS feature of `tokio-tungstenite` is pulled in.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use es_runtime_providers::{
    BoxFuture, ProviderError, SocketInfo, WebSocketInfo, WebSocketProvider, WsIncoming, WsMessage,
    WsServeOptions,
};
use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::crypto::aws_lc_rs;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::client_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::handshake::client::Response;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::http::header::SEC_WEBSOCKET_PROTOCOL;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tracing::Instrument;

use crate::accept_backoff::AcceptBackoff;
use crate::checkout::Checkout;

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
}

/// A bound server's queue of accepted (connection id, info), drained by `accept`.
type AcceptRx = mpsc::Receiver<(u64, WebSocketInfo)>;

/// A [`WebSocketProvider`] over real `tokio-tungstenite` connections. The `Arc`s
/// are cloned into each returned future so the futures stay `'static`.
#[derive(Clone, Default)]
pub struct SystemWebSocket {
    conns: Arc<Mutex<HashMap<u64, WsSlot>>>,
    servers: Arc<Mutex<HashMap<u64, AcceptRx>>>,
    next_id: Arc<AtomicU64>,
    /// TLS trust anchors. `None` ⇒ the bundled Mozilla roots (webpki-roots);
    /// tests inject a custom store via [`SystemWebSocket::with_tls_roots`].
    tls_roots: Option<Arc<RootCertStore>>,
    /// Addresses `connect` may reach (`--allow-net=<hosts>`). `None` ⇒ any.
    allow: Option<Arc<crate::HostAllowlist>>,
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

    /// Like [`new`](Self::new), but trusting `roots` for `wss:` TLS instead of
    /// the bundled Mozilla set — the test seam for a hermetic self-signed server.
    #[cfg(test)]
    fn with_tls_roots(roots: Arc<RootCertStore>) -> Self {
        Self {
            tls_roots: Some(roots),
            ..Self::default()
        }
    }

    fn id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// TLS trust anchors: the test override if set, else the bundled Mozilla
    /// roots (built once). Shares the rationale of `SystemNet::tls_roots` (D28).
    fn tls_roots(&self) -> Arc<RootCertStore> {
        if let Some(roots) = &self.tls_roots {
            return roots.clone();
        }
        static WEBPKI: OnceLock<Arc<RootCertStore>> = OnceLock::new();
        WEBPKI
            .get_or_init(|| {
                let mut store = RootCertStore::empty();
                store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
                Arc::new(store)
            })
            .clone()
    }

    /// A TLS client connector for `wss:`. The `aws-lc-rs` provider is selected
    /// explicitly (both ring and aws-lc-rs are linked, so the process default is
    /// ambiguous and `ClientConfig::builder()` would panic — DECISIONS D28). No
    /// ALPN is offered: the WebSocket upgrade rides plain HTTP/1.1.
    fn tls_connector(&self) -> Result<TlsConnector, ProviderError> {
        let provider = Arc::new(aws_lc_rs::default_provider());
        let config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(err)?
            .with_root_certificates(self.tls_roots())
            .with_no_client_auth();
        Ok(TlsConnector::from(Arc::new(config)))
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
    fn spawn<S>(ws: WebSocketStream<S>, permit: Option<OwnedSemaphorePermit>) -> WsSlot
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        let (mut sink, mut stream) = ws.split();
        let (inbound_tx, inbound_rx) = mpsc::channel::<WsIncoming>(16);
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<Cmd>(16);

        tokio::spawn(async move {
            // Released when this task ends, whichever way it ends — the peer's
            // close, a broken stream, the guest dropping the socket, or a panic.
            let _permit = permit;
            loop {
                tokio::select! {
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
                        match cmd {
                            Some(Cmd::Send(m)) => {
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
        }
    }
}

fn err(e: impl ToString) -> ProviderError {
    ProviderError::Other(e.to_string())
}

/// A tungstenite frame for one outbound message (text ⇒ Text, bytes ⇒ Binary).
fn into_message(message: WsMessage) -> Message {
    match message {
        WsMessage::Text(s) => Message::Text(s.into()),
        WsMessage::Binary(b) => Message::Binary(b.into()),
    }
}

/// The negotiated subprotocol + extensions from the handshake response headers.
fn info_of(response: &Response) -> WebSocketInfo {
    let header = |name| {
        response
            .headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string()
    };
    WebSocketInfo {
        protocol: header("sec-websocket-protocol"),
        extensions: header("sec-websocket-extensions"),
    }
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

            // Build the upgrade request, carrying any offered subprotocols.
            let mut request = url.as_str().into_client_request().map_err(err)?;
            if !protocols.is_empty() {
                let value = HeaderValue::from_str(&protocols.join(", ")).map_err(err)?;
                request.headers_mut().insert(SEC_WEBSOCKET_PROTOCOL, value);
            }

            let tcp = TcpStream::connect((host.as_str(), port))
                .await
                .map_err(err)?;
            let _ = tcp.set_nodelay(true);

            let id = this.id();
            let (info, slot) = if secure {
                let server_name = ServerName::try_from(host.clone())
                    .map_err(|_| err("invalid TLS server name"))?;
                let tls = this
                    .tls_connector()?
                    .connect(server_name, tcp)
                    .await
                    .map_err(err)?;
                let (stream, response) = client_async(request, tls).await.map_err(err)?;
                (info_of(&response), SystemWebSocket::spawn(stream, None))
            } else {
                let (stream, response) = client_async(request, tcp).await.map_err(err)?;
                (info_of(&response), SystemWebSocket::spawn(stream, None))
            };
            this.conns.lock().unwrap().insert(id, slot);
            Ok((id, info))
        })
    }

    fn send(&self, id: u64, message: WsMessage) -> BoxFuture<Result<(), ProviderError>> {
        let conns = self.conns.clone();
        Box::pin(async move {
            let msg = into_message(message);
            let tx = conns.lock().unwrap().get(&id).map(|s| s.cmd_tx.clone());
            match tx {
                Some(tx) => tx
                    .send(Cmd::Send(msg))
                    .await
                    .map_err(|_| err("WebSocket is closed")),
                None => Err(err("WebSocket is closed")),
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
            let txs: Vec<_> = {
                let guard = conns.lock().unwrap();
                ids.iter()
                    .filter_map(|id| guard.get(id).map(|s| s.cmd_tx.clone()))
                    .collect()
            };
            let sends = txs.into_iter().map(|tx| {
                let msg = msg.clone();
                async move {
                    let _ = tx.send(Cmd::Send(msg)).await; // dropped sockets are skipped
                }
            });
            futures_util::future::join_all(sends).await;
            Ok(())
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
            // Accept loop: each TCP connection's WS handshake runs in its own task
            // so a slow handshake never blocks the next accept; on success the
            // connection registers in the shared `conns` map and is queued for
            // `accept`.
            tokio::spawn(async move {
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
                    let _ = tcp.set_nodelay(true);
                    let tx = tx.clone();
                    let conns = conns.clone();
                    let next_id = next_id.clone();
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
                            // A peer that opens a connection and never sends its
                            // upgrade request is the cheapest hold there is: one
                            // syscall to the peer, a task and a descriptor to us,
                            // and tungstenite waits for that request forever.
                            let handshake = match handshake_timeout {
                                None => Some(tokio_tungstenite::accept_async(tcp).await),
                                Some(limit) => {
                                    match tokio::time::timeout(
                                        limit,
                                        tokio_tungstenite::accept_async(tcp),
                                    )
                                    .await
                                    {
                                        Ok(done) => Some(done),
                                        Err(_) => {
                                            tracing::debug!(
                                                target: "runtime::websocket",
                                                timeout_ms = limit.as_millis() as u64,
                                                "handshake timed out; closing the connection",
                                            );
                                            None
                                        }
                                    }
                                }
                            };
                            let ws = match handshake {
                                Some(Ok(ws)) => ws,
                                // A plain HTTP request to a WebSocket port, a
                                // missing `Sec-WebSocket-Key`, a peer that
                                // opened a connection and said nothing. Debug,
                                // not warn: it is peer-driven, so warning would
                                // let any client set this server's log volume.
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
                            let slot = SystemWebSocket::spawn(ws, permit);
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
        Box::pin(async move {
            servers.lock().unwrap().remove(&id);
            Ok(())
        })
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
            port: 0,
            timeouts,
            max_connections,
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

    // The same round-trip over `wss:` against a hermetic self-signed TLS server,
    // reusing the rustls stack from D28 (aws-lc-rs provider, test-injected roots).
    #[tokio::test]
    async fn wss_echoes_over_tls() {
        use tokio_rustls::TlsAcceptor;
        use tokio_rustls::rustls::ServerConfig;
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
        let mut roots = RootCertStore::empty();
        roots.add(cert).unwrap();
        let client = SystemWebSocket::with_tls_roots(Arc::new(roots));
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
    use super::tests::bound_with;
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
}
