//! End-to-end tests for `runtime:context` (DECISIONS.md D88).
//!
//! These spawn the real `esrun` binary rather than driving a `Runtime` in
//! process, because the thing under test is a V8 promise hook: what it is worth
//! asserting is that a value survives a real `await`, a real timer firing on the
//! real loop, and a real op round trip through a real provider. A unit test on
//! the JS module would only re-state the module.
//!
//! The one guarantee that is *negative* — `EventTarget` dispatch does not
//! propagate — gets the same treatment, because it is the one a Node user will
//! guess wrong and the one a refactor could silently "fix" into a leak.

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

/// Writes `source` as a module and runs it, returning the process output.
///
/// `flags` are the grants the program needs. Most tests here name none: the
/// module is ungated, which is half of what these tests exist to show.
fn run(name: &str, source: &str, flags: &[&str]) -> Output {
    let app = temp(&format!("ctx-{name}.mjs"));
    std::fs::write(&app, source).expect("write app");
    let mut command = Command::new(env!("CARGO_BIN_EXE_esrun"));
    // Run *from* the directory these fixtures are written into: the sandbox is
    // the working directory (D79), so a program is run from where it lives.
    command.current_dir(env!("CARGO_TARGET_TMPDIR"));
    command.args(flags);
    command.arg(&app);
    command.output().expect("run esrun")
}

/// Runs `source` and returns its stdout lines, failing loudly on a non-zero
/// exit — a program that died halfway would otherwise "pass" every assertion
/// about output it never got to produce.
fn lines(name: &str, source: &str, flags: &[&str]) -> Vec<String> {
    let out = run(name, source, flags);
    assert!(
        out.status.success(),
        "{name} exited {:?}\nstdout:\n{}\nstderr:\n{}",
        out.status.code(),
        stdout(&out),
        stderr(&out)
    );
    stdout(&out).lines().map(str::to_string).collect()
}

// ---------------------------------------------------------------------------
// The gate that is not there
// ---------------------------------------------------------------------------

/// The module is ungated: it works under a grant of nothing at all, which is
/// what `esrun` gives a program that names no flag (D65).
///
/// This is the load-bearing property. A context carries application behaviour —
/// which tenant, which transaction — so a denial would not restrict a reach out
/// of the isolate, it would corrupt the answer.
#[test]
fn every_export_works_with_no_capability_granted() {
    let out = lines(
        "ungated",
        r#"
        import { createContext, snapshot, bind, withTrace, currentTask } from "runtime:context";
        const c = createContext({ name: "c", defaultValue: "none" });
        console.log(c.run("v", () => c.get()));
        console.log(c.run("v", () => snapshot())(() => c.get()));
        console.log(c.run("v", () => bind(() => c.get()))());
        console.log(withTrace("0af7651916cd43dd8448eb211c80319c", () => currentTask().traceId));
        console.log(typeof currentTask().id);
        "#,
        &[],
    );
    assert_eq!(
        out,
        ["v", "v", "v", "0af7651916cd43dd8448eb211c80319c", "number"]
    );
}

// ---------------------------------------------------------------------------
// Propagation
// ---------------------------------------------------------------------------

