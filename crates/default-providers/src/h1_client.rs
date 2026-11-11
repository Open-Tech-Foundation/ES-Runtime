//! The HTTP client's transport (DECISIONS D217): the connection pool, which
//! picks the protocol and the connection for each request, and HTTP/1.1
//! spoken with `es-runtime-http`'s client codec. HTTP/2 is
//! [`crate::h2_client`].
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
//! What [`HttpClient`](crate::http_client::HttpClient) decides — redirects,
//! decoding, the headers it adds — stays there; this module only carries one
//! request to one origin and brings its response back.
//!
//! A pooled HTTP/1.1 connection is reused only when the previous response
//! ended exactly where its framing said, and is checked still open before it
//! is handed out. A request that fails on a reused connection before any of
//! its response arrived — the server closed the idle connection as it was
//! taken — is retried once on a fresh one, when its body can be sent again and
//! its method is idempotent (RFC 9110 §9.2.2): the server may have read the
//! request before it closed, and a `POST` sent twice is two `POST`s.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

use bytes::Bytes;
use es_runtime_common::ErrorCode;
use es_runtime_http::fields::Field;
use es_runtime_http::h1::{
    Chunk, ChunkedDecoder, Framing, Limits, ParsedResponse, ResponseBody, encode_last_chunk,
    encode_request_head, parse_response,
};
use es_runtime_providers::ProviderError;
use http_body_util::BodyExt;
use hyper::body::Frame;
use hyper::header::{HeaderMap, HeaderName, HeaderValue};
use hyper::{Method, Response, StatusCode, Version};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt, ReadBuf};
use tokio::sync::{mpsc, watch};

use crate::h2_client::{H2Conn, Reservation};
use crate::http_client::{Body, Conn};

/// The error type a response body reports.
pub(crate) type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// A response body, whichever version carried it.
pub(crate) type ResponseStream = http_body_util::combinators::UnsyncBoxBody<Bytes, BoxError>;

/// How long an idle pooled connection is kept.
const POOL_IDLE: Duration = Duration::from_secs(90);

/// The most idle HTTP/1.1 connections kept per origin. Well above the
/// concurrency a program usually reaches against one origin: a cap below it
/// throws away connections a moment before the next wave of requests wants
/// them. Idle ones still expire after [`POOL_IDLE`].
const MAX_IDLE_PER_ORIGIN: usize = 1024;

/// How many body frames may wait between the connection and the reader.
const BODY_CHANNEL: usize = 4;

/// The most body bytes handed over in one frame.
const MAX_FRAME: usize = 64 * 1024;

/// A buffered request body at most this large is written with its head.
const COALESCE_BODY: usize = 64 * 1024;

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

    fn put_h1(&self, origin: Origin, conn: Conn) {
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

/// Why an exchange failed, and whether it may be tried again on a fresh
/// connection.
struct Failure {
    error: ProviderError,
    /// Nothing of a response arrived: the request may not have been seen.
    before_response: bool,
}

fn io_failure(e: &std::io::Error, before_response: bool) -> Failure {
    Failure {
        error: ProviderError::Coded {
            code: ErrorCode::from_io_kind(e.kind()),
            message: format!("request failed: {e}"),
        },
        before_response,
    }
}

fn closed_early() -> ProviderError {
    ProviderError::Coded {
        code: ErrorCode::ConnectionReset,
        message: "the connection closed before the response completed".to_string(),
    }
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
) -> Result<Response<ResponseStream>, ProviderError>
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
    matches!(
        *method,
        Method::GET | Method::HEAD | Method::OPTIONS | Method::TRACE | Method::PUT | Method::DELETE
    )
}

/// The framing a request body goes out with. A method whose request usually
/// has a body says its empty one is empty, as other clients do.
fn request_framing(method: &Method, body: &Body) -> Framing {
    match body {
        Body::Empty if matches!(*method, Method::POST | Method::PUT | Method::PATCH) => {
            Framing::Length(0)
        }
        Body::Empty => Framing::NoBody,
        Body::Bytes(b) => Framing::Length(b.len() as u64),
        Body::Stream(_) => Framing::Chunked,
    }
}

