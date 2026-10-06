//! `esdev preview` serves full static semantics, not just bytes.
//!
//! Drives the real binary the way a developer would — `esdev preview
//! --dir=<fixture>` — and asserts the wire: validators and `304`s, single
//! and multiple ranges, `416`s, `HEAD`, misses with and without `--spa`, and the traversal
//! and symlink refusals. Unit coverage of the parsing and decisions lives
//! beside the code (`static_serve.rs`); what is asserted here is that the
//! server built on it answers correctly end to end.
//!
//! The preview port is never pinned: `--port=0` takes any free one, and the
//! test reads which from the command's own stderr. A fixed port would collide
//! with whatever else is running, and the symptom would not read as one.

// A test reporting why it skipped is talking to whoever reads the run.
#![allow(clippy::print_stderr)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// Deterministic fixture bytes: 0..255 four times over.
fn bytes() -> Vec<u8> {
    (0..1024u32).map(|i| (i % 256) as u8).collect()
}

fn test_registry() -> &'static str {
    static REGISTRY: OnceLock<String> = OnceLock::new();
    REGISTRY
        .get_or_init(|| {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind test registry");
            let address = listener.local_addr().expect("test registry address");
            std::thread::spawn(move || {
                for incoming in listener.incoming() {
                    let Ok(mut stream) = incoming else { continue };
                    std::thread::spawn(move || {
                        let Ok(clone) = stream.try_clone() else { return };
                        let mut reader = BufReader::new(clone);
                        let mut line = String::new();
                        loop {
                            line.clear();
                            if reader.read_line(&mut line).is_err()
                                || line.is_empty()
                                || line == "\r\n"
                            {
                                break;
                            }
                        }
                        let body = r#"{"dist-tags":{"latest":"1.2.3"}}"#;
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = stream.write_all(response.as_bytes());
                    });
                }
            });
            format!("http://{address}")
        })
        .as_str()
}

fn fixture(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "esdev-preview-static-{}-{}",
        std::process::id(),
        name
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sub")).expect("fixture dirs");
    std::fs::write(dir.join("sub").join("index.html"), "<h1>sub</h1>").expect("sub index");
    std::fs::write(dir.join("index.html"), "<h1>site</h1>").expect("index");
    std::fs::write(dir.join("app.js"), "console.log(1);").expect("js");
    std::fs::write(dir.join("space name.txt"), "encoded path").expect("encoded path file");
    std::fs::write(dir.join("data.bin"), bytes()).expect("bytes");
    std::fs::write(dir.join("empty.txt"), b"").expect("empty");
    std::fs::write(dir.join("payload.bin"), "console.log('aliased');").expect("payload");

    // Large enough to cross many 64 KiB copy chunks with a remainder, so a
    // streamed body proves itself byte-exact rather than merely short.
    let big: Vec<u8> = (0..5 * 1024 * 1024 + 12345u32)
        .map(|i| (i % 251) as u8)
        .collect();
    std::fs::write(dir.join("big.bin"), &big).expect("large fixture");
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(dir.join("data.bin"), dir.join("ok-link.bin"))
            .expect("a symlink that stays inside");
        // Same target, JavaScript name: the alias must type as asked, not as
        // stored, or browsers refuse the script.
        std::os::unix::fs::symlink(dir.join("payload.bin"), dir.join("alias.js"))
            .expect("a symlink alias with its own extension");
        // A link *directory* pointing outside: the escape the final-component
        // check cannot see, since `passwd` itself is a regular file.
        std::os::unix::fs::symlink(outside_dir(&dir), dir.join("linkdir"))
            .expect("a symlink directory pointing outside");
        // Points outside no matter how it is read: `/etc/hostname` may not
        // exist everywhere, so the test accepts either refusal below.
        std::os::unix::fs::symlink("/etc/hostname", dir.join("far-link.txt")).ok();
        // A FIFO must never be opened: opening one blocks until a writer
        // arrives, which would hang the connection. Skip where mkfifo is
        // unavailable rather than failing the suite for the fixture.
        if std::process::Command::new("mkfifo")
            .arg(dir.join("fifo.bin"))
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            std::fs::write(dir.join("fifo-present"), b"").expect("fifo marker");
        }
    }
    dir
}

/// A directory beside the fixture root, never inside it.
fn outside_dir(dir: &Path) -> PathBuf {
    let outside = dir.with_extension("outside");
    let _ = std::fs::remove_dir_all(&outside);
    std::fs::create_dir_all(&outside).expect("outside dir");
    std::fs::write(outside.join("secret.txt"), "secret").expect("outside file");
    outside
}

