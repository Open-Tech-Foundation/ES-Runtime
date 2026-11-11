//! One HTTP/1.1 connection, served by `es-runtime-http`'s sans-I/O cores
//! (DECISIONS D217) — the tokio half of the HTTP/1.1 server.
//!
//! The connection reads into one buffer it keeps for its whole life, parses
//! each request head out of it in place, and hands the request to the
//! single-threaded consumer the same way the HTTP/2 path does (a [`Pending`]
//! on the server's channel). While the handler runs, the connection keeps
//! reading: a request body is decoded and forwarded as the guest pulls it, and
//! once the body is done, the read is how a client that went away is noticed.
//!
//! Reading and writing are separate futures on the two halves of the stream,
//! run together. A handler that streams its response while the request body is
//! still arriving — an echo, a proxy — would otherwise deadlock against a
//! client that is itself blocked sending that body.
//!
//! Pipelined requests are answered in order; a request whose body was not
//! fully read by the time its response is sent ends the connection rather than
//! leaving unread bytes to be taken for the next request.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use es_runtime_http::date::{IMF_FIXDATE_LEN, imf_fixdate};
use es_runtime_http::fields::Field;
use es_runtime_http::h1::{
    Body, CHUNK_END, CONTINUE, Chunk, ChunkedDecoder, Framing, Limits, Parsed, RequestError,
    RequestHead, TargetForm, chunk_header, encode_last_chunk, encode_response_head, parse_request,
};
use es_runtime_providers::{
    HttpRequestHeaders, HttpServerBody, HttpServerRequest, HttpServerResponse, PeerCertificate,
    ProviderError, UpgradedIo,
};
use futures_util::{FutureExt, StreamExt};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::compat::TokioAsyncReadCompatExt;

use crate::body_deadline::{BodyDeadline, BodyLimit};
use crate::prefixed::Prefixed;
use crate::system_http::{Pending, PendingUpgrade, valid_authority};

/// How many decoded body chunks may wait between the connection and the
/// guest's stream. The guest pulls; the connection reads only as it does.
const BODY_CHANNEL: usize = 4;

/// The most body bytes decoded into one chunk handed to the guest.
const MAX_BODY_CHUNK: usize = 64 * 1024;

/// A response body at most this large is written in one call with its head.
const COALESCE_BODY: usize = 16 * 1024;

/// The spare room a read is given. `read_buf` on a full `Vec` grows it by a
/// few bytes at a time, which would read a large body in tiny pieces.
const READ_SIZE: usize = 16 * 1024;

/// Reads once into `buf`'s spare capacity, reserving [`READ_SIZE`] first.
async fn read_more<R: AsyncRead + Unpin>(rd: &mut R, buf: &mut Vec<u8>) -> std::io::Result<usize> {
    buf.reserve(READ_SIZE);
    rd.read_buf(buf).await
}

/// What a connection needs from the server it belongs to.
pub(crate) struct ConnContext {
    pub(crate) tx: mpsc::Sender<Pending>,
    pub(crate) origin: Arc<str>,
    pub(crate) peer_host: Arc<str>,
    pub(crate) peer_port: u16,
    pub(crate) peer_certificate: Option<Arc<PeerCertificate>>,
    pub(crate) drain_rx: watch::Receiver<bool>,
    /// The time a request head may take to arrive, counted from when the
    /// connection starts waiting for it — an idle keep-alive connection is
    /// reaped by the same deadline (D43).
    pub(crate) header_read: Option<Duration>,
    pub(crate) body_limit: Option<BodyLimit>,
    pub(crate) limits: Limits,
}

