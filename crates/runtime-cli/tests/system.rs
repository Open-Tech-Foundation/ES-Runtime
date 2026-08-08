//! End-to-end tests for `runtime:system` — child processes (DECISIONS D37).
//!
//! These spawn the real `esrun` binary, which then spawns real programs, so the
//! whole path is exercised: the `Run`-gated ops, the `SystemCommands` provider's
//! program resolution and pipes, and the web-stream plumbing in the module.
//! Unix-only: the assertions are written against `sh`, `cat`, and `sleep`.

#![cfg(unix)]

use std::path::PathBuf;
use std::process::{Command, Output};

fn temp(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name)
}

fn write(name: &str, contents: &str) -> PathBuf {
    let path = temp(name);
    std::fs::write(&path, contents).expect("write temp file");
    path
}

fn esrun() -> Command {
    Command::new(env!("CARGO_BIN_EXE_esrun"))
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}
fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Runs `source` as a script and returns its stdout, failing the test with the
/// child's stderr if it did not exit cleanly.
fn run(name: &str, source: &str) -> String {
    let app = write(name, source);
    let out = esrun().arg(&app).output().unwrap();
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    stdout(&out)
}

#[test]
fn output_runs_a_program_and_collects_it() {
    let s = run(
        "sys_output.mjs",
        r#"
import { Command } from "runtime:system";
const out = await new Command("echo", { args: ["hello", "world"] }).output();
console.log(out.success, out.code, out.signal);
console.log(new TextDecoder().decode(out.stdout).trim());
"#,
    );
    assert!(s.contains("true 0 null"), "{s}");
    assert!(s.contains("hello world"), "{s}");
}

#[test]
fn a_failing_program_reports_its_code_and_stderr() {
    let s = run(
        "sys_failure.mjs",
        r#"
import { Command } from "runtime:system";
const out = await new Command("sh", { args: ["-c", "echo boom >&2; exit 3"] }).output();
console.log(out.success, out.code, new TextDecoder().decode(out.stderr).trim());
"#,
    );
    assert!(s.contains("false 3 boom"), "{s}");
}

#[test]
fn stdin_and_stdout_are_web_streams() {
    let s = run(
        "sys_streams.mjs",
        r#"
import { Command } from "runtime:system";
const child = await new Command("cat", { stdin: "piped", stdout: "piped" }).spawn();
const writer = child.stdin.getWriter();
await writer.write(new TextEncoder().encode("through the pipe"));
await writer.close();
let text = "";
for await (const chunk of child.stdout.pipeThrough(new TextDecoderStream())) text += chunk;
console.log(text, (await child.status).code);
"#,
    );
    assert!(s.contains("through the pipe 0"), "{s}");
}

#[test]
fn stdin_accepts_a_body_and_stdout_feeds_a_response() {
    // The two ergonomics a server actually wants: pipe a body in, hand the
    // output straight to a Response.
    let s = run(
        "sys_bodies.mjs",
        r#"
import { Command } from "runtime:system";
const out = await new Command("cat", { stdin: "a plain string body" }).output();
console.log(new TextDecoder().decode(out.stdout));
const child = await new Command("echo", { args: ["as a response"] }).spawn();
console.log((await new Response(child.stdout).text()).trim());
"#,
    );
    assert!(s.contains("a plain string body"), "{s}");
    assert!(s.contains("as a response"), "{s}");
}

