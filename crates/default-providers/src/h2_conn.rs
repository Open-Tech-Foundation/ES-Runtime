//! One HTTP/2 connection, served by `es-runtime-http`'s connection state
//! machine (DECISIONS D217, phase 3) — the tokio half of the HTTP/2 server.
//!
//! One task, one loop, over everything that can happen on the connection:
//! bytes from the socket, a handler's answer arriving, the next chunk of a
//! streamed answer, the guest consuming a request body, the drain signal and
//! the keep-alive timer. Each request reaches the guest the way an HTTP/1.1
//! one does — a [`Pending`] on the server's channel — so nothing above the
//! connection knows which version carried it.
//!
//! Flow control is tied to the guest at both ends. A request body's window is
//! given back to the client only as the guest's stream yields each chunk, so
//! an upload never gets more than a window ahead of the handler. A streamed
//! answer's next chunk is pulled only once the last one has gone out under the
//! client's windows, so a client that stops opening its window cannot make the
//! server buffer the handler's output without bound.
//!
//! Rapid Reset (CVE-2023-44487) is counted where its cost is: a stream the
//! client resets stays on the connection's budget until the guest's handler
//! has actually let go of it — answered, or dropped the request — not merely
//! until the reset was read.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use es_runtime_http::fields::{is_field_value, is_token};
use es_runtime_http::h2::ErrorCode;
use es_runtime_http::h2::conn::{Config, Connection, Event};
use es_runtime_providers::{
    ByteStream, HttpRequestHeaders, HttpServerBody, HttpServerRequest, HttpServerResponse,
    PeerCertificate, ProviderError,
};
use futures_util::future::BoxFuture;
use futures_util::stream::{FuturesUnordered, StreamExt};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot, watch};

use crate::body_deadline::{BodyDeadline, BodyLimit};
use crate::system_http::{Pending, valid_authority};

/// What a connection needs from the server it belongs to.
pub(crate) struct ConnContext {
    pub(crate) tx: mpsc::Sender<Pending>,
    pub(crate) origin: Arc<str>,
    pub(crate) peer_host: Arc<str>,
    pub(crate) peer_port: u16,
    pub(crate) peer_certificate: Option<Arc<PeerCertificate>>,
    pub(crate) drain_rx: watch::Receiver<bool>,
    /// PING a connection that has been quiet this long, and drop it if
    /// nothing comes back within the same interval (D43).
    pub(crate) keep_alive: Option<Duration>,
    pub(crate) body_limit: Option<BodyLimit>,
    pub(crate) config: Config,
}

type Trailers = Vec<(String, String)>;
type RequestBodyTx = mpsc::UnboundedSender<Result<Vec<u8>, ProviderError>>;

struct StreamState {
    /// The request body's sender, while the client is still sending it.
    body_tx: Option<RequestBodyTx>,
    /// The disconnect watch's half: dropped unsent when the stream is reset
    /// before its answer, which is what `request.signal` reports.
    delivered: Option<oneshot::Sender<()>>,
    /// The request was `HEAD`: the answer has a head and no body.
    head: bool,
    /// Answer bytes still to send once its head is out.
    pending: Vec<u8>,
    /// The body ends with `pending`.
    end: bool,
    /// A streamed answer, parked here while its last chunk waits for window.
    body: Option<ByteStream>,
    /// Trailers to send once the body is out.
    trailers: Option<BoxFuture<'static, Trailers>>,
    /// The answer has gone out whole (or was abandoned).
    answered: bool,
}

enum Wake {
    Read(std::io::Result<usize>),
    Answer(u32, Result<HttpServerResponse, oneshot::error::RecvError>),
    Chunk(u32, Option<Result<Vec<u8>, ProviderError>>, ByteStream),
    Trailers(u32, Trailers),
    Released(u32, usize),
    Drain,
    KeepAlive,
}

/// The per-connection state the loop's handlers share.
struct Driver {
    conn: Connection,
    streams: HashMap<u32, StreamState>,
    /// Handlers' answers, body chunks and trailers on their way.
    wakes: FuturesUnordered<BoxFuture<'static, Wake>>,
    release_tx: mpsc::UnboundedSender<(u32, usize)>,
}

impl Drop for Driver {
    /// However the connection ends, a request body still arriving ends with
    /// an error: a sender merely dropped would read to the guest as a body
    /// that ended, and a truncated upload must never pass for a whole one.
    fn drop(&mut self) {
        for state in self.streams.values_mut() {
            if let Some(tx) = state.body_tx.take() {
                let _ = tx.send(Err(ProviderError::Other(
                    "connection closed before the request body ended".into(),
                )));
            }
        }
    }
}

