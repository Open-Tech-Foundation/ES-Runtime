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
use crate::guest::test::{FileSnapshots, RunOptions, Tally};
use crate::test::TestConfig;

/// Installs `__ops` in the page, then lets the page start. `send` is the
/// channel: everything crosses as one JSON array per call, because a channel
/// message is a serialized value and a string is the one every browser hands
/// back unchanged.
///
/// Ids are handed out here, since `test_registered` must answer at once; the
/// runner follows the page's numbering.
///
/// **Snapshots are compared twice.** A snapshot matcher has to fail where it
/// is called, and the page cannot wait on the host, so `store` carries the
/// file's stored snapshots in and the page decides pass or fail itself. Every
/// snapshot is also posted back, and the host runs the same check a process
/// run does: it keeps the tally, writes and prunes the files, and its fuller
/// message — a diff — replaces the page's one-line one in the report. The
/// page's messages are the first lines of the host's, which is what makes
/// the replacement possible.
const INSTALL: &str = r#"(send, config) => {
  let next = 0;
  const post = (...message) => send(JSON.stringify(message));
  const store = JSON.parse(config);
  const names = new Map();
  const owners = new Map();
  const keyName = (text) => text.replace(/\r/g, "\\r").replace(/\n/g, "\\n");
  const toBase64 = (bytes) => {
    let binary = "";
    for (let at = 0; at < bytes.length; at += 0x8000) {
      binary += String.fromCharCode.apply(null, bytes.subarray(at, at + 0x8000));
    }
    return btoa(binary);
  };
  // What the host answers: a screenshot's verdict, by request.
  const replies = new Map();
  let nextReply = 0;
  Object.defineProperty(globalThis, "__esdev_reply", {
    value: (request, message) => {
      const done = replies.get(request);
      replies.delete(request);
      done?.(message);
    },
  });
  const ops = {
    test_screenshot(id, name, rect, options) {
      const request = nextReply++;
      const reply = new Promise((resolve) => replies.set(request, resolve));
      post("screenshot", id, request, name, rect, String(options));
      return reply;
    },
    test_registered(name) {
      const id = next++;
      names.set(id, String(name));
      post("registered", id, String(name));
      return id;
    },
    test_running(id) { post("running", id); },
    test_skipped(id, because) { post("skipped", id, String(because ?? "")); },
    test_finished(id, ok, detail) { post("finished", id, ok === true, String(detail ?? "")); },
    test_set_file() {},
    test_bench(id, json) { post("bench", id, String(json)); },
    test_snapshot(id, key, actual, kind) {
      post("snapshot", id, String(key), String(kind), String(actual));
      const name = names.get(id) ?? "";
      const owner = owners.get(name);
      if (owner === undefined) owners.set(name, id);
      else if (owner !== id) {
        return `another test in this file is also named ${JSON.stringify(name)}, and snapshots are stored by test name — rename one of them`;
      }
      const full = keyName(`${name}: ${key}`);
      const stored = store.snapshots[full];
      if (stored !== undefined && stored[0] === kind && stored[1] === actual) return undefined;
      if (stored !== undefined && !store.update) return `snapshot changed — ${full}`;
      if (stored === undefined && store.ci) return `no stored snapshot: ${full}; --ci does not write them`;
      return undefined;
    },
    test_file_snapshot(id, name, actual) {
      let bytes;
      if (typeof actual === "string") bytes = new TextEncoder().encode(actual);
      else if (actual instanceof ArrayBuffer) bytes = new Uint8Array(actual);
      else if (ArrayBuffer.isView(actual)) bytes = new Uint8Array(actual.buffer, actual.byteOffset, actual.byteLength);
      else return "toMatchFileSnapshot accepts a string or byte buffer";
      name = String(name);
      if (name === "" || name === "." || name === ".." || /[\/\\]/.test(name)) {
        return "toMatchFileSnapshot(name) needs a filename, not a path";
      }
      const encoded = toBase64(bytes);
      post("file-snapshot", id, name, encoded);
      const path = store.filePath.replace("\u0000", name);
      const stored = store.files[name];
      if (stored === encoded) return undefined;
      if (stored !== undefined && !store.update) return `file snapshot differs: ${path}`;
      if (stored === undefined && store.ci) return `no stored file snapshot: ${path}; --ci does not write them`;
      return undefined;
    },
    test_inline_snapshot(id, stack, actual, existing) {
      post("inline-snapshot", id, String(stack), String(actual), existing);
      if (existing === actual) return undefined;
      if (existing !== null && !store.update) return "inline snapshot changed";
      if (existing === null && store.ci) return "no inline snapshot; --ci does not write them";
      return undefined;
    },
    test_options() { return JSON.stringify(store.options); },
    test_provided() { return store.provided ?? undefined; },
    test_drained() { post("drained"); },
  };
  Object.defineProperty(globalThis, "__ops", { value: Object.freeze(ops) });
  Object.defineProperty(globalThis, "__esdev_loaded", { value: (error) => post("loaded", error) });
  globalThis.__esdev_start();
}"#;

