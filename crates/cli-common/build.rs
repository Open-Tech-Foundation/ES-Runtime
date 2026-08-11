//! Builds the V8 startup snapshot at compile time and hands it to `lib.rs`
//! via `include_bytes!` — so every launch restores the prelude instead of
//! compiling and evaluating it (DECISIONS.md D8; ~2.3× cheaper runtime
//! construction, measured a few ms off process startup).
//!
//! It lives in `cli-common` rather than in a binary crate so that both `esrun`
//! and `esdev` share one build of the blob: it is expensive to produce and
//! identical for either, so building it twice would only cost compile time.
//!
//! The providers passed here are deterministic stand-ins: `build_snapshot`
//! consumes them only to register ops while snapshotting — Rust closures are
//! not serialized, so the blob captures only the op names/order and the
//! prelude's global state. The real providers are bound at launch.
//!
//! Limitation: the snapshot is built by running V8 on the *build host*, so
//! cross-compiling `es-runtime-cli` to a different architecture is not
//! supported by this script (it would need a target-run step).

use std::sync::Arc;

use es_runtime::{HostProviders, Runtime};
use es_runtime_default_providers::testing::{MockResponse, MockTransport, SeededEntropy};
use es_runtime_default_providers::{NullConsole, SystemClock};

fn main() {
    // The snapshot bakes the JS shell for every op, each holding the id it had
    // when the blob was built — and on a restored snapshot `register_op` binds
    // handlers by position and creates no shells (D8). A snapshot built from a
    // different op list than the binary registers therefore does not fail: it
    // *misbinds*, and `__ops.someName` quietly calls a different op.
    //
    // Cargo does not re-run this script when only `es-runtime`'s sources change
    // — recompiling a build-dependency is not by itself a reason to — so the op
    // registrations are named here explicitly. Verified the hard way: an op
    // added to `process_ops.rs` left the previous snapshot in place, and the
    // binary ran with the old op table.
    println!("cargo:rerun-if-changed=../runtime/src");
    println!("cargo:rerun-if-changed=../engine/src");
    let providers = HostProviders::new(
        Arc::new(SystemClock::new()),
        Arc::new(NullConsole),
        Arc::new(MockTransport::constant(MockResponse::ok(""))),
        Arc::new(SeededEntropy::new(0)),
    );
    let blob = Runtime::build_snapshot(&providers).expect("building the prelude snapshot");

    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"))
        .join("prelude.snapshot.bin");
    std::fs::write(&out, blob).expect("writing the prelude snapshot");
}
