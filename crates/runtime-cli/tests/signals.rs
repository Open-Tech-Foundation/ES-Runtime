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
        .arg("-e")
        .arg("console.log('READY'); await new Promise(() => {});")
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
