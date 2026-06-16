//! OS-backed [`NetProvider`] — tokio TCP sockets for `runtime:net` (SPEC §12).
//!
//! Each socket's I/O runs in **spawned runtime tasks** (a reader and a writer)
//! that move bytes over channels; the ops just send/recv on those channels.
//! This is the same shape the HTTP client uses: the actual I/O is driven by the
//! runtime's reactor (via spawned tasks), so reads that must wait for bytes make
//! progress — polling the raw socket future inline from the op loop would not.
//! TLS is a follow-up; `connect(tls = true)` errors rather than downgrading.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use es_runtime_providers::{BoxFuture, ConnectOptions, NetProvider, ProviderError, SocketInfo};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

type ReadRx = mpsc::Receiver<Result<Vec<u8>, String>>;
type WriteTx = mpsc::Sender<Vec<u8>>;
type AcceptRx = mpsc::Receiver<(TcpStream, SocketAddr)>;

/// A connection's channel ends. `read_rx` is taken out during a read; `write_tx`
/// is cloned to send and dropped (set to `None`) to half-close.
struct Slot {
    read_rx: Option<ReadRx>,
    write_tx: Option<WriteTx>,
}

/// A [`NetProvider`] over real tokio TCP sockets. The `Arc`s are cloned into each
/// returned future so the futures stay `'static`.
#[derive(Clone, Default)]
pub struct SystemNet {
    sockets: Arc<Mutex<HashMap<u64, Slot>>>,
    listeners: Arc<Mutex<HashMap<u64, AcceptRx>>>,
    next_id: Arc<AtomicU64>,
}

impl SystemNet {
    /// Builds an empty socket registry.
    pub fn new() -> Self {
        Self::default()
    }

    fn id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Splits `stream` and spawns its reader + writer tasks, returning the
    /// channel ends to register.
    fn spawn_socket(stream: TcpStream) -> Slot {
        let (mut r, mut w) = stream.into_split();
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
        }
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
            if opts.secure {
                return Err(err(
                    "runtime:net TLS is not supported yet (plaintext TCP only)",
                ));
            }
            let stream = TcpStream::connect((host.as_str(), port))
                .await
                .map_err(err)?;
            let _ = stream.set_nodelay(true);
            let info = info_of(stream.local_addr().ok(), stream.peer_addr().ok());
            let id = this.id();
            this.sockets
                .lock()
                .unwrap()
                .insert(id, SystemNet::spawn_socket(stream));
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
}
