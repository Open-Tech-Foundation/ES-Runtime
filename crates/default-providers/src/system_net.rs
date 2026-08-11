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
//!
//! **UDP is the other shape and takes the other approach** (DECISIONS D58):
//! datagram sockets are held as an `Arc<UdpSocket>` and each `receive` awaits
//! `recv_from` directly, with no forwarding task and no channel in between. A
//! stream needs a reader task because bytes arrive whether or not anyone is
//! asking; a datagram queue already exists in the kernel, and a second one in
//! front of it would only add a place for datagrams to be dropped that the
//! program cannot see.

use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::task::{Context, Poll};

use es_runtime_common::ErrorCode;
use es_runtime_providers::{
    BoxFuture, ConnectOptions, Datagram, DatagramOption, DatagramOptions, ListenOptions,
    MulticastMembership, NetProvider, OutgoingDatagram, ProviderError, SocketInfo,
};
use futures_util::stream::{FuturesUnordered, StreamExt};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::{Notify, mpsc, oneshot};
use tokio::time::Instant;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::crypto::aws_lc_rs;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};

use crate::accept_backoff::AcceptBackoff;
use crate::checkout::Checkout;

/// What a memoized TLS client connector is keyed by: the ALPN list offered and
/// the extra trust anchors supplied, since each produces a different config and
/// two connects agreeing on one but not the other must not share.
type ConnectorKey = (Vec<String>, Vec<u8>);

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

/// A bound UDP socket. The socket itself is shared rather than checked out:
/// `recv_from` and `send_to` both take `&self`, so several receives may be
/// outstanding at once — they race for the next datagram, which is what a
/// datagram socket does anyway.
///
/// `closed` is the half that makes `close_datagram` prompt: dropping the last
/// `Arc` would close the socket, but a **parked** receive holds one, so without
/// a way to wake it the close would be observed only when the next datagram
/// arrived — on a socket nobody is sending to any more, never.
struct DatagramSlot {
    socket: Arc<UdpSocket>,
    closed: Arc<Closed>,
    /// Receive buffers, reused across calls — see [`BufferPool`].
    buffers: Arc<BufferPool>,
}

/// The close signal for one datagram socket: a flag to read and a bell to ring.
/// Both, because a receive that checks only the flag can be closed a moment
/// later and park forever, and one that waits only on the bell misses a close
/// that already happened.
#[derive(Default)]
struct Closed {
    flag: AtomicBool,
    bell: Notify,
}