async fn exchange_h1(
    pool: &Pool,
    origin: &Origin,
    mut conn: Conn,
    exchange: &Exchange<'_>,
    body: Body,
) -> Result<Response<ResponseStream>, Failure> {
    let framing = request_framing(exchange.method, &body);
    let mut head = Vec::with_capacity(512);
    let fields = exchange
        .headers
        .iter()
        .map(|(n, v)| (n.as_str().as_bytes(), v.as_bytes()));
    encode_request_head(
        &mut head,
        exchange.method.as_str().as_bytes(),
        exchange.target.as_bytes(),
        exchange.host.as_bytes(),
        fields,
        framing,
    )
    .map_err(|e| Failure {
        error: ProviderError::Other(format!("request: {e}")),
        before_response: false,
    })?;

    // The request out. A streamed body is written by its own task while the
    // response is read: a server may answer before it has read the upload,
    // and waiting for the whole body first would deadlock against it.
    match body {
        Body::Empty => conn
            .write_all(&head)
            .await
            .map_err(|e| io_failure(&e, true))?,
        Body::Bytes(bytes) if bytes.len() <= COALESCE_BODY => {
            head.extend_from_slice(&bytes);
            conn.write_all(&head)
                .await
                .map_err(|e| io_failure(&e, true))?;
        }
        Body::Bytes(bytes) => {
            conn.write_all(&head)
                .await
                .map_err(|e| io_failure(&e, true))?;
            conn.write_all(&bytes)
                .await
                .map_err(|e| io_failure(&e, true))?;
        }
        Body::Stream(stream) => {
            // The head goes out with the first chunks, from the writer.
            let (rd, wr) = tokio::io::split(conn);
            let writer = tokio::spawn(write_chunked(wr, stream, head));
            return read_response(
                pool,
                origin,
                Halves::Split(rd),
                exchange.method,
                Some(writer),
            )
            .await;
        }
    }
    read_response(pool, origin, Halves::Whole(conn), exchange.method, None).await
}

/// Writes a streamed request body, chunked. Ends the body cleanly when the
/// stream ends; on a stream error, stops without the last chunk so the server
/// sees a truncated body rather than a complete one.
/// Hands the write half back once the body is out, so the connection can be
/// reunited and pooled.
async fn write_chunked(
    mut wr: tokio::io::WriteHalf<Conn>,
    mut stream: es_runtime_providers::ByteStream,
    head: Vec<u8>,
) -> std::io::Result<tokio::io::WriteHalf<Conn>> {
    let Ok(mut end) = crate::h1_conn::write_chunks(&mut wr, &mut stream, head).await? else {
        return Err(std::io::Error::other("the request body stream failed"));
    };
    let _ = encode_last_chunk(&mut end, std::iter::empty());
    wr.write_all(&end).await?;
    wr.flush().await?;
    Ok(wr)
}

/// The task writing a streamed request body.
type Writer = tokio::task::JoinHandle<std::io::Result<tokio::io::WriteHalf<Conn>>>;

/// The connection, whole (and so poolable) or split for a streamed upload.
enum Halves {
    Whole(Conn),
    Split(tokio::io::ReadHalf<Conn>),
}

impl Halves {
    async fn read_more(&mut self, buf: &mut Vec<u8>) -> std::io::Result<usize> {
        buf.reserve(16 * 1024);
        match self {
            Halves::Whole(c) => c.read_buf(buf).await,
            Halves::Split(r) => r.read_buf(buf).await,
        }
    }
}

