//! The endpoint `esdev start` binds: the reload stream, and — for a stack with
//! no server of its own — the files.
//!
//! # Why esdev serves anything at all
//!
//! It nearly does not. For a fullstack or backend project the server is the
//! **application's**: `esdev start` builds it and runs it, the same file that
//! runs in production, and what is bound here is one endpoint carrying one
//! message. That is the shape to keep — the thing serving your app in
//! development should be the thing serving it in production.
//!
//! A frontend-only project has no server to be that, and telling somebody to
//! write one before they can look at their page is not parity with any tool
//! they have used. So when there is no target to run, this serves the output
//! directory: `GET` and `HEAD`, files, an SPA fallback, and the HTTP file
//! semantics a browser leans on — validators and revalidation (`ETag`,
//! `Last-Modified`, `304`), single and multiple ranges (`206`, `416`) — so a
//! preview answers about the build rather than about itself. No module graph,
//! no transform, no middleware — those would be a second, different way to run
//! the app, which is what this design is arranged to avoid.
//!
//! Dev and preview only: production static traffic belongs on a CDN or a
//! proxy with real hardening, and nothing here — loopback-bound, unlogged,
//! unmetered — pretends otherwise.
//!
//! # The update channel
//!
//! `GET /@esdev/hmr` is a **WebSocket** carrying successful rebuild updates and
//! the current build failure, if there is one. It is esdev's rather than the
//! application's, so no template carries dev-only code, and it accepts any
//! origin because the page it talks to is usually on the application's port
//! rather than this one.
//!
//! ## Why a WebSocket and not the event stream it used to be
//!
//! Only one of these reasons is about today, and it is the weakest: what a
//! rebuild has to say is `reload`, which fits in a line of `text/event-stream`
//! perfectly well. The other two are about what this channel is being built to
//! carry.
//!
//! **A hot update is a module's source**, which is multi-line JavaScript. SSE is
//! a line protocol, so every patch would have to be JSON-escaped or split across
//! `data:` lines — a re-encoding on the hot path, for ever, to fit a shape the
//! payload does not have.
//!
//! **And SSE runs out of connections.** HTTP/1.1 caps a browser at roughly six
//! per origin and a stream holds one open for as long as the page is; the
//! seventh tab of your own app simply stops hot updating, with nothing anywhere
//! saying why. A silent failure a developer would reasonably blame on their own
//! code is not a thing to build a foundation on.
//!
//! What SSE gave up in exchange is real: `EventSource` reconnects on its own,
//! and a dev server restarts constantly. That is bought back by hand, in the
//! client below — the one part of this worth reading twice.
//!
//! The handshake and framing cost nothing here: `--inspect` already speaks
//! WebSocket in its server role, in this binary, on this accept loop
//! ([`crate::inspect`]).

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use futures_util::{SinkExt, StreamExt};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::sync::broadcast;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
use tokio_tungstenite::tungstenite::protocol::Role;

use crate::inspect::{read_head, request_path};

/// The path the injected script connects to.
pub const HMR_PATH: &str = "/@esdev/hmr";

/// What the dev server tells a page after a build.
///
/// An enum with one variant today, and that is the point of it being an enum:
/// the transport, the client's dispatch and the broadcast channel are all
/// already shaped for a message that says *what* changed, so the CSS swap and
/// the module patch are new variants rather than a new protocol.
#[derive(Clone, Debug)]
pub enum Update {
    /// A hot patch: the page loads it, then walks its own graph from
    /// `changed_ids` to decide what to re-run — or reloads itself if nothing on
    /// the way up accepted the change.
    Patch {
        /// Where the page fetches the patch from.
        url: String,
        /// The modules the patch replaces.
        changed_ids: Vec<String>,
    },
    /// Only stylesheets changed. The page keeps everything it has — scroll
    /// position, an open dialog, whatever was typed into a form — and fetches
    /// its stylesheets again.
    Css,
    /// Nothing finer-grained is available: load the page again.
    Reload,
    /// The build failed. The page shows it rather than sitting on the last
    /// good build in silence; the next patch, swap or reload clears it.
    Error {
        /// The failure as the terminal prints it, without colour.
        message: String,
    },
}

