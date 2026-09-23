//! `esdev test --browser` — the test files, run in a real page (D99).
//!
//! **The same file, the same `runtime:test`.** Each test file is bundled for
//! the browser with `runtime:test` resolved to the very module this runtime
//! serves ([`crate::guest::test::SOURCE`]), and loaded into a page. What a test
//! can call, how cases queue, what a hook means — none of it is reimplemented
//! for the browser, because none of it is different there.
//!
//! **What does differ is the host, and the host is eight calls.** In this
//! runtime `runtime:test` reports to `globalThis.__ops`, ops the process
//! answers. In a page the runner installs `__ops` with a `script.callFunction`
//! whose argument is a `script.channel`, and each call becomes a message on it
//! — into the same tally ([`crate::guest::test::Tally`]) and out through the
//! same report. The page waits for that call before importing anything, so no
//! test code runs without its host. A page never goes quiet the way a process
//! does, so `runtime:test` also says when its queue has drained.
//!
//! Not a preload script, though BiDi has them for exactly this: Firefox 149
//! logs `Permission denied to access property "length"` into every page that
//! has one — an empty `() => {}` included — which this runner would have to
//! report as the page's uncaught error, or learn to ignore by its text.
//!
//! **One browser, a user context per file.** Starting a browser costs seconds,
//! so a run starts one; a user context is the browser's own isolation — its own
//! cookies, storage and cache — which is what a process per file gives a
//! runtime run. Files run side by side up to `--jobs`, and each prints whole
//! once it is finished, as a process run's files do.
//!
//! **One bundle per file.** A file whose import does not resolve fails alone,
//! as it would in its own process, and one file's stylesheets never reach
//! another file's page.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures_util::StreamExt;
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

use crate::bidi::{Client, Event, Session};
use crate::guest::test::Tally;
use crate::test::TestConfig;

/// Installs `__ops` in the page, then lets the page start. `send` is the
/// channel: everything crosses as one JSON array per call, because a channel
/// message is a serialized value and a string is the one every browser hands
/// back unchanged.
///
/// Ids are handed out here, since `test_registered` must answer at once; the
/// runner follows the page's numbering. Snapshots need the snapshot files,
/// which live beside the test file rather than in the page, and are refused
/// with a message saying so rather than passing without a comparison.
const INSTALL: &str = r#"(send) => {
  let next = 0;
  const post = (...message) => send(JSON.stringify(message));
  const unavailable = (what) => `${what} is not available in a browser run yet`;
  const ops = {
    test_registered(name) { const id = next++; post("registered", id, String(name)); return id; },
    test_running(id) { post("running", id); },
    test_skipped(id, because) { post("skipped", id, String(because ?? "")); },
    test_finished(id, ok, detail) { post("finished", id, ok === true, String(detail ?? "")); },
    test_set_file() {},
    test_snapshot() { return unavailable("toMatchSnapshot"); },
    test_file_snapshot() { return unavailable("toMatchFileSnapshot"); },
    test_drained() { post("drained"); },
  };
  Object.defineProperty(globalThis, "__ops", { value: Object.freeze(ops) });
  Object.defineProperty(globalThis, "__esdev_loaded", { value: (error) => post("loaded", error) });
  globalThis.__esdev_start();
}"#;