/// A [`NetProvider`] over real tokio TCP and UDP sockets. The `Arc`s are cloned
/// into each returned future so the futures stay `'static`.
#[derive(Clone, Default)]
pub struct SystemNet {
    sockets: Arc<Mutex<HashMap<u64, Slot>>>,
    listeners: Arc<Mutex<HashMap<u64, AcceptRx>>>,
    /// Bound UDP sockets, in their own namespace: a datagram socket answers
    /// none of the stream operations, and sharing a map with them would make a
    /// misdirected id a confusing error instead of a clean one.
    datagrams: Arc<Mutex<HashMap<u64, DatagramSlot>>>,
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
    tls_connectors: Arc<Mutex<HashMap<ConnectorKey, TlsConnector>>>,
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
    fn tls_connector(&self, alpn: &[String], ca: &[u8]) -> Result<TlsConnector, ProviderError> {
        let key: ConnectorKey = (alpn.to_vec(), ca.to_vec());
        if let Some(connector) = self.tls_connectors.lock().unwrap().get(&key) {
            return Ok(connector.clone());
        }
        let provider = Arc::new(aws_lc_rs::default_provider());
        let roots = if ca.is_empty() {
            self.tls_roots()
        } else {
            // Added to the defaults, never instead of them: a caller naming a
            // private authority is saying "trust this as well", and a build
            // that quietly stopped trusting the public roots would break every
            // other host the same program talks to.
            use tokio_rustls::rustls::pki_types::CertificateDer;
            use tokio_rustls::rustls::pki_types::pem::PemObject;
            let mut store = (*self.tls_roots()).clone();
            let mut added = 0usize;
            for cert in CertificateDer::pem_slice_iter(ca) {
                store.add(cert.map_err(err)?).map_err(err)?;
                added += 1;
            }
            if added == 0 {
                return Err(ProviderError::Coded {
                    code: ErrorCode::Tls,
                    message: "the supplied CA contains no PEM certificate".to_string(),
                });
            }
            Arc::new(store)
        };
        let mut config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(err)?
            .with_root_certificates(roots)
            .with_no_client_auth();
        config.alpn_protocols = alpn.iter().map(|p| p.as_bytes().to_vec()).collect();
        let connector = TlsConnector::from(Arc::new(config));
        self.tls_connectors
            .lock()
            .unwrap()
            .insert(key, connector.clone());
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
                    .tls_connector(&opts.alpn, &opts.ca)?
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
            // Guarded so the channel goes back however this call ends: an
            // abandoned read would otherwise take it, and every later read on a
            // live socket would report EOF (see `checkout`).
            let mut rx = match sockets
                .lock()
                .unwrap()
                .get_mut(&id)
                .and_then(|s| s.read_rx.take())
            {
                Some(rx) => {
                    let back = sockets.clone();
                    Checkout::new(rx, move |rx| {
                        if let Some(slot) = back.lock().unwrap().get_mut(&id) {
                            slot.read_rx = Some(rx);
                        }
                    })
                }
                None => return Ok(None), // closed or already at EOF
            };
            match rx.get_mut().recv().await {
                Some(Ok(buf)) => Ok(Some(buf)),
                Some(Err(e)) => {
                    rx.keep_out(); // the socket errored; nothing to read again
                    Err(err(e))
                }
                None => {
                    rx.keep_out(); // reader task ended (EOF)
                    Ok(None)
                }
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
            // Shared with `runtime:http`, so the two bind on identical terms —
            // including `SO_REUSEPORT` when it was asked for.
            let listener = crate::listener::bind(host.as_str(), port, opts.reuse_port).await?;
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
                                        match acceptor.accept(tcp).await {
                                            Ok(tls) => Some((tls, remote)),
                                            // Debug, not warn: on a public port a
                                            // failed handshake is a scanner or a
                                            // client with no shared cipher, and
                                            // one line per attempt would hand any
                                            // peer a log-flooding lever. But it
                                            // is also the only signal a
                                            // misconfigured chain ever gives —
                                            // without it a server that no client
                                            // can complete a handshake with looks
                                            // exactly like a server nobody is
                                            // calling.
                                            Err(e) => {
                                                tracing::debug!(
                                                    target: "runtime::net",
                                                    peer = %remote,
                                                    error = %e,
                                                    "tls handshake failed",
                                                );
                                                None
                                            }
                                        }
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
                            // A failed handshake yields `None`; it has already
                            // logged for itself above, and ends this connection
                            // only — never the acceptor.
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
            // Guarded: an abandoned accept would take the listener's channel
            // with it and every later accept would report the listener closed.
            let mut rx = match this.listeners.lock().unwrap().remove(&id) {
                Some(rx) => {
                    let back = this.listeners.clone();
                    Checkout::new(rx, move |rx| {
                        back.lock().unwrap().insert(id, rx);
                    })
                }
                None => return Ok(None), // listener closed
            };
            let conn = rx.get_mut().recv().await;
            match conn {
                Some((accepted, remote)) => {
                    drop(rx); // back to the registry — keep accepting
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
                    rx.keep_out();
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
        ca: Vec<u8>,
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
                .tls_connector(&alpn, &ca)?
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

    fn bind_datagram(
        &self,
        host: String,
        port: u16,
        opts: DatagramOptions,
    ) -> BoxFuture<Result<(u64, SocketInfo), ProviderError>> {
        let this = self.clone();
        Box::pin(async move {
            // The same list `listen` consults, and for the same reason: this
            // takes a port and makes the process reachable on it. Checked before
            // the socket exists, so a denied bind claims nothing.
            if let Some(allow) = &this.allow_listen {
                allow.check(&host, port, "bind")?;
            }
            let socket = crate::datagram::bind(&host, port, &opts).await?;
            let info = info_of(socket.local_addr().ok(), None);
            let id = this.id();
            this.datagrams.lock().unwrap().insert(
                id,
                DatagramSlot {
                    socket: Arc::new(socket),
                    closed: Arc::new(Closed::default()),
                    buffers: Arc::new(BufferPool::default()),
                },
            );
            Ok((id, info))
        })
    }

    fn receive(&self, id: u64) -> BoxFuture<Result<Option<Datagram>, ProviderError>> {
        let datagrams = self.datagrams.clone();
        Box::pin(async move {
            let Some((socket, closed, pool)) = lookup(&datagrams, id) else {
                return Ok(None); // closed
            };
            // Registered *before* the flag is read, so a close landing between
            // the two is seen by one or the other and never by neither.
            let bell = closed.bell.notified();
            tokio::pin!(bell);
            bell.as_mut().enable();
            if closed.flag.load(Ordering::Acquire) {
                return Ok(None);
            }
            // Borrowed from the socket's pool rather than allocated: a 64 KiB
            // buffer per datagram is most of the cost of receiving a small one.
            // Returned however this call ends (`Scratch` gives it back in its
            // destructor), so an abandoned receive does not drain the pool.
            let mut scratch = pool.take();
            let (n, from) = tokio::select! {
                biased;
                () = &mut bell => return Ok(None),
                received = socket.recv_from(scratch.buf()) => {
                    received.map_err(|e| io_err("receive", e))?
                }
            };
            Ok(Some(datagram_from(scratch.buf(), n, from)))
        })
    }

    fn receive_many(
        &self,
        id: u64,
        max: usize,
    ) -> BoxFuture<Result<Option<Vec<Datagram>>, ProviderError>> {
        let datagrams = self.datagrams.clone();
        Box::pin(async move {
            let Some((socket, closed, pool)) = lookup(&datagrams, id) else {
                return Ok(None);
            };
            let bell = closed.bell.notified();
            tokio::pin!(bell);
            bell.as_mut().enable();
            if closed.flag.load(Ordering::Acquire) {
                return Ok(None);
            }
            let mut scratch = pool.take();
            let mut batch = Vec::new();
            let (n, from) = tokio::select! {
                biased;
                () = &mut bell => return Ok(None),
                received = socket.recv_from(scratch.buf()) => {
                    received.map_err(|e| io_err("receive", e))?
                }
            };
            batch.push(datagram_from(scratch.buf(), n, from));
            // Then whatever is *already* queued, and not one datagram more: a
            // batch that waits to fill trades the first datagram's latency for
            // throughput nobody asked for.
            while batch.len() < max.max(1) {
                match socket.try_recv_from(scratch.buf()) {
                    Ok((n, from)) => batch.push(datagram_from(scratch.buf(), n, from)),
                    // Empty queue, or a datagram that arrived and errored. The
                    // batch already in hand is worth more than the error on a
                    // datagram nobody has seen, so it is returned and the error
                    // surfaces on the next call.
                    Err(_) => break,
                }
            }
            Ok(Some(batch))
        })
    }

    fn send_to(
        &self,
        id: u64,
        data: Vec<u8>,
        to: Option<(String, u16)>,
    ) -> BoxFuture<Result<usize, ProviderError>> {
        let this = self.clone();
        Box::pin(async move {
            let (socket, ..) = lookup(&this.datagrams, id).ok_or_else(closed_socket)?;
            let Some((host, port)) = to else {
                // No destination: the connected peer, which `connect_datagram`
                // already checked against the allowlist.
                return socket.send(&data).await.map_err(|e| io_err("send", e));
            };
            // Every destination is checked, not just the first: one socket sends
            // to as many peers as it likes, so a per-socket check would scope
            // nothing after the bind.
            if let Some(allow) = &this.allow_connect {
                allow.check(&host, port, "send")?;
            }
            socket
                .send_to(&data, (host.as_str(), port))
                .await
                .map_err(|e| io_err(format!("send to {host}:{port}"), e))
        })
    }

    fn send_many(
        &self,
        id: u64,
        messages: Vec<OutgoingDatagram>,
    ) -> BoxFuture<Result<usize, ProviderError>> {
        let this = self.clone();
        Box::pin(async move {
            let (socket, ..) = lookup(&this.datagrams, id).ok_or_else(closed_socket)?;
            let mut sent = 0usize;
            for (data, to) in messages {
                let result = match &to {
                    None => socket.send(&data).await.map_err(|e| io_err("send", e)),
                    Some((host, port)) => {
                        // Per destination, exactly as `send_to` does it — a batch
                        // is a saved crossing, never a saved check.
                        //
                        // Collected rather than `?`-propagated: a denial is a
                        // failure *of this message*, and it has to carry the
                        // count of the ones that already went, like any other.
                        let denied = this
                            .allow_connect
                            .as_ref()
                            .and_then(|allow| allow.check(host, *port, "send").err());
                        match denied {
                            Some(e) => Err(e),
                            None => socket
                                .send_to(&data, (host.as_str(), *port))
                                .await
                                .map_err(|e| io_err(format!("send to {host}:{port}"), e)),
                        }
                    }
                };
                match result {
                    Ok(_) => sent += 1,
                    // The count so far travels with the error: "none of them"
                    // and "the first three of five" are different facts, and a
                    // caller that retries needs to know which.
                    Err(e) => {
                        return Err(match e {
                            ProviderError::Coded { code, message } => ProviderError::Coded {
                                code,
                                message: format!("{message} (after {sent} of the batch)"),
                            },
                            other => {
                                ProviderError::Other(format!("{other} (after {sent} of the batch)"))
                            }
                        });
                    }
                }
            }
            Ok(sent)
        })
    }

    fn set_datagram_option(
        &self,
        id: u64,
        option: DatagramOption,
    ) -> BoxFuture<Result<(), ProviderError>> {
        let datagrams = self.datagrams.clone();
        Box::pin(async move {
            let (socket, ..) = lookup(&datagrams, id).ok_or_else(closed_socket)?;
            crate::datagram::set_option(&socket, option)
        })
    }

    fn connect_datagram(
        &self,
        id: u64,
        host: String,
        port: u16,
    ) -> BoxFuture<Result<SocketInfo, ProviderError>> {
        let this = self.clone();
        Box::pin(async move {
            let (socket, ..) = lookup(&this.datagrams, id).ok_or_else(closed_socket)?;
            if let Some(allow) = &this.allow_connect {
                allow.check(&host, port, "connect")?;
            }
            socket
                .connect((host.as_str(), port))
                .await
                .map_err(|e| io_err(format!("connect udp {host}:{port}"), e))?;
            Ok(info_of(socket.local_addr().ok(), socket.peer_addr().ok()))
        })
    }

    fn set_multicast_membership(
        &self,
        id: u64,
        membership: MulticastMembership,
        join: bool,
    ) -> BoxFuture<Result<(), ProviderError>> {
        let datagrams = self.datagrams.clone();
        Box::pin(async move {
            let (socket, ..) = lookup(&datagrams, id).ok_or_else(closed_socket)?;
            crate::datagram::set_membership(&socket, &membership, join)
        })
    }

    fn close_datagram(&self, id: u64) -> BoxFuture<Result<(), ProviderError>> {
        let datagrams = self.datagrams.clone();
        Box::pin(async move {
            // Dropping the slot drops this map's `Arc`; a parked receive holds
            // another, which the bell is what ends.
            if let Some(slot) = datagrams.lock().unwrap().remove(&id) {
                slot.closed.flag.store(true, Ordering::Release);
                slot.closed.bell.notify_waiters();
            }
            Ok(())
        })
    }
}

/// The largest payload a UDP datagram can carry over IPv4 (65,535 − 20 − 8).
const MAX_DATAGRAM: usize = 65_507;

/// The receive buffer, **one byte past** the largest datagram IPv4 can deliver.
///
/// That extra byte is the whole truncation test: a datagram that fills the
/// buffer exactly is one the buffer could not hold, because no IPv4 datagram is
/// this long. Without it, a full buffer and a perfectly-sized datagram are the
/// same observation and the guest is told nothing.
const RECV_BUFFER: usize = MAX_DATAGRAM + 1;

/// How many receive buffers one socket keeps for reuse. Concurrent receives may
/// take more; the surplus is dropped rather than pooled, so a burst of parallel
/// receives cannot leave the socket holding megabytes it will never need again.
const POOL_LIMIT: usize = 4;

/// Receive buffers for one datagram socket.
///
/// A 64 KiB allocation per datagram is most of the cost of receiving a small
/// one, and a datagram socket receives small ones by the thousand. The buffer is
/// borrowed for the length of one receive and given back afterwards; the
/// datagram that leaves is a fresh, exactly-sized copy, so nothing here is
/// retained past the call that made it.
#[derive(Default)]
struct BufferPool {
    free: Mutex<Vec<Vec<u8>>>,
}

impl BufferPool {
    fn take(self: &Arc<Self>) -> Scratch {
        let buf = self
            .free
            .lock()
            .unwrap()
            .pop()
            .unwrap_or_else(|| vec![0u8; RECV_BUFFER]);
        Scratch {
            buf: Some(buf),
            pool: self.clone(),
        }
    }
}

/// A borrowed receive buffer, returned to its pool on drop — including when the
/// receive holding it is abandoned mid-await, which is the case a `return` in
/// the happy path would have missed.
struct Scratch {
    buf: Option<Vec<u8>>,
    pool: Arc<BufferPool>,
}

impl Scratch {
    fn buf(&mut self) -> &mut [u8] {
        self.buf
            .as_mut()
            .expect("scratch buffer is live until drop")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if let Some(buf) = self.buf.take() {
            let mut free = self.pool.free.lock().unwrap();
            if free.len() < POOL_LIMIT {
                free.push(buf);
            }
        }
    }
}

/// Copies `n` bytes out of a receive buffer into the datagram that leaves.
fn datagram_from(buf: &[u8], n: usize, from: SocketAddr) -> Datagram {
    Datagram {
        data: buf[..n].to_vec(),
        address: from.ip().to_string(),
        port: from.port(),
        // See [`RECV_BUFFER`]: a datagram this long did not fit.
        truncated: n == RECV_BUFFER,
    }
}

/// The socket, close signal and buffer pool for datagram `id`, or `None` if it
/// is closed. The lock is released before the caller awaits anything.
fn lookup(
    datagrams: &Mutex<HashMap<u64, DatagramSlot>>,
    id: u64,
) -> Option<(Arc<UdpSocket>, Arc<Closed>, Arc<BufferPool>)> {
    let slots = datagrams.lock().unwrap();
    let slot = slots.get(&id)?;
    Some((
        slot.socket.clone(),
        slot.closed.clone(),
        slot.buffers.clone(),
    ))
}

fn closed_socket() -> ProviderError {
    err("socket is closed")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HostAllowlist;
    use tokio::net::TcpListener;
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
    pub(super) fn net_trusting(cert: CertificateDer<'static>) -> SystemNet {
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
            ca: Vec::new(),
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

    /// A private certificate authority is *added* to the defaults, not swapped
    /// for them — and without naming it the same server is still refused, which
    /// is the half that proves the option grants trust rather than removing the
    /// check.
    #[tokio::test]
    async fn a_named_ca_is_trusted_and_an_unnamed_one_is_not() {
        let ck = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
        let cert = ck.cert.der().clone();
        let pem = ck.cert.pem();
        let key = PrivateKeyDer::from(PrivatePkcs8KeyDer::from(ck.signing_key.serialize_der()));

        let server_cfg =
            ServerConfig::builder_with_provider(Arc::new(aws_lc_rs::default_provider()))
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_no_client_auth()
                .with_single_cert(vec![cert], key)
                .unwrap();
        let acceptor = TlsAcceptor::from(Arc::new(server_cfg));
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        // Two connections: one that should complete, one that should not.
        tokio::spawn(async move {
            for _ in 0..2 {
                let Ok((tcp, _)) = listener.accept().await else {
                    return;
                };
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    let _ = acceptor.accept(tcp).await;
                });
            }
        });

        // The real default roots, not a test store — this is the deployment
        // shape: the public authorities, plus one private one.
        let net = SystemNet::new();
        let with_ca = ConnectOptions {
            secure: true,
            sni: Some("localhost".to_string()),
            alpn: Vec::new(),
            ca: pem.into_bytes(),
        };
        let (id, _info) = net
            .connect("localhost".to_string(), port, with_ca)
            .await
            .expect("a server signed by the named authority must be trusted");
        net.close(id).await.unwrap();

        let without_ca = ConnectOptions {
            secure: true,
            sni: Some("localhost".to_string()),
            alpn: Vec::new(),
            ca: Vec::new(),
        };
        assert!(
            net.connect("localhost".to_string(), port, without_ca)
                .await
                .is_err(),
            "the same server must be refused when its authority is not named"
        );
    }

    /// A CA argument that parses to nothing is a mistake worth naming: silently
    /// falling back to the default roots would make a typo look like it worked,
    /// against a server the caller never meant to trust.
    ///
    /// The listener is real because the CA is parsed while the connector is
    /// built, which happens *after* the TCP connect — so a closed port would
    /// fail for the wrong reason and prove nothing.
    #[tokio::test]
    async fn a_ca_with_no_certificate_in_it_is_refused() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let _ = listener.accept().await;
        });

        let net = SystemNet::new();
        let result = net
            .connect(
                "localhost".to_string(),
                port,
                ConnectOptions {
                    secure: true,
                    sni: None,
                    alpn: Vec::new(),
                    ca: b"not a certificate".to_vec(),
                },
            )
            .await;
        assert!(
            matches!(&result, Err(e) if e.to_string().contains("no PEM certificate")),
            "expected the empty-CA refusal to name itself, got {:?}",
            result.as_ref().err().map(ToString::to_string)
        );
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
            .start_tls(
                id,
                "localhost".to_string(),
                vec!["h2".to_string()],
                Vec::new(),
            )
            .await
            .unwrap();
        assert_eq!(info.alpn.as_deref(), Some("h2"));

        net.write(tls_id, b"ping".to_vec()).await.unwrap();
        assert_eq!(net.read(tls_id).await.unwrap().unwrap(), b"PING");

        // The upgraded (already-TLS) socket cannot be upgraded again.
        assert!(
            net.start_tls(tls_id, "localhost".to_string(), vec![], Vec::new())
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
            net.start_tls(999, "localhost".to_string(), vec![], Vec::new())
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
                    reuse_port: false,
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
            ca: Vec::new(),
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
                    reuse_port: false,
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

/// Abandoning a call must not take the resource with it — see the same module
/// in `system_http` for why an embedder can do this at all.
#[cfg(test)]
mod cancel_safety_tests {
    use super::*;
    use std::time::Duration;
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn an_abandoned_read_leaves_the_socket_readable() {
        let net = SystemNet::new();
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (mut tcp, _) = listener.accept().await.unwrap();
            // Say nothing at first, so the guest's read parks and is abandoned.
            tokio::time::sleep(Duration::from_millis(300)).await;
            tcp.write_all(b"late").await.unwrap();
            tokio::time::sleep(Duration::from_secs(5)).await;
        });

        let (cid, _) = net
            .connect("127.0.0.1".to_string(), port, ConnectOptions::default())
            .await
            .unwrap();

        let abandoned = tokio::time::timeout(Duration::from_millis(50), net.read(cid)).await;
        assert!(abandoned.is_err(), "the read parked, as this test needs");

        // The bytes that arrive later must still be readable — before the guard
        // this reported EOF on a perfectly live socket.
        let got = tokio::time::timeout(Duration::from_secs(10), net.read(cid))
            .await
            .expect("the read channel survived the abandoned read")
            .unwrap();
        assert_eq!(got.as_deref(), Some(&b"late"[..]));
        server.abort();
    }

    #[tokio::test]
    async fn an_abandoned_accept_leaves_the_listener_accepting() {
        let net = SystemNet::new();
        let (lid, addr) = net
            .listen("127.0.0.1".to_string(), 0, ListenOptions::default())
            .await
            .unwrap();

        let abandoned = tokio::time::timeout(Duration::from_millis(100), net.accept(lid)).await;
        assert!(abandoned.is_err(), "the accept parked, as this test needs");

        let client = net.clone();
        let port = addr.local_port;
        let connect = tokio::spawn(async move {
            client
                .connect("127.0.0.1".to_string(), port, ConnectOptions::default())
                .await
        });
        let accepted = tokio::time::timeout(Duration::from_secs(10), net.accept(lid))
            .await
            .expect("the accept channel survived the abandoned accept")
            .unwrap();
        assert!(accepted.is_some());
        let (cid, _) = connect.await.unwrap().unwrap();
        net.close(cid).await.unwrap();
        net.close_listener(lid).await.unwrap();
    }
}

/// UDP over real loopback sockets (DECISIONS D58). Its own module because none
/// of it shares the stream tests' TLS scaffolding — a datagram socket has no
/// handshake, no peer until it is told one, and no stream to be cancel-safe
/// about.
#[cfg(test)]
mod datagram_tests {
    use super::*;
    use crate::HostAllowlist;
    use std::time::Duration;