impl Update {
    /// The message as it goes over the wire.
    ///
    /// Written by hand rather than derived: it is two fields on the far side of
    /// a socket from a `JSON.parse` in a string literal, and a serde dependency
    /// for that would be a dependency for a brace.
    fn as_message(&self) -> String {
        match self {
            Self::Patch { url, changed_ids } => {
                // Two strings and a list of them, so `JSON.parse` on the far
                // side has something to parse. Ids come from module paths, which
                // can hold a quote or a backslash on a filesystem that allows
                // one, so they are escaped rather than trusted.
                let ids = changed_ids
                    .iter()
                    .map(|id| format!("\"{}\"", escape_json(id)))
                    .collect::<Vec<_>>()
                    .join(",");
                format!(
                    "{{\"type\":\"patch\",\"url\":\"{}\",\"changedIds\":[{ids}]}}",
                    escape_json(url)
                )
            }
            Self::Css => "{\"type\":\"css\"}".to_string(),
            Self::Reload => "{\"type\":\"reload\"}".to_string(),
            Self::Error { message } => {
                format!(
                    "{{\"type\":\"error\",\"message\":\"{}\"}}",
                    escape_json(message)
                )
            }
        }
    }
}

/// What the endpoint serves.
pub struct DevServer {
    /// The directory to serve files from, when there is no application server
    /// doing it.
    pub serve: Option<PathBuf>,
    /// Told after every successful rebuild.
    pub reload: broadcast::Sender<Update>,
    /// The current build failure, replayed to pages that connect after it.
    pub error: tokio::sync::watch::Sender<Option<String>>,
}

/// Accepts connections until the process ends.
pub async fn serve(listener: std::net::TcpListener, server: std::sync::Arc<DevServer>) {
    let Ok(listener) = tokio::net::TcpListener::from_std(listener) else {
        return;
    };
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            continue;
        };
        // Each connection on its own task. The reload stream is held open for
        // as long as the page is, so anything sharing this task with it would
        // wait for the developer to close their browser.
        tokio::spawn(handle(stream, server.clone()));
    }
}

/// Serves one connection.
async fn handle(mut stream: TcpStream, server: std::sync::Arc<DevServer>) {
    let Some(head) = read_head(&mut stream).await else {
        return;
    };
    let Some(target) = request_path(&head) else {
        return;
    };
    // The query string belongs to the page, not to the file it names.
    let target_path = target.split(['?', '#']).next().unwrap_or("/");
    let Some(path) = decode_path(target_path) else {
        let head_only = matches!(crate::inspect::request_method(&head), Some("HEAD"));
        respond_error(&mut stream, "400 Bad Request", "bad path", head_only, &[]).await;
        return;
    };

    if path == HMR_PATH {
        // The handshake is a GET-only upgrade (RFC 6455 §4.1: the method
        // MUST be GET), so only GET reaches the upgrader: a HEAD is
        // answered like any error here — headers, no body — and anything
        // else is refused rather than upgraded.
        match crate::inspect::request_method(&head) {
            Some("GET") => {
                updates(
                    stream,
                    &head,
                    server.reload.subscribe(),
                    server.error.subscribe(),
                )
                .await;
                return;
            }
            Some("HEAD") => {
                respond_error(
                    &mut stream,
                    "426 Upgrade Required",
                    "esdev's update channel is a WebSocket.",
                    true,
                    // A 426 names the protocol it wants: required, not
                    // courtesy.
                    &[("Upgrade", "websocket")],
                )
                .await;
                return;
            }
            _ => {
                respond_error(
                    &mut stream,
                    "405 Method Not Allowed",
                    "esdev's update channel is a WebSocket.",
                    false,
                    &[("Allow", "GET, HEAD")],
                )
                .await;
                return;
            }
        }
    }
    // Errors below honour HEAD like everything else: the same headers a GET
    // would name, no body bytes.
    let head_only = matches!(crate::inspect::request_method(&head), Some("HEAD"));
    let Some(root) = &server.serve else {
        respond_error(
            &mut stream,
            "404 Not Found",
            "esdev serves only the reload stream here: this project has a server of its own.",
            head_only,
            &[],
        )
        .await;
        return;
    };
    serve_file(&mut stream, root, &path, &head).await;
}

