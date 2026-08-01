//! End-to-end tests for `--deny-all` / `--deny-<name>` and the `permissions`
//! introspection they back (DECISIONS D38).
//!
//! These spawn the real `esrun` binary, so the flag parser, the capability set
//! it computes, the op-dispatch gate, and the `runtime:process` JS surface are
//! exercised together — the layers that must agree for a denial to be a denial.

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

fn esrun() -> Command {
    Command::new(env!("CARGO_BIN_EXE_esrun"))
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}
fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Runs `code` as an inline module under `flags`.
fn run(flags: &[&str], code: &str) -> Output {
    esrun()
        .args(flags)
        .arg("-e")
        .arg(code)
        .output()
        .expect("spawn esrun")
}

// ---- the default: nothing is denied -----------------------------------------

#[test]
fn every_capability_is_granted_by_default() {
    // esrun is permissive by default and stays that way; the sandbox is opt-in.
    let out = run(
        &[],
        "import { permissions } from 'runtime:process'; console.log(JSON.stringify(permissions.denied));",
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "[]");
}

// ---- --deny-all --------------------------------------------------------------

#[test]
fn deny_all_denies_every_host_facing_capability() {
    let out = run(
        &["--deny-all"],
        "import { permissions } from 'runtime:process'; console.log(permissions.denied.join(','));",
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(
        stdout(&out).trim(),
        "read,write,imports,net,listen,env,run,signals"
    );
}

#[test]
fn deny_all_still_runs_the_entry_file() {
    // The entry is read by the CLI before a runtime exists, so a fully denied
    // run still executes what the user actually named. This is the whole point
    // of the mode: compute freely, reach nothing.
    let app = write(
        "deny_all_entry.mjs",
        "let n = 0; for (let i = 0; i < 1000; i++) n += i; console.log('computed', n);",
    );
    let out = esrun().arg("--deny-all").arg(&app).output().unwrap();
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("computed 499500"), "{}", stdout(&out));
}

#[test]
fn deny_all_fails_a_local_import() {
    // `--deny-all` includes `--deny-imports`, so the module loader is closed:
    // a fully denied run is a single-file run.
    write("deny_all_helper.mjs", "export const v = 42;");
    let app = write(
        "deny_all_importer.mjs",
        "import { v } from './deny_all_helper.mjs'; console.log(v);",
    );
    let out = esrun().arg("--deny-all").arg(&app).output().unwrap();
    assert!(!out.status.success());
    assert!(stderr(&out).contains("imports"), "stderr: {}", stderr(&out));
}

#[test]
fn a_denied_operation_throws_not_allowed() {
    let out = run(
        &["--deny-all"],
        "import fs from 'runtime:fs'; \
         try { await fs.readDir('.'); console.log('NOT DENIED'); } \
         catch (e) { console.log(e.name, e.code); }",
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "NotAllowedError ERR_CAPABILITY_DENIED");
}

// ---- granular flags ----------------------------------------------------------

#[test]
fn a_granular_flag_denies_only_its_own_capability() {
    let out = run(
        &["--deny-net"],
        "import { permissions } from 'runtime:process'; \
         console.log(permissions.denied.join(','), permissions.has('read'));",
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "net true");
}