    /// An any-source membership on the loopback interface.
    fn group(group: &str) -> MulticastMembership {
        MulticastMembership {
            group: group.to_string(),
            interface: "127.0.0.1".to_string(),
            source: None,
        }
    }

    /// Binds a loopback datagram socket and returns (id, port).
    async fn udp(net: &SystemNet) -> (u64, u16) {
        let (id, info) = net
            .bind_datagram("127.0.0.1".to_string(), 0, DatagramOptions::default())
            .await
            .expect("bind");
        assert!(info.local_port > 0, "port 0 must bind an ephemeral port");
        (id, info.local_port)
    }

    #[tokio::test]
    async fn a_datagram_round_trips_and_carries_its_sender() {
        let net = SystemNet::new();
        let (server, server_port) = udp(&net).await;
        let (client, client_port) = udp(&net).await;

        let sent = net
            .send_to(
                client,
                b"ping".to_vec(),
                Some(("127.0.0.1".to_string(), server_port)),
            )
            .await
            .expect("send");
        assert_eq!(sent, 4);

        let got = net
            .receive(server)
            .await
            .expect("receive")
            .expect("a datagram");
        assert_eq!(got.data, b"ping");
        assert_eq!(got.address, "127.0.0.1");
        // The sender's address is the datagram's, not the socket's: the reply
        // goes back to a port nothing told the server about in advance.
        assert_eq!(got.port, client_port);

        net.send_to(
            server,
            b"pong".to_vec(),
            Some((got.address.clone(), got.port)),
        )
        .await
        .expect("reply");
        let back = net.receive(client).await.unwrap().expect("the reply");
        assert_eq!(back.data, b"pong");

        net.close_datagram(server).await.unwrap();
        net.close_datagram(client).await.unwrap();
    }

