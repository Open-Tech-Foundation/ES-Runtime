//! End-to-end tests for failures that reach the global scope: an exception out
//! of a timer callback and a promise rejection nobody handled.
//!
//! These run the real `esrun` binary, because the behaviour under test is a
//! collaboration between three layers — the engine dispatches the event, the
//! runtime reports what the guest did not claim, and the CLI turns that into an
//! error block and an exit code. Only the binary exercises all three.

use std::process::{Command, Output};

fn esrun(code: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_esrun"))
        .args(["-e", code])
        .output()
        .expect("failed to spawn esrun")
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn an_exception_out_of_a_timer_fails_the_run() {
    // Regression: this used to be swallowed in silence — the callback throw was
    // caught to keep it from unwinding into V8 and then simply dropped, so the
    // process exited 0 with no output at all.
    let out = esrun("setTimeout(() => { throw new TypeError('boom'); }, 0);");
    assert!(!out.status.success(), "expected a non-zero exit");
    let err = stderr(&out);
    assert!(err.contains("uncaught exception"), "stderr: {err}");
    assert!(err.contains("TypeError: boom"), "stderr: {err}");
}

#[test]
fn an_error_listener_claims_a_timer_exception() {
    let out = esrun(
        "globalThis.addEventListener('error', (e) => { \
           console.log('claimed:' + e.message + ':' + (e.error instanceof TypeError)); \
           e.preventDefault(); \
         }); \
         setTimeout(() => { throw new TypeError('boom'); }, 0);",
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "claimed:boom:true");
}

#[test]
fn an_unhandled_rejection_fails_the_run() {
    let out =
        esrun("Promise.reject(new Error('nobody')); await new Promise((r) => setTimeout(r, 5));");
    assert!(!out.status.success(), "expected a non-zero exit");
    let err = stderr(&out);
    assert!(err.contains("unhandled promise rejection"), "stderr: {err}");
    assert!(err.contains("Error: nobody"), "stderr: {err}");
}

#[test]
fn an_unhandledrejection_listener_claims_the_rejection() {
    let out = esrun(
        "globalThis.onunhandledrejection = (e) => { \
           console.log('claimed:' + e.reason.message + ':' + (e.promise instanceof Promise)); \
           e.preventDefault(); \
         }; \
         Promise.reject(new Error('mine')); \
         await new Promise((r) => setTimeout(r, 5));",
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "claimed:mine:true");
}

#[test]
fn a_late_handler_fires_rejectionhandled() {
    // The report has already gone out by the time the handler attaches, so the
    // run still fails — `rejectionhandled` tells the guest that a report it saw
    // has been retracted, it does not un-report it.
    // Two turns before attaching the handler, deliberately: the host drains
    // rejections at the *end* of a tick, so a handler attached on the same turn
    // the body resumes would cancel the entry before it was ever reported — and
    // then there would be nothing to retract.
    let out = esrun(
        "const tick = () => new Promise((r) => setTimeout(r, 0)); \
         globalThis.onrejectionhandled = (e) => console.log('retracted:' + (e.promise === p)); \
         const p = Promise.reject(new Error('late')); \
         await tick(); \
         await tick(); \
         p.catch(() => {}); \
         await tick(); \
         await tick();",
    );
    assert_eq!(stdout(&out).trim(), "retracted:true");
    assert!(!out.status.success(), "the report had already gone out");
}
