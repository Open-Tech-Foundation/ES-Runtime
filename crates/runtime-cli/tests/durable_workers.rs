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
/// The next call starts while the previous call's writes commit (D128), so a
/// burst on one worker coalesces its commits — and still every call is
/// answered, in order, only once what it wrote is durable. Two hundred calls
/// at once, then a fresh process reads every one of them back.
#[test]
fn a_burst_on_one_worker_is_answered_in_order_and_all_of_it_survives() {
    let base = dir("burst");
    std::fs::write(
        base.join("tally.mjs"),
        r#"
        import { DurableWorker } from "runtime:workers";
        export class Tally extends DurableWorker {
          async add(n) {
            const seen = this.state.get("seen") ?? [];
            seen.push(n);
            this.state.set("seen", seen);
            return seen.length;
          }
          async read() { return (this.state.get("seen") ?? []).join(","); }
        }
    "#,
    )
    .expect("write class");
    let first = run_in(
        &base,
        "write.mjs",
        r#"import { Tally } from "./tally.mjs";
           import { shutdown } from "runtime:workers";
           const t = Tally.get("t");
           const answers = await Promise.all(Array.from({ length: 200 }, (_, i) => t.add(i)));
           console.log(answers.every((n, i) => n === i + 1));
           await shutdown();"#,
        &[],
    );
    assert_eq!(ok(&first).trim(), "true");
    let second = run_in(
        &base,
        "read.mjs",
        r#"import { Tally } from "./tally.mjs";
           const all = (await Tally.get("t").read()).split(",").map(Number);
           console.log(all.length, all.every((n, i) => n === i));"#,
        &[],
    );
    assert_eq!(ok(&second).trim(), "200 true");
}

/// A call that writes, waits, and writes again has two flushes — the second
/// queued behind the first. Its result must wait for both. The gate once saw
/// neither: the first flush cleared the in-flight marker as it ended, so while
/// the queued one committed there was "nothing in flight", and the answer went
/// out ahead of the write. The process exits the moment it has its answers.
#[test]
fn a_result_waits_for_a_flush_queued_behind_another() {
    let base = dir("queued-flush");
    std::fs::write(
        base.join("pair.mjs"),
        r#"
        import { DurableWorker } from "runtime:workers";
        export class Pair extends DurableWorker {
          async go(i) {
            this.state.set(`a${i}`, i);
            await new Promise((r) => setTimeout(r, 0));
            this.state.set(`b${i}`, i);
            return i;
          }
          async count() { return this.state.size; }
        }
    "#,
    )
    .expect("write class");
    let first = run_in(
        &base,
        "write.mjs",
        r#"import { Pair } from "./pair.mjs";
           import { exit } from "runtime:process";
           for (let i = 0; i < 50; i++) await Pair.get("p").go(i);
           console.log("acked");
           exit(0);"#,
        &[],
    );
    assert_eq!(ok(&first).trim(), "acked");
    let second = run_in(
        &base,
        "read.mjs",
        r#"import { Pair } from "./pair.mjs";
           console.log(await Pair.get("p").count());"#,
        &[],
    );
    assert_eq!(ok(&second).trim(), "100");
}

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

// ---- collections -----------------------------------------------------------

/// What a collection is for: more than the resident ceiling will hold, queried
/// rather than kept — and still the same values going in and out, because a
/// document is stored the way a key is.
#[test]
fn documents_round_trip_and_are_queried_by_their_declared_fields() {
    let out = run(
        "collections",
        r#"
        import { DurableWorker, shutdown } from "runtime:workers";
        class Room extends DurableWorker {
          static schema = { collections: { messages: { index: ["ts", "author"] } } };
          async post(m) { return this.state.collection("messages").insert(m); }
          async postMany(ms) { return this.state.collection("messages").insertMany(ms); }
          async one(id) { return this.state.collection("messages").get(id); }
          async recent(n) {
            return this.state.collection("messages").find().sort({ ts: "desc" }).limit(n).toArray();
          }
          async by(author) { return this.state.collection("messages").find({ author }).count(); }
          async since(ts) { return this.state.collection("messages").find({ ts: { gte: ts } }).count(); }
          async authors(list) {
            return this.state.collection("messages").find({ author: { in: list } }).count();
          }
          async page(n, skip) {
            return this.state.collection("messages").find().sort({ ts: "asc" }).limit(n).offset(skip).toArray();
          }
          async edit(id) {
            return this.state.collection("messages").update(id, (d) => ({ ...d, body: `${d.body}!` }));
          }
          async drop(id) { return this.state.collection("messages").delete(id); }
          async purge(before) { return this.state.collection("messages").deleteWhere({ ts: { lt: before } }); }
          async total() { return this.state.collection("messages").count(); }
        }
        const r = Room.get("general");
        const id = await r.post({ ts: 100, author: "a", body: "one", tags: new Set(["x"]), at: new Date(7) });
        await r.postMany([
          { id: "given", ts: 200, author: "b", body: "two" },
          { ts: 300, author: "a", body: "three" },
        ]);
        const first = await r.one(id);
        console.log("id", id.length === 36, "types", first.tags instanceof Set && first.at instanceof Date);
        console.log("recent", (await r.recent(2)).map((m) => m.ts).join(","));
        console.log("by a", await r.by("a"), "since 200", await r.since(200), "in", await r.authors(["a", "z"]));
        console.log("page", (await r.page(1, 1)).map((m) => m.ts).join(","));
        console.log("edited", (await r.edit("given")).body, "drop", await r.drop("given"));
        console.log("purge", await r.purge(300), "left", await r.total());
        await shutdown();
    "#,
    );
    assert_eq!(
        ok(&out).trim(),
        "id true types true\nrecent 300,200\nby a 2 since 200 2 in 2\npage 200\nedited two! drop true\npurge 1 left 1"
    );
}

