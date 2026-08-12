//! End-to-end tests for `esdev --inspect`, driven the way a debugger drives it.
//!
//! These spawn the real binary, attach over the real WebSocket and speak real
//! Chrome DevTools Protocol. Nothing here mocks the transport, because the whole
//! feature *is* the transport: the parts that can be wrong are the handshake, the
//! ordering between a client attaching and a session being connected, and whether
//! a paused program is actually stopped.
//!
//! **They adapt to the build.** The inspector is compiled in only when
//! `ES_RUNTIME_INSPECTOR=1` was set (DECISIONS D59), so each test first asks the
//! binary which build it is. Without the inspector there is exactly one thing to
//! assert — that `--inspect` fails with the line telling you how to get one — and
//! [`inspector_available`] is itself that assertion.

// A test reporting why it skipped is talking to whoever reads the run.
#![allow(clippy::print_stderr)]

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

fn temp(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name)
}

fn write(name: &str, contents: &str) -> PathBuf {
    let path = temp(name);
    std::fs::write(&path, contents).expect("write temp file");
    path
}

fn esdev() -> Command {
    Command::new(env!("CARGO_BIN_EXE_esdev"))
}

/// Whether this build has an inspector at all — and, when it does not, the
/// assertion that it says so properly.
fn inspector_available() -> bool {
    let out = esdev()
        .arg("--inspect=0")
        .arg("-e=1")
        .output()
        .expect("spawn esdev");
    if out.status.success() {
        return true;
    }
    let message = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        message.contains("ES_RUNTIME_INSPECTOR=1"),
        "a build without an inspector must say how to get one, got: {message}"
    );
    false
}

/// A port nothing is listening on. Racy in principle; the window is one bind and
/// the alternative is a fixed port that collides with whatever else is running.
fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    listener.local_addr().expect("addr").port()
}

/// A spawned `esdev` that is killed when the test ends, however it ends.
struct Program {
    child: Child,
    port: u16,
    /// The child's stderr, held open for the process's life. Dropping it would
    /// close the pipe under a program that is still writing to it, and a failed
    /// `eprintln!` is a panic — the test would then be measuring its own reader.
    _stderr: std::io::Lines<BufReader<std::process::ChildStderr>>,
}

