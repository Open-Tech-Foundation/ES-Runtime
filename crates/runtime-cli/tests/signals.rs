//! Signal handling end-to-end, against the real OS.
//!
//! Unix-only: these send a genuine `SIGTERM` to the spawned `esrun` and assert
//! it was intercepted rather than killing the process. Windows has no `SIGTERM`
//! to send — its supported set is `SIGINT`/`SIGBREAK`, and raising those at a
//! specific child from a test harness means attaching to its console, which
//! tests the harness more than the runtime. The provider's Windows mapping is
//! covered by `es-runtime-default-providers`' own tests.
#![cfg(unix)]

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn fixture(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(rel)
}

/// Sends `signal` to `pid` via the `kill` binary — no libc dependency just for
/// a test, and it is the same thing an orchestrator does.
fn send(pid: u32, signal: &str) {
    let status = Command::new("kill")
        .arg(format!("-{signal}"))
        .arg(pid.to_string())
        .status()
        .expect("spawn kill");
    assert!(status.success(), "kill -{signal} {pid} failed");
}

/// A guest handler must intercept `SIGTERM`: the default action would kill the
/// process, so an exit code of 0 with the handler's own output is the proof
/// that the watch suppressed it.
#[test]
fn a_guest_handler_intercepts_a_real_sigterm() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_esrun"))
        .arg(fixture("signals.mjs"))
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn esrun");

    let stdout = child.stdout.take().expect("piped stdout");
    let mut lines = BufReader::new(stdout).lines();

    let available = lines.next().expect("first line").expect("read");
    assert!(
        available.starts_with("AVAILABLE ") && available.contains("SIGTERM"),
        "a unix host offers SIGTERM: {available}"
    );

    // Wait for the handler to be installed before signalling: sending early
    // would race the watch and hit the default action instead.
    let ready = lines.next().expect("second line").expect("read");
    assert_eq!(ready, "READY");

    let started = Instant::now();
    send(child.id(), "TERM");

    let got = lines.next().expect("handler line").expect("read");
    assert_eq!(got, "GOT SIGTERM count:1", "the handler ran");
    let released = lines.next().expect("release line").expect("read");
    assert_eq!(released, "RELEASED");

    let status = child.wait().expect("wait");
    assert!(
        status.success(),
        "SIGTERM was handled, so the process exits normally: {status}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "handling must be prompt, not a timeout"
    );
}

/// The other half: with no handler installed, `SIGTERM` still does what it
/// always did. Nothing about adding the capability changes a program that does
/// not ask for it.
#[test]
fn an_unwatched_signal_still_terminates() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_esrun"))
        .arg("-e=console.log('READY'); await new Promise(() => {});")
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn esrun");

    let stdout = child.stdout.take().expect("piped stdout");
    let mut lines = BufReader::new(stdout).lines();
    assert_eq!(lines.next().expect("line").expect("read"), "READY");

    send(child.id(), "TERM");
    let status = child.wait().expect("wait");
    assert!(
        !status.success(),
        "an unwatched SIGTERM kills the process: {status}"
    );
}

// ---- graceful shutdown -----------------------------------------------------

/// Reads lines from a child's stdout until one starts with `prefix`, and
/// returns the rest of it. Panics rather than hanging forever if the stream
/// ends first.
fn wait_for_line<R: BufRead>(lines: &mut std::io::Lines<R>, prefix: &str) -> String {
    for line in lines.by_ref() {
        let line = line.expect("read child stdout");
        if let Some(rest) = line.strip_prefix(prefix) {
            return rest.to_string();
        }
    }
    panic!("child stdout ended without a line starting with {prefix:?}");
}

/// Issues `GET path` over a raw socket and returns the whole response. Raw TCP
/// rather than an HTTP client dependency: the point is what reaches the wire,
/// and an empty reply is exactly the failure being tested for.
fn http_get(port: u16, path: &str) -> String {
    use std::io::{Read, Write};
    let mut sock = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
    write!(
        sock,
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
    )
    .expect("write request");
    let mut response = String::new();
    // An interrupted shutdown shows up here as a short or empty read.
    sock.read_to_string(&mut response).expect("read response");
    response
}

/// The point of the whole feature: an interrupt must not cut off a request that
/// is already being handled. The handler sleeps well past the moment SIGTERM
/// arrives, so a server that merely stopped would answer nothing.
#[test]
fn sigterm_drains_an_in_flight_request_before_exiting() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_esrun"))
        .arg(fixture("shutdown-server.mjs"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn esrun");

    let stdout = child.stdout.take().expect("piped stdout");
    let mut lines = BufReader::new(stdout).lines();
    let port: u16 = wait_for_line(&mut lines, "PORT ").parse().expect("port");

    // Fire the slow request, then interrupt while it is still in the handler.
    let request = std::thread::spawn(move || http_get(port, "/slow"));
    std::thread::sleep(Duration::from_millis(300));
    send(child.id(), "TERM");

    let response = request.join().expect("request thread");
    assert!(
        response.contains("200 OK") && response.contains("slow finished"),
        "the in-flight request must be answered in full, not dropped: {response:?}"
    );

    let status = child.wait().expect("wait");
    assert_eq!(
        status.code(),
        Some(143),
        "a drained SIGTERM still reports 128+15, which is what an orchestrator reads"
    );
}

/// The opposite guarantee: with no server running there is nothing in flight to
/// protect, so an interrupt must still be instant. Waiting out a grace period
/// for a plain script would be a regression, not a feature.
#[test]
fn an_interrupt_with_no_server_exits_at_once() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_esrun"))
        // A long grace, so a wrongly-applied drain would be unmistakable.
        .arg("--shutdown-grace=30000")
        .arg("-e=console.log('READY'); setInterval(() => {}, 1000);")
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn esrun");

    let stdout = child.stdout.take().expect("piped stdout");
    let mut lines = BufReader::new(stdout).lines();
    assert_eq!(lines.next().expect("line").expect("read"), "READY");

    let started = Instant::now();
    send(child.id(), "TERM");
    let status = child.wait().expect("wait");
    let elapsed = started.elapsed();

    assert_eq!(status.code(), Some(143), "128+15");
    assert!(
        elapsed < Duration::from_secs(5),
        "no server ⇒ no drain to wait for, but took {elapsed:?}"
    );
}

/// A handler that never finishes must not hold the process open forever: the
/// grace is the backstop, and a short one makes that observable.
#[test]
fn the_shutdown_grace_bounds_the_drain() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_esrun"))
        .arg("--shutdown-grace=300")
        .arg(format!(
            "-e={}",
            "import { serve } from 'runtime:http'; \
             const s = serve({ port: 0 }, () => new Promise(() => {})); \
             const { port } = await s.addr; console.log(`PORT ${port}`); await s.finished;",
        ))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn esrun");

    let stdout = child.stdout.take().expect("piped stdout");
    let mut lines = BufReader::new(stdout).lines();
    let port: u16 = wait_for_line(&mut lines, "PORT ").parse().expect("port");

    // A request the handler will never answer.
    std::thread::spawn(move || {
        let _ = std::panic::catch_unwind(|| http_get(port, "/"));
    });
    std::thread::sleep(Duration::from_millis(300));

    let started = Instant::now();
    send(child.id(), "TERM");
    let status = child.wait().expect("wait");
    let elapsed = started.elapsed();

    assert_eq!(status.code(), Some(143), "128+15");
    assert!(
        elapsed < Duration::from_secs(10),
        "the grace must cap the wait, but took {elapsed:?}"
    );
}
