//! Which connection a request goes on (DECISIONS D217, D218): the pool, what
//! it knows of each origin, and [`send`], which picks a free HTTP/2 stream,
//! an idle HTTP/1.1 connection or a new connection and hands the request to
//! that protocol's transport ([`super::h1`], [`super::h2`]). Also the shape a
//! transport hands back: a [`Response`] whose body is a stream of [`Frame`]s.
//!
//! The protocol is the server's choice, by ALPN, and the pool remembers it per
//! origin. A request to an origin that is HTTP/2 — or not yet known — waits
//! for a connect already in flight to it rather than starting its own, and
//! shares that connection if it negotiated `h2`; only an origin known to be
//! HTTP/1.1, where concurrency *is* more connections, connects in parallel at
//! once. An HTTP/2 connection is used while it has a stream free under the
//! server's `SETTINGS_MAX_CONCURRENT_STREAMS`; when every one is full, the
//! next request opens another.
//!
//! What [`HttpClient`](super::HttpClient) decides — redirects, decoding, the
//! headers it adds — stays there; this module only carries one request to one
//! origin and brings its response back.
//!
//! A request is sent again once, on a fresh connection, only when that is
//! known to be safe: on HTTP/2 when the server did not process it (RFC 9113
//! §8.7), and on a reused HTTP/1.1 connection that closed before any of the
//! response arrived when its body can be sent again and its method is
//! idempotent (RFC 9110 §9.2.2) — the server may have read the request before
//! it closed, and a `POST` sent twice is two `POST`s.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

use crate::client::types::{HeaderMap, Method, StatusCode, Version};
use bytes::Bytes;
use es_runtime_common::ErrorCode;
use es_runtime_providers::ProviderError;
use tokio::io::{AsyncRead, ReadBuf};
use tokio::sync::watch;

use super::h1::exchange_h1;
use crate::client::h2::{H2Conn, Reservation};
use crate::client::{Body, Conn};

/// The error type a response body reports.
pub(crate) type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// A response body, whichever version carried it.
pub(crate) type ResponseStream =
    Pin<Box<dyn futures_core::Stream<Item = Result<Frame, BoxError>> + Send>>;

/// One piece of a response body: data, or the trailer section that ends it.
pub(crate) enum Frame {
    Data(Bytes),
    Trailers(HeaderMap),
}

/// A response as a transport hands it to the client's policy: the head, and
/// the body still to come.
pub(crate) struct Response {
    pub(crate) status: StatusCode,
    pub(crate) version: Version,
    pub(crate) headers: HeaderMap,
    pub(crate) body: ResponseStream,
}

/// How long an idle pooled connection is kept.
const POOL_IDLE: Duration = Duration::from_secs(90);

/// The most idle HTTP/1.1 connections kept per origin. Well above the
/// concurrency a program usually reaches against one origin: a cap below it
/// throws away connections a moment before the next wave of requests wants
/// them. Idle ones still expire after [`POOL_IDLE`].
const MAX_IDLE_PER_ORIGIN: usize = 1024;

/// Where a connection goes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct Origin {
    pub(crate) secure: bool,
    pub(crate) host: String,
    pub(crate) port: u16,
}

struct Idle {
    conn: Conn,
    since: Instant,
}

/// What an origin's server chose, once a connection to it has said.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Learned {
    H1,
    H2,
}

#[derive(Default)]
struct PoolInner {
    h1: HashMap<Origin, Vec<Idle>>,
    h2: HashMap<Origin, Vec<H2Conn>>,
    learned: HashMap<Origin, Learned>,
    /// A connect in flight per origin: its id, and a watch that ends when it
    /// does. Requests that would share its outcome wait on it.
    connecting: HashMap<Origin, (u64, watch::Receiver<()>)>,
    next_ticket: u64,
}

/// What a request that needs a new connection does.
enum Connect {
    /// Connect; the ticket tells others waiting on this origin when it ends.
    Go(Ticket),
    /// Wait for the connect in flight, then look again.
    Wait(watch::Receiver<()>),
}

/// The right to connect to an origin others may be waiting on. Dropped —
/// connected, failed or abandoned — it wakes them.
struct Ticket {
    pool: Pool,
    origin: Origin,
    id: u64,
    _done: Option<watch::Sender<()>>,
}

impl Ticket {
    fn learned(&self, protocol: Learned) {
        let mut inner = self.pool.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.learned.insert(self.origin.clone(), protocol);
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        let mut inner = self.pool.inner.lock().unwrap_or_else(|e| e.into_inner());
        if inner
            .connecting
            .get(&self.origin)
            .is_some_and(|(id, _)| *id == self.id)
        {
            inner.connecting.remove(&self.origin);
        }
    }
}

/// The pool, shared by every clone of a client.
#[derive(Clone, Default)]
pub(crate) struct Pool {
    inner: Arc<Mutex<PoolInner>>,
}

