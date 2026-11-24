//! End-to-end tests for `Isolate` from `runtime:workers` (DECISIONS D227).
//!
//! These drive the real `esrun` binary: isolates on the shared pool, the
//! envelope between them and their parent, and the computation-only rule —
//! an isolate holds no capability, can be granted none, and reaches out only
//! through the `host` functions it is handed.

use std::path::PathBuf;
use std::process::{Command, Output};

mod common;

fn write(name: &str, contents: &str) -> PathBuf {
    let path = common::tmpdir().join(name);
    std::fs::write(&path, contents).expect("write temp file");
    path
}

/// Runs `main` (with `PLUGIN_URL` naming `plugin`, written beside it) under
/// `flags`, from the directory they are written to.
fn run(prefix: &str, plugin: &str, main: &str, flags: &[&str]) -> Output {
    write(&format!("{prefix}-plugin.mjs"), plugin);
    let app = write(
        &format!("{prefix}-main.mjs"),
        &main.replace("PLUGIN_URL", &format!("./{prefix}-plugin.mjs")),
    );
    Command::new(common::esrun_bin())
        .current_dir(common::tmpdir())
        .args(flags)
        .arg(&app)
        .output()
        .unwrap()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn lines(out: &Output) -> Vec<String> {
    stdout(out).lines().map(str::to_owned).collect()
}

const GRANT: &[&str] = &["--allow-workers"];
const GRANT_FILES: &[&str] = &["--allow-workers", "--allow-imports"];

#[test]
fn an_isolate_from_source_serves_its_exports() {
    let out = run(
        "source",
        "",
        r#"
        import { Isolate } from "runtime:workers";
        const rule = new Isolate({ source: "export const discount = (o) => o.total > 500 ? 0.1 : 0;" });
        const { discount } = await rule.exports;
        console.log(await discount({ total: 600 }), await discount({ total: 10 }));
        rule.terminate();
        "#,
        GRANT,
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(lines(&out), ["0.1 0"]);
}

#[test]
fn an_isolate_from_a_file_calls_its_host_and_imports_runtime_modules() {
    let out = run(
        "file",
        r#"
        import { host } from "runtime:workers";
        import { join } from "runtime:path";
        export async function transform(text) {
          await host.log("transforming", text.length);
          const shout = await host.config("shout");
          return (shout ? text.toUpperCase() + "!" : text) + " " + join("a", "b");
        }
        "#,
        r#"
        import { Isolate } from "runtime:workers";
        const plugin = new Isolate(new URL("PLUGIN_URL", import.meta.url), {
          host: { log: (...a) => console.log("[plugin]", ...a), config: async (k) => k === "shout" },
        });
        const { transform } = await plugin.exports;
        console.log(await transform("hello"));
        plugin.terminate();
        "#,
        GRANT_FILES,
    );
    assert!(out.status.success(), "{}", stderr(&out));
    // `runtime:path` inside an isolate follows the host's platform.
    let joined = if cfg!(windows) { r"a\b" } else { "a/b" };
    assert_eq!(
        lines(&out),
        [
            "[plugin] transforming 5".to_string(),
            format!("HELLO! {joined}")
        ]
    );
}

#[test]
fn messages_cross_both_ways() {
    let out = run(
        "messages",
        "",
        r#"
        import { Isolate } from "runtime:workers";
        const iso = new Isolate({ source: "onmessage = (e) => postMessage(e.data * 2);" });
        iso.onmessage = (e) => { console.log(e.data); iso.terminate(); };
        iso.postMessage(21);
        "#,
        GRANT,
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(lines(&out), ["42"]);
}

/// Computation only: every way out is refused with the remedy named, and the
/// class and code stay those of any denial.
#[test]
fn an_isolate_reaches_nothing_outside() {
    let out = run(
        "outside",
        "",
        r#"
        import { Isolate } from "runtime:workers";
        const cases = {
          fs: `import { file } from "runtime:fs"; export const go = () => file("x").text();`,
          fetch: `export const go = () => fetch("https://example.com");`,
          importFile: `export const go = () => import("./elsewhere.mjs");`,
          wait: `export const go = () => Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 1);`,
        };
        for (const [name, source] of Object.entries(cases)) {
          const iso = new Isolate({ source });
          try {
            await (await iso.exports).go();
            console.log(name, "allowed");
          } catch (e) {
            const said = /computation only/.test(e.message) ? "computation only" : e.message;
            console.log(name, e.name, e.code ?? "-", said);
          }
          iso.terminate();
        }
        "#,
        // Everything granted to the parent, and still nothing inside.
        &["--allow-all"],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        lines(&out),
        [
            "fs NotAllowedError ERR_CAPABILITY_DENIED computation only",
            "fetch NotAllowedError ERR_CAPABILITY_DENIED computation only",
            "importFile NotAllowedError ERR_CAPABILITY_DENIED computation only",
            "wait TypeError - Atomics.wait cannot be called in this context",
        ]
    );
}

/// An isolate is handed no environment and no arguments, and may not ask
/// where it runs: it learns nothing about the deployment.
#[test]
fn an_isolate_learns_nothing_about_its_deployment() {
    let out = run(
        "deployment",
        "",
        r#"
        import { Isolate } from "runtime:workers";
        const iso = new Isolate({ source: `
          import { env, cwd, args } from "runtime:process";
          export const look = () => JSON.stringify([Object.keys(env), args]);
          export const where = () => cwd();
        ` });
        const { look, where } = await iso.exports;
        console.log(await look());
        try { await where(); } catch (e) { console.log(e.code); }
        iso.terminate();
        "#,
        &["--allow-all"],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(lines(&out), ["[[],[]]", "ERR_CAPABILITY_DENIED"]);
}

#[test]
fn an_isolate_takes_no_permissions() {
    let out = run(
        "permissions",
        "",
        r#"
        import { Isolate } from "runtime:workers";
        try { new Isolate({ source: "" }, { permissions: ["net"] }); }
        catch (e) { console.log(e.name, /computation only/.test(e.message)); }
        "#,
        GRANT,
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(lines(&out), ["TypeError true"]);
}

/// Inside an isolate, `runtime:workers` is `host` alone: the durable workers
/// and `Isolate` need what an isolate cannot have.
#[test]
fn runtime_workers_inside_an_isolate_is_host_alone() {
    let out = run(
        "light",
        "",
        r#"
        import { Isolate } from "runtime:workers";
        const iso = new Isolate({ source: `import * as w from "runtime:workers"; export const keys = () => Object.keys(w).sort().join(",");` });
        console.log(await (await iso.exports).keys());
        iso.terminate();
        "#,
        GRANT,
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(lines(&out), ["default,host"]);
}

#[test]
fn errors_keep_their_class_and_code() {
    let out = run(
        "errors",
        "",
        r#"
        import { Isolate } from "runtime:workers";
        const iso = new Isolate({ source: `
          export function bad() { const e = new RangeError("bad input"); e.code = "ERR_X"; throw e; }
          export const fn = () => () => 1;
        ` });
        const { bad, fn } = await iso.exports;
        try { await bad(); } catch (e) { console.log(e instanceof RangeError, e.code, e.message); }
        try { await fn(); } catch (e) { console.log(e.name); }
        iso.terminate();
        "#,
        GRANT,
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(lines(&out), ["true ERR_X bad input", "DataCloneError"]);
}

#[test]
fn a_module_that_throws_rejects_its_exports_and_reports_an_error() {
    let out = run(
        "throws",
        "",
        r#"
        import { Isolate } from "runtime:workers";
        const iso = new Isolate({ source: `throw new TypeError("boom at load");` });
        iso.onerror = (e) => { e.preventDefault(); console.log("error event:", e.message); };
        try { await iso.exports; } catch (e) { console.log("exports:", e.name, e.message); }
        "#,
        GRANT,
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        lines(&out),
        [
            "error event: boom at load",
            "exports: TypeError boom at load"
        ]
    );
}

/// `terminate()` ends a call stuck in a loop, and the program goes on.
#[test]
fn terminate_ends_a_spinning_call() {
    let out = run(
        "spin",
        "",
        r#"
        import { Isolate } from "runtime:workers";
        const iso = new Isolate({ source: "export const spin = () => { for (;;) {} };" });
        const { spin } = await iso.exports;
        const call = spin();
        setTimeout(() => iso.terminate(), 50);
        try { await call; } catch (e) { console.log(e.code); }
        try { await spin(); } catch (e) { console.log(e.code); }
        "#,
        GRANT,
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        lines(&out),
        ["ERR_ISOLATE_TERMINATED", "ERR_ISOLATE_TERMINATED"]
    );
}

/// Far past a `Worker`'s 64: hundreds of isolates live at once on the pool.
#[test]
fn hundreds_of_isolates_run_at_once() {
    let out = run(
        "many",
        "",
        r#"
        import { Isolate } from "runtime:workers";
        const all = Array.from({ length: 300 }, (_, i) => new Isolate({ source: `export const id = () => ${i};` }));
        const ids = await Promise.all(all.map(async (iso) => (await iso.exports).id()));
        console.log(ids.length, ids.reduce((a, b) => a + b, 0));
        for (const iso of all) iso.terminate();
        "#,
        GRANT,
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(lines(&out), ["300 44850"]);
}

/// Creating one is the `workers` grant; from a file, reading it is `imports`
/// too, as for a `Worker`.
#[test]
fn starting_an_isolate_needs_workers_and_a_file_needs_imports() {
    let out = run(
        "grants",
        "export const x = 1;",
        r#"
        import { Isolate } from "runtime:workers";
        for (const code of [{ source: "export const x = 1;" }, new URL("PLUGIN_URL", import.meta.url)]) {
          const iso = new Isolate(code);
          iso.onerror = (e) => e.preventDefault();
          try { await iso.exports; console.log("started"); }
          catch (e) { console.log(e.code, /"(\w+)"/.exec(e.message)?.[1]); }
          iso.terminate();
        }
        "#,
        &["--allow-workers"],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(lines(&out), ["started", "ERR_CAPABILITY_DENIED imports"]);

    let out = run(
        "nogrant",
        "",
        r#"
        import { Isolate } from "runtime:workers";
        const iso = new Isolate({ source: "" });
        iso.onerror = (e) => e.preventDefault();
        try { await iso.exports; } catch (e) { console.log(e.code, /"(\w+)"/.exec(e.message)?.[1]); }
        "#,
        &[],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(lines(&out), ["ERR_CAPABILITY_DENIED workers"]);
}

// ---- cpuTime (D227 phase 3) -------------------------------------------------

/// A turn that spins past its budget is ended with `ERR_ISOLATE_CPU_LIMIT`,
/// soon after the budget — and an endless `await` chain is one turn too.
#[test]
fn cpu_time_ends_a_turn_that_spins() {
    let out = run(
        "cpu-spin",
        "",
        r#"
        import { Isolate } from "runtime:workers";
        for (const body of ["for (;;) {}", "for (;;) await null;"]) {
          const iso = new Isolate({ source: `export async function go() { ${body} }` }, { cpuTime: 50 });
          iso.onerror = (e) => e.preventDefault();
          const { go } = await iso.exports;
          const start = performance.now();
          try { await go(); } catch (e) {
            const ms = performance.now() - start;
            console.log(e.code, ms >= 40 && ms < 2000);
          }
          try { await go(); } catch (e) { console.log(e.code); }
        }
        "#,
        GRANT,
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        lines(&out),
        [
            "ERR_ISOLATE_CPU_LIMIT true",
            "ERR_ISOLATE_TERMINATED",
            "ERR_ISOLATE_CPU_LIMIT true",
            "ERR_ISOLATE_TERMINATED",
        ]
    );
}

/// The budget is a turn's, not a call's, and waiting is not in a turn: work in
/// short turns, and a long wait on a host function, both finish.
#[test]
fn cpu_time_counts_turns_and_not_waiting() {
    let out = run(
        "cpu-turns",
        "",
        r#"
        import { Isolate } from "runtime:workers";
        const iso = new Isolate({ source: `
          import { host } from "runtime:workers";
          const burn = (ms) => { const end = performance.now() + ms; while (performance.now() < end) {} };
          export async function work() {
            for (let k = 0; k < 8; k++) { burn(20); await new Promise((r) => setTimeout(r, 0)); }
            return "worked";
          }
          export const wait = () => host.slow();
        ` }, {
          cpuTime: 60,
          host: { slow: () => new Promise((r) => setTimeout(() => r("waited"), 300)) },
        });
        const { work, wait } = await iso.exports;
        console.log(await work());
        console.log(await wait());
        iso.terminate();
        "#,
        GRANT,
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(lines(&out), ["worked", "waited"]);
}

/// A module whose top level runs past the budget never serves its exports.
#[test]
fn cpu_time_covers_the_module_loading() {
    let out = run(
        "cpu-load",
        "",
        r#"
        import { Isolate } from "runtime:workers";
        const iso = new Isolate(
          { source: "const end = performance.now() + 2000; while (performance.now() < end) {} export const x = 1;" },
          { cpuTime: 50 },
        );
        iso.onerror = (e) => { e.preventDefault(); console.log("error:", e.error.code); };
        try { await iso.exports; } catch (e) { console.log("exports:", e.code); }
        "#,
        GRANT,
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        lines(&out),
        [
            "error: ERR_ISOLATE_CPU_LIMIT",
            "exports: ERR_ISOLATE_CPU_LIMIT"
        ]
    );
}

/// While one isolate spins, the isolates sharing the pool keep answering, and
/// the spinner is ended by its budget.
#[test]
fn a_spinning_isolate_does_not_hold_the_pool_past_its_budget() {
    let out = run(
        "cpu-neighbours",
        "",
        r#"
        import { Isolate } from "runtime:workers";
        const spinner = new Isolate({ source: "export const spin = () => { for (;;) {} };" }, { cpuTime: 100 });
        spinner.onerror = (e) => e.preventDefault();
        const others = Array.from({ length: 40 }, (_, k) => new Isolate({ source: `export const id = () => ${k};` }));
        const spun = (await spinner.exports).spin().catch((e) => e.code);
        const ids = await Promise.all(others.map(async (o) => (await o.exports).id()));
        console.log(ids.length, await spun);
        for (const o of others) o.terminate();
        "#,
        GRANT,
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(lines(&out), ["40 ERR_ISOLATE_CPU_LIMIT"]);
}

/// D237: with no `cpuTime`, a turn is still bounded — by the default, 1000 ms.
/// It used to run for ever.
#[test]
fn an_isolate_with_no_cpu_time_is_bounded_by_the_default() {
    let out = run(
        "cpu-default",
        "",
        r#"
        import { Isolate } from "runtime:workers";
        const spinner = new Isolate({ source: "export const spin = () => { for (;;) {} };" });
        spinner.onerror = (e) => e.preventDefault();
        const start = performance.now();
        const code = await (await spinner.exports).spin().catch((e) => e.code);
        const ms = performance.now() - start;
        console.log(code, ms >= 900, ms < 10000);
        "#,
        GRANT,
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(lines(&out), ["ERR_ISOLATE_CPU_LIMIT true true"]);
}

/// D237, the reported starvation: one unbudgeted spinner on every pool thread
/// used to leave a new isolate waiting for ever, and a spinner stalled the
/// budgeted isolates pinned beside it. Every turn is bounded now, so both
/// answer once the spinners' default budget runs out.
#[test]
fn unbudgeted_spinners_cannot_starve_the_pool() {
    let out = run(
        "cpu-starve",
        "",
        r#"
        import { Isolate } from "runtime:workers";
        const threads = navigator.hardwareConcurrency;
        const spinners = Array.from({ length: threads }, () =>
          new Isolate({ source: "export const spin = () => { for (;;) {} };" }));
        const spun = [];
        for (const s of spinners) {
          s.onerror = (e) => e.preventDefault();
          spun.push((await s.exports).spin().catch((e) => e.code));
        }
        const late = new Isolate({ source: "export const hi = () => 'hi';" });
        const beside = Array.from({ length: 24 }, (_, k) =>
          new Isolate({ source: `export const id = () => ${k};` }, { cpuTime: 100 }));
        const answered = await Promise.all(beside.map(async (b) => (await b.exports).id()));
        console.log(await (await late.exports).hi(), answered.length);
        console.log(new Set(await Promise.all(spun)).size, (await spun[0]));
        late.terminate();
        for (const b of beside) b.terminate();
        "#,
        GRANT,
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(lines(&out), ["hi 24", "1 ERR_ISOLATE_CPU_LIMIT"]);
}

/// D245: process control is not an isolate's. `exit()` used to end the
/// isolate silently, the code dropped and the parent told only that it "has
/// ended"; now it throws, inside the isolate, like every other refusal there.
#[test]
fn exit_in_an_isolate_is_refused_not_a_silent_end() {
    let out = run(
        "isolate-exit",
        "",
        r#"
        import { Isolate } from "runtime:workers";
        const plugin = new Isolate({ source: `
          import { exit } from "runtime:process";
          export function tryExit() {
            try { exit(3); return "exited"; }
            catch (e) { return e.name + " " + /computation only/.test(e.message); }
          }
          export const still = () => "alive";
        ` });
        const { tryExit, still } = await plugin.exports;
        console.log(await tryExit());
        console.log(await still());
        plugin.terminate();

        const top = new Isolate({ source: `import { exit } from "runtime:process"; exit(0);` });
        top.onerror = (e) => e.preventDefault();
        try { await top.exports; console.log("served"); }
        catch (e) { console.log(e.name, e.code === undefined); }
        "#,
        GRANT,
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(lines(&out), ["TypeError true", "alive", "TypeError true"]);
}

/// The code argument takes `source` and `name` and nothing else. A limit put
/// there instead of in the options used to be dropped, and the code ran
/// uncapped; now it is refused, naming where it belongs — and the spin below
/// never starts.
#[test]
fn a_limit_in_the_code_argument_is_refused_not_dropped() {
    let out = run(
        "code-keys",
        "",
        r#"
        import { Isolate } from "runtime:workers";
        const spin = "export function spin(){ for(;;){} }";
        for (const extra of [{ cpuTime: 50 }, { memory: 16 }, { host: {} }]) {
          try { new Isolate({ source: spin, ...extra }).terminate(); console.log("accepted"); }
          catch (e) {
            const key = Object.keys(extra)[0];
            console.log(e.name, e.message.includes(`{ ${key} }`));
          }
        }
        try { new Isolate({ source: spin, permissions: ["net"] }).terminate(); console.log("accepted"); }
        catch (e) { console.log(e.name, /computation only/.test(e.message)); }
        try { new Isolate({ source: spin, cputime: 50 }).terminate(); console.log("accepted"); }
        catch (e) { console.log(e.name, /unknown key "cputime"/.test(e.message)); }
        try { new Isolate({ source: "", name: "a" }, { name: "b" }).terminate(); console.log("accepted"); }
        catch (e) { console.log(e.name, /given twice/.test(e.message)); }
        const ok = new Isolate({ source: "export const hi = () => 'hi';", name: "fine" });
        console.log(await (await ok.exports).hi());
        ok.terminate();
        "#,
        GRANT,
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        lines(&out),
        [
            "TypeError true",
            "TypeError true",
            "TypeError true",
            "TypeError true",
            "TypeError true",
            "TypeError true",
            "hi",
        ]
    );
}

#[test]
fn cpu_time_must_be_a_positive_whole_number() {
    let out = run(
        "cpu-option",
        "",
        r#"
        import { Isolate } from "runtime:workers";
        for (const cpuTime of [0, -5, 1.5, "fast", Infinity]) {
          try { new Isolate({ source: "" }, { cpuTime }); console.log("accepted", cpuTime); }
          catch (e) { console.log(e.name); }
        }
        "#,
        GRANT,
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        lines(&out),
        [
            "TypeError",
            "TypeError",
            "TypeError",
            "TypeError",
            "TypeError"
        ]
    );
}

/// Reaching the heap ceiling is `ERR_ISOLATE_MEMORY_LIMIT`, under the same
/// prefix as the other limits an isolate can reach.
#[test]
fn an_isolate_out_of_memory_is_its_own_code() {
    let out = run(
        "memory",
        "",
        r#"
        import { Isolate } from "runtime:workers";
        const iso = new Isolate(
          { source: "const a = []; export const grow = () => { for (;;) a.push(new Array(1e5).fill({ x: 1 })); };" },
          { memory: 16 },
        );
        iso.onerror = (e) => e.preventDefault();
        try { await (await iso.exports).grow(); } catch (e) { console.log(e.code, /16MB/.test(e.message)); }
        "#,
        GRANT,
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(lines(&out), ["ERR_ISOLATE_MEMORY_LIMIT true"]);
}

/// `memory` holds an isolate's `ArrayBuffer`s too, which V8's heap ceiling
/// does not: live buffers past it end the isolate, one buffer larger than it
/// is refused, and buffers that are garbage do not count once collected.
#[test]
fn memory_counts_array_buffers() {
    let out = run(
        "buffers",
        "",
        r#"
        import { Isolate } from "runtime:workers";
        const cases = {
          hold: "const kept = []; export const go = () => { for (let i = 0; i < 100; i++) kept.push(new ArrayBuffer(1 << 20)); return kept.length; };",
          one: "export const go = () => new Uint8Array(32 << 20).length;",
          churn: "export const go = () => { let mb = 0; for (let i = 0; i < 60; i++) mb += new Uint8Array(8 << 20).length >> 20; return mb; };",
        };
        for (const [name, source] of Object.entries(cases)) {
          const iso = new Isolate({ source }, { memory: 16 });
          iso.onerror = (e) => e.preventDefault();
          try { console.log(name, await (await iso.exports).go()); }
          catch (e) { console.log(name, e.code ?? e.name); }
          iso.terminate();
        }
        const free = new Isolate({ source: "export const go = () => new Uint8Array(64 << 20).length >> 20;" });
        console.log("unlimited", await (await free.exports).go());
        free.terminate();
        "#,
        GRANT,
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        lines(&out),
        [
            "hold ERR_ISOLATE_MEMORY_LIMIT",
            "one ERR_ISOLATE_MEMORY_LIMIT",
            "churn 480",
            "unlimited 64"
        ]
    );
}

// ---- Isolate.prepare (D227 phase 4) ------------------------------------------

/// Prepared isolates are claimed by the `new Isolate()` calls that follow, and
/// a claimed one is every bit the isolate it was asked to be: its module, its
/// name, its location, its host.
#[test]
fn prepared_isolates_serve_the_isolates_made_next() {
    let out = run(
        "prepare",
        r#"
        import { host } from "runtime:workers";
        export const who = async () => `${self.name}|${location.href.split("/").pop()}|${await host.tag()}`;
        "#,
        r#"
        import { Isolate } from "runtime:workers";
        console.log("waiting", await Isolate.prepare(6));
        const all = Array.from({ length: 6 }, (_, k) => new Isolate({ source: `export const id = () => ${k};` }));
        console.log(...(await Promise.all(all.map(async (i) => (await i.exports).id()))));
        for (const i of all) i.terminate();
        await Isolate.prepare(1);
        const named = new Isolate(new URL("PLUGIN_URL", import.meta.url), { name: "plug", host: { tag: () => "t" } });
        console.log(await (await named.exports).who());
        named.terminate();
        "#,
        GRANT_FILES,
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        lines(&out),
        ["waiting 6", "0 1 2 3 4 5", "plug|prepare-plugin.mjs|t"]
    );
}

/// An isolate given its own `memory` starts as usual, past the prepared ones,
/// and its ceiling holds.
#[test]
fn an_isolate_with_its_own_memory_is_not_a_prepared_one() {
    let out = run(
        "prepare-memory",
        "",
        r#"
        import { Isolate } from "runtime:workers";
        await Isolate.prepare(2);
        const own = new Isolate({ source: "const kept = []; export const fill = () => { for (;;) kept.push(new ArrayBuffer(1 << 20)); };" }, { memory: 8 });
        own.onerror = (e) => e.preventDefault();
        try { await (await own.exports).fill(); } catch (e) { console.log(e.code); }
        "#,
        GRANT,
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(lines(&out), ["ERR_ISOLATE_MEMORY_LIMIT"]);
}

/// Prepared isolates hold no work: a program that leaves some unclaimed
/// still exits.
#[test]
fn unclaimed_prepared_isolates_do_not_keep_the_process_alive() {
    let out = run(
        "prepare-exit",
        "",
        r#"
        import { Isolate } from "runtime:workers";
        console.log(await Isolate.prepare(3));
        "#,
        GRANT,
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(lines(&out), ["3"]);
}

#[test]
fn prepare_takes_a_positive_count_and_the_workers_grant() {
    let out = run(
        "prepare-args",
        "",
        r#"
        import { Isolate } from "runtime:workers";
        for (const n of [0, -1, 2.5, "two"]) {
          try { await Isolate.prepare(n); console.log("accepted", n); } catch (e) { console.log(e.name); }
        }
        "#,
        GRANT,
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        lines(&out),
        ["TypeError", "TypeError", "TypeError", "TypeError"]
    );

    let out = run(
        "prepare-grant",
        "",
        r#"
        import { Isolate } from "runtime:workers";
        try { await Isolate.prepare(1); } catch (e) { console.log(e.code); }
        "#,
        &[],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(lines(&out), ["ERR_CAPABILITY_DENIED"]);
}

/// Less `memory` than an isolate needs to start is refused when it is asked
/// for, not reported later as a failure to start.
#[test]
fn memory_below_what_an_isolate_starts_in_is_refused() {
    let out = run(
        "memory-floor",
        "",
        r#"
        import { Isolate } from "runtime:workers";
        try { new Isolate({ source: "" }, { memory: 4 }); } catch (e) { console.log(e.name, /at least 8 MB/.test(e.message)); }
        const ok = new Isolate({ source: "export const x = () => 1;" }, { memory: 8 });
        console.log(await (await ok.exports).x());
        ok.terminate();
        "#,
        GRANT,
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(lines(&out), ["RangeError true", "1"]);
}