/// A collection is what the class declared, and a field is queryable because it
/// was declared. Neither is guessed: a name the class does not know would be a
/// second store nobody meant to have, and a field the class did not declare is
/// inside a blob the database cannot see into.
#[test]
fn only_what_the_schema_declares_can_be_queried() {
    let out = run(
        "collections-declared",
        r#"
        import { DurableWorker, shutdown } from "runtime:workers";
        class W extends DurableWorker {
          static schema = { collections: { items: { index: ["sku"] } } };
          async seed() { await this.state.collection("items").insert({ sku: "a", colour: "red" }); }
          async unknownCollection() { return this.state.collection("nope").count(); }
          async undeclaredField() { return this.state.collection("items").find({ colour: "red" }).count(); }
          async scanned() {
            return this.state.collection("items").find({ colour: "red" }, { scan: true }).count();
          }
          async badSort() { return this.state.collection("items").find().sort({ colour: "asc" }).toArray(); }
        }
        const w = W.get("a");
        await w.seed();
        for (const call of ["unknownCollection", "undeclaredField", "badSort"]) {
          try { await w[call](); console.log(call, "allowed"); } catch (e) { console.log(call, e.name); }
        }
        console.log("scanned", await w.scanned());
        await shutdown();
    "#,
    );
    assert_eq!(
        ok(&out).trim(),
        "unknownCollection TypeError\nundeclaredField TypeError\nbadSort TypeError\nscanned 1"
    );
}

/// A `unique` field is a real unique index, and a collision is the database's
/// own vocabulary — `ERR_DB_UNIQUE_VIOLATION` — rather than something this
/// module invents a second word for.
#[test]
fn a_unique_field_refuses_a_second_document() {
    let out = run(
        "collections-unique",
        r#"
        import { DurableWorker, shutdown } from "runtime:workers";
        class Box extends DurableWorker {
          static schema = { collections: { items: { unique: ["sku"] } } };
          async add(d) {
            try { await this.state.collection("items").insert(d); return "stored"; }
            catch (e) { return e.code; }
          }
          async count() { return this.state.collection("items").count(); }
        }
        const b = Box.get("b");
        console.log(await b.add({ sku: "A" }), await b.add({ sku: "A" }), await b.count());
        await shutdown();
    "#,
    );
    assert_eq!(ok(&out).trim(), "stored ERR_DB_UNIQUE_VIOLATION 1");
}

/// Declaring a field later is a migration, and it happens on the first wake
/// after the deploy: the column is added and **filled in from the documents**,
/// so a query over it does not quietly miss everything written before.
#[test]
fn a_field_declared_later_finds_the_documents_written_before_it() {
    let base = dir("collections-migrate");
    let write = |fields: &str| {
        format!(
            r#"import {{ DurableWorker, shutdown }} from "runtime:workers";
               class Log extends DurableWorker {{
                 static schema = {{ collections: {{ lines: {{ index: {fields} }} }} }};
                 async add(d) {{ return this.state.collection("lines").insert(d); }}
                 async warns() {{ return this.state.collection("lines").find({{ level: "warn" }}).count(); }}
                 async count() {{ return this.state.collection("lines").count(); }}
               }}
               export {{ Log }};"#
        )
    };
    let first = run_in(
        &base,
        "first.mjs",
        &format!(
            "{}\nawait Log.get('l').add({{ ts: 1, level: 'warn' }});\n\
             await Log.get('l').add({{ ts: 2, level: 'info' }});\n\
             console.log('written', await Log.get('l').count());\nawait shutdown();",
            write("[\"ts\"]")
        ),
        &[],
    );
    assert_eq!(ok(&first).trim(), "written 2");

    let second = run_in(
        &base,
        "second.mjs",
        &format!(
            "{}\nconsole.log('backfilled', await Log.get('l').warns());\n\
             await Log.get('l').add({{ ts: 3, level: 'warn' }});\n\
             console.log('after', await Log.get('l').warns(), await Log.get('l').count());\n\
             await shutdown();",
            write("[\"ts\", \"level\"]")
        ),
        &[],
    );
    assert_eq!(ok(&second).trim(), "backfilled 1\nafter 2 3");
}

/// One transaction over both halves of a worker's storage. It commits together
/// and it rolls back together — including the keys, which are otherwise written
/// behind the call.
#[test]
fn a_transaction_covers_the_keys_and_the_collections_together() {
    let out = run(
        "collections-transaction",
        r#"
        import { DurableWorker, shutdown } from "runtime:workers";
        class W extends DurableWorker {
          static schema = { collections: { rows: { index: ["n"] } } };
          async both() {
            return this.state.transaction(async () => {
              await this.state.collection("rows").insert({ id: "kept", n: 1 });
              await this.state.set("keptKey", true);
              return "committed";
            });
          }
          async failing() {
            try {
              await this.state.transaction(async () => {
                await this.state.collection("rows").insert({ id: "gone", n: 2 });
                await this.state.set("goneKey", true);
                throw new Error("no");
              });
            } catch (e) { return e.message; }
          }
          async state_of() {
            return [
              await this.state.collection("rows").count(),
              this.state.get("keptKey") === true,
              this.state.get("goneKey") === undefined,
            ].join(" ");
          }
        }
        const w = W.get("a");
        console.log(await w.both(), await w.failing());
        console.log(await w.state_of());
        await shutdown();
    "#,
    );
    // The rolled-back key is gone from the file; what is resident is the write
    // that was made, which is why this is checked after a fresh materialization
    // below rather than only here.
    assert_eq!(ok(&out).trim(), "committed no\n1 true false");
}

