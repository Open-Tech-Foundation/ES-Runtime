//! What it costs to run many isolates on a few threads, under `v8::Locker`.
//!
//! Phase 0 of the lightweight-isolate ("green thread") study. A pool of
//! isolates over a few OS threads has two shapes, and this measures both:
//!
//! * **pinned** — every isolate belongs to one pool thread, which locks and
//!   enters whichever of its isolates has work. No isolate changes thread.
//! * **migrating** — any pool thread takes any isolate with work (a shared
//!   queue), so consecutive turns of one isolate land on different threads and
//!   V8's `ThreadManager` archives and restores its per-thread state.
//!
//! * **entered** — owned isolates on one thread, entered and left around each
//!   turn with no `Locker` at all, which a thread that owns its isolates can do.
//!
//! Each turn is one call of a trivial JS function — the per-turn floor, not a
//! workload. Baseline: the same call with the lock held throughout.
//!
//! ```text
//! cargo run --release --example isolate_locker -p es-runtime-engine -- [ISOLATES] [THREADS]
//! ```
//!
//! It uses the `v8` crate directly, because the engine's isolates are not
//! shareable yet (that is what the study is about); `engine` is the one crate
//! allowed to.

// A benchmark's whole job is to print its numbers.
#![allow(clippy::print_stdout)]

use std::pin::pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A shared isolate with a context and a compiled `turn()` in a slot.
struct Tenant {
    isolate: v8::SharedIsolate,
}

/// The context and the function a turn calls, kept in the isolate's slot so
/// whichever thread holds the lock can reach them. Both are `Send`, as a shared
/// isolate's slots must be.
struct Handles {
    context: v8::Global<v8::Context>,
    turn: v8::Global<v8::Function>,
}

fn rss_kib() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("VmRSS:"))
                .and_then(|v| v.trim().trim_end_matches("kB").trim().parse().ok())
        })
        .unwrap_or(0)
}

fn tenant() -> Tenant {
    let isolate = owned();
    // SAFETY: the only embedder state on the isolate is `Handles`, two
    // `Global`s, which are `Send`; nothing else is attached, then or later.
    let isolate = unsafe { isolate.try_into_shared() }.expect("shareable");
    Tenant { isolate }
}

/// An owned isolate with its context and `turn()` in a slot, still entered.
fn owned() -> v8::OwnedIsolate {
    let mut isolate = v8::Isolate::new(v8::CreateParams::default());
    let handles = {
        let scope = pin!(v8::HandleScope::new(&mut isolate));
        let mut scope = scope.init();
        let context = v8::Context::new(&scope, v8::ContextOptions::default());
        let scope = &mut v8::ContextScope::new(&mut scope, context);
        let source =
            v8::String::new(scope, "let n = 0; (function turn() { return ++n; })").expect("source");
        let script = v8::Script::compile(scope, source, None).expect("compile");
        let turn = script.run(scope).expect("run");
        let turn = v8::Local::<v8::Function>::try_from(turn).expect("function");
        Handles {
            context: v8::Global::new(scope, context),
            turn: v8::Global::new(scope, turn),
        }
    };
    isolate.set_slot(handles);
    isolate
}

/// One turn: lock, enter the context, call `turn()`, unlock.
fn turn(tenant: &Tenant) {
    let mut locker = tenant.isolate.lock();
    call(&mut locker);
}

fn call(isolate: &mut v8::Isolate) {
    let (context, function) = {
        let handles = isolate.get_slot::<Handles>().expect("handles");
        (handles.context.clone(), handles.turn.clone())
    };
    let scope = pin!(v8::HandleScope::new(isolate));
    let mut scope = scope.init();
    let context = v8::Local::new(&scope, context);
    let scope = &mut v8::ContextScope::new(&mut scope, context);
    let function = v8::Local::new(scope, function);
    let receiver = v8::undefined(scope).into();
    function.call(scope, receiver, &[]).expect("call");
}

fn per(elapsed: Duration, turns: usize) -> Duration {
    elapsed / u32::try_from(turns).unwrap_or(u32::MAX)
}