/// An error answer: `text/plain` with the length a GET would carry, and no
/// body on a HEAD. `no-store` throughout: a cached 404 outlives the rebuild
/// that creates the file.
async fn respond_error(
    stream: &mut TcpStream,
    status: &str,
    message: &str,
    head_only: bool,
    extra: &[(&str, &str)],
) {
    let mut headers = vec![
        (
            "Content-Type".to_string(),
            "text/plain; charset=utf-8".to_string(),
        ),
        ("Content-Length".to_string(), message.len().to_string()),
        ("Cache-Control".to_string(), "no-store".to_string()),
    ];
    for (name, value) in extra {
        headers.push((name.to_string(), value.to_string()));
    }
    let body = (!head_only).then_some(message.as_bytes());
    let _ = respond_static(stream, status, &headers, body).await;
}

/// Holds the connection open, writing an event per rebuild.
async fn updates(
    mut stream: TcpStream,
    head: &str,
    mut reload: broadcast::Receiver<Update>,
    mut error: tokio::sync::watch::Receiver<Option<String>>,
) {
    // Not an upgrade, so not this endpoint. Answered rather than dropped: this
    // is the URL somebody reaches for when they want to know whether the dev
    // server is up, and a closed connection tells them nothing.
    let Some(key) = crate::inspect::websocket_key(head) else {
        respond_error(
            &mut stream,
            "426 Upgrade Required",
            "esdev's update channel is a WebSocket.",
            false,
            &[("Upgrade", "websocket")],
        )
        .await;
        return;
    };

    let accept = derive_accept_key(key.as_bytes());
    let handshake = format!(
        "HTTP/1.1 101 Switching Protocols\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Accept: {accept}\r\n\r\n"
    );
    if stream.write_all(handshake.as_bytes()).await.is_err() {
        return;
    }
    let socket = WebSocketStream::from_raw_socket(stream, Role::Server, None).await;
    let (mut sink, mut incoming) = socket.split();

    // A page may connect after the failed build (or reconnect while the error
    // is still current). Read the state after subscribing so a concurrent
    // transition is either represented here or wakes the select below.
    let current_error = { error.borrow_and_update().clone() };
    if let Some(message) = current_error
        && sink
            .send(Message::Text(Update::Error { message }.as_message().into()))
            .await
            .is_err()
    {
        return;
    }

    // A page has arrived, and it may not hold what the last one was sent. The
    // next patch is computed as though nothing had been delivered, so it carries
    // what this page needs rather than a delta it cannot apply.
    crate::build::forget_shipped().await;

    loop {
        tokio::select! {
            changed = error.changed() => {
                if changed.is_err() {
                    return;
                }
                let current_error = { error.borrow_and_update().clone() };
                if let Some(message) = current_error
                    && sink.send(Message::Text(Update::Error { message }.as_message().into())).await.is_err()
                {
                    return;
                }
            }
            update = reload.recv() => {
                let message = match update {
                    Ok(update) => update,
                    // The page missed a rebuild, or several. Whatever they were,
                    // the state it is in now is stale, and the answer that is
                    // correct for every combination is to start over.
                    Err(broadcast::error::RecvError::Lagged(_)) => Update::Reload,
                    Err(broadcast::error::RecvError::Closed) => return,
                };
                if sink.send(Message::Text(message.as_message().into())).await.is_err() {
                    return;
                }
            }
            // Nothing is expected from the page — the ship map that decides what
            // a patch contains is the server's own record, so a client has
            // nothing to report. This arm exists because the socket has to be
            // *polled* for its pong to be sent and for a close to be noticed,
            // and a channel nobody reads is a connection that never ends.
            frame = incoming.next() => match frame {
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
                Some(Ok(_)) => {}
            },
        }
    }
}

