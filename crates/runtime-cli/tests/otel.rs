//! End-to-end tests for `--otel`: the runtime exporting OpenTelemetry itself
//! (DECISIONS.md D89).
//!
//! These stand up a real HTTP collector on a loopback port and run the real
//! binary against it, because the claim being tested is that **an unmodified
//! program produces a usable trace**. Nothing short of the whole path — record,
//! filter, encode, POST — can show that, and the interesting failures all live
//! in the seams: a program that never imported `runtime:context` has no trace
//! id, and one that never imported `runtime:diagnostics` cannot open a span.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc;
use std::time::{Duration, Instant};

fn temp(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name)
}

/// A minimal OTLP/HTTP collector: accepts POSTs, answers 200, and hands each
/// body back over a channel.
///
/// Hand-rolled rather than pulled in as a dependency because the entire protocol
/// surface under test is "POST a JSON body to `/v1/traces`", and a test server
/// that only understands that cannot drift from the thing it is checking.
struct Collector {
    port: u16,
    bodies: mpsc::Receiver<String>,
}

impl Collector {
    fn start() -> Collector {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind collector");
        let port = listener.local_addr().expect("addr").port();
        let (tx, bodies) = mpsc::channel();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                if handle(stream, &tx).is_err() {
                    break;
                }
            }
        });
        Collector { port, bodies }
    }

    fn endpoint(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// Every payload that arrived within `timeout` of the last one.
    fn collect(&self, timeout: Duration) -> Vec<String> {
        let mut all = Vec::new();
        while let Ok(body) = self.bodies.recv_timeout(timeout) {
            all.push(body);
        }
        all
    }
}

fn handle(mut stream: TcpStream, tx: &mpsc::Sender<String>) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut length = 0usize;
    let mut path = String::new();
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Ok(());
        }
        if path.is_empty() {
            path = line.split_whitespace().nth(1).unwrap_or("").to_string();
        }
        let lower = line.to_ascii_lowercase();
        if let Some(value) = lower.strip_prefix("content-length:") {
            length = value.trim().parse().unwrap_or(0);
        }
        if line == "\r\n" || line == "\n" {
            break;
        }
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body)?;
    stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")?;
    stream.flush()?;
    // The path is prepended so a test can assert the signal it arrived on.
    let _ = tx.send(format!("{path}\n{}", String::from_utf8_lossy(&body)));
    Ok(())
}