/// Serves HTTP/1.1 on `io` until the connection ends. `buf` holds whatever was
/// read before the protocol was chosen.
pub(crate) async fn serve<S>(io: S, mut buf: Vec<u8>, mut ctx: ConnContext)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut rd, mut wr) = tokio::io::split(io);
    let mut fields: Vec<Field> = Vec::with_capacity(16);
    // The last `Host` this connection sent and what validating it gave:
    // nearly every request on a connection names the same one, and the
    // validation is a pure function of the value, so a hit cannot differ
    // from a miss.
    let mut host_cache: Option<(Vec<u8>, Option<String>)> = None;
    loop {
        // ---- the next head ------------------------------------------------
        let head = match read_head(&mut rd, &mut buf, &mut fields, &mut ctx).await {
            HeadOutcome::Head(head) => head,
            HeadOutcome::Closed => return,
            HeadOutcome::Refused(e) => {
                // The same event hyper's errors were reported under: one name to
                // filter on, whichever path refused the connection.
                tracing::debug!(target: "runtime::http", error = %e, "connection ended with an error");
                let _ = write_error(&mut wr, e.status()).await;
                let _ = wr.shutdown().await;
                return;
            }
        };

        // ---- the request, to the guest ------------------------------------
        let method = String::from_utf8_lossy(head.method.of(&buf)).into_owned();
        let is_head = method == "HEAD";
        let Some(url) = request_url(&buf, &head, &ctx.origin, &mut host_cache) else {
            let _ = write_error(&mut wr, 400).await;
            let _ = wr.shutdown().await;
            return;
        };
        let websocket = websocket_accept(&buf, &head, &fields, &method);
        let headers = deferred_headers(&buf, &head, &fields);
        let body_in = match head.body {
            Body::Empty => BodyIn::Done,
            Body::Length(n) => BodyIn::Length(n),
            Body::Chunked => BodyIn::Chunked(Box::new(ChunkedDecoder::new(ctx.limits))),
        };
        // The head's bytes are consumed; what follows is body or next request.
        buf.drain(..head.len);

        let (body, body_tx) = if body_in.is_done() {
            (HttpServerBody::Empty, None)
        } else {
            let (btx, brx) = mpsc::channel::<Result<Vec<u8>, ProviderError>>(BODY_CHANNEL);
            let stream: es_runtime_providers::ByteStream =
                Box::pin(futures_util::stream::unfold(brx, |mut rx| async move {
                    rx.recv().await.map(|item| (item, rx))
                }));
            (
                HttpServerBody::Stream(BodyDeadline::wrap(stream, ctx.body_limit)),
                Some(btx),
            )
        };
        let (upgrade_tx, upgrade) = match websocket {
            Some(accept) => {
                let (utx, urx) = oneshot::channel();
                (Some(utx), Some(PendingUpgrade { on: urx, accept }))
            }
            None => (None, None),
        };
        let request = HttpServerRequest {
            method,
            url,
            headers,
            body,
            remote_address: ctx.peer_host.to_string(),
            remote_port: ctx.peer_port,
            peer_certificate: ctx.peer_certificate.clone(),
        };
        let (rtx, rrx) = oneshot::channel();
        let (dtx, drx) = oneshot::channel();
        if ctx.tx.send((request, rtx, drx, upgrade)).await.is_err() {
            let _ = write_error(&mut wr, 503).await;
            return;
        }
        if head.expect_continue && body_tx.is_some() && wr.write_all(CONTINUE).await.is_err() {
            return;
        }

        // ---- reading and answering, together ------------------------------
        let body_done = AtomicBool::new(body_tx.is_none());
        let answering = AtomicBool::new(false);
        let draining = *ctx.drain_rx.borrow();
        let keep_alive = head.keep_alive && !draining;
        let (answered, stopped_reading) = {
            let reader = feed(&mut rd, &mut buf, body_in, body_tx, &body_done, ctx.limits);
            let writer = respond(
                &mut wr, rrx, dtx, is_head, keep_alive, &body_done, &answering,
            );
            tokio::pin!(reader);
            tokio::pin!(writer);
            let mut stopped_reading = false;
            let answered = loop {
                tokio::select! {
                    answered = &mut writer => break answered,
                    ended = &mut reader, if !stopped_reading => match ended {
                        // The client hung up, or broke its body's framing,
                        // before the guest answered: drop the answer — the
                        // disconnect watcher sees exactly that — and close.
                        ReadEnd::Eof | ReadEnd::Error if !answering.load(Ordering::Acquire) => {
                            return;
                        }
                        // Mid-answer, or the buffer is full of pipelined bytes:
                        // finish writing, then close.
                        ReadEnd::Eof | ReadEnd::Error | ReadEnd::Full => stopped_reading = true,
                    },
                }
            };
            (answered, stopped_reading)
        };
        let Ok(answered) = answered else {
            return; // the write failed: the client is gone
        };
        match answered {
            Answered::Upgraded => {
                let Some(utx) = upgrade_tx else { return };
                let io = rd.unsplit(wr);
                let leftover = std::mem::take(&mut buf);
                let stream: UpgradedIo = Box::new(Prefixed::new(io, leftover).compat());
                let _ = utx.send(stream);
                return;
            }
            Answered::Close => {
                let _ = wr.shutdown().await;
                return;
            }
            Answered::KeepAlive => {
                if stopped_reading || !body_done.load(Ordering::Acquire) || *ctx.drain_rx.borrow() {
                    let _ = wr.shutdown().await;
                    return;
                }
            }
        }
    }
}