/// A `set` inside a transaction that the body does not await is still part of
/// the transaction: the commit waits for it. Committing past it ran the write
/// after the COMMIT — outside the transaction, after the call had returned —
/// and overlapping statements panicked the engine. Found by the shop example
/// under load (`Inventory.commit`).
#[test]
fn a_transaction_waits_for_the_writes_it_did_not_await() {
    let base = dir("collections-unawaited");
    std::fs::write(
        base.join("w.mjs"),
        r#"
        import { DurableWorker } from "runtime:workers";
        export class W extends DurableWorker {
          static schema = { collections: { rows: {} } };
          async go(i) {
            await this.state.transaction(async () => {
              await this.state.collection("rows").insert({ id: String(i) });
              this.state.set("a", i);
              this.state.set("b", i + 1);
            });
            return i;
          }
          async read() {
            return [this.state.get("a"), this.state.get("b"), await this.state.collection("rows").count()].join(" ");
          }
        }
    "#,
    )
    .expect("write class");
    let first = run_in(
        &base,
        "write.mjs",
        r#"import { W } from "./w.mjs";
           import { shutdown } from "runtime:workers";
           let failed = 0;
           for (let i = 0; i < 100; i++) { try { await W.get("w").go(i); } catch { failed++; } }
           console.log("failed", failed);
           await shutdown();"#,
        &[],
    );
    assert_eq!(ok(&first).trim(), "failed 0");
    let second = run_in(
        &base,
        "read.mjs",
        r#"import { W } from "./w.mjs";
           console.log(await W.get("w").read());"#,
        &[],
    );
    assert_eq!(ok(&second).trim(), "99 100 100");
}

/// Collections and keys share one connection, and a connection is one
/// conversation. Interleaving them in a single call must not put two statements
/// on it at once — and everything the call did is durable when it returns.
#[test]
fn keys_and_collections_interleave_safely_and_survive_together() {
    let base = dir("collections-mixed");
    std::fs::write(
        base.join("mixed.mjs"),
        r#"
        import { DurableWorker } from "runtime:workers";
        export class Mixed extends DurableWorker {
          static schema = { collections: { events: { index: ["at"] } } };
          async burst(n) {
            for (let i = 0; i < n; i++) {
              this.state.set(`k${i}`, i);
              await this.state.collection("events").insert({ id: `e${i}`, at: i });
            }
            return this.state.size;
          }
          async report() {
            return `${this.state.size} ${await this.state.collection("events").count()}`;
          }
        }
    "#,
    )
    .expect("write class");

    let first = run_in(
        &base,
        "write.mjs",
        r#"import { Mixed } from "./mixed.mjs";
           console.log("wrote", await Mixed.get("m").burst(25));"#,
        &[],
    );
    assert_eq!(ok(&first).trim(), "wrote 25");

    // No shutdown above: the process simply ended. What the call returned was
    // gated on its writes, so both halves are there.
    let second = run_in(
        &base,
        "read.mjs",
        r#"import { Mixed } from "./mixed.mjs";
           console.log(await Mixed.get("m").report());"#,
        &[],
    );
    assert_eq!(ok(&second).trim(), "25 25");
}

/// The resident ceiling is the *keys'* ceiling. A collection is the answer to
/// data that outgrows it, so it is not measured against it.
#[test]
fn a_collection_is_not_bounded_by_the_resident_ceiling() {
    let out = run(
        "collections-unbounded",
        r#"
        import { DurableWorker, configure, shutdown } from "runtime:workers";
        configure({ stateLimit: 4096, valueLimit: 1024 });
        class W extends DurableWorker {
          static schema = { collections: { blobs: { index: ["n"] } } };
          async fill() {
            const docs = Array.from({ length: 50 }, (_, n) => ({ n, body: "x".repeat(2000) }));
            await this.state.collection("blobs").insertMany(docs);
            return this.state.collection("blobs").count();
          }
          async keyFails() {
            try { await this.state.set("big", "y".repeat(2000)); return "stored"; } catch (e) { return e.code; }
          }
        }
        const w = W.get("a");
        console.log(await w.fill(), await w.keyFails());
        await shutdown();
    "#,
    );
    assert_eq!(ok(&out).trim(), "50 ERR_DURABLE_STATE_TOO_LARGE");
}

/// A query is read in one turn on the connection rather than streamed while the
/// caller iterates — so a loop body that writes to this worker's own state is
/// an ordinary thing to write, instead of a wait on the iteration holding the
/// connection.
#[test]
fn a_query_can_be_iterated_while_the_worker_writes() {
    let out = run(
        "collections-iterate",
        r#"
        import { DurableWorker, shutdown } from "runtime:workers";
        class W extends DurableWorker {
          static schema = { collections: { rows: { index: ["n"] } } };
          async seed(n) {
            await this.state.collection("rows").insertMany(
              Array.from({ length: n }, (_, i) => ({ id: `r${i}`, n: i })),
            );
          }
          async sum() {
            let total = 0;
            for await (const doc of this.state.collection("rows").find().sort({ n: "asc" })) {
              total += doc.n;
              await this.state.set("seen", doc.id);   // the shape that would deadlock
            }
            return `${total} ${this.state.get("seen")}`;
          }
        }
        const w = W.get("a");
        await w.seed(10);
        console.log(await w.sum());
        await shutdown();
    "#,
    );
    assert_eq!(ok(&out).trim(), "45 r9");
}

