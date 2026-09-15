//! End-to-end tests for `runtime:process`'s self-reporting — `memoryUsage()`
//! and `uptime()` (DECISIONS.md D91).
//!
//! Against the real binary, because both answers are properties of a live
//! isolate: the heap ceiling is the one V8 was built with, and an agent's uptime
//! only differs from the process's inside a real worker on a real thread.

use std::path::PathBuf;
use std::process::{Command, Output};

fn temp(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name)
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn lines(name: &str, source: &str, flags: &[&str]) -> Vec<String> {
    let app = temp(&format!("pm-{name}.mjs"));
    std::fs::write(&app, source).expect("write app");
    let mut command = Command::new(env!("CARGO_BIN_EXE_esrun"));
    command.current_dir(env!("CARGO_TARGET_TMPDIR"));
    command.args(flags);
    command.arg(&app);
    let out = command.output().expect("run esrun");
    assert!(
        out.status.success(),
        "{name} exited {:?}\nstdout:\n{}\nstderr:\n{}",
        out.status.code(),
        stdout(&out),
        stderr(&out)
    );
    stdout(&out).lines().map(str::to_string).collect()
}

/// **Both are ungated**, by the rule `runtime:process` already applies to
/// `platform` and `args`: they report only what the caller could discover about
/// itself anyway — its own ceiling by allocating until the heap guard stops it,
/// its own age by counting.
///
/// What it could *not* discover — the process's resident set, which is about the
/// agents around it — is deliberately not here.
#[test]
fn self_reporting_needs_no_capability() {
    let out = lines(
        "ungated",
        r#"
        import { memoryUsage, uptime } from "runtime:process";
        const m = memoryUsage();
        console.log(Object.keys(m).sort().join(","));
        console.log(m.heapUsed > 0, m.heapLimit > m.heapUsed, m.external >= 0);
        console.log(typeof uptime(), uptime() >= 0);
        // The process-wide figure is not on this module.
        console.log("rss" in m);
        "#,
        // Nothing granted at all — esrun's default (D65).
        &[],
    );
    assert_eq!(out[0], "external,heapLimit,heapUsed");
    assert_eq!(out[1], "true true true");
    assert_eq!(out[2], "number true");
    assert_eq!(out[3], "false", "rss is about other agents and is not here");
}

/// `heapLimit` is the ceiling this isolate was actually built with, so
/// `heapLimit - heapUsed` is real headroom rather than a guess.
///
/// This is the gap that motivated the whole addition: the runtime *enforces* a
/// heap ceiling, and until now a program learned it was near one by being killed.
#[test]
fn the_heap_limit_is_the_one_the_flag_set() {
    let source = r#"
        import { memoryUsage } from "runtime:process";
        console.log(Math.round(memoryUsage().heapLimit / 1048576));
    "#;
    let capped = lines("capped", source, &["--max-heap=256"]);
    assert_eq!(capped[0], "256");

    let tighter = lines("tighter", source, &["--max-heap=128"]);
    assert_eq!(tighter[0], "128");
}

/// `heapUsed` tracks the isolate, so growth is visible.
#[test]
fn heap_used_follows_allocation() {
    let out = lines(
        "growth",
        r#"
        import { memoryUsage } from "runtime:process";
        const before = memoryUsage().heapUsed;
        const held = new Array(3_000_000).fill(7);
        const after = memoryUsage().heapUsed;
        console.log("grew:", after > before);
        // Held past the measurement, so the allocation cannot have been
        // collected before it was read.
        console.log("kept:", held.length === 3_000_000);
        "#,
        &[],
    );
    assert_eq!(out[0], "grew: true");
    assert_eq!(out[1], "kept: true");
}