#[test]
fn the_child_environment_is_empty_unless_it_is_asked_for() {
    let app = write(
        "sys_env.mjs",
        r#"
import { Command } from "runtime:system";
const bare = await new Command("env").output();
console.log("bare:" + JSON.stringify(new TextDecoder().decode(bare.stdout)));
const given = await new Command("sh", { args: ["-c", "echo $ONLY"], env: { ONLY: "mine" } }).output();
console.log("given:" + new TextDecoder().decode(given.stdout).trim());
const inherited = await new Command("sh", { args: ["-c", "echo $HOST_VAR"], inheritEnv: true }).output();
console.log("inherited:" + new TextDecoder().decode(inherited.stdout).trim());
"#,
    );
    let out = esrun()
        .env("HOST_VAR", "from-the-host")
        .arg(&app)
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    assert!(
        s.contains(r#"bare:"""#),
        "the host env leaked into the child: {s}"
    );
    assert!(s.contains("given:mine"), "{s}");
    assert!(s.contains("inherited:from-the-host"), "{s}");
}

#[test]
fn a_secret_env_value_is_unmasked_for_the_child() {
    let app = write(
        "sys_secret.mjs",
        r#"
import { Command } from "runtime:system";
import { env } from "runtime:process";
console.log("logged:" + String(env.API_TOKEN));
const out = await new Command("sh", {
  args: ["-c", "echo $API_TOKEN"],
  env: { API_TOKEN: env.API_TOKEN },
}).output();
console.log("child:" + new TextDecoder().decode(out.stdout).trim());
"#,
    );
    let out = esrun()
        .env("API_TOKEN", "s3cret")
        .arg(&app)
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    assert!(s.contains("logged:[redacted]"), "{s}");
    assert!(s.contains("child:s3cret"), "{s}");
}

#[test]
fn arguments_are_never_interpreted_by_a_shell() {
    let marker = temp("sys_pwned");
    let _ = std::fs::remove_file(&marker);
    let s = run(
        "sys_injection.mjs",
        &format!(
            r#"
import {{ Command }} from "runtime:system";
const out = await new Command("echo", {{ args: ["hi; touch {}", "$(whoami)"] }}).output();
console.log(new TextDecoder().decode(out.stdout).trim());
"#,
            marker.to_string_lossy(),
        ),
    );
    assert!(s.contains("hi; touch "), "{s}");
    assert!(
        s.contains("$(whoami)"),
        "the substitution was expanded: {s}"
    );
    assert!(!marker.exists(), "the argument was executed as a command");
}

#[test]
fn a_missing_program_fails_before_anything_runs() {
    let s = run(
        "sys_missing.mjs",
        r#"
import { Command } from "runtime:system";
try {
  await new Command("definitely-not-a-real-program").output();
  console.log("no throw");
} catch (e) {
  console.log(e.code);
}
"#,
    );
    assert!(s.contains("ERR_NOT_FOUND"), "{s}");
}

#[test]
fn kill_terminates_the_child_and_the_status_names_the_signal() {
    let s = run(
        "sys_kill.mjs",
        r#"
import { Command } from "runtime:system";
const child = await new Command("sleep", { args: ["30"] }).spawn();
await child.kill("SIGTERM");
const status = await child.status;
console.log(status.success, status.code, status.signal);
"#,
    );
    assert!(s.contains("false null SIGTERM"), "{s}");
}

#[test]
fn a_timeout_kills_the_child_and_rejects() {
    let s = run(
        "sys_timeout.mjs",
        r#"
import { Command } from "runtime:system";
const started = Date.now();
try {
  await new Command("sleep", { args: ["30"], timeout: 150 }).output();
  console.log("no throw");
} catch (e) {
  console.log(e.name, Date.now() - started < 10_000);
}
"#,
    );
    assert!(s.contains("TimeoutError true"), "{s}");
}

#[test]
fn an_abort_signal_kills_the_child() {
    let s = run(
        "sys_abort.mjs",
        r#"
import { Command } from "runtime:system";
const controller = new AbortController();
const running = new Command("sleep", { args: ["30"], signal: controller.signal }).output();
controller.abort(new Error("changed my mind"));
try {
  await running;
  console.log("no throw");
} catch (e) {
  console.log(e.message);
}
"#,
    );
    assert!(s.contains("changed my mind"), "{s}");
}

#[test]
fn output_past_max_buffer_is_bounded_rather_than_unbounded() {
    let s = run(
        "sys_maxbuffer.mjs",
        r#"
import { Command } from "runtime:system";
try {
  await new Command("sh", { args: ["-c", "yes | head -c 200000"], maxBuffer: 1024 }).output();
  console.log("no throw");
} catch (e) {
  console.log(e.code);
}
"#,
    );
    assert!(s.contains("ERR_MAX_BUFFER"), "{s}");
}

#[test]
fn signalling_a_reaped_child_is_the_no_op_it_promises() {
    // `kill()` documents that signalling an exited child is a no-op, because
    // the race is unavoidable for the caller. Draining both pipes *and*
    // awaiting the status releases the host handle, and after that the signal
    // was answered with ERR_FOREIGN_HANDLE — the right answer to naming another
    // agent's child, the wrong one to naming your own after it finished.
    let printed = run(
        "reaped-kill.mjs",
        "import { Command } from 'runtime:system'; \
         const p = await new Command('echo', { args: ['hi'], stdout: 'piped', stderr: 'piped' }) \
           .spawn(); \
         for (const s of [p.stdout, p.stderr]) { \
           const r = s.getReader(); \
           for (;;) { const { done } = await r.read(); if (done) break; } \
         } \
         await p.status; \
         try { await p.kill(); console.log('no-op'); } \
         catch (e) { console.log('threw', e.code); } \
         try { await p.stdin?.getWriter().close(); } catch (e) { console.log('stdin', e.code); } \
         console.log('done');",
    );
    assert_eq!(printed, "no-op\ndone\n");
}

#[test]
fn a_child_nobody_waits_on_does_not_hold_the_program_open() {
    // The liveness rule: awaiting `status` is what keeps the runtime ticking.
    // Spawn and ignore, and the program still exits — and the child does not
    // outlive it.
    let app = write(
        "sys_detached.mjs",
        r#"
import { Command } from "runtime:system";
await new Command("sleep", { args: ["45"] }).spawn();
console.log("exiting");
"#,
    );
    let started = std::time::Instant::now();
    let out = esrun().arg(&app).output().unwrap();
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("exiting"));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(30),
        "the runtime waited for a child nobody awaited"
    );
}

#[test]
fn a_child_can_run_esrun_itself() {
    // The composition test: a server runtime spawning an isolated job is the
    // point of the module.
    let worker = write("sys_worker.mjs", r#"console.log("from the worker");"#);
    let app = write(
        "sys_parent.mjs",
        &format!(
            r#"
import * as system from "runtime:system";
const out = await new system.Command({:?}, {{ args: [{:?}] }}).output();
console.log(new TextDecoder().decode(out.stdout).trim());
"#,
            env!("CARGO_BIN_EXE_esrun"),
            worker.to_string_lossy(),
        ),
    );
    let out = esrun().arg(&app).output().unwrap();
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("from the worker"), "{}", stdout(&out));
}