/// Answers a file request, falling back to `index.html` the way a single-page
/// app needs, and honouring validators and ranges the way a browser expects.
///
/// Routing settles paths rather than trusting them: the root once per
/// request, then each candidate path once (one more for a directory's
/// `index.html`, one more for the SPA fallback). Each candidate costs a
/// canonicalization for containment plus a kind metadata — the kind must
/// come from metadata because opening a directory fails on Windows — and
/// the surviving candidate costs one open whose metadata feeds the
/// validators. Every body then streams off that handle in fixed 64 KiB
/// chunks: full, single-range and multi-range alike, so no request ever
/// holds a file (or a span of one) in memory whole. The conditionals and
/// ranges parse the head already in memory.
async fn serve_file(stream: &mut TcpStream, root: &Path, path: &str, head: &str) {
    let head_only = match crate::inspect::request_method(head) {
        Some("GET") => false,
        Some("HEAD") => true,
        _ => {
            // HEAD passes the gate above, so this arm is unreachable on
            // one — `false` is what `head_only` would be here.
            respond_error(
                stream,
                "405 Method Not Allowed",
                "this server serves files",
                false,
                &[("Allow", "GET, HEAD")],
            )
            .await;
            return;
        }
    };
    let Some(relative) = safe_path(path) else {
        respond_error(stream, "400 Bad Request", "bad path", head_only, &[]).await;
        return;
    };
    let file = root.join(&relative);
    // The served root, settled once: every resolution below is checked
    // against this. `None` when the root itself is unreachable, in which
    // case nothing under it resolves either.
    let canonical_root = std::fs::canonicalize(root).ok();
    // `symlink_metadata` only reveals a link at the final component, while a
    // link in any parent (`linkdir -> /etc`, asking for `/linkdir/passwd`)
    // is followed silently — so the check cannot be "is this a link", it has
    // to be "is this path, fully settled, still inside". The root is settled
    // once per request; each candidate path is settled once more (see the
    // call count note on `serve_file`).
    // Moved by value a few times per request against syscalls that cost a
    // thousand times more; boxing it would buy nothing measurable.
    #[allow(clippy::large_enum_variant)]
    enum Resolved {
        /// Ready to answer: the path as requested (which names the MIME
        /// type), the one open handle, and that handle's metadata — so
        /// length, validators and every byte below are always one version.
        File {
            logical: PathBuf,
            file: std::fs::File,
            meta: std::fs::Metadata,
        },
        /// A path settling outside the served directory.
        Outside,
        /// Nothing to serve here.
        Missing,
    }
    fn resolve(canonical_root: Option<&Path>, logical: PathBuf) -> Resolved {
        let contained =
            |canonical: &Path| canonical_root.is_none_or(|root| canonical.starts_with(root));
        let Ok(canonical) = std::fs::canonicalize(&logical) else {
            return Resolved::Missing;
        };
        if !contained(&canonical) {
            return Resolved::Outside;
        }
        // The kind comes from metadata, never from opening: opening a
        // directory fails on Windows (CreateFileW without directory flags),
        // so a dir must be recognised before any handle exists. This
        // metadata answers routing only — length, mtime and bytes all come
        // from the handle below, so the single-version guarantee holds.
        let Ok(kind) = std::fs::metadata(&canonical) else {
            return Resolved::Missing;
        };
        // The kind decides before any handle exists. Opening a directory
        // fails on Windows, and opening a FIFO blocks until a writer
        // arrives — either one turns the request into a failure or a hung
        // connection, so only regular files reach `File::open`. The index
        // target gets its own kind check: a directory entry can name
        // anything, FIFO included. Kind metadata answers routing only —
        // length, mtime and bytes all come from the handle below, so the
        // single-version guarantee holds.
        let (canonical, logical) = if kind.is_dir() {
            let index = logical.join("index.html");
            let Ok(settled) = std::fs::canonicalize(&index) else {
                return Resolved::Missing;
            };
            if !contained(&settled) {
                return Resolved::Outside;
            }
            match std::fs::metadata(&settled) {
                Ok(kind) if kind.is_file() => (settled, index),
                _ => return Resolved::Missing,
            }
        } else if kind.is_file() {
            (canonical, logical)
        } else {
            return Resolved::Missing;
        };
        let Ok(file) = std::fs::File::open(&canonical) else {
            return Resolved::Missing;
        };
        let Ok(meta) = file.metadata() else {
            return Resolved::Missing;
        };
        Resolved::File {
            logical,
            file,
            meta,
        }
    }
    let mut resolved = resolve(canonical_root.as_deref(), file);
    // **The fallback is what makes client-side routing work.** A reload on
    // /about asks for a file nobody wrote; the app's router is in the bundle
    // index.html loads. It applies only to paths that look like routes — a
    // missing .js answered with HTML is a syntax error three steps from its
    // cause, and a missing image should be a missing image.
    if !matches!(&resolved, Resolved::File { meta, .. } if meta.is_file())
        && !matches!(resolved, Resolved::Outside)
        && Path::new(path).extension().is_none()
    {
        resolved = resolve(canonical_root.as_deref(), root.join("index.html"));
    }
    match resolved {
        Resolved::Outside => {
            respond_error(
                stream,
                "403 Forbidden",
                "outside the served directory",
                head_only,
                &[],
            )
            .await;
        }
        Resolved::Missing => {
            respond_error(
                stream,
                "404 Not Found",
                &format!("no {path} in {}", root.display()),
                head_only,
                &[],
            )
            .await;
        }
        Resolved::File { meta, .. } if !meta.is_file() => {
            respond_error(
                stream,
                "404 Not Found",
                &format!("no {path} in {}", root.display()),
                head_only,
                &[],
            )
            .await;
        }
        Resolved::File {
            logical,
            file,
            meta,
        } => {
            serve_resolved(stream, head, head_only, file, &meta, &logical).await;
        }
    }
}