/// Serves HTTP/2 on `io` until the connection ends. `buf` holds whatever was
/// read before the protocol was chosen (the preface, on cleartext).
pub(crate) async fn serve<S>(mut io: S, mut buf: Vec<u8>, mut ctx: ConnContext)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (release_tx, mut release_rx) = mpsc::unbounded_channel::<(u32, usize)>();
    let mut d = Driver {
        conn: Connection::server(ctx.config),
        streams: HashMap::new(),
        wakes: FuturesUnordered::new(),
        release_tx,
    };
    let mut draining = *ctx.drain_rx.borrow();
    if draining {
        d.conn.go_away(ErrorCode::NoError);
    }
    let mut ping_sent = false;
    let mut quiet_since = tokio::time::Instant::now();
    // What arrived with the protocol choice is handled first, as if just read.
    let mut wake = (!buf.is_empty()).then(|| Wake::Read(Ok(buf.len())));

    loop {
        if let Some(w) = wake.take() {
            match w {
                Wake::Read(Ok(0) | Err(_)) => return,
                Wake::Read(Ok(_)) => {
                    quiet_since = tokio::time::Instant::now();
                    ping_sent = false;
                    match d.conn.receive(&buf) {
                        Ok(used) => {
                            buf.drain(..used);
                        }
                        Err(e) => {
                            tracing::debug!(target: "runtime::http", error = %e, "connection ended with an error");
                            let _ = write_out(&mut io, &mut d.conn).await;
                            return;
                        }
                    }
                }
                Wake::Answer(stream, answer) => d.answer(stream, answer),
                Wake::Chunk(stream, item, body) => d.chunk(stream, item, body),
                Wake::Trailers(stream, trailers) => d.send_trailers(stream, &trailers),
                Wake::Released(stream, n) => d.conn.release(stream, n),
                Wake::Drain => {
                    draining = true;
                    d.conn.go_away(ErrorCode::NoError);
                }
                Wake::KeepAlive => {
                    if ping_sent {
                        // A whole interval and not a byte back: the peer is gone.
                        return;
                    }
                    d.conn.ping(*b"esrun-ka");
                    ping_sent = true;
                    quiet_since = tokio::time::Instant::now();
                }
            }
            while let Some(event) = d.conn.poll_event() {
                d.event(event, &mut ctx).await;
            }
        }
        if write_out(&mut io, &mut d.conn).await.is_err() || d.conn.is_failed() {
            return;
        }
        if draining && d.conn.is_idle_after_goaway() && d.streams.is_empty() {
            let _ = io.shutdown().await;
            return;
        }
        let keep_alive_at = ctx.keep_alive.map(|interval| quiet_since + interval);
        wake = Some(tokio::select! {
            n = read_more(&mut io, &mut buf) => Wake::Read(n),
            Some(w) = d.wakes.next() => w,
            Some((stream, n)) = release_rx.recv() => Wake::Released(stream, n),
            _ = ctx.drain_rx.changed(), if !draining => Wake::Drain,
            () = sleep_until(keep_alive_at), if keep_alive_at.is_some() => Wake::KeepAlive,
        });
    }
}

async fn sleep_until(at: Option<tokio::time::Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

async fn read_more<S: AsyncRead + Unpin>(io: &mut S, buf: &mut Vec<u8>) -> std::io::Result<usize> {
    buf.reserve(16 * 1024);
    io.read_buf(buf).await
}

async fn write_out<S: AsyncWrite + Unpin>(
    io: &mut S,
    conn: &mut Connection,
) -> std::io::Result<()> {
    let n = conn.output().len();
    if n > 0 {
        io.write_all(conn.output()).await?;
        conn.consume_output(n);
        io.flush().await?;
    }
    Ok(())
}

/// The URL the guest sees: the listener's scheme, the authority the request
/// named — `Host` if it sent one, else `:authority`, validated as on HTTP/1.1
/// — or the bound address, and the path.
fn request_url(origin: &str, named: Option<&[u8]>, path: Option<&[u8]>) -> Option<String> {
    let (scheme, fallback) = origin.split_once("://").unwrap_or(("http", origin));
    let host = match named.filter(|a| !a.is_empty()) {
        Some(a) => valid_authority(std::str::from_utf8(a).ok()?)?,
        None => fallback.to_owned(),
    };
    let path = std::str::from_utf8(path.unwrap_or(b"/")).ok()?;
    Some(format!("{scheme}://{host}{path}"))
}

/// The answer's head as HTTP/2 sends it: lowercase names, without the fields
/// that describe an HTTP/1.1 connection (RFC 9113 §8.2.2), with the length the
/// body actually has, and a `date`. `None` when a field could not be sent at
/// all, which makes the answer a 500 (as on HTTP/1.1).
fn head_fields(
    status: u16,
    headers: Vec<(String, String)>,
    body_len: Option<u64>,
    head: bool,
) -> Option<Vec<(Vec<u8>, Vec<u8>)>> {
    let mut out = Vec::with_capacity(headers.len() + 3);
    out.push((b":status".to_vec(), status.to_string().into_bytes()));
    let mut has_date = false;
    let mut head_length = None;
    for (name, value) in headers {
        let name = name.to_ascii_lowercase();
        if !is_token(name.as_bytes()) || !is_field_value(value.as_bytes()) {
            return None;
        }
        match name.as_str() {
            "connection" | "keep-alive" | "proxy-connection" | "transfer-encoding" | "upgrade" => {
                continue;
            }
            "content-length" => {
                // A HEAD's answer has no body to measure: the length a GET
                // would have had is the guest's to state (RFC 9110 §9.3.2).
                if head
                    && head_length.is_none()
                    && !value.is_empty()
                    && value.bytes().all(|b| b.is_ascii_digit())
                    && value.parse::<u64>().is_ok()
                {
                    head_length = Some(value);
                }
                continue;
            }
            "date" => has_date = true,
            _ => {}
        }
        out.push((name.into_bytes(), value.into_bytes()));
    }
    if let Some(v) = head_length {
        out.push((b"content-length".to_vec(), v.into_bytes()));
    } else if let Some(n) = body_len {
        out.push((b"content-length".to_vec(), n.to_string().into_bytes()));
    }
    if !has_date {
        out.push((b"date".to_vec(), crate::h1_conn::date().to_vec()));
    }
    Some(out)
}

fn pull(stream: u32, mut body: ByteStream) -> BoxFuture<'static, Wake> {
    Box::pin(async move {
        let item = body.next().await;
        Wake::Chunk(stream, item, body)
    })
}