/// A browser session set up to run test files: subscribed to the events a
/// page reports through, with one router sending them to the file each came
/// from. Made once per session, so `--watch` can run pass after pass in the
/// same browser.
pub struct Runner {
    client: Arc<Client>,
    routes: Routes,
    router: tokio::task::JoinHandle<()>,
    /// The browser's name, as a reference screenshot's file names it.
    browser: String,
}

impl Runner {
    pub async fn new(session: &Session, browser: &str) -> Result<Runner, String> {
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
        Ok(Runner {
            client,
            routes,
            router,
            browser: browser.to_string(),
        })
    }

    /// Runs `files`, prints each file's report, hands each result to
    /// `reporter`, and returns how many failed. `browser` names the browser
    /// in each file's report, for a run that uses several.
    pub async fn run(
        &self,
        root: &Path,
        files: &[PathBuf],
        config: &TestConfig,
        browser: Option<&str>,
        reporter: &mut crate::test::Reporter<'_>,
    ) -> Result<usize, String> {
        run(self, root, files, config, browser, reporter).await
    }
}

impl Drop for Runner {
    fn drop(&mut self) {
        self.router.abort();
    }
}

async fn run(
    runner: &Runner,
    root: &Path,
    files: &[PathBuf],
    config: &TestConfig,
    browser: Option<&str>,
    reporter: &mut crate::test::Reporter<'_>,
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
    // The project's `public/` at the root of the origin, as a build puts it and
    // as Vite serves it, so `/logo.svg` in a test is the file it will be.
    let public = Some(root.join("public")).filter(|dir| dir.is_dir());
    let server = tokio::spawn(serve(listener, stage.clone(), public));

    // With a machine reporter on stdout, what the pages printed goes to stderr.
    let quiet = !config.terminal_human();
    let jobs = config
        .jobs
        .unwrap_or_else(crate::test::jobs)
        .min(files.len())
        .max(1);
    let named = |file: &Path| {
        let name = file
            .strip_prefix(root)
            .unwrap_or(file)
            .display()
            .to_string();
        match browser {
            Some(browser) => format!("{name} [{browser}]"),
            None => name,
        }
    };
    // `--bail`: tests failed so far across files; a file is not started once
    // the limit is reached, and each one started is told how many more may fail.
    let failed_tests = std::sync::atomic::AtomicUsize::new(0);
    let not_run = std::sync::atomic::AtomicUsize::new(0);
    let failed_tests = &failed_tests;
    let not_run = &not_run;
    let runs = files.iter().enumerate().map(|(index, file)| {
        let mut options = config.run_options();
        options.module_tags = std::fs::read_to_string(file)
            .map(|source| crate::tags::module_tags(&source))
            .unwrap_or_default();
        let job = Job {
            client: Arc::clone(&runner.client),
            routes: Arc::clone(&runner.routes),
            root: root.to_path_buf(),
            dir: stage.join(format!("f{index}")),
            path: format!("f{index}"),
            origin: origin.clone(),
            runtime_test: runtime_test.clone(),
            browser: runner.browser.clone(),
            stage: stage.clone(),
            file: file.clone(),
            name: named(file),
        };
        async move {
            if let Some(limit) = config.bail {
                let failed = failed_tests.load(std::sync::atomic::Ordering::SeqCst);
                if failed >= limit {
                    not_run.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    return None;
                }
                options.bail = Some(limit - failed);
            }
            let ran = job.run(config, options).await;
            failed_tests.fetch_add(ran.failed_tests, std::sync::atomic::Ordering::SeqCst);
            Some(ran)
        }
    });
    let mut failed = 0usize;
    let mut results = futures_util::stream::iter(runs).buffer_unordered(jobs);
    while let Some(ran) = results.next().await {
        let Some(ran) = ran else {
            continue;
        };
        if quiet {
            eprint!("{}", ran.output);
        } else {
            println!("{}", ran.name);
            print!("{}", ran.output);
        }
        if !ran.passed {
            failed += 1;
        }
        let mut result = ran.result;
        result.browser = browser.map(str::to_string);
        reporter.file(&result);
    }

    server.abort();
    crate::test::report_not_run(not_run.load(std::sync::atomic::Ordering::SeqCst), quiet);
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
    /// The browser it runs in.
    browser: String,
}

