//! Lightweight startup/throughput benchmark for the runtime (SPEC.md §6.8).
//!
//! Deliberately uses only `std::time` — no external benchmark framework — to
//! keep the supply-chain surface minimal. Run it (release is meaningful) with:
//!
//! ```text
//! cargo run --release --example bench -p es-runtime-default-providers
//! ```
//!
//! It reports the cost of constructing a runtime two ways — compiling+running
//! the prelude from scratch ([`Runtime::new`]) versus restoring it from a baked
//! startup snapshot ([`Runtime::with_snapshot`], DECISIONS.md D8) — and the
//! per-call cost of op dispatch.

// A benchmark's whole job is to print its numbers.
#![allow(clippy::print_stdout)]

use std::sync::Arc;
use std::time::Instant;

use es_runtime_common::Limits;
use es_runtime_default_providers::testing::{MockResponse, MockTransport};
use es_runtime_default_providers::{NullConsole, OsEntropy, SystemClock};
use es_runtime_runtime::{HostProviders, OpDecl, Runtime, V8Engine, Value};

fn providers() -> HostProviders {
    HostProviders::new(
        Arc::new(SystemClock::new()),
        Arc::new(NullConsole),
        Arc::new(MockTransport::constant(MockResponse::ok(""))),
        Arc::new(OsEntropy),
    )
}

fn fresh_runtime() -> Runtime {
    let engine = V8Engine::new(Limits::default()).expect("engine");
    Runtime::new(Box::new(engine), providers()).expect("runtime")
}

fn main() {
    const RUNS: u32 = 50;

    // Warm up V8 one-time initialization so it isn't charged to the first run.
    drop(fresh_runtime());

    // 1. Fresh construction: compile + run the whole prelude each time.
    let start = Instant::now();
    for _ in 0..RUNS {
        drop(fresh_runtime());
    }
    let fresh = start.elapsed() / RUNS;

    // 2. Snapshot restore: bake once, then deserialize the prelude state.
    let blob = Runtime::build_snapshot(&providers()).expect("build snapshot");
    let start = Instant::now();
    for _ in 0..RUNS {
        drop(Runtime::with_snapshot(blob.clone(), providers()).expect("restore"));
    }
    let restored = start.elapsed() / RUNS;

    // 3. Op-dispatch throughput: a no-op sync op called in a tight JS loop.
    let mut rt = fresh_runtime();
    rt.register_op(OpDecl::sync("nop", |_args| Ok(Value::Undefined)))
        .expect("register nop");
    const CALLS: u32 = 1_000_000;
    let start = Instant::now();
    rt.eval(&format!(
        "for (let i = 0; i < {CALLS}; i++) {{ __ops.nop(); }}"
    ))
    .expect("op loop");
    let per_call = start.elapsed() / CALLS;

    let speedup = fresh.as_secs_f64() / restored.as_secs_f64().max(f64::MIN_POSITIVE);
    println!("Runtime::new (fresh prelude):   {fresh:>10.2?} / runtime");
    println!("Runtime::with_snapshot:         {restored:>10.2?} / runtime");
    println!("  startup speedup:              {speedup:>10.1}x");
    println!("  snapshot blob size:           {:>10} bytes", blob.len());
    println!("op dispatch (nop):              {per_call:>10.2?} / call");
}