    /// Message boundaries are the point of UDP: three sends are three receives,
    /// never one coalesced read the way a stream would deliver them.
    #[tokio::test]
    async fn datagram_boundaries_are_preserved_including_an_empty_one() {
        let net = SystemNet::new();
        let (server, port) = udp(&net).await;
        let (client, _) = udp(&net).await;

        for payload in [&b"one"[..], &b""[..], &b"three"[..]] {
            net.send_to(
                client,
                payload.to_vec(),
                Some(("127.0.0.1".to_string(), port)),
            )
            .await
            .expect("send");
        }

        let mut seen = Vec::new();
        for _ in 0..3 {
            seen.push(net.receive(server).await.unwrap().expect("a datagram").data);
        }
        // A zero-length datagram is a message, not an end of stream.
        assert_eq!(seen, vec![b"one".to_vec(), Vec::new(), b"three".to_vec()]);

        net.close_datagram(server).await.unwrap();
        net.close_datagram(client).await.unwrap();
    }

    /// A connected socket sends with no address and hears only its peer — the
    /// second half is what makes `connect` more than a convenience.
    #[tokio::test]
    async fn a_connected_socket_sends_without_an_address_and_filters_the_rest() {
        let net = SystemNet::new();
        let (server, server_port) = udp(&net).await;
        let (client, client_port) = udp(&net).await;
        let (stranger, _) = udp(&net).await;

        let info = net
            .connect_datagram(client, "127.0.0.1".to_string(), server_port)
            .await
            .expect("connect");
        assert_eq!(info.remote_port, server_port);
        assert_eq!(info.local_port, client_port);

        net.send_to(client, b"hello".to_vec(), None)
            .await
            .expect("a connected send needs no address");
        assert_eq!(
            net.receive(server).await.unwrap().expect("a datagram").data,
            b"hello"
        );

        // The stranger's datagram is discarded by the OS; the peer's arrives.
        net.send_to(
            stranger,
            b"not for you".to_vec(),
            Some(("127.0.0.1".to_string(), client_port)),
        )
        .await
        .expect("send");
        net.send_to(
            server,
            b"for you".to_vec(),
            Some(("127.0.0.1".to_string(), client_port)),
        )
        .await
        .expect("send");
        assert_eq!(
            net.receive(client).await.unwrap().expect("a datagram").data,
            b"for you"
        );

        for id in [server, client, stranger] {
            net.close_datagram(id).await.unwrap();
        }
    }