/// Serves a resolved file: validators, conditionals and ranges around the
/// read. Split from routing above so the two stay readable apart — one finds
/// and opens the file, the other answers for it.
///
/// The handle is taken by value and every byte below comes out of it, so the
/// length, the validators and the body cannot straddle a rebuild.
#[allow(clippy::too_many_arguments)]
async fn serve_resolved(
    stream: &mut TcpStream,
    head: &str,
    head_only: bool,
    mut file: std::fs::File,
    meta: &std::fs::Metadata,
    logical: &Path,
) {
    let len = meta.len();
    let mtime = meta.modified().ok();
    let mtime_secs = mtime.map(|t| {
        t.duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    });
    let tag = crate::static_serve::etag(len, mtime.unwrap_or(UNIX_EPOCH));
    // Typed by what was asked for, read from where it settled: an in-root
    // `app.js` pointing at a `.bin` is JavaScript to the browser that
    // asked for it.
    let content_type = content_type(logical).to_string();
    // `no-cache`: always revalidate, so a rebuild is never stale — but
    // revalidations still answer `304` instead of resending.
    let mut headers = vec![
        ("ETag".to_string(), tag.clone()),
        ("Accept-Ranges".to_string(), "bytes".to_string()),
        ("Cache-Control".to_string(), "no-cache".to_string()),
    ];
    if let Some(t) = mtime {
        headers.push((
            "Last-Modified".to_string(),
            crate::static_serve::last_modified(t),
        ));
    }
    match crate::static_serve::decide(head, len, &tag, mtime_secs) {
        crate::static_serve::Decision::NotModified => {
            let _ = respond_static(stream, "304 Not Modified", &headers, None).await;
        }
        crate::static_serve::Decision::Unsatisfiable => {
            headers.push(("Content-Range".to_string(), format!("bytes */{len}")));
            let _ = respond_static(
                stream,
                "416 Range Not Satisfiable",
                &headers,
                Some(b"".as_slice()),
            )
            .await;
        }
        crate::static_serve::Decision::Full => {
            headers.push(("Content-Type".to_string(), content_type));
            headers.push(("Content-Length".to_string(), len.to_string()));
            if head_only {
                let _ = respond_static(stream, "200 OK", &headers, None).await;
                return;
            }
            // Streamed like every other body: the head promises the length
            // from the handle's metadata, then the bytes follow in fixed
            // chunks — no request ever holds a whole file in memory.
            if write_head(stream, "200 OK", &headers).await.is_err() {
                return;
            }
            if len > 0
                && copy_span(
                    &mut file,
                    crate::static_serve::Span {
                        start: 0,
                        end: len - 1,
                    },
                    stream,
                )
                .await
                .is_err()
            {
                return;
            }
            let _ = stream.flush().await;
        }
        crate::static_serve::Decision::Single(span) => {
            headers.push(("Content-Type".to_string(), content_type));
            headers.push((
                "Content-Range".to_string(),
                format!("bytes {}-{}/{len}", span.start, span.end),
            ));
            headers.push(("Content-Length".to_string(), span.len().to_string()));
            if head_only {
                let _ = respond_static(stream, "206 Partial Content", &headers, None).await;
                return;
            }
            if write_head(stream, "206 Partial Content", &headers)
                .await
                .is_err()
            {
                return;
            }
            if copy_span(&mut file, span, stream).await.is_err() {
                return;
            }
            let _ = stream.flush().await;
        }
        crate::static_serve::Decision::Multi(spans) => {
            let boundary = crate::static_serve::boundary();
            // The length up front, arithmetically — it is what a `HEAD`
            // answers with, and what a `GET` streams exactly.
            let length = crate::static_serve::multipart_content_length(
                &boundary,
                &content_type,
                len,
                &spans,
            );
            headers.push((
                "Content-Type".to_string(),
                format!("multipart/byteranges; boundary={boundary}"),
            ));
            headers.push(("Content-Length".to_string(), length.to_string()));
            if head_only {
                let _ = respond_static(stream, "206 Partial Content", &headers, None).await;
                return;
            }
            // Streamed, never assembled: framing lines plus span bytes copied
            // in chunks off the one handle, so thirty-two duplicate
            // full-file ranges cost a 64 KiB buffer rather than tens of
            // gigabytes. Past the status line the only honest failure is a
            // closed connection, so errors end the body rather than
            // answering anything.
            if write_head(stream, "206 Partial Content", &headers)
                .await
                .is_err()
            {
                return;
            }
            let mut ok = true;
            for span in &spans {
                let head = crate::static_serve::part_head(&boundary, &content_type, len, *span);
                if stream.write_all(head.as_bytes()).await.is_err()
                    || copy_span(&mut file, *span, stream).await.is_err()
                    || stream.write_all(b"\r\n").await.is_err()
                {
                    ok = false;
                    break;
                }
            }
            if ok {
                let tail = format!("--{boundary}--\r\n");
                let _ = stream.write_all(tail.as_bytes()).await;
            }
            let _ = stream.flush().await;
        }
    }
}