enum HeadOutcome {
    Head(RequestHead),
    Closed,
    Refused(RequestError),
}

/// Reads until a complete head is at the start of `buf`.
async fn read_head<R: AsyncRead + Unpin>(
    rd: &mut R,
    buf: &mut Vec<u8>,
    fields: &mut Vec<Field>,
    ctx: &mut ConnContext,
) -> HeadOutcome {
    let deadline = ctx.header_read.map(|d| tokio::time::Instant::now() + d);
    loop {
        if !buf.is_empty() {
            match parse_request(buf, &ctx.limits, fields) {
                Ok(Parsed::Complete(head)) => return HeadOutcome::Head(head),
                Ok(Parsed::Partial) => {}
                Err(e) => return HeadOutcome::Refused(e),
            }
        }
        // Idle between requests, a drain ends the connection now; mid-head,
        // the client gets to finish (the deadline still bounds it).
        if buf.is_empty() && *ctx.drain_rx.borrow() {
            return HeadOutcome::Closed;
        }
        let idle = buf.is_empty();
        let read = async {
            tokio::select! {
                n = read_more(rd, buf) => Some(n),
                _ = ctx.drain_rx.changed(), if idle => None,
            }
        };
        let outcome = match deadline {
            Some(at) => match tokio::time::timeout_at(at, read).await {
                Ok(outcome) => outcome,
                // D43: the policy working, not an error worth a log line.
                Err(_) => return HeadOutcome::Closed,
            },
            None => read.await,
        };
        match outcome {
            None => return HeadOutcome::Closed, // draining
            Some(Ok(0)) | Some(Err(_)) => return HeadOutcome::Closed,
            Some(Ok(_)) => {}
        }
    }
}

/// Where the request body stands.
enum BodyIn {
    Done,
    Length(u64),
    Chunked(Box<ChunkedDecoder>),
}

impl BodyIn {
    fn is_done(&self) -> bool {
        matches!(self, BodyIn::Done)
    }
}

/// Why the reading half stopped.
enum ReadEnd {
    Eof,
    Error,
    Full,
}