/// Runs `files` in the browser `session` is on, prints each file's report,
/// and returns how many failed.
pub async fn run(
    session: &Session,
    root: &Path,
    files: &[PathBuf],
    config: &TestConfig,
) -> Result<usize, String> {
    let staged = Stage::create()?;
    let stage = staged.0.clone();
    let runtime_test = stage.join("runtime-test.js");
    std::fs::write(&runtime_test, crate::guest::test::SOURCE)
        .map_err(|err| format!("cannot write {}: {err}", runtime_test.display()))?;

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|err| format!("cannot serve the test pages: {err}"))?;
    let origin = format!(
        "http://{}",
        listener
            .local_addr()
            .map_err(|err| format!("cannot serve the test pages: {err}"))?
    );
    let server = tokio::spawn(serve(listener, stage.clone()));

    let client = Arc::clone(&session.client);
    client
        .command(
            "session.subscribe",
            json!({ "events": ["script.message", "log.entryAdded"] }),
        )
        .await?;
    let routes: Routes = Arc::default();
    let router = tokio::spawn(route(
        client
            .take_events()
            .ok_or("the browser's events are already taken")?,
        Arc::clone(&routes),
    ));

    let quiet = config.reporter.as_deref() == Some("json");
    let jobs = config
        .jobs
        .unwrap_or_else(crate::test::jobs)
        .min(files.len())
        .max(1);
    let named = |file: &Path| {
        file.strip_prefix(root)
            .unwrap_or(file)
            .display()
            .to_string()
    };
    let runs = files.iter().enumerate().map(|(index, file)| {
        let job = Job {
            client: Arc::clone(&client),
            routes: Arc::clone(&routes),
            root: root.to_path_buf(),
            dir: stage.join(format!("f{index}")),
            path: format!("f{index}"),
            origin: origin.clone(),
            runtime_test: runtime_test.clone(),
            stage: stage.clone(),
            file: file.clone(),
            name: named(file),
        };
        job.run(config)
    });
    let mut failed = 0usize;
    let mut results = futures_util::stream::iter(runs).buffer_unordered(jobs);
    while let Some(ran) = results.next().await {
        if !quiet {
            println!("{}", ran.name);
        }
        print!("{}", ran.output);
        if !ran.passed {
            failed += 1;
        }
    }

    router.abort();
    server.abort();
    Ok(failed)
}

/// The directory a run's bundles and pages are written to, removed when the
/// run is over however it ends — including when it is stopped part way.
struct Stage(PathBuf);

