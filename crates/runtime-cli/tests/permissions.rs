//! End-to-end tests for the `--allow-*` / `--deny-*` grammar and the
//! `permissions` introspection it backs (DECISIONS D38, D65).
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

// ---- the default: nothing is granted -----------------------------------------

#[test]
fn nothing_is_granted_by_default() {
    // esrun is deny-by-default (D65): a run reaches what the command line named
    // and nothing else, so a line with no permission flag denies all nine.
    let out = run(
        &[],
        "import { permissions } from 'runtime:process'; console.log(permissions.denied.join(','));",
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(
        stdout(&out).trim(),
        "read,write,imports,net,listen,env,run,signals,workers"
    );
}

#[test]
fn allow_all_grants_every_capability() {
    // The other end of the same line, and the only way to get the pre-D65
    // default back.
    let out = run(
        &["--allow-all"],
        "import { permissions } from 'runtime:process'; console.log(JSON.stringify(permissions.denied));",
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "[]");

    // `-A` is the same flag.
    let short = run(
        &["-A"],
        "import { permissions } from 'runtime:process'; console.log(JSON.stringify(permissions.denied));",
    );
    assert_eq!(stdout(&short).trim(), "[]");
}

#[test]
fn deny_all_restates_the_default() {
    // Kept, and deliberately a no-op: a deploy line that says `--deny-all`
    // outright is stating the grant it expects rather than trusting the reader
    // to know which way this binary defaults.
    let bare = run(
        &[],
        "import { permissions } from 'runtime:process'; console.log(permissions.denied.join(','));",
    );
    let explicit = run(
        &["--deny-all"],
        "import { permissions } from 'runtime:process'; console.log(permissions.denied.join(','));",
    );
    assert!(explicit.status.success(), "stderr: {}", stderr(&explicit));
    assert_eq!(stdout(&bare), stdout(&explicit));
}

#[test]
fn allow_all_and_deny_all_are_mutually_exclusive() {
    let out = run(&["--allow-all", "--deny-all"], "console.log(1)");
    assert!(!out.status.success());
    let err = stderr(&out);
    assert!(err.contains("--allow-all and --deny-all disagree"), "{err}");
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
        "read,write,imports,net,listen,env,run,signals,workers"
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

#[test]
fn import_meta_resolve_is_denied_with_the_loader() {
    // `resolve` reads package.json files to answer, so a run denied the loader
    // must not get it as a filesystem-probing oracle: it fails like an import
    // rather than reporting what is or is not installed (D41).
    let out = run(
        &["--deny-all"],
        "try { console.log(import.meta.resolve('greeter')); } \
         catch (e) { console.log(e.name, e.code ?? ''); }",
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let printed = stdout(&out);
    assert!(
        printed.contains("NotAllowedError") || printed.contains("TypeError"),
        "a denied run must refuse to resolve: {printed}"
    );
    assert!(
        !printed.contains("file://"),
        "a denied run must not learn where a package lives: {printed}"
    );
}

// ---- granular flags ----------------------------------------------------------

#[test]
fn a_granular_flag_denies_only_its_own_capability() {
    let out = run(
        &["--allow-all", "--deny-net"],
        "import { permissions } from 'runtime:process'; \
         console.log(permissions.denied.join(','), permissions.has('read'));",
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "net true");
}

#[test]
fn workers_is_a_permission_name_like_any_other() {
    // `--deny-workers` denies it and `new Worker(url, { permissions })` takes
    // it, so `has()` has to answer for it too. It did not: the capability
    // arrived with workers (D48) and the introspection list was left at eight
    // names, so a program asking the supported way got a TypeError telling it
    // `workers` is not a permission — while `denied` listed it, since that
    // comes from the Rust side.
    let out = run(
        &["--allow-all", "--deny-workers"],
        "import { permissions } from 'runtime:process'; \
         console.log(permissions.has('workers'), permissions.denied.join(','));",
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "false workers");

    let granted = run(
        &["--allow-workers"],
        "import { permissions } from 'runtime:process'; \
         console.log(permissions.has('workers'));",
    );
    assert_eq!(stdout(&granted).trim(), "true");
}

#[test]
fn granular_flags_accumulate() {
    let out = run(
        &["--allow-all", "--deny-net", "--deny-run", "--deny-write"],
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
    let out = esrun()
        .args(["--allow-all", "--deny-read"])
        .arg(&app)
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    assert!(s.contains("imported 7"), "{s}");
    assert!(s.contains("read denied: NotAllowedError"), "{s}");
}

#[test]
fn a_datagram_socket_is_checked_against_both_grants() {
    // UDP is the one surface that needs two capabilities (D58): binding takes a
    // port, which is `listen`, and sending reaches a peer, which is `net`. The
    // two halves are asserted separately, because a socket that could send
    // under `listen` alone — or bind under `net` alone — would be a hole in
    // whichever grant was skipped.
    let bind_and_send = "import { bind } from 'runtime:net';\
        let r = '';\
        try {\
          const s = bind({ hostname: '127.0.0.1', port: 0 });\
          await s.addr; r += 'BIND:ok';\
          try { await s.send(new Uint8Array([1]), '127.0.0.1:9'); r += ':SEND:ok'; }\
          catch (e) { r += ':SEND:' + e.name; }\
          await s.close();\
        } catch (e) { r += 'BIND:' + e.name; }\
        console.log(r);";

    // `net` denied: the port is still bindable, the datagram is not sendable.
    let out = run(&["--allow-all", "--deny-net"], bind_and_send);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "BIND:ok:SEND:NotAllowedError");

    // `listen` denied: there is no socket to send from in the first place.
    let out = run(&["--allow-all", "--deny-listen"], bind_and_send);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "BIND:NotAllowedError");

    // Both granted, and named: it works, which is what makes the two denials
    // above evidence of the gate rather than of a broken example.
    let out = run(&["--allow-net", "--allow-listen"], bind_and_send);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "BIND:ok:SEND:ok");
}

#[test]
fn a_datagram_destination_is_scoped_by_allow_net() {
    // …and the address lists apply too: `--allow-listen` decides where a socket
    // may bind, `--allow-net` where each datagram may go — checked per send,
    // since one socket sends to as many peers as it likes.
    let code = "import { bind } from 'runtime:net';\
        const s = bind({ hostname: '127.0.0.1', port: 0 });\
        await s.addr;\
        const to = async (address) => {\
          try { await s.send(new Uint8Array([1]), address); return 'ok'; }\
          catch (e) { return e.code; }\
        };\
        console.log(await to('127.0.0.1:9'), await to('127.0.0.1:53'));\
        await s.close();";
    let out = run(
        &[
            "--deny-all",
            "--allow-listen=127.0.0.1:0",
            "--allow-net=127.0.0.1:9",
        ],
        code,
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "ok ERR_PERMISSION_DENIED");

    // The bind is the listen list's business, and an address outside it is
    // refused before the port is claimed.
    let out = run(
        &["--deny-all", "--allow-listen=127.0.0.1:7070", "--allow-net"],
        "import { bind } from 'runtime:net';\
         try { await bind({ hostname: '127.0.0.1', port: 0 }).addr; console.log('bound'); }\
         catch (e) { console.log(e.code); }",
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "ERR_PERMISSION_DENIED");
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
    assert_eq!(
        stdout(&out).trim(),
        "read,write,imports,listen,run,signals,workers"
    );
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
fn deny_requires_allow_all() {
    // Rule 2, now pointing the other way (D65): against the default baseline —
    // nothing granted — a denial is a no-op or a contradiction, so it is
    // rejected rather than silently doing nothing.
    let out = run(&["--deny-net"], "console.log('ran')");
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("--deny-net requires --allow-all"),
        "stderr: {}",
        stderr(&out)
    );
}

#[test]
fn allow_cannot_be_mixed_with_granular_denials() {
    // `--allow-net --deny-read` has no --allow-all, so rule 2 catches it: the
    // two directions never appear on one command line.
    let out = run(&["--allow-net", "--deny-read"], "console.log('ran')");
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("requires --allow-all"),
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
        "--allow-workers",
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
fn a_value_on_a_flag_that_takes_none_is_named_not_swallowed() {
    // Every permission name is scopable now, so the flag that still has to
    // reject a value is one that could never carry one. The rejection path for
    // an unenforceable *permission* value stays in the parser for the next
    // capability that lands (see `Permissions::record`).
    let out = run(&["--env-override=yes"], "console.log(1)");
    assert!(!out.status.success());
    let s = stderr(&out);
    assert!(s.contains("--env-override takes no value"), "{s}");
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
fn an_allow_beside_allow_all_is_rejected() {
    // The permissive mode's own rule 2: --allow-all already granted everything
    // --allow-net would, so the pair is a contradiction rather than a narrowing.
    let out = run(&["--allow-all", "--allow-net"], "console.log(1)");
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("--allow-all cannot be combined with --allow-net"),
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
         import 'runtime:serialization'; import 'runtime:hashing'; \
         import 'runtime:system'; import 'runtime:wasi'; \
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
        &["--allow-all", "--deny-env"],
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

/// A one-shot HTTP server on an ephemeral port, for the address tests. Returns
/// the port and the thread, so a test can join it.
fn one_shot_http(response: &'static str) -> (u16, std::thread::JoinHandle<()>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
    let port = listener.local_addr().unwrap().port();
    let handle = std::thread::spawn(move || {
        if let Ok((mut sock, _)) = listener.accept() {
            let mut buf = [0u8; 1024];
            let _ = sock.read(&mut buf);
            let _ = sock.write_all(response.as_bytes());
            let _ = sock.flush();
        }
    });
    (port, handle)
}

/// A port the OS has just confirmed is free.
fn free_port() -> u16 {
    std::net::TcpListener::bind(("127.0.0.1", 0))
        .expect("bind")
        .local_addr()
        .unwrap()
        .port()
}

#[test]
fn allow_net_reaches_the_named_host_and_refuses_the_rest() {
    let (port, server) = one_shot_http("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhi");
    let out = run(
        &["--deny-all", &format!("--allow-net=127.0.0.1:{port}")],
        &format!(
            "const ok = await fetch('http://127.0.0.1:{port}/'); \
             console.log('allowed', ok.status, await ok.text()); \
             try {{ await fetch('http://127.0.0.1:1/'); console.log('NO THROW'); }} \
             catch (e) {{ console.log('refused', e.cause?.code ?? e.code); }}"
        ),
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    assert!(s.contains("allowed 200 hi"), "{s}");
    assert!(s.contains("refused"), "{s}");
    assert!(s.contains("ERR_PERMISSION_DENIED"), "{s}");
    server.join().unwrap();
}

#[test]
fn allow_net_is_enforced_on_every_redirect_hop() {
    // The reason this capability was the hard one: the redirect is followed by
    // the HTTP client on a policy set once per client, so an allowlist checked
    // only at the front door would hand the guest a denied host's response.
    let (port, server) = one_shot_http(
        "HTTP/1.1 302 Found\r\nLocation: http://169.254.169.254/latest/meta-data/\r\nContent-Length: 0\r\n\r\n",
    );
    let out = run(
        &["--deny-all", &format!("--allow-net=127.0.0.1:{port}")],
        &format!(
            "try {{ await fetch('http://127.0.0.1:{port}/'); console.log('NO THROW'); }} \
             catch (e) {{ console.log('refused', e.cause?.message ?? e.message); }}"
        ),
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    assert!(s.contains("refused"), "{s}");
    assert!(s.contains("169.254.169.254"), "{s}");
    server.join().unwrap();
}

#[test]
fn allow_listen_binds_the_named_address_and_refuses_the_rest() {
    let port = free_port();
    let out = run(
        &["--deny-all", &format!("--allow-listen=127.0.0.1:{port}")],
        &format!(
            "import net from 'runtime:net'; \
             const l = net.listen({{ port: {port}, hostname: '127.0.0.1' }}); \
             console.log('bound'); \
             await l.close(); \
             try {{ await net.listen({{ port: {port}, hostname: '0.0.0.0' }}).addr; \
                    console.log('NO THROW'); }} \
             catch (e) {{ console.log('refused', e.code); }}"
        ),
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    assert!(s.contains("bound"), "{s}");
    assert!(s.contains("refused ERR_PERMISSION_DENIED"), "{s}");
}

#[test]
fn a_bare_port_in_a_listen_list_allows_any_interface() {
    let port = free_port();
    let out = run(
        &["--deny-all", &format!("--allow-listen={port}")],
        &format!(
            "import net from 'runtime:net'; \
             const l = net.listen({{ port: {port}, hostname: '0.0.0.0' }}); \
             console.log('bound'); await l.close();"
        ),
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "bound");
}

#[test]
fn net_and_listen_are_separate_lists() {
    // Reaching out and being reachable are separate capabilities, so an
    // address allowed for one says nothing about the other.
    let port = free_port();
    let out = run(
        &[
            "--deny-all",
            &format!("--allow-listen=127.0.0.1:{port}"),
            "--allow-net=example.com",
        ],
        &format!(
            "try {{ await fetch('http://127.0.0.1:{port}/'); console.log('NO THROW'); }} \
             catch (e) {{ console.log('refused', e.cause?.code ?? e.code); }}"
        ),
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("ERR_PERMISSION_DENIED"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn a_malformed_address_is_an_argument_error() {
    // Reported against the flag, before anything runs — not as a provider
    // failure at the first connect.
    for value in ["--allow-net=example.com:99999", "--allow-listen=[::1"] {
        let out = run(&["--deny-all", value], "console.log(1)");
        assert!(!out.status.success(), "{value} should be rejected");
        let s = stderr(&out);
        assert!(s.contains("An entry is a host"), "{value}: {s}");
    }
}

/// A directory tree for the path tests: `data/ok.txt` and a `secrets.env`
/// beside it, with an empty `out/`. Returns the (canonicalized) root.
fn scoped_tree(name: &str) -> PathBuf {
    let root = temp(name);
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).expect("mkdir");
    std::fs::create_dir_all(root.join("out")).expect("mkdir");
    std::fs::write(root.join("data/ok.txt"), "fine").expect("write");
    std::fs::write(root.join("secrets.env"), "TOKEN=1").expect("write");
    std::fs::canonicalize(&root).expect("canonicalize")
}

/// Runs `code` with `dir` as the working directory — the directory a relative
/// `--allow-read=data` is resolved against.
fn run_in(dir: &PathBuf, flags: &[&str], code: &str) -> Output {
    esrun()
        .current_dir(dir)
        .args(flags)
        .arg(format!("-e={}", code))
        .output()
        .expect("spawn esrun")
}

#[test]
fn allow_read_reaches_the_listed_paths_and_refuses_the_rest() {
    let root = scoped_tree("scoped_read");
    let out = run_in(
        &root,
        &["--deny-all", "--allow-read=data"],
        "import fs from 'runtime:fs'; \
         console.log('read', await fs.file('data/ok.txt').text()); \
         try { await fs.file('secrets.env').text(); console.log('NO THROW'); } \
         catch (e) { console.log('refused', e.code); }",
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    assert!(s.contains("read fine"), "{s}");
    // A scoped denial, not a jail escape: the file is inside the project root,
    // it is simply not one this run may read.
    assert!(s.contains("refused ERR_PERMISSION_DENIED"), "{s}");
}

#[test]
fn allow_write_is_a_separate_list_from_allow_read() {
    let root = scoped_tree("scoped_write");
    let out = run_in(
        &root,
        &["--deny-all", "--allow-read=data", "--allow-write=out"],
        "import fs from 'runtime:fs'; \
         await fs.write('out/report.json', '{}'); console.log('wrote'); \
         try { await fs.write('data/ok.txt', 'x'); console.log('NO THROW'); } \
         catch (e) { console.log('refused', e.code); } \
         try { await fs.file('out/report.json').text(); console.log('NO THROW'); } \
         catch (e) { console.log('unreadable', e.code); }",
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    assert!(s.contains("wrote"), "{s}");
    // Writable where it may not read, readable where it may not write: two
    // grants, two lists, no implication either way.
    assert!(s.contains("refused ERR_PERMISSION_DENIED"), "{s}");
    assert!(s.contains("unreadable ERR_PERMISSION_DENIED"), "{s}");
    assert!(root.join("out/report.json").exists());
    assert_eq!(
        std::fs::read_to_string(root.join("data/ok.txt")).unwrap(),
        "fine"
    );
}

#[test]
#[cfg(unix)]
fn a_symlink_cannot_walk_out_of_a_path_list() {
    // Why the check runs after canonicalization: `data/escape/secrets.env` is a
    // name inside the list for a file outside it.
    let root = scoped_tree("scoped_symlink");
    std::os::unix::fs::symlink(&root, root.join("data/escape")).expect("symlink");
    let out = run_in(
        &root,
        &["--deny-all", "--allow-read=data"],
        "import fs from 'runtime:fs'; \
         try { await fs.file('data/escape/secrets.env').text(); console.log('NO THROW'); } \
         catch (e) { console.log('refused', e.code); }",
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("refused ERR_PERMISSION_DENIED"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn a_path_outside_the_jail_is_reachable_only_when_the_command_line_names_it() {
    // D54: the jail is the default boundary, and an explicit path on the
    // command line is the operator widening it. This is what lets a server read
    // a TLS certificate under /etc, which no project root contains.
    let root = scoped_tree("scoped_jail");
    let outside = temp("scoped_jail_outside");
    let _ = std::fs::remove_dir_all(&outside);
    std::fs::create_dir_all(&outside).expect("mkdir");
    std::fs::write(outside.join("cert.pem"), "PEM").expect("write");
    let outside = std::fs::canonicalize(&outside).expect("canonicalize");
    let granted = outside.display().to_string();

    let code = format!(
        "import fs from 'runtime:fs'; \
         console.log((await fs.file('{granted}/cert.pem').text()).trim()); \
         try {{ await fs.file('/etc/hostname').text(); console.log('NO THROW'); }} \
         catch (e) {{ console.log('refused', e.code); }}"
    );
    let out = run_in(
        &root,
        &["--deny-all", &format!("--allow-read={granted}")],
        &code,
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    // The granted path reads; everything else outside the jail still does not.
    assert_eq!(stdout(&out), "PEM\nrefused ERR_JAIL_ESCAPE\n");
}

#[test]
fn a_granted_read_path_does_not_become_writable() {
    // Two flags, two grants: reading a certificate must never imply the power
    // to replace it.
    let root = scoped_tree("scoped_jail_rw");
    let outside = temp("scoped_jail_rw_outside");
    let _ = std::fs::remove_dir_all(&outside);
    std::fs::create_dir_all(&outside).expect("mkdir");
    std::fs::write(outside.join("cert.pem"), "PEM").expect("write");
    let granted = std::fs::canonicalize(&outside).expect("canonicalize");
    let granted = granted.display().to_string();

    let code = format!(
        "import fs from 'runtime:fs'; \
         try {{ await fs.write('{granted}/cert.pem', new Uint8Array([1])); \
                console.log('NO THROW'); }} \
         catch (e) {{ console.log('refused', e.code); }}"
    );
    let out = run_in(
        &root,
        &[
            "--deny-all",
            &format!("--allow-read={granted}"),
            "--allow-write=./out",
        ],
        &code,
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out), "refused ERR_JAIL_ESCAPE\n");
}

#[test]
fn a_granted_read_path_does_not_widen_module_resolution() {
    // The loader detects its own root and never consults these lists, so a path
    // grant makes bytes readable — not code importable. Otherwise
    // `--allow-read` would quietly be a way to run anything on the disk.
    let root = scoped_tree("scoped_jail_import");
    let outside = temp("scoped_jail_import_outside");
    let _ = std::fs::remove_dir_all(&outside);
    std::fs::create_dir_all(&outside).expect("mkdir");
    std::fs::write(outside.join("mod.mjs"), "export const v = 'imported';").expect("write");
    let granted = std::fs::canonicalize(&outside).expect("canonicalize");
    let granted = granted.display().to_string();

    let code = format!(
        "import fs from 'runtime:fs'; \
         console.log((await fs.file('{granted}/mod.mjs').text()).length > 0); \
         try {{ await import('{granted}/mod.mjs'); console.log('IMPORTED'); }} \
         catch {{ console.log('import refused'); }}"
    );
    let out = run_in(
        &root,
        &[
            "--deny-all",
            "--allow-imports",
            &format!("--allow-read={granted}"),
        ],
        &code,
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out), "true\nimport refused\n");
}

#[test]
fn a_path_inside_the_jail_is_still_the_ordinary_case() {
    // The refusal above must not have cost the flag its job.
    let root = scoped_tree("scoped_jail_ok");
    let out = run_in(
        &root,
        &["--deny-all", "--allow-read=./data"],
        "import fs from 'runtime:fs'; \
         console.log((await fs.file('./data/ok.txt').text()).trim()); \
         try { await fs.file('./secrets.env').text(); console.log('NO THROW'); } \
         catch (e) { console.log('refused', e.code); }",
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("refused ERR_PERMISSION_DENIED"),
        "{}",
        stdout(&out)
    );
}

/// A project with two local modules and two packages, plus an `app.mjs` that
/// imports one of each and probes the other two. Returns the canonical root.
fn import_project(name: &str) -> PathBuf {
    let root = temp(name);
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("vendor")).unwrap();
    std::fs::write(root.join("package.json"), r#"{"name":"t","type":"module"}"#).unwrap();
    std::fs::write(root.join("src/lib.mjs"), "export const v = 1;").unwrap();
    std::fs::write(root.join("vendor/x.mjs"), "export const v = 2;").unwrap();
    for pkg in ["good", "evil"] {
        let dir = root.join("node_modules").join(pkg);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("package.json"),
            format!(r#"{{"name":"{pkg}","type":"module","main":"index.js"}}"#),
        )
        .unwrap();
        std::fs::write(dir.join("index.js"), format!("export const n = '{pkg}';")).unwrap();
    }
    let root = std::fs::canonicalize(&root).unwrap();
    std::fs::write(
        root.join("app.mjs"),
        "import { v } from './src/lib.mjs'; \
         import { n } from 'good'; \
         console.log('loaded', v, n); \
         try { await import('./vendor/x.mjs'); console.log('NO THROW'); } \
         catch (e) { console.log('path refused'); } \
         try { await import('evil'); console.log('NO THROW'); } \
         catch (e) { console.log('package refused'); }",
    )
    .unwrap();
    root
}

/// Runs `app.mjs` in `root` under an import policy written to `policy.json`.
fn run_with_policy(root: &PathBuf, policy: &str, flags: &[&str]) -> Output {
    std::fs::write(root.join("policy.json"), policy).unwrap();
    esrun()
        .current_dir(root)
        .args(flags)
        .arg("--import-policy=policy.json")
        .arg("app.mjs")
        .output()
        .expect("spawn esrun")
}

#[test]
fn an_import_policy_allow_list_admits_named_packages_and_paths() {
    let root = import_project("policy_allow");
    let out = run_with_policy(
        &root,
        r#"{ "allow": ["./src", "good"] }"#,
        &["--allow-imports"],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    // The entry file is exempt — it is read before a loader exists, and the
    // user named it.
    assert!(s.contains("loaded 1 good"), "{s}");
    assert!(s.contains("path refused"), "{s}");
    assert!(s.contains("package refused"), "{s}");
}

#[test]
fn an_import_policy_without_an_allow_list_only_denies() {
    // The shape for excluding a handful of packages without enumerating the
    // whole graph.
    let root = import_project("policy_deny_only");
    let out = run_with_policy(&root, r#"{ "deny": ["evil"] }"#, &["--allow-imports"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    assert!(s.contains("loaded 1 good"), "{s}");
    assert!(s.contains("NO THROW"), "{s}"); // ./vendor is not denied
    assert!(s.contains("package refused"), "{s}");
}

#[test]
fn an_import_policy_is_not_a_way_around_the_imports_capability() {
    // Two layers: the capability decides whether the loader runs at all, the
    // policy decides what it may resolve. A policy cannot re-open a loader that
    // --deny-all closed.
    let root = import_project("policy_vs_capability");
    let out = run_with_policy(&root, r#"{ "allow": ["./src", "good"] }"#, &["--deny-all"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("imports"), "stderr: {}", stderr(&out));
}

#[test]
fn an_import_policy_composes_with_the_imports_capability() {
    let root = import_project("policy_with_capability");
    let out = run_with_policy(
        &root,
        r#"{ "allow": ["./src", "good"] }"#,
        &["--deny-all", "--allow-imports"],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("loaded 1 good"), "{}", stdout(&out));
}

#[test]
fn a_broken_import_policy_is_reported_before_anything_runs() {
    let root = import_project("policy_broken");
    for (policy, expected) in [
        (r#"{ "allowed": ["good"] }"#, "unknown key"),
        (r#"{ "allow": "good" }"#, "must be an array"),
        ("{", "invalid JSON"),
        (r#"{ "allow": [] }"#, "nothing may be imported"),
    ] {
        let out = run_with_policy(&root, policy, &[]);
        assert!(!out.status.success(), "{policy} should be rejected");
        let s = stderr(&out);
        assert!(s.contains(expected), "{policy} → {s}");
        assert!(s.contains("policy.json"), "{policy} → {s}");
    }
}

#[test]
fn a_missing_import_policy_file_is_an_error() {
    let root = import_project("policy_missing");
    let out = esrun()
        .current_dir(&root)
        .arg("--import-policy=nope.json")
        .arg("app.mjs")
        .output()
        .expect("spawn esrun");
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("cannot read import policy"),
        "stderr: {}",
        stderr(&out)
    );
}

#[test]
fn allow_imports_takes_no_value_and_points_at_the_policy() {
    // What may be loaded is not a capability scope; the error says where it
    // lives instead of pretending the flag will grow a list.
    let out = run(&["--deny-all", "--allow-imports=./src"], "console.log(1)");
    assert!(!out.status.success());
    let s = stderr(&out);
    assert!(s.contains("--allow-imports takes no value"), "{s}");
    assert!(s.contains("--import-policy=<file>"), "{s}");
}

#[test]
fn allow_signals_narrows_what_may_be_watched_and_what_is_reported() {
    // A watch suppresses the signal's default action, so the list decides which
    // deaths the program may decline. `signals()` reports only what it may use.
    let out = run(
        &["--deny-all", "--allow-signals=SIGTERM"],
        "import { signals, onSignal, offSignal } from 'runtime:process';          console.log('available', signals().join(','));          const noop = () => {};          onSignal('SIGTERM', noop); console.log('watched SIGTERM');          try { onSignal('SIGINT', noop); console.log('NO THROW'); }          catch (e) { console.log('refused', e.code); }          offSignal('SIGTERM', noop);",
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    assert!(s.contains("available SIGTERM"), "{s}");
    assert!(s.contains("watched SIGTERM"), "{s}");
    assert!(s.contains("refused ERR_PERMISSION_DENIED"), "{s}");
}

#[test]
fn an_unknown_signal_name_in_a_scope_list_is_an_argument_error() {
    let out = run(&["--deny-all", "--allow-signals=SIGFOO"], "console.log(1)");
    assert!(!out.status.success());
    let s = stderr(&out);
    assert!(s.contains("SIGFOO is not a signal name"), "{s}");
    assert!(s.contains("SIGTERM"), "{s}");
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
fn every_capability_name_but_imports_takes_a_scope_list() {
    // Seven of the eight. `imports` is deliberately not scoped by a flag —
    // what may be loaded is an import policy (D39), not a capability scope —
    // and this fails if a capability is added without enforcement behind it.
    for name in ["read", "write", "net", "listen", "env", "run", "signals"] {
        let value = match name {
            "net" | "listen" => "127.0.0.1:9",
            "signals" => "SIGTERM",
            "read" | "write" => ".",
            _ => "X",
        };
        let out = run(
            &["--deny-all", &format!("--allow-{name}={value}")],
            "console.log('ran')",
        );
        assert!(
            out.status.success(),
            "--allow-{name}={value} should be accepted: {}",
            stderr(&out)
        );
    }
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
fn a_scoped_allow_needs_no_mode_flag() {
    // The line that D65 exists to make writable: a grant, narrowed, and nothing
    // else on it. Everything not named stays denied.
    let out = run(
        &["--allow-env=HOME"],
        "import { permissions, env } from 'runtime:process'; \
         console.log(permissions.has('env'), typeof env.HOME, permissions.has('net'));",
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "true string false");
}

// ---- the permissions API itself ----------------------------------------------

#[test]
fn has_rejects_a_per_value_check_rather_than_ignoring_the_value() {
    // `has("read", "/etc/passwd")` used to answer about the capability and drop
    // the path — true under `--allow-read=./data`, which is the same lie the
    // parser refuses to tell when it rejects a flag it cannot enforce. Whether
    // one path is reachable is the deployment's business, answered by making
    // the call.
    let out = run(
        &[],
        "import { permissions } from 'runtime:process'; \
         try { permissions.has('read', '/etc/passwd'); console.log('NO THROW'); } \
         catch (e) { console.log(e.constructor.name, e.message); }",
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    assert!(s.contains("TypeError"), "{s}");
    assert!(s.contains("takes one argument"), "{s}");
    assert!(s.contains("ERR_PERMISSION_DENIED"), "{s}");
}

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
        &["--allow-all", "--deny-write"],
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