/// A schema is a literal in the source, so a mistake in one is reported by the
/// line that addresses the worker rather than by a filesystem it never reached.
#[test]
fn a_malformed_schema_is_refused_at_the_address() {
    let out = run(
        "collections-schema",
        r#"
        import { DurableWorker } from "runtime:workers";
        class A extends DurableWorker { static schema = { collection: {} }; }
        class B extends DurableWorker { static schema = { collections: { "no spaces": {} } }; }
        class C extends DurableWorker { static schema = { collections: { ok: { index: "ts" } } }; }
        class D extends DurableWorker { static schema = { collections: { ok: { index: ["id"] } } }; }
        class E extends DurableWorker { static schema = { collections: { ok: { sorted: [] } } }; }
        for (const cls of [A, B, C, D, E]) {
          try { cls.get("x"); console.log("allowed"); } catch (e) { console.log(e.name); }
        }
    "#,
    );
    assert_eq!(
        ok(&out).trim(),
        "TypeError\nTypeError\nTypeError\nTypeError\nTypeError"
    );
}

// ---- alarms ----------------------------------------------------------------

/// The point of a durable timer: the process that set it is gone, and it still
/// runs. Nothing was left running to remember it — the time is in the worker's
/// own file, and the catalog is what makes it findable without opening every
/// file to look.
#[test]
fn an_alarm_set_in_one_process_runs_in_the_next() {
    let base = dir("alarm-restart");
    std::fs::write(
        base.join("job.mjs"),
        r#"
        import { DurableWorker } from "runtime:workers";
        export class Job extends DurableWorker {
          async at(ms) { await this.state.alarm.set(Date.now() + ms); }
          async alarm() { await this.state.set("ran", new Date()); }
          async ran() { return this.state.get("ran") instanceof Date; }
          async pending() { return this.state.alarm.get() !== null; }
        }
    "#,
    )
    .expect("write class");

    let first = run_in(
        &base,
        "set.mjs",
        r#"import { Job } from "./job.mjs";
           import { shutdown } from "runtime:workers";
           await Job.get("j").at(20);
           console.log("scheduled", await Job.get("j").pending(), await Job.get("j").ran());
           await shutdown();"#,
        &[],
    );
    assert_eq!(ok(&first).trim(), "scheduled true false");

    let second = run_in(
        &base,
        "wake.mjs",
        r#"import { Job } from "./job.mjs";
           import { startAlarms, shutdown } from "runtime:workers";
           const alarms = startAlarms({ classes: [Job] });
           await new Promise((r) => setTimeout(r, 300));
           console.log("ran", await Job.get("j").ran(), "pending", await Job.get("j").pending());
           await alarms.stop();
           await shutdown();"#,
        &[],
    );
    assert_eq!(ok(&second).trim(), "ran true pending false");
}

/// The alarm is cleared before the handler runs, so setting the next one inside
/// it is how a worker repeats — and a handler that sets nothing is not woken
/// again.
#[test]
fn an_alarm_repeats_only_while_its_handler_asks_to() {
    let out = run(
        "alarm-repeat",
        r#"
        import { DurableWorker, startAlarms, shutdown } from "runtime:workers";
        class Ticker extends DurableWorker {
          async start_at(ms) { await this.state.alarm.set(Date.now() + ms); }
          async alarm() {
            const n = (this.state.get("n") ?? 0) + 1;
            this.state.set("n", n);
            if (n < 3) await this.state.alarm.set(Date.now() + 10);
          }
          async n() { return this.state.get("n") ?? 0; }
          async pending() { return this.state.alarm.get() !== null; }
        }
        const t = Ticker.get("a");
        await t.start_at(10);
        const alarms = startAlarms({ classes: [Ticker] });
        await new Promise((r) => setTimeout(r, 400));
        console.log(await t.n(), await t.pending());
        await alarms.stop();
        await shutdown();
    "#,
    );
    assert_eq!(ok(&out).trim(), "3 false");
}

/// A failing alarm is retried, and when it has failed for the last time it is
/// *reported* rather than dropped: a scheduled job that fails silently is how a
/// queue loses work. The retry count is stored, so it survives a restart too.
/// An alarm runs as one entry in its worker's mailbox — clearing it, running
/// the handler and recording a retry — so a worker under memory pressure is
/// never closed halfway through one. It was: the scheduler cleared the alarm
/// outside the mailbox, the worker looked idle, and an eviction made by
/// somebody else's call closed its connection mid-alarm. Found by the shop
/// example under load.
#[test]
fn an_alarm_is_not_cut_short_by_an_eviction() {
    let out = run(
        "alarm-eviction",
        r#"
        import { DurableWorker, configure, startAlarms, shutdown } from "runtime:workers";
        configure({ maxLive: 2, evictAfter: 1 });
        class Job extends DurableWorker {
          async arm() { await this.state.alarm.set(Date.now()); }
          async alarm() {
            await new Promise((r) => setTimeout(r, 5));
            this.state.set("ran", true);
          }
          async ran() { return this.state.get("ran") === true; }
        }
        class Other extends DurableWorker { async touch() { return 1; } }
        for (let i = 0; i < 20; i++) await Job.get(`j${i}`).arm();
        const errors = [];
        const alarms = startAlarms({ classes: [Job], onError: (e, c) => errors.push(`${c}: ${e.message}`) });
        // Somebody else's traffic, materializing workers — and so evicting —
        // while the alarms run.
        let ran = 0;
        const deadline = Date.now() + 30_000;
        for (let n = 0; ran < 20 && Date.now() < deadline; n++) {
          for (let i = 0; i < 10; i++) await Other.get(`o${(n * 10 + i) % 40}`).touch();
          ran = 0;
          for (let i = 0; i < 20; i++) if (await Job.get(`j${i}`).ran()) ran++;
        }
        await alarms.stop();
        console.log(ran, errors.length ? errors[0] : "no errors");
        await shutdown();
    "#,
    );
    assert_eq!(ok(&out).trim(), "20 no errors");
}