impl Driver {
    async fn event(&mut self, event: Event, ctx: &mut ConnContext) {
        match event {
            Event::Headers {
                stream,
                pseudo,
                fields,
                end_stream,
            } => self.request(stream, pseudo, fields, end_stream, ctx).await,
            Event::Data {
                stream,
                data,
                end_stream,
            } => {
                let n = data.len();
                let tx = self.streams.get(&stream).and_then(|s| s.body_tx.as_ref());
                // Nobody left to read it (answered, or the guest dropped the
                // body): its window comes back now.
                if n > 0 && tx.is_none_or(|tx| tx.send(Ok(data)).is_err()) {
                    self.conn.release(stream, n);
                }
                if end_stream {
                    self.body_ended(stream);
                }
            }
            Event::Trailers { stream, .. } => self.body_ended(stream),
            Event::Reset { stream, .. } => {
                if let Some(mut state) = self.streams.remove(&stream) {
                    if let Some(tx) = state.body_tx.take() {
                        let _ = tx.send(Err(ProviderError::Other(
                            "the client reset the stream before the request body ended".into(),
                        )));
                    }
                    // Dropped unsent: the guest's request.signal fires.
                    drop(state.delivered.take());
                    if state.answered {
                        self.conn.done_with(stream);
                    }
                    // Otherwise the handler is still on it: the stream counts
                    // toward the reset budget until its answer resolves.
                }
            }
            Event::SendCapacity { stream } => {
                let ids: Vec<u32> = if stream == 0 {
                    self.streams.keys().copied().collect()
                } else {
                    vec![stream]
                };
                for id in ids {
                    self.flush(id);
                }
            }
            Event::GoAway { .. } => {}
        }
    }

    async fn request(
        &mut self,
        stream: u32,
        pseudo: es_runtime_http::h2::headers::Pseudo,
        fields: Vec<(Vec<u8>, Vec<u8>)>,
        end_stream: bool,
        ctx: &mut ConnContext,
    ) {
        let host = fields
            .iter()
            .find(|(n, _)| n == b"host")
            .map(|(_, v)| v.as_slice());
        let named = host.or(pseudo.authority.as_deref());
        let Some(url) = request_url(&ctx.origin, named, pseudo.path.as_deref()) else {
            // An authority that is not one never reaches the guest: there is
            // no URL both faithful to it and safe to route on.
            self.refuse(stream, b"400", end_stream);
            return;
        };
        let method =
            String::from_utf8_lossy(pseudo.method.as_deref().unwrap_or(b"GET")).into_owned();
        let head = method == "HEAD";
        // Converted only if the guest reads them (D216).
        let headers = HttpRequestHeaders::deferred(move || {
            fields
                .into_iter()
                .map(|(n, v)| {
                    // Visible ASCII only, as on HTTP/1.1: anything else reads
                    // as an empty value rather than as mangled text.
                    let value = if v.iter().all(|&b| b == b'\t' || (0x20..0x7f).contains(&b)) {
                        String::from_utf8(v).unwrap_or_default()
                    } else {
                        String::new()
                    };
                    (String::from_utf8(n).unwrap_or_default(), value)
                })
                .collect()
        });
        let (body, body_tx) = if end_stream {
            (HttpServerBody::Empty, None)
        } else {
            let (tx, rx) = mpsc::unbounded_channel::<Result<Vec<u8>, ProviderError>>();
            let releaser = self.release_tx.clone();
            // Each chunk the guest takes gives its bytes of window back.
            let chunks = futures_util::stream::unfold(rx, move |mut rx| {
                let releaser = releaser.clone();
                async move {
                    let item = rx.recv().await?;
                    if let Ok(chunk) = &item {
                        let _ = releaser.send((stream, chunk.len()));
                    }
                    Some((item, rx))
                }
            });
            (
                HttpServerBody::Stream(BodyDeadline::wrap(Box::pin(chunks), ctx.body_limit)),
                Some(tx),
            )
        };
        let request = HttpServerRequest {
            method,
            url,
            headers,
            body,
            // Every stream reports the connection's peer: they are one
            // connection.
            remote_address: ctx.peer_host.to_string(),
            remote_port: ctx.peer_port,
            peer_certificate: ctx.peer_certificate.clone(),
        };
        let (rtx, rrx) = oneshot::channel();
        let (dtx, drx) = oneshot::channel();
        if ctx.tx.send((request, rtx, drx, None)).await.is_err() {
            // The server closed: nobody will answer.
            self.refuse(stream, b"503", end_stream);
            return;
        }
        self.streams.insert(
            stream,
            StreamState {
                body_tx,
                delivered: Some(dtx),
                head,
                pending: Vec::new(),
                end: false,
                body: None,
                trailers: None,
                answered: false,
            },
        );
        self.wakes
            .push(Box::pin(async move { Wake::Answer(stream, rrx.await) }));
    }

