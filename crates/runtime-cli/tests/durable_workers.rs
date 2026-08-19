//! End-to-end tests for `runtime:workers` (DECISIONS.md D80).
//!
//! These drive the real `esrun` binary, because the claims are about what
//! survives a process: state written by one run is read back by the next, a
//! killed process leaves nothing acknowledged behind, and a directory that one
//! process has open is refused to another. None of that is observable inside a
//! single isolate, which is why there is no unit-test version of this file.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

fn dir(name: &str) -> PathBuf {
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("durable-{name}"));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("create temp dir");
    path
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Writes `source` as a module in `base` and runs it there. The working
/// directory is the jail (D79), so each test's state lives inside its own.
fn run_in(base: &PathBuf, name: &str, source: &str, flags: &[&str]) -> Output {
    let app = base.join(name);
    std::fs::write(&app, source).expect("write module");
    let grants: &[&str] = if flags.is_empty() {
        &["--allow-read", "--allow-write", "--allow-imports"]
    } else {
        flags
    };
    Command::new(env!("CARGO_BIN_EXE_esrun"))
        .current_dir(base)
        .args(grants)
        .arg(name)
        .output()
        .expect("run esrun")
}

/// One program, its own directory.
fn run(test: &str, source: &str) -> Output {
    run_in(&dir(test), "app.mjs", source, &[])
}

fn ok(out: &Output) -> String {
    assert!(
        out.status.success(),
        "esrun failed: {}{}",
        stdout(out),
        stderr(out)
    );
    stdout(out)
}

// ---- the state outlives the process ----------------------------------------

/// The whole promise in one test: what a worker stored is there for the next
/// process, as the value it was — not as the JSON shadow of one.
#[test]
fn state_survives_a_restart_with_every_value_kind() {
    let base = dir("restart");
    let define = r#"
        import { DurableWorker } from "runtime:workers";
        export class Vault extends DurableWorker {
          async put() {
            this.state.set("when", new Date(86_400_000));
            this.state.set("who", new Map([["a", 1n]]));
            this.state.set("bytes", new Uint8Array([1, 2, 3]));
            this.state.set("set", new Set(["x"]));
            await this.state.set("plain", { n: 42, deep: [null, true] });
            return this.state.size;
          }
          async read() {
            const when = this.state.get("when");
            const who = this.state.get("who");
            return [
              when instanceof Date && when.getTime() === 86_400_000,
              who instanceof Map && who.get("a") === 1n,
              this.state.get("bytes") instanceof Uint8Array,
              this.state.get("set") instanceof Set,
              this.state.get("plain").deep[1] === true,
              this.state.keys().join(","),
            ].join(" ");
          }
        }
    "#;
    std::fs::write(base.join("vault.mjs"), define).expect("write class");

    let first = run_in(
        &base,
        "write.mjs",
        r#"import { Vault } from "./vault.mjs";
           import { shutdown } from "runtime:workers";
           console.log("stored", await Vault.get("v1").put());
           await shutdown();"#,
        &[],
    );
    assert_eq!(ok(&first).trim(), "stored 5");

    let second = run_in(
        &base,
        "read.mjs",
        r#"import { Vault } from "./vault.mjs";
           console.log(await Vault.get("v1").read());"#,
        &[],
    );
    assert_eq!(
        ok(&second).trim(),
        "true true true true true bytes,plain,set,when,who"
    );
}

/// The gate: a call's result is not handed back until what it wrote is
/// committed. So every value the first process *acknowledged* is there after it
/// is killed outright — no shutdown, no flush, no chance to tidy up.
#[test]
fn acknowledged_writes_survive_a_kill() {
    let base = dir("kill");
    std::fs::write(
        base.join("ledger.mjs"),
        r#"
        import { DurableWorker } from "runtime:workers";
        export class Ledger extends DurableWorker {
          async append(entry) {
            const all = this.state.get("entries") ?? [];
            all.push(entry);
            this.state.set("entries", all);
            return all.length;
          }
          async read() { return (this.state.get("entries") ?? []).join(","); }
        }
    "#,
    )
    .expect("write class");

    let app = base.join("write.mjs");
    std::fs::write(
        &app,
        r#"import { Ledger } from "./ledger.mjs";
           const l = Ledger.get("main");
           for (let i = 1; i <= 5; i++) await l.append(`e${i}`);
           console.log("ACKED");
           await new Promise(() => {});"#,
    )
    .expect("write module");

    let mut child = Command::new(env!("CARGO_BIN_EXE_esrun"))
        .current_dir(&base)
        .args(["--allow-read", "--allow-write", "--allow-imports"])
        .arg("write.mjs")
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn esrun");

    let out = child.stdout.take().expect("piped stdout");
    let mut lines = BufReader::new(out).lines();
    let marker = lines
        .next()
        .expect("a line before the kill")
        .expect("read child stdout");
    assert_eq!(marker, "ACKED");

    // Abrupt: SIGKILL on unix, TerminateProcess on Windows. Nothing in the
    // guest runs after this — no `stop()`, no flush, no close.
    child.kill().expect("kill esrun");
    child.wait().expect("reap esrun");

    let after = run_in(
        &base,
        "read.mjs",
        r#"import { Ledger } from "./ledger.mjs";
           console.log(await Ledger.get("main").read());"#,
        &[],
    );
    assert_eq!(ok(&after).trim(), "e1,e2,e3,e4,e5");
}

