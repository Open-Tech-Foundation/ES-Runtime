//! Hot module replacement, driven through a real browser.
//!
//! # Why a browser, and why these assertions
//!
//! Everything HMR claims is about what a *running page* does, and none of it can
//! be seen from the output of a build. A patch on disk proves nothing; a module
//! re-running proves nothing either, because the failure this feature exists to
//! avoid is a page that reloads. So each test here sets a marker on `window`,
//! makes an edit, and asks two questions: **did the new code take effect**, and
//! **is the marker still there**. A marker that survives is the whole claim —
//! it can only survive if the page was never loaded again.
//!
//! Every bug found while writing this feature would have passed a weaker test.
//! A walk that dropped the module cache and never re-ran anything looked like a
//! success, because the accept callback set the text being asserted on. So the
//! assertions here are deliberately on the *rendered* state and the marker
//! together, never on one of them.
//!
//! # It skips rather than fails when there is no browser
//!
//! Chromium is not a build dependency and will not be on every machine. Absent
//! one, these say so and pass — the same shape the `--inspect` tests use for a
//! sibling binary. A test that fails for want of a tool teaches people to ignore
//! it.
//!
//! The fixtures depend on **nothing from npm**, which is what makes this
//! runnable at all: the react half of hot replacement needs an install and is
//! covered by hand, but the module graph, the boundary walk and the transport
//! are all exercised here by two files and a document.

// A test reporting why it skipped is talking to whoever reads the run.
#![allow(clippy::print_stderr)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

/// A headless browser, or `None` when there is none to be had.
fn chromium() -> Option<&'static str> {
    [
        "chromium",
        "chromium-browser",
        "google-chrome",
        "google-chrome-stable",
    ]
    .into_iter()
    .find(|name| {
        Command::new(name)
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    })
}