    /// Answers `stream` with a bare status, without the guest.
    fn refuse(&mut self, stream: u32, status: &[u8], body_ended: bool) {
        let date = crate::h1_conn::date();
        let _ = self.conn.send_headers(
            stream,
            [(&b":status"[..], status), (b"date", &date[..])],
            true,
        );
        self.conn.done_with(stream);
        if !body_ended {
            // Whatever body the client is still sending is not wanted.
            self.conn.reset(stream, ErrorCode::NoError);
        }
    }

    fn body_ended(&mut self, stream: u32) {
        if let Some(state) = self.streams.get_mut(&stream) {
            state.body_tx = None;
            if state.answered {
                self.streams.remove(&stream);
            }
        }
    }

    fn answer(
        &mut self,
        stream: u32,
        answer: Result<HttpServerResponse, oneshot::error::RecvError>,
    ) {
        let Some(state) = self.streams.get_mut(&stream) else {
            // Reset before the answer: the handler has let go of it now.
            self.conn.done_with(stream);
            return;
        };
        // "Delivered", so a handler awaiting the disconnect signal learns the
        // request completed rather than that the client left.
        if let Some(tx) = state.delivered.take() {
            let _ = tx.send(());
        }
        let HttpServerResponse {
            status,
            headers,
            body,
            trailers,
        } = answer.unwrap_or_else(|_| HttpServerResponse {
            // The guest dropped the request without answering.
            status: 500,
            headers: Vec::new(),
            body: HttpServerBody::Empty,
            trailers: None,
        });
        // A final answer is 2xx and up (to 999, as a guest may invent); a
        // 1xx — 101 included, which HTTP/2 has no use for — is not one.
        let status = if (200..=999).contains(&status) {
            status
        } else {
            500
        };
        let head = state.head;
        let no_body = head || status == 204 || status == 304;
        let (body_len, body) = match body {
            _ if no_body => (None, HttpServerBody::Empty),
            HttpServerBody::Bytes(b) => (Some(b.len() as u64), HttpServerBody::Bytes(b)),
            HttpServerBody::Empty => (Some(0), HttpServerBody::Empty),
            streamed @ HttpServerBody::Stream(_) => (None, streamed),
        };
        let trailers = if no_body { None } else { trailers };
        let Some(fields) = head_fields(status, headers, body_len, head) else {
            let _ = self
                .conn
                .send_headers(stream, [(&b":status"[..], &b"500"[..])], true);
            self.finish(stream);
            return;
        };
        let empty = matches!(body, HttpServerBody::Empty) && trailers.is_none();
        let pairs = fields.iter().map(|(n, v)| (n.as_slice(), v.as_slice()));
        if self.conn.send_headers(stream, pairs, empty).is_err() || empty {
            self.finish(stream);
            return;
        }
        let state = self.streams.get_mut(&stream).expect("checked above");
        state.trailers = trailers;
        match body {
            HttpServerBody::Bytes(data) => {
                state.pending = data;
                state.end = true;
            }
            HttpServerBody::Empty => state.end = true,
            HttpServerBody::Stream(body) => {
                self.wakes.push(pull(stream, body));
                return;
            }
        }
        self.flush(stream);
    }

    fn chunk(
        &mut self,
        stream: u32,
        item: Option<Result<Vec<u8>, ProviderError>>,
        body: ByteStream,
    ) {
        let Some(state) = self.streams.get_mut(&stream) else {
            return; // reset meanwhile: the body goes with the stream
        };
        if state.answered {
            return;
        }
        match item {
            Some(Ok(chunk)) => {
                state.pending = chunk;
                state.body = Some(body);
            }
            None => state.end = true,
            Some(Err(_)) => {
                // A failed body ends the answer mid-stream: the client sees a
                // reset, never a short body that looks whole.
                self.streams.remove(&stream);
                self.conn.reset(stream, ErrorCode::InternalError);
                self.conn.done_with(stream);
                return;
            }
        }
        self.flush(stream);
    }

