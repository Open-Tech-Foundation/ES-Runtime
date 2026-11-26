//! The event loop: ticks, timers, microtasks, async ops and rejections.

use super::*;

#[test]
fn snapshot_runtime_runs_baked_prelude() {
    // Bake the real ops + full prelude into a snapshot, restore a runtime
    // from it, and exercise several op-backed APIs to prove the baked
    // context behaves like a freshly-built one (DECISIONS.md D8).
    let _g = v8_guard();
    let blob = Runtime::build_snapshot(&test_providers()).expect("build snapshot");
    let mut rt = Runtime::with_snapshot(blob, test_providers()).expect("restore");
    let out = eval_async(
        &mut rt,
        "const u = new URL('https://x.test/a?b=1'); \
         const h = await crypto.subtle.digest('SHA-256', new TextEncoder().encode('abc')); \
         const id = crypto.randomUUID(); \
         console.log('from snapshot'); \
         return `${u.host}|${new Uint8Array(h).length}|${id.length}`;",
    );
    assert_eq!(out, Value::String("x.test|32|36".into()));
}

/// Many runtimes on one thread (D227): each restored from the snapshot,
/// left after construction, and entered only for its turns, with async ops
/// and timers settling across them.
#[test]
fn runtimes_on_one_thread_take_turns() {
    let _g = v8_guard();
    let blob = Runtime::build_snapshot(&test_providers()).expect("build snapshot");
    let mut runtimes: Vec<Runtime> = (0..4)
        .map(|_| {
            let mut rt = Runtime::with_snapshot(blob.clone(), test_providers()).expect("restore");
            rt.leave();
            rt
        })
        .collect();
    for (i, rt) in runtimes.iter_mut().enumerate() {
        rt.enter();
        rt.eval(&format!("globalThis.me = {i};")).expect("eval");
        rt.leave();
    }
    for (i, rt) in runtimes.iter_mut().enumerate().rev() {
        rt.enter();
        let out = eval_async(
            rt,
            "await new Promise((r) => setTimeout(r, 0)); \
             const h = await crypto.subtle.digest('SHA-256', new Uint8Array([me])); \
             return `${me}:${new Uint8Array(h).length}`;",
        );
        rt.leave();
        assert_eq!(out, Value::String(format!("{i}:32")));
    }
    // Dropped oldest first, every one of them left.
    drop(runtimes);
}

#[test]
fn snapshot_runtime_async_ops_and_timers_work() {
    // The driven loop (timers + async settling) must work over a restored
    // engine just as over a fresh one.
    let _g = v8_guard();
    let blob = Runtime::build_snapshot(&test_providers()).expect("build snapshot");
    let mut rt = Runtime::with_snapshot(blob, test_providers()).expect("restore");
    // `eval_async` drives ticks at now=0, so use a 0ms timer (fires at 0);
    // this still exercises the baked `setTimeout` builtin + the driven loop.
    let out = eval_async(
        &mut rt,
        "let v = 0; \
         await new Promise((r) => setTimeout(() => { v = 7; r(); }, 0)); \
         return v;",
    );
    assert_eq!(out, Value::Number(7.0));
}

/// A throw out of a timer callback used to be swallowed whole: the callback
/// has no caller left to propagate to, and nothing reported it. It must
/// reach the embedder.
#[test]
fn an_exception_out_of_a_timer_reaches_the_embedder() {
    let _g = v8_guard();
    let mut rt = runtime();
    rt.eval("setTimeout(() => { throw new TypeError('from a timer'); }, 0);")
        .unwrap();
    let status = rt.tick(0);
    assert_eq!(status.timers_fired, 1);
    assert_eq!(
        status.uncaught_errors.len(),
        1,
        "{:?}",
        status.uncaught_errors
    );
    assert!(
        status.uncaught_errors[0]
            .to_string()
            .contains("from a timer"),
        "{}",
        status.uncaught_errors[0]
    );
    // Drained, not repeated on the next tick.
    assert!(rt.tick(1).uncaught_errors.is_empty());
}

