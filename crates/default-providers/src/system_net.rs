//! OS-backed [`NetProvider`] — tokio TCP sockets for `runtime:net` (SPEC §12).
//!
//! Each socket's I/O runs in **spawned runtime tasks** (a reader and a writer)
//! that move bytes over channels; the ops just send/recv on those channels.
//! This is the same shape the HTTP client uses: the actual I/O is driven by the
//! runtime's reactor (via spawned tasks), so reads that must wait for bytes make
//! progress — polling the raw socket future inline from the op loop would not.
//!
//! `connect({ secureTransport: "on" })` negotiates TLS (rustls via tokio-rustls,
//! the `aws-lc-rs` provider, `webpki-roots` trust anchors) with SNI + ALPN before
//! the same reader/writer tasks take over the encrypted stream (DECISIONS D28).

use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::task::{Context, Poll};

use es_runtime_providers::{BoxFuture, ConnectOptions, NetProvider, ProviderError, SocketInfo};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::crypto::aws_lc_rs;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};

type ReadRx = mpsc::Receiver<Result<Vec<u8>, String>>;
type WriteTx = mpsc::Sender<Vec<u8>>;
type AcceptRx = mpsc::Receiver<(TcpStream, SocketAddr)>;
type TcpRead = tokio::io::ReadHalf<TcpStream>;
type TcpWrite = tokio::io::WriteHalf<TcpStream>;

/// Handles that pull a plaintext socket's raw halves back out of its reader and
/// writer tasks so the stream can be wrapped in TLS in place (`startTls`). Each
/// task parks on its receiver; sending it a one-shot sender makes it stop and
/// hand the half back. Present only on plaintext client sockets — `None` once a
/// socket is already TLS, accepted, or upgraded.
struct Reclaim {
    read: oneshot::Sender<oneshot::Sender<TcpRead>>,
    write: oneshot::Sender<oneshot::Sender<TcpWrite>>,
}

/// A connection's channel ends. `read_rx` is taken out during a read; `write_tx`
/// is cloned to send and dropped (set to `None`) to half-close. `reclaim` is
/// taken once, by `startTls`, to upgrade the socket.
struct Slot {
    read_rx: Option<ReadRx>,
    write_tx: Option<WriteTx>,
    reclaim: Option<Reclaim>,
}

/// A [`NetProvider`] over real tokio TCP sockets. The `Arc`s are cloned into each
/// returned future so the futures stay `'static`.
#[derive(Clone, Default)]
pub struct SystemNet {
    sockets: Arc<Mutex<HashMap<u64, Slot>>>,
    listeners: Arc<Mutex<HashMap<u64, AcceptRx>>>,
    next_id: Arc<AtomicU64>,
    /// TLS trust anchors. `None` ⇒ the bundled Mozilla roots (webpki-roots);
    /// tests inject a custom store via [`SystemNet::with_tls_roots`].
    tls_roots: Option<Arc<RootCertStore>>,
    /// Memoized TLS client connectors, keyed by the offered ALPN list (the only
    /// per-connect input to the config). Building a [`ClientConfig`] re-parses
    /// the whole root store, so this is shared across clones and reused for every
    /// connect with the same ALPN set; a `TlsConnector` is an `Arc` inside, so a
    /// cache hit is a refcount bump.
    tls_connectors: Arc<Mutex<HashMap<Vec<String>, TlsConnector>>>,
}

impl SystemNet {
    /// Builds an empty socket registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Like [`new`](Self::new), but trusting `roots` for TLS instead of the
    /// bundled Mozilla set. Test seam for hermetic TLS against a self-signed
    /// server (no public CA involved).
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
    /// roots (built once). webpki-roots needs no platform I/O, so runs are
    /// portable and deterministic.
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

