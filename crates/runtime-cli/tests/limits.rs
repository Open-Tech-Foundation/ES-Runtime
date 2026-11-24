//! End-to-end tests for how a run that hit a limit ends: the status it exits
//! with and the error it prints (D238).
//!
//! `--timeout` and `--max-heap` are the guardrails a deployment puts around a
//! program, so the one thing they must never do is report success. Both stop a
//! program by terminating the engine, and the drive loop returns normally when
//! that happens; these hold the CLI to reading *why* it stopped, whichever path
//! — the watchdog thread, the loop's own deadline, the heap guard — got there.

use std::process::{Command, Output};

mod common;

const TIMEOUT: i32 = 124;

fn esrun(flags: &[&str], code: &str) -> Output {
    Command::new(common::esrun_bin())
        .args(flags)
        .arg(format!("-e={code}"))
        .output()
        .expect("failed to spawn esrun")
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn a_synchronous_runaway_times_out_with_124() {
    let out = esrun(&["--timeout=200"], "for (;;) {}");
    assert_eq!(out.status.code(), Some(TIMEOUT), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("execution timed out after 200 ms"),
        "{}",
        stderr(&out)
    );
}

/// The reported race: the watchdog and the loop's deadline fire at the same
/// instant, and when the watchdog won the run exited `0` with nothing printed.
/// Repeated, since a race that loses one run in six passes a single attempt.
#[test]
fn an_await_that_never_settles_times_out_with_124_every_time() {
    for attempt in 0..6 {
        let out = esrun(&["--timeout=150"], "await new Promise(() => {});");
        assert_eq!(
            out.status.code(),
            Some(TIMEOUT),
            "attempt {attempt}: {}",
            stderr(&out)
        );
        assert!(stderr(&out).contains("timed out"), "{}", stderr(&out));
    }
}

#[test]
fn a_run_waiting_on_a_timer_past_its_deadline_times_out() {
    let out = esrun(
        &["--timeout=150"],
        "await new Promise((r) => setTimeout(r, 10000));",
    );
    assert_eq!(out.status.code(), Some(TIMEOUT), "{}", stderr(&out));
}

#[test]
fn a_run_inside_its_deadline_succeeds() {
    let out = esrun(
        &["--timeout=5000"],
        "await new Promise((r) => setTimeout(r, 50)); console.log('done');",
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "done");
}

/// A program that asked to exit chose its status; the deadline it was racing
/// does not override that.
#[test]
fn exit_wins_over_the_timeout() {
    let out = esrun(
        &["--timeout=3000"],
        "import { exit } from 'runtime:process'; \
         setTimeout(() => exit(7), 20); await new Promise(() => {});",
    );
    assert_eq!(out.status.code(), Some(7), "{}", stderr(&out));
}

/// The reported silent success: running out of heap inside a timer ended the
/// run with status `0` and no message, while the same failure at the top level
/// was reported.
#[test]
fn running_out_of_heap_in_a_timer_is_reported_and_fails() {
    let out = esrun(
        &["--max-heap=64"],
        "setTimeout(() => { const a = []; for (;;) a.push(new Array(1e5).fill(1)); }, 0);",
    );
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("heap limit exceeded"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn running_out_of_heap_at_the_top_level_is_reported_and_fails() {
    let out = esrun(
        &["--max-heap=64"],
        "const a = []; for (;;) a.push(new Array(1e5).fill(1));",
    );
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("heap limit exceeded"),
        "{}",
        stderr(&out)
    );
}

// D241: a long native operation ran on the thread the watchdog's termination
// and a signal land on, so neither was noticed until it returned — a 300 ms
// `--timeout` took the derive's whole duration, and a SIGTERM was ignored and
// the run exited 0. These derive for far longer than the bound they assert.

/// PBKDF2 at 2^24 iterations: tens of seconds on a debug build, so a run that
/// waited for it would blow well past the bound below.
const LONG_DERIVE: &str = "const key = await crypto.subtle.importKey('raw', new Uint8Array(8), 'PBKDF2', false, ['deriveBits']); \
     console.log('DERIVING'); \
     await crypto.subtle.deriveBits({ name: 'PBKDF2', hash: 'SHA-256', salt: new Uint8Array(16), iterations: 1 << 24 }, key, 256); \
     console.log('done');";

#[test]
fn a_timeout_stops_a_long_key_derivation_promptly() {
    let started = std::time::Instant::now();
    let out = esrun(&["--timeout=300"], LONG_DERIVE);
    assert_eq!(out.status.code(), Some(TIMEOUT), "{}", stderr(&out));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "the timeout waited for the derive: {:?}",
        started.elapsed()
    );
    assert!(!String::from_utf8_lossy(&out.stdout).contains("done"));
}

#[cfg(unix)]
#[test]
fn a_signal_ends_a_run_during_a_long_key_derivation() {
    use std::io::{BufRead, BufReader};
    for (signal, code) in [("TERM", 143), ("INT", 130)] {
        let mut child = Command::new(common::esrun_bin())
            .arg(format!("-e={LONG_DERIVE}"))
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("spawn esrun");
        let mut lines = BufReader::new(child.stdout.take().expect("stdout")).lines();
        assert_eq!(lines.next().expect("line").expect("read"), "DERIVING");
        std::thread::sleep(std::time::Duration::from_millis(100));
        let sent = std::time::Instant::now();
        let status = Command::new("kill")
            .arg(format!("-{signal}"))
            .arg(child.id().to_string())
            .status()
            .expect("kill");
        assert!(status.success());
        let status = child.wait().expect("wait");
        assert_eq!(status.code(), Some(code), "SIG{signal}");
        assert!(
            sent.elapsed() < std::time::Duration::from_secs(5),
            "SIG{signal} waited for the derive: {:?}",
            sent.elapsed()
        );
    }
}

#[test]
fn a_timeout_stops_a_slow_password_hash_promptly() {
    let started = std::time::Instant::now();
    let out = esrun(
        &["--timeout=300"],
        "import { password } from 'runtime:hashing'; \
         await password.hash('pw', { algorithm: 'bcrypt', cost: 18 }); console.log('done');",
    );
    assert_eq!(out.status.code(), Some(TIMEOUT), "{}", stderr(&out));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
}

/// Past the inline bound a symmetric operation runs on the pool (D241); what it
/// computes must not change with where it ran. Checked against
/// `runtime:hashing`'s synchronous `hash()`, a separate code path, on either
/// side of the 64 KiB bound.
#[test]
fn a_digest_is_the_same_on_either_side_of_the_inline_bound() {
    let out = esrun(
        &[],
        "import { hash } from 'runtime:hashing'; \
         for (const size of [1024, 70 * 1024]) { \
           const data = new Uint8Array(size).map((_, i) => i * 7); \
           const subtle = new Uint8Array(await crypto.subtle.digest('SHA-256', data)); \
           const sync = hash('sha256', data); \
           console.log(size, subtle.every((b, i) => b === sync[i]) && subtle.length === 32); \
         }",
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "1024 true\n71680 true"
    );
}
