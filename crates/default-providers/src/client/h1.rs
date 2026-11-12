//! HTTP/1.1 for the client (DECISIONS D217), spoken with `es-runtime-http`'s
//! client codec: one request on one connection, its response read back, and
//! the connection returned to the pool ([`super::select`]) when it can carry
//! another.
//!
//! A pooled connection is reused only when the previous response ended exactly
//! where its framing said, and is checked still open before it is handed out
//! again.

use crate::client::types::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Version};
use bytes::Bytes;
use es_runtime_common::ErrorCode;
use es_runtime_http::fields::Field;
use es_runtime_http::h1::{
    Chunk, ChunkedDecoder, Framing, Limits, ParsedResponse, ResponseBody, encode_last_chunk,
    encode_request_head, parse_response,
};
use es_runtime_providers::ProviderError;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

use super::select::{BoxError, Exchange, Frame, Origin, Pool, Response, ResponseStream};
use crate::client::{Body, Conn};

/// How many body frames may wait between the connection and the reader.
const BODY_CHANNEL: usize = 4;

/// The most body bytes handed over in one frame.
const MAX_FRAME: usize = 64 * 1024;

/// A buffered request body at most this large is written with its head.
const COALESCE_BODY: usize = 64 * 1024;

/// Why an exchange failed, and whether it may be tried again on a fresh
/// connection.
pub(super) struct Failure {
    pub(super) error: ProviderError,
    /// Nothing of a response arrived: the request may not have been seen.
    pub(super) before_response: bool,
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

/// The framing a request body goes out with. A method whose request usually
/// has a body says its empty one is empty, as other clients do.
fn request_framing(method: &Method, body: &Body) -> Framing {
    match body {
        Body::Empty if [Method::POST, Method::PUT, Method::PATCH].contains(method) => {
            Framing::Length(0)
        }
        Body::Empty => Framing::NoBody,
        Body::Bytes(b) => Framing::Length(b.len() as u64),
        Body::Stream(_) => Framing::Chunked,
    }
}

pub(super) async fn exchange_h1(
    pool: &Pool,
    origin: &Origin,
    mut conn: Conn,
    exchange: &Exchange<'_>,
    body: Body,
) -> Result<Response, Failure> {
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
) -> Result<Response, Failure> {
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
    let mut response = Response {
        status,
        version: Version::Http11,
        headers,
        body: empty_body(),
    };

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
        response.body = Box::pin(futures_util::stream::iter(frames.into_iter().map(Ok)));
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
    response.body = Box::pin(tokio_stream_from(rx));
    Ok(response)
}

/// The body's frames, when all of it is already in `buf`, and how many bytes
/// of `buf` it took. `None` when more must be read (or the body runs until
/// the connection closes).
fn buffered_body(
    buf: &[u8],
    framing: ResponseBody,
) -> Option<(std::collections::VecDeque<Frame>, usize)> {
    let mut frames = std::collections::VecDeque::new();
    match framing {
        ResponseBody::Empty => Some((frames, 0)),
        ResponseBody::Length(n) => {
            let n = usize::try_from(n).ok().filter(|&n| n <= buf.len())?;
            frames.push_back(Frame::Data(Bytes::copy_from_slice(&buf[..n])));
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
                frames.push_back(Frame::Data(Bytes::from(data)));
            }
            let mut trailers = HeaderMap::new();
            for (n, v) in decoder.trailers() {
                if let (Ok(n), Ok(v)) = (HeaderName::from_bytes(n), HeaderValue::from_bytes(v)) {
                    trailers.append(n, v);
                }
            }
            if !trailers.is_empty() {
                frames.push_back(Frame::Trailers(trailers));
            }
            Some((frames, at))
        }
        ResponseBody::UntilClose => None,
    }
}

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
                    if tx.send(Ok(Frame::Data(data))).await.is_err() {
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
                if !out.is_empty() && tx.send(Ok(Frame::Data(Bytes::from(out)))).await.is_err() {
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
                    if !trailers.is_empty() && tx.send(Ok(Frame::Trailers(trailers))).await.is_err()
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
                if tx.send(Ok(Frame::Data(data))).await.is_err() {
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

/// A body fed by the task reading the connection.
fn tokio_stream_from(
    mut rx: mpsc::Receiver<FrameResult>,
) -> impl futures_core::Stream<Item = FrameResult> + Send {
    futures_util::stream::poll_fn(move |cx| rx.poll_recv(cx))
}

type FrameResult = Result<Frame, BoxError>;

fn empty_body() -> ResponseStream {
    Box::pin(futures_util::stream::empty())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use crate::client::types::{Method, StatusCode};
    use futures_util::StreamExt;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;
    use url::Url;

    use crate::TrustStore;
    use crate::client::{Body, HttpClient, Outbound};

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
        assert_eq!(response.status, StatusCode::from_u16(413).unwrap());
    }
}