/// A response with caller-chosen headers and an optional body.
///
/// [`respond`] stays for the fixed-shape text answers; this one is for file
/// responses, whose headers depend on the decision and whose `HEAD` answers
/// carry lengths without bodies. Callers name every header except `Date` —
/// the server has a clock, so HTTP wants it stamped on 2xx, 3xx and 4xx,
/// and stamping it here covers them all — including `Content-Length` when
/// there is one.
async fn respond_static(
    stream: &mut TcpStream,
    status: &str,
    headers: &[(String, String)],
    body: Option<&[u8]>,
) -> std::io::Result<()> {
    write_head(stream, status, headers).await?;
    if let Some(body) = body {
        stream.write_all(body).await?;
    }
    stream.flush().await
}

/// The status line and headers, without the body: what a streamed response
/// writes before producing its bytes.
async fn write_head(
    stream: &mut TcpStream,
    status: &str,
    headers: &[(String, String)],
) -> std::io::Result<()> {
    let date = crate::static_serve::http_date(std::time::SystemTime::now());
    let mut head = format!("HTTP/1.1 {status}\r\nDate: {date}\r\n");
    for (name, value) in headers {
        head.push_str(name);
        head.push_str(": ");
        head.push_str(value);
        head.push_str("\r\n");
    }
    head.push_str("Connection: close\r\n\r\n");
    stream.write_all(head.as_bytes()).await
}