    /// Sends what fits of `stream`'s pending bytes; once they are out, pulls a
    /// streamed answer's next chunk, or ends the body, or awaits the trailers.
    fn flush(&mut self, stream: u32) {
        let Some(state) = self.streams.get_mut(&stream) else {
            return;
        };
        if state.answered || (state.pending.is_empty() && state.body.is_none() && !state.end) {
            return; // nothing to send yet: a chunk is being pulled
        }
        let ends_here = state.end && state.trailers.is_none();
        match self.conn.send_data(stream, &state.pending, ends_here) {
            Ok(n) => {
                state.pending.drain(..n);
            }
            Err(_) => {
                self.finish(stream);
                return;
            }
        }
        if !state.pending.is_empty() {
            return; // waits for window: Event::SendCapacity comes back here
        }
        if let Some(body) = state.body.take() {
            self.wakes.push(pull(stream, body));
        } else if state.end {
            match state.trailers.take() {
                Some(trailers) => {
                    state.end = false;
                    self.wakes.push(Box::pin(
                        async move { Wake::Trailers(stream, trailers.await) },
                    ));
                }
                None => self.finish(stream),
            }
        }
    }

    fn send_trailers(&mut self, stream: u32, trailers: &Trailers) {
        if !self.streams.contains_key(&stream) {
            return;
        }
        let fields: Vec<(Vec<u8>, Vec<u8>)> = trailers
            .iter()
            .map(|(n, v)| (n.to_ascii_lowercase().into_bytes(), v.as_bytes().to_vec()))
            .filter(|(n, v)| is_token(n) && is_field_value(v))
            .collect();
        // No trailers after all: the body ends with an empty DATA frame, not
        // an empty HEADERS frame that would read as a trailer section.
        let _ = if fields.is_empty() {
            self.conn.send_data(stream, &[], true).map(|_| ())
        } else {
            self.conn.send_headers(
                stream,
                fields.iter().map(|(n, v)| (n.as_slice(), v.as_slice())),
                true,
            )
        };
        self.finish(stream);
    }

