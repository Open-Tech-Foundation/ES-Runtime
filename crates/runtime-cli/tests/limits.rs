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
