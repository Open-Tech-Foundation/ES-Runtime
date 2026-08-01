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
        .arg(format!("-e={}", code))
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

// ---- --allow-<name> ----------------------------------------------------------

#[test]
fn allow_grants_a_capability_back_under_deny_all() {
    let out = run(
        &["--deny-all", "--allow-net", "--allow-env"],
        "import { permissions } from 'runtime:process'; console.log(permissions.denied.join(','));",
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "read,write,imports,listen,run,signals");
}

#[test]
fn allow_imports_makes_deny_all_usable_for_a_multi_file_app() {
    // Without this, `--deny-all` is single-file only — which is why the allow
    // layer exists at all.
    write("allow_imports_helper.mjs", "export const v = 11;");
    let app = write(
        "allow_imports_app.mjs",
        "import { v } from './allow_imports_helper.mjs'; \
         import fs from 'runtime:fs'; \
         console.log('imported', v); \
         try { await fs.readDir('.'); console.log('NOT DENIED'); } \
         catch (e) { console.log('read still denied'); }",
    );
    let out = esrun()
        .arg("--deny-all")
        .arg("--allow-imports")
        .arg(&app)
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    assert!(s.contains("imported 11"), "{s}");
    assert!(s.contains("read still denied"), "{s}");
}

#[test]
fn allow_requires_deny_all() {
    // Rule 2: against the default baseline an allow is a no-op or a
    // contradiction, so it is rejected rather than silently doing nothing.
    let out = run(&["--allow-net"], "console.log('ran')");
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("--allow-net requires --deny-all"),
        "stderr: {}",
        stderr(&out)
    );
}

#[test]
fn allow_cannot_be_mixed_with_granular_denials() {
    // `--deny-read --allow-net` has no --deny-all, so rule 2 catches it: the two
    // directions never appear on one command line.
    let out = run(&["--deny-read", "--allow-net"], "console.log('ran')");
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("requires --deny-all"),
        "stderr: {}",
        stderr(&out)
    );
}

#[test]
fn allowing_everything_back_is_the_same_as_no_flags() {
    let flags = [
        "--deny-all",
        "--allow-read",
        "--allow-write",
        "--allow-imports",
        "--allow-net",
        "--allow-listen",
        "--allow-env",
        "--allow-run",
        "--allow-signals",
    ];
    let out = run(
        &flags,
        "import { permissions } from 'runtime:process'; console.log(JSON.stringify(permissions.denied));",
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "[]");
}

// ---- parser strictness -------------------------------------------------------

#[test]
fn a_value_on_a_permission_flag_is_rejected_not_ignored() {
    // The dangerous case: accepting and ignoring `=host` would leave the user
    // believing the run is scoped when it is wide open.
    let out = run(&["--deny-all", "--allow-net=example.com"], "console.log(1)");
    assert!(!out.status.success());
    let s = stderr(&out);
    assert!(s.contains("--allow-net takes no value"), "{s}");
    assert!(s.contains("not implemented yet"), "{s}");
}

#[test]
fn an_empty_value_on_a_no_value_flag_is_rejected() {
    let out = run(&["--deny-all", "--allow-net="], "console.log(1)");
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("--allow-net takes no value"),
        "stderr: {}",
        stderr(&out)
    );
}

#[test]
fn an_empty_value_on_a_value_flag_is_rejected() {
    let out = run(&["--timeout="], "console.log(1)");
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("has an empty value"),
        "stderr: {}",
        stderr(&out)
    );
}

#[test]
fn deny_all_takes_no_value() {
    let out = run(&["--deny-all=1"], "console.log(1)");
    assert!(!out.status.success());
    let s = stderr(&out);
    assert!(s.contains("--deny-all takes no value"), "{s}");
    // Not the scoping message — scoping could never apply to a mode switch.
    assert!(!s.contains("not implemented yet"), "{s}");
}

#[test]
fn a_space_separated_value_names_the_real_problem() {
    // `--allow-net example.com app.js` would otherwise take example.com as the
    // script and hand app.js to it — a "cannot read" three steps from the cause.
    let app = write("space_value.mjs", "console.log('ran');");
    let out = esrun()
        .arg("--deny-all")
        .arg("--allow-net")
        .arg("example.com")
        .arg(&app)
        .output()
        .unwrap();
    assert!(!out.status.success());
    let s = stderr(&out);
    assert!(s.contains("it follows --allow-net"), "{s}");
    assert!(s.contains("--flag=value"), "{s}");
}

#[test]
fn a_permission_flag_after_the_script_is_rejected() {
    // Silently passing it to the script would leave the user believing the run
    // is sandboxed when it is not.
    let app = write("after_script.mjs", "console.log('ran');");
    let out = esrun().arg(&app).arg("--deny-net").output().unwrap();
    assert!(!out.status.success());
    let s = stderr(&out);
    assert!(s.contains("appears after"), "{s}");
    assert!(s.contains("does nothing to the run"), "{s}");
}

