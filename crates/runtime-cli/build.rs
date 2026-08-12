//! `esrun`'s build script exists for exactly one check (DECISIONS.md D59).
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