/// What one file's run printed, and whether it passed.
struct Ran {
    name: String,
    output: String,
    passed: bool,
    /// How many of its tests failed, for `--bail`.
    failed_tests: usize,
    /// Its results, for a machine reporter.
    result: crate::report::FileResult,
}

impl Job {
    async fn run(self, config: &TestConfig, options: RunOptions) -> Ran {
        let (output, passed, failed_tests, result) = match self.attempt(config, &options).await {
            Ok(done) => done,
            Err(err) => (
                format!("  FAIL {err}\n"),
                false,
                1,
                crate::report::FileResult::broken(
                    self.file.display().to_string(),
                    self.name.clone(),
                    &err,
                ),
            ),
        };
        Ran {
            name: self.name,
            output,
            passed,
            failed_tests,
            result,
        }
    }

    async fn attempt(
        &self,
        config: &TestConfig,
        options: &RunOptions,
    ) -> Result<(String, bool, usize, crate::report::FileResult), String> {
        let page = self.bundle(config).await?;
        let user_context = self
            .client
            .command("browser.createUserContext", json!({}))
            .await?["userContext"]
            .as_str()
            .ok_or("browser.createUserContext: no user context in the answer")?
            .to_string();
        let ran = self.in_context(&user_context, &page, config, options).await;
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
        // A stylesheet's `url()`s are placeholders until the files they name
        // are beside the page, as a build copies them into its assets.
        let assets = self.dir.join("assets");
        let mut styles = String::new();
        for sheet in &sheets {
            let mut code = sheet.code.clone();
            for referenced in &sheet.referenced {
                let bytes = std::fs::read(&referenced.path)
                    .map_err(|e| format!("cannot read {}: {e}", referenced.path.display()))?;
                let name = crate::html::hashed_name(&referenced.path, &bytes);
                std::fs::create_dir_all(&assets)
                    .map_err(|e| format!("cannot create {}: {e}", assets.display()))?;
                std::fs::write(assets.join(&name), &bytes)
                    .map_err(|e| format!("cannot write {name}: {e}"))?;
                code = code.replace(
                    &referenced.placeholder,
                    &format!("/{}/assets/{name}", self.path),
                );
            }
            styles.push_str(&format!("<style>{code}</style>"));
        }
        let page = format!(
            "<!doctype html>\n<html><head><meta charset=\"utf-8\"><title>{title}</title>{styles}</head>\
             <body><script type=\"module\">\n\
             (async () => {{\n  \
             await new Promise((start) => Object.defineProperty(globalThis, \"__esdev_start\", {{ value: start }}));\n  \
             for (const src of [{scripts}]) await import(src);\n\
             }})().then(\n  \
             () => globalThis.__esdev_loaded(null),\n  \
             (error) => {{\n    \
             const head = String(error), stack = error && error.stack;\n    \
             // Firefox's stack leaves out the message; V8's starts with it.\n    \
             globalThis.__esdev_loaded(!stack ? head : stack.startsWith(head) ? stack : `${{head}}\\n${{stack}}`);\n  \
             }},\n);\n\
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
        options: &RunOptions,
    ) -> Result<(String, bool, usize, crate::report::FileResult), String> {
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
        let mut snapshots = FileSnapshots::new(
            self.file.clone(),
            config.update_snapshots,
            config.ci,
            config.full_diff,
            // Pruning needs every file's tests to have run, which a filter
            // says they did not.
            config.filters.is_empty(),
        );
        let store = json!({
            "options": options.to_json(),
            "update": config.update_snapshots,
            "ci": config.ci,
            "snapshots": snapshots
                .stored()?
                .into_iter()
                .map(|(key, kind, body)| (key, json!([kind, body])))
                .collect::<serde_json::Map<_, _>>(),
            "files": snapshots
                .stored_files()
                .into_iter()
                .map(|(name, bytes)| (name, json!(bytes)))
                .collect::<serde_json::Map<_, _>>(),
            // `\0` stands for the name, which no file name contains.
            "filePath": snapshots.file_path("\0").unwrap_or_default(),
            // What global setup provided, as the JSON `inject` reads.
            "provided": config
                .provided
                .as_ref()
                .and_then(|path| std::fs::read_to_string(path).ok()),
        })
        .to_string();
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
                    "arguments": [
                        { "type": "channel", "value": { "channel": format!("esdev-{}", self.path) } },
                        { "type": "string", "value": store },
                    ],
                    "target": { "context": context },
                    "awaitPromise": false,
                }),
            )
            .await?;

        let mut state = FileState {
            tally: Tally::for_file(self.file.clone()),
            locations: Locations::new(&self.origin, &self.stage, &self.runtime_test),
            snapshots: Some(snapshots),
            ..FileState::default()
        };
        // How many screenshots each case has taken under each name, for the
        // names made from the test's. A case that runs again starts over.
        let mut shots: HashMap<(u64, String), u32> = HashMap::new();
        let waited = async {
            while let Some(event) = events.recv().await {
                if let Some(message) = page_message(&event) {
                    let id = message.get(1).and_then(Value::as_u64);
                    match (message.first().and_then(Value::as_str), id) {
                        (Some("screenshot"), _) => {
                            self.screenshot(&context, &message, &state, &mut shots, config)
                                .await;
                            continue;
                        }
                        (Some("running"), Some(id)) => shots.retain(|(case, _), _| *case != id),
                        _ => {}
                    }
                }
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
        // The path a machine report names its file by: the one it was run
        // from, which is absolute, as in a process run.
        let file = self.file.display().to_string();
        let human = config.terminal_human();
        let (report, passed) = state.tally.render();
        let mut output = state.console;
        if human {
            output.push_str(&report);
        }
        let mut result = state.tally.result(&file, &self.name);
        // Written after the page is done with them, as a process run writes
        // them when it exits.
        let mut snapshots_written = true;
        if let Some(snapshots) = &mut state.snapshots {
            match snapshots.finish(&state.tally, human) {
                Ok(summary) => output.push_str(&summary),
                Err(err) => {
                    output.push_str(&format!("error: {err}\n"));
                    snapshots_written = false;
                }
            }
        }
        let why = match ended {
            Ended::Finished => None,
            Ended::TimedOut => Some(crate::test::timed_out(config.timeout)),
            Ended::Closed => Some(
                "  FAIL the browser closed before the file finished\n    \
                 It exited or crashed; the results above are what it reported first."
                    .to_string(),
            ),
        };
        if let Some(why) = &why {
            output.push_str(why);
            output.push('\n');
            // A file that stopped without a failure of its own still failed.
            if result.failed() == 0 {
                result = crate::report::FileResult::broken(
                    result.file,
                    result.name,
                    why.trim().trim_start_matches("FAIL ").trim(),
                );
            }
        }
        let finished = ended == Ended::Finished && snapshots_written;
        let failed_tests = state.tally.failed().max(result.failed());
        Ok((output, passed && finished, failed_tests, result))
    }
}

