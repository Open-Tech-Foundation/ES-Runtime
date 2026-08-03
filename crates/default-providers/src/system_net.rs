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

use es_runtime_common::ErrorCode;
use es_runtime_providers::{
    BoxFuture, ConnectOptions, ListenOptions, NetProvider, ProviderError, SocketInfo,
};
use futures_util::stream::{FuturesUnordered, StreamExt};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::crypto::aws_lc_rs;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};

use crate::accept_backoff::AcceptBackoff;

type ReadRx = mpsc::Receiver<Result<Vec<u8>, String>>;
type WriteTx = mpsc::Sender<Vec<u8>>;
type AcceptRx = mpsc::Receiver<(Accepted, SocketAddr)>;
type TcpRead = tokio::io::ReadHalf<TcpStream>;
type TcpWrite = tokio::io::WriteHalf<TcpStream>;

/// One inbound connection ready for [`accept`](SystemNet::accept), carrying its
/// stream. A TLS listener finishes the server-side handshake in the accept task
/// (so a slow client can't head-of-line-block other connections) and forwards the
/// established `TlsStream`; a plaintext listener forwards the raw `TcpStream`.
enum Accepted {
    Plain(TcpStream),
    Tls(Box<tokio_rustls::server::TlsStream<TcpStream>>),
}

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
    /// The accept-forwarding task per listener, kept so `close_listener` can
    /// abort it. Aborting drops the task's channel sender, which makes any
    /// **parked** `accept` (whose `recv` would otherwise wait forever, since it
    /// holds the only `AcceptRx` out of the map) resolve to `None`.
    listener_tasks: Arc<Mutex<HashMap<u64, tokio::task::JoinHandle<()>>>>,
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
    /// Addresses `connect` may reach (`--allow-net=<hosts>`). `None` ⇒ any.
    allow_connect: Option<Arc<crate::HostAllowlist>>,
    /// Addresses `listen` may bind (`--allow-listen=<addresses>`). `None` ⇒ any.
    /// Separate from [`allow_connect`](Self::allow_connect) because reaching out
    /// and being reachable are separate capabilities (`Net` / `NetListen`), and
    /// the addresses that make sense for each have nothing to do with the other.
    allow_listen: Option<Arc<crate::HostAllowlist>>,
}

impl SystemNet {
    /// Builds an empty socket registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Restricts `connect` to `allow` — `esrun --allow-net=<hosts>` (D38). The
    /// host is judged **as written**, before resolution: a name is a name, so a
    /// denied name cannot be smuggled through by an attacker-controlled DNS
    /// answer, and an IP entry never silently admits a name that resolves to it.
    #[must_use]
    pub fn with_allowlist(mut self, allow: crate::HostAllowlist) -> Self {
        self.allow_connect = Some(Arc::new(allow));
        self
    }

