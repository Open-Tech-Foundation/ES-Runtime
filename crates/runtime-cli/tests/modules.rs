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
fn uninstalled_bare_package_is_not_found() {
    // bare.mjs imports "lodash", which is not in any node_modules here.
    let out = run_file("bare.mjs");
    assert!(!out.status.success(), "should exit non-zero");
    assert!(
        stderr(&out).contains("cannot find package"),
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
fn resolves_a_bare_esm_package_from_node_modules() {
    let out = run_file("uses-package.mjs");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("hi world from greeter"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn rejects_a_commonjs_package() {
    let out = run_file("uses-cjs-package.mjs");
    assert!(!out.status.success(), "should exit non-zero");
    assert!(stderr(&out).contains("CommonJS"), "{}", stderr(&out));
}

#[test]
fn dynamic_import_resolves_relative_and_node_modules() {
    let out = run_file("dynamic.mjs");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let stdout = stdout(&out);
    assert!(stdout.contains("hello modules"), "{stdout}");
    assert!(stdout.contains("hi dynamic from greeter"), "{stdout}");
}

#[test]
fn all_esm_export_import_patterns_work() {
    // esm/consumer.mjs exercises every standardized export/import form against
    // the export fixtures and throws on any mismatch.
    let out = run_file("esm/consumer.mjs");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("ESM-SUITE-OK"), "{}", stdout(&out));
}

#[test]
fn node_modules_export_patterns_work() {
    // A node_modules package with an exports map: ".", a subpath, and a wildcard
    // subpath, with named + default exports and an internal re-export.
    let out = run_file("esm/consumer-pkg.mjs");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("PKG-SUITE-OK"), "{}", stdout(&out));
}

#[test]
fn runtime_process_exposes_env_args_platform_cwd() {
    let out = esrun()
        .env("ESRUN_TEST_VAR", "hello")
        .arg("-e")
        .arg(
            "import { env, args, platform, arch, cwd } from 'runtime:process'; \
             console.log(env.ESRUN_TEST_VAR, platform, arch, args.join(','), typeof cwd());",
        )
        .arg("alpha")
        .arg("beta")
        .output()
        .expect("spawn esrun");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    assert!(s.contains("hello"), "env: {s}");
    // platform is the OS-native std value (linux/macos/windows).
    assert!(
        s.contains("linux") || s.contains("macos") || s.contains("windows"),
        "platform: {s}"
    );
    // arch is the OS-native std value (x86_64/aarch64/arm/...).
    assert!(
        s.contains("x86_64") || s.contains("aarch64") || s.contains("arm"),
        "arch: {s}"
    );
    assert!(s.contains("alpha,beta"), "args: {s}"); // user args only, in order
    assert!(s.contains("string"), "cwd: {s}");
}

#[test]
fn runtime_process_exit_sets_exit_code() {
    let out = esrun()
        .arg("-e")
        .arg("import { exit } from 'runtime:process'; console.log('before'); exit(5); console.log('after');")
        .output()
        .expect("spawn esrun");
    assert_eq!(out.status.code(), Some(5), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("before"), "{}", stdout(&out));
    assert!(
        !stdout(&out).contains("after"),
        "exit did not halt: {}",
        stdout(&out)
    );
}

#[test]
fn unknown_runtime_builtin_module_errors() {
    let out = esrun()
        .arg("-e")
        .arg("import 'runtime:nope';")
        .output()
        .expect("spawn esrun");
    assert!(!out.status.success(), "should exit non-zero");
    assert!(
        stderr(&out).contains("unknown built-in module"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn version_flag_succeeds() {
    let out = esrun().arg("--version").output().expect("spawn esrun");
    assert!(out.status.success());
    assert!(stdout(&out).contains("esrun"), "{}", stdout(&out));
}