#[test]
fn a_failing_alarm_is_retried_and_then_reported() {
    let out = run(
        "alarm-retry",
        r#"
        import { DurableWorker, configure, startAlarms, shutdown } from "runtime:workers";
        configure({ alarmRetries: 2 });
        class Flaky extends DurableWorker {
          async at(ms) { await this.state.alarm.set(Date.now() + ms); }
          async alarm() {
            const n = (this.state.get("tries") ?? 0) + 1;
            await this.state.set("tries", n);
            throw new Error(`boom ${n}`);
          }
          async tries() { return this.state.get("tries") ?? 0; }
          async pending() { return this.state.alarm.get() !== null; }
        }
        const f = Flaky.get("f");
        await f.at(5);
        const reported = [];
        const alarms = startAlarms({ classes: [Flaky], onError: (e) => reported.push(e.message) });
        await new Promise((r) => setTimeout(r, 4000));
        console.log(await f.tries(), await f.pending(), reported.join(","));
        await alarms.stop();
        await shutdown();
    "#,
    );
    assert_eq!(ok(&out).trim(), "3 false boom 3");
}

/// An alarm runs through the same mailbox a call does, so it cannot interleave
/// with one — the reason a worker's state never needs a lock.
#[test]
fn an_alarm_waits_for_the_call_in_flight() {
    let out = run(
        "alarm-mailbox",
        r#"
        import { DurableWorker, startAlarms, shutdown } from "runtime:workers";
        class W extends DurableWorker {
          async slow() {
            this.state.set("log", [...(this.state.get("log") ?? []), "call:start"]);
            await new Promise((r) => setTimeout(r, 120));
            this.state.set("log", [...(this.state.get("log") ?? []), "call:end"]);
          }
          async alarm() {
            this.state.set("log", [...(this.state.get("log") ?? []), "alarm"]);
          }
          async at(ms) { await this.state.alarm.set(Date.now() + ms); }
          async log() { return (this.state.get("log") ?? []).join(" "); }
        }
        const w = W.get("a");
        await w.at(10);
        const alarms = startAlarms({ classes: [W] });
        await w.slow();
        await new Promise((r) => setTimeout(r, 200));
        console.log(await w.log());
        await alarms.stop();
        await shutdown();
    "#,
    );
    assert_eq!(ok(&out).trim(), "call:start call:end alarm");
}

/// Setting an alarm on a class with no `alarm()` is refused where the mistake
/// is — at the `set` — rather than by a scheduler nobody is watching.
#[test]
fn an_alarm_needs_a_handler_to_set_it_on() {
    let out = run(
        "alarm-handler",
        r#"
        import { DurableWorker } from "runtime:workers";
        class Deaf extends DurableWorker {
          async trySet() {
            try { await this.state.alarm.set(Date.now() + 1000); return "allowed"; }
            catch (e) { return e.name; }
          }
        }
        console.log(await Deaf.get("a").trySet());
    "#,
    );
    assert_eq!(ok(&out).trim(), "TypeError");
}

/// Alarms run because a process said it would run them. Stopping means stopping:
/// the timer is dropped, the process is free to exit, and what was due stays due
/// for whoever runs next.
#[test]
fn stopping_the_scheduler_leaves_the_alarm_for_next_time() {
    let out = run(
        "alarm-stop",
        r#"
        import { DurableWorker, startAlarms, shutdown } from "runtime:workers";
        class W extends DurableWorker {
          async at(ms) { await this.state.alarm.set(Date.now() + ms); }
          async alarm() { await this.state.set("ran", true); }
          async ran() { return this.state.get("ran") ?? false; }
          async pending() { return this.state.alarm.get() !== null; }
        }
        const w = W.get("a");
        await w.at(150);
        const alarms = startAlarms({ classes: [W] });
        await alarms.stop();
        await new Promise((r) => setTimeout(r, 300));
        console.log(await w.ran(), await w.pending(), alarms.running);
        await shutdown();
    "#,
    );
    assert_eq!(ok(&out).trim(), "false true false");
}

/// A worker whose class this process never defined cannot be woken here — and
/// must not stop the scheduler from doing everything else, nor turn it into a
/// spin on a row that is permanently overdue.
#[test]
fn an_alarm_for_a_class_this_process_does_not_have_is_left_alone() {
    let base = dir("alarm-unknown");
    std::fs::write(
        base.join("both.mjs"),
        r#"
        import { DurableWorker } from "runtime:workers";
        export class Absent extends DurableWorker {
          async at(ms) { await this.state.alarm.set(Date.now() + ms); }
          async alarm() { await this.state.set("ran", true); }
          async ran() { return this.state.get("ran") ?? false; }
        }
    "#,
    )
    .expect("write class");

    let first = run_in(
        &base,
        "set.mjs",
        r#"import { Absent } from "./both.mjs";
           import { shutdown } from "runtime:workers";
           await Absent.get("a").at(5);
           console.log("set");
           await shutdown();"#,
        &[],
    );
    assert_eq!(ok(&first).trim(), "set");

    // This process defines a different class entirely: the overdue row is not
    // one it can run.
    let second = run_in(
        &base,
        "other.mjs",
        r#"import { DurableWorker, startAlarms, shutdown } from "runtime:workers";
           class Present extends DurableWorker {
             async at(ms) { await this.state.alarm.set(Date.now() + ms); }
             async alarm() { await this.state.set("ran", true); }
             async ran() { return this.state.get("ran") ?? false; }
           }
           const p = Present.get("p");
           await p.at(10);
           const alarms = startAlarms({ classes: [Present], onError: (e) => console.log("REPORTED", e.message) });
           await new Promise((r) => setTimeout(r, 300));
           console.log("mine ran:", await p.ran());
           await alarms.stop();
           await shutdown();"#,
        &[],
    );
    assert_eq!(ok(&second).trim(), "mine ran: true");

    // …and the one it could not run is still waiting for a process that can.
    let third = run_in(
        &base,
        "back.mjs",
        r#"import { Absent } from "./both.mjs";
           import { startAlarms, shutdown } from "runtime:workers";
           const alarms = startAlarms({ classes: [Absent] });
           await new Promise((r) => setTimeout(r, 300));
           console.log("ran:", await Absent.get("a").ran());
           await alarms.stop();
           await shutdown();"#,
        &[],
    );
    assert_eq!(ok(&third).trim(), "ran: true");
}