    /// A TLS client connector trusting [`tls_roots`](Self::tls_roots) and
    /// offering `alpn`, memoized by ALPN set (see [`tls_connectors`](Self::tls_connectors)).
    /// The `aws-lc-rs` provider is chosen explicitly because the process-default
    /// crypto provider is ambiguous (both ring and aws-lc-rs are linked, so
    /// `ClientConfig::builder()` would panic).
    fn tls_connector(&self, alpn: &[String]) -> Result<TlsConnector, ProviderError> {
        if let Some(connector) = self.tls_connectors.lock().unwrap().get(alpn) {
            return Ok(connector.clone());
        }
        let provider = Arc::new(aws_lc_rs::default_provider());
        let mut config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(err)?
            .with_root_certificates(self.tls_roots())
            .with_no_client_auth();
        config.alpn_protocols = alpn.iter().map(|p| p.as_bytes().to_vec()).collect();
        let connector = TlsConnector::from(Arc::new(config));
        self.tls_connectors
            .lock()
            .unwrap()
            .insert(alpn.to_vec(), connector.clone());
        Ok(connector)
    }

    /// Splits `stream` and spawns its reader + writer tasks, returning the
    /// channel ends to register. Generic over the stream so the same machinery
    /// drives a plain [`TcpStream`] or a TLS stream.
    fn spawn_socket<S>(stream: S) -> Slot
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + 'static,
    {
        let (mut r, mut w) = tokio::io::split(stream);
        let (read_tx, read_rx) = mpsc::channel::<Result<Vec<u8>, String>>(8);
        let (write_tx, mut write_rx) = mpsc::channel::<Vec<u8>>(8);

        tokio::spawn(async move {
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                match r.read(&mut buf).await {
                    Ok(0) => break, // EOF — dropping read_tx signals it
                    Ok(n) => {
                        if read_tx.send(Ok(buf[..n].to_vec())).await.is_err() {
                            break; // consumer gone
                        }
                    }
                    Err(e) => {
                        let _ = read_tx.send(Err(e.to_string())).await;
                        break;
                    }
                }
            }
        });

        tokio::spawn(async move {
            while let Some(data) = write_rx.recv().await {
                if w.write_all(&data).await.is_err() {
                    break;
                }
            }
            let _ = w.shutdown().await; // write_tx dropped (half-close / close)
        });

        Slot {
            read_rx: Some(read_rx),
            write_tx: Some(write_tx),
            reclaim: None,
        }
    }

    /// Like [`spawn_socket`](Self::spawn_socket), but for a plaintext [`TcpStream`]
    /// that may later be upgraded with `startTls`. The reader and writer keep
    /// their halves reclaimable: each `select!`s its normal work against a
    /// reclaim request, and on request hands its half back instead of looping, so
    /// [`start_tls`](Self::start_tls) can rejoin the raw stream and wrap it in TLS.
    fn spawn_upgradable(tcp: TcpStream) -> Slot {
        let (mut r, mut w) = tokio::io::split(tcp);
        let (read_tx, read_rx) = mpsc::channel::<Result<Vec<u8>, String>>(8);
        let (write_tx, mut write_rx) = mpsc::channel::<Vec<u8>>(8);
        let (recl_read_tx, mut recl_read_rx) = oneshot::channel::<oneshot::Sender<TcpRead>>();
        let (recl_write_tx, mut recl_write_rx) = oneshot::channel::<oneshot::Sender<TcpWrite>>();

        tokio::spawn(async move {
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                tokio::select! {
                    biased;
                    // Reclaim wins over a pending read; the cancelled read is
                    // cancel-safe (no bytes consumed), so nothing is lost.
                    give = &mut recl_read_rx => {
                        if let Ok(give) = give {
                            let _ = give.send(r);
                        }
                        return; // upgraded or closed — stop reading
                    }
                    res = r.read(&mut buf) => match res {
                        Ok(0) => break, // EOF — dropping read_tx signals it
                        Ok(n) => {
                            if read_tx.send(Ok(buf[..n].to_vec())).await.is_err() {
                                break; // consumer gone
                            }
                        }
                        Err(e) => {
                            let _ = read_tx.send(Err(e.to_string())).await;
                            break;
                        }
                    },
                }
            }
        });

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    give = &mut recl_write_rx => {
                        // Flush whatever is still queued before handing the half
                        // back (upgrade) or sending FIN (close).
                        while let Ok(data) = write_rx.try_recv() {
                            if w.write_all(&data).await.is_err() {
                                break;
                            }
                        }
                        match give {
                            Ok(give) => {
                                let _ = w.flush().await;
                                let _ = give.send(w);
                            }
                            Err(_) => {
                                let _ = w.shutdown().await;
                            }
                        }
                        return;
                    }
                    data = write_rx.recv() => match data {
                        Some(data) => {
                            if w.write_all(&data).await.is_err() {
                                break;
                            }
                        }
                        None => {
                            let _ = w.shutdown().await; // write_tx dropped (half-close / close)
                            break;
                        }
                    },
                }
            }
        });

        Slot {
            read_rx: Some(read_rx),
            write_tx: Some(write_tx),
            reclaim: Some(Reclaim {
                read: recl_read_tx,
                write: recl_write_tx,
            }),
        }
    }
}