    /// An unconnected socket has no peer, so a send with no address is a
    /// mistake worth reporting rather than a datagram sent nowhere.
    #[tokio::test]
    async fn an_unconnected_send_with_no_address_fails() {
        let net = SystemNet::new();
        let (id, _) = udp(&net).await;
        assert!(net.send_to(id, b"x".to_vec(), None).await.is_err());
        net.close_datagram(id).await.unwrap();
    }

    /// The case the close signal exists for: a receive already parked on a
    /// socket nobody is sending to. Without the bell it would wait forever,
    /// because the parked receive holds the socket alive itself.
    #[tokio::test]
    async fn closing_ends_a_parked_receive() {
        let net = SystemNet::new();
        let (id, _) = udp(&net).await;

        let waiting = net.clone();
        let parked = tokio::spawn(async move { waiting.receive(id).await });
        // Long enough for the receive to be parked rather than pending.
        tokio::time::sleep(Duration::from_millis(50)).await;
        net.close_datagram(id).await.unwrap();

        let ended = tokio::time::timeout(Duration::from_secs(5), parked)
            .await
            .expect("the parked receive must end at the close")
            .unwrap()
            .expect("close is not an error");
        assert!(ended.is_none(), "a closed socket receives nothing");
    }

    #[tokio::test]
    async fn a_closed_socket_receives_nothing_and_sends_nothing() {
        let net = SystemNet::new();
        let (id, port) = udp(&net).await;
        net.close_datagram(id).await.unwrap();
        // Idempotent, like every other close here.
        net.close_datagram(id).await.unwrap();
        assert!(net.receive(id).await.unwrap().is_none());
        assert!(
            net.send_to(id, b"x".to_vec(), Some(("127.0.0.1".to_string(), port)))
                .await
                .is_err()
        );
    }