async fn read_response(
    pool: &Pool,
    origin: &Origin,
    mut io: Halves,
    method: &Method,
    writer: Option<Writer>,
) -> Result<Response<ResponseStream>, Failure> {
    let limits = Limits::default();
    let to_head = *method == Method::HEAD;
    let mut buf = Vec::new();
    let mut fields: Vec<Field> = Vec::new();
    let head = loop {
        match parse_response(&buf, &limits, &mut fields, to_head) {
            Ok(ParsedResponse::Complete(head)) if head.is_interim() => {
                // 100 Continue, 103 Early Hints: the final response follows.
                buf.drain(..head.len);
                continue;
            }
            Ok(ParsedResponse::Complete(head)) => break head,
            Ok(ParsedResponse::Partial) => {}
            Err(e) => {
                return Err(Failure {
                    error: ProviderError::Other(format!("request failed: {e}")),
                    before_response: false,
                });
            }
        }
        match io.read_more(&mut buf).await {
            Ok(0) => {
                return Err(Failure {
                    error: closed_early(),
                    before_response: buf.is_empty(),
                });
            }
            Ok(_) => {}
            Err(e) => return Err(io_failure(&e, buf.is_empty())),
        }
    };

    let status = StatusCode::from_u16(head.status).map_err(|_| Failure {
        error: ProviderError::Other("request failed: invalid status code".to_string()),
        before_response: false,
    })?;
    let mut headers = HeaderMap::with_capacity(fields.len());
    for f in &fields {
        let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(f.name.of(&buf)),
            HeaderValue::from_bytes(f.value.of(&buf)),
        ) else {
            continue; // validated by the parser; unreachable in practice
        };
        headers.append(name, value);
    }
    buf.drain(..head.len);
    let mut response = Response::new(empty_body());
    *response.status_mut() = status;
    *response.version_mut() = Version::HTTP_11;
    *response.headers_mut() = headers;

    // A switch of protocols is not something this client follows: the
    // connection goes with the response and is not pooled.
    let poolable = head.keep_alive && head.status != 101;
    // An upload the server read to its end before answering — the usual
    // order — has finished writing by now: take the halves back, so the
    // response can go the direct way below.
    let (mut io, writer) = match (io, writer) {
        (Halves::Split(rd), Some(writer)) if writer.is_finished() => match writer.await {
            Ok(Ok(wr)) => (Halves::Whole(rd.unsplit(wr)), None),
            // The upload failed: the connection is not reused.
            _ => (Halves::Split(rd), None),
        },
        other => other,
    };
    let reunited = matches!(io, Halves::Whole(_));
    // The whole body already read with the head — the usual case for a small
    // response — is handed over as it is: no task, no channel, and the
    // connection back in the pool before the caller reads a byte.
    if writer.is_none()
        && let Some((frames, rest)) = buffered_body(&buf, head.body)
    {
        if poolable
            && reunited
            && rest == buf.len()
            && let Halves::Whole(conn) = io
        {
            pool.put_h1(origin.clone(), conn);
        }
        *response.body_mut() = ReadyBody { frames }.boxed_unsync();
        return Ok(response);
    }
    let (tx, rx) = mpsc::channel(BODY_CHANNEL);
    let pool = pool.clone();
    let origin = origin.clone();
    tokio::spawn(async move {
        let reusable = read_body(&mut io, buf, head.body, &tx).await;
        // The upload's write half, back once the body went out whole.
        let write_half = match writer {
            Some(writer) => match writer.await {
                Ok(Ok(wr)) => Some(wr),
                _ => return,
            },
            None => None,
        };
        if !(reusable && poolable) {
            return;
        }
        let conn = match (io, write_half) {
            (Halves::Whole(conn), None) => conn,
            (Halves::Split(rd), Some(wr)) => rd.unsplit(wr),
            _ => return,
        };
        pool.put_h1(origin, conn);
    });
    *response.body_mut() = ChannelBody { rx }.boxed_unsync();
    Ok(response)
}

/// The body's frames, when all of it is already in `buf`, and how many bytes
/// of `buf` it took. `None` when more must be read (or the body runs until
/// the connection closes).
fn buffered_body(
    buf: &[u8],
    framing: ResponseBody,
) -> Option<(std::collections::VecDeque<Frame<Bytes>>, usize)> {
    let mut frames = std::collections::VecDeque::new();
    match framing {
        ResponseBody::Empty => Some((frames, 0)),
        ResponseBody::Length(n) => {
            let n = usize::try_from(n).ok().filter(|&n| n <= buf.len())?;
            frames.push_back(Frame::data(Bytes::copy_from_slice(&buf[..n])));
            Some((frames, n))
        }
        ResponseBody::Chunked => {
            let mut decoder = ChunkedDecoder::new(Limits::default());
            let mut data = Vec::new();
            let mut at = 0;
            loop {
                match decoder.decode(&buf[at..]) {
                    Ok((used, Chunk::Data(d))) => {
                        data.extend_from_slice(d);
                        at += used;
                    }
                    Ok((used, Chunk::End)) => {
                        at += used;
                        break;
                    }
                    // Incomplete, or broken: the reading task reports either.
                    Ok((_, Chunk::NeedMore)) | Err(_) => return None,
                }
            }
            if !data.is_empty() {
                frames.push_back(Frame::data(Bytes::from(data)));
            }
            let mut trailers = HeaderMap::new();
            for (n, v) in decoder.trailers() {
                if let (Ok(n), Ok(v)) = (HeaderName::from_bytes(n), HeaderValue::from_bytes(v)) {
                    trailers.append(n, v);
                }
            }
            if !trailers.is_empty() {
                frames.push_back(Frame::trailers(trailers));
            }
            Some((frames, at))
        }
        ResponseBody::UntilClose => None,
    }
}