impl Job {
    /// Takes the screenshot a page asked for, checks it against its reference,
    /// and gives the page the verdict: nothing, or why it failed.
    async fn screenshot(
        &self,
        context: &str,
        message: &[Value],
        state: &FileState,
        shots: &mut HashMap<(u64, String), u32>,
        config: &TestConfig,
    ) {
        let Some(request) = message.get(2).and_then(Value::as_u64) else {
            return;
        };
        let verdict = self
            .take_screenshot(context, message, state, shots, config)
            .await;
        let reply = match verdict {
            None => json!({ "type": "null" }),
            Some(why) => json!({ "type": "string", "value": why }),
        };
        let _ = self
            .client
            .command(
                "script.callFunction",
                json!({
                    "functionDeclaration": "(request, message) => globalThis.__esdev_reply(request, message)",
                    "arguments": [{ "type": "number", "value": request }, reply],
                    "target": { "context": context },
                    "awaitPromise": false,
                }),
            )
            .await;
    }

    async fn take_screenshot(
        &self,
        context: &str,
        message: &[Value],
        state: &FileState,
        shots: &mut HashMap<(u64, String), u32>,
        config: &TestConfig,
    ) -> Option<String> {
        let case = message.get(1).and_then(Value::as_u64).unwrap_or(u64::MAX);
        let options: Value = message
            .get(5)
            .and_then(Value::as_str)
            .and_then(|text| serde_json::from_str(text).ok())
            .unwrap_or_default();
        let name = match message.get(3).and_then(Value::as_str) {
            Some(name) => name.to_string(),
            // The test's full name, numbered, as its snapshots are.
            None => {
                let test = state
                    .ids
                    .get(&case)
                    .and_then(|index| state.tally.name(*index))
                    .unwrap_or("screenshot")
                    .to_string();
                let count = shots.entry((case, test.clone())).or_insert(0);
                *count += 1;
                format!("{test} {count}")
            }
        };
        let rect = message.get(4)?;
        let number = |key: &str| rect.get(key).and_then(Value::as_f64).unwrap_or(0.0);
        let clip = json!({
            "type": "box",
            "x": number("x"),
            "y": number("y"),
            "width": number("width"),
            "height": number("height"),
        });
        let comparator = options
            .get("comparatorOptions")
            .cloned()
            .unwrap_or_default();
        let tolerance = crate::screenshot::Tolerance {
            threshold: comparator
                .get("threshold")
                .and_then(Value::as_f64)
                .unwrap_or(0.1),
            allowed_pixels: comparator
                .get("allowedMismatchedPixels")
                .and_then(Value::as_u64),
            allowed_ratio: comparator
                .get("allowedMismatchedPixelRatio")
                .and_then(Value::as_f64),
        };
        let timeout = options
            .get("timeout")
            .and_then(Value::as_u64)
            .unwrap_or(5000);
        let png = match self.stable_screenshot(context, &clip, timeout).await {
            Ok(png) => png,
            Err(err) => return Some(err),
        };
        let reference = crate::screenshot::reference_path(&self.file, &name, &self.browser);
        crate::screenshot::Check {
            root: &self.root,
            reference: &reference,
            update: config.update_snapshots,
            ci: config.ci,
            tolerance,
        }
        .run(&png)
    }

