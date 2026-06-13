//! End-to-end ES module tests: run the real `esrun` binary against fixture
//! `.mjs` files (so the actual `FsModuleLoader` + real filesystem + process
//! exit codes are exercised, which the in-process runtime tests — using an
//! in-memory loader — do not). `CARGO_BIN_EXE_esrun` is set by Cargo and points
//! at the freshly built binary.

use std::path::PathBuf;
use std::process::{Command, Output};

/// A `Command` for the built `esrun` binary.
fn esrun() -> Command {
    Command::new(env!("CARGO_BIN_EXE_esrun"))
}

/// Absolute path to a file under `tests/fixtures/`.
fn fixture(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(rel)
}

fn run_file(rel: &str) -> Output {
    esrun()
        .arg(fixture(rel))
        .output()
        .expect("failed to spawn esrun")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}
fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn runs_a_module_with_imports_meta_and_tla() {
    let out = run_file("main.mjs");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let stdout = stdout(&out);
    assert!(stdout.contains("hello modules"), "{stdout}");
    // import.meta.url is a file: URL ending in the entry's path.
    assert!(stdout.contains("URL:file://"), "{stdout}");
    assert!(stdout.contains("main.mjs"), "{stdout}");
    // Top-level await resolved.
    assert!(stdout.contains("AWAITED:42"), "{stdout}");
}

#[test]
fn resolves_parent_directory_imports_on_disk() {
    let out = run_file("sub/nested.mjs");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("nested:hello modules"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn runs_an_inline_module_snippet() {
    let out = esrun()
        .arg("-e")
        .arg("console.log('inline', 6 * 7)")
        .output()
        .expect("spawn esrun");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("inline 42"), "{}", stdout(&out));
}

#[test]
fn inline_snippet_supports_top_level_await() {
    let out = esrun()
        .arg("-e")
        .arg("const x = await Promise.resolve(5); console.log('awaited', x)")
        .output()
        .expect("spawn esrun");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("awaited 5"), "{}", stdout(&out));
}

#[test]
fn top_level_throw_fails_with_uncaught_report() {
    let out = run_file("throws.mjs");
    assert!(!out.status.success(), "should exit non-zero");
    // Side effects before the throw still ran...
    assert!(stdout(&out).contains("before throw"), "{}", stdout(&out));
    // ...and the throw is reported once as Uncaught.
    let stderr = stderr(&out);
    assert!(stderr.contains("Uncaught"), "{stderr}");
    assert!(stderr.contains("fixture boom"), "{stderr}");
}

#[test]
fn missing_import_is_a_load_error() {
    let out = run_file("missing.mjs");
    assert!(!out.status.success(), "should exit non-zero");
    assert!(
        stderr(&out).contains("module loading failed"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn bare_specifier_is_rejected() {
    let out = run_file("bare.mjs");
    assert!(!out.status.success(), "should exit non-zero");
    assert!(
        stderr(&out).contains("bare module specifier"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn nonexistent_entry_file_errors_cleanly() {
    let out = run_file("no-such-file.mjs");
    assert!(!out.status.success(), "should exit non-zero");
    assert!(stderr(&out).contains("cannot read"), "{}", stderr(&out));
}

#[test]
fn version_flag_succeeds() {
    let out = esrun().arg("--version").output().expect("spawn esrun");
    assert!(out.status.success());
    assert!(stdout(&out).contains("esrun"), "{}", stdout(&out));
}