impl Drop for Program {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Program {
    /// Starts `esdev <flag> <entry>` and waits for the line announcing the port,
    /// so a test never races the listener.
    fn start(flag: &str, entry: &PathBuf) -> Program {
        let port = free_port();
        let mut child = esdev()
            .arg(format!("{flag}={port}"))
            .arg(entry)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn esdev");

        let stderr = child.stderr.take().expect("stderr");
        let mut lines = BufReader::new(stderr).lines();
        let announced = lines
            .next()
            .and_then(Result::ok)
            .unwrap_or_else(|| String::from("<no output>"));
        assert!(
            announced.contains(&format!("ws://127.0.0.1:{port}/")),
            "expected the debugger's address, got: {announced}"
        );
        Program {
            child,
            port,
            _stderr: lines,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("127.0.0.1:{}{path}", self.port)
    }

    fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

/// A plain HTTP GET, since the discovery endpoints are plain HTTP and this crate
/// has no client.
async fn get(address: &str, path: &str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut stream = TcpStream::connect(address).await.expect("connect");
    stream
        .write_all(format!("GET {path} HTTP/1.1\r\nHost: {address}\r\n\r\n").as_bytes())
        .await
        .expect("send request");
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .await
        .expect("read response");
    response
}

/// A Chrome DevTools Protocol client: enough of one to drive a debugger.
struct Debugger {
    socket: WebSocketStream<TcpStream>,
    next_id: i64,
    /// Notifications that arrived while a response was being waited for.
    ///
    /// Kept rather than dropped because V8 sends them *first*: the
    /// `Debugger.scriptParsed` for every script it already knows arrives before
    /// the reply to the `Debugger.enable` that asked for them, so a client that
    /// reads until it sees its response has already thrown away the answer.
    events: Vec<String>,
}

impl Debugger {
    async fn attach(program: &Program) -> Debugger {
        // The WebSocket URL is the one `/json/list` advertised, not a guess —
        // which is both what a real client does and what proves the two agree.
        let listing = get(&program.url(""), "/json/list").await;
        let url = listing
            .split("\"webSocketDebuggerUrl\":\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .unwrap_or_else(|| panic!("no target in {listing}"))
            .to_string();

        let stream = TcpStream::connect(program.url(""))
            .await
            .expect("connect to the debugger");
        let (socket, _) = tokio_tungstenite::client_async(url, stream)
            .await
            .expect("websocket handshake");
        Debugger {
            socket,
            next_id: 0,
            events: Vec::new(),
        }
    }

    /// Sends a command and returns its response, collecting the notifications
    /// that arrive first.
    async fn call(&mut self, method: &str, params: &str) -> String {
        self.next_id += 1;
        let id = self.next_id;
        let message = format!(r#"{{"id":{id},"method":"{method}","params":{params}}}"#);
        self.socket
            .send(Message::Text(message.into()))
            .await
            .expect("send command");
        loop {
            let reply = self.receive().await;
            if reply.contains(&format!("\"id\":{id}")) {
                return reply;
            }
            self.events.push(reply);
        }
    }

    /// Waits for a notification of `method`, failing rather than hanging if the
    /// program never sends one.
    async fn wait_for(&mut self, method: &str) -> String {
        let needle = format!("\"method\":\"{method}\"");
        if let Some(index) = self.events.iter().position(|e| e.contains(&needle)) {
            return self.events.remove(index);
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            let message = self.receive().await;
            if message.contains(&needle) {
                return message;
            }
        }
        panic!("no {method} within 10s");
    }

    async fn receive(&mut self) -> String {
        match tokio::time::timeout(Duration::from_secs(10), self.socket.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => text.as_str().to_string(),
            Ok(Some(Ok(_))) => String::new(),
            other => panic!("the debugger connection ended: {other:?}"),
        }
    }
}

#[tokio::test]
async fn the_discovery_endpoints_describe_the_target() {
    if !inspector_available() {
        return;
    }
    let app = write("inspect-list.mjs", "setInterval(() => {}, 1000);\n");
    let program = Program::start("--inspect", &app);

    let version = get(&program.url(""), "/json/version").await;
    assert!(
        version.contains("\"Protocol-Version\":\"1.3\""),
        "{version}"
    );

    let listing = get(&program.url(""), "/json/list").await;
    assert!(listing.contains("\"type\":\"node\""), "{listing}");
    assert!(
        listing.contains(&format!("ws://127.0.0.1:{}/", program.port)),
        "{listing}"
    );
    assert!(listing.contains("inspect-list.mjs"), "{listing}");
}

#[tokio::test]
async fn a_breakpoint_stops_the_program_where_it_was_set() {
    if !inspector_available() {
        return;
    }
    let app = write(
        "inspect-break.mjs",
        "let n = 0;\nfunction step() {\n  n += 1;\n}\nsetInterval(step, 50);\n",
    );
    let program = Program::start("--inspect", &app);
    let mut debugger = Debugger::attach(&program).await;

    debugger.call("Runtime.enable", "{}").await;
    debugger.call("Debugger.enable", "{}").await;

    // The entry module is announced like any other script, with its own URL —
    // which is what makes a breakpoint set by URL land in the file the developer
    // is looking at.
    let mut url = None;
    for _ in 0..10 {
        let parsed = debugger.wait_for("Debugger.scriptParsed").await;
        if parsed.contains("inspect-break.mjs") {
            url = parsed
                .split("\"url\":\"")
                .nth(1)
                .and_then(|rest| rest.split('"').next())
                .map(str::to_string);
            break;
        }
    }
    let url = url.expect("the entry module was never announced to the debugger");

    let set = debugger
        .call(
            "Debugger.setBreakpointByUrl",
            &format!(r#"{{"lineNumber":2,"url":"{url}"}}"#),
        )
        .await;
    assert!(set.contains("breakpointId"), "{set}");

    let paused = debugger.wait_for("Debugger.paused").await;
    assert!(paused.contains("\"lineNumber\":2"), "{paused}");

    // A paused program is a stopped one: the local is readable, and reading it
    // through the debugger is the thing an inspector exists for.
    let frame = paused
        .split("\"callFrameId\":\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("a call frame");
    let evaluated = debugger
        .call(
            "Debugger.evaluateOnCallFrame",
            &format!(r#"{{"callFrameId":"{frame}","expression":"n"}}"#),
        )
        .await;
    assert!(evaluated.contains("\"type\":\"number\""), "{evaluated}");

    let resumed = debugger.call("Debugger.resume", "{}").await;
    assert!(!resumed.contains("\"error\""), "{resumed}");
}

#[tokio::test]
async fn evaluate_answers_while_the_event_loop_is_parked() {
    if !inspector_available() {
        return;
    }
    // Nothing to do for a full minute: without the driver's waker being rung,
    // this is a request that would not be answered until the timer came due.
    let app = write(
        "inspect-idle.mjs",
        "globalThis.marker = 'alive';\nsetTimeout(() => {}, 60000);\n",
    );
    let program = Program::start("--inspect", &app);
    let mut debugger = Debugger::attach(&program).await;
    debugger.call("Runtime.enable", "{}").await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    let started = Instant::now();
    let evaluated = debugger
        .call(
            "Runtime.evaluate",
            r#"{"expression":"globalThis.marker","returnByValue":true}"#,
        )
        .await;
    assert!(evaluated.contains("alive"), "{evaluated}");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "an idle program answered its debugger only after {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn inspect_brk_holds_the_program_before_its_first_statement() {
    if !inspector_available() {
        return;
    }
    let app = write("inspect-brk.mjs", "console.log('ran');\n");
    let mut program = Program::start("--inspect-brk", &app);

    // A program this short is over in milliseconds. Still running after a
    // second is the observable form of "it has not run a statement".
    std::thread::sleep(Duration::from_millis(800));
    assert!(
        program.is_running(),
        "--inspect-brk let the program run without a debugger"
    );

    let mut debugger = Debugger::attach(&program).await;
    debugger.call("Runtime.enable", "{}").await;
    debugger.call("Debugger.enable", "{}").await;
    debugger.call("Runtime.runIfWaitingForDebugger", "{}").await;

    let paused = debugger.wait_for("Debugger.paused").await;
    assert!(paused.contains("Break on start"), "{paused}");
    debugger.call("Debugger.resume", "{}").await;

    let mut stdout = String::new();
    let out = program.child.stdout.take().expect("stdout");
    std::io::Read::read_to_string(&mut std::io::BufReader::new(out), &mut stdout)
        .expect("read stdout");
    assert_eq!(stdout.trim(), "ran");
}

#[tokio::test]
async fn a_disconnected_debugger_does_not_leave_the_program_stopped() {
    if !inspector_available() {
        return;
    }
    let app = write("inspect-drop.mjs", "console.log('ran');\n");
    let mut program = Program::start("--inspect-brk", &app);
    {
        // Attach and leave without ever releasing it. A debugger that gave up is
        // not a reason for the program never to start.
        let _debugger = Debugger::attach(&program).await;
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while program.is_running() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        !program.is_running(),
        "the program stayed held after its debugger disconnected"
    );
}

#[test]
fn esrun_has_no_inspect_flag_at_all() {
    // Cargo only exports `CARGO_BIN_EXE_*` for this package's binaries, so
    // `esrun` is found beside us or the test has nothing to check.
    let Some(esrun) = PathBuf::from(env!("CARGO_BIN_EXE_esdev"))
        .parent()
        .map(|dir| dir.join(format!("esrun{}", std::env::consts::EXE_SUFFIX)))
        .filter(|path| path.exists())
    else {
        eprintln!("skipped: esrun is not built in this target directory");
        return;
    };
    let out = Command::new(esrun)
        .arg("--inspect")
        .arg("-e=1")
        .output()
        .expect("spawn esrun");
    assert!(!out.status.success(), "esrun accepted --inspect");
    let message = String::from_utf8_lossy(&out.stderr);
    assert!(
        message.contains("unknown option: --inspect"),
        "esrun must not know the flag at all, got: {message}"
    );
}
