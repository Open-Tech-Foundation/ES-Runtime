//! `esrun`'s build script: one check (DECISIONS.md D59), and the main thread's
//! stack on Windows (D189).
//!
//! `ES_RUNTIME_INSPECTOR=1` compiles the V8 inspector into `es-runtime-engine`,
//! which both binaries sit on. Set it for a build that also produces `esrun` and
//! you would get a production binary with a debugger port compiled in — a total
//! bypass of the capability model it exists to enforce, arrived at silently.
//!
//! So this refuses to build at all while it is set. The separation is then not a
//! rule to remember but something a build fails on, and the honest way to get an
//! `esdev` with an inspector is the one the error names: build it alone, in an
//! invocation `esrun` is not part of.

// A build script's whole job is to talk to cargo, which listens on stdout.
#![allow(clippy::print_stdout)]

fn main() {
    reserve_main_stack();
    println!("cargo::rerun-if-env-changed=ES_RUNTIME_INSPECTOR");
    if std::env::var("ES_RUNTIME_INSPECTOR").as_deref() == Ok("1") {
        println!(
            "cargo::error=ES_RUNTIME_INSPECTOR=1 is set, and esrun must never be built with it: \
             it compiles a debugger port into the binary whose whole point is that a deployment \
             cannot be debugged into. Build esdev on its own instead — \
             ES_RUNTIME_INSPECTOR=1 cargo build --release -p es-runtime-dev-cli — and build esrun \
             without it."
        );
    }
}

/// Reserves 8 MiB of stack for the main thread on Windows (DECISIONS D189).
/// The JS thread is the main thread, and MSVC's default is 1 MiB: less than
/// V8's own JS limit leaves room for, so deep JS or a deep document in a
/// native codec would abort the process instead of raising an error. Linux and
/// macOS already give the main thread 8 MiB; Node reserves the same. It is
/// address space, not memory — pages are committed as the stack grows.
fn reserve_main_stack() {
    const STACK: u64 = 8 * 1024 * 1024;
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    match std::env::var("CARGO_CFG_TARGET_ENV").as_deref() {
        Ok("msvc") => println!("cargo::rustc-link-arg-bins=/STACK:{STACK}"),
        Ok("gnu") => println!("cargo::rustc-link-arg-bins=-Wl,--stack,{STACK}"),
        _ => {}
    }
}