/// A reclaimed plaintext stream with any bytes the reader task had already
/// buffered (but the guest never read) replayed ahead of the live socket, so a
/// `startTls` upgrade keeps anything the peer sent between its go-ahead and the
/// TLS handshake.
struct Prefixed {
    prefix: io::Cursor<Vec<u8>>,
    inner: TcpStream,
}

impl AsyncRead for Prefixed {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let pos = self.prefix.position() as usize;
        let bytes = self.prefix.get_ref();
        if pos < bytes.len() {
            let n = (bytes.len() - pos).min(buf.remaining());
            buf.put_slice(&bytes[pos..pos + n]);
            self.prefix.set_position((pos + n) as u64);
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for Prefixed {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

fn err(e: impl ToString) -> ProviderError {
    ProviderError::Other(e.to_string())
}

fn info_of(local: Option<SocketAddr>, remote: Option<SocketAddr>) -> SocketInfo {
    SocketInfo {
        remote_address: remote.map(|a| a.ip().to_string()).unwrap_or_default(),
        remote_port: remote.map(|a| a.port()).unwrap_or(0),
        local_address: local.map(|a| a.ip().to_string()).unwrap_or_default(),
        local_port: local.map(|a| a.port()).unwrap_or(0),
        alpn: None,
    }
}

impl NetProvider for SystemNet {
    fn connect(
        &self,
        host: String,
        port: u16,
        opts: ConnectOptions,
    ) -> BoxFuture<Result<(u64, SocketInfo), ProviderError>> {
        let this = self.clone();
        Box::pin(async move {
            let tcp = TcpStream::connect((host.as_str(), port))
                .await
                .map_err(err)?;
            let _ = tcp.set_nodelay(true);
            // Addresses come off the raw TCP stream before the TLS handshake
            // consumes it.
            let mut info = info_of(tcp.local_addr().ok(), tcp.peer_addr().ok());
            let id = this.id();
            let slot = if opts.secure {
                // SNI defaults to the connect host (WinterTC: `sni` overrides it).
                let name = opts.sni.unwrap_or_else(|| host.clone());
                let server_name =
                    ServerName::try_from(name).map_err(|_| err("invalid TLS server name"))?;
                let tls = this
                    .tls_connector(&opts.alpn)?
                    .connect(server_name, tcp)
                    .await
                    .map_err(err)?;
                info.alpn = tls
                    .get_ref()
                    .1
                    .alpn_protocol()
                    .map(|p| String::from_utf8_lossy(p).into_owned());
                SystemNet::spawn_socket(tls)
            } else {
                // Plaintext (`"off"` or `"starttls"`): keep the stream
                // reclaimable so a later startTls can upgrade it in place.
                SystemNet::spawn_upgradable(tcp)
            };
            this.sockets.lock().unwrap().insert(id, slot);
            Ok((id, info))
        })
    }

    fn read(&self, id: u64) -> BoxFuture<Result<Option<Vec<u8>>, ProviderError>> {
        let sockets = self.sockets.clone();
        Box::pin(async move {
            let mut rx = match sockets
                .lock()
                .unwrap()
                .get_mut(&id)
                .and_then(|s| s.read_rx.take())
            {
                Some(rx) => rx,
                None => return Ok(None), // closed or already at EOF
            };
            match rx.recv().await {
                Some(Ok(buf)) => {
                    if let Some(slot) = sockets.lock().unwrap().get_mut(&id) {
                        slot.read_rx = Some(rx);
                    }
                    Ok(Some(buf))
                }
                Some(Err(e)) => Err(err(e)),
                None => Ok(None), // reader task ended (EOF) — leave it taken
            }
        })
    }

    fn write(&self, id: u64, data: Vec<u8>) -> BoxFuture<Result<(), ProviderError>> {
        let sockets = self.sockets.clone();
        Box::pin(async move {
            let tx = sockets
                .lock()
                .unwrap()
                .get(&id)
                .and_then(|s| s.write_tx.clone());
            match tx {
                Some(tx) => tx.send(data).await.map_err(|_| err("socket is closed")),
                None => Err(err("socket is closed")),
            }
        })
    }

    fn shutdown(&self, id: u64) -> BoxFuture<Result<(), ProviderError>> {
        let sockets = self.sockets.clone();
        Box::pin(async move {
            // Drop the sender: the writer task's recv() ends and it shuts down
            // the write half (FIN). The read half keeps working.
            if let Some(slot) = sockets.lock().unwrap().get_mut(&id) {
                slot.write_tx = None;
            }
            Ok(())
        })
    }

    fn close(&self, id: u64) -> BoxFuture<Result<(), ProviderError>> {
        let sockets = self.sockets.clone();
        Box::pin(async move {
            // Dropping the slot drops both channel ends, ending both tasks.
            sockets.lock().unwrap().remove(&id);
            Ok(())
        })
    }

    fn listen(
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
            let (tx, rx) = mpsc::channel::<(TcpStream, SocketAddr)>(8);
            tokio::spawn(async move {
                while let Ok(conn) = listener.accept().await {
                    if tx.send(conn).await.is_err() {
                        break; // listener closed (rx dropped)
                    }
                }
            });
            let id = this.id();
            this.listeners.lock().unwrap().insert(id, rx);
            Ok((id, info_of(local, None)))
        })
    }

    fn accept(&self, id: u64) -> BoxFuture<Result<Option<(u64, SocketInfo)>, ProviderError>> {
        let this = self.clone();
        Box::pin(async move {
            let mut rx = match this.listeners.lock().unwrap().remove(&id) {
                Some(rx) => rx,
                None => return Ok(None), // listener closed
            };
            let conn = rx.recv().await;
            this.listeners.lock().unwrap().insert(id, rx); // keep accepting
            match conn {
                Some((stream, remote)) => {
                    let _ = stream.set_nodelay(true);
                    let info = info_of(stream.local_addr().ok(), Some(remote));
                    let sid = this.id();
                    this.sockets
                        .lock()
                        .unwrap()
                        .insert(sid, SystemNet::spawn_socket(stream));
                    Ok(Some((sid, info)))
                }
                None => Ok(None),
            }
        })
    }

    fn close_listener(&self, id: u64) -> BoxFuture<Result<(), ProviderError>> {
        let listeners = self.listeners.clone();
        Box::pin(async move {
            listeners.lock().unwrap().remove(&id);
            Ok(())
        })
    }

    fn start_tls(
        &self,
        id: u64,
        server_name: String,
        alpn: Vec<String>,
    ) -> BoxFuture<Result<(u64, SocketInfo), ProviderError>> {
        let this = self.clone();
        Box::pin(async move {
            // Take the reclaim handles and drain anything the reader already
            // buffered but the guest never read, so the upgrade loses nothing the
            // peer sent before the handshake.
            let (reclaim, prefix) = {
                let mut socks = this.sockets.lock().unwrap();
                let slot = socks.get_mut(&id).ok_or_else(|| err("socket is closed"))?;
                let reclaim = slot
                    .reclaim
                    .take()
                    .ok_or_else(|| err("socket cannot be upgraded to TLS"))?;
                let mut prefix = Vec::new();
                if let Some(rx) = slot.read_rx.as_mut() {
                    while let Ok(Ok(mut chunk)) = rx.try_recv() {
                        prefix.append(&mut chunk);
                    }
                }
                (reclaim, prefix)
            };

            // Stop both tasks and rejoin the raw stream from their halves.
            let (rtx, rrx) = oneshot::channel();
            reclaim.read.send(rtx).map_err(|_| err("socket is closed"))?;
            let read_half = rrx.await.map_err(|_| err("socket is closed"))?;
            let (wtx, wrx) = oneshot::channel();
            reclaim
                .write
                .send(wtx)
                .map_err(|_| err("socket is closed"))?;
            let write_half = wrx.await.map_err(|_| err("socket is closed"))?;
            let tcp = read_half.unsplit(write_half);
            let (local, remote) = (tcp.local_addr().ok(), tcp.peer_addr().ok());

            // Wrap reader/writer tasks back over the TLS stream under a fresh id;
            // the old id is consumed (WinterTC returns a new Socket).
            let stream = Prefixed {
                prefix: io::Cursor::new(prefix),
                inner: tcp,
            };
            let name = ServerName::try_from(server_name)
                .map_err(|_| err("invalid TLS server name"))?;
            let tls = this
                .tls_connector(&alpn)?
                .connect(name, stream)
                .await
                .map_err(err)?;
            let mut info = info_of(local, remote);
            info.alpn = tls
                .get_ref()
                .1
                .alpn_protocol()
                .map(|p| String::from_utf8_lossy(p).into_owned());

            let new_id = this.id();
            let mut socks = this.sockets.lock().unwrap();
            socks.remove(&id);
            socks.insert(new_id, SystemNet::spawn_socket(tls));
            Ok((new_id, info))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_rustls::TlsAcceptor;
    use tokio_rustls::rustls::ServerConfig;
    use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

    /// A throwaway self-signed cert for `localhost`: (cert DER, PKCS#8 key DER).
    fn self_signed() -> (CertificateDer<'static>, PrivateKeyDer<'static>) {
        let ck = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
        let cert = ck.cert.der().clone();
        let key = PrivateKeyDer::from(PrivatePkcs8KeyDer::from(ck.signing_key.serialize_der()));
        (cert, key)
    }

    /// A `SystemNet` trusting only `cert` (so the self-signed server verifies).
    fn net_trusting(cert: CertificateDer<'static>) -> SystemNet {
        let mut roots = RootCertStore::empty();
        roots.add(cert).unwrap();
        SystemNet::with_tls_roots(Arc::new(roots))
    }

    // A real TLS handshake over loopback: SNI + ALPN negotiation and an
    // encrypted write/read round-trip, all against a hermetic self-signed server.
    #[tokio::test]
    async fn tls_connect_negotiates_alpn_and_roundtrips() {
        let (cert, key) = self_signed();

        let mut server_cfg =
            ServerConfig::builder_with_provider(Arc::new(aws_lc_rs::default_provider()))
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_no_client_auth()
                .with_single_cert(vec![cert.clone()], key)
                .unwrap();
        server_cfg.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        let acceptor = TlsAcceptor::from(Arc::new(server_cfg));

        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();

        // One connection: accept, read a chunk, echo it back uppercased.
        let server = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut tls = acceptor.accept(tcp).await.unwrap();
            let mut buf = [0u8; 32];
            let n = tls.read(&mut buf).await.unwrap();
            let up: Vec<u8> = buf[..n].iter().map(u8::to_ascii_uppercase).collect();
            tls.write_all(&up).await.unwrap();
            tls.flush().await.unwrap();
        });

        let net = net_trusting(cert);
        let opts = ConnectOptions {
            secure: true,
            sni: Some("localhost".to_string()),
            alpn: vec!["h2".to_string(), "http/1.1".to_string()],
        };
        let (id, info) = net
            .connect("localhost".to_string(), port, opts)
            .await
            .unwrap();
        // Both sides offer h2 first, so it must be the negotiated protocol.
        assert_eq!(info.alpn.as_deref(), Some("h2"));

        net.write(id, b"ping".to_vec()).await.unwrap();
        let echoed = net.read(id).await.unwrap().expect("an echoed chunk");
        assert_eq!(echoed, b"PING");

        net.close(id).await.unwrap();
        server.await.unwrap();
    }

    // A secure connect to a server that never speaks TLS must fail the handshake,
    // not hang or silently downgrade.
    #[tokio::test]
    async fn tls_connect_rejects_a_plaintext_server() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            if let Ok((mut tcp, _)) = listener.accept().await {
                let mut b = [0u8; 8];
                let _ = tcp.read(&mut b).await; // read the ClientHello, then drop
            }
        });