/// A running preview, stopped however the test ends.
struct Preview {
    child: Child,
    dir: PathBuf,
    // Drains the child's stderr for the test's duration: dropping the pipe
    // after the port is parsed would leave a closed read end behind, and the
    // first later write (a panic message included) would SIGPIPE the server
    // out from under the test. The log names the culprit instead.
    _stderr: std::thread::JoinHandle<()>,
}

impl Drop for Preview {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            let _ = Command::new("kill")
                .args(["-TERM", &self.child.id().to_string()])
                .status();
            let deadline = Instant::now() + Duration::from_secs(15);
            while Instant::now() < deadline {
                if let Ok(Some(_)) = self.child.try_wait() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        #[cfg(not(unix))]
        {
            let _ = self.child.kill();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
        let _ = std::fs::remove_dir_all(outside_dir(&self.dir));
    }
}

fn start(dir: &Path) -> (Preview, u16) {
    start_with(dir, &[])
}

fn start_with(dir: &Path, extra: &[String]) -> (Preview, u16) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_esdev"))
        .args([
            "preview".to_string(),
            format!("--dir={}", dir.display()),
            "--port=0".to_string(),
        ])
        .args(extra)
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn esdev preview");
    let stderr = child.stderr.take().expect("stderr");
    let mut lines = BufReader::new(stderr).lines();
    let mut port = None;
    for line in lines.by_ref().map_while(Result::ok) {
        if let Some(at) = line.find("http://localhost:") {
            let digits: String = line[at + "http://localhost:".len()..]
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if let Ok(p) = digits.parse::<u16>() {
                port = Some(p);
                break;
            }
        }
    }
    let port = port.expect("preview never named its port");
    let log = dir.join("server-stderr.log");
    let _stderr = std::thread::spawn(move || {
        use std::io::Write;
        let mut out = std::fs::File::create(&log).expect("stderr log");
        for line in lines.by_ref().map_while(Result::ok) {
            let _ = writeln!(out, "{line}");
        }
    });
    (
        Preview {
            child,
            dir: dir.to_path_buf(),
            _stderr,
        },
        port,
    )
}

struct Answer {
    status: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

fn get(port: u16, request: &str) -> Answer {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("timeout");
    stream.write_all(request.as_bytes()).expect("request");
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).expect("response");
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("a head");
    let head = String::from_utf8(raw[..split].to_vec()).expect("head text");
    let mut lines = head.lines();
    let status = lines.next().unwrap_or("").to_string();
    let mut headers = HashMap::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }
    Answer {
        status,
        headers,
        body: raw[split + 4..].to_vec(),
    }
}

fn req(method: &str, path: &str, extra: &[&str]) -> String {
    let mut r = format!("{method} {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n");
    for h in extra {
        r.push_str(h);
        r.push_str("\r\n");
    }
    r.push_str("\r\n");
    r
}

