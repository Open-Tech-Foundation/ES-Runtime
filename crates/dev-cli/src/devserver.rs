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
//! `GET /@esdev/hmr` is a **WebSocket** carrying one message per successful
//! rebuild. It is esdev's rather than the application's, so no template carries
//! dev-only code, and it accepts any origin because the page it talks to is
//! usually on the application's port rather than this one.
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

use crate::inspect::{read_head, request_path, respond};

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
    let path = target.split(['?', '#']).next().unwrap_or("/").to_string();

    if path == HMR_PATH {
        updates(stream, &head, server.reload.subscribe()).await;
        return;
    }
    let Some(root) = &server.serve else {
        let _ = respond(
            &mut stream,
            "404 Not Found",
            "text/plain",
            "esdev serves only the reload stream here: this project has a server of its own.",
        )
        .await;
        return;
    };
    serve_file(&mut stream, root, &path, &head).await;
}

/// Holds the connection open, writing an event per rebuild.
async fn updates(mut stream: TcpStream, head: &str, mut reload: broadcast::Receiver<Update>) {
    // Not an upgrade, so not this endpoint. Answered rather than dropped: this
    // is the URL somebody reaches for when they want to know whether the dev
    // server is up, and a closed connection tells them nothing.
    let Some(key) = crate::inspect::websocket_key(head) else {
        let _ = respond(
            &mut stream,
            "426 Upgrade Required",
            "text/plain; charset=utf-8",
            "esdev's update channel is a WebSocket.",
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

    // A page has arrived, and it may not hold what the last one was sent. The
    // next patch is computed as though nothing had been delivered, so it carries
    // what this page needs rather than a delta it cannot apply.
    crate::build::forget_shipped().await;

    loop {
        tokio::select! {
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
/// validators and whose handle feeds every read. The conditionals and
/// ranges parse the head already in memory, and only ranged reads add seeks
/// after that (reading just the spans, never the whole file for a slice of
/// it).
async fn serve_file(stream: &mut TcpStream, root: &Path, path: &str, head: &str) {
    let head_only = match crate::inspect::request_method(head) {
        Some("GET") => false,
        Some("HEAD") => true,
        _ => {
            let _ = respond_static(
                stream,
                "405 Method Not Allowed",
                &[
                    ("Allow".to_string(), "GET, HEAD".to_string()),
                    ("Cache-Control".to_string(), "no-store".to_string()),
                ],
                Some(b"this server serves files".as_slice()),
            )
            .await;
            return;
        }
    };
    let Some(relative) = safe_path(path) else {
        let _ = respond(stream, "400 Bad Request", "text/plain", "bad path").await;
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
    fn resolve(canonical_root: Option<&Path>, mut logical: PathBuf) -> Resolved {
        let contained = |canonical: &Path| {
            canonical_root.is_none_or(|root| canonical.starts_with(root))
        };
        let Ok(mut canonical) = std::fs::canonicalize(&logical) else {
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
        if kind.is_dir() {
            logical = logical.join("index.html");
            let Ok(settled) = std::fs::canonicalize(&logical) else {
                return Resolved::Missing;
            };
            if !contained(&settled) {
                return Resolved::Outside;
            }
            canonical = settled;
        }
        let Ok(file) = std::fs::File::open(&canonical) else {
            return Resolved::Missing;
        };
        let Ok(meta) = file.metadata() else {
            return Resolved::Missing;
        };
        Resolved::File { logical, file, meta }
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
            let _ = respond(
                stream,
                "403 Forbidden",
                "text/plain",
                "outside the served directory",
            )
            .await;
        }
        Resolved::Missing => {
            let _ = respond(
                stream,
                "404 Not Found",
                "text/plain",
                &format!("no {path} in {}", root.display()),
            )
            .await;
        }
        Resolved::File { meta, .. } if !meta.is_file() => {
            let _ = respond(
                stream,
                "404 Not Found",
                "text/plain",
                &format!("no {path} in {}", root.display()),
            )
            .await;
        }
        Resolved::File { logical, file, meta } => {
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
            headers.push((
                "Content-Range".to_string(),
                format!("bytes */{len}"),
            ));
            let _ = respond_static(
                stream,
                "416 Range Not Satisfiable",
                &headers,
                Some(b"".as_slice()),
            )
            .await;
        }
        crate::static_serve::Decision::Full => {
            use std::io::Read;
            headers.push(("Content-Type".to_string(), content_type));
            headers.push(("Content-Length".to_string(), len.to_string()));
            let body = (!head_only).then(|| {
                let mut body = Vec::with_capacity(len.min(8 << 20) as usize);
                file.read_to_end(&mut body).ok().map(|_| body)
            }).flatten();
            if !head_only && body.is_none() {
                let _ = respond(stream, "500 Internal Server Error", "text/plain", "unreadable").await;
                return;
            }
            let _ = respond_static(stream, "200 OK", &headers, body.as_deref()).await;
        }
        crate::static_serve::Decision::Single(span) => {
            headers.push(("Content-Type".to_string(), content_type));
            headers.push((
                "Content-Range".to_string(),
                format!("bytes {}-{}/{len}", span.start, span.end),
            ));
            headers.push(("Content-Length".to_string(), span.len().to_string()));
            let body = (!head_only).then(|| read_span(&mut file, span).ok()).flatten();
            if !head_only && body.is_none() {
                let _ = respond(stream, "500 Internal Server Error", "text/plain", "unreadable").await;
                return;
            }
            let _ =
                respond_static(stream, "206 Partial Content", &headers, body.as_deref()).await;
        }
        crate::static_serve::Decision::Multi(spans) => {
            let boundary = crate::static_serve::boundary();
            let (parts, ok) = match head_only {
                true => (Vec::new(), true),
                false => {
                    let mut parts = Vec::with_capacity(spans.len());
                    let mut ok = true;
                    for span in &spans {
                        match read_span(&mut file, *span) {
                            Ok(bytes) => parts.push((*span, bytes)),
                            Err(_) => {
                                ok = false;
                                break;
                            }
                        }
                    }
                    (parts, ok)
                }
            };
            if !ok {
                let _ = respond(stream, "500 Internal Server Error", "text/plain", "unreadable").await;
                return;
            }
            // A `HEAD` answers the length its `GET` would send, computed
            // arithmetically — no span is read for a body nobody receives.
            let body = (!head_only).then(|| {
                crate::static_serve::multipart_body(&boundary, &content_type, len, &parts)
            });
            let length = match &body {
                Some(b) => b.len() as u64,
                None => crate::static_serve::multipart_content_length(
                    &boundary,
                    &content_type,
                    len,
                    &spans,
                ),
            };
            headers.push((
                "Content-Type".to_string(),
                format!("multipart/byteranges; boundary={boundary}"),
            ));
            headers.push(("Content-Length".to_string(), length.to_string()));
            let _ =
                respond_static(stream, "206 Partial Content", &headers, body.as_deref()).await;
        }
    }
}

/// Reads one byte span through the open handle, seeking straight to it.
///
/// What keeps a `<video>` seek cheap: the bytes around the span are never
/// read, which a full read plus a slice would do on every seek of a large
/// file. Sharing the handle with every other read is what keeps the spans
/// of one response on one version of the file.
fn read_span(file: &mut std::fs::File, span: crate::static_serve::Span) -> std::io::Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    file.seek(SeekFrom::Start(span.start))?;
    let mut buf = vec![0u8; span.len() as usize];
    file.read_exact(&mut buf)?;
    Ok(buf)
}

/// A response with caller-chosen headers and an optional body.
///
/// [`respond`] stays for the fixed-shape text answers; this one is for file
/// responses, whose headers depend on the decision and whose `HEAD` answers
/// carry lengths without bodies. Nothing is added implicitly — callers name
/// every header, including `Content-Length` when there is one.
async fn respond_static(
    stream: &mut TcpStream,
    status: &str,
    headers: &[(String, String)],
    body: Option<&[u8]>,
) -> std::io::Result<()> {
    let mut head = format!("HTTP/1.1 {status}\r\n");
    for (name, value) in headers {
        head.push_str(name);
        head.push_str(": ");
        head.push_str(value);
        head.push_str("\r\n");
    }
    head.push_str("Connection: close\r\n\r\n");
    stream.write_all(head.as_bytes()).await?;
    if let Some(body) = body {
        stream.write_all(body).await?;
    }
    stream.flush().await
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

#[cfg(test)]
mod tests {
    use super::*;

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