/// A body that was complete before anyone asked for it.
struct ReadyBody {
    frames: std::collections::VecDeque<Frame<Bytes>>,
}

impl hyper::body::Body for ReadyBody {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, BoxError>>> {
        Poll::Ready(self.frames.pop_front().map(Ok))
    }

    fn is_end_stream(&self) -> bool {
        self.frames.is_empty()
    }
}

type FrameResult = Result<Frame<Bytes>, BoxError>;

/// Reads the body after the head, handing frames to `tx`. Returns whether it
/// ended exactly where its framing said, with nothing read past it — the
/// condition for the connection to be reused.
async fn read_body(
    io: &mut Halves,
    mut buf: Vec<u8>,
    framing: ResponseBody,
    tx: &mpsc::Sender<FrameResult>,
) -> bool {
    let fail = |e: BoxError| async move {
        let _ = tx.send(Err(e)).await;
        false
    };
    match framing {
        ResponseBody::Empty => buf.is_empty(),
        ResponseBody::Length(mut remaining) => {
            loop {
                if !buf.is_empty() {
                    let n = remaining.min(buf.len() as u64).min(MAX_FRAME as u64) as usize;
                    let data = Bytes::copy_from_slice(&buf[..n]);
                    buf.drain(..n);
                    remaining -= n as u64;
                    if tx.send(Ok(Frame::data(data))).await.is_err() {
                        return false; // the reader went away
                    }
                }
                if remaining == 0 {
                    return buf.is_empty();
                }
                match io.read_more(&mut buf).await {
                    Ok(0) => return fail(Box::new(closed_early())).await,
                    Ok(_) => {}
                    Err(e) => return fail(Box::new(e)).await,
                }
            }
        }
        ResponseBody::Chunked => {
            let mut decoder = ChunkedDecoder::new(Limits::default());
            loop {
                let mut out = Vec::new();
                let mut at = 0;
                let mut ended = false;
                while out.len() < MAX_FRAME {
                    match decoder.decode(&buf[at..]) {
                        Ok((used, Chunk::Data(d))) => {
                            out.extend_from_slice(d);
                            at += used;
                        }
                        Ok((used, Chunk::End)) => {
                            at += used;
                            ended = true;
                            break;
                        }
                        Ok((used, Chunk::NeedMore)) => {
                            at += used;
                            break;
                        }
                        Err(e) => return fail(Box::new(e)).await,
                    }
                }
                buf.drain(..at);
                if !out.is_empty() && tx.send(Ok(Frame::data(Bytes::from(out)))).await.is_err() {
                    return false;
                }
                if ended {
                    let mut trailers = HeaderMap::new();
                    for (n, v) in decoder.trailers() {
                        if let (Ok(n), Ok(v)) =
                            (HeaderName::from_bytes(n), HeaderValue::from_bytes(v))
                        {
                            trailers.append(n, v);
                        }
                    }
                    if !trailers.is_empty() && tx.send(Ok(Frame::trailers(trailers))).await.is_err()
                    {
                        return false;
                    }
                    return buf.is_empty();
                }
                match io.read_more(&mut buf).await {
                    Ok(0) => return fail(Box::new(closed_early())).await,
                    Ok(_) => {}
                    Err(e) => return fail(Box::new(e)).await,
                }
            }
        }
        ResponseBody::UntilClose => loop {
            if !buf.is_empty() {
                let data = Bytes::from(std::mem::take(&mut buf));
                if tx.send(Ok(Frame::data(data))).await.is_err() {
                    return false;
                }
            }
            match io.read_more(&mut buf).await {
                Ok(0) => return false, // the end, and never reusable
                Ok(_) => {}
                Err(e) => return fail(Box::new(e)).await,
            }
        },
    }
}

