//! What one op call costs, and how much of that is ours (DECISIONS D228).
//!
//! Each row is a JS loop calling one function, timed from Rust around the
//! `eval`, best of several runs. Three kinds of row:
//!
//! * **op** — a real op through the engine's dispatch (`__ops.<name>`): every
//!   argument marshaled into a `Value`, collected in a `Vec`, the op state
//!   borrowed, capabilities and the diagnostics gate checked, the result
//!   marshaled back.
//! * **v8 slow** — a bare V8 `FunctionTemplate` callback reading the same
//!   arguments straight off `FunctionCallbackArguments`. The floor any
//!   callback-based dispatch can reach; the gap to the op row is ours.
//! * **v8 fast** — the same function with a V8 fast-call overload, which
//!   optimized code calls directly. Only scalar signatures have one: V8 has no
//!   typed-array fast type any more.
//!
//! The loop is the floor too: `js loop` is the same loop calling a plain JS
//! function, subtracted from nothing — read the rows against it.
//!
//! ```text
//! cargo run --release --example op_crossing -p es-runtime-engine
//! ```
//!
//! It uses the `v8` crate directly for the floor rows; `engine` is the one
//! crate allowed to.

// A benchmark's whole job is to print its numbers.
#![allow(clippy::print_stdout)]

use std::time::Instant;

use es_runtime_common::Limits;
use es_runtime_engine::{Engine, OpDecl, V8Engine, Value};

const CALLS: u32 = 2_000_000;
const RUNS: u32 = 5;

/// The loop every row runs: `f` is bound to the function under test, `s` to a
/// short string, `b` to 1 KiB of bytes.
fn loop_source(call: &str) -> String {
    format!(
        "(() => {{ let acc = 0; for (let i = 0; i < {CALLS}; i++) {{ acc += {call}; }} return acc; }})()"
    )
}

const SETUP: &str = r#"
globalThis.s = "content-type";
globalThis.b = new Uint8Array(1024);
globalThis.jsNop = function () { return 0; };
globalThis.jsAdd = function (a, x) { return (a + x) >>> 0; };
"#;

/// Best per-call time over [`RUNS`] runs of `run`, after one untimed run so
/// the JIT has settled.
fn best(mut run: impl FnMut()) -> f64 {
    run();
    (0..RUNS)
        .map(|_| {
            let start = Instant::now();
            run();
            start.elapsed().as_secs_f64() * 1e9 / f64::from(CALLS)
        })
        .fold(f64::INFINITY, f64::min)
}

fn row(name: &str, ns_per_call: f64) {
    println!("{name:<28} {ns_per_call:>8.1} ns/call");
}

// --- engine dispatch -------------------------------------------------------

fn engine_rows() {
    let mut engine = V8Engine::new(Limits::default()).expect("engine");
    engine
        .register_op(OpDecl::sync("nop", |_| Ok(Value::Number(0.0))))
        .expect("nop");
    engine
        .register_op(OpDecl::sync("add_u32", |args| {
            let n = |i: usize| match args.get(i) {
                Some(Value::Number(n)) => *n as u32,
                _ => 0,
            };
            Ok(Value::Number(f64::from(n(0).wrapping_add(n(1)))))
        }))
        .expect("add_u32");
    engine
        .register_op(OpDecl::sync("str_len", |args| {
            Ok(Value::Number(
                args.first().and_then(Value::as_str).map_or(0, str::len) as f64,
            ))
        }))
        .expect("str_len");
    engine
        .register_op(OpDecl::sync("bytes_len", |args| {
            Ok(Value::Number(
                args.first()
                    .and_then(Value::as_bytes)
                    .map_or(0, <[u8]>::len) as f64,
            ))
        }))
        .expect("bytes_len");
    engine.eval(SETUP).expect("setup");

    let mut time = |name: &str, call: &str| {
        let source = loop_source(call);
        row(name, best(|| drop(engine.eval(&source).expect(name))));
    };
    time("js loop: f()", "jsNop()");
    time("js loop: f(i, 1)", "jsAdd(i, 1)");
    time("op: nop()", "__ops.nop()");
    time("op: add_u32(i, 1)", "__ops.add_u32(i, 1)");
    time("op: str_len(12 chars)", "__ops.str_len(s)");
    time("op: bytes_len(1 KiB)", "__ops.bytes_len(b)");
}