/// Waits for the preview's stderr log (after its URL line) to mention `text`.
fn logged(dir: &Path, text: &str) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if std::fs::read_to_string(dir.join("server-stderr.log"))
            .is_ok_and(|log| log.contains(text))
        {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

fn status(a: &Answer) -> &str {
    a.status.split_whitespace().nth(1).unwrap_or("")
}

#[test]
fn files_serve_with_validators() {
    const NAME: &str = "files_serve_with_validators";
    let dir = fixture(NAME);
    let (preview, port) = start(&dir);
    let _preview = preview;
    let a = get(port, &req("GET", "/data.bin", &[]));
    assert_eq!(status(&a), "200", "{}", a.status);
    assert_eq!(
        a.headers.get("accept-ranges").map(String::as_str),
        Some("bytes")
    );
    assert_eq!(
        a.headers.get("cache-control").map(String::as_str),
        Some("no-cache")
    );
    let etag = a.headers.get("etag").expect("an etag").clone();
    assert!(etag.starts_with("W/\""), "weak: {etag}");
    let last = a
        .headers
        .get("last-modified")
        .expect("a last-modified")
        .clone();
    assert!(last.ends_with("GMT"), "imf-fixdate: {last}");
    assert_eq!(a.body, bytes());

    // Revalidation answers without resending.
    let b = get(
        port,
        &req("GET", "/data.bin", &[&format!("If-None-Match: {etag}")]),
    );
    assert_eq!(status(&b), "304");
    assert!(b.body.is_empty());
    let c = get(port, &req("GET", "/data.bin", &["If-None-Match: *"]));
    assert_eq!(status(&c), "304");
    let d = get(
        port,
        &req("GET", "/data.bin", &[&format!("If-Modified-Since: {last}")]),
    );
    assert_eq!(status(&d), "304");

    // Stale validators fall through to the file.
    let e = get(
        port,
        &req("GET", "/data.bin", &["If-None-Match: W/\"0-0\""]),
    );
    assert_eq!(status(&e), "200");
    assert_eq!(e.body, bytes());
    let f = get(
        port,
        &req(
            "GET",
            "/data.bin",
            &["If-Modified-Since: Thu, 01 Jan 1970 00:00:00 GMT"],
        ),
    );
    assert_eq!(status(&f), "200");
    // An extreme year is rejected, not computed — and never panics the server.
    let g = get(
        port,
        &req(
            "GET",
            "/data.bin",
            &["If-Modified-Since: Sun, 06 Nov 9999999999 08:49:37 GMT"],
        ),
    );
    assert_eq!(status(&g), "200");
    assert_eq!(g.body, bytes());
    // An impossible calendar day is no validator either.
    let h = get(
        port,
        &req(
            "GET",
            "/data.bin",
            &["If-Modified-Since: Sat, 31 Feb 2024 08:49:37 GMT"],
        ),
    );
    assert_eq!(status(&h), "200");
    assert_eq!(h.body, bytes());
}

#[test]
fn encoded_path_segments_resolve_to_files() {
    const NAME: &str = "encoded_path_segments_resolve_to_files";
    let dir = fixture(NAME);
    let (preview, port) = start(&dir);
    let _preview = preview;
    let a = get(port, &req("GET", "/space%20name.txt", &[]));
    assert_eq!(status(&a), "200", "{}", a.status);
    assert_eq!(a.body, b"encoded path");
}
#[test]
fn a_rapid_same_size_rewrite_changes_the_etag() {
    const NAME: &str = "a_rapid_same_size_rewrite_changes_the_etag";
    let dir = fixture(NAME);
    let (preview, port) = start(&dir);
    let _preview = preview;
    let first = get(port, &req("GET", "/data.bin", &[]));
    let etag1 = first.headers.get("etag").expect("an etag").clone();
    // Same bytes, rewritten at once: size and second are unchanged, so only
    // sub-second precision can tell the generations apart.
    std::fs::write(dir.join("data.bin"), bytes()).expect("rewrite");
    let second = get(port, &req("GET", "/data.bin", &[]));
    let etag2 = second.headers.get("etag").expect("an etag").clone();
    assert_ne!(etag1, etag2, "a rewrite mints a new validator");
    // And the old one no longer revalidates.
    let stale = get(
        port,
        &req("GET", "/data.bin", &[&format!("If-None-Match: {etag1}")]),
    );
    assert_eq!(status(&stale), "200");
}

#[test]
fn head_answers_lengths_without_bodies() {
    const NAME: &str = "head_answers_lengths_without_bodies";
    let dir = fixture(NAME);
    let (preview, port) = start(&dir);
    let _preview = preview;
    let a = get(port, &req("HEAD", "/app.js", &[]));
    assert_eq!(status(&a), "200");
    assert_eq!(
        a.headers.get("content-length").map(String::as_str),
        Some("15")
    );
    assert!(a.body.is_empty());
    // Ranges on a HEAD answer the same headers a GET would, still bodiless.
    let b = get(port, &req("HEAD", "/data.bin", &["Range: bytes=0-9"]));
    assert_eq!(status(&b), "206");
    assert_eq!(
        b.headers.get("content-range").map(String::as_str),
        Some("bytes 0-9/1024")
    );
    assert!(b.body.is_empty());
}

#[test]
fn ranges_slice() {
    const NAME: &str = "ranges_slice";
    let dir = fixture(NAME);
    let (preview, port) = start(&dir);
    let _preview = preview;
    let full = bytes();
    let a = get(port, &req("GET", "/data.bin", &["Range: bytes=0-9"]));
    assert_eq!(status(&a), "206");
    assert_eq!(
        a.headers.get("content-range").map(String::as_str),
        Some("bytes 0-9/1024")
    );
    assert_eq!(a.body, full[0..10]);
    // Suffix, open-ended, and clamped past the end.
    let b = get(port, &req("GET", "/data.bin", &["Range: bytes=-10"]));
    assert_eq!(status(&b), "206");
    assert_eq!(b.body, full[1014..1024]);
    let c = get(port, &req("GET", "/data.bin", &["Range: bytes=1000-"]));
    assert_eq!(c.body, full[1000..1024]);
    let d = get(port, &req("GET", "/data.bin", &["Range: bytes=0-99999"]));
    assert_eq!(status(&d), "206");
    assert_eq!(d.body, full);
    // What the transport does not understand is answered in full — including
    // a malformed range past the end, which is invalid rather than
    // unsatisfiable.
    let e = get(port, &req("GET", "/data.bin", &["Range: items=0-9"]));
    assert_eq!(status(&e), "200");
    assert_eq!(e.body, full);
    let f = get(
        port,
        &req("GET", "/data.bin", &["Range: bytes=999999-nope"]),
    );
    assert_eq!(status(&f), "200");
    assert_eq!(f.body, full);
    // Duplicate full-file ranges coalesce to one span: the ask cannot
    // multiply what the server holds, and one span is a plain 206.
    let g = get(
        port,
        &req("GET", "/data.bin", &["Range: bytes=0-1023, 0-1023"]),
    );
    assert_eq!(status(&g), "206");
    assert_eq!(
        g.headers.get("content-range").map(String::as_str),
        Some("bytes 0-1023/1024")
    );
    assert_eq!(g.body, full);
}

#[test]
fn multiple_ranges_come_back_multipart() {
    const NAME: &str = "multiple_ranges_come_back_multipart";
    let dir = fixture(NAME);
    let (preview, port) = start(&dir);
    let _preview = preview;
    let a = get(port, &req("GET", "/data.bin", &["Range: bytes=0-1, 10-11"]));
    assert_eq!(status(&a), "206");
    let ctype = a
        .headers
        .get("content-type")
        .expect("a content type")
        .clone();
    let boundary = ctype
        .split("boundary=")
        .nth(1)
        .expect("a multipart boundary");
    let text = String::from_utf8_lossy(&a.body);
    assert!(text.starts_with(&format!("--{boundary}\r\n")));
    assert!(text.contains("Content-Range: bytes 0-1/1024"));
    assert!(text.contains("Content-Range: bytes 10-11/1024"));
    assert!(text.ends_with(&format!("--{boundary}--\r\n")));
    let full = bytes();
    assert!(a.body.windows(2).any(|w| w == &full[0..2]));
    assert!(a.body.windows(2).any(|w| w == &full[10..12]));
    // A HEAD over the same ranges reports the GET length. Boundaries are
    // per-response unique, so the comparison adjusts for their lengths.
    let h = get(
        port,
        &req("HEAD", "/data.bin", &["Range: bytes=0-1, 10-11"]),
    );
    assert_eq!(status(&h), "206");
    let htype = h
        .headers
        .get("content-type")
        .expect("a content type")
        .clone();
    let hboundary = htype.split("boundary=").nth(1).expect("a boundary");
    let hlen: usize = h
        .headers
        .get("content-length")
        .expect("a length")
        .parse()
        .expect("a number");
    assert_eq!(
        hlen,
        a.body.len() - boundary.len() + hboundary.len(),
        "HEAD length is the GET length under its own boundary"
    );
    assert!(h.body.is_empty());
}

#[test]
fn unsatisfiable_ranges_and_conditional_ranges() {
    const NAME: &str = "unsatisfiable_ranges_and_conditional_ranges";
    let dir = fixture(NAME);
    let (preview, port) = start(&dir);
    let _preview = preview;
    let a = get(port, &req("GET", "/data.bin", &["Range: bytes=5000-6000"]));
    assert_eq!(status(&a), "416");
    assert_eq!(
        a.headers.get("content-range").map(String::as_str),
        Some("bytes */1024")
    );
    // An empty file satisfies nothing.
    let b = get(port, &req("GET", "/empty.txt", &["Range: bytes=0-"]));
    assert_eq!(status(&b), "416");
    // A stale If-Range restores the file; a fresh one keeps the slice.
    let full = get(port, &req("GET", "/data.bin", &[]));
    let last = full.headers.get("last-modified").expect("lm").clone();
    let c = get(
        port,
        &req(
            "GET",
            "/data.bin",
            &["Range: bytes=0-1", "If-Range: W/\"0-0\""],
        ),
    );
    assert_eq!(status(&c), "200");
    assert_eq!(c.body, bytes());
    let d = get(
        port,
        &req(
            "GET",
            "/data.bin",
            &["Range: bytes=0-1", &format!("If-Range: {last}")],
        ),
    );
    assert_eq!(status(&d), "206");
    assert_eq!(d.body, bytes()[0..2]);
}

#[test]
fn routing_fallbacks_and_refusals_hold() {
    const NAME: &str = "routing_fallbacks_and_refusals_hold";
    let dir = fixture(NAME);
    let (preview, port) = start_with(&dir, &["--spa".to_string()]);
    let _preview = preview;
    // With --spa, extensionless misses fall back to the app shell; real files 404 —
    // while a directory serves its own index rather than falling back.
    let a = get(port, &req("GET", "/about", &[]));
    assert_eq!(status(&a), "200");
    assert_eq!(a.body, b"<h1>site</h1>");
    let sub = get(port, &req("GET", "/sub/", &[]));
    assert_eq!(status(&sub), "200");
    assert_eq!(sub.body, b"<h1>sub</h1>");
    let b = get(port, &req("GET", "/missing.js", &[]));
    assert_eq!(status(&b), "404");
    // Only reads are served.
    let c = get(port, &req("POST", "/app.js", &[]));
    assert_eq!(status(&c), "405");
    assert_eq!(
        c.headers.get("allow").map(String::as_str),
        Some("GET, HEAD")
    );
    // Errors honour HEAD too: the headers a GET would name, no body bytes.
    let h = get(port, &req("HEAD", "/missing.js", &[]));
    assert_eq!(status(&h), "404");
    assert!(h.body.is_empty());
    let hlen: usize = h
        .headers
        .get("content-length")
        .expect("a length")
        .parse()
        .expect("a number");
    assert!(hlen > 0, "the length its GET would send");
    let h400 = get(port, &req("HEAD", "/../../etc/hostname", &[]));
    assert_eq!(status(&h400), "400");
    assert!(h400.body.is_empty());
    // Every answer carries the clock: HTTP wants Date on 2xx–4xx.
    let ok = get(port, &req("GET", "/app.js", &[]));
    assert!(
        ok.headers
            .get("date")
            .map(|d| d.ends_with("GMT"))
            .unwrap_or(false)
    );
    assert!(h.headers.contains_key("date"));
    // Climbing out is refused before the filesystem is touched.
    let d = get(port, &req("GET", "/../../etc/hostname", &[]));
    assert_eq!(status(&d), "400");
    // The update channel is a GET-only upgrade: HEAD answers bodilessly,
    // and anything else is refused rather than upgraded. Every 426 names
    // the protocol it wants.
    let hmr_head = get(port, &req("HEAD", "/@esdev/hmr", &[]));
    assert_eq!(status(&hmr_head), "426");
    assert!(hmr_head.body.is_empty());
    assert_eq!(
        hmr_head.headers.get("upgrade").map(String::as_str),
        Some("websocket")
    );
    let hmr_get = get(port, &req("GET", "/@esdev/hmr", &[]));
    assert_eq!(status(&hmr_get), "426");
    assert!(!hmr_get.body.is_empty());
    assert_eq!(
        hmr_get.headers.get("upgrade").map(String::as_str),
        Some("websocket")
    );
    let hmr_missing_connection_upgrade = get(
        port,
        &req(
            "GET",
            "/@esdev/hmr",
            &[
                "Upgrade: websocket",
                "Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==",
                "Sec-WebSocket-Version: 13",
            ],
        ),
    );
    assert_eq!(status(&hmr_missing_connection_upgrade), "426");
    let hmr_missing_version = get(
        port,
        &req(
            "GET",
            "/@esdev/hmr",
            &[
                "Upgrade: websocket",
                "Connection: Upgrade",
                "Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==",
            ],
        ),
    );
    assert_eq!(status(&hmr_missing_version), "426");
    let hmr_http_10 = req(
        "GET",
        "/@esdev/hmr",
        &[
            "Upgrade: websocket",
            "Connection: Upgrade",
            "Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==",
            "Sec-WebSocket-Version: 13",
        ],
    )
    .replacen("HTTP/1.1", "HTTP/1.0", 1);
    let hmr_http_10 = get(port, &hmr_http_10);
    assert_eq!(status(&hmr_http_10), "426");
    let hmr_post = get(
        port,
        &req(
            "POST",
            "/@esdev/hmr",
            &[
                "Upgrade: websocket",
                "Connection: Upgrade",
                "Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==",
                "Sec-WebSocket-Version: 13",
            ],
        ),
    );
    assert_eq!(status(&hmr_post), "405", "{}", hmr_post.status);
    assert_eq!(
        hmr_post.headers.get("allow").map(String::as_str),
        Some("GET, HEAD")
    );
}

#[test]
fn a_miss_is_a_404_by_default() {
    const NAME: &str = "a_miss_is_a_404_by_default";
    let dir = fixture(NAME);
    let (preview, port) = start(&dir);
    let _preview = preview;
    // The default is said at startup, with the flag that changes it.
    assert!(
        logged(&dir, "static: a missing path is a 404 (--spa"),
        "no startup line naming the miss behaviour"
    );
    // No fallback without --spa: a route-like miss is a miss.
    let route = get(port, &req("GET", "/about", &[]));
    assert_eq!(status(&route), "404");
    assert_ne!(route.body, b"<h1>site</h1>");
    assert_eq!(status(&get(port, &req("GET", "/missing.js", &[]))), "404");
    // What exists is still served, directories by their index.
    assert_eq!(get(port, &req("GET", "/", &[])).body, b"<h1>site</h1>");
    assert_eq!(get(port, &req("GET", "/sub/", &[])).body, b"<h1>sub</h1>");
}

#[test]
fn a_site_404_page_answers_every_miss_with_404() {
    const NAME: &str = "a_site_404_page_answers_every_miss_with_404";
    let dir = fixture(NAME);
    std::fs::write(dir.join("404.html"), "<h1>not here</h1>").expect("404 page");
    let (preview, port) = start(&dir);
    let _preview = preview;
    assert!(
        logged(&dir, "static: a missing path gets 404.html with status 404"),
        "no startup line naming the site's 404 page"
    );
    // A route-like miss gets the site's page, not the index, and says 404.
    let route = get(port, &req("GET", "/missing/page", &[]));
    assert_eq!(status(&route), "404");
    assert_eq!(route.body, b"<h1>not here</h1>");
    assert_eq!(
        route.headers.get("content-type").map(String::as_str),
        Some("text/html; charset=utf-8")
    );
    assert_eq!(
        route.headers.get("cache-control").map(String::as_str),
        Some("no-store")
    );
    // So does a missing asset.
    let asset = get(port, &req("GET", "/missing.js", &[]));
    assert_eq!(status(&asset), "404");
    assert_eq!(asset.body, b"<h1>not here</h1>");
    // HEAD: the length a GET would send, no body.
    let head = get(port, &req("HEAD", "/missing/page", &[]));
    assert_eq!(status(&head), "404");
    assert!(head.body.is_empty());
    assert_eq!(
        head.headers.get("content-length").map(String::as_str),
        Some("17")
    );
    // What exists is still served, and the page itself answers 200.
    assert_eq!(status(&get(port, &req("GET", "/", &[]))), "200");
    assert_eq!(status(&get(port, &req("GET", "/sub/", &[]))), "200");
    assert_eq!(status(&get(port, &req("GET", "/404.html", &[]))), "200");
    // Refusals keep their own status.
    let up = get(port, &req("GET", "/../../etc/hostname", &[]));
    assert_eq!(status(&up), "400");
}

#[test]
fn spa_routes_reach_the_index_even_beside_a_404_page() {
    const NAME: &str = "spa_routes_reach_the_index_even_beside_a_404_page";
    let dir = fixture(NAME);
    std::fs::write(dir.join("404.html"), "<h1>not here</h1>").expect("404 page");
    let (preview, port) = start_with(&dir, &["--spa".to_string()]);
    let _preview = preview;
    assert!(
        logged(
            &dir,
            "--spa: a missing path without an extension gets index.html"
        ),
        "no startup line for --spa"
    );
    // The flag says the output is an app: its router owns every route.
    let route = get(port, &req("GET", "/missing/page", &[]));
    assert_eq!(status(&route), "200");
    assert_eq!(route.body, b"<h1>site</h1>");
    assert_eq!(status(&get(port, &req("GET", "/missing.js", &[]))), "404");
}

#[test]
#[cfg(unix)]
fn symlinks_cannot_leave_the_root() {
    const NAME: &str = "symlinks_cannot_leave_the_root";
    let dir = fixture(NAME);
    let (preview, port) = start(&dir);
    let _preview = preview;
    let inside = get(port, &req("GET", "/ok-link.bin", &[]));
    assert_eq!(status(&inside), "200");
    assert_eq!(inside.body, bytes());
    let alias = get(port, &req("GET", "/alias.js", &[]));
    assert_eq!(status(&alias), "200");
    assert_eq!(
        alias.headers.get("content-type").map(String::as_str),
        Some("text/javascript; charset=utf-8"),
        "typed by the name asked for, not the target stored"
    );
    assert_eq!(alias.body, b"console.log('aliased');");
    // A link in a *parent* directory: the final component is an ordinary
    // file, so only the settled-path check can refuse it.
    let escape = get(port, &req("GET", "/linkdir/secret.txt", &[]));
    assert_eq!(status(&escape), "403", "{}", escape.status);
    let far = get(port, &req("GET", "/far-link.txt", &[]));
    assert!(
        status(&far) == "403" || status(&far) == "404",
        "{}",
        far.status
    );
}

#[test]
#[cfg(unix)]
fn a_fifo_answers_instead_of_hanging() {
    const NAME: &str = "a_fifo_answers_instead_of_hanging";
    let dir = fixture(NAME);
    if !dir.join("fifo-present").is_file() {
        eprintln!("SKIP: mkfifo unavailable");
        return;
    }
    let (preview, port) = start(&dir);
    let _preview = preview;
    // Would block in `read_to_end` for the full timeout if the server
    // opened the FIFO: the open blocks until a writer arrives.
    let fifo = get(port, &req("GET", "/fifo.bin", &[]));
    assert_eq!(status(&fifo), "404", "{}", fifo.status);
    // And the server is still answering afterwards.
    let again = get(port, &req("GET", "/app.js", &[]));
    assert_eq!(status(&again), "200");
}

#[test]
fn large_bodies_stream_byte_exact() {
    const NAME: &str = "large_bodies_stream_byte_exact";
    let dir = fixture(NAME);
    let (preview, port) = start(&dir);
    let _preview = preview;
    let big: Vec<u8> = (0..5 * 1024 * 1024 + 12345u32)
        .map(|i| (i % 251) as u8)
        .collect();
    // A whole 5 MiB body, streamed rather than buffered server-side.
    let full = get(port, &req("GET", "/big.bin", &[]));
    assert_eq!(status(&full), "200");
    assert_eq!(full.body.len(), big.len());
    assert_eq!(full.body, big);
    // A range straddling copy-chunk boundaries.
    let span = get(port, &req("GET", "/big.bin", &["Range: bytes=60000-70000"]));
    assert_eq!(status(&span), "206");
    assert_eq!(span.body, big[60000..70001]);
}

/// A scaffolded project previews what it built: `create` writes it, `build`
/// writes `dist/`, and the release output serves with an entry document.
///
/// Vanilla uses a local registry fixture: a test that downloads the internet
/// is a test that flakes on it. stdin is closed throughout, so the scaffold
/// takes its defaults.
#[test]
fn a_scaffolded_project_previews_what_it_built() {
    let parent =
        std::env::temp_dir().join(format!("esdev-preview-scaffold-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&parent);
    std::fs::create_dir_all(&parent).expect("scaffold parent");

    let created = Command::new(env!("CARGO_BIN_EXE_esdev"))
        .args(["create", "shop", "--template=vanilla", "--no-install"])
        .current_dir(&parent)
        .stdin(Stdio::null())
        .env("npm_config_registry", test_registry())
        .env("NPM_CONFIG_REGISTRY", test_registry())
        .output()
        .expect("spawn esdev create");
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );
    let dir = parent.join("shop");

    let built = Command::new(env!("CARGO_BIN_EXE_esdev"))
        .arg("build")
        .current_dir(&dir)
        .stdin(Stdio::null())
        .output()
        .expect("spawn esdev build");
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    assert!(dir.join("dist/index.html").is_file(), "no built site");

    let dist = dir.join("dist");
    let (preview, port) = start(&dist);
    let _preview = preview;
    let index = get(port, &req("GET", "/", &[]));
    assert_eq!(status(&index), "200", "{}", index.status);
    let body = String::from_utf8(index.body).expect("the document is text");
    assert!(body.contains("<title>shop</title>"), "{body}");
    assert!(index.headers.contains_key("etag"), "no validator");

    let _ = std::fs::remove_dir_all(&parent);
}

/// A stand-in API server: it answers every request with what it received —
/// the request line, `Host`, `X-Forwarded-Host` and the body — and a
/// WebSocket-style upgrade with `101`, then echoes whatever bytes follow.
fn api_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind the API server");
    let port = listener.local_addr().expect("address").port();
    std::thread::spawn(move || {
        for incoming in listener.incoming() {
            let Ok(mut stream) = incoming else { continue };
            std::thread::spawn(move || {
                let Ok(clone) = stream.try_clone() else {
                    return;
                };
                let mut reader = BufReader::new(clone);
                let mut head = Vec::new();
                let mut line = String::new();
                loop {
                    line.clear();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    head.push(line.trim_end().to_string());
                }
                let header = |name: &str| {
                    head.iter()
                        .skip(1)
                        .find_map(|l| {
                            let (n, v) = l.split_once(':')?;
                            n.trim()
                                .eq_ignore_ascii_case(name)
                                .then(|| v.trim().to_string())
                        })
                        .unwrap_or_default()
                };
                if header("upgrade").eq_ignore_ascii_case("websocket") {
                    let _ = stream.write_all(
                        b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n",
                    );
                    let mut buf = [0u8; 64];
                    while let Ok(n) = reader.read(&mut buf) {
                        if n == 0 || stream.write_all(&buf[..n]).is_err() {
                            break;
                        }
                    }
                    return;
                }
                let length: usize = header("content-length").parse().unwrap_or(0);
                let mut body = vec![0u8; length];
                let _ = reader.read_exact(&mut body);
                let echo = format!(
                    "{}\nhost={}\nforwarded-host={}\nconnection={}\nbody={}",
                    head[0],
                    header("host"),
                    header("x-forwarded-host"),
                    header("connection"),
                    String::from_utf8_lossy(&body)
                );
                let _ = stream.write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{echo}",
                        echo.len()
                    )
                    .as_bytes(),
                );
            });
        }
    });
    port
}

