//! The compile-time switch for the V8 inspector (DECISIONS.md D59).
//!
//! `--cfg inspector` is set when — and only when — `ES_RUNTIME_INSPECTOR=1` is
//! in the environment of the `cargo` invocation that builds this crate:
//!
//! ```sh
//! ES_RUNTIME_INSPECTOR=1 cargo build --release -p es-runtime-dev-cli
//! ```
//!
//! **Why an environment variable and not a Cargo feature.** Cargo unifies
//! features across everything built in one invocation, so a feature declared by
//! `dev-cli` would also be enabled in the `es-runtime-cli` built beside it —
//! and an inspector port in the production binary is a total bypass of the
//! capability model it exists to enforce. A feature is *declared*, and so is on
//! forever; this variable exists only when a human types it. `runtime-cli`'s own
//! build script refuses to build while it is set, so one invocation can never
//! produce both an `esdev` with an inspector and an `esrun` at all.

// A build script's whole job is to talk to cargo, which listens on stdout.
#![allow(clippy::print_stdout)]

fn main() {
    println!("cargo::rustc-check-cfg=cfg(inspector)");
    println!("cargo::rerun-if-env-changed=ES_RUNTIME_INSPECTOR");
    if std::env::var("ES_RUNTIME_INSPECTOR").as_deref() == Ok("1") {
        println!("cargo::rustc-cfg=inspector");
    }
}