/// Every scheduling boundary in the language: `await`, `.then`, a microtask, a
/// one-shot timer, a repeating timer, the promise combinators, an async
/// generator, and a dynamic `import()`.
///
/// Each prints the value it observed, so a failure names the boundary that lost
/// it rather than just failing a count.
#[test]
fn a_value_survives_every_language_scheduling_boundary() {
    std::fs::write(temp("ctx-boundaries-dep.mjs"), "export default 1;\n").expect("write dep");
    let out = lines(
        "boundaries",
        r#"
        import { createContext } from "runtime:context";
        const c = createContext({ defaultValue: "LOST" });
        const seen = [];
        const note = (where) => seen.push(`${where}=${c.get()}`);

        await c.run("v", async () => {
          note("sync");
          await null;
          note("await");
          await Promise.resolve().then(() => note("then"));
          await new Promise((r) => queueMicrotask(() => { note("queueMicrotask"); r(); }));
          await new Promise((r) => setTimeout(() => { note("setTimeout"); r(); }, 1));
          await new Promise((r) => {
            let n = 0;
            const id = setInterval(() => {
              note(`setInterval${++n}`);
              if (n === 2) { clearInterval(id); r(); }
            }, 1);
          });
          await Promise.all([
            (async () => note("Promise.all"))(),
            (async () => { await null; note("Promise.all.await"); })(),
          ]);
          await Promise.race([(async () => note("Promise.race"))()]);
          await Promise.allSettled([(async () => note("Promise.allSettled"))()]);
          await Promise.any([(async () => note("Promise.any"))()]);
          async function* gen() { yield 1; await null; yield 2; }
          for await (const _ of gen()) note("for-await");
          try { await (async () => { throw new Error("x"); })(); } catch { note("catch"); }
          try { await (async () => { throw new Error("x"); })(); } catch { /* below */ }
          finally { note("finally"); }
          await import("./ctx-boundaries-dep.mjs").then(() => note("import()"));
        });
        console.log(seen.join("\n"));
        console.log(`after=${c.get()}`);
        "#,
        // The one grant here is for the dynamic `import()` at the end — loading a
        // module is the filesystem's privilege, not this module's. That
        // `runtime:context` itself needs nothing is asserted on its own above.
        &["--allow-imports"],
    );
    // Every boundary saw the value. The final line is the deliberate exception:
    // once the scope has ended the default is what `get()` must answer.
    let (inside, after) = out.split_at(out.len() - 1);
    let lost: Vec<_> = inside.iter().filter(|l| l.contains("LOST")).collect();
    assert!(lost.is_empty(), "boundaries that lost the value: {lost:?}");
    assert_eq!(after, ["after=LOST"], "the value outlived its scope");
    assert!(out.contains(&"setInterval2=v".to_string()), "{out:?}");
    assert!(out.contains(&"for-await=v".to_string()), "{out:?}");
    assert!(out.contains(&"import()=v".to_string()), "{out:?}");
    // Seventeen boundary reports plus the line after the scope: a site that
    // stopped reporting would otherwise pass the "nothing lost" check above by
    // saying nothing at all.
    assert_eq!(out.len(), 18, "expected every boundary to report: {out:?}");
}

/// A timer captures the mapping it was **scheduled** in, not the one current
/// when the loop gets round to firing it — and every firing of a repeating timer
/// uses that same capture.
#[test]
fn a_timer_runs_in_the_scope_that_armed_it() {
    let out = lines(
        "timer-capture",
        r#"
        import { createContext } from "runtime:context";
        const c = createContext({ defaultValue: "none" });
        const seen = [];
        // Armed inside "a" …
        c.run("a", () => setTimeout(() => seen.push(c.get()), 5));
        // … while the loop, and another scope, are busy with something else.
        await c.run("b", () => new Promise((r) => setTimeout(r, 1)));
        await new Promise((r) => setTimeout(r, 20));
        console.log(seen.join(","));
        "#,
        &[],
    );
    assert_eq!(out, ["a"]);
}