/// …unless a listener claims it. `preventDefault()` is how guest code takes
/// responsibility, and the host must then stay quiet.
#[test]
fn an_error_listener_can_claim_a_timer_exception() {
    let _g = v8_guard();
    let mut rt = runtime();
    rt.eval(
        "globalThis.seen = null; \
         globalThis.addEventListener('error', (e) => { \
           globalThis.seen = e.message; e.preventDefault(); }); \
         setTimeout(() => { throw new TypeError('claimed'); }, 0);",
    )
    .unwrap();
    let status = rt.tick(0);
    assert!(
        status.uncaught_errors.is_empty(),
        "{:?}",
        status.uncaught_errors
    );
    assert_eq!(
        rt.eval("globalThis.seen").unwrap(),
        Value::String("claimed".into())
    );
}

/// A rejection nobody handled is reported — and a listener that claims it
/// with `preventDefault()` keeps it away from the embedder entirely.
#[test]
fn an_unhandled_rejection_can_be_claimed_by_a_listener() {
    let _g = v8_guard();
    let mut rt = runtime();
    rt.eval("Promise.reject(new Error('unclaimed'));").unwrap();
    let status = rt.tick(0);
    assert_eq!(
        status.unhandled_rejections.len(),
        1,
        "{:?}",
        status.unhandled_rejections
    );

    let mut rt = runtime();
    rt.eval(
        "globalThis.reason = null; \
         globalThis.addEventListener('unhandledrejection', (e) => { \
           globalThis.reason = e.reason.message; e.preventDefault(); }); \
         Promise.reject(new Error('claimed'));",
    )
    .unwrap();
    let status = rt.tick(0);
    assert!(
        status.unhandled_rejections.is_empty(),
        "{:?}",
        status.unhandled_rejections
    );
    assert_eq!(
        rt.eval("globalThis.reason").unwrap(),
        Value::String("claimed".into())
    );
}

/// Attaching a handler *after* the report has gone out fires
/// `rejectionhandled` — the spec's retraction. It cannot be covered by the
/// conformance suite, which would have to leave the report unclaimed and so
/// fail the `esrun` runner it also runs under.
#[test]
fn a_late_handler_fires_rejectionhandled() {
    let _g = v8_guard();
    let mut rt = runtime();
    rt.eval(
        "globalThis.retracted = 0; \
         globalThis.addEventListener('rejectionhandled', (e) => { \
           if (e.promise === globalThis.p) globalThis.retracted++; }); \
         globalThis.p = Promise.reject(new Error('late'));",
    )
    .unwrap();
    // The report goes out: no listener claimed it.
    assert_eq!(rt.tick(0).unhandled_rejections.len(), 1);
    assert_eq!(rt.eval("globalThis.retracted").unwrap(), Value::Number(0.0));

    // Now a handler arrives, retracting it.
    rt.eval("globalThis.p.catch(() => {});").unwrap();
    rt.tick(1);
    assert_eq!(rt.eval("globalThis.retracted").unwrap(), Value::Number(1.0));
}

#[test]
fn clearing_a_timer_releases_the_loop_immediately() {
    // Regression: `clearTimeout` cancelled the callback but left the entry
    // in the schedule, so the runtime reported pending work — and a driver
    // slept — until the original delay elapsed. A cleared 60s timer must
    // leave nothing outstanding and no deadline to wait on.
    let _g = v8_guard();
    let mut rt = runtime();
    rt.eval("const id = setTimeout(() => {}, 60000); clearTimeout(id);")
        .unwrap();
    let outcome = rt.tick(0);
    assert_eq!(outcome.timers_fired, 0);
    assert_eq!(outcome.next_timer_deadline_ms, None);
    assert!(!outcome.has_pending_work);
    assert!(!rt.has_pending_work());
}

#[test]
fn a_timer_callbacks_microtasks_run_before_the_next_timer() {
    // Two timers due in the same tick are two tasks: the first callback's
    // promise reaction runs before the second callback starts.
    let _g = v8_guard();
    let mut rt = runtime();
    rt.eval(
        "globalThis.log = []; \
         setTimeout(() => { log.push('t1'); Promise.resolve().then(() => log.push('micro')); }, 0); \
         setTimeout(() => { log.push('t2'); queueMicrotask(() => log.push('micro2')); }, 0); \
         setTimeout(() => log.push('t3'), 0);",
    )
    .unwrap();
    assert_eq!(rt.tick(0).timers_fired, 3);
    assert_eq!(
        rt.eval("log.join(',')").unwrap(),
        Value::String("t1,micro,t2,micro2,t3".into())
    );
}

