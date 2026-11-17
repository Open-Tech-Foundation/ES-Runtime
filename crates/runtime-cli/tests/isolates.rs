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
    assert_eq!(lines(&out), ["[plugin] transforming 5", "HELLO! a/b"]);
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
