//! End-to-end tests for `runtime:process`'s `unrefTimer` and `refTimer`
//! (DECISIONS.md D111).
//!
//! Against the real binary, because the property under test is the process's
//! own lifetime: whether a scheduled timer is a reason for it to keep running.

use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

fn temp(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name)
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Runs `source` under `esrun --deny-all`, and returns its lines and how long
/// the process lived.
fn run(name: &str, source: &str) -> (Vec<String>, Duration) {
    let app = temp(&format!("timers-{name}.mjs"));
    std::fs::write(&app, source).expect("write app");
    let started = Instant::now();
    let out = Command::new(env!("CARGO_BIN_EXE_esrun"))
        .current_dir(env!("CARGO_TARGET_TMPDIR"))
        // Ungated: it decides only when this program may end.
        .arg("--deny-all")
        .arg(&app)
        .output()
        .expect("run esrun");
    let lived = started.elapsed();
    assert!(
        out.status.success(),
        "{name} exited {:?}\nstdout:\n{}\nstderr:\n{}",
        out.status.code(),
        stdout(&out),
        stderr(&out)
    );
    (stdout(&out).lines().map(str::to_string).collect(), lived)
}

/// A heartbeat that should not be why a program never exits: it fires while
/// other work keeps the process running, and does not keep it running alone.
#[test]
fn an_unreferenced_interval_runs_until_the_real_work_is_done() {
    let (out, lived) = run(
        "heartbeat",
        r#"
        import { unrefTimer } from "runtime:process";
        let beats = 0;
        unrefTimer(setInterval(() => { beats += 1; }, 20));
        setTimeout(() => console.log("beat while working:", beats >= 3), 150);
        "#,
    );
    assert_eq!(out, ["beat while working: true"]);
    assert!(
        lived < Duration::from_secs(5),
        "the heartbeat held the process: {lived:?}"
    );
}

#[test]
fn a_timer_referenced_again_holds_the_process_until_it_fires() {
    let (out, _) = run(
        "reref",
        r#"
        import { refTimer, unrefTimer } from "runtime:process";
        const t = setTimeout(() => console.log("fired"), 50);
        unrefTimer(t);
        refTimer(t);
        "#,
    );
    assert_eq!(out, ["fired"]);
}

/// An unreferenced timeout that nothing outlives never fires: the process has
/// no reason to wait for it.
#[test]
fn an_unreferenced_timeout_alone_does_not_keep_the_process() {
    let (out, lived) = run(
        "alone",
        r#"
        import { unrefTimer } from "runtime:process";
        unrefTimer(setTimeout(() => console.log("fired"), 10_000));
        console.log("done");
        "#,
    );
    assert_eq!(out, ["done"]);
    assert!(lived < Duration::from_secs(5), "{lived:?}");
}

#[test]
fn only_a_timer_id_is_accepted_and_a_spent_one_is_left_alone() {
    let (out, _) = run(
        "ids",
        r#"
        import { unrefTimer, refTimer } from "runtime:process";
        try { unrefTimer("soon"); } catch (e) { console.log(e.name, e.message); }
        const spent = setTimeout(() => {}, 0);
        clearTimeout(spent);
        unrefTimer(spent);
        refTimer(123456);
        console.log("ok");
        // The builtin behind them is not part of the global surface.
        console.log(Object.keys(globalThis).includes("__timer_ref"));
        "#,
    );
    assert_eq!(
        out,
        [
            "TypeError unrefTimer needs the id setTimeout or setInterval returned",
            "ok",
            "false",
        ]
    );
}

/// Each agent has its own timers: in a worker, `unrefTimer` lets that worker's
/// timer go, which still fires while the worker works. Whether the *worker*
/// holds the process open is the parent's `worker.unref()`, not this.
#[test]
fn a_worker_lets_its_own_timer_go_and_its_parent_lets_the_worker_go() {
    let worker = temp("timers-worker.mjs");
    std::fs::write(
        &worker,
        r#"
        import { unrefTimer } from "runtime:process";
        let beats = 0;
        unrefTimer(setInterval(() => { beats += 1; }, 20));
        setTimeout(() => postMessage(`worker beat while working: ${beats >= 3}`), 150);
        "#,
    )
    .expect("write worker");
    let app = temp("timers-worker-main.mjs");
    std::fs::write(
        &app,
        r#"
        const w = new Worker(new URL("./timers-worker.mjs", import.meta.url), {
          type: "module",
          permissions: ["imports"],
        });
        w.onmessage = (e) => {
          console.log(e.data);
          // Its job done, the worker is no reason to stay.
          w.unref();
        };
        "#,
    )
    .expect("write app");
    let started = Instant::now();
    let out = Command::new(env!("CARGO_BIN_EXE_esrun"))
        .current_dir(env!("CARGO_TARGET_TMPDIR"))
        .args(["--allow-imports", "--allow-workers"])
        .arg(&app)
        .output()
        .expect("run esrun");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    assert_eq!(stdout(&out).trim(), "worker beat while working: true");
    assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
}