#[test]
fn any_esrun_flag_after_the_script_is_rejected() {
    // Not just the permission flags: order is part of the grammar, and a
    // misplaced --timeout is a silent no-op too.
    let app = write("after_script_timeout.mjs", "console.log('ran');");
    let out = esrun().arg(&app).arg("--timeout=500").output().unwrap();
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("appears after"),
        "stderr: {}",
        stderr(&out)
    );
}

#[test]
fn a_scripts_own_unrelated_flags_still_pass_through() {
    // Only flags esrun itself knows are rejected; the script keeps its own.
    let app = write(
        "after_script_own.mjs",
        "import { args } from 'runtime:process'; console.log(args.join(' '));",
    );
    let out = esrun()
        .arg(&app)
        .arg("--verbose")
        .arg("--out=dist")
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "--verbose --out=dist");
}

#[test]
fn a_double_dash_lets_a_script_take_the_argument_itself() {
    let app = write(
        "after_script_escaped.mjs",
        "import { args } from 'runtime:process'; console.log(args.join(' '));",
    );
    let out = esrun()
        .arg(&app)
        .arg("--")
        .arg("--deny-net")
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("--deny-net"), "{}", stdout(&out));
}

#[test]
fn allow_all_points_at_the_default() {
    let out = run(&["--allow-all"], "console.log(1)");
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("there is no --allow-all"),
        "stderr: {}",
        stderr(&out)
    );
}

#[test]
fn one_grammar_applies_to_every_flag_not_just_permissions() {
    // The rule is the parser's, not the permission model's: `--timeout 500`
    // would leave `500` to be mistaken for the script to run.
    let out = run(&["--timeout", "500"], "console.log(1)");
    assert!(!out.status.success());
    let s = stderr(&out);
    assert!(s.contains("--timeout requires a value"), "{s}");
    assert!(s.contains("attached with '='"), "{s}");
}

#[test]
fn the_equals_form_works_for_every_value_flag() {
    let app = write("equals_form.mjs", "console.log('ran');");
    let envf = write("equals_form.env", "A=1\n");
    let out = esrun()
        .arg("--timeout=5000")
        .arg(format!("--env-file={}", envf.display()))
        .arg("--shutdown-grace=1000")
        .arg(&app)
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "ran");
}

#[test]
fn an_unknown_allow_name_is_rejected_with_the_vocabulary() {
    let out = run(&["--deny-all", "--allow-ffi"], "console.log(1)");
    assert!(!out.status.success());
    let s = stderr(&out);
    assert!(s.contains("unknown option: --allow-ffi"), "{s}");
    assert!(s.contains("--allow-read"), "{s}");
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

// ---- scoped grants: --allow-<name>=<list> ------------------------------------

#[test]
fn allow_env_narrows_the_environment_to_the_named_variables() {
    // The point of scoping `env` on a server: the process holds credentials the
    // guest has no business reading, and the guest needs two variables.
    let out = esrun()
        .env("ESRUN_SCOPE_PORT", "8080")
        .env("ESRUN_SCOPE_SECRET", "hunter2")
        .args(["--deny-all", "--allow-env=ESRUN_SCOPE_PORT"])
        .arg(
            "-e=import { env } from 'runtime:process'; \
             console.log('port', env.ESRUN_SCOPE_PORT); \
             console.log('secret', env.ESRUN_SCOPE_SECRET); \
             console.log('names', Object.keys(env).join(','));",
        )
        .output()
        .expect("spawn esrun");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    assert!(s.contains("port 8080"), "{s}");
    // Absent, not merely unreadable — the value and the *name* are both denied.
    assert!(s.contains("secret undefined"), "{s}");
    assert!(s.contains("names ESRUN_SCOPE_PORT\n"), "{s}");
}

#[test]
fn a_scoped_grant_still_reports_the_capability_as_granted() {
    // `has("env")` answers "is the door open", which it is — the narrowing is
    // the provider's, not the capability bit's. Reporting it as denied would be
    // the lie that matters: code would take the no-env path and never read the
    // variable it was explicitly given.
    let out = run(
        &["--deny-all", "--allow-env=HOME"],
        "import { permissions } from 'runtime:process'; \
         console.log(permissions.has('env'), permissions.denied.includes('env'));",
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "true false");
}

#[test]
#[cfg(unix)]
fn allow_run_spawns_the_named_program_and_refuses_the_rest() {
    let out = run(
        &["--deny-all", "--allow-run=echo"],
        "import { Command } from 'runtime:system'; \
         const ok = await new Command('echo', { args: ['allowed'] }).output(); \
         console.log(new TextDecoder().decode(ok.stdout).trim()); \
         try { await new Command('sh', { args: ['-c', 'echo pwned'] }).output(); \
               console.log('NO THROW'); } \
         catch (e) { console.log('refused', e.name, e.code); }",
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    assert!(s.contains("allowed"), "{s}");
    // A *scoped* denial, distinct from the capability denial that
    // `--deny-run` produces (ERR_CAPABILITY_DENIED): the guest has `run`, just
    // not this program.
    assert!(s.contains("refused"), "{s}");
    assert!(s.contains("ERR_PERMISSION_DENIED"), "{s}");
}

#[test]
#[cfg(unix)]
fn an_absolute_path_to_an_allowed_program_is_still_that_program() {
    // Matching is on the name as written *and* the resolved file name, so the
    // allowlist is not defeated by spelling the path out.
    let out = run(
        &["--deny-all", "--allow-run=echo"],
        "import { Command } from 'runtime:system'; \
         const out = await new Command('/bin/echo', { args: ['ok'] }).output(); \
         console.log(new TextDecoder().decode(out.stdout).trim());",
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "ok");
}

// ---- the value grammar (D38) -------------------------------------------------

#[test]
fn scope_entries_are_comma_separated_and_trimmed() {
    // `--allow-env="A, B"` and `--allow-env=A,B` are the same thing: quoting is
    // a shell convenience, not a second syntax.
    let out = esrun()
        .env("ESRUN_SCOPE_A", "1")
        .env("ESRUN_SCOPE_B", "2")
        .args(["--deny-all", "--allow-env= ESRUN_SCOPE_A , ESRUN_SCOPE_B "])
        .arg(
            "-e=import { env } from 'runtime:process'; \
             console.log(Object.keys(env).join(','));",
        )
        .output()
        .expect("spawn esrun");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "ESRUN_SCOPE_A,ESRUN_SCOPE_B");
}

#[test]
fn repeating_a_scoped_flag_unions_its_entries() {
    // Two flags that both add read in any order — no flag overrides another.
    let out = esrun()
        .env("ESRUN_SCOPE_A", "1")
        .env("ESRUN_SCOPE_B", "2")
        .args([
            "--deny-all",
            "--allow-env=ESRUN_SCOPE_A",
            "--allow-env=ESRUN_SCOPE_B",
        ])
        .arg(
            "-e=import { env } from 'runtime:process'; \
             console.log(Object.keys(env).join(','));",
        )
        .output()
        .expect("spawn esrun");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "ESRUN_SCOPE_A,ESRUN_SCOPE_B");
}