#[test]
fn clearing_one_timer_leaves_the_others_scheduled() {
    // Pruning must not take live timers with it: the surviving timer keeps
    // its own deadline and still fires.
    let _g = v8_guard();
    let mut rt = runtime();
    rt.eval(
        "globalThis.fired = false; \
         const dead = setTimeout(() => {}, 60000); \
         setTimeout(() => { globalThis.fired = true; }, 10); \
         clearTimeout(dead);",
    )
    .unwrap();
    assert_eq!(rt.tick(0).next_timer_deadline_ms, Some(10));
    let outcome = rt.tick(10);
    assert_eq!(outcome.timers_fired, 1);
    assert_eq!(rt.eval("globalThis.fired").unwrap(), Value::Bool(true));
    assert!(!rt.has_pending_work());
}

/// The smallest valid module: `(func (export "add") (param i32 i32)
/// (result i32) local.get 0 local.get 1 i32.add)`.
const ADD_WASM: &str = "new Uint8Array([\
    0,97,115,109,1,0,0,0,\
    1,7,1,96,2,127,127,1,127,\
    3,2,1,0,\
    7,7,1,3,97,100,100,0,0,\
    10,9,1,7,0,32,0,32,1,106,11])";

/// The waiting helper must stop the moment the condition holds — the hot
/// path is every `eval_async` in this file, including a 20k-iteration soak.
#[test]
fn pump_until_returns_as_soon_as_the_condition_holds() {
    let _g = v8_guard();
    let mut rt = runtime();
    let mut ticks = 0u32;
    let started = std::time::Instant::now();
    pump_until(&mut rt, "three ticks", |_| {
        ticks += 1;
        ticks == 3
    });
    assert_eq!(ticks, 3);
    // Three spins, no sleeps: this is microseconds, not milliseconds.
    assert!(started.elapsed() < std::time::Duration::from_millis(100));
}

/// The bug this helper replaced: a count-bounded loop gave up *silently*,
/// so the test went on to assert against a result that had not arrived and
/// reported the miss as a behaviour failure. Running out of time must be a
/// panic that names what was awaited.
#[test]
#[should_panic(expected = "waiting for something that never happens")]
fn pump_until_panics_instead_of_falling_through() {
    let _g = v8_guard();
    let mut rt = runtime();
    pump_until_within(
        &mut rt,
        std::time::Duration::from_millis(50),
        "something that never happens",
        |_| false,
    );
}

/// An async WebAssembly compile settles only because the loop pumps V8's
/// foreground task queue, and counts as pending work until it does — without
/// which a driver would exit (or park forever) mid-compile. The conformance
/// suite cannot cover this: it runs files without a driver.
#[test]
fn async_wasm_compile_settles_and_holds_the_loop_open() {
    let _g = v8_guard();
    let mut rt = runtime();

    rt.eval(&format!(
        "globalThis.mod = null; \
         WebAssembly.compile({ADD_WASM}).then((m) => {{ globalThis.mod = m; }});"
    ))
    .unwrap();

    // In flight: nothing has resolved, and the runtime must report work
    // outstanding even though no op, timer, or import is pending.
    assert_eq!(rt.eval("globalThis.mod").unwrap(), Value::Null);
    assert!(
        rt.has_pending_work(),
        "an in-flight wasm compile must keep the loop alive"
    );

    // Ticking pumps the platform until V8's compile task lands. How long
    // that takes is up to V8's background threads, so wait on the clock.
    pump_until(&mut rt, "the wasm compile to settle", |rt| {
        !rt.has_pending_work()
    });

    assert_eq!(
        rt.eval("globalThis.mod instanceof WebAssembly.Module")
            .unwrap(),
        Value::Bool(true),
        "the compile promise never settled"
    );
    assert!(
        !rt.has_pending_work(),
        "the compile should no longer be pending"
    );
}

/// A rejected compile must release its pending-work claim too, or the loop
/// would never quiesce after a bad module.
#[test]
fn failed_async_wasm_compile_releases_pending_work() {
    let _g = v8_guard();
    let mut rt = runtime();

    rt.eval(
        "globalThis.err = null; \
         WebAssembly.compile(new Uint8Array([0,1,2,3])).catch((e) => { globalThis.err = e.name; });",
    )
    .unwrap();

    pump_until(&mut rt, "the failed wasm compile to settle", |rt| {
        !rt.has_pending_work()
    });

    assert_eq!(
        rt.eval("globalThis.err").unwrap(),
        Value::String("CompileError".into())
    );
    assert!(!rt.has_pending_work());
}

