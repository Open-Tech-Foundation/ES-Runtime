//! Build failures reach live pages as overlay messages.
//!
//! No browser here: this speaks the update channel directly, asserting what
//! the overlay renders — the failure as the terminal prints it, without
//! colour, naming the file. That a page shows and clears it is `tests/hot.rs`
//! (a real Chromium); what arrives on the wire is asserted here, where no
//! browser is needed.
//!
//! The port is never pinned: `--port=0` takes any free one, and the test
//! reads which from the loop's own stderr.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use futures_util::StreamExt;

/// A frontend project with one script: breaking it breaks the build, and
/// nothing else has to go wrong first.
fn project(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("esdev-overlay-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).expect("create the fixture");
    std::fs::write(
        dir.join("src/main.mjs"),
        "document.getElementById(\"out\").textContent = \"ok\";\n",
    )
    .expect("write the entry");
    std::fs::write(
        dir.join("index.html"),
        "<!doctype html><html><head>\
         <script type=\"module\" src=\"./src/main.mjs\"></script></head>\
         <body><div id=out>pending</div></body></html>\n",
    )
    .expect("write the document");
    std::fs::write(
        dir.join("esdev.json"),
        r#"{"build": {"targets": {"web": {"entry": "index.html", "outdir": "dist"}}}}"#,
    )
    .expect("write the config");
    dir
}

/// A running dev loop, stopped however the test ends.
struct Loop {
    child: Child,
    dir: PathBuf,
}

impl Drop for Loop {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Loop {
    /// Starts the loop logging to a file, and reads the announced URL from
    /// it — the file stays readable after, whatever else the loop prints.
    fn start(dir: PathBuf) -> (Loop, u16) {
        let log = dir.join("loop-stderr.log");
        let log_file = std::fs::File::create(&log).expect("create the log");
        let mut child = Command::new(env!("CARGO_BIN_EXE_esdev"))
            .args(["start", "--port=0"])
            .current_dir(&dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(log_file)
            .spawn()
            .expect("spawn esdev start");
        let deadline = Instant::now() + Duration::from_secs(60);
        // One move at the end, outside any branch: the child changes hands
        // exactly once, whatever the loop saw.
        let port: u16 = loop {
            let text = std::fs::read_to_string(&log).unwrap_or_default();
            if let Some(at) = text.find("http://localhost:") {
                let port: Option<u16> = text[at + "http://localhost:".len()..]
                    .chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect::<String>()
                    .parse()
                    .ok();
                if let Some(port) = port {
                    break port;
                }
            }
            if Instant::now() > deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!(
                    "the loop never announced its URL:\n{}",
                    std::fs::read_to_string(&log).unwrap_or_default()
                );
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        (Loop { child, dir }, port)
    }
}

/// The next text frame on the update channel, or none before the deadline.
async fn next_frame(
    socket: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    timeout: Duration,
) -> Option<String> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(remaining, socket.next()).await {
            Ok(Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text)))) => {
                return Some(text.to_string());
            }
            Ok(Some(_)) => continue,
            _ => return None,
        }
    }
    None
}

/// Breaking the entry shows on the channel as an error naming the file —
/// and fixing it sends an update again, which is what clears the overlay.
#[tokio::test]
async fn a_failed_build_reaches_live_pages_as_an_error() {
    let dir = project("wire");
    let (_dev, port) = Loop::start(dir.clone());

    // The channel's path is protocol: pages already open reconnect at it.
    let stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect to the loop");
    let url = format!("ws://127.0.0.1:{port}/@esdev/hmr");
    let (mut socket, _) = tokio_tungstenite::client_async(url, stream)
        .await
        .expect("websocket handshake with the loop");

    std::fs::write(dir.join("src/main.mjs"), "export const broken = ;\n").expect("break the entry");
    let mut error = None;
    for _ in 0..60 {
        match next_frame(&mut socket, Duration::from_secs(2)).await {
            Some(text) if text.contains("\"type\":\"error\"") => {
                error = Some(text);
                break;
            }
            Some(_) => continue,
            None => break,
        }
    }
    let error = error.expect("no error frame for the broken build");
    assert!(error.contains("main.mjs"), "names the file: {error}");
    assert!(!error.contains('\x1b'), "paint leaked in: {error:?}");

    std::fs::write(
        dir.join("src/main.mjs"),
        "document.getElementById(\"out\").textContent = \"fixed\";\n",
    )
    .expect("fix the entry");
    let mut cleared = false;
    for _ in 0..60 {
        match next_frame(&mut socket, Duration::from_secs(2)).await {
            Some(text) if !text.contains("\"type\":\"error\"") => {
                cleared = true;
                break;
            }
            Some(_) => continue,
            None => break,
        }
    }
    assert!(cleared, "no update after the fix");
}

/// The first build can fail before any page exists. The current failure must
/// be replayed to a page that connects after the loop has reported ready.
#[tokio::test]
async fn an_initial_failed_build_reaches_late_pages_as_an_error() {
    let dir = project("initial");
    std::fs::write(dir.join("src/main.mjs"), "export const broken = ;\n")
        .expect("break the entry before startup");
    let (_dev, port) = Loop::start(dir.clone());

    let stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect to the loop");
    let url = format!("ws://127.0.0.1:{port}/@esdev/hmr");
    let (mut socket, _) = tokio_tungstenite::client_async(url, stream)
        .await
        .expect("websocket handshake with the loop");

    let error = next_frame(&mut socket, Duration::from_secs(5))
        .await
        .expect("the current initial error should be replayed");
    assert!(
        error.contains("\"type\":\"error\""),
        "not an error frame: {error}"
    );
    assert!(error.contains("main.mjs"), "names the file: {error}");

    std::fs::write(
        dir.join("src/main.mjs"),
        "document.getElementById(\"out\").textContent = \"fixed\";\n",
    )
    .expect("fix the entry");
    let update = next_frame(&mut socket, Duration::from_secs(10))
        .await
        .expect("a successful rebuild should clear the error");
    assert!(
        !update.contains("\"type\":\"error\""),
        "still an error: {update}"
    );
}