#[test]
fn an_empty_entry_in_a_scope_list_is_an_error() {
    // A stray comma is a typo, and a typo must not quietly change what the run
    // may reach.
    for value in ["--allow-env=A,,B", "--allow-env=A,", "--allow-env=,A"] {
        let out = run(&["--deny-all", value], "console.log(1)");
        assert!(!out.status.success(), "{value} should be rejected");
        assert!(stderr(&out).contains("has an empty entry"), "{value}");
    }
}

#[test]
fn an_empty_scope_list_is_an_error_not_an_empty_grant() {
    let out = run(&["--deny-all", "--allow-env="], "console.log(1)");
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("has an empty value"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn a_denial_takes_no_scope_list() {
    // Scoping narrows a grant; "everything except these hosts" is the other
    // direction, and a mode has exactly one (D38 rule 3).
    let out = run(&["--deny-run=git"], "console.log(1)");
    assert!(!out.status.success());
    let s = stderr(&out);
    assert!(s.contains("--deny-run takes no value"), "{s}");
    assert!(s.contains("--allow-run=<list>"), "{s}");
}

#[test]
fn scoping_an_unimplemented_capability_is_rejected_and_says_what_works() {
    let out = run(&["--deny-all", "--allow-read=/etc"], "console.log(1)");
    assert!(!out.status.success());
    let s = stderr(&out);
    assert!(s.contains("Scoping read is not implemented yet"), "{s}");
    assert!(s.contains("--allow-env=<list>"), "{s}");
    assert!(s.contains("--allow-run=<list>"), "{s}");
}

#[test]
fn granting_a_capability_whole_and_narrowed_at_once_is_an_error() {
    // Neither reading wins: the wider flag would widen a run the user asked to
    // narrow, the narrower one would ignore a flag they typed.
    for flags in [
        ["--allow-env", "--allow-env=HOME"],
        ["--allow-env=HOME", "--allow-env"],
    ] {
        let out = run(&["--deny-all", flags[0], flags[1]], "console.log(1)");
        assert!(!out.status.success(), "{flags:?} should be rejected");
        let s = stderr(&out);
        assert!(s.contains("disagree"), "{s}");
        assert!(s.contains("--allow-env=HOME"), "{s}");
    }
}

#[test]
fn a_scoped_allow_still_requires_deny_all() {
    // Rule 2 is unchanged by scoping: against "everything granted" there is
    // nothing for an allow to add.
    let out = run(&["--allow-env=HOME"], "console.log(1)");
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("--allow-env=HOME requires --deny-all"),
        "stderr: {}",
        stderr(&out)
    );
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
