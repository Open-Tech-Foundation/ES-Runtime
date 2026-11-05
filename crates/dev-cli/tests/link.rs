//! The link between `esdev start` and its server (DECISIONS D207), end to end:
//! a server's unhandled error reaches live pages, a restart stops the server
//! through the link, and a server whose `esdev start` is killed exits too.
//!
//! The update channel's port is read from the page the dev build wrote — it is
//! the address the page itself connects to.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::{Duration, Instant};

use futures_util::StreamExt;

const SERVER: &str = r#"import { serve } from "runtime:http";
import { env } from "runtime:process";
serve({ hostname: "127.0.0.1", port: Number(env.PORT) }, (request) => {
  if (new URL(request.url).pathname === "/boom") throw new RangeError("the ledger exploded");
  return new Response("ok");
});
"#;

/// A full-stack project: a page, and a server the loop runs.
fn project(name: &str) -> PathBuf {
    static NEXT_PORT: AtomicU16 = AtomicU16::new(21_500);
    let app = NEXT_PORT.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("esdev-link-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).expect("create the fixture");
    std::fs::write(dir.join("src/server.mjs"), SERVER).expect("write the server");
    std::fs::write(dir.join("src/main.mjs"), "document.title = 'linked';\n")
        .expect("write the page");
    std::fs::write(
        dir.join("index.html"),
        "<!doctype html><html><head>\
         <script type=\"module\" src=\"./src/main.mjs\"></script></head><body></body></html>\n",
    )
    .expect("write the document");
    std::fs::write(
        dir.join("esdev.json"),
        format!(
            r#"{{ "build": {{ "targets": {{
                  "web": {{ "entry": "index.html", "outdir": "dist/web" }},
                  "server": {{ "entry": "src/server.mjs", "out": "dist/server.js" }} }} }},
                "dev": {{ "run": "server", "app": {{ "port": {app} }} }} }}"#
        ),
    )
    .expect("write the config");
    dir
}

struct Loop {
    child: Child,
    dir: PathBuf,
    log: PathBuf,
}

impl Drop for Loop {
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
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Loop {
    fn start(dir: PathBuf) -> Loop {
        let log = dir.join("loop.log");
        let child = Command::new(env!("CARGO_BIN_EXE_esdev"))
            .arg("start")
            .current_dir(&dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(&log).expect("create the log"))
            .spawn()
            .expect("spawn esdev start");
        Loop { child, dir, log }
    }

    fn log(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    /// The app's port, once it answers.
    fn app(&self) -> u16 {
        let deadline = Instant::now() + Duration::from_secs(120);
        while Instant::now() < deadline {
            let log = self.log();
            if let Some(at) = log.find("the app is on http://localhost:") {
                let port: u16 = log[at + "the app is on http://localhost:".len()..]
                    .chars()
                    .take_while(char::is_ascii_digit)
                    .collect::<String>()
                    .parse()
                    .expect("a port");
                if get(port, "/").contains("200 OK") {
                    return port;
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("the app never answered:\n{}", self.log());
    }

    /// The update channel's port, from the page the dev build wrote.
    fn channel(&self) -> u16 {
        let page = find_html(&self.dir.join(".dev")).expect("a dev page");
        let text = std::fs::read_to_string(page).expect("read the page");
        let at = text.find("ws://127.0.0.1:").expect("the update channel");
        text[at + "ws://127.0.0.1:".len()..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>()
            .parse()
            .expect("a port")
    }
}

fn find_html(dir: &Path) -> Option<PathBuf> {
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_html(&path) {
                return Some(found);
            }
        } else if path.extension().is_some_and(|e| e == "html") {
            return Some(path);
        }
    }
    None
}

fn get(port: u16, path: &str) -> String {
    use std::io::{Read, Write};
    let Ok(mut stream) = std::net::TcpStream::connect(("127.0.0.1", port)) else {
        return String::new();
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"
    );
    let mut out = String::new();
    let _ = stream.read_to_string(&mut out);
    out
}

#[tokio::test]
async fn a_server_error_reaches_live_pages() {
    let dev = Loop::start(project("error"));
    let app = dev.app();
    let channel = dev.channel();
    let stream = tokio::net::TcpStream::connect(("127.0.0.1", channel))
        .await
        .expect("connect to the loop");
    let (mut socket, _) =
        tokio_tungstenite::client_async(format!("ws://127.0.0.1:{channel}/@esdev/hmr"), stream)
            .await
            .expect("the update channel");

    assert!(get(app, "/boom?token=secret").contains("500"));
    let frame = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(Ok(message)) = socket.next().await {
            if let tokio_tungstenite::tungstenite::Message::Text(text) = message
                && text.contains("\"type\":\"error\"")
            {
                return text.to_string();
            }
        }
        String::new()
    })
    .await
    .expect("no server error reached the page");
    assert!(frame.contains("\"title\":\"Server error\""), "{frame}");
    assert!(frame.contains("runtime:http: GET /boom failed"), "{frame}");
    assert!(frame.contains("RangeError: the ledger exploded"), "{frame}");
    assert!(!frame.contains("token=secret"), "{frame}");
    // The terminal has it too, once — from the server, not repeated by esdev.
    assert_eq!(
        dev.log().matches("the ledger exploded").count(),
        1,
        "{}",
        dev.log()
    );
}

/// A restart stops the server the way production does, so a server that
/// handles `SIGTERM` itself runs its own shutdown — and it is prompt. Found in
/// `examples/shop`, whose handler closes its shards and exits: stopped by a
/// drain that bypassed the handler, it waited out the whole grace (10s).
#[test]
fn a_restart_runs_the_servers_own_shutdown_and_is_prompt() {
    let dir = project("restart");
    let server = dir.join("src/server.mjs");
    let handled = format!(
        "{}import {{ onSignal, exit }} from \"runtime:process\";\n\
         // A timer keeps the loop alive, as a real server's pools and shards do.\n\
         setInterval(() => {{}}, 1000);\n\
         onSignal(\"SIGTERM\", () => {{ console.error(\"OWN SHUTDOWN RAN\"); exit(0); }});\n",
        SERVER
    );
    std::fs::write(&server, &handled).expect("add a shutdown handler");
    let dev = Loop::start(dir);
    let app = dev.app();
    let deadline = Instant::now() + Duration::from_secs(60);
    std::fs::write(&server, handled.replace("\"ok\"", "\"ok again\"")).expect("edit the server");
    let edited = Instant::now();
    while Instant::now() < deadline && !get(app, "/").contains("ok again") {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(get(app, "/").contains("ok again"), "{}", dev.log());
    assert!(
        edited.elapsed() < Duration::from_secs(5),
        "the restart waited out the grace: {:?}\n{}",
        edited.elapsed(),
        dev.log()
    );
    assert!(dev.log().contains("OWN SHUTDOWN RAN"), "{}", dev.log());
}

/// The orphan D62 left open: a leader killed outright leaves no server.
#[cfg(unix)]
#[test]
fn a_server_exits_when_its_esdev_start_is_killed() {
    let mut dev = Loop::start(project("orphan"));
    let app = dev.app();
    let _ = dev.child.kill();
    let _ = dev.child.wait();
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && std::net::TcpStream::connect(("127.0.0.1", app)).is_ok() {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        std::net::TcpStream::connect(("127.0.0.1", app)).is_err(),
        "the server outlived esdev start:\n{}",
        dev.log()
    );
}