        let (cert, _key) = self_signed();
        let net = net_trusting(cert);
        let opts = ConnectOptions {
            secure: true,
            sni: Some("localhost".to_string()),
            ..Default::default()
        };
        let res = net.connect("localhost".to_string(), port, opts).await;
        assert!(res.is_err(), "TLS to a plaintext server must error");
    }

    // A STARTTLS upgrade: connect plaintext, exchange a line in the clear, then
    // upgrade the *same* connection to TLS and round-trip over the encrypted
    // stream — the SMTP/IMAP/XMPP "STARTTLS" shape.
    #[tokio::test]
    async fn starttls_upgrades_a_live_plaintext_socket() {
        let (cert, key) = self_signed();

        let mut server_cfg =
            ServerConfig::builder_with_provider(Arc::new(aws_lc_rs::default_provider()))
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_no_client_auth()
                .with_single_cert(vec![cert.clone()], key)
                .unwrap();
        server_cfg.alpn_protocols = vec![b"h2".to_vec()];
        let acceptor = TlsAcceptor::from(Arc::new(server_cfg));

        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let server = tokio::spawn(async move {
            let (mut tcp, _) = listener.accept().await.unwrap();
            // Plaintext go-ahead, then upgrade the same socket to TLS.
            let mut buf = [0u8; 16];
            let n = tcp.read(&mut buf).await.unwrap();
            assert_eq!(&buf[..n], b"STARTTLS\n");
            tcp.write_all(b"OK\n").await.unwrap();
            tcp.flush().await.unwrap();
            let mut tls = acceptor.accept(tcp).await.unwrap();
            let mut b = [0u8; 32];
            let n = tls.read(&mut b).await.unwrap();
            let up: Vec<u8> = b[..n].iter().map(u8::to_ascii_uppercase).collect();
            tls.write_all(&up).await.unwrap();
            tls.flush().await.unwrap();
        });

        let net = net_trusting(cert);
        let (id, _) = net
            .connect("localhost".to_string(), port, ConnectOptions::default())
            .await
            .unwrap();
        net.write(id, b"STARTTLS\n".to_vec()).await.unwrap();
        assert_eq!(net.read(id).await.unwrap().unwrap(), b"OK\n");

        let (tls_id, info) = net
            .start_tls(id, "localhost".to_string(), vec!["h2".to_string()])
            .await
            .unwrap();
        assert_eq!(info.alpn.as_deref(), Some("h2"));

        net.write(tls_id, b"ping".to_vec()).await.unwrap();
        assert_eq!(net.read(tls_id).await.unwrap().unwrap(), b"PING");

        // The upgraded (already-TLS) socket cannot be upgraded again.
        assert!(
            net.start_tls(tls_id, "localhost".to_string(), vec![])
                .await
                .is_err(),
            "a TLS socket has no reclaimable raw stream"
        );

        net.close(tls_id).await.unwrap();
        server.await.unwrap();
    }

    // startTls on an id that was never opened (or already closed) errors rather
    // than panicking.
    #[tokio::test]
    async fn start_tls_on_an_unknown_socket_errors() {
        let net = SystemNet::new();
        assert!(
            net.start_tls(999, "localhost".to_string(), vec![])
                .await
                .is_err()
        );
    }

    // Plaintext connect still works unchanged through the generic spawn path.
    #[tokio::test]
    async fn plaintext_connect_still_roundtrips() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (mut tcp, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 32];
            let n = tcp.read(&mut buf).await.unwrap();
            tcp.write_all(&buf[..n]).await.unwrap();
            tcp.flush().await.unwrap();
        });

        let net = SystemNet::new();
        let (id, info) = net
            .connect("127.0.0.1".to_string(), port, ConnectOptions::default())
            .await
            .unwrap();
        assert!(info.alpn.is_none());
        net.write(id, b"hi".to_vec()).await.unwrap();
        assert_eq!(net.read(id).await.unwrap().unwrap(), b"hi");
        net.close(id).await.unwrap();
        server.await.unwrap();
    }
}