#[test]
fn granular_flags_accumulate() {
    let out = run(
        &["--deny-net", "--deny-run", "--deny-write"],
        "import { permissions } from 'runtime:process'; console.log(permissions.denied.join(','));",
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    // Reported in capability order, not the order the flags were given.
    assert_eq!(stdout(&out).trim(), "write,net,run");
}

#[test]
fn deny_read_leaves_imports_working() {
    // `read` is the `runtime:fs` surface; the module loader is `imports`. They
    // are separate capabilities, so denying one must not close the other.
    write("deny_read_helper.mjs", "export const v = 7;");
    let app = write(
        "deny_read_importer.mjs",
        "import { v } from './deny_read_helper.mjs'; \
         import fs from 'runtime:fs'; \
         console.log('imported', v); \
         try { await fs.readDir('.'); console.log('NOT DENIED'); } \
         catch (e) { console.log('read denied:', e.name); }",
    );
    let out = esrun().arg("--deny-read").arg(&app).output().unwrap();
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    assert!(s.contains("imported 7"), "{s}");
    assert!(s.contains("read denied: NotAllowedError"), "{s}");
}

// ---- the mutual-exclusion rule -----------------------------------------------

#[test]
fn deny_all_cannot_be_combined_with_a_granular_flag() {
    let out = run(&["--deny-all", "--deny-net"], "console.log('ran')");
    assert!(!out.status.success());
    let s = stderr(&out);
    assert!(
        s.contains("--deny-all cannot be combined with --deny-net"),
        "{s}"
    );
}

#[test]
fn the_combination_is_rejected_in_either_order() {
    let out = run(&["--deny-net", "--deny-all"], "console.log('ran')");
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("cannot be combined"),
        "stderr: {}",
        stderr(&out)
    );
}

#[test]
fn an_unknown_denial_name_is_rejected_with_the_vocabulary() {
    // Never silently ignored: an unrecognised --deny-* would otherwise read as
    // a sandbox that is not actually on.
    let out = run(&["--deny-ffi"], "console.log('ran')");
    assert!(!out.status.success());
    let s = stderr(&out);
    assert!(s.contains("unknown option: --deny-ffi"), "{s}");
    assert!(s.contains("--deny-read"), "{s}");
    assert!(s.contains("--deny-signals"), "{s}");
}

// ---- the D26 invariant: importing a runtime: module always works -------------

#[test]
fn runtime_modules_import_even_under_deny_all() {
    // The gate is the op, never the import (D26). Every built-in must load.
    let out = run(
        &["--deny-all"],
        "import 'runtime:process'; import 'runtime:path'; import 'runtime:fs'; \
         import 'runtime:net'; import 'runtime:http'; import 'runtime:websocket'; \
         import 'runtime:serialization'; import 'runtime:system'; import 'runtime:wasi'; \
         console.log('all imported');",
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "all imported");
}

#[test]
fn deny_env_leaves_exit_and_permissions_working() {
    // Denying `env` must deny reading the environment — not the unrelated
    // ability to exit, nor the ability to ask what is denied.
    let out = run(
        &["--deny-env"],
        "import { env, exit, permissions, platform } from 'runtime:process'; \
         console.log('platform', typeof platform === 'string'); \
         console.log('denied', permissions.denied.join(',')); \
         try { console.log(env.HOME); } catch (e) { console.log('env denied:', e.name); } \
         exit(3);",
    );
    assert_eq!(out.status.code(), Some(3), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    assert!(s.contains("platform true"), "{s}");
    assert!(s.contains("denied env"), "{s}");
    assert!(s.contains("env denied: NotAllowedError"), "{s}");
}

// ---- the permissions API itself ----------------------------------------------

#[test]
fn has_rejects_a_name_outside_the_vocabulary() {
    // A typo must not read as a denial and silently take the degraded path.
    let out = run(
        &[],
        "import { permissions } from 'runtime:process'; \
         try { permissions.has('nett'); console.log('NO THROW'); } \
         catch (e) { console.log(e.constructor.name); }",
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "TypeError");
}

#[test]
fn permissions_agrees_with_what_actually_throws() {
    // The API is only worth having if `has(x) === false` predicts the denial.
    let out = run(
        &["--deny-write"],
        "import { permissions } from 'runtime:process'; \
         import fs from 'runtime:fs'; \
         const allowed = permissions.has('write'); \
         let threw = false; \
         try { await fs.write('perm_probe.txt', 'x'); } catch { threw = true; } \
         console.log(allowed === !threw ? 'agrees' : 'DISAGREES');",
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "agrees");
}