#[test]
fn set_timeout_fires_only_after_its_deadline() {
    let _g = v8_guard();
    let mut rt = runtime();
    rt.eval("globalThis.fired = false; setTimeout(() => { globalThis.fired = true; }, 50);")
        .unwrap();

    // Before the deadline: scheduled, not fired.
    let early = rt.tick(10);
    assert_eq!(early.timers_fired, 0);
    assert_eq!(early.next_timer_deadline_ms, Some(50));
    assert_eq!(rt.eval("globalThis.fired").unwrap(), Value::Bool(false));

    // At/after the deadline: fires exactly once, then no work remains.
    let late = rt.tick(50);
    assert_eq!(late.timers_fired, 1);
    assert_eq!(rt.eval("globalThis.fired").unwrap(), Value::Bool(true));
    assert!(!rt.has_pending_work());
}

#[test]
fn clear_timeout_prevents_firing() {
    let _g = v8_guard();
    let mut rt = runtime();
    rt.eval(
        "globalThis.fired = false; \
         const id = setTimeout(() => { globalThis.fired = true; }, 20); \
         clearTimeout(id);",
    )
    .unwrap();
    let status = rt.tick(100);
    assert_eq!(status.timers_fired, 0);
    assert_eq!(rt.eval("globalThis.fired").unwrap(), Value::Bool(false));
}

#[test]
fn unhandled_rejection_is_reported() {
    let _g = v8_guard();
    let mut rt = runtime();
    rt.eval("Promise.reject('boom'); undefined").unwrap();
    let status = rt.tick(0);
    assert!(
        status
            .unhandled_rejections
            .iter()
            .any(|error| error.to_string().contains("boom")),
        "got {:?}",
        status.unhandled_rejections
    );
}

#[test]
fn idle_runtime_reports_no_work() {
    let _g = v8_guard();
    let mut rt = runtime();
    rt.eval("1 + 1").unwrap();
    let status = rt.tick(0);
    assert!(!status.has_pending_work);
    assert_eq!(status.next_timer_deadline_ms, None);
}

#[test]
fn queue_microtask_runs_at_the_checkpoint() {
    let _g = v8_guard();
    let mut rt = runtime();
    rt.eval("globalThis.x = 0; queueMicrotask(() => { globalThis.x = 1; });")
        .unwrap();
    // Explicit microtask policy: not run until the tick's checkpoint.
    assert_eq!(rt.eval("globalThis.x").unwrap(), Value::Number(0.0));
    rt.tick(0);
    assert_eq!(rt.eval("globalThis.x").unwrap(), Value::Number(1.0));
}

#[test]
fn set_timeout_forwards_trailing_arguments_to_the_callback() {
    let _g = v8_guard();
    let mut rt = runtime();
    let out = eval_async(
        &mut rt,
        "return await new Promise((resolve) => \
           setTimeout((a, b, c) => resolve(`${a}|${b}|${c}`), 0, 1, 'two', null));",
    );
    assert_eq!(out, Value::String("1|two|null".into()));
}

#[test]
fn set_timeout_with_no_extra_arguments_passes_none() {
    let _g = v8_guard();
    let mut rt = runtime();
    let out = eval_async(
        &mut rt,
        "return await new Promise((resolve) => \
           setTimeout((...args) => resolve(args.length), 0));",
    );
    assert_eq!(out, Value::Number(0.0));
}

#[test]
fn set_interval_forwards_its_arguments_on_every_firing() {
    let _g = v8_guard();
    let mut rt = runtime();
    // A repeating timer keeps its arguments across firings, not just the
    // first — they are stored with the timer, not consumed by it.
    let out = eval_async(
        &mut rt,
        "return await new Promise((resolve) => { \
           let seen = ''; \
           const id = setInterval((tag) => { \
             seen += tag; \
             if (seen.length === 3) { clearInterval(id); resolve(seen); } \
           }, 0, 'x'); \
         });",
    );
    assert_eq!(out, Value::String("xxx".into()));
}