    /// Multicast end to end over loopback: two sockets sharing a port both
    /// receive one send to the group.
    #[tokio::test]
    async fn a_multicast_send_reaches_every_member() {
        // An administratively scoped group (RFC 2365) on the loopback
        // interface: no network, no neighbour disturbed — and, unlike a
        // well-known group such as mDNS's `224.0.0.251`, nothing *else* on the
        // machine is joined to it. That matters for the leave half: Linux's
        // `IP_MULTICAST_ALL` delivers a group to any socket bound to the port
        // once anything on the host has joined it, so a group avahi is sitting
        // in would keep arriving after this socket left.
        const GROUP: &str = "239.255.42.99";
        let net = SystemNet::new();
        let member = |port: u16| {
            let net = net.clone();
            async move {
                net.bind_datagram(
                    "0.0.0.0".to_string(),
                    port,
                    DatagramOptions {
                        reuse_address: true,
                        reuse_port: cfg!(unix),
                        multicast_loopback: Some(true),
                        ..DatagramOptions::default()
                    },
                )
                .await
            }
        };
        let Ok((first, info)) = member(0).await else {
            return; // no multicast-capable interface here
        };
        let port = info.local_port;
        let (second, _) = member(port).await.expect("a second member on one port");
        for id in [first, second] {
            if net
                .set_multicast_membership(id, group(GROUP), true)
                .await
                .is_err()
            {
                return; // loopback multicast unavailable (some CI kernels)
            }
        }

        let (sender, _) = udp(&net).await;
        net.send_to(
            sender,
            b"announce".to_vec(),
            Some((GROUP.to_string(), port)),
        )
        .await
        .expect("send to the group");

        for id in [first, second] {
            let got = tokio::time::timeout(Duration::from_secs(2), net.receive(id)).await;
            let Ok(Ok(Some(datagram))) = got else {
                return; // the datagram did not loop back on this host
            };
            assert_eq!(datagram.data, b"announce");
        }

        // Leaving is the other half: with no member left, the same send is not
        // delivered. *Both* have to leave — Linux's `IP_MULTICAST_ALL` is on by
        // default, so a socket bound to the port keeps receiving a group any
        // socket on the host is still joined to.
        for id in [first, second] {
            net.set_multicast_membership(id, group(GROUP), false)
                .await
                .expect("leave");
        }
        net.send_to(
            sender,
            b"after leaving".to_vec(),
            Some((GROUP.to_string(), port)),
        )
        .await
        .expect("send to the group");
        let after = tokio::time::timeout(Duration::from_millis(300), net.receive(first)).await;
        assert!(
            after.is_err(),
            "a group nobody is joined to delivers nothing"
        );

        for id in [first, second, sender] {
            net.close_datagram(id).await.unwrap();
        }
    }

