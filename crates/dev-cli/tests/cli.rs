//! End-to-end tests for the `esdev` binary.
//!
//! The point of these is **parity**, not coverage of the runtime: `esdev` and
//! `esrun` share every line that decides how a run behaves
//! (`es-runtime-cli-common`), and the whole design rests on a program not being
//! able to behave one way under one binary and differently under the other. So
//! these spawn the real binary and assert that the shared surface — the module
//! load, the capability model, the D38 flag grammar, the error block — is the
//! same one `esrun` presents, plus the few things that are `esdev`'s own.

use std::path::PathBuf;
use std::process::{Command, Output};

fn temp(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name)
}

fn write(name: &str, contents: &str) -> PathBuf {
    let path = temp(name);
    std::fs::write(&path, contents).expect("write temp file");
    path
}

fn esdev() -> Command {
    Command::new(env!("CARGO_BIN_EXE_esdev"))
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}
fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn runs_a_module_file() {
    let app = write("run.mjs", "console.log('ran', 6 * 7);\n");
    let out = esdev().arg(&app).output().expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "ran 42");
}

#[test]
fn runs_an_inline_snippet() {
    let out = esdev()
        .arg("-e=console.log('inline')")
        .output()
        .expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "inline");
}

#[test]
fn top_level_await_and_imports_work() {
    let dep = write("dep.mjs", "export const answer = 42;\n");
    let app = write(
        "tla.mjs",
        &format!(
            "const m = await import({:?});\nconsole.log(m.answer);\n",
            dep.to_string_lossy()
        ),
    );
    let out = esdev().arg(&app).output().expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "42");
}

#[test]
fn reports_its_own_name_in_version_and_help() {
    let version = esdev().arg("--version").output().expect("spawn esdev");
    assert!(
        stdout(&version).starts_with("esdev "),
        "{}",
        stdout(&version)
    );

    let help = esdev().arg("--help").output().expect("spawn esdev");
    let text = stdout(&help);
    assert!(text.contains("esdev"), "{text}");
    // The boundary is part of the help, not just the docs: this binary is not a
    // deployment target and the usage text has to say so.
    assert!(text.contains("not a deployment target"), "{text}");
}

/// The capability model is the whole reason the two binaries are separate, so
/// `esdev` must enforce it exactly as `esrun` does — a dev binary that quietly
/// granted more would make every permission flag a developer tests with a lie.
#[test]
fn deny_all_denies_under_esdev_too() {
    let out = esdev()
        .arg("--deny-all")
        .arg("-e=const { env } = await import('runtime:process'); console.log(env.HOME);")
        .output()
        .expect("spawn esdev");
    assert!(!out.status.success());
    assert!(stderr(&out).contains("NotAllowedError"), "{}", stderr(&out));
}

#[test]
fn a_scoped_grant_narrows_and_still_reports_the_capability() {
    let app = write(
        "scoped.mjs",
        "import { env, permissions } from 'runtime:process';\n\
         console.log('has:', permissions.has('env'));\n\
         console.log('KEPT:', env.KEPT);\n\
         console.log('HIDDEN:', env.HIDDEN);\n",
    );
    let out = esdev()
        .arg("--deny-all")
        .arg("--allow-env=KEPT")
        .arg(&app)
        .env("KEPT", "yes")
        .env("HIDDEN", "no")
        .output()
        .expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("has: true"), "{text}");
    assert!(text.contains("KEPT: yes"), "{text}");
    assert!(text.contains("HIDDEN: undefined"), "{text}");
}

/// D38 rule 2, enforced by the shared grammar rather than by a copy of it.
#[test]
fn allow_without_deny_all_is_rejected() {
    let out = esdev()
        .arg("--allow-net")
        .arg("-e=1")
        .output()
        .expect("spawn esdev");
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("requires --deny-all"),
        "{}",
        stderr(&out)
    );
}

/// D38 rule 1.
#[test]
fn deny_all_cannot_be_combined_with_a_named_denial() {
    let out = esdev()
        .arg("--deny-all")
        .arg("--deny-net")
        .arg("-e=1")
        .output()
        .expect("spawn esdev");
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("cannot be combined"),
        "{}",
        stderr(&out)
    );
}

/// The single grammar rule: a value attaches with `=`, never as the next word.
#[test]
fn a_separated_value_is_rejected_rather_than_read() {
    let out = esdev()
        .arg("--timeout")
        .arg("500")
        .arg("-e=1")
        .output()
        .expect("spawn esdev");
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("requires a value, attached with '='"),
        "{}",
        stderr(&out)
    );
}

/// Order is part of the grammar, and a silently-ignored `--deny-*` is a security
/// failure rather than a no-op — so it is an error under `esdev` as well.
#[test]
fn a_flag_after_the_script_is_rejected() {
    let app = write("after.mjs", "console.log('x');\n");
    let out = esdev()
        .arg(&app)
        .arg("--deny-net")
        .output()
        .expect("spawn esdev");
    assert!(!out.status.success());
    let text = stderr(&out);
    assert!(text.contains("appears after"), "{text}");
    assert!(
        text.contains("esdev's flags come before the script"),
        "{text}"
    );
}

#[test]
fn a_script_argument_after_a_double_dash_is_left_alone() {
    let app = write(
        "args.mjs",
        "import { args } from 'runtime:process';\nconsole.log(args.join(','));\n",
    );
    let out = esdev()
        .arg(&app)
        .arg("--")
        .arg("--deny-net")
        .output()
        .expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("--deny-net"), "{}", stdout(&out));
}

#[test]
fn the_watchdog_stops_a_runaway() {
    let out = esdev()
        .arg("--timeout=200")
        .arg("-e=while (true) {}")
        .output()
        .expect("spawn esdev");
    assert!(!out.status.success());
    assert!(stderr(&out).contains("timed out"), "{}", stderr(&out));
}

#[test]
fn max_heap_of_zero_is_rejected() {
    let out = esdev()
        .arg("--max-heap=0")
        .arg("-e=1")
        .output()
        .expect("spawn esdev");
    assert!(!out.status.success());
    assert!(stderr(&out).contains("no heap at all"), "{}", stderr(&out));
}

/// `esrun`'s subcommands are deliberately not `esdev`'s: shipping `upgrade` in
/// two binaries would give a machine two things to keep in step, and `types`
/// belongs with the runtime that documents them.
#[test]
fn esrun_only_subcommands_are_not_silently_accepted() {
    for subcommand in ["upgrade", "types"] {
        let out = esdev().arg(subcommand).output().expect("spawn esdev");
        // Treated as a path (it is a bare word), so it fails as a missing file
        // rather than doing something surprising.
        assert!(!out.status.success(), "{subcommand} should not succeed");
        assert!(
            stderr(&out).contains("cannot read") || stderr(&out).contains("cannot resolve"),
            "{subcommand}: {}",
            stderr(&out)
        );
    }
}

/// An uncaught error is one block, and it names the file — the Phase 13 error
/// model, reached through the shared printer rather than a second copy of it.
#[test]
fn an_uncaught_error_is_reported_as_one_block() {
    let app = write("throws.mjs", "throw new Error('boom');\n");
    let out = esdev().arg(&app).output().expect("spawn esdev");
    assert!(!out.status.success());
    let text = stderr(&out);
    assert!(text.contains("uncaught exception in"), "{text}");
    assert!(text.contains("boom"), "{text}");
}