impl Stage {
    fn create() -> Result<Stage, String> {
        let dir = std::env::temp_dir().join(format!("esdev-browser-run-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)
            .map_err(|err| format!("cannot create {}: {err}", dir.display()))?;
        Ok(Stage(dir))
    }
}

impl Drop for Stage {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Where each browsing context's events go.
type Routes = Arc<std::sync::Mutex<HashMap<String, mpsc::UnboundedSender<Event>>>>;

/// Sends each event to the file whose page raised it.
async fn route(mut events: mpsc::UnboundedReceiver<Event>, routes: Routes) {
    while let Some(event) = events.recv().await {
        let Some(context) = event
            .params
            .pointer("/source/context")
            .and_then(Value::as_str)
        else {
            continue;
        };
        let sender = routes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(context)
            .cloned();
        if let Some(sender) = sender {
            let _ = sender.send(event);
        }
    }
    // The connection is gone — the browser or its driver exited. Dropping
    // every file's sender ends each file's wait now, rather than leaving it
    // waiting for events that will never come.
    routes
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();
}

/// One test file's run.
struct Job {
    client: Arc<Client>,
    routes: Routes,
    root: PathBuf,
    /// Where this file's bundle and page are written, and `path` is where
    /// they are served.
    dir: PathBuf,
    path: String,
    origin: String,
    runtime_test: PathBuf,
    stage: PathBuf,
    file: PathBuf,
    /// The file as the report names it.
    name: String,
}

/// What one file's run printed, and whether it passed.
struct Ran {
    name: String,
    output: String,
    passed: bool,
}

impl Job {
    async fn run(self, config: &TestConfig) -> Ran {
        let (output, passed) = match self.attempt(config).await {
            Ok(done) => done,
            Err(err) => (format!("  FAIL {err}\n"), false),
        };
        Ran {
            name: self.name,
            output,
            passed,
        }
    }

    async fn attempt(&self, config: &TestConfig) -> Result<(String, bool), String> {
        let page = self.bundle(config).await?;
        let user_context = self
            .client
            .command("browser.createUserContext", json!({}))
            .await?["userContext"]
            .as_str()
            .ok_or("browser.createUserContext: no user context in the answer")?
            .to_string();
        let ran = self.in_context(&user_context, &page, config).await;
        // Closes the tab with it.
        let _ = self
            .client
            .command(
                "browser.removeUserContext",
                json!({ "userContext": user_context }),
            )
            .await;
        ran
    }

    /// Bundles the file, and the setup modules ahead of it, and writes the
    /// page that loads them. Returns the page's URL.
    async fn bundle(&self, config: &TestConfig) -> Result<String, String> {
        std::fs::create_dir_all(&self.dir)
            .map_err(|err| format!("cannot create {}: {err}", self.dir.display()))?;
        // Each setup module is an entry of its own and the test file is the
        // last, imported by the page in that order. One bundler run, so they
        // share one `runtime:test` — a setup file's `beforeEach` is the test
        // file's.
        let mut entries = Vec::new();
        for (index, module) in config.setup.iter().enumerate() {
            let import = match url::Url::parse(module) {
                Ok(url) if url.scheme() == "file" => url
                    .to_file_path()
                    .map_err(|()| format!("cannot read setup module {module}"))?
                    .display()
                    .to_string(),
                _ => module.clone(),
            };
            entries.push((format!("setup{index}"), import));
        }
        entries.push(("test".to_string(), self.file.display().to_string()));
        let (written, sheets, _) = crate::build::bundle_browser_entries(
            entries,
            &self.root,
            &self.dir,
            true,
            false,
            Vec::new(),
            Vec::new(),
            vec![(
                "runtime:test".to_string(),
                self.runtime_test.display().to_string(),
            )],
            Some("external".to_string()),
            None,
            crate::contract::Jsx::default(),
            config.jsx.clone(),
            &[],
        )
        .await?;

        let scripts = written
            .iter()
            .map(|(_, filename)| json!(format!("/{}/{filename}", self.path)).to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let styles: String = sheets
            .iter()
            .map(|sheet| format!("<style>{}</style>", sheet.code))
            .collect();
        let page = format!(
            "<!doctype html>\n<html><head><meta charset=\"utf-8\"><title>{title}</title>{styles}</head>\
             <body><script type=\"module\">\n\
             (async () => {{\n  \
             await new Promise((start) => Object.defineProperty(globalThis, \"__esdev_start\", {{ value: start }}));\n  \
             for (const src of [{scripts}]) await import(src);\n\
             }})().then(\n  \
             () => globalThis.__esdev_loaded(null),\n  \
             (error) => globalThis.__esdev_loaded(String((error && error.stack) || error)),\n);\n\
             </script></body></html>\n",
            title = escape_html(&self.name),
        );
        std::fs::write(self.dir.join("index.html"), page)
            .map_err(|err| format!("cannot write the test page: {err}"))?;
        Ok(format!("{}/{}/index.html", self.origin, self.path))
    }

    async fn in_context(
        &self,
        user_context: &str,
        page: &str,
        config: &TestConfig,
    ) -> Result<(String, bool), String> {
        let context = self
            .client
            .command(
                "browsingContext.create",
                json!({ "type": "tab", "userContext": user_context }),
            )
            .await?["context"]
            .as_str()
            .ok_or("browsingContext.create: no context in the answer")?
            .to_string();
        let (sender, mut events) = mpsc::unbounded_channel();
        self.routes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(context.clone(), sender);
        self.client
            .command(
                "browsingContext.navigate",
                json!({ "context": context, "url": page, "wait": "complete" }),
            )
            .await?;
        self.client
            .command(
                "script.callFunction",
                json!({
                    "functionDeclaration": INSTALL,
                    "arguments": [{ "type": "channel", "value": { "channel": format!("esdev-{}", self.path) } }],
                    "target": { "context": context },
                    "awaitPromise": false,
                }),
            )
            .await?;

        let mut state = FileState {
            locations: Locations::new(&self.origin, &self.stage, &self.runtime_test),
            ..FileState::default()
        };
        let waited = async {
            while let Some(event) = events.recv().await {
                state.apply(&event);
                if state.done() {
                    return Ended::Finished;
                }
            }
            Ended::Closed
        };
        let ended = match config.timeout {
            Some(ms) => tokio::time::timeout(std::time::Duration::from_millis(ms), waited)
                .await
                .unwrap_or(Ended::TimedOut),
            None => waited.await,
        };
        self.routes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&context);

        // The path a process run's JSON names its file by: the one the child
        // was given, which is absolute.
        let file = self.file.display().to_string();
        let as_json = (config.reporter.as_deref() == Some("json")).then_some(file.as_str());
        let (report, passed) = state.tally.render(as_json);
        let mut output = state.console;
        output.push_str(&report);
        match ended {
            Ended::Finished => {}
            Ended::TimedOut => {
                output.push_str(&crate::test::timed_out(config.timeout));
                output.push('\n');
            }
            Ended::Closed => output.push_str(
                "  FAIL the browser closed before the file finished\n    \
                 It exited or crashed; the results above are what it reported first.\n",
            ),
        }
        let finished = ended == Ended::Finished;
        Ok((output, passed && finished))
    }
}

/// How waiting on a page ended.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Ended {
    Finished,
    TimedOut,
    /// The browser's connection closed first.
    Closed,
}

/// What a page has reported so far.
#[derive(Default)]
struct FileState {
    tally: Tally,
    /// Puts a stack's frames back in the files they were written in.
    locations: Locations,
    /// The page's case ids, to the tally's.
    ids: HashMap<u64, usize>,
    /// How many cases the page registered — as distinct from the ones this
    /// side adds for errors that belong to no case.
    registered: usize,
    /// Whether the page's modules have finished loading.
    loaded: bool,
    /// Whether `runtime:test`'s queue drained after its last registration.
    drained: bool,
    /// What the page wrote to its console, printed ahead of the report as a
    /// process run's output is.
    console: String,
}

impl FileState {
    fn apply(&mut self, event: &Event) {
        match event.method.as_str() {
            "script.message" => {
                let Some(text) = event.params.pointer("/data/value").and_then(Value::as_str) else {
                    return;
                };
                if let Ok(Value::Array(message)) = serde_json::from_str::<Value>(text) {
                    self.message(&message);
                }
            }
            "log.entryAdded" => {
                let text = event
                    .params
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                match event.params.get("type").and_then(Value::as_str) {
                    Some("console") => {
                        self.console.push_str(text);
                        self.console.push('\n');
                    }
                    // An error nothing caught. Inside a case `runtime:test`
                    // already failed it; one that reaches the page is a case
                    // of its own, as a stray rejection is in a process run.
                    Some("javascript") => {
                        let detail = self.locations.remap(&stack(text, &event.params));
                        let id = self.tally.register("uncaught error".to_string());
                        self.tally.finished(id, false, detail);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn message(&mut self, message: &[Value]) {
        let kind = message.first().and_then(Value::as_str).unwrap_or_default();
        let id = message.get(1).and_then(Value::as_u64);
        let index = id.and_then(|id| self.ids.get(&id).copied());
        match (kind, index) {
            ("registered", _) => {
                let name = message
                    .get(2)
                    .and_then(Value::as_str)
                    .unwrap_or("(unnamed)");
                let index = self.tally.register(name.to_string());
                if let Some(id) = id {
                    self.ids.insert(id, index);
                }
                self.registered += 1;
                self.drained = false;
            }
            ("running", Some(index)) => self.tally.running(index),
            ("skipped", Some(index)) => {
                let because = message.get(2).and_then(Value::as_str).unwrap_or_default();
                self.tally.skipped(index, because);
            }
            ("finished", Some(index)) => {
                let passed = message.get(2).and_then(Value::as_bool) == Some(true);
                let detail = message
                    .get(3)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                self.tally
                    .finished(index, passed, self.locations.remap(&detail));
            }
            ("drained", _) => self.drained = true,
            ("loaded", _) => {
                self.loaded = true;
                // A module that threw while loading: the cases it registered
                // before throwing still run, and the throw is a failure of its
                // own rather than a file that silently tested less.
                if let Some(error) = message.get(1).and_then(Value::as_str) {
                    let id = self.tally.register("the file failed to load".to_string());
                    self.tally.finished(id, false, self.locations.remap(error));
                }
            }
            _ => {}
        }
    }

    /// Finished: loaded, every case settled, and — if it registered any — the
    /// queue said it drained after the last.
    fn done(&self) -> bool {
        self.loaded && self.tally.settled() && (self.registered == 0 || self.drained)
    }
}

/// Where a page's stack frames point, and where they were written.
///
/// A frame in a page names the bundle as the browser fetched it,
/// `http://127.0.0.1:<port>/f0/test.js:711:13`. That URL is the staged file,
/// and the staged file has its map beside it — so the frame becomes a
/// `file://` URL and goes through the same remapping an uncaught error in
/// this runtime does ([`es_runtime_cli_common::sourcemap`]). Frames inside
/// `runtime:test` land in the staged copy of it, and are named `runtime:test`
/// as they are in a process run.
#[derive(Default)]
struct Locations {
    /// `http://127.0.0.1:<port>/`, and the stage as a `file://` URL.
    served: Option<(String, String)>,
    /// The staged `runtime:test`, as a `file://` URL.
    runtime_test: Option<String>,
}

impl Locations {
    fn new(origin: &str, stage: &Path, runtime_test: &Path) -> Locations {
        let stage = url::Url::from_directory_path(stage).ok();
        let runtime_test = url::Url::from_file_path(runtime_test).ok();
        Locations {
            served: stage.map(|stage| (format!("{origin}/"), stage.to_string())),
            runtime_test: runtime_test.map(|url| url.to_string()),
        }
    }

    fn remap(&self, text: &str) -> String {
        let Some((origin, stage)) = &self.served else {
            return text.to_string();
        };
        let local = text.replace(origin.as_str(), stage);
        let mapped = es_runtime_cli_common::sourcemap::remap(&local);
        match &self.runtime_test {
            Some(runtime_test) => mapped.replace(runtime_test.as_str(), "runtime:test"),
            None => mapped,
        }
    }
}

/// An uncaught error's text and where it was thrown.
fn stack(text: &str, params: &Value) -> String {
    let mut out = text.to_string();
    let frames = params
        .pointer("/stackTrace/callFrames")
        .and_then(Value::as_array);
    for frame in frames.into_iter().flatten() {
        let function = frame
            .get("functionName")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .unwrap_or("<anonymous>");
        let url = frame.get("url").and_then(Value::as_str).unwrap_or_default();
        let line = frame.get("lineNumber").and_then(Value::as_u64).unwrap_or(0) + 1;
        let column = frame
            .get("columnNumber")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            + 1;
        out.push_str(&format!("\n    at {function} ({url}:{line}:{column})"));
    }
    out
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Serves the staged pages and bundles, read-only, to the browser on
/// loopback.
async fn serve(listener: TcpListener, stage: PathBuf) {
    while let Ok((mut stream, _)) = listener.accept().await {
        let stage = stage.clone();
        tokio::spawn(async move {
            let Some(head) = crate::inspect::read_head(&mut stream).await else {
                return;
            };
            let path = crate::inspect::request_path(&head).unwrap_or_default();
            let (status, content_type, body) = match file_for(&stage, &path) {
                Some((file, content_type)) => match std::fs::read_to_string(&file) {
                    Ok(body) => ("200 OK", content_type, body),
                    Err(_) => ("404 Not Found", "text/plain", String::new()),
                },
                None => ("404 Not Found", "text/plain", String::new()),
            };
            let _ = crate::inspect::respond(&mut stream, status, content_type, &body).await;
        });
    }
}

/// The staged file a request path names, and its type. Only plain names
/// under the stage: nothing climbs out of it.
fn file_for(stage: &Path, path: &str) -> Option<(PathBuf, &'static str)> {
    let path = path.split(['?', '#']).next()?;
    let mut file = stage.to_path_buf();
    for part in path.split('/').filter(|part| !part.is_empty()) {
        if part == "." || part == ".." || part.contains('\\') {
            return None;
        }
        file.push(part);
    }
    let content_type = match file.extension()?.to_str()? {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        _ => "application/octet-stream",
    };
    Some((file, content_type))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(parts: Value) -> Event {
        Event {
            method: "script.message".to_string(),
            params: json!({ "data": { "type": "string", "value": parts.to_string() } }),
        }
    }

    #[test]
    fn a_page_is_done_once_loaded_settled_and_drained() {
        let mut state = FileState::default();
        state.apply(&message(json!(["registered", 0, "adds"])));
        state.apply(&message(json!(["registered", 1, "subtracts"])));
        state.apply(&message(json!(["loaded", null])));
        assert!(!state.done());
        state.apply(&message(json!(["running", 0])));
        state.apply(&message(json!(["finished", 0, true, ""])));
        state.apply(&message(json!(["running", 1])));
        state.apply(&message(json!(["finished", 1, false, "Error: 2 !== 3"])));
        // Settled, but the queue has not said it drained: an `afterAll` may
        // still be running, and may still fail.
        assert!(!state.done());
        state.apply(&message(json!(["drained"])));
        assert!(state.done());
        let (report, passed) = state.tally.render(None);
        assert!(!passed);
        assert!(
            report.contains("  FAIL subtracts\n    Error: 2 !== 3"),
            "{report}"
        );
        assert!(report.contains("1 passed, 1 failed"), "{report}");
    }

    #[test]
    fn a_page_with_no_tests_is_done_once_loaded() {
        let mut state = FileState::default();
        assert!(!state.done());
        state.apply(&message(json!(["loaded", null])));
        assert!(state.done());
        assert_eq!(state.tally.render(None), (String::new(), true));
    }

    #[test]
    fn a_file_that_failed_to_load_fails_but_keeps_what_ran() {
        let mut state = FileState::default();
        state.apply(&message(json!(["registered", 0, "ran before the throw"])));
        state.apply(&message(json!([
            "loaded",
            "Error: boom\n    at test.js:3:7"
        ])));
        state.apply(&message(json!(["running", 0])));
        state.apply(&message(json!(["finished", 0, true, ""])));
        state.apply(&message(json!(["drained"])));
        assert!(state.done());
        let (report, passed) = state.tally.render(None);
        assert!(!passed);
        assert!(report.contains("FAIL the file failed to load"), "{report}");
        assert!(report.contains("1 passed, 1 failed"), "{report}");
    }

    #[test]
    fn a_registration_after_draining_is_waited_for() {
        let mut state = FileState::default();
        state.apply(&message(json!(["loaded", null])));
        state.apply(&message(json!(["registered", 0, "first"])));
        state.apply(&message(json!(["finished", 0, true, ""])));
        state.apply(&message(json!(["drained"])));
        assert!(state.done());
        state.apply(&message(json!(["registered", 1, "late"])));
        assert!(!state.done());
    }

    #[test]
    fn console_output_and_uncaught_errors_are_the_files() {
        let mut state = FileState::default();
        state.apply(&Event {
            method: "log.entryAdded".to_string(),
            params: json!({ "type": "console", "level": "info", "text": "hello from the page" }),
        });
        state.apply(&Event {
            method: "log.entryAdded".to_string(),
            params: json!({
                "type": "javascript",
                "level": "error",
                "text": "Error: thrown in a timer",
                "stackTrace": { "callFrames": [
                    { "functionName": "", "url": "http://127.0.0.1:1/f0/test.js", "lineNumber": 4, "columnNumber": 10 }
                ] },
            }),
        });
        assert_eq!(state.console, "hello from the page\n");
        let (report, passed) = state.tally.render(None);
        assert!(!passed);
        assert!(report.contains("FAIL uncaught error"), "{report}");
        assert!(
            report.contains("at <anonymous> (http://127.0.0.1:1/f0/test.js:5:11)"),
            "{report}"
        );
    }

    #[test]
    fn a_page_frame_is_named_by_the_file_it_came_from() {
        let stage = std::env::temp_dir().join("esdev-browser-locations");
        let locations = Locations::new(
            "http://127.0.0.1:4000",
            &stage,
            &stage.join("runtime-test.js"),
        );
        let stage_url = url::Url::from_directory_path(&stage).unwrap();
        // No map beside either file, so the frames keep their positions: what
        // changes is which file they name.
        let text = "Error: no\n    at fail (http://127.0.0.1:4000/runtime-test.js:9:1)\n    \
                    at http://127.0.0.1:4000/f0/test.js:3:5";
        let remapped = locations.remap(text);
        assert_eq!(
            remapped,
            format!("Error: no\n    at fail (runtime:test:9:1)\n    at {stage_url}f0/test.js:3:5")
        );
        // Text that names no page location is left as it was.
        assert_eq!(locations.remap("Error: plain"), "Error: plain");
    }

    #[tokio::test]
    async fn a_closed_browser_releases_every_file_waiting_on_it() {
        let (events_in, events) = mpsc::unbounded_channel();
        let routes: Routes = Arc::default();
        let (sender, mut waiting) = mpsc::unbounded_channel();
        routes.lock().unwrap().insert("ctx".to_string(), sender);
        let router = tokio::spawn(route(events, Arc::clone(&routes)));
        events_in
            .send(Event {
                method: "script.message".to_string(),
                params: json!({ "source": { "context": "ctx" } }),
            })
            .unwrap();
        assert!(waiting.recv().await.is_some(), "an event reaches its file");
        // The connection ends: nothing is left to send on.
        drop(events_in);
        router.await.unwrap();
        assert!(waiting.recv().await.is_none(), "the file's wait ends");
        assert!(routes.lock().unwrap().is_empty());
    }

    #[test]
    fn nothing_outside_the_stage_is_served() {
        let stage = Path::new("/stage");
        assert_eq!(
            file_for(stage, "/f0/index.html?x=1"),
            Some((
                PathBuf::from("/stage/f0/index.html"),
                "text/html; charset=utf-8"
            ))
        );
        assert_eq!(file_for(stage, "/f0/../../etc/passwd.js"), None);
        assert_eq!(file_for(stage, "/f0/..\\x.js"), None);
        assert_eq!(file_for(stage, "/f0/noextension"), None);
    }
}
