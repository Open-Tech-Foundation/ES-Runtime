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
};
use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
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
}

impl SystemWebSocket {
    /// Builds an empty connection registry.
    pub fn new() -> Self {
        Self::default()
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
    fn spawn<S>(ws: WebSocketStream<S>) -> WsSlot
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        let (mut sink, mut stream) = ws.split();
        let (inbound_tx, inbound_rx) = mpsc::channel::<WsIncoming>(16);
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<Cmd>(16);

        tokio::spawn(async move {
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
                    cmd = cmd_rx.recv() => match cmd {
                        Some(Cmd::Send(m)) => {
                            if sink.send(m).await.is_err() {
                                break;
                            }
                        }
                        Some(Cmd::Close { code, reason }) => {
                            let frame = code.map(|c| CloseFrame {
                                code: CloseCode::from(c),
                                reason: reason.into(),
                            });
                            let _ = sink.send(Message::Close(frame)).await;
                            // Keep looping to receive the peer's close acknowledgement.
                        }
                        None => break, // the runtime dropped the socket
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
                (info_of(&response), SystemWebSocket::spawn(stream))
            } else {
                let (stream, response) = client_async(request, tcp).await.map_err(err)?;
                (info_of(&response), SystemWebSocket::spawn(stream))
            };
            this.conns.lock().unwrap().insert(id, slot);
            Ok((id, info))
        })
    }

    fn send(&self, id: u64, message: WsMessage) -> BoxFuture<Result<(), ProviderError>> {
        let conns = self.conns.clone();
        Box::pin(async move {
            let msg = match message {
                WsMessage::Text(s) => Message::Text(s.into()),
                WsMessage::Binary(b) => Message::Binary(b.into()),
            };
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

    fn recv(&self, id: u64) -> BoxFuture<Result<Option<WsIncoming>, ProviderError>> {
        let conns = self.conns.clone();
        Box::pin(async move {
            let mut rx = match conns
                .lock()
                .unwrap()
                .get_mut(&id)
                .and_then(|s| s.inbound_rx.take())
            {
                Some(rx) => rx,
                None => return Ok(None), // closed or already ended
            };
            match rx.recv().await {
                Some(item) => {
                    if let Some(slot) = conns.lock().unwrap().get_mut(&id) {
                        slot.inbound_rx = Some(rx);
                    }
                    Ok(Some(item))
                }
                // The actor ended (clean close already delivered, or abnormal):
                // drop the registry entry and signal end-of-stream.
                None => {
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
        host: String,
        port: u16,
    ) -> BoxFuture<Result<(u64, SocketInfo), ProviderError>> {
        let this = self.clone();
        Box::pin(async move {
            let listener = TcpListener::bind((host.as_str(), port))
                .await
                .map_err(err)?;
            let local = listener.local_addr().ok();
            let (tx, rx) = mpsc::channel::<(u64, WebSocketInfo)>(64);
            let conns = this.conns.clone();
            let next_id = this.next_id.clone();
            // Accept loop: each TCP connection's WS handshake runs in its own task
            // so a slow handshake never blocks the next accept; on success the
            // connection registers in the shared `conns` map and is queued for
            // `accept`.
            tokio::spawn(async move {
                loop {
                    let (tcp, _) = match listener.accept().await {
                        Ok(c) => c,
                        Err(_) => break,
                    };
                    if tx.is_closed() {
                        break; // server closed (accept rx dropped)
                    }
                    let _ = tcp.set_nodelay(true);
                    let tx = tx.clone();
                    let conns = conns.clone();
                    let next_id = next_id.clone();
                    tokio::spawn(async move {
                        let ws = match tokio_tungstenite::accept_async(tcp).await {
                            Ok(ws) => ws,
                            Err(_) => return, // failed handshake
                        };
                        let id = next_id.fetch_add(1, Ordering::Relaxed) + 1;
                        let slot = SystemWebSocket::spawn(ws);
                        conns.lock().unwrap().insert(id, slot);
                        if tx.send((id, WebSocketInfo::default())).await.is_err() {
                            conns.lock().unwrap().remove(&id); // server gone before accept
                        }
                    });
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
            let mut rx = match servers.lock().unwrap().remove(&id) {
                Some(rx) => rx,
                None => return Ok(None), // server closed
            };
            let conn = rx.recv().await;
            servers.lock().unwrap().insert(id, rx); // keep accepting
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
    use futures_util::{SinkExt, StreamExt};
    use tokio::net::TcpListener;

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

    // The server side: serve → accept → the accepted connection echoes back
    // (text + binary) over the same send/recv/close used by client sockets.
    #[tokio::test]
    async fn ws_server_accepts_and_echoes() {
        let sys = SystemWebSocket::new();
        let (server_id, info) = sys.serve("127.0.0.1".to_string(), 0).await.unwrap();
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
}
