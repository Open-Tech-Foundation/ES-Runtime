//! End-to-end tests for `esrun upgrade`'s argument handling.
//!
//! The upgrade itself reaches the network, so what is pinned here is the
//! grammar, offline: `--help` prints, `--dry-run` is accepted (the live check
//! is exercised by hand), and anything else is refused by name. Refusing
//! matters because the old behavior ran the replacement past a stray argument
//! — `esrun upgrade --help` used to upgrade the binary.

use std::process::{Command, Output};

fn esrun(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_esrun"))
        .args(args)
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
fn upgrade_answers_help() {
    for flag in ["--help", "-h"] {
        let out = esrun(&["upgrade", flag]);
        assert!(out.status.success(), "{flag}: {}", stderr(&out));
        let text = stdout(&out);
        assert!(text.contains("esrun upgrade"), "{text}");
        assert!(text.contains("--dry-run"), "{text}");
    }
}

#[test]
fn upgrade_refuses_stray_arguments() {
    for args in [["upgrade", "0.24.0"], ["upgrade", "--dry-run=yes"]] {
        let out = esrun(&args);
        assert!(!out.status.success(), "{args:?} should not succeed");
        assert!(
            stderr(&out).contains("takes no arguments"),
            "{args:?}: {}",
            stderr(&out)
        );
    }
}

#[test]
fn top_level_help_advertises_dry_run() {
    let out = esrun(&["--help"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("upgrade [--dry-run]"),
        "{}",
        stdout(&out)
    );
}