/// Copies one span onto the socket in chunks, off the shared handle.
///
/// The buffer is fixed at 64 KiB no matter how long the span: a full-file
/// body streams through it rather than landing in memory whole, and a
/// `<video>` seek never reads the bytes around its span. Sharing the handle
/// with every other read is what keeps the spans of one response on one
/// version of the file.
async fn copy_span(
    file: &mut std::fs::File,
    span: crate::static_serve::Span,
    stream: &mut TcpStream,
) -> std::io::Result<()> {
    use std::io::{Read, Seek, SeekFrom};
    file.seek(SeekFrom::Start(span.start))?;
    let mut remaining = span.len();
    let mut buf = [0u8; 65536];
    while remaining > 0 {
        let n = remaining.min(65536) as usize;
        file.read_exact(&mut buf[..n])?;
        stream.write_all(&buf[..n]).await?;
        remaining -= n as u64;
    }
    Ok(())
}

/// The request path as a relative path, or `None` if it tries to leave the
/// directory being served.
///
/// A dev server binds loopback and serves a directory the developer chose, so
/// this is not the last line of anything — but `..` in a URL is never a
/// legitimate way to ask for a file, and a tool that followed one would be
/// handing out whatever the browser asked for.
fn safe_path(path: &str) -> Option<PathBuf> {
    let trimmed = path.trim_start_matches('/');
    if trimmed.is_empty() {
        return Some(PathBuf::from("index.html"));
    }
    let mut safe = PathBuf::new();
    for part in trimmed.split('/') {
        match part {
            "" | "." => {}
            ".." => return None,
            part if part.contains('\\') => return None,
            part => safe.push(part),
        }
    }
    Some(safe)
}

/// Decodes one URL path before filesystem routing. `+` remains a plus (it is
/// not a space in a URL path), malformed escapes and invalid UTF-8 are refused.
/// Traversal is checked after decoding by [`safe_path`].
fn decode_path(path: &str) -> Option<String> {
    let bytes = path.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' {
            let high = *bytes.get(at + 1)?;
            let low = *bytes.get(at + 2)?;
            decoded.push((hex(high)? << 4) | hex(low)?);
            at += 3;
        } else {
            decoded.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// The `Content-Type` for a file, by extension.
///
/// Short and explicit rather than a table of every type there is: what a dev
/// server hands a browser is what the build wrote, and the build writes
/// JavaScript, documents, stylesheets and whatever the author put in `public`.
/// The default is `application/octet-stream`, which a browser downloads rather
/// than guesses at — the safe direction to be wrong in.
pub(crate) fn content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "gif" => "image/gif",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "wasm" => "application/wasm",
        "txt" | "map" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// [`respond`] for a body that is not text.
pub(crate) async fn respond_bytes(
    stream: &mut TcpStream,
    content_type: &str,
    body: &[u8],
) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {}\r\n\
         Cache-Control: no-store\r\n\
         Connection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body).await?;
    stream.flush().await
}

/// The address the endpoint binds: loopback, always.
///
/// Not an option, and not for the reason `--inspect`'s is warned about rather
/// than refused. A debugger port is a way to run code in a process; this one
/// hands out files from a directory and says one word. What makes it loopback
/// is that it is *development*: it exists for the person at the keyboard, and a
/// build tool that puts a port on a coffee-shop network by default has made a
/// decision nobody asked it to make.
pub fn address(port: u16) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], port))
}

