//! End-to-end tests for `runtime:db` (DECISIONS.md D56).
//!
//! These drive the real `esrun` binary, so each one exercises the whole path:
//! the JS module, the parameter buffer, the op boundary and its capability
//! gates, the jailed VFS, and the engine writing real files.

use std::path::PathBuf;
use std::process::{Command, Output};

fn dir(name: &str) -> PathBuf {
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("db-{name}"));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("create temp dir");
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

/// Runs `source` in its own directory, so each test gets its own database and
/// its own jail.
fn run(name: &str, source: &str, flags: &[&str]) -> Output {
    let base = dir(name);
    let app = base.join("app.mjs");
    std::fs::write(&app, source).expect("write module");
    esrun()
        .current_dir(&base)
        .args(flags)
        .arg("app.mjs")
        .output()
        .unwrap()
}

#[test]
fn a_database_round_trips_every_value_kind() {
    let out = run(
        "values",
        r#"
        import { connect, sqlite } from "runtime:db";
        const db = await connect("sqlite:./app.db", { driver: sqlite });
        await db.execute("CREATE TABLE t (i INTEGER, r REAL, s TEXT, b BLOB, n INTEGER)");
        await db.execute("INSERT INTO t VALUES (?, ?, ?, ?, ?)",
          [7, 1.5, "hi", new Uint8Array([1, 2]), null]);
        const row = await (await db.query("SELECT i, r, s, b, n FROM t")).first();
        console.log(row.i, row.r, row.s, [...row.b].join("-"), row.n);
        console.log(JSON.stringify(row));
        await db.close();
        "#,
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let lines: Vec<_> = stdout(&out).lines().map(str::to_string).collect();
    assert_eq!(lines[0], "7 1.5 hi 1-2 null");
    // Columns are enumerable, so a row serializes without a conversion step.
    assert_eq!(
        lines[1],
        r#"{"i":7,"r":1.5,"s":"hi","b":{"0":1,"1":2},"n":null}"#
    );
}

/// A 64-bit id does not fit a JS number. It crosses as eight bytes in both
/// directions and comes back as a bigint rather than as a rounded double —
/// which is the whole reason parameters are a buffer and not a value array.
#[test]
fn an_integer_too_large_for_a_number_survives_the_round_trip() {
    let out = run(
        "bigint",
        r#"
        import { connect, sqlite } from "runtime:db";
        const db = await connect("sqlite:./app.db", { driver: sqlite });
        await db.execute("CREATE TABLE t (id INTEGER PRIMARY KEY)");
        await db.execute("INSERT INTO t VALUES (?)", [9007199254740993n]);
        const row = await (await db.query("SELECT id FROM t")).first();
        console.log(typeof row.id, row.id.toString());
        await db.close();
        "#,
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "bigint 9007199254740993");
}

#[test]
fn the_sql_tag_binds_every_interpolation_as_a_parameter() {
    let out = run(
        "sql-tag",
        r#"
        import { connect, sqlite, sql } from "runtime:db";
        const db = await connect("sqlite:./app.db", { driver: sqlite });
        await db.execute("CREATE TABLE t (name TEXT)");
        // The classic injection: as a parameter it is a name, not syntax.
        const hostile = "'); DROP TABLE t; --";
        await db.execute(sql`INSERT INTO t VALUES (${hostile})`);
        const row = await (await db.query(sql`SELECT name FROM t WHERE name = ${hostile}`)).first();
        console.log(row.name === hostile, (await (await db.query("SELECT count(*) AS n FROM t")).first()).n);
        await db.close();
        "#,
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "true 1");
}

#[test]
fn parameters_bind_by_position_and_by_name() {
    let out = run(
        "params",
        r#"
        import { connect, sqlite } from "runtime:db";
        const db = await connect("sqlite:./app.db", { driver: sqlite });
        await db.execute("CREATE TABLE t (a INTEGER, b TEXT)");
        await db.execute("INSERT INTO t VALUES (:a, :b)", { a: 1, b: "one" });
        await db.execute("INSERT INTO t VALUES (?, ?)", [2, "two"]);
        for await (const row of await db.query("SELECT b FROM t ORDER BY a")) console.log(row.b);
        await db.close();
        "#,
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "one\ntwo");
}

/// The result set is pulled a batch at a time, so a table far larger than a
/// batch streams through at the cost of one — and stopping early is ordinary
/// code, not an error.
#[test]
fn a_result_set_streams_and_can_be_abandoned() {
    let out = run(
        "streaming",
        r#"
        import { connect, sqlite } from "runtime:db";
        const db = await connect("sqlite:./app.db", { driver: sqlite });
        await db.execute("CREATE TABLE t (a INTEGER, pad TEXT)");
        await db.transaction(async (tx) => {
          for (let i = 0; i < 5000; i++) {
            await tx.execute("INSERT INTO t VALUES (?, ?)", [i, "x".repeat(200)]);
          }
        });
        let all = 0;
        for await (const _ of await db.query("SELECT a, pad FROM t")) all++;
        let some = 0;
        for await (const _ of await db.query("SELECT a, pad FROM t")) { if (++some === 3) break; }
        // The abandoned cursor is closed on the way out, so the connection is
        // still usable for the next statement.
        const after = await (await db.query("SELECT count(*) AS n FROM t")).first();
        console.log(all, some, after.n);
        await db.close();
        "#,
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "5000 3 5000");
}

#[test]
fn a_transaction_commits_or_rolls_back_and_nests_by_savepoint() {
    let out = run(
        "transactions",
        r#"
        import { connect, sqlite } from "runtime:db";
        const db = await connect("sqlite:./app.db", { driver: sqlite });
        await db.execute("CREATE TABLE t (a INTEGER)");
        await db.transaction(async (tx) => { await tx.execute("INSERT INTO t VALUES (1)"); });
        try {
          await db.transaction(async (tx) => {
            await tx.execute("INSERT INTO t VALUES (2)");
            throw new Error("no");
          });
        } catch {}
        // A nested transaction becomes a savepoint: the inner one rolls back
        // without taking the outer one with it.
        await db.transaction(async (tx) => {
          await tx.execute("INSERT INTO t VALUES (3)");
          try {
            await tx.transaction(async (inner) => {
              await inner.execute("INSERT INTO t VALUES (4)");
              throw new Error("no");
            });
          } catch {}
        });
        const rows = await (await db.query("SELECT a FROM t ORDER BY a")).toArray();
        console.log(rows.map((r) => r.a).join(","));
        await db.close();
        "#,
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "1,3");
}

/// A denied capability stays a denied capability. Classifying it as a database
/// error would mean an application testing `ERR_CAPABILITY_DENIED` had to know
/// that this particular call went through a database.
#[test]
fn opening_a_database_is_gated_like_a_file() {
    let source = r#"
        import { connect, sqlite } from "runtime:db";
        const report = async (label, fn) => {
          try { await fn(); console.log(`${label}: ok`); }
          catch (e) { console.log(`${label}: ${e.code}`); }
        };
        await report("write", async () => {
          const db = await connect("sqlite:./app.db", { driver: sqlite });
          await db.execute("CREATE TABLE IF NOT EXISTS t (a INTEGER)");
          await db.close();
        });
        await report("read", async () => {
          const db = await connect("sqlite:./app.db", { driver: sqlite, readOnly: true });
          await (await db.query("SELECT count(*) AS n FROM t")).first();
          await db.close();
        });
    "#;

    let granted = run("caps-granted", source, &[]);
    assert!(granted.status.success(), "stderr: {}", stderr(&granted));
    assert_eq!(stdout(&granted).trim(), "write: ok\nread: ok");

    // The database exists now; a read-only grant may open it and may not write.
    let base = dir("caps-read");
    std::fs::write(base.join("app.mjs"), source).unwrap();
    let seed = esrun().current_dir(&base).arg("app.mjs").output().unwrap();
    assert!(seed.status.success(), "stderr: {}", stderr(&seed));
    let read_only = esrun()
        .current_dir(&base)
        .args(["--deny-all", "--allow-read"])
        .arg("app.mjs")
        .output()
        .unwrap();
    assert_eq!(
        stdout(&read_only).trim(),
        "write: ERR_CAPABILITY_DENIED\nread: ok"
    );

    let nothing = esrun()
        .current_dir(&base)
        .arg("--deny-all")
        .arg("app.mjs")
        .output()
        .unwrap();
    assert_eq!(
        stdout(&nothing).trim(),
        "write: ERR_CAPABILITY_DENIED\nread: ERR_CAPABILITY_DENIED"
    );
}

/// A crossing costs about the same whatever it carries, so a loop that crosses
/// once per row spends its time on the boundary rather than in the database.
/// `executeMany` crosses once and prepares once.
#[test]
fn execute_many_crosses_once_and_is_all_or_nothing() {
    let out = run(
        "execute-many",
        r#"
        import { connect, sqlite, sql, DbErrorCode } from "runtime:db";
        const db = await connect("sqlite::memory:", { driver: sqlite });
        await db.execute("CREATE TABLE t (a INTEGER PRIMARY KEY, b TEXT)");

        const rows = [];
        for (let i = 0; i < 5000; i++) rows.push([i, `row-${i}`]);
        const result = await db.executeMany("INSERT INTO t VALUES (?, ?)", rows);
        console.log("changes:", result.changes);
        console.log("count:", (await (await db.query("SELECT count(*) AS n FROM t")).first()).n);

        // All or nothing: the batch runs in its own transaction, so a set that
        // fails part-way leaves none of it behind.
        try {
          await db.executeMany("INSERT INTO t VALUES (?, ?)", [[9998, "ok"], [1, "clash"]]);
        } catch (e) {
          console.log("clash:", e.code === DbErrorCode.UniqueViolation);
        }
        console.log("after:", (await (await db.query("SELECT count(*) AS n FROM t")).first()).n);

        // Inside a transaction it joins that one rather than opening a second.
        await db.transaction(async (tx) => {
          await tx.executeMany("INSERT INTO t VALUES (?, ?)", [[10001, "a"], [10002, "b"]]);
        });
        console.log("joined:", (await (await db.query("SELECT count(*) AS n FROM t")).first()).n);

        // A template with values describes one row, so it is refused rather
        // than silently running the first row's values for every row.
        try {
          await db.executeMany(sql`INSERT INTO t VALUES (${1}, ${"x"})`, [[1, "x"]]);
        } catch (e) {
          console.log("template:", e.code === DbErrorCode.Unsupported);
        }
        await db.close();
        "#,
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(
        stdout(&out).trim(),
        "changes: 5000
count: 5000
clash: true
after: 5000
joined: 5002
template: true"
    );
}

/// A result that fits one batch comes back with the query itself — no cursor is
/// minted, so there is nothing to fetch and nothing to close. `exhausted` is
/// how a caller (or a pool) can tell.
#[test]
fn a_small_result_costs_one_crossing() {
    let out = run(
        "one-crossing",
        r#"
        import { connect, sqlite } from "runtime:db";
        const db = await connect("sqlite::memory:", { driver: sqlite });
        await db.execute("CREATE TABLE t (a INTEGER, pad TEXT)");
        const rows = [];
        for (let i = 0; i < 4000; i++) rows.push([i, "x".repeat(200)]);
        await db.executeMany("INSERT INTO t VALUES (?, ?)", rows);

        const small = await db.query("SELECT a FROM t WHERE a = 1");
        console.log("small exhausted:", small.exhausted);
        console.log("small rows:", (await small.toArray()).length);

        // Far more than one batch of 64 KiB, so this one keeps a cursor.
        const big = await db.query("SELECT a, pad FROM t");
        console.log("big exhausted:", big.exhausted);
        console.log("big rows:", (await big.toArray()).length);
        await db.close();
        "#,
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(
        stdout(&out).trim(),
        "small exhausted: true
small rows: 1
big exhausted: false
big rows: 4000"
    );
}

/// `sqlite:` can genuinely interrupt a running statement, so `{ signal }` means
/// the same thing there as on a networked backend: the work stops, and the
/// connection survives it.
#[test]
fn a_signal_cancels_a_running_statement_and_leaves_the_connection_usable() {
    let out = run(
        "signal",
        r#"
        import { connect, sqlite } from "runtime:db";
        const db = await connect("sqlite::memory:", { driver: sqlite });
        await db.execute("CREATE TABLE t (a INTEGER)");
        const rows = [];
        for (let i = 0; i < 40000; i++) rows.push([i]);
        await db.executeMany("INSERT INTO t VALUES (?)", rows);

        // A cross join over 40k rows is long enough to be interrupted rather
        // than merely finished before the signal is noticed.
        const controller = new AbortController();
        setTimeout(() => controller.abort(new Error("enough")), 200);
        const started = performance.now();
        let outcome = "completed";
        try {
          await db.query(
            "SELECT count(*) AS n FROM t a, t b WHERE a.a < b.a",
            [],
            { signal: controller.signal },
          ).then((r) => r.first());
        } catch (e) {
          outcome = e.message;
        }
        const elapsed = performance.now() - started;
        console.log("aborted:", outcome === "enough");
        console.log("promptly:", elapsed < 5000);
        // The whole point of cancelling rather than hanging up.
        console.log("usable:", (await (await db.query("SELECT count(*) AS n FROM t")).first()).n);
        await db.close();
        "#,
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(
        stdout(&out).trim(),
        "aborted: true
promptly: true
usable: 40000"
    );
}

/// The pool is protocol-blind: it knows how to make a thing and how to destroy
/// one, and it cannot know whether a returned one is fit to reuse. So the driver
/// says, and anything not explicitly clean is thrown away — which is the rule
/// that stops an aborted transaction leaking from one request into the next.
#[test]
fn a_pool_reuses_what_is_clean_and_discards_everything_else() {
    let out = run(
        "pool",
        r#"
        import { Pool, DbErrorCode } from "runtime:db";

        let made = 0;
        const destroyed = [];
        const pool = new Pool({
          create: async () => ({ id: ++made }),
          destroy: (r) => destroyed.push(r.id),
          max: 2,
          acquireTimeout: 200,
        });

        // Clean goes back to the pool; the next caller gets that very one.
        const a = await pool.acquire();
        pool.release(a, { clean: true });
        const again = await pool.acquire();
        console.log("reused:", again.id === a.id, "| made:", made);

        // Not clean is destroyed, and the caller after that gets a new one.
        pool.release(again, { clean: false });
        console.log("destroyed:", destroyed.join(","));
        const fresh = await pool.acquire();
        console.log("fresh:", fresh.id !== a.id);

        // The default is destroy: when nobody said it was clean, it is not.
        pool.release(fresh);
        console.log("default discards:", destroyed.length === 2);

        // A full pool queues, and a release lets the queued caller through.
        const one = await pool.acquire();
        const two = await pool.acquire();
        let third = "waiting";
        const queued = pool.acquire().then((r) => { third = "got it"; return r; });
        console.log("pending:", pool.pending, "| size:", pool.size);
        pool.release(one, { clean: true });
        const got = await queued;
        console.log("queued through:", third);

        // A pool that stays full refuses rather than waiting forever. `two` is
        // still borrowed, so taking the one just released fills it again.
        pool.release(got, { clean: true });
        const hold = await pool.acquire();
        try {
          await pool.acquire();
          console.log("timeout: none (should not happen)");
        } catch (e) {
          console.log("timeout:", e.code === DbErrorCode.Timeout);
        }

        // Closing refuses everyone still waiting rather than leaving them parked.
        const parked = pool.acquire().then(() => "resolved", (e) => e.code);
        await pool.close();
        console.log("closed waiter:", await parked === DbErrorCode.Closed);
        void two; void hold;
        "#,
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(
        stdout(&out).trim(),
        "reused: true | made: 1
destroyed: 1
fresh: true
default discards: true
pending: 1 | size: 2
queued through: got it
timeout: true
closed waiter: true"
    );
}

/// `pool: true` is a property of the call rather than a different object
/// reached a different way, and what comes back answers the same surface one
/// connection does.
#[test]
fn pooling_is_an_option_on_connect() {
    let out = run(
        "pooled",
        r#"
        import { connect, sqlite, DbErrorCode } from "runtime:db";

        const pool = await connect("sqlite:./app.db", { driver: sqlite, pool: { max: 2 } });
        console.log("empty until used:", pool.size === 0);
        await pool.execute("CREATE TABLE t (n INTEGER)");
        await pool.executeMany("INSERT INTO t VALUES (?)", [[1], [2], [3]]);
        console.log("rows:", (await (await pool.query("SELECT count(*) AS n FROM t")).first()).n);
        console.log("one connection, returned:", pool.size === 1 && pool.idle === 1);
        console.log("same surface:", pool.backend, pool.dialect.name);

        // A transaction holds one connection for the whole of it.
        await pool.transaction(async (tx) => { await tx.execute("INSERT INTO t VALUES (4)"); });
        console.log("after commit:", (await (await pool.query("SELECT count(*) AS n FROM t")).first()).n);
        await pool.close();

        // Every `sqlite::memory:` open is its own database, so a pool of them
        // would be a pool of unrelated databases. Refused by name.
        try {
          await connect("sqlite::memory:", { driver: sqlite, pool: true });
          console.log("memory pool: allowed");
        } catch (e) {
          console.log("memory pool:", e.code === DbErrorCode.Unsupported);
        }
        "#,
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(
        stdout(&out).trim(),
        "empty until used: true
rows: 3
one connection, returned: true
same surface: sqlite sqlite
after commit: 4
memory pool: true"
    );
}

/// A resource the driver rejects on the way out is not handed to the next
/// caller, and a create that fails must still wake whoever was queued behind
/// the slot it freed — otherwise a pool whose connections all fail parks every
/// caller forever.
#[test]
fn a_pool_validates_on_the_way_out_and_recovers_from_a_failed_create() {
    let out = run(
        "pool-validate",
        r#"
        import { Pool } from "runtime:db";

        let made = 0;
        const pool = new Pool({
          create: async () => ({ id: ++made, ok: true }),
          destroy: () => {},
          validate: (r) => r.ok,
          max: 4,
        });

        const a = await pool.acquire();
        a.ok = false;                      // it went bad while borrowed
        pool.release(a, { clean: true });  // the driver still thought it fine
        const b = await pool.acquire();
        console.log("stale one not reused:", b.id !== a.id);
        pool.release(b, { clean: true });

        let failing = 0;
        const broken = new Pool({
          create: async () => {
            failing++;
            throw new Error("refused");
          },
          destroy: () => {},
          max: 1,
          acquireTimeout: 500,
        });
        const first = broken.acquire().then(() => "ok", () => "failed");
        const second = broken.acquire().then(() => "ok", () => "failed");
        console.log("both fail:", (await first) === "failed", (await second) === "failed");
        console.log("attempts:", failing >= 2);
        await pool.close();
        await broken.close();
        "#,
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(
        stdout(&out).trim(),
        "stale one not reused: true
both fail: true true
attempts: true"
    );
}

/// The suite a third-party driver runs to show it behaves like the built-ins.
/// The built-ins run it too — a conformance suite the reference backend does
/// not pass is a description of nothing.
#[test]
fn the_built_in_backend_passes_its_own_conformance_suite() {
    for (name, url, flags) in [
        ("conformance-file", "sqlite:./app.db", &[][..]),
        ("conformance-memory", "sqlite::memory:", &["--deny-all"][..]),
    ] {
        let base = dir(name);
        std::fs::write(
            base.join("app.mjs"),
            format!(
                r#"
                import {{ connect, sqlite, runBackendConformance }} from "runtime:db";
                const report = await runBackendConformance(() => connect("{url}", {{ driver: sqlite }}));
                for (const f of report.failures) console.log(`FAIL ${{f.name}}: ${{f.error}}`);
                console.log(`ok=${{report.ok}} passed=${{report.passed}} skipped=${{report.skipped}}`);
                "#
            ),
        )
        .unwrap();
        let out = esrun()
            .current_dir(&base)
            .args(flags)
            .arg("app.mjs")
            .output()
            .unwrap();
        assert!(out.status.success(), "{name} stderr: {}", stderr(&out));
        assert_eq!(
            stdout(&out).trim(),
            "ok=true passed=16 skipped=0",
            "{name} did not pass its own suite"
        );
    }
}

/// A row is a lazy view over its batch. That buys a query which selects more
/// columns than it reads, and it costs the spread shorthand — so the explicit
/// spelling has to work, and spreading must not reach the batch buffer.
#[test]
fn a_row_materializes_explicitly_and_leaks_nothing() {
    let out = run(
        "row-shape",
        r#"
        import { connect, sqlite } from "runtime:db";
        const db = await connect("sqlite::memory:", { driver: sqlite });
        await db.execute("CREATE TABLE t (a INTEGER, b TEXT)");
        await db.execute("INSERT INTO t VALUES (1, 'x')");
        const row = await (await db.query("SELECT a, b FROM t")).first();
        console.log(JSON.stringify(row.toObject()));
        console.log(JSON.stringify(row));
        console.log(row.values().join(","));
        const spread = { ...row };
        console.log(Object.keys(spread).length, Object.getOwnPropertySymbols(spread).length);
        await db.close();
        "#,
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(
        stdout(&out).trim(),
        "{\"a\":1,\"b\":\"x\"}\n{\"a\":1,\"b\":\"x\"}\n1,x\n0 0"
    );
}

/// An in-memory database reaches no filesystem, so it needs no filesystem
/// grant — it is the one open that works under `--deny-all`. Which is also the
/// reason its op takes no path: an ungated op that accepted one would be a way
/// to open any database on disk without `FileRead`.
#[test]
fn an_in_memory_database_needs_no_capability_and_leaves_no_files() {
    let base = dir("memory");
    std::fs::write(
        base.join("app.mjs"),
        r#"
        import { connect, sqlite, DbErrorCode } from "runtime:db";
        const db = await connect("sqlite::memory:", { driver: sqlite });
        await db.execute("CREATE TABLE t (a INTEGER)");
        await db.execute("INSERT INTO t VALUES (1)");
        console.log("rows:", (await (await db.query("SELECT count(*) AS n FROM t")).first()).n);

        // Each connection gets its own, so nothing is shared by name — which is
        // why the named spelling is refused rather than quietly not sharing.
        const other = await connect("sqlite::memory:", { driver: sqlite });
        try {
          await other.query("SELECT a FROM t");
          console.log("shared: yes");
        } catch (e) {
          console.log("independent:", e.code === DbErrorCode.UndefinedTable);
        }
        await db.close();
        await other.close();

        try {
          await connect("sqlite::memory:named", { driver: sqlite });
        } catch (e) {
          console.log("named:", e.code === DbErrorCode.Unsupported);
        }
        "#,
    )
    .unwrap();
    let out = esrun()
        .current_dir(&base)
        .arg("--deny-all")
        .arg("app.mjs")
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(
        stdout(&out).trim(),
        "rows: 1\nindependent: true\nnamed: true"
    );

    // The engine picks its storage from the IO it is handed, not from the path,
    // so getting this wrong writes a *file* called `:memory:` and reports the
    // database as in-memory anyway. Nothing but the module should be here.
    let left: Vec<_> = std::fs::read_dir(&base)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(left, ["app.mjs"], "an in-memory database wrote files");
}

/// The jail is the same one `runtime:fs` uses, so a path outside it is refused
/// for a database exactly as it is for a file — and it says *jail escape*
/// rather than the blunter "capability denied", because the grant was held and
/// the path was the problem.
#[test]
fn a_database_outside_the_scope_is_refused() {
    let out = run(
        "scope",
        r#"
        import { connect, sqlite } from "runtime:db";
        try {
          await connect("sqlite:/etc/passwd.db", { driver: sqlite });
          console.log("opened");
        } catch (e) {
          console.log(e.code);
        }
        "#,
        &["--deny-all", "--allow-read=.", "--allow-write=."],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "ERR_JAIL_ESCAPE");
}

/// Keys in URLs end up in logs, error messages and stack traces. The refusal is
/// the point: honouring it quietly is how a key gets into all three.
#[test]
fn a_key_in_the_connection_string_is_refused() {
    let out = run(
        "url-key",
        r#"
        import { connect, sqlite, DbErrorCode } from "runtime:db";
        try {
          await connect("sqlite:./app.db?key=hunter2", { driver: sqlite });
        } catch (e) {
          console.log(e.code === DbErrorCode.Unsupported, /options object/.test(e.message));
        }
        "#,
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "true true");
}

#[test]
fn an_encrypted_database_needs_its_key() {
    let out = run(
        "encrypted",
        r#"
        import { connect, sqlite } from "runtime:db";
        const key = new Uint8Array(32).fill(7);
        const db = await connect("sqlite:./secret.db", { driver: sqlite, key });
        await db.execute("CREATE TABLE t (a INTEGER)");
        await db.execute("INSERT INTO t VALUES (1)");
        await db.close();

        const again = await connect("sqlite:./secret.db", { driver: sqlite, key });
        console.log("with key:", (await (await again.query("SELECT a FROM t")).first()).a);
        await again.close();

        try {
          const plain = await connect("sqlite:./secret.db", { driver: sqlite });
          await (await plain.query("SELECT a FROM t")).first();
          console.log("without key: opened");
        } catch (e) {
          console.log("without key:", e.code !== undefined);
        }
        "#,
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "with key: 1\nwithout key: true");
}

#[test]
fn a_backend_maps_its_errors_onto_the_portable_codes() {
    let out = run(
        "errors",
        r#"
        import { connect, sqlite, DbErrorCode } from "runtime:db";
        const db = await connect("sqlite:./app.db", { driver: sqlite });
        await db.execute("CREATE TABLE t (a INTEGER PRIMARY KEY, b TEXT NOT NULL)");
        await db.execute("INSERT INTO t VALUES (1, 'x')");
        const code = async (sql) => {
          try { await db.execute(sql); return "no error"; } catch (e) { return e.code; }
        };
        console.log(await code("INSERT INTO t VALUES (1, 'y')") === DbErrorCode.UniqueViolation);
        console.log(await code("INSERT INTO t VALUES (2, NULL)") === DbErrorCode.NotNullViolation);
        console.log(await code("SELECT * FROM nope") === DbErrorCode.UndefinedTable);
        await db.close();
        "#,
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "true\ntrue\ntrue");
}

/// A driver is a value, and that is the whole extension story: a third party
/// defines one and hands it to `connect`, which needs no knowledge of it and no
/// registry to look it up in.
#[test]
fn a_third_party_driver_is_just_a_value() {
    let out = run(
        "driver",
        r#"
        import { connect, sqlite, defineDriver, BaseConnection, Dialect } from "runtime:db";

        const dialect = new Dialect({ name: "toy", placeholder: (i) => `$${i}` });
        class ToyConnection extends BaseConnection {
          constructor() { super({ dialect, backend: "toy" }); }
          async _query({ text, positional }) { return { text, positional }; }
          async _execute({ text }) { return { text }; }
          async _close() {}
        }
        const toy = defineDriver({
          name: "toy",
          schemes: ["toy"],
          dialect,
          open: async () => new ToyConnection(),
        });

        const db = await connect("toy://anywhere", { driver: toy });
        // The dialect renders the placeholders, so one template targets any
        // backend: this one numbers them, sqlite writes `?`.
        const { text, positional } = await db.query(
          (await import("runtime:db")).sql`SELECT ${1} , ${2}`,
        );
        console.log(text, positional.join(","));
        console.log("backend:", db.backend, "| schemes:", toy.schemes.join(","));

        // The scheme is checked against the driver, so a URL and a driver that
        // do not belong together are caught at the call rather than inside a
        // parser that was never meant to see it.
        for (const [url, driver, label] of [
          ["postgres://x", toy, "wrong driver"],
          ["sqlite::memory:", toy, "wrong driver"],
        ]) {
          try { await connect(url, { driver }); console.log(`${label}: allowed`); }
          catch (e) { console.log(`${label}: refused (${e.code})`); }
        }
        // And a connect with no driver at all names the fix.
        try { await connect("toy://x"); }
        catch (e) { console.log("no driver:", e.message.slice(0, 20)); }
        "#,
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(
        stdout(&out).trim(),
        "SELECT $1 , $2 1,2
backend: toy | schemes: toy
wrong driver: refused (ERR_DB_UNSUPPORTED)
wrong driver: refused (ERR_DB_UNSUPPORTED)
no driver: a driver is required"
    );
}

/// The families `runtime:db` has no backend for yet — a document store, a
/// vector index, a graph — are the ones most likely to find the driver tier
/// shaped around SQL over a socket. This is one of them, written the way a
/// third party would write it: no SQL, no transactions, values that are already
/// JavaScript, a generated key that is a string, and a capability of its own to
/// declare.
///
/// It is a test rather than a paragraph because D56's own lesson was that an
/// extension point with no consumer is a comment.
#[test]
fn a_backend_that_is_not_a_sql_database_over_a_socket() {
    let out = run(
        "docdb",
        r#"
        import {
          BaseConnection, Dialect, DbError, DbErrorCode, Rows,
          connect, defineDriver, queryAst,
        } from "runtime:db";

        const dialect = new Dialect({
          name: "docdb",
          placeholder: () => {
            throw new DbError("docdb has no placeholders", { code: DbErrorCode.QueryForm });
          },
          // The three the kit acts on, and one of this backend's own — which an
          // ORM written before this backend existed can still branch on.
          supports: { sqlText: false, queryAst: true, transactions: false, vectorSearch: true },
        });

        const store = new Map();
        let seq = 0;

        class DocConnection extends BaseConnection {
          constructor() { super({ dialect, backend: "docdb" }); }

          async _query(q) {
            const op = q.ast;
            if (op.find !== undefined) {
              const docs = [...store.values()].filter((d) =>
                Object.entries(op.find).every(([k, v]) => d[k] === v));
              // The documents are already objects: they are handed over, not
              // encoded into the batch layout for a decoder to undo.
              return Rows.fromObjects(docs);
            }
            const scored = [...store.values()]
              .map((d) => ({ ...d, score: distance(d.embedding, op.nearest) }))
              .sort((a, b) => a.score - b.score)
              .slice(0, op.k ?? 3);
            return Rows.fromObjects(scored);
          }

          async _execute(q) {
            const id = `doc_${++seq}`;
            store.set(id, { id, ...q.ast.insert });
            return { changes: 1, lastInsertRowid: id };
          }

          async _close() {}
        }

        function distance(a, b) {
          let sum = 0;
          for (let i = 0; i < b.length; i++) sum += (a[i] - b[i]) ** 2;
          return Math.sqrt(sum);
        }

        const driver = defineDriver({
          name: "docdb", schemes: ["docdb"], dialect,
          open: async () => new DocConnection(),
        });

        const db = await connect("docdb://memory", { driver });
        const written = await db.execute(queryAst({ insert: { name: "ada", embedding: [1, 0, 0] } }));
        await db.execute(queryAst({ insert: { name: "grace", embedding: [0, 1, 0] } }));
        // A generated key that is not a rowid, which is what every backend
        // outside SQLite's family has.
        console.log("key:", written.lastInsertRowid);

        const [row] = await (await db.query(queryAst({ find: { name: "ada" } }))).toArray();
        // A nested value survives as itself. Through the byte layout it would
        // have had to be flattened or JSON-encoded on the way in and parsed on
        // the way back out.
        console.log("nested:", Array.isArray(row.embedding), JSON.stringify(row.toObject()));

        const near = await (await db.query(queryAst({ nearest: [0.9, 0.1, 0], k: 2 }))).toArray();
        console.log("nearest:", near.map((r) => r.name).join(","));

        // The columns are the union of the documents' keys, in first-seen order:
        // a document store's rows do not have to agree on a shape.
        const all = await db.query(queryAst({ find: {} }));
        console.log("columns:", all.columns.map((c) => c.name).join(","));
        await all.close();

        console.log("capability:", db.dialect.supports.vectorSearch,
          "| before connecting:", driver.dialect.supports.vectorSearch);

        // The portable surface, on a connection and on a pool alike.
        const pool = await connect("docdb://memory", { driver, pool: true });
        console.log("withConnection:", typeof db.withConnection, typeof pool.withConnection);
        console.log("usable/reusable:", db.usable, db.reusable, pool.usable, pool.reusable);
        console.log("held:", await pool.withConnection(async (c) => c.backend));
        await pool.close();

        for (const [what, attempt] of [
          ["text", () => db.query("SELECT 1")],
          ["transaction", () => db.transaction(async () => {})],
        ]) {
          try { await attempt(); console.log(`${what}: allowed`); }
          catch (e) { console.log(`${what}: refused (${e.code})`); }
        }
        await db.close();
        "#,
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(
        stdout(&out).trim(),
        "key: doc_1
nested: true {\"id\":\"doc_1\",\"name\":\"ada\",\"embedding\":[1,0,0]}
nearest: ada,grace
columns: id,name,embedding
capability: true | before connecting: true
withConnection: function function
usable/reusable: true true true true
held: docdb
text: refused (ERR_DB_QUERY_FORM)
transaction: refused (ERR_DB_UNSUPPORTED)"
    );
}

/// `executeMany` means the same thing on every backend from the day the backend
/// exists. A driver overrides the batch path to make it fast; not overriding it
/// must leave the *semantics* intact, not raise a TypeError naming a private
/// method at an application developer.
#[test]
fn a_backend_that_does_not_optimize_batching_still_supports_it() {
    let out = run(
        "batch-default",
        r#"
        import { connect, defineDriver, BaseConnection, Dialect } from "runtime:db";

        const dialect = new Dialect({ name: "toy", placeholder: (i) => `$${i}` });
        const seen = [];
        class Toy extends BaseConnection {
          constructor() { super({ dialect, backend: "toy" }); }
          async _query() { return { columns: [], async *[Symbol.asyncIterator]() {} }; }
          async _execute({ text, positional }) {
            seen.push(`${text}|${positional.join(",")}`);
            return { changes: 1, lastInsertRowid: null };
          }
          async _close() {}
        }
        const toy = defineDriver({ name: "toy", schemes: ["toy"], dialect, open: async () => new Toy() });

        const db = await connect("toy://x", { driver: toy });
        const result = await db.executeMany("INSERT INTO t VALUES ($1)", [[1], [2], [3]]);
        console.log("changes:", result.changes);
        // The default is a loop over _execute — and it is still wrapped in the
        // transaction, so the batch is atomic on a backend that never thought
        // about batching.
        console.log("calls:", seen.length);
        console.log("bracketed:", seen[0] === "BEGIN|" && seen[seen.length - 1] === "COMMIT|");
        await db.close();
        "#,
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(
        stdout(&out).trim(),
        "changes: 3
calls: 5
bracketed: true"
    );
}

/// The AST form is in the contract from the first release, so an engine that
/// never speaks SQL can be a first-class backend later. The backends that ship
/// today refuse it by name rather than by a type error somewhere downstream.
#[test]
fn a_query_ast_is_refused_by_name() {
    let out = run(
        "ast",
        r#"
        import { connect, sqlite, queryAst, DbErrorCode } from "runtime:db";
        const db = await connect("sqlite:./app.db", { driver: sqlite });
        try {
          await db.query(queryAst({ select: ["a"], from: "t" }));
        } catch (e) {
          console.log(e.code === DbErrorCode.QueryForm, /takes SQL text/.test(e.message));
        }
        await db.close();
        "#,
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "true true");
}