    /// Screenshots of `clip` until two in a row are the same, so an animation
    /// or a late load is not what is compared. Vitest waits the same way.
    async fn stable_screenshot(
        &self,
        context: &str,
        clip: &Value,
        timeout: u64,
    ) -> Result<Vec<u8>, String> {
        use base64::Engine as _;
        let capture = || async {
            let answer = self
                .client
                .command(
                    "browsingContext.captureScreenshot",
                    json!({ "context": context, "origin": "document", "clip": clip }),
                )
                .await
                .map_err(|err| format!("the browser did not take the screenshot: {err}"))?;
            let data = answer["data"]
                .as_str()
                .ok_or("the browser's screenshot has no data")?;
            base64::engine::general_purpose::STANDARD
                .decode(data)
                .map_err(|err| format!("the browser's screenshot is not base64: {err}"))
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout);
        let mut last = capture().await?;
        loop {
            let next = capture().await?;
            if next == last {
                return Ok(next);
            }
            if std::time::Instant::now() >= deadline {
                return Err(format!(
                    "the element was still changing after {timeout}ms;                      wait for it to settle, or raise the screenshot's timeout"
                ));
            }
            last = next;
        }
    }
}

/// The message a page posted, as the array it sent.
fn page_message(event: &Event) -> Option<Vec<Value>> {
    if event.method != "script.message" {
        return None;
    }
    let text = event.params.pointer("/data/value")?.as_str()?;
    match serde_json::from_str(text).ok()? {
        Value::Array(message) => Some(message),
        _ => None,
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
    /// Whether every case was already settled when the page finished loading
    /// — so none was queued, and no drain is coming to say it is done.
    settled_at_load: bool,
    /// What the page wrote to its console, printed ahead of the report as a
    /// process run's output is.
    console: String,
    /// The file's snapshots, checked again here as the page reports them.
    snapshots: Option<FileSnapshots>,
    /// The host's message for each snapshot that failed, by case — the fuller
    /// form of what the page threw.
    snapshot_failures: Vec<(usize, String)>,
    /// Cases that have started, so a second start is known to be a retry.
    started: std::collections::HashSet<usize>,
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
            ("running", Some(index)) => {
                // A retry: what the earlier attempt counted no longer stands.
                if !self.started.insert(index) {
                    if let Some(snapshots) = &mut self.snapshots {
                        snapshots.restart(index);
                    }
                    self.snapshot_failures.retain(|(case, _)| *case != index);
                }
                self.tally.running(index);
            }
            ("snapshot", Some(index)) => {
                let text = |at: usize| message.get(at).and_then(Value::as_str).unwrap_or_default();
                let name = self.tally.name(index).unwrap_or_default().to_string();
                if let Some(snapshots) = &mut self.snapshots
                    && let Err(failure) = snapshots.check(
                        index,
                        &name,
                        text(2),
                        text(3).to_string(),
                        text(4).to_string(),
                    )
                {
                    self.snapshot_failures.push((index, failure));
                }
            }
            ("inline-snapshot", Some(index)) => {
                let text = |at: usize| message.get(at).and_then(Value::as_str).unwrap_or_default();
                let stack = self.locations.remap(text(2));
                let existing = message.get(4).and_then(Value::as_str).map(str::to_string);
                if let Some(snapshots) = &mut self.snapshots
                    && let Err(failure) =
                        snapshots.check_inline(index, &stack, text(3).to_string(), existing)
                {
                    self.snapshot_failures.push((index, failure));
                }
            }
            ("file-snapshot", Some(index)) => {
                let text = |at: usize| message.get(at).and_then(Value::as_str).unwrap_or_default();
                if let Some(snapshots) = &mut self.snapshots
                    && let Err(failure) = snapshots.check_file(index, text(2), text(3))
                {
                    self.snapshot_failures.push((index, failure));
                }
            }
            ("bench", Some(index)) => {
                let json = message.get(2).and_then(Value::as_str).unwrap_or_default();
                self.tally.bench(index, json);
            }
            ("skipped", Some(index)) => {
                let because = message.get(2).and_then(Value::as_str).unwrap_or_default();
                self.tally.skipped(index, because);
            }
            ("finished", Some(index)) => {
                let mut passed = message.get(2).and_then(Value::as_bool) == Some(true);
                let mut detail = message
                    .get(3)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                // A snapshot the host refused that the page could not — one
                // taken in a loop, say — fails the case all the same.
                if passed {
                    let refused: Vec<&str> = self
                        .snapshot_failures
                        .iter()
                        .filter(|(case, _)| *case == index)
                        .map(|(_, failure)| failure.as_str())
                        .collect();
                    if !refused.is_empty() {
                        passed = false;
                        detail = refused.join("\n");
                    }
                }
                // Each snapshot failure the page reported in one line, in the
                // host's words: with the diff, and the file it is stored in.
                for (_, failure) in self
                    .snapshot_failures
                    .iter()
                    .filter(|(case, _)| *case == index)
                {
                    let first = failure.lines().next().unwrap_or_default();
                    if !first.is_empty() && detail.contains(first) {
                        detail = detail.replacen(first, failure, 1);
                    }
                }
                self.tally
                    .finished(index, passed, self.locations.remap(&detail));
            }
            ("drained", _) => self.drained = true,
            ("loaded", _) => {
                self.loaded = true;
                // A file whose tests were all skipped, filtered or listed
                // queued none, so its queue never drains.
                self.settled_at_load = self.tally.settled();
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
        self.loaded
            && self.tally.settled()
            && (self.registered == 0 || self.drained || self.settled_at_load)
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

/// Serves the staged pages and bundles, and the project's `public/` beneath
/// them, read-only, to the browser on loopback. The stage wins a name both
/// have: it holds the pages themselves.
async fn serve(listener: TcpListener, stage: PathBuf, public: Option<PathBuf>) {
    while let Ok((mut stream, _)) = listener.accept().await {
        let stage = stage.clone();
        let public = public.clone();
        tokio::spawn(async move {
            let Some(head) = crate::inspect::read_head(&mut stream).await else {
                return;
            };
            let path = crate::inspect::request_path(&head).unwrap_or_default();
            let found = std::iter::once(&stage)
                .chain(public.as_ref())
                .filter_map(|dir| file_for(dir, &path))
                .find_map(|file| std::fs::read(&file).ok().map(|body| (file, body)));
            let _ = match found {
                Some((file, body)) => {
                    let content_type = crate::devserver::content_type(&file);
                    crate::devserver::respond_bytes(&mut stream, content_type, &body).await
                }
                None => {
                    crate::inspect::respond(&mut stream, "404 Not Found", "text/plain", "").await
                }
            };
        });
    }
}

/// The file under `dir` a request path names. Only plain names under it:
/// nothing climbs out.
fn file_for(dir: &Path, path: &str) -> Option<PathBuf> {
    let path = path.split(['?', '#']).next()?;
    let mut file = dir.to_path_buf();
    for part in path.split('/').filter(|part| !part.is_empty()) {
        if part == "." || part == ".." || part.contains('\\') {
            return None;
        }
        file.push(part);
    }
    Some(file)
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
        let (report, passed) = state.tally.render();
        assert!(!passed);
        assert!(
            report.contains("  FAIL subtracts\n    Error: 2 !== 3"),
            "{report}"
        );
        assert!(report.contains("1 passed, 1 failed"), "{report}");
    }

    #[test]
    fn a_page_whose_tests_were_all_skipped_is_done_once_loaded() {
        // Nothing was queued, so no drain will ever say it is done.
        let mut state = FileState::default();
        state.apply(&message(json!(["registered", 0, "later"])));
        state.apply(&message(json!(["skipped", 0, ""])));
        state.apply(&message(json!(["loaded", null])));
        assert!(state.done());
    }

    #[test]
    fn a_page_with_no_tests_is_done_once_loaded() {
        let mut state = FileState::default();
        assert!(!state.done());
        state.apply(&message(json!(["loaded", null])));
        assert!(state.done());
        assert_eq!(state.tally.render(), (String::new(), true));
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
        let (report, passed) = state.tally.render();
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
        let (report, passed) = state.tally.render();
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
            Some(PathBuf::from("/stage/f0/index.html"))
        );
        assert_eq!(file_for(stage, "/f0/../../etc/passwd.js"), None);
        assert_eq!(file_for(stage, "/f0/..\\x.js"), None);
    }
}