/// A directory belongs to one process. The second is refused by name rather
/// than by the engine's own words about a file it never opened — and the
/// refusal clears the moment the first process is gone, with nothing to clean
/// up by hand.
#[test]
fn a_second_process_is_refused_while_the_first_holds_the_directory() {
    let base = dir("locked");
    std::fs::write(
        base.join("w.mjs"),
        r#"import { DurableWorker } from "runtime:workers";
           export class W extends DurableWorker { async ping() { return "pong"; } }"#,
    )
    .expect("write class");

    let app = base.join("hold.mjs");
    std::fs::write(
        &app,
        r#"import { W } from "./w.mjs";
           await W.get("a").ping();
           console.log("HOLDING");
           await new Promise(() => {});"#,
    )
    .expect("write module");

    let mut child = Command::new(env!("CARGO_BIN_EXE_esrun"))
        .current_dir(&base)
        .args(["--allow-read", "--allow-write", "--allow-imports"])
        .arg("hold.mjs")
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn esrun");
    let out = child.stdout.take().expect("piped stdout");
    let mut lines = BufReader::new(out).lines();
    assert_eq!(
        lines.next().expect("a line").expect("read"),
        "HOLDING",
        "the first process should have the directory open"
    );

    let refused = run_in(
        &base,
        "second.mjs",
        r#"import { W } from "./w.mjs";
           try { await W.get("a").ping(); console.log("ALLOWED"); }
           catch (e) { console.log(e.code); }"#,
        &[],
    );
    assert_eq!(ok(&refused).trim(), "ERR_DURABLE_LOCKED");

    child.kill().expect("kill esrun");
    child.wait().expect("reap esrun");

    let allowed = run_in(
        &base,
        "third.mjs",
        r#"import { W } from "./w.mjs";
           console.log(await W.get("a").ping());"#,
        &[],
    );
    assert_eq!(ok(&allowed).trim(), "pong");
}

// ---- one call at a time ----------------------------------------------------

/// Calls queue. Three that overlap in wall-clock time still run one after the
/// other, in the order they were made — which is the reason to hold state in a
/// worker rather than in a row somebody has to lock.
#[test]
fn calls_to_one_worker_run_one_at_a_time_in_order() {
    let out = run(
        "ordered",
        r#"
        import { DurableWorker } from "runtime:workers";
        class Slow extends DurableWorker {
          async touch(ms, tag) {
            const seen = this.state.get("seen") ?? [];
            await new Promise((r) => setTimeout(r, ms));
            seen.push(tag);
            this.state.set("seen", seen);
            return seen.join("");
          }
        }
        const s = Slow.get("one");
        // The slowest first: without a mailbox the others would finish before it.
        await Promise.all([s.touch(40, "a"), s.touch(1, "b"), s.touch(1, "c")]);
        console.log(await s.touch(0, "d"));
    "#,
    );
    assert_eq!(ok(&out).trim(), "abcd");
}

/// Two workers of the same class are two mailboxes: one being busy is not the
/// other being busy.
#[test]
fn different_ids_are_different_workers() {
    let out = run(
        "isolation",
        r#"
        import { DurableWorker } from "runtime:workers";
        class Counter extends DurableWorker {
          async add() {
            const n = (this.state.get("n") ?? 0) + 1;
            this.state.set("n", n);
            return `${this.id}=${n}`;
          }
        }
        console.log(await Counter.get("a").add(), await Counter.get("b").add(), await Counter.get("a").add());
    "#,
    );
    assert_eq!(ok(&out).trim(), "a=1 b=1 a=2");
}