    /// The allowlist is checked per **destination**, not once per socket: one
    /// datagram socket sends to as many peers as it likes.
    #[tokio::test]
    async fn a_send_outside_the_allowlist_is_refused() {
        let net =
            SystemNet::new().with_allowlist(HostAllowlist::parse(["127.0.0.1:9999"]).unwrap());
        // Binding is the listen list's business, and nothing was scoped there.
        let (id, _) = udp(&net).await;

        net.send_to(
            id,
            b"allowed".to_vec(),
            Some(("127.0.0.1".to_string(), 9999)),
        )
        .await
        .expect("the named destination is reachable");

        let err = net
            .send_to(id, b"denied".to_vec(), Some(("127.0.0.1".to_string(), 53)))
            .await
            .expect_err("a destination outside the list must be refused");
        assert_eq!(err.code(), Some(ErrorCode::PermissionDenied), "{err}");

        // …and so is fixing the peer to one, which is the same reach by another
        // route.
        let Err(err) = net.connect_datagram(id, "127.0.0.1".to_string(), 53).await else {
            panic!("connect is a destination too");
        };
        assert_eq!(err.code(), Some(ErrorCode::PermissionDenied), "{err}");
        net.close_datagram(id).await.unwrap();
    }

    #[tokio::test]
    async fn a_bind_outside_the_listen_allowlist_is_refused() {
        let net = SystemNet::new()
            .with_listen_allowlist(HostAllowlist::parse(["127.0.0.1:7070"]).unwrap());
        let Err(err) = net
            .bind_datagram("127.0.0.1".to_string(), 7071, DatagramOptions::default())
            .await
        else {
            panic!("an address outside the list must be refused");
        };
        assert_eq!(err.code(), Some(ErrorCode::PermissionDenied), "{err}");
        // Nothing was claimed: the same port binds when it is allowed.
        let (id, _) = net
            .bind_datagram("127.0.0.1".to_string(), 7070, DatagramOptions::default())
            .await
            .expect("the allowed address binds");
        net.close_datagram(id).await.unwrap();
    }

    /// Truncation is reported by the one observation that can distinguish it:
    /// a datagram that filled the buffer exactly. The buffer is deliberately a
    /// byte longer than IPv4 can deliver, so a full one means the message did
    /// not fit — there is no size of real datagram that reaches this.
    ///
    /// Tested on the classifier rather than over a socket: producing a truncated
    /// datagram needs an IPv6 jumbogram, which loopback will not carry.
    #[test]
    fn a_datagram_that_fills_the_buffer_is_reported_as_truncated() {
        let from: SocketAddr = "127.0.0.1:1234".parse().unwrap();
        let buf = vec![0u8; RECV_BUFFER];
        assert!(datagram_from(&buf, RECV_BUFFER, from).truncated);
        // The largest datagram IPv4 can actually deliver is *not* truncated.
        let whole = datagram_from(&buf, MAX_DATAGRAM, from);
        assert!(!whole.truncated);
        assert_eq!(whole.data.len(), MAX_DATAGRAM);
    }

    /// The pool hands a buffer back after each receive and keeps a bounded
    /// number of them, so a burst of concurrent receives cannot leave the socket
    /// holding megabytes it will never need again.
    #[test]
    fn the_buffer_pool_reuses_and_stays_bounded() {
        let pool = Arc::new(BufferPool::default());
        {
            let mut first = pool.take();
            assert_eq!(first.buf().len(), RECV_BUFFER);
        }
        assert_eq!(pool.free.lock().unwrap().len(), 1, "returned on drop");
        {
            // More at once than the pool retains.
            let _held: Vec<Scratch> = (0..POOL_LIMIT + 3).map(|_| pool.take()).collect();
        }
        assert_eq!(
            pool.free.lock().unwrap().len(),
            POOL_LIMIT,
            "the surplus is dropped rather than pooled"
        );
    }

    /// A batch is one call and many datagrams — each still whole, each still
    /// its own message on the wire.
    #[tokio::test]
    async fn send_many_and_receive_many_move_a_batch() {
        let net = SystemNet::new();
        let (server, port) = udp(&net).await;
        let (client, _) = udp(&net).await;
        let to = || Some(("127.0.0.1".to_string(), port));

        let sent = net
            .send_many(
                client,
                vec![
                    (b"one".to_vec(), to()),
                    (b"two".to_vec(), to()),
                    (b"three".to_vec(), to()),
                ],
            )
            .await
            .expect("send the batch");
        assert_eq!(sent, 3);

        // The first receive waits; the rest are taken without waiting, so one
        // call can return all three.
        let mut bodies = Vec::new();
        while bodies.len() < 3 {
            let batch = net
                .receive_many(server, 16)
                .await
                .expect("receive")
                .expect("a batch");
            assert!(!batch.is_empty(), "a batch is never empty");
            bodies.extend(batch.into_iter().map(|d| d.data));
        }
        assert_eq!(
            bodies,
            vec![b"one".to_vec(), b"two".to_vec(), b"three".to_vec()]
        );

        net.close_datagram(server).await.unwrap();
        net.close_datagram(client).await.unwrap();
    }

    /// `max` is honoured, so a caller can bound how much one crossing hands
    /// back — the rest stays in the kernel for the next call.
    #[tokio::test]
    async fn receive_many_stops_at_max() {
        let net = SystemNet::new();
        let (server, port) = udp(&net).await;
        let (client, _) = udp(&net).await;
        for _ in 0..4 {
            net.send_to(client, b"x".to_vec(), Some(("127.0.0.1".to_string(), port)))
                .await
                .expect("send");
        }
        let batch = net.receive_many(server, 2).await.unwrap().expect("a batch");
        assert!(batch.len() <= 2, "got {} datagrams", batch.len());
        net.close_datagram(server).await.unwrap();
        net.close_datagram(client).await.unwrap();
    }

