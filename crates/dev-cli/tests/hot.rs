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
    /// esdev's output, kept outside the project so writing it does not count
    /// as a change to the project being watched.
    log: PathBuf,
    /// The browser's profile, outside the project for the same reason: the
    /// browser writes to it as it runs, and each write inside the project
    /// was a rebuild and a reload the test did not make.
    profile: PathBuf,
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
        // A failed hot-reload assertion says what the page saw; esdev's own
        // account of what it rebuilt and sent is the other half.
        if std::thread::panicking() {
            let log = std::fs::read_to_string(&self.log).unwrap_or_default();
            eprintln!("--- esdev output ---\n{log}--- end ---");
        }
        let _ = std::fs::remove_dir_all(&self.dir);
        let _ = std::fs::remove_file(&self.log);
        let _ = std::fs::remove_dir_all(&self.profile);
    }
}

/// A port derived from the test's own name, and the one after it for the
/// browser — **both below the ephemeral range**.
///
/// This was 41000–44999, which is entirely inside the window Linux allocates
/// outbound source ports from (`net.ipv4.ip_local_port_range`, 32768–60999 by
/// default). A dev server or a debugger that cannot bind leaves the page never
/// loading, and the symptom is not a bind error — it is [`Page::open`] polling
/// for thirty seconds and the assertion after it failing, which reads like
/// hot replacement being broken. It only showed up under
/// `cargo test --workspace`, because that is where enough connections are open
/// at once for the kernel to have taken one of these numbers.
///
/// Even numbers in 30000–31998, with the browser on the odd one after, stays
/// under the 32768 floor and clear of the 20000–29999 that `tests/cli.rs`
/// takes. Stepping by two is what keeps one fixture's server off another
/// fixture's debugger.
///
/// Two names can still hash to one port, and that would be a *permanent*
/// failure rather than an occasional one — so
/// [`every_fixture_gets_a_port_of_its_own`] asserts they do not, and a new
/// fixture that collides fails there with the reason rather than in whichever
/// test lost the race.
fn port_for(name: &str) -> u16 {
    let hash = name.bytes().fold(0u32, |acc, b| {
        acc.wrapping_mul(31).wrapping_add(u32::from(b))
    });
    30000 + u16::try_from((hash % 1000) * 2).unwrap_or(0)
}

impl Fixture {
    /// Writes a two-module project whose entry is `main`, and starts everything.
    fn start(name: &str, main: &str) -> Option<Fixture> {
        Fixture::start_with(name, main, None)
    }