/// A string, as a JSON string body.
///
/// The two characters that end a JSON string early, plus the control range that
/// is not allowed in one raw. Enough for module ids and a URL this code built —
/// and deliberately not a JSON library, for one field of one message.
fn escape_json(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Terminal escapes out of overlay text.
///
/// The server's stderr is usually a terminal, so diagnostics arrive painted —
/// and a painted string in the DOM reads as garbage around the message. Only
/// CSI sequences (`ESC [ params … final`: SGR colours and the reset this
/// codebase and its diagnostics emit); anything else passes through.
pub(crate) fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        // An escape: consume a CSI sequence, keep anything else as written
        // (a lone ESC is data, not paint, and dropping data corrupts).
        let mut clipped = String::from(c);
        let mut is_csi = false;
        for c in chars.by_ref() {
            clipped.push(c);
            if !is_csi {
                if c == '[' {
                    is_csi = true;
                } else {
                    break;
                }
                continue;
            }
            if ('@'..='~').contains(&c) {
                clipped.clear();
                break;
            }
        }
        out.push_str(&clipped);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_error_update_carries_its_message_as_json() {
        let message = Update::Error {
            message: "target \"web\": boom \"quoted\"".to_string(),
        }
        .as_message();
        assert_eq!(
            message,
            r#"{"type":"error","message":"target \"web\": boom \"quoted\""}"#
        );
        // And it parses as the object the client reads.
        assert!(message.starts_with("{\"type\":\"error\","));
    }

    /// Paint out, text intact: SGR colours and the reset go, everything else
    /// — including a lone ESC and a truncated sequence — stays byte for byte.
    #[test]
    fn overlay_text_is_stripped_of_paint() {
        assert_eq!(
            strip_ansi("\x1b[32mbuilt\x1b[0m → dist/\nplain"),
            "built → dist/\nplain"
        );
        assert_eq!(strip_ansi("no escapes here"), "no escapes here");
        assert_eq!(strip_ansi("lone \x1b"), "lone \x1b");
        assert_eq!(strip_ansi("cut \x1b[32"), "cut \x1b[32");
        assert_eq!(strip_ansi("\x1b[mreset-empty\x1b[0m"), "reset-empty");
    }

    #[test]
    fn a_path_that_climbs_out_is_refused() {
        assert_eq!(safe_path("/"), Some(PathBuf::from("index.html")));
        assert_eq!(safe_path("/app.js"), Some(PathBuf::from("app.js")));
        assert_eq!(
            safe_path("/assets/main-abc.js"),
            Some(PathBuf::from("assets/main-abc.js"))
        );
        assert_eq!(safe_path("/./a.js"), Some(PathBuf::from("a.js")));

        assert_eq!(safe_path("/../../etc/passwd"), None);
        assert_eq!(safe_path("/assets/../../secret"), None);
        assert_eq!(safe_path("/a\\..\\b"), None);
    }

    #[test]
    fn url_paths_are_decoded_before_filesystem_safety_checks() {
        assert_eq!(
            decode_path("/assets/my%20image-%C3%A9.png"),
            Some("/assets/my image-é.png".into())
        );
        assert_eq!(decode_path("/a+b.txt"), Some("/a+b.txt".into()));
        assert_eq!(decode_path("/%2e%2e/secret"), Some("/../secret".into()));
        assert_eq!(safe_path(&decode_path("/%2e%2e/secret").unwrap()), None);
        assert_eq!(decode_path("/bad%2"), None);
        assert_eq!(decode_path("/%FF"), None);
    }

    #[test]
    fn the_content_type_follows_the_extension() {
        assert_eq!(
            content_type(Path::new("/d/index.html")),
            "text/html; charset=utf-8"
        );
        assert_eq!(
            content_type(Path::new("/d/assets/main-abc.js")),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(content_type(Path::new("/d/logo.png")), "image/png");
        // Unknown is downloaded rather than guessed at.
        assert_eq!(
            content_type(Path::new("/d/data.bin")),
            "application/octet-stream"
        );
    }
}