    /// A batch that fails part-way says how much of it left — "none of them"
    /// and "the first two of three" call for different recovery.
    #[tokio::test]
    async fn a_refused_destination_reports_how_much_of_the_batch_went() {
        let net =
            SystemNet::new().with_allowlist(HostAllowlist::parse(["127.0.0.1:9999"]).unwrap());
        let (id, _) = udp(&net).await;
        let allowed = || Some(("127.0.0.1".to_string(), 9999));
        let Err(err) = net
            .send_many(
                id,
                vec![
                    (b"a".to_vec(), allowed()),
                    (b"b".to_vec(), allowed()),
                    (b"c".to_vec(), Some(("127.0.0.1".to_string(), 53))),
                ],
            )
            .await
        else {
            panic!("the third destination is outside the list");
        };
        assert_eq!(err.code(), Some(ErrorCode::PermissionDenied), "{err}");
        assert!(err.to_string().contains("after 2 of the batch"), "{err}");
        net.close_datagram(id).await.unwrap();
    }

    /// Post-bind options reach the socket, and the ones that are IPv4-only are
    /// refused on a v6 socket rather than silently setting nothing.
    #[tokio::test]
    async fn options_can_be_set_after_the_bind() {
        let net = SystemNet::new();
        let (id, _) = udp(&net).await;
        for option in [
            DatagramOption::Ttl(7),
            DatagramOption::MulticastTtl(3),
            DatagramOption::Broadcast(true),
            DatagramOption::MulticastLoopback(false),
            DatagramOption::MulticastInterface("127.0.0.1".to_string()),
        ] {
            net.set_datagram_option(id, option)
                .await
                .expect("the option applies");
        }
        net.close_datagram(id).await.unwrap();

        let bound = net
            .bind_datagram("::1".to_string(), 0, DatagramOptions::default())
            .await;
        let Ok((v6, _)) = bound else { return }; // no IPv6 on this host
        let refused = net
            .set_datagram_option(v6, DatagramOption::Broadcast(true))
            .await;
        assert!(refused.is_err(), "IPv6 has no broadcast");
        // …while the v6 spelling of a shared option still applies.
        net.set_datagram_option(v6, DatagramOption::MulticastLoopback(false))
            .await
            .expect("the v6 option applies");
        net.close_datagram(v6).await.unwrap();
    }

    /// Source-specific multicast is a different membership, not a filter on the
    /// ordinary one — so it is joined and left with the source named both times.
    #[tokio::test]
    async fn a_source_specific_membership_is_joined_and_left() {
        let net = SystemNet::new();
        let (id, _) = udp(&net).await;
        let ssm = |source: &str| MulticastMembership {
            group: "239.255.42.97".to_string(),
            interface: "127.0.0.1".to_string(),
            source: Some(source.to_string()),
        };
        // A host with no multicast-capable loopback cannot run this.
        if net
            .set_multicast_membership(id, ssm("127.0.0.1"), true)
            .await
            .is_err()
        {
            net.close_datagram(id).await.unwrap();
            return;
        }
        net.set_multicast_membership(id, ssm("127.0.0.1"), false)
            .await
            .expect("leave the same membership");

        // A malformed source is refused before any syscall.
        let Err(err) = net
            .set_multicast_membership(id, ssm("not-an-address"), true)
            .await
        else {
            panic!("a source is an address");
        };
        assert!(err.to_string().contains("source"), "{err}");

        // IPv6 has no source-specific membership here, and says so.
        let v6 = MulticastMembership {
            group: "ff02::fb".to_string(),
            interface: String::new(),
            source: Some("::1".to_string()),
        };
        let Err(err) = net.set_multicast_membership(id, v6, true).await else {
            panic!("v6 SSM is not supported here");
        };
        assert!(err.to_string().contains("IPv4-only"), "{err}");
        net.close_datagram(id).await.unwrap();
    }

    /// A datagram id names nothing in the stream namespace, and vice versa —
    /// the two maps are separate on purpose.
    #[tokio::test]
    async fn the_two_namespaces_do_not_overlap() {
        let net = SystemNet::new();
        let (id, _) = udp(&net).await;
        assert!(net.read(id).await.unwrap().is_none());
        assert!(net.receive(id + 1).await.unwrap().is_none());
        net.close_datagram(id).await.unwrap();
    }
}

/// A TLS listener that no client can complete a handshake against is
/// indistinguishable, from the server side, from a listener nobody is calling —
/// the connection is dropped and the acceptor moves on, by design. This event
/// is the only thing that tells those two apart.
#[cfg(test)]
mod tracing_tests {
    use super::tests::net_trusting;
    use super::*;
    use crate::trace_capture;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn a_failed_tls_handshake_is_logged_with_its_peer() {
        trace_capture::install();
        let ck = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
        let net = net_trusting(ck.cert.der().clone());
        let (lid, addr) = net
            .listen(
                "127.0.0.1".to_string(),
                0,
                ListenOptions {
                    cert: ck.cert.pem().into_bytes(),
                    key: ck.signing_key.serialize_pem().into_bytes(),
                    alpn: vec![],
                    reuse_port: false,
                },
            )
            .await
            .unwrap();

        // Plaintext at a TLS port: rejected as a bad first record.
        let mut tcp = tokio::net::TcpStream::connect(("127.0.0.1", addr.local_port))
            .await
            .unwrap();
        let peer = tcp.local_addr().unwrap();
        let _ = tcp.write_all(b"hello, not a client hello").await;

        let mine = trace_capture::wait_for(
            &["tls handshake failed", &format!("peer={peer}")],
            std::time::Duration::from_secs(5),
        )
        .await;
        assert!(
            !mine.is_empty(),
            "the handshake failure must be logged against the peer that caused it; saw: {:?}",
            trace_capture::lines_containing(&["tls handshake failed"]),
        );
        let line = &mine[0];
        assert!(
            line.contains("[DEBUG] runtime::net"),
            "peer-driven failures log at debug on the net target, not louder: {line}",
        );
        assert!(
            line.contains("error="),
            "the reason is the whole point of the event: {line}",
        );
        net.close_listener(lid).await.unwrap();
    }
}