fn empty_body() -> ResponseStream {
    http_body_util::Empty::new()
        .map_err(|never| match never {})
        .boxed_unsync()
}

/// A body fed by the task reading the connection.
struct ChannelBody {
    rx: mpsc::Receiver<FrameResult>,
}

impl hyper::body::Body for ChannelBody {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, BoxError>>> {
        self.rx.poll_recv(cx)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use futures_util::StreamExt;
    use hyper::{Method, StatusCode};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;
    use url::Url;

    use crate::TrustStore;
    use crate::http_client::{Body, HttpClient, Outbound};

    /// A server that hands each accepted connection, and its number, to `serve`.
    async fn scripted<F, Fut>(serve: F) -> (u16, Arc<AtomicUsize>)
    where
        F: Fn(TcpStream, usize) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let count = Arc::new(AtomicUsize::new(0));
        let seen = count.clone();
        let serve = Arc::new(serve);
        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let n = seen.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(serve(socket, n));
            }
        });
        (port, count)
    }

    /// Reads one request head (up to its blank line).
    async fn read_head(socket: &mut TcpStream) -> Option<Vec<u8>> {
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            if socket.read(&mut byte).await.ok()? == 0 {
                return None;
            }
            head.push(byte[0]);
        }
        Some(head)
    }

    fn get(port: u16, path: &str) -> Outbound {
        Outbound::get(Url::parse(&format!("http://127.0.0.1:{port}{path}")).unwrap())
    }

    async fn text(client: &HttpClient, request: Outbound) -> Result<(StatusCode, String), String> {
        let response = client.send(request).await.map_err(|e| e.to_string())?;
        let status = response.status;
        let body = response.bytes(1 << 20).await.map_err(|e| e.to_string())?;
        Ok((status, String::from_utf8_lossy(&body).into_owned()))
    }

    #[tokio::test]
    async fn a_connection_is_reused_while_its_framing_allows() {
        let (port, connections) = scripted(|mut s, _| async move {
            while read_head(&mut s).await.is_some() {
                s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                    .await
                    .unwrap();
            }
        })
        .await;
        let client = HttpClient::new(&TrustStore::bundled());
        for _ in 0..3 {
            assert_eq!(text(&client, get(port, "/")).await.unwrap().1, "ok");
        }
        assert_eq!(connections.load(Ordering::SeqCst), 1);
    }

    /// The server takes the second request on a kept connection and closes
    /// without answering — the race between reuse and an idle close. A GET is
    /// retried on a fresh connection; a POST is not, since it may have run.
    #[tokio::test]
    async fn a_reused_connection_that_was_closed_is_retried_only_when_idempotent() {
        let (port, connections) = scripted(|mut s, n| async move {
            if read_head(&mut s).await.is_none() {
                return;
            }
            if n == 0 {
                s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nfirst")
                    .await
                    .unwrap();
                let _ = read_head(&mut s).await; // taken, never answered
                return;
            }
            s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nretry")
                .await
                .unwrap();
        })
        .await;
        let client = HttpClient::new(&TrustStore::bundled());
        assert_eq!(text(&client, get(port, "/")).await.unwrap().1, "first");
        assert_eq!(text(&client, get(port, "/")).await.unwrap().1, "retry");
        assert_eq!(connections.load(Ordering::SeqCst), 2);

        let (port, posts) = scripted(|mut s, n| async move {
            if read_head(&mut s).await.is_none() {
                return;
            }
            if n == 0 {
                s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                    .await
                    .unwrap();
                let _ = read_head(&mut s).await;
            }
        })
        .await;
        let client = HttpClient::new(&TrustStore::bundled());
        assert_eq!(text(&client, get(port, "/")).await.unwrap().1, "ok");
        let mut post = get(port, "/");
        post.method = Method::POST;
        post.body = Body::Bytes(bytes::Bytes::from_static(b"x"));
        // Refused rather than retried: one connection, and an error.
        assert!(text(&client, post).await.is_err());
        assert_eq!(
            posts.load(Ordering::SeqCst),
            1,
            "a POST must not be sent twice"
        );
    }

    /// A streamed upload that went out whole leaves its connection reusable:
    /// the two halves are put back together and pooled.
    #[tokio::test]
    async fn a_connection_is_reused_after_a_streamed_upload() {
        let (port, connections) = scripted(|mut s, _| async move {
            loop {
                let Some(_) = read_head(&mut s).await else {
                    return;
                };
                // Read the chunked body to its last chunk.
                let mut tail = Vec::new();
                let mut byte = [0u8; 1];
                while !tail.ends_with(b"0\r\n\r\n") {
                    if s.read(&mut byte).await.unwrap_or(0) == 0 {
                        return;
                    }
                    tail.push(byte[0]);
                }
                s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                    .await
                    .unwrap();
            }
        })
        .await;
        let client = HttpClient::new(&TrustStore::bundled());
        for _ in 0..3 {
            let mut post = get(port, "/");
            post.method = Method::POST;
            post.body = Body::Stream(
                futures_util::stream::iter([Ok(b"abc".to_vec()), Ok(b"def".to_vec())]).boxed(),
            );
            assert_eq!(text(&client, post).await.unwrap().1, "ok");
        }
        assert_eq!(connections.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn interim_responses_are_skipped() {
        let (port, _) = scripted(|mut s, _| async move {
            read_head(&mut s).await;
            s.write_all(
                b"HTTP/1.1 103 Early Hints\r\nLink: </a.css>; rel=preload\r\n\r\nHTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok",
            )
            .await
            .unwrap();
        })
        .await;
        let (status, body) = text(&HttpClient::new(&TrustStore::bundled()), get(port, "/"))
            .await
            .unwrap();
        assert_eq!((status, body.as_str()), (StatusCode::OK, "ok"));
    }

    #[tokio::test]
    async fn a_body_without_a_length_runs_until_the_close_and_is_not_reused() {
        let (port, connections) = scripted(|mut s, _| async move {
            read_head(&mut s).await;
            s.write_all(b"HTTP/1.1 200 OK\r\n\r\nall of it")
                .await
                .unwrap();
        })
        .await;
        let client = HttpClient::new(&TrustStore::bundled());
        assert_eq!(text(&client, get(port, "/")).await.unwrap().1, "all of it");
        assert_eq!(text(&client, get(port, "/")).await.unwrap().1, "all of it");
        assert_eq!(connections.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn an_http_1_0_response_is_refused() {
        let (port, _) = scripted(|mut s, _| async move {
            read_head(&mut s).await;
            s.write_all(b"HTTP/1.0 200 OK\r\n\r\nold").await.unwrap();
        })
        .await;
        let err = text(&HttpClient::new(&TrustStore::bundled()), get(port, "/"))
            .await
            .unwrap_err();
        assert!(err.contains("other than 1.1"), "{err}");
    }

    /// The server answers before reading an upload that never ends. Waiting
    /// for the body to be sent first would hang; the answer arrives instead.
    #[tokio::test]
    async fn an_answer_before_a_streamed_upload_ends_is_received() {
        let (port, _) = scripted(|mut s, _| async move {
            read_head(&mut s).await;
            s.write_all(
                b"HTTP/1.1 413 Content Too Large\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();
            tokio::time::sleep(Duration::from_millis(200)).await;
        })
        .await;
        let endless = futures_util::stream::unfold((), |()| async {
            tokio::time::sleep(Duration::from_millis(5)).await;
            Some((Ok(vec![b'x'; 1024]), ()))
        })
        .boxed();
        let mut put = get(port, "/upload");
        put.method = Method::PUT;
        put.body = Body::Stream(endless);
        let response = tokio::time::timeout(
            Duration::from_secs(5),
            HttpClient::new(&TrustStore::bundled()).send(put),
        )
        .await
        .expect("the answer must not wait for the upload")
        .unwrap();
        assert_eq!(response.status, StatusCode::PAYLOAD_TOO_LARGE);
    }
}
