//! Shared harness paths for the `esrun` CLI test suites.
//!
//! Every path the suites need comes through here so the same binaries run
//! somewhere Cargo's paths do not exist — a Windows guest via winrun — by
//! pointing three environment variables at the staged tree instead:
//!
//! - `ESRUN_TEST_BIN`: the `esrun` binary under test.
//! - `ESRUN_TEST_TMPDIR`: a writable scratch directory.
//! - `ESRUN_TEST_MANIFEST`: a directory mirroring this crate, i.e. holding
//!   `tests/fixtures`, so every `.join("tests/fixtures/...")` below keeps
//!   working verbatim.
//!
//! With nothing set (plain `cargo test`) each falls back to the compile-time
//! Cargo value, and behaviour is exactly as before.

#![allow(dead_code)]

use std::path::PathBuf;

fn var(name: &str, fallback: &str) -> PathBuf {
    std::env::var_os(name)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(fallback))
}

/// The `esrun` binary under test.
pub fn esrun_bin() -> PathBuf {
    var("ESRUN_TEST_BIN", env!("CARGO_BIN_EXE_esrun"))
}

/// A writable scratch directory.
pub fn tmpdir() -> PathBuf {
    var("ESRUN_TEST_TMPDIR", env!("CARGO_TARGET_TMPDIR"))
}

/// This crate's directory, or its staged mirror.
pub fn manifest_dir() -> PathBuf {
    var("ESRUN_TEST_MANIFEST", env!("CARGO_MANIFEST_DIR"))
}

/// The `tests/fixtures` tree.
pub fn fixtures() -> PathBuf {
    manifest_dir().join("tests/fixtures")
}
