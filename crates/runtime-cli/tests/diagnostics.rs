//! End-to-end tests for `runtime:diagnostics` (DECISIONS.md D89).
//!
//! Against the real binary, because what is worth asserting is that a real op on
//! a real provider produces a record with real timings — and, more importantly,
//! that a program nothing is watching never reaches JS at all. That last one is
//! the module's entire premise and it cannot be checked from inside the host.

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

fn run(name: &str, source: &str, flags: &[&str]) -> Output {
    let app = temp(&format!("diag-{name}.mjs"));
    std::fs::write(&app, source).expect("write app");
    let mut command = Command::new(env!("CARGO_BIN_EXE_esrun"));
    // The sandbox is the working directory (D79), so a program runs where it lives.
    command.current_dir(env!("CARGO_TARGET_TMPDIR"));
    command.args(flags);
    command.arg(&app);
    command.output().expect("run esrun")
}

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
// The premise: off costs nothing
// ---------------------------------------------------------------------------

/// **Nothing is subscribed, so nothing crosses into JS.**
///
/// Asserted with a counter on the binding the host would call, not by timing:
/// a timing test measures the machine, and what is being claimed here is
/// categorical — an unsubscribed event never reaches the JS engine. This is the
/// specific mistake `async_hooks` makes, so it gets the specific test.
#[test]
fn an_unsubscribed_program_never_reaches_the_delivery_hook() {
    let out = lines(
        "no-jscall",
        r#"
        import "runtime:diagnostics";
        import { write, remove, file } from "runtime:fs";

        // Count every call the host makes into the delivery hook.
        let calls = 0;
        const real = globalThis.__dispatch_diagnostics;
        globalThis.__dispatch_diagnostics = (...args) => { calls++; return real(...args); };

        // Plenty of work of every recordable kind, with nothing subscribed.
        for (let i = 0; i < 20; i++) {
          await write("./churn.txt", "x");
          await file("./churn.txt").text();
          await remove("./churn.txt");
          await new Promise((r) => setTimeout(r, 1));
        }
        console.log(`calls=${calls}`);
        "#,
        &["--allow-read", "--allow-write", "--allow-diagnostics"],
    );
    assert_eq!(
        out,
        ["calls=0"],
        "the host called into JS with no subscriber"
    );
}

/// And with a subscription, delivery is **batched per tick** rather than per
/// record — the other half of the same claim.
#[test]
fn delivery_is_one_call_per_tick_not_one_per_record() {
    let out = lines(
        "batched",
        r#"
        import { subscribe } from "runtime:diagnostics";
        import { write, remove, file } from "runtime:fs";

        let calls = 0;
        const real = globalThis.__dispatch_diagnostics;
        globalThis.__dispatch_diagnostics = (...args) => { calls++; return real(...args); };

        let records = 0;
        const sub = subscribe({ kinds: ["op"] }, (batch) => { records += batch.records.length; });
        for (let i = 0; i < 20; i++) {
          await write("./churn-batched.txt", "x");
          await file("./churn-batched.txt").text();
          await remove("./churn-batched.txt");
        }
        for (let i = 0; i < 3; i++) await new Promise((r) => setTimeout(r, 1));
        sub.close();
        console.log(`records=${records > 20} calls<records=${calls < records}`);
        "#,
        &["--allow-read", "--allow-write", "--allow-diagnostics"],
    );
    assert_eq!(out, ["records=true calls<records=true"]);
}

// ---------------------------------------------------------------------------
// Capabilities
// ---------------------------------------------------------------------------

/// Nothing in the module works without `diagnostics`.
#[test]
fn every_export_is_denied_without_the_capability() {
    let out = lines(
        "denied",
        r#"
        import { subscribe, inventory, metrics, span } from "runtime:diagnostics";
        for (const [name, call] of [
          ["subscribe", () => subscribe({}, () => {})],
          ["inventory", () => inventory()],
          ["metrics", () => metrics()],
          ["span", () => span("x").end()],
        ]) {
          try { call(); console.log(`${name} ALLOWED`); }
          catch (e) { console.log(`${name} ${e.name}`); }
        }
        "#,
        // Importing needs nothing — the gate is the op, never the module (D7).
        &[],
    );
    assert_eq!(
        out,
        [
            "subscribe NotAllowedError",
            "inventory NotAllowedError",
            "metrics NotAllowedError",
            "span NotAllowedError"
        ]
    );
}