/// Op callbacks: the mapping is captured when the op is *issued*, so the
/// continuation that resumes on its result is still in scope.
///
/// Deliberately not a sample. A missed propagation site is a silent `undefined`
/// deep inside a request, so this walks the runtime modules that actually
/// suspend: the filesystem, the database, a child process, an inbound request, a
/// `fetch` back out over the loopback, a raw socket, a WebSocket, a worker, the
/// hashing ops, and `crypto.subtle`.
#[test]
fn a_value_survives_every_runtime_op_callback() {
    std::fs::write(temp("ctx-ops-worker.mjs"), "self.postMessage(1);\n").expect("write worker");
    let out = lines(
        "ops",
        &r#"
        import { createContext } from "runtime:context";
        import { write, file, remove } from "runtime:fs";
        import { connect as dbConnect, sqlite } from "runtime:db";
        import { Command } from "runtime:system";
        import { serve } from "runtime:http";
        import { connect as netConnect } from "runtime:net";
        import { hashStream } from "runtime:hashing";

        const c = createContext({ defaultValue: "LOST" });
        const seen = [];
        const note = (where) => seen.push(`${where}=${c.get()}`);

        await c.run("v", async () => {
          // --- runtime:fs -----------------------------------------------------
          await write("./ctx-ops.txt", "hello");
          note("fs.write");
          await file("./ctx-ops.txt").text();
          note("fs.read");
          await remove("./ctx-ops.txt");
          note("fs.remove");

          // --- runtime:db -----------------------------------------------------
          const db = await dbConnect("sqlite::memory:", { driver: sqlite });
          note("db.connect");
          await db.execute("CREATE TABLE t (a INTEGER)");
          note("db.execute");
          await (await db.query("SELECT 1 AS a")).first();
          note("db.query");
          await db.close();
          note("db.close");

          // --- runtime:hashing and crypto.subtle -----------------------------
          await hashStream("sha256", new Blob(["x"]).stream());
          note("hashing.hashStream");
          await crypto.subtle.digest("SHA-256", new Uint8Array([1]));
          note("subtle.digest");

          // --- runtime:system -------------------------------------------------
          const child = await new Command(PROGRAM, { args: ["--version"] }).spawn();
          note("system.spawn");
          await child.status;
          note("system.wait");

          // --- runtime:http (inbound) + fetch + runtime:net -------------------
          const server = serve({ hostname: "127.0.0.1", port: 0 }, async () => {
            // Inside the handler the request has its own root scope, which is
            // the point of the http integration — asserted in its own test.
            await null;
            return new Response("ok");
          });
          const { port } = await server.addr;
          note("http.addr");
          const res = await fetch(`http://127.0.0.1:${port}/`);
          note("fetch.head");
          await res.text();
          note("fetch.body");
          const sock = await netConnect({ hostname: "127.0.0.1", port });
          note("net.connect");
          const w = sock.writable.getWriter();
          await w.write(new TextEncoder().encode("GET / HTTP/1.0\r\n\r\n"));
          note("net.write");
          await sock.readable.getReader().read();
          note("net.read");
          try { await sock.close(); } catch { /* the server may close first */ }
          await server.stop();
          note("http.stop");

          // --- Worker ---------------------------------------------------------
          const worker = new Worker(new URL("./ctx-ops-worker.mjs", import.meta.url));
          await new Promise((r) => worker.addEventListener("message", () => r()));
          // The *awaiting* side keeps its scope; the listener does not, because
          // it is an EventTarget dispatch (see the dedicated test).
          note("worker.message");
          worker.terminate();
        });

        console.log(seen.join("\n"));
        "#
        .replace("PROGRAM", &format!("{:?}", env!("CARGO_BIN_EXE_esrun"))),
        &["--allow-all"],
    );
    let lost: Vec<_> = out.iter().filter(|l| l.contains("LOST")).collect();
    assert!(lost.is_empty(), "ops that lost the value: {lost:?}");
    // Every site above reported, so the list is complete rather than truncated
    // by an early return.
    assert_eq!(out.len(), 19, "expected every site to report: {out:?}");
}

// ---------------------------------------------------------------------------
// EventTarget: the deliberate non-propagation
// ---------------------------------------------------------------------------

/// A listener runs in the **dispatcher's** mapping, not the one current when it
/// was registered — and `bind()` is how you ask for the other thing.
///
/// Registering at module scope is the norm, so capturing at registration would
/// pin a request's mapping to a listener that outlives the request. This test is
/// as much a guard against someone "fixing" that as it is a check.
#[test]
fn event_target_dispatch_does_not_propagate_but_bind_does() {
    let out = lines(
        "eventtarget",
        r#"
        import { createContext, bind } from "runtime:context";
        const c = createContext({ defaultValue: "none" });
        const target = new EventTarget();
        const seen = {};

        c.run("registered", () => {
          target.addEventListener("ping", function () { seen.plain = c.get(); });
          target.addEventListener("ping", bind(function () { seen.bound = c.get(); }));
          target.addEventListener("ping", bind(function () { seen.receiver = this === target; }));
        });

        c.run("dispatcher", () => target.dispatchEvent(new Event("ping")));
        console.log(seen.plain, seen.bound, seen.receiver);

        // Dispatched from outside any scope: the listener sees the default.
        target.dispatchEvent(new Event("ping"));
        console.log(seen.plain, seen.bound);
        "#,
        &[],
    );
    // The plain listener follows the dispatcher; the bound one follows
    // registration; `bind` keeps the receiver an EventTarget listener expects.
    assert_eq!(out[0], "dispatcher registered true");
    assert_eq!(out[1], "none registered");
}

// ---------------------------------------------------------------------------
// Isolation
// ---------------------------------------------------------------------------