/// The reading half while a request is answered: forwards the body (if any)
/// as the guest pulls it, then keeps reading so a vanished client is noticed,
/// buffering any pipelined request up to the head limit.
async fn feed<R: AsyncRead + Unpin>(
    rd: &mut R,
    buf: &mut Vec<u8>,
    mut body: BodyIn,
    mut tx: Option<mpsc::Sender<Result<Vec<u8>, ProviderError>>>,
    body_done: &AtomicBool,
    limits: Limits,
) -> ReadEnd {
    loop {
        // Forward what the buffer already holds.
        while !body.is_done() {
            let Some(sender) = &tx else { break };
            let (chunk, consumed, finished) = match &mut body {
                BodyIn::Done => unreachable!(),
                BodyIn::Length(remaining) => {
                    let n = (*remaining)
                        .min(buf.len() as u64)
                        .min(MAX_BODY_CHUNK as u64) as usize;
                    *remaining -= n as u64;
                    (buf[..n].to_vec(), n, *remaining == 0)
                }
                BodyIn::Chunked(decoder) => {
                    let mut out = Vec::new();
                    let mut at = 0;
                    let mut end = false;
                    while out.len() < MAX_BODY_CHUNK {
                        match decoder.decode(&buf[at..]) {
                            Ok((used, Chunk::Data(d))) => {
                                out.extend_from_slice(d);
                                at += used;
                            }
                            Ok((used, Chunk::End)) => {
                                at += used;
                                end = true;
                                break;
                            }
                            Ok((used, Chunk::NeedMore)) => {
                                at += used;
                                break;
                            }
                            Err(e) => {
                                let _ = sender.send(Err(ProviderError::Other(e.to_string()))).await;
                                return ReadEnd::Error;
                            }
                        }
                    }
                    (out, at, end)
                }
            };
            buf.drain(..consumed);
            if !chunk.is_empty() && sender.send(Ok(chunk)).await.is_err() {
                // The guest dropped the body unread: stop forwarding. Its
                // remaining bytes are never read, so the connection will close.
                tx = None;
                break;
            }
            if finished {
                body = BodyIn::Done;
                tx = None; // ends the guest's stream
                body_done.store(true, Ordering::Release);
                break;
            }
            if consumed == 0 {
                break; // need more bytes
            }
        }
        // An abandoned body is read no further on its own behalf; what is
        // still read below is only how a disconnect is noticed.
        if buf.len() >= limits.max_head_bytes {
            return ReadEnd::Full;
        }
        match read_more(rd, buf).await {
            Ok(0) => {
                if let Some(sender) = &tx {
                    let _ = sender
                        .send(Err(ProviderError::Other(
                            "connection closed before the request body ended".into(),
                        )))
                        .await;
                }
                return ReadEnd::Eof;
            }
            Ok(_) => {}
            Err(_) => return ReadEnd::Error,
        }
    }
}

/// How a response ended.
enum Answered {
    KeepAlive,
    Close,
    Upgraded,
}

/// The writing half: waits for the guest's answer and writes it.
async fn respond<W: AsyncWrite + Unpin>(
    wr: &mut W,
    rrx: oneshot::Receiver<HttpServerResponse>,
    dtx: oneshot::Sender<()>,
    is_head: bool,
    keep_alive: bool,
    body_done: &AtomicBool,
    answering: &AtomicBool,
) -> std::io::Result<Answered> {
    let answer = rrx.await;
    // Past here the answer is written even if the client goes quiet: the
    // guest has already been told it was delivered.
    answering.store(true, Ordering::Release);
    let response = match answer {
        Ok(response) => response,
        Err(_) => {
            // The guest dropped the request without answering.
            let _ = dtx.send(());
            write_error(wr, 500).await?;
            return Ok(Answered::Close);
        }
    };
    // "Delivered", so a handler waiting on the disconnect signal learns the
    // request completed rather than that the client left.
    let _ = dtx.send(());

    let status = response.status;
    let switching = status == 101;
    // A connection whose request body is still unread cannot carry another
    // request: say so in this response.
    let keep_alive = keep_alive && (switching || body_done.load(Ordering::Acquire));
    let HttpServerResponse {
        headers,
        body,
        trailers,
        ..
    } = response;
    let head_length = if is_head {
        head_content_length(&headers)
    } else {
        None
    };
    let no_body = (100..200).contains(&status) || status == 204 || status == 304;
    let framing = if no_body {
        Framing::NoBody
    } else if is_head {
        match (&body, head_length) {
            (_, Some(n)) => Framing::Length(n),
            (HttpServerBody::Bytes(b), None) => Framing::Length(b.len() as u64),
            (HttpServerBody::Empty, None) => Framing::Length(0),
            (HttpServerBody::Stream(_), None) => Framing::NoBody,
        }
    } else {
        match (&body, &trailers) {
            (HttpServerBody::Empty, None) => Framing::Length(0),
            (HttpServerBody::Bytes(b), None) => Framing::Length(b.len() as u64),
            _ => Framing::Chunked,
        }
    };

    let mut out = Vec::with_capacity(256);
    let fields = headers.iter().map(|(n, v)| (n.as_bytes(), v.as_bytes()));
    if encode_response_head(&mut out, status, fields, framing, &date(), keep_alive).is_err() {
        // A header hyper would have refused makes the answer a 500, as before.
        write_error(wr, 500).await?;
        return Ok(Answered::Close);
    }
    if switching {
        wr.write_all(&out).await?;
        wr.flush().await?;
        return Ok(Answered::Upgraded);
    }
    if is_head || no_body {
        wr.write_all(&out).await?;
    } else {
        match body {
            HttpServerBody::Empty if framing != Framing::Chunked => wr.write_all(&out).await?,
            HttpServerBody::Bytes(bytes) if framing != Framing::Chunked => {
                if bytes.len() <= COALESCE_BODY {
                    out.extend_from_slice(&bytes);
                    wr.write_all(&out).await?;
                } else {
                    wr.write_all(&out).await?;
                    wr.write_all(&bytes).await?;
                }
            }
            body => {
                let mut stream: es_runtime_providers::ByteStream = match body {
                    HttpServerBody::Stream(s) => s,
                    HttpServerBody::Bytes(b) => Box::pin(futures_util::stream::iter([Ok(b)])),
                    HttpServerBody::Empty => Box::pin(futures_util::stream::empty()),
                };
                // The head goes out with the first chunks.
                let Ok(mut end) = write_chunks(wr, &mut stream, out).await? else {
                    // A failed body ends the response mid-stream; a client
                    // sees the truncation, not a well-formed lie.
                    return Ok(Answered::Close);
                };
                let trailers = match trailers {
                    Some(fut) => fut.await,
                    None => Vec::new(),
                };
                let mark = end.len();
                let pairs = trailers.iter().map(|(n, v)| (n.as_bytes(), v.as_bytes()));
                if encode_last_chunk(&mut end, pairs).is_err() {
                    end.truncate(mark);
                    let _ = encode_last_chunk(&mut end, std::iter::empty());
                }
                wr.write_all(&end).await?;
            }
        }
    }
    wr.flush().await?;
    Ok(if keep_alive {
        Answered::KeepAlive
    } else {
        Answered::Close
    })
}