/// `detail` is decided by the host, not claimed by the caller: the same program
/// gets empty attributes under `observe` and populated ones under `detail`.
#[test]
fn attributes_are_empty_with_observe_and_populated_with_detail() {
    const APP: &str = r#"
        import { subscribe, span } from "runtime:diagnostics";
        import { write, remove } from "runtime:fs";
        const seen = [];
        const sub = subscribe({ kinds: ["op", "user"] }, (b) => seen.push(...b.records));
        await write("./attr.txt", "hi");
        await remove("./attr.txt");
        span("query", { attributes: { table: "users", rows: 3, cached: false } }).end();
        await new Promise((r) => setTimeout(r, 10));
        sub.close();
        const op = seen.find((r) => r.name === "fs_write");
        const user = seen.find((r) => r.kind === "user");
        console.log(JSON.stringify(op.attributes));
        console.log(JSON.stringify(user.attributes));
        // Timings and kinds are there either way — a profiler needs no payload.
        console.log(op.status, op.source, op.endedAt >= op.startedAt);
    "#;

    let observe = lines(
        "attr-observe",
        APP,
        &["--allow-read", "--allow-write", "--allow-diagnostics"],
    );
    assert_eq!(observe[0], "{}", "observe leaked an op payload");
    assert_eq!(observe[1], "{}", "observe leaked a user payload");
    assert_eq!(observe[2], "ok runtime true");

    let detail = lines(
        "attr-detail",
        APP,
        &[
            "--allow-read",
            "--allow-write",
            "--allow-diagnostics-detail",
        ],
    );
    assert_eq!(detail[0], r#"{"target":"./attr.txt"}"#);
    assert_eq!(detail[1], r#"{"table":"users","rows":3,"cached":false}"#);
    assert_eq!(detail[2], "ok runtime true");
}

/// `diagnostics-detail` implies `diagnostics`: granting only the wider one must
/// not leave every export denied.
#[test]
fn detail_alone_grants_the_whole_module() {
    let out = lines(
        "detail-implies",
        r#"
        import { metrics, inventory } from "runtime:diagnostics";
        console.log(typeof metrics().ticks, Array.isArray(inventory().handles));
        "#,
        &["--allow-diagnostics-detail"],
    );
    assert_eq!(out, ["number true"]);
}

// ---------------------------------------------------------------------------
// Records
// ---------------------------------------------------------------------------

/// The record shape, and the three timestamps meaning what they say.
#[test]
fn a_record_carries_the_documented_shape() {
    let out = lines(
        "shape",
        r#"
        import { subscribe } from "runtime:diagnostics";
        import { currentTask } from "runtime:context";
        import { write, remove } from "runtime:fs";
        const seen = [];
        const sub = subscribe({ kinds: ["op"] }, (b) => seen.push(...b.records));
        // After an await, so this runs inside a loop turn rather than during
        // module evaluation — which happens before the first tick and is
        // therefore attributed to turn 0.
        await new Promise((r) => setTimeout(r, 1));
        await write("./shape.txt", "hi");
        await remove("./shape.txt");
        await new Promise((r) => setTimeout(r, 10));
        sub.close();
        const r = seen.find((x) => x.name === "fs_write");
        console.log(Object.keys(r).sort().join(","));
        console.log(r.kind, r.source, r.status);
        // An op has no observable queue, so its delay is zero rather than faked.
        console.log("delay0:", r.startedAt === r.scheduledAt);
        console.log("ordered:", r.endedAt >= r.startedAt);
        console.log("trace:", r.traceId === currentTask().traceId);
        console.log("tick:", Number.isInteger(r.tick) && r.tick > 0);
        console.log("origin:", r.origin);
        "#,
        &["--allow-read", "--allow-write", "--allow-diagnostics"],
    );
    assert_eq!(
        out[0],
        "attributes,endedAt,id,kind,name,origin,parentId,scheduledAt,source,startedAt,status,tick,traceId"
    );
    assert_eq!(out[1], "op runtime ok");
    assert_eq!(out[2], "delay0: true");
    assert_eq!(out[3], "ordered: true");
    assert_eq!(out[4], "trace: true");
    assert_eq!(out[5], "tick: true");
    // Origins are not captured; the field is honest about it rather than absent.
    assert_eq!(out[6], "origin: 0");
}

/// A failing op is recorded with `status: "error"` rather than dropped — a span
/// you only get when it works is not a diagnostic.
#[test]
fn a_failed_op_is_recorded_as_an_error() {
    let out = lines(
        "error-status",
        r#"
        import { subscribe } from "runtime:diagnostics";
        import { file } from "runtime:fs";
        const seen = [];
        const sub = subscribe({ kinds: ["op"] }, (b) => seen.push(...b.records));
        try { await file("./definitely-not-here.txt").text(); } catch { /* expected */ }
        await new Promise((r) => setTimeout(r, 10));
        sub.close();
        console.log(seen.some((r) => r.status === "error"));
        "#,
        &["--allow-read", "--allow-diagnostics"],
    );
    assert_eq!(out, ["true"]);
}

/// A timer's queue delay is lag past its **deadline**, not the delay it asked
/// for. A 50ms timer that fires on time has ~0 delay, not 50.
#[test]
fn a_timers_queue_delay_is_lag_not_the_requested_delay() {
    let out = lines(
        "timer-lag",
        r#"
        import { subscribe } from "runtime:diagnostics";
        const seen = [];
        const sub = subscribe({ kinds: ["timer"] }, (b) => seen.push(...b.records));
        await new Promise((r) => setTimeout(r, 50));
        await new Promise((r) => setTimeout(r, 10));
        sub.close();
        const t = seen.find((r) => r.name === "setTimeout");
        console.log("name:", t.name);
        // Were `scheduledAt` the arming time this would be ~50.
        console.log("lag small:", t.startedAt - t.scheduledAt < 25);
        console.log("lag >= 0:", t.startedAt - t.scheduledAt >= 0);
        "#,
        &["--allow-diagnostics"],
    );
    assert_eq!(out[0], "name: setTimeout");
    assert_eq!(out[1], "lag small: true");
    assert_eq!(out[2], "lag >= 0: true");
}

/// A repeating timer is a new deadline each firing, so its lag does not
/// accumulate into nonsense over time.
#[test]
fn an_interval_re_anchors_its_deadline_each_firing() {
    let out = lines(
        "interval-lag",
        r#"
        import { subscribe } from "runtime:diagnostics";
        const seen = [];
        const sub = subscribe({ kinds: ["timer"] }, (b) => seen.push(...b.records));
        await new Promise((done) => {
          let n = 0;
          const id = setInterval(() => { if (++n === 4) { clearInterval(id); done(); } }, 5);
        });
        await new Promise((r) => setTimeout(r, 10));
        sub.close();
        const intervals = seen.filter((r) => r.name === "setInterval");
        console.log("fires:", intervals.length >= 3);
        console.log("bounded lag:", intervals.every((t) => t.startedAt - t.scheduledAt < 25));
        "#,
        &["--allow-diagnostics"],
    );
    assert_eq!(out[0], "fires: true");
    assert_eq!(out[1], "bounded lag: true");
}

// ---------------------------------------------------------------------------
// Loop-tick attribution — the differentiator
// ---------------------------------------------------------------------------

/// A tick record gives the turn its own timings, and every other record names
/// the turn it landed in. Together they separate "slow" from "waited".
#[test]
fn tick_records_attribute_work_to_a_loop_turn() {
    let out = lines(
        "ticks",
        r#"
        import { subscribe } from "runtime:diagnostics";
        import { write, remove } from "runtime:fs";
        const seen = [];
        const sub = subscribe({}, (b) => seen.push(...b.records));

        await new Promise((r) => setTimeout(r, 1));
        await write("./tick.txt", "hi");
        // A deliberately long synchronous stretch inside a turn: that turn is
        // long, which is exactly what a `tick` record is for.
        await new Promise((r) => {
          setTimeout(() => {
            // `Date.now()` rather than `performance.now()`: the latter is an
            // op, so busy-waiting on it would record thousands of spans and
            // measure the recorder instead of the turn.
            const until = Date.now() + 30;
            while (Date.now() < until) { /* burn this turn */ }
            r();
          }, 1);
        });
        await remove("./tick.txt");
        for (let i = 0; i < 3; i++) await new Promise((r) => setTimeout(r, 1));
        sub.close();

        const ticks = seen.filter((r) => r.kind === "tick");
        console.log("ticks:", ticks.length > 1);
        // Monotonic and numbered in order.
        console.log("ordered:", ticks.every((t, i) => i === 0 || t.tick > ticks[i - 1].tick));
        console.log("durations:", ticks.every((t) => t.endedAt >= t.startedAt));
        // At least one turn took a while — the one the busy loop ran in.
        console.log("a long turn:", ticks.some((t) => t.endedAt - t.startedAt >= 25));
        // Every record names a turn…
        const others = seen.filter((r) => r.kind !== "tick");
        console.log("numbered:", others.length > 0 && others.every((r) => r.tick > 0));
        // …and every turn that finished has its own record. The final turn is
        // excluded: closing flushes records from a turn whose own record is
        // emitted when that turn ends, which is after the flush.
        const numbers = new Set(ticks.map((t) => t.tick));
        const last = Math.max(...others.map((r) => r.tick));
        console.log("attributed:", others.filter((r) => r.tick < last).every((r) => numbers.has(r.tick)));
        "#,
        &["--allow-read", "--allow-write", "--allow-diagnostics"],
    );
    assert_eq!(out[0], "ticks: true");
    assert_eq!(out[1], "ordered: true");
    assert_eq!(out[2], "durations: true");
    assert_eq!(out[3], "a long turn: true");
    assert_eq!(out[4], "numbered: true");
    assert_eq!(out[5], "attributed: true");
}

// ---------------------------------------------------------------------------
// Filters, buffering, sampling
// ---------------------------------------------------------------------------

/// `kinds` and `minDuration` are applied host-side: a rejected record never
/// arrives, rather than arriving and being discarded in JS.
#[test]
fn filters_are_applied_before_delivery() {
    let out = lines(
        "filters",
        r#"
        import { subscribe } from "runtime:diagnostics";
        import { write, remove } from "runtime:fs";
        await new Promise((r) => setTimeout(r, 1));
        const kinds = [];
        const a = subscribe({ kinds: ["timer"] }, (b) => kinds.push(...b.records.map((r) => r.kind)));
        const slow = [];
        const b2 = subscribe({ minDuration: 10_000 }, (b) => slow.push(...b.records));
        await write("./filter.txt", "hi");
        await remove("./filter.txt");
        await new Promise((r) => setTimeout(r, 10));
        a.close();
        b2.close();
        console.log("only timers:", kinds.length > 0 && kinds.every((k) => k === "timer"));
        console.log("nothing that slow:", slow.length);
        "#,
        &["--allow-read", "--allow-write", "--allow-diagnostics"],
    );
    assert_eq!(out[0], "only timers: true");
    assert_eq!(out[1], "nothing that slow: 0");
}

/// An unknown kind is refused at the boundary rather than silently matching
/// nothing — a filter that quietly returns no records is indistinguishable from
/// a quiet program.
#[test]
fn an_unknown_kind_is_refused() {
    let out = lines(
        "bad-filter",
        r#"
        import { subscribe } from "runtime:diagnostics";
        for (const filter of [
          { kinds: ["nope"] },
          { kinds: "op" },
          { minDuration: -1 },
          { sample: 2 },
          { bufferSize: 0 },
        ]) {
          try { subscribe(filter, () => {}); console.log("ACCEPTED"); }
          catch (e) { console.log(e.name); }
        }
        "#,
        &["--allow-diagnostics"],
    );
    assert_eq!(out, ["TypeError"; 5]);
}

/// Overflow drops the **newest** and reports the count, and what is delivered
/// stays coherent — no record arrives whose turn was never delivered.
#[test]
fn buffer_overflow_reports_a_dropped_count_and_leaves_no_orphans() {
    let out = lines(
        "overflow",
        r#"
        import { subscribe } from "runtime:diagnostics";
        import { write, remove, file } from "runtime:fs";
        let dropped = 0;
        const seen = [];
        const sub = subscribe({ kinds: ["op"], bufferSize: 4 }, (b) => {
          dropped += b.dropped;
          seen.push(...b.records);
        });
        // Far more ops in one turn than the buffer can hold.
        for (let i = 0; i < 40; i++) {
          await write("./of.txt", "x");
          await file("./of.txt").text();
        }
        await remove("./of.txt");
        await new Promise((r) => setTimeout(r, 20));
        sub.close();
        console.log("dropped:", dropped > 0);
        console.log("ids unique:", new Set(seen.map((r) => r.id)).size === seen.length);
        // Every record delivered is whole: an end without its start is what
        // drop-oldest would have produced.
        console.log("whole:", seen.every((r) => r.endedAt >= r.startedAt && r.name.length > 0));
        "#,
        &["--allow-read", "--allow-write", "--allow-diagnostics"],
    );
    assert_eq!(out[0], "dropped: true");
    assert_eq!(out[1], "ids unique: true");
    assert_eq!(out[2], "whole: true");
}

/// Sampling is per **trace**: a trace is kept whole or dropped whole, never
/// half. `sample: 0` keeps nothing that belongs to a trace; `sample: 1` keeps
/// everything.
#[test]
fn sampling_is_per_trace() {
    let out = lines(
        "sampling",
        r#"
        import { subscribe } from "runtime:diagnostics";
        import { write, remove } from "runtime:fs";
        const none = [];
        const all = [];
        const a = subscribe({ kinds: ["op"], sample: 0 }, (b) => none.push(...b.records));
        const b2 = subscribe({ kinds: ["op"], sample: 1 }, (b) => all.push(...b.records));
        await write("./s.txt", "hi");
        await remove("./s.txt");
        await new Promise((r) => setTimeout(r, 10));
        a.close();
        b2.close();
        // Everything here shares this agent's root trace, so it is one trace:
        // whole under sample:1 and absent under sample:0.
        console.log("kept none:", none.length);
        console.log("kept all:", all.length > 0);
        console.log("one trace:", new Set(all.map((r) => r.traceId)).size === 1);
        "#,
        &["--allow-read", "--allow-write", "--allow-diagnostics"],
    );
    assert_eq!(out[0], "kept none: 0");
    assert_eq!(out[1], "kept all: true");
    assert_eq!(out[2], "one trace: true");
}

/// Two subscriptions are independent: their filters, their buffers, and their
/// closing. One closing must not disturb the other.
#[test]
fn subscriptions_are_independent() {
    let out = lines(
        "independent",
        r#"
        import { subscribe } from "runtime:diagnostics";
        const ops = [];
        const timers = [];
        const a = subscribe({ kinds: ["op"] }, (b) => ops.push(...b.records));
        const b2 = subscribe({ kinds: ["timer"] }, (b) => timers.push(...b.records));
        await new Promise((r) => setTimeout(r, 5));
        a.close();
        a.close(); // idempotent
        const before = timers.length;
        await new Promise((r) => setTimeout(r, 5));
        b2.close();
        console.log("timers kept flowing:", timers.length > before);
        console.log("ops only ops:", ops.every((r) => r.kind === "op"));
        "#,
        &["--allow-diagnostics"],
    );
    assert_eq!(out[0], "timers kept flowing: true");
    assert_eq!(out[1], "ops only ops: true");
}

/// A subscriber that throws does not take down the turn that delivered to it,
/// and stays subscribed.
#[test]
fn a_throwing_subscriber_is_reported_and_kept() {
    let out = lines(
        "throwing",
        r#"
        import { subscribe } from "runtime:diagnostics";
        let batches = 0;
        addEventListener("error", (event) => event.preventDefault());
        const sub = subscribe({ kinds: ["timer"] }, () => { batches++; throw new Error("boom"); });
        await new Promise((r) => setTimeout(r, 5));
        await new Promise((r) => setTimeout(r, 5));
        await new Promise((r) => setTimeout(r, 5));
        sub.close();
        console.log("delivered more than once:", batches > 1);
        "#,
        &["--allow-diagnostics"],
    );
    assert_eq!(out, ["delivered more than once: true"]);
}

// ---------------------------------------------------------------------------
// User spans
// ---------------------------------------------------------------------------

/// A user span shares the runtime's id space and timeline, distinguished only by
/// `source`. Ending twice records once.
#[test]
fn user_spans_share_the_id_space_and_end_once() {
    let out = lines(
        "user-spans",
        r#"
        import { subscribe, span } from "runtime:diagnostics";
        import { write, remove } from "runtime:fs";
        const seen = [];
        const sub = subscribe({}, (b) => seen.push(...b.records));
        await write("./u.txt", "hi");
        const s = span("checkout");
        await new Promise((r) => setTimeout(r, 5));
        s.end();
        s.end();      // a second end is the bug, not a second span
        span("failed").fail();
        span("gone").cancel();
        await remove("./u.txt");
        await new Promise((r) => setTimeout(r, 10));
        sub.close();
        const users = seen.filter((r) => r.source === "user");
        console.log("names:", users.map((r) => r.name).sort().join(","));
        console.log("statuses:", users.map((r) => `${r.name}=${r.status}`).sort().join(","));
        console.log("once:", users.filter((r) => r.name === "checkout").length);
        console.log("measured:", users.find((r) => r.name === "checkout").endedAt
          - users.find((r) => r.name === "checkout").startedAt >= 4);
        // One id space with the runtime's own spans.
        console.log("unique ids:", new Set(seen.map((r) => r.id)).size === seen.length);
        console.log("kind:", users.every((r) => r.kind === "user"));
        "#,
        &["--allow-read", "--allow-write", "--allow-diagnostics"],
    );
    assert_eq!(out[0], "names: checkout,failed,gone");
    assert_eq!(out[1], "statuses: checkout=ok,failed=error,gone=cancelled");
    assert_eq!(out[2], "once: 1");
    assert_eq!(out[3], "measured: true");
    assert_eq!(out[4], "unique ids: true");
    assert_eq!(out[5], "kind: true");
}

/// `span()` refuses what it cannot record.
#[test]
fn span_validates_its_arguments() {
    let out = lines(
        "span-args",
        r#"
        import { span } from "runtime:diagnostics";
        for (const call of [
          () => span(42),
          () => span("x", { attributes: "no" }),
          () => span("x", { attributes: null }),
        ]) {
          try { call(); console.log("ACCEPTED"); } catch (e) { console.log(e.name); }
        }
        "#,
        &["--allow-diagnostics"],
    );
    assert_eq!(out, ["TypeError"; 3]);
}

// ---------------------------------------------------------------------------
// inventory / metrics
// ---------------------------------------------------------------------------

/// `inventory()` reports the handle registries D50 already maintains — an id and
/// a kind, never the resource. A live HTTP server shows up; nothing does once
/// the agent holds none of a kind.
#[test]
fn inventory_reports_handles_this_agent_owns() {
    let out = lines(
        "inventory",
        r#"
        import { inventory } from "runtime:diagnostics";
        import { serve } from "runtime:http";
        console.log("before:", JSON.stringify(inventory().handles));
        const server = serve({ hostname: "127.0.0.1", port: 0 }, () => new Response("ok"));
        await server.addr;
        const during = inventory().handles;
        console.log("kinds:", during.map((h) => h.kind).join(","));
        const http = during.find((h) => h.kind === "HTTP server");
        console.log("shape:", Object.keys(http).sort().join(","), http.count, http.ids.length);
        // An id and a kind — never the resource, which is what made async_hooks
        // impossible to fix.
        console.log("ids are numbers:", http.ids.every((id) => typeof id === "number"));
        await server.stop();
        "#,
        &["--allow-listen", "--allow-diagnostics"],
    );
    assert_eq!(out[0], "before: []");
    assert_eq!(out[1], "kinds: HTTP server");
    assert_eq!(out[2], "shape: count,ids,kind 1 1");
    assert_eq!(out[3], "ids are numbers: true");
}

/// `metrics()` is pull-only and reports the loop's own shape.
#[test]
fn metrics_reports_the_loops_shape() {
    let out = lines(
        "metrics",
        r#"
        import { metrics } from "runtime:diagnostics";
        const before = metrics();
        // Several turns, one of them deliberately long. A turn's duration is
        // recorded when it *ends*, so a reading taken mid-turn never includes
        // the turn it is taken in.
        for (let i = 0; i < 3; i++) {
          await new Promise((r) => setTimeout(r, 1));
        }
        await new Promise((r) => {
          setTimeout(() => {
            const until = Date.now() + 25;
            while (Date.now() < until) { /* burn this turn */ }
            r();
          }, 1);
        });
        for (let i = 0; i < 3; i++) {
          await new Promise((r) => setTimeout(r, 1));
        }
        const after = metrics();
        console.log("keys:", Object.keys(after).sort().join(","));
        console.log("hist keys:", Object.keys(after.tickDurationMs).sort().join(","));
        console.log("ticks advance:", after.ticks > before.ticks);
        console.log("durations recorded:", after.tickDurationMs.count > 0);
        console.log("a long turn:", after.tickDurationMs.max >= 20);
        console.log("lag recorded:", after.loopLagMs.count > 0 && after.loopLagMs.min >= 0);
        console.log("quantiles ordered:", after.tickDurationMs.p50 <= after.tickDurationMs.p99);
        "#,
        &["--allow-diagnostics"],
    );
    assert_eq!(out[0], "keys: loopLagMs,tick,tickDurationMs,ticks");
    assert_eq!(out[1], "hist keys: count,max,mean,min,p50,p99");
    assert_eq!(out[2], "ticks advance: true");
    assert_eq!(out[3], "durations recorded: true");
    assert_eq!(out[4], "a long turn: true");
    assert_eq!(out[5], "lag recorded: true");
    assert_eq!(out[6], "quantiles ordered: true");
}

/// `resolveOrigin` is gated on `detail` and says plainly that nothing captures
/// an origin yet, rather than returning a fabricated position.
#[test]
fn resolve_origin_is_gated_and_honest() {
    let denied = lines(
        "origin-denied",
        r#"
        import { resolveOrigin } from "runtime:diagnostics";
        try { resolveOrigin(1); } catch (e) { console.log(e.name); }
        "#,
        &["--allow-diagnostics"],
    );
    assert_eq!(denied, ["NotAllowedError"]);

    let granted = lines(
        "origin-granted",
        r#"
        import { resolveOrigin } from "runtime:diagnostics";
        try { resolveOrigin(1); } catch (e) { console.log(e.name, e.message); }
        try { resolveOrigin(-1); } catch (e) { console.log(e.name); }
        "#,
        &["--allow-diagnostics-detail"],
    );
    assert_eq!(
        granted[0],
        "Error span origins are not captured: every record's `origin` is 0"
    );
    assert_eq!(granted[1], "TypeError");
}

// ---------------------------------------------------------------------------
// The one-directional dependency
// ---------------------------------------------------------------------------

/// Diagnostics reads a trace id from `runtime:context`. Nothing goes the other
/// way: no context value reaches a record, and none is reachable through a
/// diagnostics export.
#[test]
fn no_context_value_reaches_a_diagnostics_record() {
    let out = lines(
        "one-way",
        r#"
        import { subscribe } from "runtime:diagnostics";
        import { createContext, currentTask } from "runtime:context";
        import { write, remove } from "runtime:fs";

        const secret = createContext({ name: "tenant", defaultValue: "none" });
        const seen = [];
        const sub = subscribe({}, (b) => seen.push(...b.records));

        await secret.run("SENSITIVE-VALUE", async () => {
          await write("./ow.txt", "hi");
          await remove("./ow.txt");
        });
        await new Promise((r) => setTimeout(r, 10));
        sub.close();

        const dumped = JSON.stringify(seen);
        console.log("value absent:", !dumped.includes("SENSITIVE-VALUE"));
        console.log("context name absent:", !dumped.includes("tenant"));
        // The trace *is* shared — that is the whole dependency, and it runs one way.
        console.log("trace shared:", seen.some((r) => r.traceId === currentTask().traceId));
        "#,
        &[
            "--allow-read",
            "--allow-write",
            "--allow-diagnostics-detail",
        ],
    );
    assert_eq!(out[0], "value absent: true");
    assert_eq!(out[1], "context name absent: true");
    assert_eq!(out[2], "trace shared: true");
}