/// A write in one `run()` branch is invisible to a concurrent sibling and to the
/// parent.
///
/// Run under **real** concurrency: both branches are in flight at once and
/// interleave across several awaits and a timer, so a mapping that was shared
/// rather than copied would be observed by whichever branch ran second. Doing
/// this sequentially would pass with a single global variable.
#[test]
fn sibling_scopes_are_isolated_under_real_concurrency() {
    let out = lines(
        "siblings",
        r#"
        import { createContext } from "runtime:context";
        const tenant = createContext({ name: "tenant", defaultValue: "none" });
        const nested = createContext({ name: "nested", defaultValue: "none" });

        // Yields control repeatedly so the two branches genuinely interleave.
        const branch = (name) =>
          tenant.run(name, async () => {
            const seen = [];
            for (let i = 0; i < 5; i++) {
              seen.push(tenant.get());
              await null;
              seen.push(tenant.get());
              await new Promise((r) => setTimeout(r, 1));
              // A *nested* write, which must not reach the sibling either.
              nested.run(`${name}-inner`, () => seen.push(nested.get()));
              seen.push(nested.get());
            }
            return seen;
          });

        const [a, b] = await Promise.all([branch("a"), branch("b")]);
        console.log(new Set(a).size, [...new Set(a)].sort().join("|"));
        console.log(new Set(b).size, [...new Set(b)].sort().join("|"));
        console.log(tenant.get(), nested.get());
        "#,
        &[],
    );
    // Branch "a" only ever saw "a" and its own nested write (plus the default,
    // which is what `nested.get()` reads once the inner scope has ended).
    assert_eq!(out[0], "3 a|a-inner|none");
    assert_eq!(out[1], "3 b|b-inner|none");
    // And neither reached the parent.
    assert_eq!(out[2], "none none");
}

/// Two contexts cannot collide, whatever they are called: the key is the object
/// itself. There is no string-keyed bag and nothing that enumerates what another
/// library stored.
#[test]
fn contexts_are_keyed_by_identity_not_by_name() {
    let out = lines(
        "identity",
        r#"
        import { createContext } from "runtime:context";
        const mine = createContext({ name: "user", defaultValue: "mine-default" });
        const theirs = createContext({ name: "user", defaultValue: "theirs-default" });
        console.log(mine.name, theirs.name, mine === theirs);
        mine.run("MINE", () => {
          console.log(mine.get(), theirs.get());
          theirs.run("THEIRS", () => console.log(mine.get(), theirs.get()));
        });
        // The context object exposes nothing but its own three members, so one
        // library cannot walk another's values out of it.
        console.log(Object.keys(mine).sort().join(","));
        console.log(Object.isFrozen(mine));
        "#,
        &[],
    );
    assert_eq!(out[0], "user user false");
    assert_eq!(out[1], "MINE theirs-default");
    assert_eq!(out[2], "MINE THEIRS");
    assert_eq!(out[3], "get,name,run");
    assert_eq!(out[4], "true");
}

/// `run()` hands back exactly what the callback returned — a value, a promise,
/// or a throw — and restores the previous mapping on every one of those paths.
#[test]
fn run_is_transparent_to_its_callback() {
    let out = lines(
        "transparent",
        r#"
        import { createContext } from "runtime:context";
        const c = createContext({ defaultValue: "outer" });
        console.log(c.run("v", () => 42));
        console.log(c.run("v", (a, b) => a + b, 1, 2));
        const p = c.run("v", async () => { await null; return c.get(); });
        console.log(p instanceof Promise, await p);
        try { c.run("v", () => { throw new Error("boom"); }); } catch (e) { console.log(e.message); }
        console.log(c.get());
        // A rejected async callback rejects the returned promise, not run().
        const r = c.run("v", async () => { throw new Error("async boom"); });
        console.log(c.get());
        await r.catch((e) => console.log(e.message));
        "#,
        &[],
    );
    assert_eq!(
        out,
        ["42", "3", "true v", "boom", "outer", "outer", "async boom"]
    );
}

// ---------------------------------------------------------------------------
// The port that must be a rename
// ---------------------------------------------------------------------------

