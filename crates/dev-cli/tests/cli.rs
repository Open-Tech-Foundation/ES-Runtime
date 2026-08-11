//! End-to-end tests for the `esdev` binary.
//!
//! The point of these is **parity**, not coverage of the runtime: `esdev` and
//! `esrun` share every line that decides how a run behaves
//! (`es-runtime-cli-common`), and the whole design rests on a program not being
//! able to behave one way under one binary and differently under the other. So
//! these spawn the real binary and assert that the shared surface — the module
//! load, the capability model, the D38 flag grammar, the error block — is the
//! same one `esrun` presents, plus the few things that are `esdev`'s own.

// A test reporting why it skipped is talking to whoever reads the run.
#![allow(clippy::print_stderr)]

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

/// Another workspace binary from the same target directory, or `None` if it has
/// not been built. Cargo only exports `CARGO_BIN_EXE_*` for the *current*
/// package's binaries, and `esrun` belongs to another.
fn sibling_binary(name: &str) -> Option<PathBuf> {
    let path = PathBuf::from(env!("CARGO_BIN_EXE_esdev"))
        .parent()?
        .join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    path.exists().then_some(path)
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

// ---------------------------------------------------------------------------
// TypeScript / JSX (DECISIONS D59)
//
// The unit tests in `transform.rs` check what the stripper emits. These check
// the part only the real binary can: that a `.ts` entry *and* the `.ts` files
// it imports both go through it, that `esrun` still refuses the same file, and
// that the transform changes nothing else about the run.
// ---------------------------------------------------------------------------

#[test]
fn a_typescript_entry_runs() {
    let app = write(
        "ts_entry.ts",
        "interface U { id: number }\n\
         const u: U = { id: 7 };\n\
         function show(x: U): string { return `id=${x.id}`; }\n\
         console.log(show(u));\n",
    );
    let out = esdev().arg(&app).output().expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "id=7");
}

/// The entry is read directly rather than through the loader, and imports come
/// through the loader — two different paths, and a transform wired into only
/// one of them passes a test like the one above while failing every real
/// program.
#[test]
fn an_imported_typescript_module_is_stripped_too() {
    write(
        "ts_dep.ts",
        "export interface P { n: number }\n\
         export const twice = (p: P): number => p.n * 2;\n",
    );
    let app = write(
        "ts_main.ts",
        "import type { P } from './ts_dep.ts';\n\
         import { twice } from './ts_dep.ts';\n\
         const p: P = { n: 21 };\n\
         console.log(twice(p));\n",
    );
    let out = esdev().arg(&app).output().expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "42");
}

/// `enum` is the construct that panicked the transformer before the semantic
/// pass was told to evaluate enum members — a crash, not an error, and only a
/// real run surfaced it.
#[test]
fn an_enum_runs_rather_than_crashing() {
    let app = write(
        "ts_enum.ts",
        "enum Color { Red, Green, Blue }\nconsole.log(Color.Blue, Color[2]);\n",
    );
    let out = esdev().arg(&app).output().expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "2 Blue");
}

#[test]
fn jsx_uses_the_pragma_to_choose_its_runtime() {
    // No JSX runtime is installed, so the proof is *which* module it went
    // looking for: the pragma decided, not a hardcoded default.
    let app = write(
        "jsx_pragma.tsx",
        "/** @jsxImportSource my-ui */\nexport const el = <div>hi</div>;\n",
    );
    let out = esdev().arg(&app).output().expect("spawn esdev");
    assert!(!out.status.success());
    assert!(stderr(&out).contains("my-ui"), "{}", stderr(&out));
}

#[test]
fn a_type_error_is_not_checked_and_does_not_stop_the_run() {
    // Types are erased, never checked — the same contract Node's strip-types
    // mode has. A typechecker on the critical path of every run would be a
    // different product.
    let app = write(
        "ts_unchecked.ts",
        "const n: number = 'actually a string' as unknown as number;\nconsole.log(typeof n);\n",
    );
    let out = esdev().arg(&app).output().expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "string");
}

#[test]
fn a_typescript_syntax_error_names_the_file() {
    let app = write("ts_broken.ts", "const x: = ;\n");
    let out = esdev().arg(&app).output().expect("spawn esdev");
    assert!(!out.status.success());
    assert!(stderr(&out).contains("ts_broken.ts"), "{}", stderr(&out));
}

/// The boundary that makes the split worth having: `esdev` strips, `esrun` does
/// not. If this ever passes under `esrun`, TypeScript has leaked into the
/// production binary.
#[test]
fn esrun_still_refuses_the_typescript_that_esdev_runs() {
    let app = write("ts_boundary.ts", "const n: number = 1;\nconsole.log(n);\n");
    let dev = esdev().arg(&app).output().expect("spawn esdev");
    assert!(dev.status.success(), "{}", stderr(&dev));

    // `CARGO_BIN_EXE_*` only names binaries of this package, so esrun is found
    // beside esdev — they share a target directory. Under a `-p
    // es-runtime-dev-cli` run it may not have been built; the workspace test
    // job that CI runs always builds it.
    let Some(esrun) = sibling_binary("esrun") else {
        eprintln!("skipping: esrun is not built beside esdev");
        return;
    };
    let prod = Command::new(esrun).arg(&app).output().expect("spawn esrun");
    assert!(
        !prod.status.success(),
        "esrun ran TypeScript — the transform has leaked into production"
    );
}

/// A `.js` file must not be reprinted on its way through: every byte the
/// stripper changed would be a byte the stack traces no longer match.
#[test]
fn a_javascript_file_keeps_its_own_line_numbers() {
    let app = write(
        "js_frames.js",
        "\n\n\n\nfunction boom() { throw new Error('x'); }\nboom();\n",
    );
    let out = esdev().arg(&app).output().expect("spawn esdev");
    assert!(!out.status.success());
    // The throw is on line 5 of the file as written.
    assert!(stderr(&out).contains(":5:"), "{}", stderr(&out));
}