/// A dev server and the browser pointed at it, both stopped on drop.
///
/// On `Drop` rather than at the end of a passing test: an assertion that fires
/// unwinds past any explicit stop, and what would be left behind is a dev server
/// holding a port and a headless browser holding a profile — which the *next*
/// run then collides with. The same lesson `tests/cli.rs` learned.
struct Fixture {
    dir: PathBuf,
    esdev: Option<Child>,
    browser: Option<Child>,
    port: u16,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for child in [self.esdev.as_mut(), self.browser.as_mut()]
            .into_iter()
            .flatten()
        {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A port unlikely to collide, derived from the test's own name.
fn port_for(name: &str) -> u16 {
    let hash = name.bytes().fold(0u32, |acc, b| {
        acc.wrapping_mul(31).wrapping_add(u32::from(b))
    });
    41000 + u16::try_from(hash % 4000).unwrap_or(0)
}

impl Fixture {
    /// Writes a two-module project whose entry is `main`, and starts everything.
    fn start(name: &str, main: &str) -> Option<Fixture> {
        let browser = chromium()?;
        let dir = std::env::temp_dir().join(format!("esdev-hot-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).expect("create the fixture");

        write(&dir, "src/counter.mjs", "export const label = \"ONE\";\n");
        write(&dir, "src/main.mjs", main);
        write(&dir, "styles.css", "body{color:red}\n");
        write(
            &dir,
            "index.html",
            "<!doctype html><html><head><link rel=\"stylesheet\" href=\"./styles.css\">\
             <script type=\"module\" src=\"./src/main.mjs\"></script></head>\
             <body><div id=out>pending</div></body></html>\n",
        );
        let port = port_for(name);
        write(
            &dir,
            "esdev.json",
            &format!(
                r#"{{ "targets": {{ "web": {{ "entry": "index.html", "outdir": "dist" }} }},
                     "start": {{ "port": {port} }} }}"#
            ),
        );

        let esdev = Command::new(env!("CARGO_BIN_EXE_esdev"))
            .arg("start")
            .current_dir(&dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn esdev start");

        let profile = dir.join("browser-profile");
        let debug_port = port + 1;
        let browser = Command::new(browser)
            .args([
                "--headless",
                "--no-sandbox",
                "--disable-gpu",
                &format!("--remote-debugging-port={debug_port}"),
                &format!("--user-data-dir={}", profile.display()),
                "about:blank",
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn the browser");

        Some(Fixture {
            dir,
            esdev: Some(esdev),
            browser: Some(browser),
            port,
        })
    }

    fn edit(&self, path: &str, contents: &str) {
        write(&self.dir, path, contents);
    }
}

fn write(dir: &Path, name: &str, contents: &str) {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create a directory");
    }
    std::fs::write(path, contents).expect("write a fixture file");
}

/// One CDP session against the page.
struct Page {
    socket: WebSocketStream<TcpStream>,
    next_id: u64,
}

impl Page {
    /// Opens the page and waits for it to have run its bundle.
    async fn open(fixture: &Fixture) -> Page {
        let debug_port = fixture.port + 1;
        let target = http_json(debug_port, "/json/list").await;
        let url = target
            .split("\"webSocketDebuggerUrl\": \"")
            .nth(1)
            .or_else(|| target.split("\"webSocketDebuggerUrl\":\"").nth(1))
            .and_then(|rest| rest.split('"').next())
            .unwrap_or_else(|| panic!("no debuggable page in {target}"))
            .to_string();

        let stream = TcpStream::connect(format!("127.0.0.1:{debug_port}"))
            .await
            .expect("connect to the browser");
        let (socket, _) = tokio_tungstenite::client_async(url, stream)
            .await
            .expect("websocket handshake with the browser");
        let mut page = Page { socket, next_id: 0 };
        page.call("Page.enable", "{}").await;
        page.call("Runtime.enable", "{}").await;
        page.call(
            "Page.navigate",
            &format!(r#"{{"url":"http://127.0.0.1:{}/"}}"#, fixture.port),
        )
        .await;
        // The bundle is a deferred module, so the document being there is not
        // the bundle having run. Waited for by its effect rather than by a
        // duration, so a slow machine is slow rather than flaky.
        page.until("document.getElementById('out').textContent !== 'pending'")
            .await;
        page
    }

    async fn call(&mut self, method: &str, params: &str) -> String {
        self.next_id += 1;
        let id = self.next_id;
        let message = format!(r#"{{"id":{id},"method":"{method}","params":{params}}}"#);
        self.socket
            .send(Message::Text(message.into()))
            .await
            .expect("send");
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_secs(20), self.socket.next()).await {
                Ok(Some(Ok(Message::Text(text)))) => {
                    if text.contains(&format!("\"id\":{id}")) {
                        return text.as_str().to_string();
                    }
                }
                Ok(Some(Ok(_))) => {}
                other => panic!("the browser connection ended: {other:?}"),
            }
        }
        panic!("no reply to {method}");
    }

    /// Evaluates an expression and returns its value as JSON.
    async fn eval(&mut self, expression: &str) -> String {
        let escaped = expression.replace('\\', "\\\\").replace('"', "\\\"");
        let reply = self
            .call(
                "Runtime.evaluate",
                &format!(r#"{{"expression":"{escaped}","returnByValue":true}}"#),
            )
            .await;
        reply
            .split("\"value\":")
            .nth(1)
            .and_then(|rest| rest.split(",\"").next().or(Some(rest)))
            .unwrap_or("")
            .trim_end_matches('}')
            .trim_matches('"')
            .to_string()
    }

    /// Polls until `expression` is true, or gives up.
    async fn until(&mut self, expression: &str) -> bool {
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if self.eval(expression).await == "true" {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
        false
    }
}

async fn http_json(port: u16, path: &str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if let Ok(mut stream) = TcpStream::connect(format!("127.0.0.1:{port}")).await {
            let request =
                format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
            if stream.write_all(request.as_bytes()).await.is_ok() {
                // Read until the answer is *there*, rather than until the socket
                // closes. The browser holds the connection open regardless of
                // what was asked, so waiting for the end is waiting for ever —
                // and a read that is merely bounded returns nothing useful,
                // because the buffer belongs to the future that was cancelled.
                let mut body = String::new();
                let mut chunk = [0_u8; 4096];
                let until = Instant::now() + Duration::from_secs(5);
                while Instant::now() < until {
                    match tokio::time::timeout(Duration::from_secs(1), stream.read(&mut chunk))
                        .await
                    {
                        Ok(Ok(0)) => break,
                        Ok(Ok(n)) => body.push_str(&String::from_utf8_lossy(&chunk[..n])),
                        Ok(Err(_)) => break,
                        Err(_) => break,
                    }
                    if body.contains("webSocketDebuggerUrl") {
                        return body;
                    }
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!("the browser never served {path}");
}

/// The marker that only a page load can destroy.
const MARK: &str = "window.__esdev_test_marker = 'here'";

/// **The claim, in one test.** A module that accepts is replaced in place: the
/// new code takes effect and the page is never loaded again.
#[tokio::test]
async fn an_accepting_module_is_replaced_without_reloading_the_page() {
    let Some(fixture) = Fixture::start(
        "accept",
        "import { label } from \"./counter.mjs\";\n\
         document.getElementById(\"out\").textContent = label;\n\
         import.meta.hot.accept();\n",
    ) else {
        eprintln!("skipped: no chromium on this machine");
        return;
    };

    let mut page = Page::open(&fixture).await;
    assert_eq!(
        page.eval("document.getElementById('out').textContent")
            .await,
        "ONE"
    );
    page.eval(MARK).await;

    fixture.edit("src/counter.mjs", "export const label = \"TWO\";\n");

    assert!(
        page.until("document.getElementById('out').textContent === 'TWO'")
            .await,
        "the edit never reached the page"
    );
    assert_eq!(
        page.eval("window.__esdev_test_marker").await,
        "here",
        "the page reloaded, so this was a reload rather than a hot replacement"
    );
}

/// The other half, and it is not a failure: a module that says nothing about
/// how to replace itself has not earned being replaced.
#[tokio::test]
async fn a_change_nothing_accepts_reloads_the_page() {
    let Some(fixture) = Fixture::start(
        "reload",
        "import { label } from \"./counter.mjs\";\n\
         document.getElementById(\"out\").textContent = label;\n",
    ) else {
        eprintln!("skipped: no chromium on this machine");
        return;
    };

    let mut page = Page::open(&fixture).await;
    page.eval(MARK).await;

    fixture.edit("src/counter.mjs", "export const label = \"TWO\";\n");

    assert!(
        page.until("document.getElementById('out').textContent === 'TWO'")
            .await,
        "the edit never reached the page"
    );
    assert_eq!(
        page.eval("window.__esdev_test_marker").await,
        "",
        "the marker survived, so the page was never reloaded — but nothing accepted the change"
    );
}

/// A stylesheet is the one thing that can be swapped with no module graph
/// involved at all, and it must not cost a reload either.
#[tokio::test]
async fn a_stylesheet_is_swapped_without_reloading_the_page() {
    let Some(fixture) = Fixture::start(
        "css",
        "import { label } from \"./counter.mjs\";\n\
         document.getElementById(\"out\").textContent = label;\n",
    ) else {
        eprintln!("skipped: no chromium on this machine");
        return;
    };

    let mut page = Page::open(&fixture).await;
    page.eval(MARK).await;

    fixture.edit("styles.css", "body{color:rgb(0, 128, 0)}\n");

    assert!(
        page.until("getComputedStyle(document.body).color === 'rgb(0, 128, 0)'")
            .await,
        "the stylesheet never changed in the page"
    );
    assert_eq!(
        page.eval("window.__esdev_test_marker").await,
        "here",
        "a stylesheet edit reloaded the page"
    );
}

/// `accept(dep)` re-runs **the dependency** and notifies the acceptor with its
/// new exports. Re-running the acceptor instead is not merely wrong but
/// impossible — the patch ships the dependency's factory and not the
/// acceptor's — and getting it wrong fails with a missing factory.
#[tokio::test]
async fn accepting_a_dependency_notifies_with_its_new_exports() {
    let Some(fixture) = Fixture::start(
        "dep",
        "import { label } from \"./counter.mjs\";\n\
         document.getElementById(\"out\").textContent = label;\n\
         import.meta.hot.accept(\"./counter.mjs\", (mod) => {\n\
         document.getElementById(\"out\").textContent = \"dep:\" + mod.label;\n\
         });\n",
    ) else {
        eprintln!("skipped: no chromium on this machine");
        return;
    };

    let mut page = Page::open(&fixture).await;
    page.eval(MARK).await;

    fixture.edit("src/counter.mjs", "export const label = \"TWO\";\n");

    assert!(
        page.until("document.getElementById('out').textContent === 'dep:TWO'")
            .await,
        "the acceptor was not told, or was told the old exports"
    );
    assert_eq!(page.eval("window.__esdev_test_marker").await, "here");
}

/// `signal` is the teardown that needs no teardown code, and the bug it exists
/// to stop is cumulative: without it a listener is added on every replacement
/// and removed on none, so the twentieth save has twenty of them.
#[tokio::test]
async fn a_replaced_module_s_listeners_are_aborted() {
    let Some(fixture) = Fixture::start(
        "signal",
        "import { label } from \"./counter.mjs\";\n\
         window.__fired = window.__fired || 0;\n\
         addEventListener(\"probe\", () => { window.__fired += 1; }, \
         { signal: import.meta.hot.signal });\n\
         document.getElementById(\"out\").textContent = label;\n\
         import.meta.hot.accept();\n",
    ) else {
        eprintln!("skipped: no chromium on this machine");
        return;
    };

    let mut page = Page::open(&fixture).await;
    page.eval(MARK).await;

    for label in ["TWO", "THREE"] {
        fixture.edit(
            "src/counter.mjs",
            &format!("export const label = \"{label}\";\n"),
        );
        assert!(
            page.until(&format!(
                "document.getElementById('out').textContent === '{label}'"
            ))
            .await,
            "the edit to {label} never landed"
        );
    }

    page.eval("dispatchEvent(new Event('probe'))").await;
    assert_eq!(
        page.eval("window.__fired").await,
        "1",
        "three module instances registered a listener and more than one survived"
    );
    assert_eq!(page.eval("window.__esdev_test_marker").await, "here");
}

/// State carried across a replacement, in one call site rather than the two
/// that `dispose` and `data` take.
#[tokio::test]
async fn keep_survives_every_replacement() {
    let Some(fixture) = Fixture::start(
        "keep",
        "import { label } from \"./counter.mjs\";\n\
         const runs = import.meta.hot.keep(\"runs\", () => ({ n: 0 }));\n\
         runs.n += 1;\n\
         document.getElementById(\"out\").textContent = label + runs.n;\n\
         import.meta.hot.accept();\n",
    ) else {
        eprintln!("skipped: no chromium on this machine");
        return;
    };

    let mut page = Page::open(&fixture).await;
    assert_eq!(
        page.eval("document.getElementById('out').textContent")
            .await,
        "ONE1"
    );
    page.eval(MARK).await;

    fixture.edit("src/counter.mjs", "export const label = \"TWO\";\n");
    assert!(
        page.until("document.getElementById('out').textContent === 'TWO2'")
            .await,
        "the kept value did not survive, or the module did not re-run"
    );
    fixture.edit("src/counter.mjs", "export const label = \"THREE\";\n");
    assert!(
        page.until("document.getElementById('out').textContent === 'THREE3'")
            .await,
        "the kept value did not survive a second replacement"
    );
    assert_eq!(page.eval("window.__esdev_test_marker").await, "here");
}