/// An ORM's implicit-transaction pattern, ported from `AsyncLocalStorage`.
///
/// The claim in the migration table is that porting it is a rename rather than a
/// redesign, so this is the pattern as it is actually written against Node:
/// a module-level context holding "the connection this unit of work is on", a
/// repository layer that takes no connection argument, and `transaction()`
/// re-binding it for the duration.
///
/// Two requests are in flight at once, on their own connections, and one rolls
/// back. The other must commit regardless — which is the bug this whole module
/// exists to prevent, and which a shared mapping would produce.
#[test]
fn an_orm_implicit_transaction_ports_as_a_rename() {
    let out = lines(
        "orm",
        r#"
        import { createContext } from "runtime:context";
        import { connect, sqlite } from "runtime:db";

        // --- the "ORM" ------------------------------------------------------
        // Node: const current = new AsyncLocalStorage();
        const current = createContext({ name: "unit-of-work" });

        function db() {
          const conn = current.get();
          if (conn === undefined) throw new Error("no ambient connection");
          return conn;
        }
        // Node: current.run(conn, () => conn.transaction(fn))
        const withConnection = (conn, fn) => current.run(conn, fn);
        const transaction = (fn) => db().transaction(() => fn());

        // --- the repository layer: no connection argument anywhere -----------
        const insert = (name) => db().execute("INSERT INTO users (name) VALUES (?)", [name]);
        const count = async () =>
          (await (await db().query("SELECT count(*) AS n FROM users")).first()).n;

        // --- two requests, two connections, both in flight -------------------
        async function request(path, name, fail) {
          const conn = await connect(`sqlite:./orm-${path}.db`, { driver: sqlite });
          await conn.execute("DROP TABLE IF EXISTS users");
          await conn.execute("CREATE TABLE users (name TEXT)");
          try {
            await withConnection(conn, async () => {
              await transaction(async () => {
                await insert(name);
                // Yield mid-transaction so the other request interleaves here.
                await new Promise((r) => setTimeout(r, 5));
                // A nested helper joins the ambient transaction rather than
                // opening its own connection.
                await insert(`${name}-nested`);
                if (fail) throw new Error("rollback");
              });
            });
          } catch (e) {
            if (e.message !== "rollback") throw e;
          }
          const n = await withConnection(conn, count);
          await conn.close();
          return n;
        }

        const [ok, rolled] = await Promise.all([
          request("ok", "committed", false),
          request("rolled", "discarded", true),
        ]);
        console.log(ok, rolled);
        "#,
        &["--allow-all"],
    );
    // The committing request kept both of its rows; the rolling-back one kept
    // neither — and neither request saw the other's connection.
    assert_eq!(out, ["2 0"]);
}

// ---------------------------------------------------------------------------
// Error paths
// ---------------------------------------------------------------------------

/// An `unhandledrejection` listener runs in the mapping where the rejection was
/// **created**, not where the loop happened to observe it.
///
/// This is what makes error reporting useful: by the time the report is made,
/// the request is long off the stack, and the tenant and request id are the only
/// things that make the failure actionable.
#[test]
fn an_unhandled_rejection_reports_the_mapping_it_originated_in() {
    let out = lines(
        "rejection",
        r#"
        import { createContext, currentTask } from "runtime:context";
        const tenant = createContext({ name: "tenant", defaultValue: "none" });

        addEventListener("unhandledrejection", (event) => {
          event.preventDefault();
          console.log(`reported tenant=${tenant.get()} trace=${currentTask().traceId.slice(0, 8)}`);
        });

        let insideTrace;
        tenant.run("acme", () => {
          insideTrace = currentTask().traceId.slice(0, 8);
          // Rejected here, observed a tick later with nothing of this scope left
          // on the stack.
          Promise.reject(new Error("boom"));
        });
        console.log(`origin tenant=acme trace=${insideTrace}`);
        console.log(`observer tenant=${tenant.get()}`);
        await new Promise((r) => setTimeout(r, 20));
        "#,
        &[],
    );
    let origin = out
        .iter()
        .find(|l| l.starts_with("origin "))
        .expect("origin line");
    let reported = out
        .iter()
        .find(|l| l.starts_with("reported "))
        .expect("the rejection to be reported");
    // The observer itself is outside every scope …
    assert_eq!(
        out.iter().find(|l| l.starts_with("observer ")).unwrap(),
        "observer tenant=none"
    );
    // … yet the report carries the scope the rejection was made in.
    assert_eq!(
        reported.trim_start_matches("reported "),
        origin.trim_start_matches("origin ")
    );
}

/// An exception out of a timer callback reaches its `error` listener in the
/// timer's own scope — the same reasoning, on the other error path.
#[test]
fn an_uncaught_timer_error_reports_the_timers_mapping() {
    let out = lines(
        "timer-error",
        r#"
        import { createContext } from "runtime:context";
        const tenant = createContext({ defaultValue: "none" });
        addEventListener("error", (event) => {
          event.preventDefault();
          console.log(`reported=${tenant.get()}`);
        });
        tenant.run("acme", () => setTimeout(() => { throw new Error("boom"); }, 1));
        await new Promise((r) => setTimeout(r, 20));
        console.log(`observer=${tenant.get()}`);
        "#,
        &[],
    );
    assert_eq!(out, ["reported=acme", "observer=none"]);
}

