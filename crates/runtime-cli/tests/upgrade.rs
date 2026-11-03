//! End-to-end tests for `esrun upgrade`'s removal (DECISIONS D204).
//!
//! The command moved to `esdev upgrade`. What is pinned here is that the old
//! word gets an answer that names where it went, rather than being read as a
//! script path, and that the help no longer offers it.

use std::process::{Command, Output};

mod common;

fn esrun(args: &[&str]) -> Output {
    Command::new(common::esrun_bin())
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
fn upgrade_names_where_it_moved() {
    for args in [&["upgrade"][..], &["upgrade", "--dry-run"][..]] {
        let out = esrun(args);
        assert!(!out.status.success(), "{args:?} should not succeed");
        let text = stderr(&out);
        assert!(text.contains("esrun no longer upgrades itself"), "{text}");
        assert!(text.contains("esdev upgrade"), "{text}");
        assert!(text.contains("ESRUN_VERSION="), "{text}");
        assert!(text.contains("install.ps1"), "{text}");
    }
}

#[test]
fn help_no_longer_offers_upgrade() {
    let out = esrun(&["--help"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(!stdout(&out).contains("upgrade"), "{}", stdout(&out));
}
