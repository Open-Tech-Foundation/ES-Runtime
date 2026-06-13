//! Builds the V8 startup snapshot at compile time and hands it to `main.rs`
//! via `include_bytes!` — so every `esrun` launch restores the prelude instead
//! of compiling and evaluating it (DECISIONS.md D8; ~2.3× cheaper runtime
//! construction, measured a few ms off process startup).
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
    // Rebuilds are driven by cargo's fingerprint of the build-dependencies
    // themselves (a prelude or runtime change recompiles `es-runtime`, which
    // re-triggers this script); no rerun-if-changed needed beyond the default.
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