/// A queue that grows without limit is a queue that hides the failure. Past the
/// mailbox limit a call is refused, by name, rather than waited on.
#[test]
fn a_full_mailbox_refuses_rather_than_grows() {
    let out = run(
        "mailbox",
        r#"
        import { DurableWorker, configure } from "runtime:workers";
        configure({ mailbox: 2 });
        class Slow extends DurableWorker {
          async wait() { await new Promise((r) => setTimeout(r, 20)); return "ok"; }
        }
        const s = Slow.get("one");
        const settled = await Promise.allSettled([s.wait(), s.wait(), s.wait()]);
        console.log(settled.map((r) => (r.status === "fulfilled" ? r.value : r.reason.code)).join(","));
    "#,
    );
    assert_eq!(ok(&out).trim(), "ok,ok,ERR_DURABLE_BUSY");
}

/// More workers than may be open at once, all being called at once: every one
/// is evicted and reopened repeatedly while work is in flight. What must hold
/// is that no count is lost and none is double-counted — a worker closing must
/// finish closing before the next call opens it again.
#[test]
fn workers_evicted_under_pressure_keep_an_exact_count() {
    let out = run(
        "pressure",
        r#"
        import { DurableWorker, configure, shutdown } from "runtime:workers";
        configure({ maxLive: 4, evictAfter: 1 });
        class Tally extends DurableWorker {
          async bump() {
            const n = (this.state.get("n") ?? 0) + 1;
            this.state.set("n", n);
            return n;
          }
        }
        const ids = Array.from({ length: 12 }, (_, i) => `w${i}`);
        for (let round = 0; round < 5; round++) {
          await Promise.all(ids.map((id) => Tally.get(id).bump()));
        }
        const counts = await Promise.all(ids.map((id) => Tally.get(id).bump()));
        console.log([...new Set(counts)].join(","));
        await shutdown();
    "#,
    );
    assert_eq!(ok(&out).trim(), "6");
}

// ---- the rules the API enforces --------------------------------------------

/// A durable worker is addressed, never constructed: the runtime is what knows
/// which state a given instance is holding.
#[test]
fn a_durable_worker_cannot_be_constructed() {
    let out = run(
        "construct",
        r#"
        import { DurableWorker } from "runtime:workers";
        class W extends DurableWorker {}
        try { new W(); console.log("constructed"); } catch (e) { console.log(e.name); }
    "#,
    );
    assert_eq!(ok(&out).trim(), "TypeError");
}

/// The state is resident in memory, so its ceiling is a real one. Both the
/// per-value and the whole-worker limit refuse at the write.
#[test]
fn state_over_the_limit_is_refused_at_the_write() {
    let out = run(
        "limits",
        r#"
        import { DurableWorker, configure } from "runtime:workers";
        configure({ valueLimit: 512, stateLimit: 2048 });
        class W extends DurableWorker {
          async big() { return this.state.set("v", "x".repeat(1024)); }
          async many() {
            for (let i = 0; i < 20; i++) this.state.set(`k${i}`, "y".repeat(200));
          }
        }
        const w = W.get("a");
        for (const call of ["big", "many"]) {
          try { await w[call](); console.log(`${call}: stored`); }
          catch (e) { console.log(`${call}: ${e.code}`); }
        }
    "#,
    );
    assert_eq!(
        ok(&out).trim(),
        "big: ERR_DURABLE_STATE_TOO_LARGE\nmany: ERR_DURABLE_STATE_TOO_LARGE"
    );
}

/// Arguments cross by structured clone even though nothing crosses a thread
/// yet, so what may be passed is the same rule it will be once a worker runs on
/// a shard — rather than one that tightens later.
#[test]
fn an_argument_that_cannot_be_cloned_is_refused() {
    let out = run(
        "clone",
        r#"
        import { DurableWorker } from "runtime:workers";
        class W extends DurableWorker { async take(x) { return typeof x; } }
        const w = W.get("a");
        console.log(await w.take(new Map([["k", new Date(0)]])) === "object");
        try { await w.take(() => {}); console.log("passed"); } catch (e) { console.log(e.name); }
    "#,
    );
    assert_eq!(ok(&out).trim(), "true\nDataCloneError");
}

