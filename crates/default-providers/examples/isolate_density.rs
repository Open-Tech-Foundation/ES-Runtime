//! What one more isolate costs, when many share one OS thread.
//!
//! Phase 0 of the lightweight-isolate ("green thread") study: before designing
//! a pool of isolates over a few threads, measure the floor such a design
//! would stand on. Run it (release is meaningful) with:
//!
//! ```text
//! cargo run --release --example isolate_density -p es-runtime-default-providers -- [N]
//! ```
//!
//! Every isolate is kept alive and idle, so the numbers are what N isolates
//! cost on top of the process — no event loop, no `Worker` machinery. Two
//! kinds:
//!
//! * `bare` — `V8Engine::new`: an isolate and an empty context. V8's floor.
//! * `runtime` — `Runtime::with_snapshot_and_limits`: the full prelude restored
//!   from the snapshot every agent uses, ops bound, `post_snapshot_source` run.
//!   What a green thread with esrun's web globals would start from.
//!
//! Resident memory is read from `/proc/self/status`, so the memory rows are
//! Linux only.

// A benchmark's whole job is to print its numbers.
#![allow(clippy::print_stdout)]

use std::sync::Arc;
use std::time::Instant;

use es_runtime::{Engine, HostProviders, Runtime, V8Engine};
use es_runtime_common::Limits;
use es_runtime_default_providers::testing::{MockResponse, MockTransport};
use es_runtime_default_providers::{NullConsole, OsEntropy, SystemClock};

fn providers() -> HostProviders {
    HostProviders::new(
        Arc::new(SystemClock::new()),
        Arc::new(NullConsole),
        Arc::new(MockTransport::constant(MockResponse::ok(""))),
        Arc::new(OsEntropy),
    )
}

/// Resident set size in KiB, or 0 where `/proc` is not there.
fn rss_kib() -> u64 {
    let Ok(status) = std::fs::read_to_string("/proc/self/status") else {
        return 0;
    };
    status
        .lines()
        .find_map(|l| l.strip_prefix("VmRSS:"))
        .and_then(|v| v.trim().trim_end_matches("kB").trim().parse().ok())
        .unwrap_or(0)
}

/// Starts `n` isolates with `make`, each on a parked thread of its own, makes
/// each run a line of JS so its pages are really touched, and reports what
/// they cost together.
///
/// One thread each, because today's engine cannot hold two: an isolate is
/// entered when it is created and only the last one entered can run code
/// (`OwnedIsolate`), so a thread serving many isolates is an engine change —
/// what `isolate_locker` in `es-runtime-engine` measures. An idle parked thread
/// adds its touched stack pages (tens of KiB), not its 8 MiB reservation.
fn measure<T: 'static>(name: &str, n: usize, make: fn() -> T, call: fn(&mut T)) {
    use std::sync::mpsc;
    let before = rss_kib();
    let start = Instant::now();
    let (ready_tx, ready_rx) = mpsc::channel();
    let mut stops = Vec::with_capacity(n);
    let mut threads = Vec::with_capacity(n);
    for _ in 0..n {
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let ready = ready_tx.clone();
        stops.push(stop_tx);
        threads.push(std::thread::spawn(move || {
            let mut isolate = make();
            call(&mut isolate);
            ready.send(()).expect("ready");
            let _ = stop_rx.recv();
        }));
    }
    for _ in 0..n {
        ready_rx.recv().expect("isolate up");
    }
    let created = start.elapsed();
    let after = rss_kib();

    #[allow(clippy::cast_precision_loss)]
    let per_isolate_mib = (after.saturating_sub(before)) as f64 / 1024.0 / n as f64;
    println!(
        "{name:<8} n={n:<5} ready {:>9.2?}/isolate (wall, {} threads starting at once)   rss {per_isolate_mib:>5.2} MiB/isolate",
        created / u32::try_from(n).unwrap_or(u32::MAX),
        n,
    );
    drop(stops);
    for t in threads {
        t.join().expect("join");
    }
}

/// The snapshot every `runtime` isolate restores, shared as a binary's
/// `include_bytes!` blob is.
static BLOB: std::sync::OnceLock<&'static [u8]> = std::sync::OnceLock::new();

fn main() {
    let n: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(500);

    // Bake once, before any other isolate exists (V8 forbids building a
    // snapshot alongside them), and share one copy.
    let blob: &'static [u8] = Runtime::build_snapshot(&providers())
        .expect("build snapshot")
        .leak();
    let _ = BLOB.set(blob);
    // V8's one-time initialization is not charged to the first row.
    drop(V8Engine::new(Limits::default()).expect("warm-up"));

    println!(
        "process rss before any isolate: {:.1} MiB",
        rss_kib() as f64 / 1024.0
    );

    measure(
        "bare",
        n,
        || V8Engine::new(Limits::default()).expect("engine"),
        |engine| {
            engine
                .eval("globalThis.n = (globalThis.n | 0) + 1")
                .expect("eval");
        },
    );
    measure(
        "runtime",
        n,
        || {
            let blob = BLOB.get().copied().expect("snapshot built");
            Runtime::with_snapshot_and_limits(blob, Limits::default(), providers())
                .expect("runtime")
        },
        |runtime| {
            runtime
                .eval("globalThis.n = (globalThis.n | 0) + 1; new URL('https://a/b').pathname")
                .expect("eval");
        },
    );
}