    /// Restricts `listen` to `allow` — `esrun --allow-listen=<addresses>` (D38).
    /// The bind address must match an entry exactly; write a bare port to allow
    /// any interface.
    #[must_use]
    pub fn with_listen_allowlist(mut self, allow: crate::HostAllowlist) -> Self {
        self.allow_listen = Some(Arc::new(allow));
        self
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

/// Classifies an io error with a stable guest-facing code (SPEC §6 Phase 13).
/// Name-resolution failures surface as an uncategorized io error, so the
/// message is sniffed to give guests the DNS classification.
fn io_err(context: impl std::fmt::Display, e: io::Error) -> ProviderError {
    if e.to_string().contains("lookup") {
        return ProviderError::Coded {
            code: ErrorCode::Dns,
            message: format!("{context}: {e}"),
        };
    }
    ProviderError::from_io(context, &e)
}

/// A TLS handshake / certificate failure with the stable `ERR_TLS` code.
fn tls_err(e: impl ToString) -> ProviderError {
    ProviderError::Coded {
        code: ErrorCode::Tls,
        message: e.to_string(),
    }
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
            if let Some(allow) = &this.allow_connect {
                allow.check(&host, port, "connect")?;
            }
            let tcp = TcpStream::connect((host.as_str(), port))
                .await
                .map_err(|e| io_err(format!("connect {host}:{port}"), e))?;
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
                    .map_err(tls_err)?;
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
        opts: ListenOptions,
    ) -> BoxFuture<Result<(u64, SocketInfo), ProviderError>> {
        let this = self.clone();
        Box::pin(async move {
            // Before the acceptor is built and before the port is claimed: a
            // denied bind must leave nothing behind.
            if let Some(allow) = &this.allow_listen {
                allow.check(&host, port, "listen")?;
            }
            // Build the TLS acceptor (cert/key parse, config assembly) once, at
            // bind time, before any connection arrives.
            let acceptor = if opts.cert.is_empty() && opts.key.is_empty() {
                None
            } else {
                Some(crate::tls::server_acceptor(
                    &opts.cert, &opts.key, &opts.alpn,
                )?)
            };
            let listener = TcpListener::bind((host.as_str(), port))
                .await
                .map_err(|e| io_err(format!("listen {host}:{port}"), e))?;
            let local = listener.local_addr().ok();
            let (tx, rx) = mpsc::channel::<(Accepted, SocketAddr)>(8);
            // One task owns the sole `tx` so `close_listener` aborting it drops the
            // sender and resolves a parked accept to `None`. TLS handshakes run
            // concurrently inside it (a `FuturesUnordered`) rather than in spawned
            // tasks holding `tx` clones, so a stalled handshake neither blocks the
            // next accept nor keeps the channel alive past a close.
            let task = tokio::spawn(async move {
                let mut handshakes = FuturesUnordered::new();
                // Errors from `accept` are retried, never fatal — see
                // [`AcceptBackoff`](crate::accept_backoff). The wait is a
                // branch of the select rather than an inline sleep so that
                // handshakes already in flight keep advancing through it: a
                // `FuturesUnordered` only makes progress while it is polled.
                let mut backoff = AcceptBackoff::new();
                let mut retry_at: Option<Instant> = None;
                loop {
                    tokio::select! {
                        accepted = listener.accept(), if retry_at.is_none() => {
                            let (tcp, remote) = match accepted {
                                Ok(accepted) => {
                                    backoff.reset();
                                    accepted
                                }
                                Err(e) => {
                                    let delay = backoff.next_delay();
                                    tracing::warn!(
                                        target: "runtime::net",
                                        error = %e,
                                        backoff_ms = delay.as_millis() as u64,
                                        "accept failed; retrying",
                                    );
                                    retry_at = Some(Instant::now() + delay);
                                    continue;
                                }
                            };
                            let _ = tcp.set_nodelay(true);
                            match &acceptor {
                                None => {
                                    if tx.send((Accepted::Plain(tcp), remote)).await.is_err() {
                                        break; // listener closed (rx dropped)
                                    }
                                }
                                Some(acceptor) => {
                                    let acceptor = acceptor.clone();
                                    handshakes.push(async move {
                                        acceptor.accept(tcp).await.ok().map(|tls| (tls, remote))
                                    });
                                }
                            }
                        }
                        // `pending` when there is nothing to wait for: every
                        // branch expression is evaluated up front, so this one
                        // has to be safe to build with no retry outstanding.
                        () = async move {
                            match retry_at {
                                Some(at) => tokio::time::sleep_until(at).await,
                                None => std::future::pending().await,
                            }
                        } => {
                            retry_at = None;
                        }
                        Some(done) = handshakes.next(), if !handshakes.is_empty() => {
                            // A failed handshake yields `None` — drop it silently.
                            if let Some((tls, remote)) = done
                                && tx.send((Accepted::Tls(Box::new(tls)), remote)).await.is_err()
                            {
                                break; // listener closed (rx dropped)
                            }
                        }
                    }
                }
            });
            let id = this.id();
            this.listeners.lock().unwrap().insert(id, rx);
            this.listener_tasks.lock().unwrap().insert(id, task);
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
            match conn {
                Some((accepted, remote)) => {
                    this.listeners.lock().unwrap().insert(id, rx); // keep accepting
                    let sid = this.id();
                    // The stream is already TLS-terminated (if this is a TLS
                    // listener); just build the slot and surface the negotiated
                    // ALPN from the handshake.
                    let (slot, info) = match accepted {
                        Accepted::Plain(tcp) => {
                            let info = info_of(tcp.local_addr().ok(), Some(remote));
                            (SystemNet::spawn_socket(tcp), info)
                        }
                        Accepted::Tls(tls) => {
                            let (io, conn) = tls.get_ref();
                            let mut info = info_of(io.local_addr().ok(), Some(remote));
                            info.alpn = conn
                                .alpn_protocol()
                                .map(|p| String::from_utf8_lossy(p).into_owned());
                            (SystemNet::spawn_socket(*tls), info)
                        }
                    };
                    this.sockets.lock().unwrap().insert(sid, slot);
                    Ok(Some((sid, info)))
                }
                // Listener closed (sender dropped, e.g. close_listener aborted the
                // task): don't re-insert the dead rx; drop its task handle.
                None => {
                    this.listener_tasks.lock().unwrap().remove(&id);
                    Ok(None)
                }
            }
        })
    }

    fn close_listener(&self, id: u64) -> BoxFuture<Result<(), ProviderError>> {
        let listeners = self.listeners.clone();
        let listener_tasks = self.listener_tasks.clone();
        Box::pin(async move {
            listeners.lock().unwrap().remove(&id);
            // Abort the accept task so its sender drops, unblocking any parked
            // accept with `None` (the rx may be out of the map in a parked accept,
            // so removing it above isn't enough on its own).
            if let Some(task) = listener_tasks.lock().unwrap().remove(&id) {
                task.abort();
            }
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
            reclaim
                .read
                .send(rtx)
                .map_err(|_| err("socket is closed"))?;
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
            let name =
                ServerName::try_from(server_name).map_err(|_| err("invalid TLS server name"))?;
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
    use crate::HostAllowlist;
    // The production paths build their server-side TLS through `crate::tls`;
    // these tests stand up their own peer, so they need the types directly.
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

    // The `runtime:net` half of the invariant the HTTP acceptor has: the accept
    // loop keeps looping, rather than ending on the first error and leaving the
    // port bound but dead. The errno is not provokable in-process (the retry
    // policy is unit-tested in `accept_backoff`), so what is asserted is that a
    // burst of connections abandoned on arrival leaves the listener able to
    // hand over the next one.
    #[tokio::test]
    async fn the_accept_loop_keeps_working_after_a_burst_of_abandoned_connections() {
        let net = SystemNet::new();
        let (lid, addr) = net
            .listen("127.0.0.1".to_string(), 0, ListenOptions::default())
            .await
            .unwrap();
        let port = addr.local_port;

        for _ in 0..64 {
            drop(
                tokio::net::TcpStream::connect(("127.0.0.1", port))
                    .await
                    .unwrap(),
            );
        }

        let client = net.clone();
        let connect = tokio::spawn(async move {
            client
                .connect("127.0.0.1".to_string(), port, ConnectOptions::default())
                .await
        });
        let accepted = tokio::time::timeout(std::time::Duration::from_secs(10), net.accept(lid))
            .await
            .expect("the accept loop is still accepting")
            .unwrap();
        assert!(accepted.is_some(), "the connection after the burst arrives");

        let (cid, _) = connect.await.unwrap().unwrap();
        net.close(cid).await.unwrap();
        net.close_listener(lid).await.unwrap();
    }

    // close_listener must unblock an already-parked accept (resolve it to None),
    // not leave it waiting forever — the bug behind a `for await (conn of server)`
    // loop that never ends when closed from another context.
    #[tokio::test]
    async fn close_listener_unblocks_a_parked_accept() {
        let net = SystemNet::new();
        let (lid, _) = net
            .listen("127.0.0.1".to_string(), 0, ListenOptions::default())
            .await
            .unwrap();

        let probe = net.clone();
        let accept = tokio::spawn(async move { probe.accept(lid).await });

        // Let the accept park on recv (no incoming connection), then close it.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        net.close_listener(lid).await.unwrap();

        let res = tokio::time::timeout(std::time::Duration::from_secs(5), accept)
            .await
            .expect("a closed listener must not hang accept")
            .unwrap()
            .unwrap();
        assert!(res.is_none(), "a closed listener's accept yields None");
    }

    // Server-side TLS termination on `listen`: a TLS listener built from a PEM
    // cert/key accepts an encrypted client, negotiates ALPN both ways, and
    // round-trips over the terminated stream — the full server path end to end,
    // against a hermetic self-signed cert.
    #[tokio::test]
    async fn listen_terminates_tls_and_negotiates_alpn() {
        let ck = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
        let cert_der = ck.cert.der().clone();
        let cert_pem = ck.cert.pem().into_bytes();
        let key_pem = ck.signing_key.serialize_pem().into_bytes();

        // One SystemNet does both roles: serve with the PEM cert/key, and connect
        // as a client trusting that self-signed cert.
        let net = net_trusting(cert_der);
        let (lid, addr) = net
            .listen(
                "127.0.0.1".to_string(),
                0,
                ListenOptions {
                    cert: cert_pem,
                    key: key_pem,
                    alpn: vec!["h2".to_string(), "http/1.1".to_string()],
                },
            )
            .await
            .unwrap();
        let port = addr.local_port;

        // Server: accept the (already TLS-terminated) connection, echo uppercased.
        let server_net = net.clone();
        let server = tokio::spawn(async move {
            let (sid, info) = server_net.accept(lid).await.unwrap().expect("a connection");
            // Both sides offer h2 first, so it is the negotiated protocol.
            assert_eq!(info.alpn.as_deref(), Some("h2"));
            let buf = server_net.read(sid).await.unwrap().expect("a chunk");
            let up: Vec<u8> = buf.iter().map(u8::to_ascii_uppercase).collect();
            server_net.write(sid, up).await.unwrap();
        });

        let opts = ConnectOptions {
            secure: true,
            sni: Some("localhost".to_string()),
            alpn: vec!["h2".to_string(), "http/1.1".to_string()],
        };
        let (cid, cinfo) = net
            .connect("localhost".to_string(), port, opts)
            .await
            .unwrap();
        assert_eq!(cinfo.alpn.as_deref(), Some("h2"));
        net.write(cid, b"ping".to_vec()).await.unwrap();
        assert_eq!(net.read(cid).await.unwrap().unwrap(), b"PING");

        net.close(cid).await.unwrap();
        net.close_listener(lid).await.unwrap();
        server.await.unwrap();
    }

    // A TLS listener built from unparseable PEM fails at bind time (in `listen`),
    // not later per connection.
    #[tokio::test]
    async fn listen_with_invalid_cert_errors_at_bind() {
        let net = SystemNet::new();
        let res = net
            .listen(
                "127.0.0.1".to_string(),
                0,
                ListenOptions {
                    cert: b"-----BEGIN CERTIFICATE-----\nnonsense\n-----END CERTIFICATE-----\n"
                        .to_vec(),
                    key: b"-----BEGIN PRIVATE KEY-----\nnonsense\n-----END PRIVATE KEY-----\n"
                        .to_vec(),
                    alpn: vec![],
                },
            )
            .await;
        assert!(
            res.is_err(),
            "a TLS listener with a bad cert must fail to bind"
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

    // ---- the address allowlist (D38) ----------------------------------------

    #[tokio::test]
    async fn connect_refuses_an_address_outside_the_allowlist() {
        // Nothing is listening on the refused address and nothing needs to be:
        // the check runs before the socket, so a denial costs no DNS lookup and
        // sends no packet.
        let net =
            SystemNet::new().with_allowlist(HostAllowlist::parse(["db.internal:5432"]).unwrap());
        let err = net
            .connect("evil.test".to_string(), 5432, ConnectOptions::default())
            .await
            .err()
            .expect("an address outside the list must be refused");
        assert_eq!(
            err.code(),
            Some(es_runtime_common::ErrorCode::PermissionDenied)
        );
        assert!(err.to_string().contains("evil.test:5432"), "{err}");
    }

    #[tokio::test]
    async fn connect_permits_an_address_on_the_allowlist() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let _ = listener.accept().await;
        });
        let net = SystemNet::new()
            .with_allowlist(HostAllowlist::parse([format!("127.0.0.1:{port}")]).unwrap());
        let connected = net
            .connect("127.0.0.1".to_string(), port, ConnectOptions::default())
            .await;
        assert!(connected.is_ok(), "{:?}", connected.err());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn listen_refuses_a_bind_outside_the_allowlist() {
        // The port must not be claimed by a refused bind: the check comes before
        // the acceptor and before `TcpListener::bind`.
        // The list names the interface, not a port, so the allowed half binds
        // port 0 and never races another test for a number.
        let net =
            SystemNet::new().with_listen_allowlist(HostAllowlist::parse(["127.0.0.1"]).unwrap());
        let port = 0;
        let err = net
            .listen("0.0.0.0".to_string(), port, ListenOptions::default())
            .await
            .err()
            .expect("a bind outside the list must be refused");
        assert!(
            err.to_string().contains(&format!("0.0.0.0:{port}")),
            "{err}"
        );
        // Binding is not the same grant as reaching out: an allowlist for one
        // says nothing about the other.
        let (id, _) = net
            .listen("127.0.0.1".to_string(), port, ListenOptions::default())
            .await
            .expect("the allowed address binds");
        net.close_listener(id).await.unwrap();
    }
}