/// A reference is not a thenable. `await` on one must not call a method named
/// `then`, which would be a hang wearing an await's clothes.
#[test]
fn a_reference_is_not_a_thenable() {
    let out = run(
        "thenable",
        r#"
        import { DurableWorker } from "runtime:workers";
        class W extends DurableWorker { async ping() { return "pong"; } }
        const ref = await Promise.resolve(W.get("a"));
        console.log(ref.id, await ref.ping());
    "#,
    );
    assert_eq!(ok(&out).trim(), "a pong");
}

/// Calling something the class does not have is a `TypeError` from the call,
/// not a promise that never settles.
#[test]
fn calling_a_method_that_does_not_exist_throws() {
    let out = run(
        "missing",
        r#"
        import { DurableWorker } from "runtime:workers";
        class W extends DurableWorker { async ping() { return "pong"; } }
        try { await W.get("a").nope(); console.log("called"); } catch (e) { console.log(e.name); }
        try { await W.get("a").start(); console.log("started"); } catch (e) { console.log(e.name); }
    "#,
    );
    assert_eq!(ok(&out).trim(), "TypeError\nTypeError");
}

/// Two classes storing under one name would share a state neither declared, so
/// the second one to be addressed is refused.
#[test]
fn two_classes_cannot_claim_one_storage_name() {
    let out = run(
        "names",
        r#"
        import { DurableWorker } from "runtime:workers";
        class W extends DurableWorker {}
        class X extends DurableWorker { static durableName = "W"; }
        W.get("a");
        try { X.get("a"); console.log("allowed"); } catch (e) { console.log(e.name); }
    "#,
    );
    assert_eq!(ok(&out).trim(), "TypeError");
}

// ---- the lifecycle ---------------------------------------------------------

/// `start()` runs once per materialization and `stop()` once per close, and a
/// worker evicted for being idle comes back with its state.
#[test]
fn an_idle_worker_is_closed_and_comes_back() {
    let out = run(
        "evict",
        r#"
        import { DurableWorker, configure, shutdown } from "runtime:workers";
        configure({ evictAfter: 10 });
        class W extends DurableWorker {
          async start() { console.log(`start ${this.id}`); }
          async stop(reason) { console.log(`stop ${this.id} ${reason}`); }
          async bump() {
            const n = (this.state.get("n") ?? 0) + 1;
            await this.state.set("n", n);
            return n;
          }
        }
        console.log("first", await W.get("a").bump());
        await new Promise((r) => setTimeout(r, 30));
        // Work on another worker is what sweeps the idle one: eviction happens
        // when something arrives, never on a timer the process would wait for.
        await W.get("b").bump();
        console.log("again", await W.get("a").bump());
        await shutdown();
    "#,
    );
    assert_eq!(
        ok(&out).trim(),
        "start a\nfirst 1\nstop a idle\nstart b\nstart a\nagain 2\nstop b shutdown\nstop a shutdown"
    );
}

/// Deleting takes the state with it — the next call to that id starts from
/// nothing — and the catalog knows what exists either way.
#[test]
fn deleting_a_worker_removes_its_state() {
    let out = run(
        "delete",
        r#"
        import { DurableWorker } from "runtime:workers";
        class W extends DurableWorker {
          async set() { return this.state.set("v", "here"); }
          async get() { return this.state.get("v") ?? "gone"; }
        }
        await W.get("a").set();
        await W.get("b").set();
        console.log("listed", (await W.list()).map((w) => w.id).sort().join(","));
        console.log("deleted", await W.delete("a"), "again", await W.delete("a"));
        console.log("read", await W.get("a").get(), await W.get("b").get());
    "#,
    );
    assert_eq!(
        ok(&out).trim(),
        "listed a,b\ndeleted true again false\nread gone here"
    );
}

// ---- capabilities ----------------------------------------------------------

/// The module adds no authority of its own: importing it is always allowed
/// (D26), and what it does with the filesystem is gated exactly as
/// `runtime:fs` and `runtime:db` are.
#[test]
fn without_a_filesystem_grant_the_first_call_is_denied() {
    let base = dir("denied");
    let out = run_in(
        &base,
        "app.mjs",
        r#"import { DurableWorker } from "runtime:workers";
           class W extends DurableWorker { async ping() { return "pong"; } }
           console.log("imported");
           try { await W.get("a").ping(); console.log("allowed"); }
           catch (e) { console.log(e.code ?? e.name); }"#,
        &["--deny-all"],
    );
    assert_eq!(ok(&out).trim(), "imported\nERR_CAPABILITY_DENIED");
}