/// The most framed body bytes gathered before they are written.
const GATHER: usize = 64 * 1024;

/// Writes `stream` as chunks after `pending` (a head, or nothing), gathering
/// every chunk already available into one write and writing whenever the
/// stream would have to wait — one syscall for a body produced at once, and no
/// delay for one produced slowly. Returns what is still unwritten when the
/// stream ends, for the caller to finish with the last chunk; `Err(())` when
/// the stream failed.
pub(crate) async fn write_chunks<W: AsyncWrite + Unpin>(
    wr: &mut W,
    stream: &mut es_runtime_providers::ByteStream,
    mut pending: Vec<u8>,
) -> std::io::Result<Result<Vec<u8>, ()>> {
    loop {
        let item = match stream.next().now_or_never() {
            Some(item) => item,
            None => {
                if !pending.is_empty() {
                    wr.write_all(&pending).await?;
                    pending.clear();
                }
                stream.next().await
            }
        };
        match item {
            None => return Ok(Ok(pending)),
            Some(Err(_)) => {
                if !pending.is_empty() {
                    wr.write_all(&pending).await?;
                }
                return Ok(Err(()));
            }
            Some(Ok(chunk)) if chunk.is_empty() => {}
            Some(Ok(chunk)) => {
                pending.extend_from_slice(chunk_header(chunk.len()).as_bytes());
                pending.extend_from_slice(&chunk);
                pending.extend_from_slice(CHUNK_END);
                if pending.len() >= GATHER {
                    wr.write_all(&pending).await?;
                    pending.clear();
                }
            }
        }
    }
}

/// A well-formed `Content-Length` a handler gave a HEAD response, which has no
/// body to state one (the rule `build_response` applies on HTTP/2).
fn head_content_length(headers: &[(String, String)]) -> Option<u64> {
    let (_, value) = headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case("content-length"))?;
    if value.is_empty() || value.len() > 20 || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

/// An empty response with `status`, and `Connection: close`.
async fn write_error<W: AsyncWrite + Unpin>(wr: &mut W, status: u16) -> std::io::Result<()> {
    let mut out = Vec::with_capacity(128);
    encode_response_head(
        &mut out,
        status,
        std::iter::empty(),
        Framing::Length(0),
        &date(),
        false,
    )
    .expect("a fixed head is valid");
    wr.write_all(&out).await?;
    wr.flush().await
}

