//! What creating an `ArrayBuffer` costs, and how V8's array-buffer sweeping
//! changes it (DECISIONS D228, D227).
//!
//! Every byte result an op returns, every body chunk and every typed array a
//! program makes is an off-heap `ArrayBuffer`: rusty_v8 builds V8 with
//! `v8_typed_array_max_size_in_heap = 0`, so even an 11-byte `Uint8Array`
//! has a backing store. This times making them through the engine as it is
//! configured for a program (`V8Engine` with default limits), at four sizes
//! and two patterns:
//!
//! * **dropped** — each buffer is garbage as soon as it is made;
//! * **live 64** — the last 64 stay reachable, as chunks in flight do.
//!
//! V8 flags are process-wide and fixed before the first isolate, so each
//! setting is its own run:
//!
//! ```text
//! cargo run --release --example buffer_churn -p es-runtime-engine
//! cargo run --release --example buffer_churn -p es-runtime-engine -- --no-concurrent-array-buffer-sweeping
//! ```

// A benchmark's whole job is to print its numbers.
#![allow(clippy::print_stdout)]

use std::time::Instant;

use es_runtime_common::Limits;
use es_runtime_engine::{Engine, V8Engine};

const RUNS: u32 = 5;

/// Sizes in bytes, with how many buffers one run makes of each.
const SIZES: [(usize, u32); 4] = [
    (11, 1_000_000),
    (1024, 1_000_000),
    (65536, 100_000),
    (1 << 20, 5_000),
];

fn main() {
    let flags = std::env::args().skip(1).collect::<Vec<_>>().join(" ");
    if !flags.is_empty() {
        // Before the engine initializes V8, which reads flags once.
        v8::V8::set_flags_from_string(&flags);
    }
    println!(
        "flags: {}",
        if flags.is_empty() { "(none)" } else { &flags }
    );
    println!("{:<10} {:>12} {:>12}", "size", "dropped", "live 64");
    let mut engine = V8Engine::new(Limits::default()).expect("engine");
    for (size, count) in SIZES {
        let dropped = format!(
            "(() => {{ let a = 0; for (let i = 0; i < {count}; i++) a += new Uint8Array({size}).length; return a; }})()"
        );
        let live = format!(
            "(() => {{ const keep = new Array(64); let a = 0; for (let i = 0; i < {count}; i++) {{ const b = new Uint8Array({size}); keep[i & 63] = b; a += b.length; }} return a; }})()"
        );
        let mut time = |source: &str| {
            engine.eval(source).expect("warm-up");
            (0..RUNS)
                .map(|_| {
                    let start = Instant::now();
                    engine.eval(source).expect("run");
                    start.elapsed().as_secs_f64() * 1e9 / f64::from(count)
                })
                .fold(f64::INFINITY, f64::min)
        };
        let label = match size {
            s if s >= 1 << 20 => format!("{} MiB", s >> 20),
            s if s >= 1024 => format!("{} KiB", s >> 10),
            s => format!("{s} B"),
        };
        let d = time(&dropped);
        let l = time(&live);
        println!("{label:<10} {d:>9.0} ns {l:>9.0} ns");
    }
}