/// The class list is required, because a scheduler that guessed would service
/// whatever this process happened to have addressed — firing an alarm on a busy
/// deployment and not on an idle one.
#[test]
fn the_scheduler_must_be_told_which_classes_it_runs() {
    let out = run(
        "alarm-classes",
        r#"
        import { DurableWorker, startAlarms } from "runtime:workers";
        class W extends DurableWorker { async alarm() {} }
        for (const bad of [undefined, {}, { classes: [] }, { classes: [class X {}] }]) {
          try { startAlarms(bad); console.log("allowed"); } catch (e) { console.log(e.name); }
        }
        // …and one at a time: a second call would otherwise ignore the classes
        // it was given.
        const alarms = startAlarms({ classes: [W] });
        try { startAlarms({ classes: [W] }); console.log("twice"); } catch (e) { console.log(e.name); }
        await alarms.stop();
    "#,
    );
    assert_eq!(
        ok(&out).trim(),
        "TypeError\nTypeError\nTypeError\nTypeError\nTypeError"
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

// ---- shards (DECISIONS D122) -----------------------------------------------

/// A sharded program needs `workers` to start its shards and `imports` for
/// them to load the classes, beside the grants every durable program needs.
const SHARD_GRANTS: &[&str] = &[
    "--allow-read",
    "--allow-write",
    "--allow-imports",
    "--allow-workers",
];

/// The classes every shard test runs, in the module the shards import.
const SHARDED: &str = r#"
    import { DurableWorker } from "runtime:workers";
    export class Cart extends DurableWorker {
      static schema = { collections: { lines: { index: ["qty"] } } };
      async add(item) {
        const items = this.state.get("items") ?? [];
        items.push(item);
        this.state.set("items", items);
        return items.length;
      }
      async read() { return (this.state.get("items") ?? []).join(","); }
      onThread() {
        return typeof DedicatedWorkerGlobalScope === "function" &&
          self instanceof DedicatedWorkerGlobalScope;
      }
      async kinds() {
        await this.state.setMany({ d: new Date(5), m: new Map([[1, 2]]), b: 10n });
        return [this.state.get("d") instanceof Date, this.state.get("m").get(1), typeof this.state.get("b")].join(" ");
      }
      async kindsBack() {
        return [this.state.get("d").getTime(), this.state.get("m").get(1), this.state.get("b")].join(" ");
      }
      async lines() {
        const lines = this.state.collection("lines");
        await lines.insertMany([{ id: "a", qty: 1 }, { id: "b", qty: 2 }, { id: "c", qty: 3 }]);
        await lines.update("a", (doc) => ({ ...doc, qty: 9 }));
        await lines.update("b", { qty: 0 });
        const big = await lines.find({ qty: { gte: 3 } }).sort({ qty: "desc" }).toArray();
        return `${big.map((d) => d.id + d.qty).join(",")} ${await lines.count()}`;
      }
      async rollback() {
        try {
          await this.state.transaction(async () => {
            await this.state.collection("lines").insert({ id: "gone", qty: 5 });
            await this.state.set("goneKey", 1);
            throw new RangeError("undone");
          });
        } catch (e) {
          return `${e.name}:${e.message}`;
        }
      }
      async committed() {
        await this.state.transaction(async () => {
          await this.state.collection("lines").insert({ id: "kept", qty: 4 });
          await this.state.set("keptKey", 1);
        });
      }
      async fsDenied() {
        const fs = await import("runtime:fs");
        try { await fs.stat("app.mjs"); return "read"; } catch (e) { return e.code ?? e.name; }
      }
      async big() { await this.state.set("big", new Uint8Array(200_000)); }
      async callOther(id) { return Cart.get(id).add("from-" + this.id); }
      async listed() { return (await Cart.list()).map((w) => w.id).sort().join(","); }
      async remove(id) { return Cart.delete(id); }
      spin() { for (;;) {} }
      async crash() {
        setTimeout(() => { throw new Error("boom"); });
        await new Promise((r) => setTimeout(r, 100));
      }
      async stop(reason) { this.state.set("stopped", reason); }
      async stopped() { return this.state.get("stopped") ?? "never"; }
      async signalled() { return this.ctx.signal.aborted; }
    }
    export class Ticker extends DurableWorker {
      async alarm() {
        const n = (this.state.get("n") ?? 0) + 1;
        this.state.set("n", n);
        if (n < 3) await this.state.alarm.set(Date.now());
      }
      async arm() { await this.state.alarm.set(Date.now()); }
      async n() { return this.state.get("n") ?? 0; }
      async pending() { return this.state.alarm.get() !== null; }
    }
    export class Flaky extends DurableWorker {
      async alarm() {
        this.state.set("tries", (this.state.get("tries") ?? 0) + 1);
        throw new Error("not yet");
      }
      async arm() { await this.state.alarm.set(Date.now()); }
      async seen() { return [this.state.get("tries"), this.state.alarm.get() instanceof Date].join(" "); }
    }
"#;

/// A directory with the shared classes in it, and a program that configures
/// `shards` shards over them before running `body`.
fn sharded(test: &str) -> PathBuf {
    let base = dir(test);
    std::fs::write(base.join("classes.mjs"), SHARDED).expect("write classes");
    base
}

fn run_sharded(base: &PathBuf, name: &str, shards: u32, body: &str) -> Output {
    let source = format!(
        r#"import {{ configure, startAlarms, shutdown }} from "runtime:workers";
           import {{ Cart, Ticker, Flaky }} from "./classes.mjs";
           configure({{ shards: {shards}, module: new URL("./classes.mjs", import.meta.url) }});
           {body}"#
    );
    run_in(base, name, &source, SHARD_GRANTS)
}

/// The class's code runs on a thread of its own and the storage stays on the
/// host: state written there is state the next process — sharded or not —
/// reads back, with every value kind intact.
#[test]
fn a_sharded_worker_runs_on_its_own_thread_and_its_state_survives() {
    let base = sharded("shard-restart");
    let first = run_sharded(
        &base,
        "write.mjs",
        2,
        r#"const c = Cart.get("c1");
           console.log(await c.onThread(), await c.add("x"), await c.add("y"), await c.kinds());
           await shutdown();"#,
    );
    assert_eq!(ok(&first).trim(), "true 1 2 true 2 bigint");

    let second = run_in(
        &base,
        "read.mjs",
        r#"import { Cart } from "./classes.mjs";
           const c = Cart.get("c1");
           console.log(await c.onThread(), await c.read(), await c.kindsBack());"#,
        &[],
    );
    assert_eq!(ok(&second).trim(), "false x,y 5 2 10");
}

/// A shard holds no filesystem grant — the file is the host's — and needs none.
#[test]
fn a_shard_needs_no_filesystem_grant() {
    let base = sharded("shard-nofs");
    let out = run_sharded(
        &base,
        "app.mjs",
        1,
        r#"const c = Cart.get("c");
           console.log(await c.fsDenied(), await c.add("x"));
           await shutdown();"#,
    );
    assert_eq!(ok(&out).trim(), "ERR_CAPABILITY_DENIED 1");
}

/// Collections and transactions go to the host by message, so a shard's query,
/// its function `update` and its rollback behave as they do unsharded.
#[test]
fn collections_and_transactions_work_from_a_shard() {
    let base = sharded("shard-collections");
    let out = run_sharded(
        &base,
        "app.mjs",
        1,
        r#"const c = Cart.get("c");
           console.log(await c.lines());
           console.log(await c.rollback());
           await c.committed();
           await shutdown();"#,
    );
    assert_eq!(ok(&out).trim(), "a9,c3 3\nRangeError:undone");

    // Read back unsharded, from the file: the rolled-back key and document are
    // not there, the committed ones are.
    let count = run_in(
        &base,
        "count.mjs",
        r#"import { Cart } from "./classes.mjs";
           Cart.prototype.probe = async function () {
             return [await this.state.collection("lines").count(), this.state.has("goneKey"), this.state.has("keptKey")].join(" ");
           };
           console.log(await Cart.get("c").probe());"#,
        &[],
    );
    assert_eq!(ok(&count).trim(), "4 false true");
}

/// The gate holds across the thread: what a sharded call acknowledged is on
/// disk when the process is killed outright.
#[test]
fn acknowledged_writes_from_a_shard_survive_a_kill() {
    let base = sharded("shard-kill");
    std::fs::write(
        base.join("write.mjs"),
        r#"import { configure } from "runtime:workers";
           import { Cart } from "./classes.mjs";
           configure({ shards: 2, module: new URL("./classes.mjs", import.meta.url) });
           const c = Cart.get("main");
           for (let i = 1; i <= 5; i++) await c.add(`e${i}`);
           console.log("ACKED");
           await new Promise(() => {});"#,
    )
    .expect("write module");
    let mut child = Command::new(env!("CARGO_BIN_EXE_esrun"))
        .current_dir(&base)
        .args(SHARD_GRANTS)
        .arg("write.mjs")
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn esrun");
    let out = child.stdout.take().expect("piped stdout");
    let marker = BufReader::new(out)
        .lines()
        .next()
        .expect("a line before the kill")
        .expect("read child stdout");
    assert_eq!(marker, "ACKED");
    child.kill().expect("kill esrun");
    child.wait().expect("reap esrun");

    let after = run_in(
        &base,
        "read.mjs",
        r#"import { Cart } from "./classes.mjs";
           console.log(await Cart.get("main").read());"#,
        &[],
    );
    assert_eq!(ok(&after).trim(), "e1,e2,e3,e4,e5");
}

/// An idle shard is not a reason to stay up: a script that used one exits
/// without calling `shutdown()`, as an unsharded one does.
#[test]
fn an_idle_shard_does_not_keep_the_process_alive() {
    let base = sharded("shard-exit");
    std::fs::write(
        base.join("app.mjs"),
        r#"import { configure } from "runtime:workers";
           import { Cart } from "./classes.mjs";
           configure({ shards: 2, module: new URL("./classes.mjs", import.meta.url) });
           console.log(await Cart.get("c").add("x"));"#,
    )
    .expect("write module");
    let mut child = Command::new(env!("CARGO_BIN_EXE_esrun"))
        .current_dir(&base)
        .args(SHARD_GRANTS)
        .arg("app.mjs")
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn esrun");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll esrun") {
            break status;
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            panic!("the process did not exit with an idle shard");
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    assert!(status.success());
}

/// A call that never returns to the event loop is ended by the watchdog. Only
/// that shard goes; its workers come back, from disk, on the next call.
#[test]
fn a_shard_stuck_in_a_loop_is_ended_and_its_workers_come_back() {
    let base = sharded("shard-stuck");
    let out = run_sharded(
        &base,
        "app.mjs",
        1,
        r#"const c = Cart.get("c");
           await c.add("before");
           try { await c.spin(); console.log("returned"); }
           catch (e) { console.log(e.code, /event loop/.test(e.message)); }
           console.log(await c.read(), await c.add("after"));
           await shutdown();"#,
    );
    assert_eq!(ok(&out).trim(), "ERR_DURABLE_SHARD_LOST true\nbefore 2");
}

/// A shard that dies of an uncaught error loses the call in flight and nothing
/// else — the next call is answered by a replacement.
#[test]
fn a_shard_that_crashes_loses_only_the_call_in_flight() {
    let base = sharded("shard-crash");
    let out = run_sharded(
        &base,
        "app.mjs",
        1,
        r#"const c = Cart.get("c");
           await c.add("kept");
           try { await c.crash(); console.log("returned"); }
           catch (e) { console.log(e.code, /boom/.test(e.message)); }
           console.log(await c.read());
           await shutdown();"#,
    );
    assert_eq!(ok(&out).trim(), "ERR_DURABLE_SHARD_LOST true\nkept");
}

/// A worker on a shard reaches other workers — and `list`/`delete` — through
/// the host, where the workers are.
#[test]
fn a_sharded_worker_can_address_other_workers() {
    let base = sharded("shard-ref");
    let out = run_sharded(
        &base,
        "app.mjs",
        2,
        r#"const a = Cart.get("a");
           console.log(await a.callOther("b"), await a.callOther("c"), await Cart.get("b").read());
           console.log(await a.listed(), await a.remove("c"), await a.listed());
           await shutdown();"#,
    );
    assert_eq!(ok(&out).trim(), "1 1 from-a\na,b,c true a,b");
}

/// The rules that hold unsharded hold on a shard: the ceilings, a missing
/// method, and `stop()` running — and writing — when the worker is closed.
#[test]
fn a_sharded_worker_keeps_the_same_rules() {
    let base = sharded("shard-rules");
    let out = run_sharded(
        &base,
        "app.mjs",
        1,
        r#"const c = Cart.get("c");
           try { await c.big(); } catch (e) { console.log(e.code); }
           try { await c.nope(); } catch (e) { console.log(e.name, e.message); }
           await shutdown();"#,
    );
    assert_eq!(
        ok(&out).trim(),
        "ERR_DURABLE_STATE_TOO_LARGE\nTypeError Cart(\"c\") has no method nope()"
    );
    let after = run_in(
        &base,
        "read.mjs",
        r#"import { Cart } from "./classes.mjs";
           console.log(await Cart.get("c").stopped());"#,
        &[],
    );
    assert_eq!(ok(&after).trim(), "shutdown");
}

/// Alarms fire on the shard a worker runs on. A handler that sets the next
/// time repeats; one that fails is retried, and the retry the scheduler sets
/// is what the shard's `alarm.get()` then reports.
#[test]
fn alarms_run_on_a_shard() {
    let base = sharded("shard-alarms");
    let out = run_sharded(
        &base,
        "app.mjs",
        2,
        r#"const reported = [];
           const alarms = startAlarms({ classes: [Ticker, Flaky], onError: (e, c) => reported.push(c) });
           await Ticker.get("t").arm();
           await Flaky.get("f").arm();
           const until = Date.now() + 5000;
           while (Date.now() < until && (await Ticker.get("t").n()) < 3) {
             await new Promise((r) => setTimeout(r, 20));
           }
           await new Promise((r) => setTimeout(r, 200));
           console.log(await Ticker.get("t").n(), await Ticker.get("t").pending());
           console.log(await Flaky.get("f").seen(), reported.length);
           await alarms.stop();
           await shutdown();"#,
    );
    assert_eq!(ok(&out).trim(), "3 false\n1 true 0");
}

/// A shard has to be told which module defines its classes, as an absolute
/// URL — every shard imports it, and a relative string would mean a different
/// file to each of them.
#[test]
fn shards_are_configured_with_an_absolute_module() {
    let out = run(
        "shard-config",
        r#"import { configure } from "runtime:workers";
           try { configure({ shards: 2 }); } catch (e) { console.log(e.name); }
           try { configure({ shards: 2, module: "./classes.mjs" }); } catch (e) { console.log(e.name); }
           console.log(configure({ shards: 0 }).shards);"#,
    );
    assert_eq!(ok(&out).trim(), "TypeError\nTypeError\n0");
}

/// A shard runs only what its module exports; anything else is refused by
/// name at the first call, rather than run somewhere it cannot be found.
#[test]
fn a_class_the_module_does_not_export_is_refused() {
    let base = sharded("shard-unexported");
    let out = run_sharded(
        &base,
        "app.mjs",
        1,
        r#"class Hidden extends Cart { static durableName = "Hidden"; }
           try { await Hidden.get("h").read(); console.log("ran"); }
           catch (e) { console.log(e.name, /does not export/.test(e.message)); }
           await shutdown();"#,
    );
    assert_eq!(ok(&out).trim(), "TypeError true");
}