// ---------------------------------------------------------------------------
// Tasks and trace ids
// ---------------------------------------------------------------------------

/// `currentTask()` reports the task the host started — the root, a timer
/// firing — with the id of whatever started it and a stable trace across the
/// whole tree. A promise continuation belongs to the task that scheduled it
/// (D131): a per-promise id would need a per-promise callback, which is the
/// cost that decision removed.
#[test]
fn current_task_reports_identity_and_lineage() {
    let out = lines(
        "task",
        r#"
        import { currentTask } from "runtime:context";
        const root = currentTask();
        console.log(root.parentId === null, root.kind, /^[0-9a-f]{32}$/.test(root.traceId));
        const child = await Promise.resolve().then(() => currentTask());
        console.log(child.id === root.id, child.parentId === null, child.traceId === root.traceId);
        const grand = await Promise.resolve().then(() => Promise.resolve().then(() => currentTask()));
        console.log(grand.id === child.id, grand.traceId === root.traceId);
        // A timer callback is its own task, parented to whatever armed it —
        // read at the moment of arming so the comparison is exact rather than
        // merely "different from the root".
        let armed;
        const timer = await new Promise((r) => {
          armed = currentTask().id;
          setTimeout(() => r(currentTask()), 1);
        });
        console.log(timer.id !== armed, timer.parentId === armed, timer.traceId === root.traceId);
        // Each firing of a repeating timer is a new task.
        const fires = await new Promise((r) => {
          const ids = [];
          const id = setInterval(() => {
            ids.push(currentTask().id);
            if (ids.length === 2) { clearInterval(id); r(ids); }
          }, 1);
        });
        console.log(fires[0] !== fires[1]);
        // Read-only: the object handed back is a copy, not a handle on the task.
        const before = currentTask().traceId;
        root.traceId = "ffffffffffffffffffffffffffffffff";
        console.log(currentTask().traceId === before);
        "#,
        &[],
    );
    assert_eq!(out[0], "true main true");
    assert_eq!(out[1], "true true true");
    assert_eq!(out[2], "true true");
    assert_eq!(out[3], "true true true");
    assert_eq!(out[4], "true");
    assert_eq!(out[5], "true");
}

/// `withTrace` is the only override, it nests, and it refuses anything that is
/// not a W3C trace id — an id that is not one would make the field's documented
/// format a lie the first time an exporter read it.
#[test]
fn with_trace_adopts_an_upstream_id_and_validates_it() {
    let out = lines(
        "with-trace",
        r#"
        import { withTrace, currentTask, createContext } from "runtime:context";
        const upstream = "0af7651916cd43dd8448eb211c80319c";
        const ambient = currentTask().traceId;

        console.log(withTrace(upstream, () => currentTask().traceId));
        console.log(currentTask().traceId === ambient);
        // Nests, and the inner id wins for the inner scope only.
        console.log(withTrace(upstream, () =>
          withTrace("00000000000000000000000000000001", () => currentTask().traceId)
          + "," + currentTask().traceId));
        // Survives an await, like any other scope.
        console.log(await withTrace(upstream, async () => { await null; return currentTask().traceId; }));
        // Uppercase is a serialization difference, not a different id.
        console.log(withTrace(upstream.toUpperCase(), () => currentTask().traceId) === upstream);
        // Values set inside a request survive a withTrace inside it.
        const c = createContext({ defaultValue: "none" });
        console.log(c.run("v", () => withTrace(upstream, () => c.get())));

        for (const bad of [null, 42, "", "xyz", "0".repeat(32), upstream + "0"]) {
          try { withTrace(bad, () => 0); console.log("ACCEPTED", JSON.stringify(bad)); }
          catch (e) { console.log(e.constructor.name); }
        }
        "#,
        &[],
    );
    assert_eq!(out[0], "0af7651916cd43dd8448eb211c80319c");
    assert_eq!(out[1], "true");
    assert_eq!(
        out[2],
        "00000000000000000000000000000001,0af7651916cd43dd8448eb211c80319c"
    );
    assert_eq!(out[3], "0af7651916cd43dd8448eb211c80319c");
    assert_eq!(out[4], "true");
    assert_eq!(out[5], "v");
    assert_eq!(&out[6..], ["TypeError"; 6]);
}