/// Runs `source` under `--otel`, pointed at a collector, and returns the
/// payloads it received.
fn export(name: &str, source: &str, flags: &[&str]) -> Vec<String> {
    let collector = Collector::start();
    let app = temp(&format!("otel-{name}.mjs"));
    std::fs::write(&app, source).expect("write app");

    let mut command = Command::new(env!("CARGO_BIN_EXE_esrun"));
    command.current_dir(env!("CARGO_TARGET_TMPDIR"));
    command.arg(format!("--otel={}", collector.endpoint()));
    command.args(flags);
    command.arg(&app);
    let out = command.output().expect("run esrun");
    assert!(
        out.status.success(),
        "{name} exited {:?}\nstderr:\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    collector.collect(Duration::from_millis(1500))
}

/// Every span across every payload, as `(name, kind, spanId, parentSpanId,
/// traceId, status)`.
fn spans(payloads: &[String]) -> Vec<(String, i64, String, String, String, i64)> {
    let mut all = Vec::new();
    for payload in payloads {
        let (_path, body) = payload.split_once('\n').expect("path line");
        for span in body.split(r#"{"traceId":""#).skip(1) {
            let field = |key: &str| -> String {
                span.split_once(&format!(r#""{key}":""#))
                    .and_then(|(_, rest)| rest.split_once('"'))
                    .map(|(v, _)| v.to_string())
                    .unwrap_or_default()
            };
            let number = |key: &str| -> i64 {
                span.split_once(&format!(r#""{key}":"#))
                    .and_then(|(_, rest)| {
                        rest.split(|c: char| !c.is_ascii_digit())
                            .next()
                            .and_then(|d| d.parse().ok())
                    })
                    .unwrap_or(-1)
            };
            let trace = span
                .split_once('"')
                .map(|(v, _)| v.to_string())
                .unwrap_or_default();
            all.push((
                field("name"),
                number("kind"),
                field("spanId"),
                field("parentSpanId"),
                trace,
                number("code"),
            ));
        }
    }
    all
}

// ---------------------------------------------------------------------------

/// **An unmodified program produces a trace.**
///
/// The program below imports neither `runtime:context` nor
/// `runtime:diagnostics`. It does not know it is being observed. This is the
/// whole point of `--otel` being a deployment flag rather than an API.
#[test]
fn an_unmodified_program_exports_a_usable_trace() {
    let payloads = export(
        "unmodified",
        r#"
        import { serve } from "runtime:http";
        import { write, remove } from "runtime:fs";
        const server = serve({ hostname: "127.0.0.1", port: 0 }, async () => {
          await write("./otel-a.txt", "hi");
          await remove("./otel-a.txt");
          return new Response("ok");
        });
        const { port } = await server.addr;
        await fetch(`http://127.0.0.1:${port}/api/users?id=1`).then((r) => r.text());
        for (let i = 0; i < 4; i++) await new Promise((r) => setTimeout(r, 5));
        await server.stop();
        "#,
        &[
            "--allow-read",
            "--allow-write",
            "--allow-listen",
            "--allow-net",
        ],
    );
    assert!(!payloads.is_empty(), "nothing was exported");
    // Every payload is OTLP, on the traces signal, with the service named.
    for payload in &payloads {
        assert!(
            payload.starts_with("/v1/traces\n"),
            "wrong signal: {payload}"
        );
        assert!(payload.contains(r#""resourceSpans""#));
        assert!(payload.contains(r#""key":"service.name""#));
    }

    let spans = spans(&payloads);
    // The request is a SERVER span (kind 2) and is the root of its trace.
    let request = spans
        .iter()
        .find(|(name, kind, ..)| name == "GET" && *kind == 2)
        .expect("a SERVER span for the request");
    assert!(request.3.is_empty(), "the request span has a parent");

    // The handler's ops are its children, in the same trace.
    let children: Vec<_> = spans.iter().filter(|s| s.3 == request.2).collect();
    assert!(
        children.iter().any(|(name, ..)| name == "fs_write"),
        "the handler's work did not nest under the request: {spans:?}"
    );
    assert!(
        children.iter().all(|(.., trace, _)| *trace == request.4),
        "a child landed in a different trace"
    );

    // The outbound fetch is a CLIENT span (kind 3) — and in its *own* trace,
    // because it is the caller's work and not the request's.
    let fetch = spans
        .iter()
        .find(|(name, kind, ..)| name == "fetch" && *kind == 3)
        .expect("a CLIENT span for fetch");
    assert_ne!(fetch.4, request.4, "client and server share a trace");
}

/// Ids are the shapes OTLP requires: 32 hex for a trace, 16 for a span, and a
/// parent that names a span in the same payload set.
#[test]
fn exported_ids_are_well_formed_and_resolve() {
    let payloads = export(
        "ids",
        r#"
        import { serve } from "runtime:http";
        import { write, remove } from "runtime:fs";
        const server = serve({ hostname: "127.0.0.1", port: 0 }, async () => {
          await write("./otel-b.txt", "hi");
          await remove("./otel-b.txt");
          return new Response("ok");
        });
        const { port } = await server.addr;
        await fetch(`http://127.0.0.1:${port}/`).then((r) => r.text());
        for (let i = 0; i < 4; i++) await new Promise((r) => setTimeout(r, 5));
        await server.stop();
        "#,
        &[
            "--allow-read",
            "--allow-write",
            "--allow-listen",
            "--allow-net",
        ],
    );
    let spans = spans(&payloads);
    assert!(!spans.is_empty());
    let hex = |s: &str, n: usize| s.len() == n && s.chars().all(|c| c.is_ascii_hexdigit());
    for (name, _, span_id, parent, trace, _) in &spans {
        assert!(hex(trace, 32), "{name}: bad traceId {trace:?}");
        assert!(hex(span_id, 16), "{name}: bad spanId {span_id:?}");
        assert!(
            parent.is_empty() || hex(parent, 16),
            "{name}: bad parent {parent:?}"
        );
    }
    let ids: Vec<&String> = spans.iter().map(|(_, _, id, ..)| id).collect();
    let parented: Vec<_> = spans
        .iter()
        .filter(|(_, _, _, p, _, _)| !p.is_empty())
        .collect();
    assert!(!parented.is_empty(), "nothing was nested");
    for (name, _, _, parent, ..) in &parented {
        assert!(
            ids.contains(&parent),
            "{name}: parent {parent} names no span"
        );
    }
}

/// Timestamps are absolute Unix nanoseconds, near now, and ordered.
#[test]
fn exported_timestamps_are_absolute_and_ordered() {
    let payloads = export(
        "times",
        r#"
        import { write, remove } from "runtime:fs";
        await write("./otel-c.txt", "hi");
        await remove("./otel-c.txt");
        await new Promise((r) => setTimeout(r, 20));
        "#,
        &["--allow-read", "--allow-write"],
    );
    let body = payloads.first().expect("a payload").clone();
    let times: Vec<u128> = body
        .split(r#"TimeUnixNano":""#)
        .skip(1)
        .filter_map(|rest| rest.split_once('"').and_then(|(v, _)| v.parse().ok()))
        .collect();
    assert!(times.len() >= 2, "no timestamps in {body}");
    let now_ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("after the epoch")
        .as_nanos();
    for t in &times {
        // Within an hour of now: absolute wall time, not a monotonic reading
        // that happens to be a small number.
        let delta = now_ns.abs_diff(*t);
        assert!(
            delta < 3_600_000_000_000,
            "timestamp {t} is not near now ({now_ns})"
        );
    }
    // Start before end, pairwise, in emission order.
    for pair in times.chunks(2) {
        if let [start, end] = pair {
            assert!(end >= start, "a span ended before it started");
        }
    }
}

/// `--otel-min-duration` and `--otel-sample` are applied before anything is
/// sent, so a deployment can bound the volume without touching the program.
#[test]
fn export_filters_are_applied_before_sending() {
    const APP: &str = r#"
        import { write, remove } from "runtime:fs";
        for (let i = 0; i < 5; i++) {
          await write("./otel-d.txt", "hi");
          await remove("./otel-d.txt");
        }
        await new Promise((r) => setTimeout(r, 20));
    "#;
    let unfiltered = spans(&export("nofilter", APP, &["--allow-read", "--allow-write"]));
    assert!(!unfiltered.is_empty(), "baseline exported nothing");

    // Nothing here takes a whole second.
    let slow = export(
        "slow",
        APP,
        &["--allow-read", "--allow-write", "--otel-min-duration=1000"],
    );
    assert!(
        spans(&slow).is_empty(),
        "min-duration did not filter: {slow:?}"
    );

    // Everything shares this agent's root trace, so zero sampling drops it all.
    let none = export(
        "sample0",
        APP,
        &["--allow-read", "--allow-write", "--otel-sample=0"],
    );
    assert!(
        spans(&none).is_empty(),
        "sample=0 exported something: {none:?}"
    );
}

/// A bad `--otel` option is refused before the program runs, rather than
/// producing a run that silently exports nothing.
#[test]
fn malformed_otel_options_are_refused() {
    for flag in [
        "--otel-sample=2",
        "--otel-sample=-1",
        "--otel-sample=abc",
        "--otel-min-duration=-5",
        "--otel-min-duration=nope",
        "--otel-service",
    ] {
        let app = temp("otel-bad.mjs");
        std::fs::write(&app, "console.log('ran');\n").expect("write app");
        let out = Command::new(env!("CARGO_BIN_EXE_esrun"))
            .current_dir(env!("CARGO_TARGET_TMPDIR"))
            .arg(flag)
            .arg(&app)
            .output()
            .expect("run esrun");
        assert!(!out.status.success(), "{flag} was accepted");
        assert!(
            !String::from_utf8_lossy(&out.stdout).contains("ran"),
            "{flag} ran the program before refusing"
        );
    }
}

/// A collector that is down must not take the program with it.
///
/// Telemetry is best-effort by construction: the export is handed to the sink
/// and the loop moves on, so an unreachable collector costs a warning and
/// nothing else.
#[test]
fn an_unreachable_collector_does_not_fail_the_program() {
    // A port nothing is listening on: bind, read the port, drop the listener.
    let port = {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.local_addr().expect("addr").port()
    };
    let app = temp("otel-down.mjs");
    std::fs::write(
        &app,
        r#"
        import { write, remove } from "runtime:fs";
        await write("./otel-e.txt", "hi");
        await remove("./otel-e.txt");
        await new Promise((r) => setTimeout(r, 30));
        console.log("finished");
        "#,
    )
    .expect("write app");

    let started = Instant::now();
    let out = Command::new(env!("CARGO_BIN_EXE_esrun"))
        .current_dir(env!("CARGO_TARGET_TMPDIR"))
        .arg(format!("--otel=http://127.0.0.1:{port}"))
        .args(["--allow-read", "--allow-write"])
        .arg(&app)
        .output()
        .expect("run esrun");
    assert!(
        out.status.success(),
        "an unreachable collector failed the run: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("finished"));
    // And it did not wait on the collector to find out.
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "the run blocked on a dead collector"
    );
}

/// Exporting needs no capability **in the program**: the deployment turned it
/// on, so the program neither reaches the collector nor can read its own traces.
#[test]
fn exporting_grants_the_program_nothing() {
    let collector = Collector::start();
    let app = temp("otel-caps.mjs");
    std::fs::write(
        &app,
        r#"
        import { subscribe } from "runtime:diagnostics";
        // The deployment is exporting, but this program was granted nothing:
        // it can neither read the recording nor reach the collector.
        try { subscribe({}, () => {}); console.log("READ: allowed"); }
        catch (e) { console.log(`READ: ${e.name}`); }
        try { await fetch("http://127.0.0.1:1/"); console.log("NET: allowed"); }
        catch (e) { console.log(`NET: ${e.name}`); }
        await new Promise((r) => setTimeout(r, 20));
        "#,
    )
    .expect("write app");

    let out = Command::new(env!("CARGO_BIN_EXE_esrun"))
        .current_dir(env!("CARGO_TARGET_TMPDIR"))
        .arg(format!("--otel={}", collector.endpoint()))
        .arg(&app)
        .output()
        .expect("run esrun");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("READ: NotAllowedError"), "stdout: {stdout}");
    assert!(stdout.contains("NET: NotAllowedError"), "stdout: {stdout}");
    // And the deployment still got its telemetry.
    let payloads = collector.collect(Duration::from_millis(1500));
    assert!(
        !payloads.is_empty(),
        "granting nothing also exported nothing"
    );
}

/// `--otel-service` names the service; without it the entry's file name does.
#[test]
fn the_service_name_defaults_to_the_entry() {
    let payloads = export(
        "named-service",
        r#"
        import { write, remove } from "runtime:fs";
        await write("./otel-f.txt", "hi");
        await remove("./otel-f.txt");
        await new Promise((r) => setTimeout(r, 20));
        "#,
        &["--allow-read", "--allow-write"],
    );
    let body = payloads.first().expect("a payload");
    // Two services both exporting as "esrun" are indistinguishable in a
    // collector, which is the one thing service.name exists to prevent.
    assert!(
        body.contains(r#""stringValue":"otel-named-service""#),
        "{body}"
    );

    let explicit = export(
        "explicit-service",
        r#"await new Promise((r) => setTimeout(r, 20));"#,
        &["--otel-service=checkout-api"],
    );
    assert!(
        explicit
            .iter()
            .any(|b| b.contains(r#""stringValue":"checkout-api""#)),
        "{explicit:?}"
    );
}

/// A failing op is exported with `status.code: 2` — a trace that only shows
/// successes is not a trace.
#[test]
fn a_failure_is_exported_as_an_error_status() {
    let payloads = export(
        "errors",
        r#"
        import { file } from "runtime:fs";
        try { await file("./definitely-not-here.txt").text(); } catch { /* expected */ }
        await new Promise((r) => setTimeout(r, 20));
        "#,
        &["--allow-read"],
    );
    let spans = spans(&payloads);
    assert!(
        spans.iter().any(|(.., status)| *status == 2),
        "no error status exported: {spans:?}"
    );
}

/// A failure is exported as OpenTelemetry records one: a `status.message` and a
/// timestamped `exception` **event**.
///
/// A span that only said `status: ERROR` is counted in an error rate and useless
/// to open — the event is what a backend's exception view is built on.
#[test]
fn a_failure_is_exported_with_an_exception_event() {
    let payloads = export(
        "exception-event",
        r#"
        import { file } from "runtime:fs";
        try { await file("./definitely-not-here.txt").text(); } catch { /* expected */ }
        await new Promise((r) => setTimeout(r, 20));
        "#,
        &["--allow-read"],
    );
    let body = payloads.join("");
    assert!(body.contains(r#""code":2"#), "no error status: {body}");
    assert!(
        body.contains(r#""name":"exception""#),
        "no exception event: {body}"
    );
    assert!(body.contains(r#""key":"exception.type""#), "{body}");
    assert!(body.contains(r#""key":"exception.message""#), "{body}");
    // The message rides on the status too, which is where a list view reads it.
    assert!(body.contains(r#""message":"#), "{body}");
    // The exporter holds `detail`, so the reason is present rather than blank.
    assert!(body.contains("definitely-not-here"), "{body}");
}

/// The resource says where the spans came from, and the scope says what produced
/// them. Omitting either is legal and makes a trace look like it came from
/// nowhere in particular.
#[test]
fn exported_spans_carry_resource_and_scope_identity() {
    let payloads = export(
        "resource",
        r#"
        import { write, remove } from "runtime:fs";
        await write("./res.txt", "hi");
        await remove("./res.txt");
        await new Promise((r) => setTimeout(r, 20));
        "#,
        &["--allow-read", "--allow-write"],
    );
    let body = payloads.join("");
    for key in [
        "service.name",
        "telemetry.sdk.name",
        "telemetry.sdk.language",
        "telemetry.sdk.version",
        "process.runtime.name",
        "process.runtime.version",
    ] {
        assert!(
            body.contains(&format!(r#""key":"{key}""#)),
            "missing {key}: {body}"
        );
    }
    assert!(
        body.contains(r#""name":"esrun","version":"#),
        "no scope version: {body}"
    );
    // Everything exported was sampled by definition — it would not be here
    // otherwise — so the flag says so rather than being left unset.
    assert!(body.contains(r#""flags":1"#), "no sampled flag: {body}");
}