/// `dev.server.proxy` sends a path prefix to another server and leaves every
/// other path to the files (D150).
#[test]
fn a_path_prefix_is_forwarded_to_the_api_server() {
    const NAME: &str = "proxy_forwards";
    let dir = fixture(NAME);
    let api = api_server();
    // A port nothing listens on: bound, read, and let go.
    let closed = TcpListener::bind("127.0.0.1:0")
        .expect("bind")
        .local_addr()
        .expect("address")
        .port();
    let config = dir.with_extension("esdev.json");
    std::fs::write(
        &config,
        format!(
            r#"{{ "dev": {{ "server": {{ "proxy": {{
                "/api": "http://127.0.0.1:{api}/v1",
                "/down": "http://127.0.0.1:{closed}"
            }} }} }}, "build": {{ "targets": {{ "web": {{ "entry": "index.html" }} }} }} }}"#
        ),
    )
    .expect("config");
    let (preview, port) = start_with(&dir, &[format!("--config={}", config.display())]);
    let _preview = preview;

    let a = get(port, &req("GET", "/api/users?page=2", &[]));
    assert_eq!(status(&a), "200", "{}", a.status);
    let echo = String::from_utf8_lossy(&a.body).into_owned();
    assert!(
        echo.starts_with("GET /v1/api/users?page=2 HTTP/1.1"),
        "{echo}"
    );
    assert!(echo.contains(&format!("host=127.0.0.1:{api}")), "{echo}");
    assert!(echo.contains("forwarded-host=x"), "{echo}");
    assert!(echo.contains("connection=close"), "{echo}");

    let posted = get(
        port,
        "POST /api/items HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\n\
         Content-Length: 10\r\nConnection: close\r\n\r\n{\"a\":true}",
    );
    assert_eq!(status(&posted), "200", "{}", posted.status);
    assert!(
        String::from_utf8_lossy(&posted.body).ends_with("body={\"a\":true}"),
        "{}",
        String::from_utf8_lossy(&posted.body)
    );

    // Not a prefix: the preview's own files.
    let page = get(port, &req("GET", "/index.html", &[]));
    assert_eq!(status(&page), "200");
    assert_eq!(page.body, b"<h1>site</h1>");

    // A target that is not running is a 502 that says which.
    let down = get(port, &req("GET", "/down/x", &[]));
    assert_eq!(status(&down), "502", "{}", down.status);
    assert!(
        String::from_utf8_lossy(&down.body).contains(&format!("127.0.0.1:{closed}")),
        "{}",
        String::from_utf8_lossy(&down.body)
    );

    // An upgrade is a tunnel: the 101 comes back, and bytes go both ways.
    let mut ws = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    ws.set_read_timeout(Some(Duration::from_secs(10)))
        .expect("timeout");
    ws.write_all(
        b"GET /api/socket HTTP/1.1\r\nHost: x\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
          Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n",
    )
    .expect("upgrade");
    let mut reader = BufReader::new(ws.try_clone().expect("clone"));
    let mut status_line = String::new();
    reader.read_line(&mut status_line).expect("status");
    assert!(status_line.starts_with("HTTP/1.1 101"), "{status_line}");
    let mut line = String::new();
    loop {
        line.clear();
        reader.read_line(&mut line).expect("header");
        if line == "\r\n" {
            break;
        }
    }
    ws.write_all(b"ping").expect("send");
    let mut echoed = [0u8; 4];
    reader.read_exact(&mut echoed).expect("echo");
    assert_eq!(&echoed, b"ping");

    let _ = std::fs::remove_file(&config);
}