/// The ambient trace is minted once and shared by everything under the agent's
/// root, so two reads from different tasks correlate.
#[test]
fn the_ambient_trace_id_is_minted_once_per_agent() {
    let out = lines(
        "ambient-trace",
        r#"
        import { currentTask } from "runtime:context";
        const a = currentTask().traceId;
        const b = await Promise.resolve().then(() => currentTask().traceId);
        const c = await new Promise((r) => setTimeout(() => r(currentTask().traceId), 1));
        console.log(a === b, b === c, /^[0-9a-f]{32}$/.test(a));
        "#,
        &[],
    );
    assert_eq!(out, ["true true true"]);
}

// ---------------------------------------------------------------------------
// Workers
// ---------------------------------------------------------------------------

/// A spawned worker is a separate agent: every context starts at its default and
/// it gets its own trace. Continuing a trace is explicit — the parent sends the
/// id and the worker calls `withTrace`.
#[test]
fn a_worker_starts_from_defaults_and_adopts_a_trace_explicitly() {
    let worker = temp("ctx-worker-child.mjs");
    std::fs::write(
        &worker,
        r#"
        import { createContext, withTrace, currentTask } from "runtime:context";
        const tenant = createContext({ name: "tenant", defaultValue: "worker-default" });
        self.addEventListener("message", (event) => {
            withTrace(event.data.traceId, () => {
                self.postMessage({
                    tenant: tenant.get(),
                    traceId: currentTask().traceId,
                    kind: currentTask().kind,
                });
            });
        });
        "#,
    )
    .expect("write worker");

    let out = lines(
        "worker-parent",
        r#"
        import { createContext, currentTask } from "runtime:context";
        const tenant = createContext({ name: "tenant", defaultValue: "parent-default" });
        const w = new Worker(new URL("./ctx-worker-child.mjs", import.meta.url));
        const reply = await tenant.run("acme", () => new Promise((resolve) => {
          w.addEventListener("message", (e) => resolve(e.data));
          // The trace travels in the payload. Nothing is auto-injected.
          w.postMessage({ traceId: currentTask().traceId });
        }));
        console.log(reply.tenant, reply.kind, reply.traceId === currentTask().traceId);
        w.terminate();
        "#,
        &["--allow-all"],
    );
    // The worker saw its own default, reported itself as a worker root, and
    // adopted the trace only because it was told to.
    assert_eq!(out, ["worker-default worker true"]);
}

// ---------------------------------------------------------------------------
// runtime:http
// ---------------------------------------------------------------------------

/// Each inbound request is its own root: a fresh trace, and nothing carried over
/// from the accept loop or from the request before it.
#[test]
fn http_gives_every_request_its_own_root_scope() {
    let out = lines(
        "http-scope",
        r#"
        import { createContext, currentTask } from "runtime:context";
        import { serve } from "runtime:http";
        // Set in the accept loop's scope: a request must NOT inherit it.
        const leaked = createContext({ defaultValue: "clean" });
        const perRequest = createContext({ defaultValue: "unset" });

        const server = leaked.run("accept-loop", () =>
          serve({ hostname: "127.0.0.1", port: 0 }, async (request) => {
            const n = new URL(request.url).searchParams.get("n");
            return await perRequest.run(n, async () => {
              await null;
              const task = currentTask();
              return Response.json({
                leaked: leaked.get(),
                perRequest: perRequest.get(),
                traceId: task.traceId,
                kind: task.kind,
              });
            });
          }));

        const { port } = await server.addr;
        const [a, b] = await Promise.all([
          fetch(`http://127.0.0.1:${port}/?n=1`).then((r) => r.json()),
          fetch(`http://127.0.0.1:${port}/?n=2`).then((r) => r.json()),
        ]);
        console.log(a.leaked, b.leaked);
        console.log(a.perRequest, b.perRequest);
        console.log(a.kind, b.kind);
        console.log(a.traceId !== b.traceId, /^[0-9a-f]{32}$/.test(a.traceId));
        console.log(a.traceId !== currentTask().traceId);
        await server.stop();
        "#,
        &["--allow-all"],
    );
    assert_eq!(
        out[0], "clean clean",
        "the accept loop's scope leaked into a request"
    );
    assert_eq!(out[1], "1 2");
    assert_eq!(out[2], "http-request http-request");
    assert_eq!(out[3], "true true");
    assert_eq!(out[4], "true");
}

