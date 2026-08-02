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
        .arg(format!("-e={code}"))
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

// ---- a failed socket must not report twice -----------------------------------
//
// One connect (or bind) failure rejects several promises, because `opened`,
// `addr`, the streams, close() and startTls() all derive from the same pending
// op. A program can only handle one of them, so the leftovers used to reach the
// global scope and fail a run that had already dealt with the error — an
// unreachable host taking down a server that handled it. `--deny-all
// --allow-net=<other>` is the deterministic way to fail a connect: the
// allowlist refuses it before DNS, so these do not touch the network.

/// Runs `code` under `flags` — the socket cases need a permission flag to fail
/// a connection without depending on name resolution.
fn esrun_with(flags: &[&str], code: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_esrun"))
        .args(flags)
        .arg(format!("-e={code}"))
        .output()
        .expect("failed to spawn esrun")
}

#[test]
fn a_connect_failure_handled_through_the_streams_does_not_also_fail_the_run() {
    // The regression: the program reads `readable`, catches the error there,
    // and the eagerly-built `opened` was left over as an unhandled rejection.
    let out = esrun_with(
        &["--deny-all", "--allow-net=allowed.invalid"],
        "import { connect } from 'runtime:net'; \
         const sock = connect({ hostname: 'denied.invalid', port: 443 }); \
         try { await sock.readable.getReader().read(); } \
         catch (e) { console.log('caught:' + e.code); } \
         console.log('continued');",
    );
    assert!(
        out.status.success(),
        "a handled connect failure must not fail the run; stderr: {}",
        stderr(&out)
    );
    assert!(
        stdout(&out).contains("caught:ERR_PERMISSION_DENIED"),
        "{}",
        stdout(&out)
    );
    assert!(stdout(&out).contains("continued"), "{}", stdout(&out));
    assert!(
        !stderr(&out).contains("unhandled"),
        "stderr: {}",
        stderr(&out)
    );
}

#[test]
fn awaiting_opened_still_rejects() {
    // The other half of the fix: marking the duplicate handled must not consume
    // the error for the surface that documents it.
    let out = esrun_with(
        &["--deny-all", "--allow-net=allowed.invalid"],
        "import { connect } from 'runtime:net'; \
         const sock = connect({ hostname: 'denied.invalid', port: 443 }); \
         try { await sock.opened; console.log('NO THROW'); } \
         catch (e) { console.log('caught:' + e.code); }",
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "caught:ERR_PERMISSION_DENIED");
}

#[test]
fn an_ignored_failed_socket_does_not_take_the_process_down() {
    // A socket nobody ever consumes. The failure goes unreported — the tradeoff
    // for not killing a server over a connection it never looked at — but the
    // run itself must survive it.
    let out = esrun_with(
        &["--deny-all", "--allow-net=allowed.invalid"],
        "import { connect } from 'runtime:net'; \
         connect({ hostname: 'denied.invalid', port: 443 }); \
         await new Promise((r) => setTimeout(r, 50)); \
         console.log('survived');",
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "survived");
}

#[test]
fn a_bind_failure_handled_through_accept_does_not_also_fail_the_run() {
    // Listener.addr is the same shape as Socket.opened.
    let out = esrun_with(
        &["--deny-all", "--allow-listen=9"],
        "import { listen } from 'runtime:net'; \
         const l = listen({ port: 8123 }); \
         try { await l.accept(); } catch (e) { console.log('caught:' + e.code); } \
         console.log('continued');",
    );
    assert!(
        out.status.success(),
        "a handled bind failure must not fail the run; stderr: {}",
        stderr(&out)
    );
    assert!(
        stdout(&out).contains("caught:ERR_PERMISSION_DENIED"),
        "{}",
        stdout(&out)
    );
    assert!(
        !stderr(&out).contains("unhandled"),
        "stderr: {}",
        stderr(&out)
    );
}