/// A worker is its own isolate with its own ceiling, and its own age.
///
/// `performance.now()` cannot answer either: a worker is handed its parent's
/// clock, so it counts from when the *process's* runtime was built and reads the
/// same in every agent. That is exactly why `uptime()` exists.
#[test]
fn each_agent_answers_for_itself() {
    std::fs::write(
        temp("pm-worker-child.mjs"),
        r#"
        import { memoryUsage, uptime } from "runtime:process";
        self.postMessage({
            limit: memoryUsage().heapLimit,
            uptime: uptime(),
            perf: performance.now(),
        });
        "#,
    )
    .expect("write worker");

    let out = lines(
        "worker",
        r#"
        import { memoryUsage, uptime } from "runtime:process";
        // Let the parent age before the worker exists, so the two cannot be
        // confused for one another.
        await new Promise((r) => setTimeout(r, 250));
        const w = new Worker(new URL("./pm-worker-child.mjs", import.meta.url), { memory: 64 });
        const d = await new Promise((r) => w.addEventListener("message", (e) => r(e.data)));
        console.log("parent MB:", Math.round(memoryUsage().heapLimit / 1048576));
        console.log("worker MB:", Math.round(d.limit / 1048576));
        console.log("parent older:", uptime() > 200);
        console.log("worker younger:", d.uptime < 100);
        // …and performance.now() would have said otherwise, which is the point.
        console.log("perf says process:", d.perf > 200);
        w.terminate();
        "#,
        &["--allow-all", "--max-heap=512"],
    );
    assert_eq!(out[0], "parent MB: 512");
    assert_eq!(
        out[1], "worker MB: 64",
        "a worker reported its parent's ceiling"
    );
    assert_eq!(out[2], "parent older: true");
    assert_eq!(
        out[3], "worker younger: true",
        "a worker reported the process's age"
    );
    assert_eq!(out[4], "perf says process: true");
}

/// `cpuTime()` measures **this agent's thread**, which is what makes "is it *me*
/// burning the CPU?" answerable — a process-wide number cannot say.
#[test]
fn cpu_time_measures_this_agent() {
    let out = lines(
        "cpu",
        r#"
        import { cpuTime } from "runtime:process";
        const before = cpuTime();
        const until = Date.now() + 80;
        while (Date.now() < until) { /* burn */ }
        const after = cpuTime();
        console.log("advanced:", after > before);
        // A burn is almost all CPU, so the two should be close. Generous either
        // way: this runs beside a whole test suite.
        console.log("plausible:", after - before > 20 && after - before < 400);
        "#,
        &[],
    );
    assert_eq!(out[0], "advanced: true");
    assert_eq!(out[1], "plausible: true");
}

/// A worker burning CPU is charged to the worker, not to the agent that started
/// it — the property the per-thread reading exists for.
#[test]
fn a_busy_worker_is_charged_to_itself() {
    std::fs::write(
        temp("pm-cpu-child.mjs"),
        r#"
        import { cpuTime } from "runtime:process";
        const until = Date.now() + 150;
        while (Date.now() < until) { /* burn */ }
        self.postMessage({ cpu: cpuTime() });
        "#,
    )
    .expect("write worker");

    let out = lines(
        "cpu-worker",
        r#"
        import { cpuTime } from "runtime:process";
        const w = new Worker(new URL("./pm-cpu-child.mjs", import.meta.url));
        const d = await new Promise((r) => w.addEventListener("message", (e) => r(e.data)));
        console.log("worker burned:", d.cpu > 100);
        // The parent was idle while the worker burned, so its own CPU must not
        // have followed.
        console.log("parent idle:", cpuTime() < 100);
        w.terminate();
        "#,
        &["--allow-all"],
    );
    assert_eq!(out[0], "worker burned: true");
    assert_eq!(
        out[1], "parent idle: true",
        "a worker's CPU landed on its parent"
    );
}

/// The process-wide figures are **not** on `runtime:process`: they describe the
/// agents around you, so they sit behind `diagnostics`.
#[test]
fn process_wide_figures_need_the_diagnostics_capability() {
    let out = lines(
        "process-gated",
        r#"
        import * as process from "runtime:process";
        import { metrics } from "runtime:diagnostics";
        console.log("not on process:", ["rss", "processCpu", "cpuUsage"].every((k) => !(k in process)));
        try { metrics(); console.log("metrics: allowed"); }
        catch (e) { console.log(`metrics: ${e.name}`); }
        "#,
        &[],
    );
    assert_eq!(out[0], "not on process: true");
    assert_eq!(out[1], "metrics: NotAllowedError");
}