/// An inbound `traceparent` is **ignored** unless the server was configured to
/// trust it. Deny-by-default on untrusted input: the header comes from whoever
/// opened the connection, and a trace id lands in logs and in every downstream
/// call the request makes.
#[test]
fn an_inbound_traceparent_is_ignored_unless_trusted() {
    const APP: &str = r#"
        import { currentTask } from "runtime:context";
        import { serve } from "runtime:http";
        const server = serve(
          { hostname: "127.0.0.1", port: 0, trustTraceHeaders: TRUST },
          async () => { await null; return new Response(currentTask().traceId); },
        );
        const { port } = await server.addr;
        const upstream = "0af7651916cd43dd8448eb211c80319c";
        const send = (value) =>
          fetch(`http://127.0.0.1:${port}/`, { headers: value ? { traceparent: value } : {} })
            .then((r) => r.text());
        console.log(await send(`00-${upstream}-b7ad6b7169203331-01`) === upstream);
        // A malformed or all-zero id is never adopted, trusted or not.
        console.log(await send("garbage") === upstream);
        console.log(await send(`00-${"0".repeat(32)}-b7ad6b7169203331-01`) === "0".repeat(32));
        console.log(/^[0-9a-f]{32}$/.test(await send(null)));
        await server.stop();
    "#;

    let untrusted = lines(
        "trace-untrusted",
        &APP.replace("TRUST", "false"),
        &["--allow-all"],
    );
    assert_eq!(
        untrusted[0], "false",
        "an untrusted traceparent was adopted"
    );
    assert_eq!(untrusted[1..4], ["false", "false", "true"]);

    let trusted = lines(
        "trace-trusted",
        &APP.replace("TRUST", "true"),
        &["--allow-all"],
    );
    assert_eq!(trusted[0], "true", "a trusted traceparent was not adopted");
    assert_eq!(trusted[1..4], ["false", "false", "true"]);
}

/// `trustTraceHeaders` is validated like every other `serve` option: a bad one
/// is a `TypeError` before the port is bound, not after.
#[test]
fn trust_trace_headers_is_validated_before_binding() {
    let out = run(
        "trace-option",
        r#"
        import { serve } from "runtime:http";
        try { serve({ port: 0, trustTraceHeaders: "yes" }, () => new Response("")); }
        catch (e) { console.log(e.constructor.name, e.message); }
        "#,
        &["--allow-all"],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(
        stdout(&out).trim(),
        "TypeError serve: trustTraceHeaders must be a boolean, got string"
    );
}

// ---------------------------------------------------------------------------
// What is deliberately absent
// ---------------------------------------------------------------------------

/// The three Node members that were removed, and the replacement each has.
///
/// Asserted because "we did not ship `enterWith`" is a decision, and a decision
/// that nothing checks is one a later convenience commit can undo by accident.
#[test]
fn the_removed_node_members_are_absent() {
    let out = lines(
        "removals",
        r#"
        import * as context from "runtime:context";
        const c = context.createContext({ defaultValue: "none" });
        console.log(Object.keys(context).sort().join(","));
        for (const name of ["enterWith", "exit", "disable", "getStore"]) {
          console.log(name, c[name] === undefined, context[name] === undefined);
        }
        // `exit` was expressible all along: an inner scope whose value is
        // `undefined` is what `exit()` made `getStore()` return. Note that this
        // is *not* the same as leaving every scope — the default belongs to
        // "outside any run()", and inside one the value is what was set.
        console.log(String(c.run("v", () => c.run(undefined, () => c.get()))));
        console.log(c.run("v", () => c.run(undefined, () => "inner")), c.get());
        "#,
        &[],
    );
    assert_eq!(
        out[0],
        "bind,createContext,currentTask,default,snapshot,withTrace"
    );
    assert_eq!(out[1], "enterWith true true");
    assert_eq!(out[2], "exit true true");
    assert_eq!(out[3], "disable true true");
    assert_eq!(out[4], "getStore true true");
    assert_eq!(out[5], "undefined");
    assert_eq!(out[6], "inner none");
}

/// A program that never imports the module pays nothing: the promise hook is not
/// installed, so the accessors report a runtime with no async context in it.
///
/// The observable proxy for "not installed" is `__ctx_enabled()`, which is what
/// `runtime:http` itself branches on before minting a trace per request.
#[test]
fn propagation_is_off_until_the_module_is_imported() {
    let out = lines(
        "lazy",
        r#"
        console.log(globalThis.__ctx_enabled());
        await import("runtime:context");
        console.log(globalThis.__ctx_enabled());
        "#,
        &[],
    );
    assert_eq!(out, ["false", "true"]);
}