impl Pool {
    fn take_h1(&self, origin: &Origin) -> Option<Conn> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let idle = inner.h1.get_mut(origin)?;
        while let Some(mut entry) = idle.pop() {
            if entry.since.elapsed() < POOL_IDLE && still_open(&mut entry.conn) {
                return Some(entry.conn);
            }
        }
        None
    }

    pub(super) fn put_h1(&self, origin: Origin, conn: Conn) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let idle = inner.h1.entry(origin).or_default();
        idle.retain(|e| e.since.elapsed() < POOL_IDLE);
        if idle.len() < MAX_IDLE_PER_ORIGIN {
            idle.push(Idle {
                conn,
                since: Instant::now(),
            });
        }
    }

    /// A free stream on an HTTP/2 connection to `origin`, if one has room.
    /// Closed connections are forgotten on the way.
    fn reserve_h2(&self, origin: &Origin) -> Option<Reservation> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let conns = inner.h2.get_mut(origin)?;
        conns.retain(|c| !c.is_closed());
        conns.iter().find_map(H2Conn::try_reserve)
    }

    fn put_h2(&self, origin: Origin, conn: H2Conn) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.h2.entry(origin).or_default().push(conn);
    }

    /// Whether this request connects now or waits for a connect in flight.
    /// Only a secure origin can turn out to be HTTP/2, so only there is a
    /// connect worth sharing; and one known to be HTTP/1.1 never waits.
    fn begin_connect(&self, origin: &Origin) -> Connect {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let may_share = origin.secure && inner.learned.get(origin) != Some(&Learned::H1);
        if may_share && let Some((_, done)) = inner.connecting.get(origin) {
            return Connect::Wait(done.clone());
        }
        inner.next_ticket += 1;
        let id = inner.next_ticket;
        let done = if may_share {
            let (tx, rx) = watch::channel(());
            inner.connecting.insert(origin.clone(), (id, rx));
            Some(tx)
        } else {
            None
        };
        Connect::Go(Ticket {
            pool: self.clone(),
            origin: origin.clone(),
            id,
            _done: done,
        })
    }
}

/// Whether an idle connection is still usable: nothing to read and not
/// closed. Anything readable on an idle connection is either the server's
/// close or bytes nobody asked for, and either way it is not reused.
fn still_open(conn: &mut Conn) -> bool {
    let mut cx = Context::from_waker(Waker::noop());
    let mut byte = [0u8; 1];
    let mut buf = ReadBuf::new(&mut byte);
    matches!(Pin::new(conn).poll_read(&mut cx, &mut buf), Poll::Pending)
}

/// One request on `conn`, reused or not.
pub(crate) struct Exchange<'a> {
    pub(crate) method: &'a Method,
    /// The request-target: origin form, or the absolute URL when forwarding
    /// through a proxy.
    pub(crate) target: &'a str,
    /// The `Host` value: the URL's authority.
    pub(crate) host: &'a str,
    pub(crate) headers: &'a HeaderMap,
}

/// Sends a request to `origin` on whichever connection the pool picks — a
/// free HTTP/2 stream, an idle HTTP/1.1 connection, or a new connection —
/// retrying once when that is known to be safe: on HTTP/2 when the server did
/// not process the request (RFC 9113 §8.7), on a reused HTTP/1.1 connection
/// when it closed before any response and the method is idempotent.
pub(crate) async fn send<F, Fut>(
    pool: &Pool,
    origin: &Origin,
    exchange: Exchange<'_>,
    body: Body,
    connect: F,
) -> Result<Response, ProviderError>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<Conn, ProviderError>>,
{
    let mut body = body;
    let mut retried = false;
    loop {
        // What is sent again if this attempt may be retried.
        let replay = replay(&body);
        let outcome = if let Some(stream) = pool.reserve_h2(origin) {
            stream
                .send(&exchange, origin.secure, body)
                .await
                .map_err(|f| (f.error, f.retry))
        } else if let Some(conn) = pool.take_h1(origin) {
            exchange_h1(pool, origin, conn, &exchange, body)
                .await
                .map_err(|f| {
                    let retry = f.before_response && idempotent(exchange.method);
                    (f.error, retry)
                })
        } else {
            match pool.begin_connect(origin) {
                Connect::Wait(mut done) => {
                    // Ends when the connect in flight does; then look again.
                    let _ = done.changed().await;
                    continue;
                }
                Connect::Go(ticket) => {
                    let conn = connect().await?;
                    if conn.is_h2() {
                        let h2 = H2Conn::start(conn).await?;
                        ticket.learned(Learned::H2);
                        // This request's stream is taken before the others
                        // waiting are woken, so they see what is left.
                        let stream = h2.try_reserve();
                        pool.put_h2(origin.clone(), h2);
                        drop(ticket);
                        match stream {
                            Some(stream) => stream
                                .send(&exchange, origin.secure, body)
                                .await
                                .map_err(|f| (f.error, f.retry)),
                            // Going away before any stream (a server
                            // restarting sends GOAWAY with its SETTINGS), or
                            // allowing none: nothing was sent, so once more
                            // on a fresh connection.
                            None => Err((
                                ProviderError::Coded {
                                    code: ErrorCode::ConnectionReset,
                                    message: "request failed: the server took no request on \
                                              its connection"
                                        .to_string(),
                                },
                                true,
                            )),
                        }
                    } else {
                        ticket.learned(Learned::H1);
                        drop(ticket);
                        // A fresh connection: a failure on it is not the
                        // stale-connection case, and is not retried.
                        exchange_h1(pool, origin, conn, &exchange, body)
                            .await
                            .map_err(|f| (f.error, false))
                    }
                }
            }
        };
        match outcome {
            Ok(response) => return Ok(response),
            Err((error, true)) if !retried => match replay {
                Some(again) => {
                    retried = true;
                    body = again;
                }
                None => return Err(error),
            },
            Err((error, _)) => return Err(error),
        }
    }
}

/// A copy of `body` to send again, unless it is a stream (read once).
fn replay(body: &Body) -> Option<Body> {
    match body {
        Body::Empty => Some(Body::Empty),
        Body::Bytes(b) => Some(Body::Bytes(b.clone())),
        Body::Stream(_) => None,
    }
}

/// RFC 9110 §9.2.2: a request that may be repeated with the same effect.
fn idempotent(method: &Method) -> bool {
    [
        Method::GET,
        Method::HEAD,
        Method::OPTIONS,
        Method::TRACE,
        Method::PUT,
        Method::DELETE,
    ]
    .contains(method)
}