fn main() {
    let mut args = std::env::args().skip(1);
    let isolates: usize = args.next().and_then(|a| a.parse().ok()).unwrap_or(1000);
    let threads: usize = args
        .next()
        .and_then(|a| a.parse().ok())
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(4, std::num::NonZero::get));
    const ROUNDS: usize = 50;

    // Every thread that locks must be created after V8 is initialized (see
    // `SharedIsolate`'s docs on memory protection keys).
    v8::V8::initialize_platform(v8::new_default_platform(0, false).make_shared());
    v8::V8::initialize();

    let before = rss_kib();
    let start = Instant::now();
    let tenants: Arc<Vec<Tenant>> = Arc::new((0..isolates).map(|_| tenant()).collect());
    let created = start.elapsed();
    #[allow(clippy::cast_precision_loss)]
    let mib = rss_kib().saturating_sub(before) as f64 / 1024.0 / isolates as f64;
    println!(
        "{isolates} bare shared isolates on one thread: create {:.2?}/isolate, rss {mib:.2} MiB/isolate",
        per(created, isolates)
    );

    // Baseline: one isolate, lock held, call after call.
    {
        let mut locker = tenants[0].isolate.lock();
        let turns = 200_000;
        let start = Instant::now();
        for _ in 0..turns {
            call(&mut locker);
        }
        println!(
            "baseline   1 isolate, lock held          {:>9.2?}/turn",
            per(start.elapsed(), turns)
        );
    }

    // Same isolate, lock taken every turn, always on this thread.
    {
        let turns = 200_000;
        let start = Instant::now();
        for _ in 0..turns {
            turn(&tenants[0]);
        }
        println!(
            "lock/turn  1 isolate, one thread         {:>9.2?}/turn",
            per(start.elapsed(), turns)
        );
    }

    // Pinned: one thread, round-robin over every isolate.
    {
        let turns = ROUNDS * isolates;
        let start = Instant::now();
        for _ in 0..ROUNDS {
            for t in tenants.iter() {
                turn(t);
            }
        }
        println!(
            "pinned     {isolates} isolates, 1 thread        {:>9.2?}/turn",
            per(start.elapsed(), turns)
        );
    }

    // Pinned across the pool: thread k owns isolates k, k+threads, … .
    {
        let turns = ROUNDS * isolates;
        let start = Instant::now();
        let handles: Vec<_> = (0..threads)
            .map(|k| {
                let tenants = Arc::clone(&tenants);
                std::thread::spawn(move || {
                    for _ in 0..ROUNDS {
                        for t in tenants.iter().skip(k).step_by(threads) {
                            turn(t);
                        }
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().expect("join");
        }
        let elapsed = start.elapsed();
        #[allow(clippy::cast_precision_loss)]
        let rate = turns as f64 / elapsed.as_secs_f64();
        println!(
            "pinned     {isolates} isolates, {threads} threads      {:>9.2?}/turn wall   {rate:>10.0} turns/s",
            per(elapsed, turns)
        );
    }

    // Migrating: one shared queue of turns, any thread takes the next. A
    // round-robin queue means consecutive turns of one isolate almost always
    // run on different threads — the worst case, and a work-stealing
    // executor's common one.
    {
        let turns = ROUNDS * isolates;
        let next = Arc::new(AtomicUsize::new(0));
        let moved = Arc::new(AtomicUsize::new(0));
        let last: Arc<Vec<Mutex<usize>>> =
            Arc::new((0..isolates).map(|_| Mutex::new(usize::MAX)).collect());
        let start = Instant::now();
        let handles: Vec<_> = (0..threads)
            .map(|k| {
                let (tenants, next, moved, last) = (
                    Arc::clone(&tenants),
                    Arc::clone(&next),
                    Arc::clone(&moved),
                    Arc::clone(&last),
                );
                std::thread::spawn(move || {
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        if i >= turns {
                            break;
                        }
                        let t = i % tenants.len();
                        {
                            let mut seen = last[t].lock().expect("last");
                            if *seen != k {
                                moved.fetch_add(1, Ordering::Relaxed);
                            }
                            *seen = k;
                        }
                        turn(&tenants[t]);
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().expect("join");
        }
        let elapsed = start.elapsed();
        #[allow(clippy::cast_precision_loss)]
        let rate = turns as f64 / elapsed.as_secs_f64();
        #[allow(clippy::cast_precision_loss)]
        let share = moved.load(Ordering::Relaxed) as f64 * 100.0 / turns as f64;
        println!(
            "migrating  {isolates} isolates, {threads} threads      {:>9.2?}/turn wall   {rate:>10.0} turns/s   ({share:.0}% of turns changed thread)",
            per(elapsed, turns)
        );
    }

    // Pinned without a lock: owned isolates on one thread, each left after it
    // is made and entered again for its turn. Nothing else may use them, so
    // V8 needs no `Locker` — this is what a pool thread owning its isolates
    // outright would pay.
    {
        let mut owned: Vec<v8::OwnedIsolate> = (0..isolates).map(|_| owned()).collect();
        for isolate in owned.iter().rev() {
            // SAFETY: each is the current isolate when exited, newest first.
            unsafe { isolate.exit() };
        }
        let turns = ROUNDS * isolates;
        let start = Instant::now();
        for _ in 0..ROUNDS {
            for isolate in &mut owned {
                // SAFETY: entered and exited in pairs on the thread that owns
                // them, so the entered stack is unchanged after each turn.
                unsafe { isolate.enter() };
                call(isolate);
                unsafe { isolate.exit() };
            }
        }
        println!(
            "entered    {isolates} isolates, 1 thread        {:>9.2?}/turn (enter/exit, no Locker)",
            per(start.elapsed(), turns)
        );
        // `OwnedIsolate::drop` exits the isolate, so it must be the current one.
        while let Some(isolate) = owned.pop() {
            // SAFETY: entered once, so its drop's exit balances.
            unsafe { isolate.enter() };
            drop(isolate);
        }
    }

    // Shared isolates may be dropped from any thread; drop them here.
    drop(tenants);
}
