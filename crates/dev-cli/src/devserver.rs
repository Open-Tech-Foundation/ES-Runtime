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
//! directory: `GET`, files, an SPA fallback, and nothing else. No module graph,
//! no transform, no middleware — those would be a second, different way to run
//! the app, which is what this design is arranged to avoid.
//!
//! # The reload stream
//!
//! `GET /@esdev/reload` is an event stream that says `reload` after each
//! successful rebuild. It is esdev's rather than the application's, so no
//! template carries dev-only code, and it is CORS-open because the page it
//! talks to is usually on the application's port rather than this one.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::sync::broadcast;

use crate::inspect::{read_head, request_path, respond};

/// The path the injected script connects to.
pub const RELOAD_PATH: &str = "/@esdev/reload";

/// What the endpoint serves.
pub struct DevServer {
    /// The directory to serve files from, when there is no application server
    /// doing it.
    pub serve: Option<PathBuf>,
    /// Told after every successful rebuild.
    pub reload: broadcast::Sender<()>,
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

    if path == RELOAD_PATH {
        stream_reloads(stream, server.reload.subscribe()).await;
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
    serve_file(&mut stream, root, &path).await;
}

/// Holds the connection open, writing an event per rebuild.
async fn stream_reloads(mut stream: TcpStream, mut reload: broadcast::Receiver<()>) {
    let head = "HTTP/1.1 200 OK\r\n\
         Content-Type: text/event-stream\r\n\
         Cache-Control: no-store\r\n\
         Connection: keep-alive\r\n\
         Access-Control-Allow-Origin: *\r\n\r\n";
    if stream.write_all(head.as_bytes()).await.is_err() {
        return;
    }
    loop {
        match reload.recv().await {
            Ok(()) => {
                if stream.write_all(b"data: reload\n\n").await.is_err() {
                    return;
                }
            }
            // Lagged: the page missed a rebuild or several, and the answer to
            // every one of them is the same single word.
            Err(broadcast::error::RecvError::Lagged(_)) => {
                if stream.write_all(b"data: reload\n\n").await.is_err() {
                    return;
                }
            }
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}

/// Answers a file request, falling back to `index.html` the way a single-page
/// app needs.
async fn serve_file(stream: &mut TcpStream, root: &Path, path: &str) {
    let Some(relative) = safe_path(path) else {
        let _ = respond(stream, "400 Bad Request", "text/plain", "bad path").await;
        return;
    };
    let mut file = root.join(&relative);
    if file.is_dir() {
        file = file.join("index.html");
    }
    // **The fallback is what makes client-side routing work.** A reload on
    // /about asks for a file nobody wrote; the app's router is in the bundle
    // index.html loads. It applies only to paths that look like routes — a
    // missing .js answered with HTML is a syntax error three steps from its
    // cause, and a missing image should be a missing image.
    if !file.is_file() && Path::new(path).extension().is_none() {
        file = root.join("index.html");
    }
    let Ok(bytes) = std::fs::read(&file) else {
        let _ = respond(
            stream,
            "404 Not Found",
            "text/plain",
            &format!("no {path} in {}", root.display()),
        )
        .await;
        return;
    };
    let _ = respond_bytes(stream, content_type(&file), &bytes).await;
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
fn content_type(path: &Path) -> &'static str {
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
async fn respond_bytes(
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