    /// [`Fixture::start`], with a project plugin module listed in `esdev.json`.
    fn start_with(name: &str, main: &str, plugin: Option<&str>) -> Option<Fixture> {
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
        let plugins = match plugin {
            Some(source) => {
                write(&dir, "plugin.mjs", source);
                r#""plugins": ["./plugin.mjs"],"#
            }
            None => "",
        };
        write(
            &dir,
            "esdev.json",
            &format!(
                r#"{{ {plugins} "build": {{ "targets": {{ "web": {{ "entry": "index.html", "outdir": "dist" }} }} }},
                     "dev": {{ "server": {{ "port": {port} }} }} }}"#
            ),
        );

        let log = std::env::temp_dir().join(format!("esdev-hot-{name}-{}.log", std::process::id()));
        let output = std::fs::File::create(&log).expect("create the esdev log");
        let esdev = Command::new(env!("CARGO_BIN_EXE_esdev"))
            .arg("start")
            .current_dir(&dir)
            .stdout(output.try_clone().expect("clone the log handle"))
            .stderr(output)
            .spawn()
            .expect("spawn esdev start");

        let profile =
            std::env::temp_dir().join(format!("esdev-hot-{name}-{}-profile", std::process::id()));
        let _ = std::fs::remove_dir_all(&profile);
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
            log,
            profile,
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
        // The list holds every target the browser has, and a page is only one
        // kind: Chrome lists its own UI (the omnibox popup) and its extensions'
        // service workers too, and in no promised order — so the first entry
        // is not the page.
        let url = page_target(debug_port).await;

        let stream = TcpStream::connect(format!("127.0.0.1:{debug_port}"))
            .await
            .expect("connect to the browser");
        let (socket, _) = tokio_tungstenite::client_async(url, stream)
            .await
            .expect("websocket handshake with the browser");
        let mut page = Page { socket, next_id: 0 };
        page.call("Page.enable", "{}").await;
        page.call("Runtime.enable", "{}").await;
        // `Page.navigate` answers before the new document replaces the old
        // one, so a check run straight after it can read the old document —
        // which, when the browser already showed this page, says `ONE` and
        // passes. Marking the old document tells the two apart.
        page.eval("window.__esdev_previous_document = true").await;
        page.call(
            "Page.navigate",
            &format!(r#"{{"url":"http://127.0.0.1:{}/"}}"#, fixture.port),
        )
        .await;
        // The bundle is a deferred module, so the document being there is not
        // the bundle having run. Waited for by its effect rather than by a
        // duration, so a slow machine is slow rather than flaky.
        assert!(
            page.until(
                "window.__esdev_previous_document === undefined && \
                 (document.getElementById('out')?.textContent ?? 'pending') !== 'pending'"
            )
            .await,
            "the page never ran its bundle"
        );
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

/// The debugger URL of the browser's page, waiting for it to have one.
async fn page_target(port: u16) -> String {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut last = String::new();
    while Instant::now() < deadline {
        last = http_json(port, "/json/list").await;
        let targets: Vec<serde_json::Value> = serde_json::from_str(&last).unwrap_or_default();
        if let Some(url) = targets
            .iter()
            .find(|t| t["type"] == "page")
            .and_then(|t| t["webSocketDebuggerUrl"].as_str())
        {
            return url.to_string();
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!("no page among the browser's targets: {last}");
}

/// The body of a GET to the browser's HTTP endpoint, read to its
/// `Content-Length`.
///
/// Read until the whole answer is *there*, rather than until the socket
/// closes: the browser holds the connection open regardless of what was
/// asked, so waiting for the end is waiting for ever.
async fn http_json(port: u16, path: &str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if let Ok(mut stream) = TcpStream::connect(format!("127.0.0.1:{port}")).await {
            let request =
                format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
            if stream.write_all(request.as_bytes()).await.is_ok() {
                let mut raw = Vec::new();
                let mut chunk = [0_u8; 4096];
                let until = Instant::now() + Duration::from_secs(5);
                while Instant::now() < until {
                    match tokio::time::timeout(Duration::from_secs(1), stream.read(&mut chunk))
                        .await
                    {
                        Ok(Ok(n)) if n > 0 => raw.extend_from_slice(&chunk[..n]),
                        _ => break,
                    }
                    if let Some(body) = complete_body(&raw) {
                        return body;
                    }
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!("the browser never served {path}");
}

/// The body of `raw`, once it holds the whole of a response.
fn complete_body(raw: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(raw);
    let (head, body) = text.split_once("\r\n\r\n")?;
    let length: usize = head.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("content-length")
            .then(|| value.trim().parse().ok())
            .flatten()
    })?;
    (body.len() >= length).then(|| body[..length].to_string())
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
    // Waited for, not read once: the page can still be settling (a late
    // watcher event reloads it once) when `open` returns.
    assert!(
        page.until("document.getElementById('out').textContent === 'ONE'")
            .await,
        "the page never showed ONE"
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

/// The port scheme's own invariant.
///
/// Fixed ports derived from a name are fine until two names hash together, and
/// the failure then is a fixture that can never bind — indistinguishable, from
/// the outside, from hot replacement being broken. Cheaper to assert here.
#[test]
fn every_fixture_gets_a_port_of_its_own() {
    // Every name passed to `Fixture::start` in this file.
    const FIXTURES: &[&str] = &[
        "accept",
        "reload",
        "css",
        "dep",
        "signal",
        "keep",
        "overlay",
        "plugin-error",
    ];

    let mut taken: Vec<(u16, &str)> = Vec::new();
    for name in FIXTURES {
        let port = port_for(name);
        // The browser sits on the port after the server's.
        for (port, role) in [(port, "server"), (port + 1, "browser")] {
            assert!(
                port < 32768,
                "{name}'s {role} port {port} is inside the ephemeral range, \
                 where the kernel may already have handed it out"
            );
            if let Some((_, other)) = taken.iter().find(|(held, _)| *held == port) {
                panic!("{name} and {other} both want port {port} — change the range or a name");
            }
            taken.push((port, name));
        }
    }
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
    // Waited for, not read once: the page can still be settling (a late
    // watcher event reloads it once) when `open` returns.
    assert!(
        page.until("document.getElementById('out').textContent === 'ONE1'")
            .await,
        "the page never showed ONE1"
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

/// **The overlay.** A build that fails shows in the page — what the terminal
/// shows — and the fix clears it. The page is never loaded again in between:
/// the marker survives a break-and-fix cycle that a reload would destroy.
#[tokio::test]
async fn a_failed_build_shows_in_the_page_until_it_is_fixed() {
    let Some(fixture) = Fixture::start(
        "overlay",
        "import { label } from \"./counter.mjs\";\n\
         document.getElementById(\"out\").textContent = label;\n\
         import.meta.hot.accept();\n",
    ) else {
        eprintln!("skipped: no chromium on this machine");
        return;
    };

    let mut page = Page::open(&fixture).await;

    fixture.edit("src/counter.mjs", "export const label = ;\n");
    assert!(
        page.until("document.getElementById('__esdev_err') !== null")
            .await,
        "no overlay for the broken build"
    );
    assert!(
        page.eval("document.getElementById('__esdev_err_pre').textContent")
            .await
            .contains("counter.mjs"),
        "the overlay names the file"
    );

    fixture.edit("src/counter.mjs", "export const label = \"TWO\";\n");
    assert!(
        page.until("document.getElementById('__esdev_err') === null")
            .await,
        "the fix did not clear the overlay"
    );
    // The fix reached the page however the engine delivered it: recovery
    // after an errored build reloads rather than patching (Vite does the
    // same), so no marker survives — but the overlay is gone and the new
    // code runs.
    assert!(
        page.until("document.getElementById('out').textContent === 'TWO'")
            .await,
        "the fix never reached the page"
    );
}

/// **A plugin's error, fixed, does not take the dev loop down.** A transform
/// that throws fails the full build, which leaves the bundler with no module
/// graph. The save that fixes it then asked for a hot update against that
/// missing graph, and the bundler panicked — when the changed file was one a
/// module had named in `dependsOn`, as a route table names its pages, since
/// that is the path that still finds a module to update. The fix has to reach
/// the page, and the edit after it has to hot-swap again: the graph is back.
#[tokio::test]
async fn a_plugin_error_fixed_does_not_stop_the_dev_loop() {
    let Some(fixture) = Fixture::start_with(
        "plugin-error",
        "import { label } from \"virtual:label\";\n\
         document.getElementById(\"out\").textContent = label;\n\
         import.meta.hot.accept();\n",
        Some(
            r#"const counter = new URL("./src/counter.mjs", import.meta.url).pathname;
export default {
  name: "strict",
  resolve: {
    filter: { id: "virtual:label" },
    handler: () => ({ id: "virtual:label", virtual: true }),
  },
  load: {
    filter: { id: "virtual:label" },
    handler: () => ({
      code: `export { label } from ${JSON.stringify(counter)};`,
      dependsOn: [counter],
    }),
  },
  transform: {
    filter: { id: /counter\.mjs$/ },
    handler(code) {
      if (code.includes("BROKEN")) throw new Error("strict: BROKEN is not allowed");
      return null;
    },
  },
};
"#,
        ),
    ) else {
        eprintln!("skipped: no chromium on this machine");
        return;
    };

    let mut page = Page::open(&fixture).await;
    // Waited for, not read once: the page can still be settling (a late
    // watcher event reloads it once) when `open` returns.
    assert!(
        page.until("document.getElementById('out').textContent === 'ONE'")
            .await,
        "the page never showed ONE"
    );

    fixture.edit("src/counter.mjs", "export const label = \"BROKEN\";\n");
    assert!(
        page.until("document.getElementById('__esdev_err') !== null")
            .await,
        "no overlay for the plugin's error"
    );

    fixture.edit("src/counter.mjs", "export const label = \"TWO\";\n");
    assert!(
        page.until("document.getElementById('out').textContent === 'TWO'")
            .await,
        "the fix never reached the page: the dev loop stopped"
    );
    assert!(
        page.until("document.getElementById('__esdev_err') === null")
            .await,
        "the fix did not clear the overlay"
    );

    // The graph is whole again: the next edit is a patch, not a reload.
    page.eval(MARK).await;
    fixture.edit("src/counter.mjs", "export const label = \"THREE\";\n");
    assert!(
        page.until("document.getElementById('out').textContent === 'THREE'")
            .await,
        "the edit after the fix never arrived"
    );
    assert_eq!(
        page.eval("window.__esdev_test_marker").await,
        "here",
        "the edit after the fix reloaded the page instead of patching it"
    );
}