    /// The answer is out, or abandoned: the stream leaves the reset budget,
    /// and its state goes once the request body has ended too.
    fn finish(&mut self, stream: u32) {
        self.conn.done_with(stream);
        let Some(state) = self.streams.get_mut(&stream) else {
            return;
        };
        state.answered = true;
        state.pending = Vec::new();
        state.body = None;
        state.trailers = None;
        match &state.body_tx {
            None => {
                self.streams.remove(&stream);
            }
            // Answered while the client is still sending a body the guest no
            // longer reads: stop it (RFC 9113 §8.1), and its window with it.
            Some(tx) if tx.is_closed() => {
                self.streams.remove(&stream);
                self.conn.reset(stream, ErrorCode::NoError);
            }
            Some(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::head_fields;

    fn head(
        headers: &[(&str, &str)],
        body_len: Option<u64>,
        head: bool,
    ) -> Option<Vec<(String, String)>> {
        let headers = headers
            .iter()
            .map(|(n, v)| (n.to_string(), v.to_string()))
            .collect();
        head_fields(201, headers, body_len, head).map(|fields| {
            fields
                .into_iter()
                .map(|(n, v)| (String::from_utf8(n).unwrap(), String::from_utf8(v).unwrap()))
                .collect()
        })
    }

    #[test]
    fn an_answer_keeps_its_headers_in_order_with_repeats_lowercased() {
        let fields = head(
            &[
                ("x-a", "1"),
                ("set-cookie", "a=1"),
                ("set-cookie", "b=2"),
                ("X-B", "2"),
            ],
            Some(2),
            false,
        )
        .unwrap();
        let names: Vec<&str> = fields.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            [
                ":status",
                "x-a",
                "set-cookie",
                "set-cookie",
                "x-b",
                "content-length",
                "date"
            ]
        );
        assert_eq!(fields[0].1, "201");
        assert_eq!(fields[2].1, "a=1");
        assert_eq!(fields[3].1, "b=2");
    }

    #[test]
    fn framing_and_connection_fields_from_the_guest_are_not_sent_as_given() {
        // Matched in any case: the body decides the length, and HTTP/2 has no
        // use for fields that describe an HTTP/1.1 connection (§8.2.2).
        let fields = head(
            &[
                ("Transfer-Encoding", "chunked"),
                ("Content-Length", "99"),
                ("connection", "close"),
                ("keep-alive", "5"),
                ("upgrade", "websocket"),
            ],
            Some(2),
            false,
        )
        .unwrap();
        let length: Vec<_> = fields
            .iter()
            .filter(|(n, _)| n == "content-length")
            .collect();
        assert_eq!(length, [&("content-length".to_string(), "2".to_string())]);
        assert!(!fields.iter().any(|(n, _)| {
            ["transfer-encoding", "connection", "keep-alive", "upgrade"].contains(&n.as_str())
        }));
        // A streamed body states no length at all.
        let fields = head(&[("content-length", "99")], None, false).unwrap();
        assert!(!fields.iter().any(|(n, _)| n == "content-length"));
        // A HEAD answer keeps a well-formed length, since it has no body to
        // give one; a malformed one is dropped.
        let fields = head(&[("content-length", "99")], None, true).unwrap();
        assert!(fields.contains(&("content-length".into(), "99".into())));
        let fields = head(&[("content-length", "9x")], None, true).unwrap();
        assert!(!fields.iter().any(|(n, _)| n == "content-length"));
    }

    #[test]
    fn a_field_that_cannot_be_sent_makes_the_answer_a_500() {
        for (name, value) in [
            ("bad name", "v"),
            ("x-ok", "line\nbreak"),
            ("x-ok", "nul\0"),
        ] {
            assert!(
                head(&[("x-a", "1"), (name, value)], Some(0), false).is_none(),
                "{name:?}: {value:?}"
            );
        }
    }

    #[test]
    fn a_date_is_added_unless_the_guest_set_one() {
        let fields = head(&[], Some(0), false).unwrap();
        assert!(
            fields
                .iter()
                .any(|(n, v)| n == "date" && v.ends_with(" GMT"))
        );
        let fields = head(&[("date", "Sun, 06 Nov 1994 08:49:37 GMT")], Some(0), false).unwrap();
        let dates: Vec<_> = fields.iter().filter(|(n, _)| n == "date").collect();
        assert_eq!(dates.len(), 1);
        assert_eq!(dates[0].1, "Sun, 06 Nov 1994 08:49:37 GMT");
    }
}

/// The driver serving over an in-memory pipe, with our own client state
/// machine as the peer and the test as the guest: what h2spec cannot see —
/// flow control tied to the guest's reading, and Rapid Reset counted until
/// the guest lets go.
#[cfg(test)]
mod driver_tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use es_runtime_http::h2::ErrorCode;
    use es_runtime_http::h2::conn::{Config, Connection, Event};
    use es_runtime_providers::{HttpServerBody, HttpServerResponse};
    use futures_util::StreamExt;
    use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
    use tokio::sync::{mpsc, watch};

    use super::{ConnContext, serve};
    use crate::system_http::Pending;

    struct Peer {
        client: Connection,
        io: DuplexStream,
        /// The server closed the pipe.
        closed: bool,
        goaway: Option<ErrorCode>,
    }

    impl Peer {
        /// Writes what the client owes, then reads what the server says until
        /// it has been quiet for a moment.
        async fn exchange(&mut self) -> Vec<Event> {
            let out = self.client.output().to_vec();
            self.client.consume_output(out.len());
            if !out.is_empty() && self.io.write_all(&out).await.is_err() {
                self.closed = true;
            }
            let mut events = Vec::new();
            let mut buf = Vec::new();
            loop {
                let mut chunk = [0u8; 64 * 1024];
                match tokio::time::timeout(Duration::from_millis(50), self.io.read(&mut chunk))
                    .await
                {
                    Ok(Ok(0) | Err(_)) => {
                        self.closed = true;
                        break;
                    }
                    Ok(Ok(n)) => {
                        buf.extend_from_slice(&chunk[..n]);
                        let used = self.client.receive(&buf).expect("the server speaks HTTP/2");
                        buf.drain(..used);
                        while let Some(e) = self.client.poll_event() {
                            if let Event::GoAway { code, .. } = e {
                                self.goaway = Some(code);
                            }
                            events.push(e);
                        }
                        // What the client owes back (ACKs, window updates).
                        let out = self.client.output().to_vec();
                        self.client.consume_output(out.len());
                        if !out.is_empty() && self.io.write_all(&out).await.is_err() {
                            self.closed = true;
                        }
                    }
                    Err(_) => break,
                }
            }
            events
        }
    }

    fn start(
        server: Config,
        client: Config,
    ) -> (Peer, mpsc::Receiver<Pending>, watch::Sender<bool>) {
        let (a, b) = tokio::io::duplex(1 << 20);
        let (tx, rx) = mpsc::channel(1024);
        let (drain, drain_rx) = watch::channel(false);
        let ctx = ConnContext {
            tx,
            origin: Arc::from("http://127.0.0.1:1"),
            peer_host: Arc::from("127.0.0.1"),
            peer_port: 2,
            peer_certificate: None,
            drain_rx,
            keep_alive: None,
            body_limit: None,
            config: server,
        };
        tokio::spawn(serve(b, Vec::new(), ctx));
        let peer = Peer {
            client: Connection::client(client),
            io: a,
            closed: false,
            goaway: None,
        };
        (peer, rx, drain)
    }

    fn get(path: &[u8]) -> Vec<(&[u8], &[u8])> {
        vec![
            (b":method", b"GET"),
            (b":scheme", b"http"),
            (b":authority", b"example.test"),
            (b":path", path),
        ]
    }

    fn ok(body: HttpServerBody) -> HttpServerResponse {
        HttpServerResponse {
            status: 200,
            headers: vec![("content-type".into(), "text/plain".into())],
            body,
            trailers: None,
        }
    }

    #[tokio::test]
    async fn a_request_reaches_the_guest_and_its_answer_the_client() {
        let (mut peer, mut rx, _drain) = start(Config::default(), Config::default());
        let stream = peer.client.open_stream(get(b"/a?b=1"), true).unwrap();
        peer.exchange().await;
        let (request, answer, _delivered, upgrade) = rx.recv().await.unwrap();
        assert_eq!(request.method, "GET");
        assert_eq!(request.url, "http://example.test/a?b=1");
        assert!(upgrade.is_none(), "HTTP/2 has no upgrade");
        answer
            .send(ok(HttpServerBody::Bytes(b"hello".to_vec())))
            .ok()
            .unwrap();
        let events = peer.exchange().await;
        assert!(
            matches!(&events[0], Event::Headers { stream: s, pseudo, fields, end_stream: false }
            if *s == stream && pseudo.status == Some(200)
                && fields.iter().any(|(n, v)| n == b"content-length" && v == b"5")
                && fields.iter().any(|(n, _)| n == b"date")),
            "{events:?}"
        );
        assert_eq!(
            events[1],
            Event::Data {
                stream,
                data: b"hello".to_vec(),
                end_stream: true
            }
        );
    }

    /// An upload gets at most a stream window ahead of a guest that does not
    /// read, and moves on as soon as it does.
    #[tokio::test]
    async fn a_request_body_moves_only_as_the_guest_reads_it() {
        let window = 64 * 1024;
        let server = Config {
            stream_window: window,
            connection_window: 1 << 20,
            ..Config::default()
        };
        let (mut peer, mut rx, _drain) = start(server, Config::default());
        let mut post = get(b"/upload");
        post[0].1 = b"POST";
        let stream = peer.client.open_stream(post, false).unwrap();
        peer.exchange().await;
        let body = vec![1u8; 1 << 20];
        let mut sent = peer.client.send_data(stream, &body, true).unwrap();
        peer.exchange().await;
        // The SETTINGS above bound the stream; nothing more fits until read.
        sent += peer.client.send_data(stream, &body[sent..], true).unwrap();
        peer.exchange().await;
        assert_eq!(sent, window as usize, "the upload outran the window");

        let (request, answer, _, _) = rx.recv().await.unwrap();
        let HttpServerBody::Stream(mut chunks) = request.body else {
            panic!("a streamed body");
        };
        let mut read = 0;
        while read < body.len() {
            // The guest reads what has arrived, which gives window back.
            while let Ok(Some(chunk)) =
                tokio::time::timeout(Duration::from_millis(20), chunks.next()).await
            {
                read += chunk.unwrap().len();
            }
            peer.exchange().await;
            if sent < body.len() {
                sent += peer.client.send_data(stream, &body[sent..], true).unwrap();
            }
            peer.exchange().await;
            assert!(sent - read <= window as usize, "{sent} sent, {read} read");
        }
        assert_eq!(read, body.len());
        answer.send(ok(HttpServerBody::Empty)).ok().unwrap();
        let events = peer.exchange().await;
        assert!(
            matches!(
                &events[..],
                [Event::Headers {
                    end_stream: true,
                    ..
                }]
            ),
            "{events:?}"
        );
    }

    /// A streamed answer is pulled from the guest only as the client's window
    /// lets it out: a client that never opens its window costs one chunk.
    #[tokio::test]
    async fn a_streamed_answer_is_pulled_only_as_the_window_allows() {
        let client = Config {
            stream_window: 65_535,
            connection_window: 65_535,
            ..Config::default()
        };
        let (mut peer, mut rx, _drain) = start(Config::default(), client);
        let stream = peer.client.open_stream(get(b"/stream"), true).unwrap();
        peer.exchange().await;
        let (_, answer, _, _) = rx.recv().await.unwrap();
        let pulled = Arc::new(AtomicUsize::new(0));
        let counter = pulled.clone();
        let chunk = vec![9u8; 16 * 1024];
        let endless = futures_util::stream::repeat_with(move || {
            counter.fetch_add(chunk.len(), Ordering::SeqCst);
            Ok(chunk.clone())
        });
        answer
            .send(ok(HttpServerBody::Stream(Box::pin(endless))))
            .ok()
            .unwrap();
        let mut received = 0;
        for e in peer.exchange().await {
            if let Event::Data { data, .. } = e {
                received += data.len();
            }
        }
        assert_eq!(received, 65_535, "the client's whole window, no more");
        let at_rest = pulled.load(Ordering::SeqCst);
        assert!(
            at_rest <= 65_535 + 16 * 1024,
            "pulled {at_rest} for a 65,535-byte window"
        );
        // Opening the window lets more out.
        peer.client.release(stream, received);
        let more: usize = peer
            .exchange()
            .await
            .into_iter()
            .map(|e| {
                if let Event::Data { data, .. } = e {
                    data.len()
                } else {
                    0
                }
            })
            .sum();
        assert!(more > 0);
    }

    /// CVE-2023-44487: requests opened and reset while the guest is still on
    /// them end the connection past the budget...
    #[tokio::test]
    async fn rapid_reset_against_a_busy_guest_ends_the_connection() {
        let server = Config {
            max_pending_resets: 10,
            ..Config::default()
        };
        let (mut peer, mut rx, _drain) = start(server, Config::default());
        let mut held = Vec::new();
        for _ in 0..30 {
            let stream = peer.client.open_stream(get(b"/"), true).unwrap();
            peer.client.reset(stream, ErrorCode::Cancel);
            peer.exchange().await;
            // The guest has the request and is still working on it.
            while let Ok(pending) = rx.try_recv() {
                held.push(pending);
            }
            if peer.goaway.is_some() {
                break;
            }
        }
        assert_eq!(peer.goaway, Some(ErrorCode::EnhanceYourCalm));
    }

    /// ...but a client that cancels requests the guest then lets go of — a
    /// browser navigating — never runs out.
    #[tokio::test]
    async fn ordinary_cancellations_do_not_exhaust_the_budget() {
        let server = Config {
            max_pending_resets: 10,
            ..Config::default()
        };
        let (mut peer, mut rx, _drain) = start(server, Config::default());
        for _ in 0..100 {
            let stream = peer.client.open_stream(get(b"/"), true).unwrap();
            peer.client.reset(stream, ErrorCode::Cancel);
            peer.exchange().await;
            // The guest notices the cancellation and drops the request.
            while let Ok(pending) = rx.try_recv() {
                drop(pending);
            }
            peer.exchange().await;
        }
        assert_eq!(peer.goaway, None);
        assert!(!peer.closed);
    }

    /// A connection lost mid-upload fails the guest's body; it never reads as
    /// a body that ended.
    #[tokio::test]
    async fn a_connection_lost_mid_body_fails_the_body() {
        let (mut peer, mut rx, _drain) = start(Config::default(), Config::default());
        let mut post = get(b"/upload");
        post[0].1 = b"POST";
        let stream = peer.client.open_stream(post, false).unwrap();
        peer.client.send_data(stream, b"half of it", false).unwrap();
        peer.exchange().await;
        let (request, _answer, _, _) = rx.recv().await.unwrap();
        let HttpServerBody::Stream(mut body) = request.body else {
            panic!("a streamed body");
        };
        assert_eq!(body.next().await.unwrap().unwrap(), b"half of it");
        drop(peer); // the client vanishes
        assert!(
            matches!(body.next().await, Some(Err(_))),
            "a truncated body must fail"
        );
    }

    #[tokio::test]
    async fn a_reset_before_the_answer_fires_the_disconnect_signal() {
        let (mut peer, mut rx, _drain) = start(Config::default(), Config::default());
        let stream = peer.client.open_stream(get(b"/"), true).unwrap();
        peer.exchange().await;
        let (_, _answer, delivered, _) = rx.recv().await.unwrap();
        peer.client.reset(stream, ErrorCode::Cancel);
        peer.exchange().await;
        assert!(delivered.await.is_err(), "the guest's signal should fire");
    }

    #[tokio::test]
    async fn trailers_follow_the_body() {
        let (mut peer, mut rx, _drain) = start(Config::default(), Config::default());
        let stream = peer.client.open_stream(get(b"/grpc"), true).unwrap();
        peer.exchange().await;
        let (_, answer, _, _) = rx.recv().await.unwrap();
        let mut response = ok(HttpServerBody::Bytes(b"payload".to_vec()));
        response.trailers = Some(Box::pin(async {
            vec![("grpc-status".to_string(), "0".to_string())]
        }));
        answer.send(response).ok().unwrap();
        let events = peer.exchange().await;
        assert!(
            matches!(
                &events[1],
                Event::Data {
                    end_stream: false,
                    ..
                }
            ),
            "{events:?}"
        );
        assert_eq!(
            events[2],
            Event::Trailers {
                stream,
                fields: vec![(b"grpc-status".to_vec(), b"0".to_vec())]
            }
        );
    }

    #[tokio::test]
    async fn a_head_answer_keeps_its_length_and_sends_no_body() {
        let (mut peer, mut rx, _drain) = start(Config::default(), Config::default());
        let mut head = get(b"/");
        head[0].1 = b"HEAD";
        peer.client.open_stream(head, true).unwrap();
        peer.exchange().await;
        let (_, answer, _, _) = rx.recv().await.unwrap();
        let mut response = ok(HttpServerBody::Bytes(b"ignored".to_vec()));
        response
            .headers
            .push(("content-length".into(), "1234".into()));
        answer.send(response).ok().unwrap();
        let events = peer.exchange().await;
        assert!(
            matches!(&events[..], [Event::Headers { end_stream: true, fields, .. }]
            if fields.iter().any(|(n, v)| n == b"content-length" && v == b"1234")),
            "{events:?}"
        );
    }

    #[tokio::test]
    async fn draining_finishes_the_open_request_then_closes() {
        let (mut peer, mut rx, drain) = start(Config::default(), Config::default());
        let stream = peer.client.open_stream(get(b"/"), true).unwrap();
        peer.exchange().await;
        let (_, answer, _, _) = rx.recv().await.unwrap();
        drain.send(true).unwrap();
        peer.exchange().await;
        assert_eq!(peer.goaway, Some(ErrorCode::NoError));
        assert!(!peer.closed, "the open request is still owed its answer");
        answer
            .send(ok(HttpServerBody::Bytes(b"last".to_vec())))
            .ok()
            .unwrap();
        let events = peer.exchange().await;
        assert!(events.contains(&Event::Data {
            stream,
            data: b"last".to_vec(),
            end_stream: true
        }));
        assert!(peer.closed, "nothing left open: the connection closes");
    }
}