/// Today's IMF-fixdate, formatted at most once a second per thread.
pub(crate) fn date() -> [u8; IMF_FIXDATE_LEN] {
    thread_local! {
        static CACHE: std::cell::Cell<(u64, [u8; IMF_FIXDATE_LEN])> =
            const { std::cell::Cell::new((u64::MAX, [0; IMF_FIXDATE_LEN])) };
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    CACHE.with(|cache| {
        let (second, formatted) = cache.get();
        if second == now {
            return formatted;
        }
        let formatted = imf_fixdate(now);
        cache.set((now, formatted));
        formatted
    })
}

/// The absolute URL the guest sees: the listener's scheme, the authority the
/// request named (validated, as on HTTP/2) or the bound address, and the path.
fn request_url(
    buf: &[u8],
    head: &RequestHead,
    origin: &str,
    host_cache: &mut Option<(Vec<u8>, Option<String>)>,
) -> Option<String> {
    let (scheme, fallback) = origin.split_once("://").unwrap_or(("http", origin));
    let target = std::str::from_utf8(head.target.of(buf)).ok()?;
    let host_bytes = head.host.of(buf);
    // The path as hyper's URI would give it: the target's path and query (an
    // absolute-form target's own path), "/" for authority-form, the fragment
    // dropped.
    let path = match head.target_form {
        TargetForm::Origin | TargetForm::Asterisk => target,
        TargetForm::Absolute => {
            let after_scheme = target.split_once("://").map_or("", |(_, rest)| rest);
            after_scheme.find('/').map_or("/", |i| &after_scheme[i..])
        }
        TargetForm::Authority => "/",
    };
    let path = path.split_once('#').map_or(path, |(p, _)| p);
    let path = if path.is_empty() { "/" } else { path };
    let host: &str = if host_bytes.is_empty() {
        fallback
    } else {
        match host_cache {
            Some((raw, validated)) if raw.as_slice() == host_bytes => validated.as_deref()?,
            _ => {
                let validated = std::str::from_utf8(host_bytes)
                    .ok()
                    .and_then(valid_authority);
                let entry = host_cache.insert((host_bytes.to_vec(), validated));
                entry.1.as_deref()?
            }
        }
    };
    let mut url = String::with_capacity(scheme.len() + 3 + host.len() + path.len());
    url.push_str(scheme);
    url.push_str("://");
    url.push_str(host);
    url.push_str(path);
    Some(url)
}

/// The request's headers, converted only if the guest reads them (D216): the
/// head's bytes are kept, and the pairs built from them on demand.
fn deferred_headers(buf: &[u8], head: &RequestHead, fields: &[Field]) -> HttpRequestHeaders {
    let bytes = buf[..head.len].to_vec();
    let spans: Vec<Field> = fields.to_vec();
    HttpRequestHeaders::deferred(move || {
        spans
            .iter()
            .map(|f| {
                let name = String::from_utf8_lossy(f.name.of(&bytes)).to_ascii_lowercase();
                // Visible ASCII only, as `HeaderValue::to_str` gave it: a value
                // with obs-text reads as empty.
                let value = f.value.of(&bytes);
                let value = if value
                    .iter()
                    .all(|&b| b == b'\t' || (0x20..0x7f).contains(&b))
                {
                    String::from_utf8_lossy(value).into_owned()
                } else {
                    String::new()
                };
                (name, value)
            })
            .collect()
    })
}

/// `Sec-WebSocket-Accept` for a WebSocket handshake (RFC 6455 §4.2.1): a `GET`
/// asking to upgrade to `websocket`, with a key.
fn websocket_accept(
    buf: &[u8],
    head: &RequestHead,
    fields: &[Field],
    method: &str,
) -> Option<String> {
    if !head.upgrade || method != "GET" {
        return None;
    }
    let value = |name: &[u8]| {
        fields
            .iter()
            .find(|f| f.name.of(buf).eq_ignore_ascii_case(name))
            .map(|f| f.value.of(buf))
    };
    if !value(b"upgrade")?.eq_ignore_ascii_case(b"websocket") {
        return None;
    }
    let key = value(b"sec-websocket-key")?;
    Some(tokio_tungstenite::tungstenite::handshake::derive_accept_key(key))
}