// --- bare V8 floors --------------------------------------------------------

fn slow_nop(_: &mut v8::PinScope, _: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    rv.set_int32(0);
}

fn slow_add(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let a = args.get(0).uint32_value(scope).unwrap_or(0);
    let b = args.get(1).uint32_value(scope).unwrap_or(0);
    rv.set_uint32(a.wrapping_add(b));
}

fn slow_str_len(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let len = args.get(0).to_rust_string_lossy(scope).len();
    rv.set_uint32(len as u32);
}

fn slow_bytes_len(
    _: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let len =
        v8::Local::<v8::ArrayBufferView>::try_from(args.get(0)).map_or(0, |v| v.byte_length());
    rv.set_uint32(len as u32);
}

fn fast_nop(_recv: v8::Local<v8::Object>) -> u32 {
    0
}

fn fast_add(_recv: v8::Local<v8::Object>, a: u32, b: u32) -> u32 {
    a.wrapping_add(b)
}

const FAST_NOP: &[v8::fast_api::CFunction] = &[v8::fast_api::CFunction::new(
    fast_nop as _,
    &v8::fast_api::CFunctionInfo::new(
        v8::fast_api::Type::Uint32.as_info(),
        &[v8::fast_api::Type::V8Value.as_info()],
        v8::fast_api::Int64Representation::Number,
    ),
)];

const FAST_ADD: &[v8::fast_api::CFunction] = &[v8::fast_api::CFunction::new(
    fast_add as _,
    &v8::fast_api::CFunctionInfo::new(
        v8::fast_api::Type::Uint32.as_info(),
        &[
            v8::fast_api::Type::V8Value.as_info(),
            v8::fast_api::Type::Uint32.as_info(),
            v8::fast_api::Type::Uint32.as_info(),
        ],
        v8::fast_api::Int64Representation::Number,
    ),
)];

fn v8_rows() {
    let isolate = &mut v8::Isolate::new(v8::CreateParams::default());
    v8::scope!(let scope, isolate);
    let context = v8::Context::new(scope, Default::default());
    let scope = &mut v8::ContextScope::new(scope, context);
    let global = context.global(scope);

    let set = |scope: &mut v8::PinScope, name: &str, t: v8::Local<v8::FunctionTemplate>| {
        let f = t.get_function(scope).expect("function");
        let key = v8::String::new(scope, name).expect("name");
        global.set(scope, key.into(), f.into()).expect("set");
    };
    let t = v8::FunctionTemplate::new(scope, slow_nop);
    set(scope, "slowNop", t);
    let t = v8::FunctionTemplate::new(scope, slow_add);
    set(scope, "slowAdd", t);
    let t = v8::FunctionTemplate::new(scope, slow_str_len);
    set(scope, "slowStrLen", t);
    let t = v8::FunctionTemplate::new(scope, slow_bytes_len);
    set(scope, "slowBytesLen", t);
    let t = v8::FunctionTemplate::builder(slow_nop).build_fast(scope, FAST_NOP);
    set(scope, "fastNop", t);
    let t = v8::FunctionTemplate::builder(slow_add).build_fast(scope, FAST_ADD);
    set(scope, "fastAdd", t);

    let run = |scope: &mut v8::PinScope, source: &str| {
        let code = v8::String::new(scope, source).expect("source");
        let script = v8::Script::compile(scope, code, None).expect("compile");
        script.run(scope).expect("run");
    };
    run(scope, SETUP);
    let mut time = |name: &str, call: &str| {
        let source = loop_source(call);
        row(name, best(|| run(scope, &source)));
    };
    time("v8 slow: nop()", "slowNop()");
    time("v8 slow: add(i, 1)", "slowAdd(i, 1)");
    time("v8 slow: str_len(12 chars)", "slowStrLen(s)");
    time("v8 slow: bytes_len(1 KiB)", "slowBytesLen(b)");
    time("v8 fast: nop()", "fastNop()");
    time("v8 fast: add(i, 1)", "fastAdd(i, 1)");
}

fn main() {
    println!("{CALLS} calls per run, best of {RUNS}\n");
    // The engine first: it initializes the V8 platform the bare rows reuse.
    engine_rows();
    println!();
    v8_rows();
}
