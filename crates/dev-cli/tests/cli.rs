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

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;

fn temp(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name)
}

fn write(name: &str, contents: &str) -> PathBuf {
    let path = temp(name);
    std::fs::write(&path, contents).expect("write temp file");
    path
}

fn esdev() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_esdev"));
    // The sandbox is the working directory (D79): run from the directory these
    // fixtures are written into, as a user runs a program from its own.
    command.current_dir(env!("CARGO_TARGET_TMPDIR"));
    command
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

/// Diagnostic paths are native filesystem paths; compare their separators as
/// paths rather than accidentally making a Unix spelling part of the contract.
fn slash_paths(text: &str) -> String {
    text.replace('\\', "/")
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
    // A `file:` URL rather than the path as written: on Windows an absolute
    // path is a URL whose scheme is `c`, so the import never reaches the
    // loader as a file — the same spelling the `esrun` module suite uses.
    let dep = url::Url::from_file_path(&dep)
        .expect("a file URL")
        .to_string();
    let app = write(
        "tla.mjs",
        &format!("const m = await import({dep:?});\nconsole.log(m.answer);\n"),
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
    // `-v` and `-V` are the same flag in two spellings, and the help says so.
    for flag in ["-v", "-V"] {
        let out = esdev().arg(flag).output().expect("spawn esdev");
        assert!(out.status.success(), "{flag} failed");
        assert!(stdout(&out).starts_with("esdev "), "{flag}");
    }

    let help = esdev().arg("--help").output().expect("spawn esdev");
    let text = stdout(&help);
    assert!(text.contains("esdev"), "{text}");
    assert!(text.contains("-V"), "{text}");
    // The boundary is part of the help, not just the docs: this binary is not a
    // deployment target and the usage text has to say so.
    assert!(text.contains("not a deployment target"), "{text}");
    // Every subcommand is reachable from the help; `upgrade` is the newest and
    // the one nothing else would tell you about.
    for command in ["create", "start", "build", "test", "upgrade"] {
        assert!(
            text.contains(command),
            "{command} is not in the help:\n{text}"
        );
    }
}

#[test]
fn build_help_names_module_and_stylesheet_entry_types() {
    let out = esdev()
        .args(["build", "--help"])
        .output()
        .expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));
    let help = stdout(&out);
    assert!(help.contains(".js/.mjs/.ts/.tsx/.jsx module"), "{help}");
    assert!(help.contains("<entry.css>"), "{help}");
}

/// A document is not a module: naming an HTML file as the build entry is
/// refused with where it belongs (an esdev.json target), rather than fed to
/// the module bundler whose JSX parse error is three steps from the cause.
#[test]
fn a_document_build_entry_is_refused_with_a_pointer_to_targets() {
    write("build-entry-index.html", "<!DOCTYPE html>\n");
    let out = esdev()
        .args(["build", "./build-entry-index.html"])
        .output()
        .expect("spawn esdev");
    assert!(!out.status.success(), "built a document as a module");
    let message = slash_paths(&stderr(&out));
    assert!(message.contains("is a document"), "{message}");
    assert!(message.contains("esdev.json"), "{message}");
    assert!(!message.contains("JSX"), "{message}");
}

/// The help used to claim every flag but `--file`/`--watch` was also an
/// esdev.json key; `--update-snapshots`, `--ci` and `--full-diff` are flags
/// only. Both halves are pinned: what the help says, and that the file
/// refuses the key.
#[test]
fn test_help_names_which_options_live_in_the_file() {
    let out = esdev()
        .args(["test", "--help"])
        .output()
        .expect("spawn esdev test --help");
    let text = stdout(&out);
    assert!(
        text.contains("are also esdev.json"),
        "help does not name the file keys:\n{text}"
    );
    assert!(
        !text.contains("Everything but --file and --watch"),
        "help still claims every flag is a key:\n{text}"
    );

    let dir = build_dir("t_test_keys");
    write_in(
        &dir,
        "one.test.ts",
        "import { test } from \"runtime:test\";\ntest(\"x\", () => {});\n",
    );
    write_in(&dir, "app.mjs", "console.log(1);\n");
    write_in(
        &dir,
        "esdev.json",
        r#"{ "targets": { "app": { "entry": "app.mjs", "out": "dist/app.js" } },
             "test": { "updateSnapshots": true } }"#,
    );
    let ran = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    assert!(
        !ran.status.success(),
        "a flag-only option was accepted as a key"
    );
    assert!(
        stderr(&ran).contains("unknown key `updateSnapshots`"),
        "{}",
        stderr(&ran)
    );
}

/// The help for `esdev test` is where the test API is learned, and it described
/// an ambient `test()` that D71 replaced with an import — so a file written from
/// it failed with `ReferenceError: test is not defined`. Both halves are pinned:
/// what the help says, and that a file written from it runs.
#[test]
fn test_help_documents_the_api_that_exists() {
    let out = esdev()
        .args(["test", "--help"])
        .output()
        .expect("spawn esdev test --help");
    let text = stdout(&out);
    assert!(text.contains(r#"from "runtime:test""#), "{text}");
    assert!(!text.contains("globals already defined"), "{text}");

    let dir = build_dir("t_help_api");
    write_in(
        &dir,
        "help.test.ts",
        "import { test, assert, assertEquals, assertThrows, assertRejects } from \"runtime:test\";\n\
         test(\"it adds\", () => {\n\
         \u{20}\u{20}assertEquals(1 + 1, 2);\n\
         \u{20}\u{20}assert(true);\n\
         \u{20}\u{20}assertThrows(() => {\n\
         \u{20}\u{20}\u{20}\u{20}throw new TypeError(\"no\");\n\
         \u{20}\u{20}}, TypeError);\n\
         });\n\
         test(\"it rejects\", async () => {\n\
         \u{20}\u{20}await assertRejects(() => Promise.reject(new Error(\"nope\")));\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    assert!(
        ran.status.success(),
        "the documented API does not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );

    let _ = std::fs::remove_dir_all(&dir);
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

/// `types` was never `esdev`'s — it belongs with the runtime that documents
/// them — and `upgrade` now is: install.sh installs both binaries, so a
/// developer whose esdev is stale had no way to move it but the installer
/// (D77).
#[test]
fn types_is_refused_and_upgrade_is_a_subcommand() {
    let types = esdev().arg("types").output().expect("spawn esdev");
    // Treated as a path (it is a bare word), so it fails as a missing file
    // rather than doing something surprising.
    assert!(!types.status.success(), "types should not succeed");
    assert!(
        stderr(&types).contains("cannot read") || stderr(&types).contains("cannot resolve"),
        "types: {}",
        stderr(&types)
    );

    // `upgrade` reaches the network, so what is pinned here is that it is a
    // subcommand rather than a path: a stray argument is refused by name, which
    // a file path would not be.
    let extra = esdev()
        .args(["upgrade", "0.4.0"])
        .output()
        .expect("spawn esdev upgrade");
    assert!(!extra.status.success());
    assert!(
        stderr(&extra).contains("takes no arguments"),
        "{}",
        stderr(&extra)
    );
}

/// `upgrade` takes no arguments, but like every other subcommand it answers
/// `--help` — the top-level help promises each command takes it, and this one
/// refused it.
#[test]
fn upgrade_answers_help() {
    for flag in ["--help", "-h"] {
        let out = esdev()
            .args(["upgrade", flag])
            .output()
            .expect("spawn esdev upgrade --help");
        assert!(out.status.success(), "{flag}: {}", stderr(&out));
        let text = stdout(&out);
        assert!(text.contains("esdev upgrade"), "{text}");
        assert!(text.contains("--dry-run"), "{text}");
    }
}

/// `--dry-run` is the read half of `upgrade`: it reaches the network, so what
/// is pinned here is the grammar — a value is refused by name without going
/// anywhere, which a live check then honors. (The live path is exercised by
/// hand: `esdev upgrade --dry-run` against the real release listing.)
#[test]
fn upgrade_dry_run_takes_no_value() {
    let out = esdev()
        .args(["upgrade", "--dry-run=yes"])
        .output()
        .expect("spawn esdev upgrade --dry-run=yes");
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("takes no arguments"),
        "{}",
        stderr(&out)
    );
}

/// A flag that is taken and dropped is one somebody keeps passing and keeps
/// believing. `esdev start` does not run your program — it runs a build's
/// output as a child, under esdev.json's grants — so the run-shaping flags it
/// used to accept and ignore are refused by name. `--shutdown-grace` is the one
/// that does apply: it bounds the drain on a restart.
///
/// Run in a directory with no esdev.json, so nothing here can start a server:
/// the refused flags fail while parsing, and the kept one gets past parsing and
/// fails on the missing file — which is exactly the difference being tested.
#[test]
fn start_refuses_the_run_flags_it_cannot_apply() {
    let dir = build_dir("s_run_flags");

    for flag in ["--timeout=500", "--max-heap=64", "--env-override"] {
        let out = esdev_in(&dir)
            .args(["start", flag])
            .output()
            .expect("spawn esdev start");
        assert!(!out.status.success(), "{flag} was accepted");
        assert!(
            stderr(&out).contains("does not run your program"),
            "{flag}: {}",
            stderr(&out)
        );
    }

    let grace = esdev_in(&dir)
        .args(["start", "--shutdown-grace=100"])
        .output()
        .expect("spawn esdev start");
    assert!(!grace.status.success());
    assert!(stderr(&grace).contains("esdev.json"), "{}", stderr(&grace));
    assert!(
        !stderr(&grace).contains("does not run your program"),
        "{}",
        stderr(&grace)
    );

    let _ = std::fs::remove_dir_all(&dir);
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
    let prod = Command::new(esrun)
        .current_dir(app.parent().expect("fixture dir"))
        .arg(app.file_name().expect("entry"))
        .output()
        .expect("spawn esrun");
    assert!(
        !prod.status.success(),
        "esrun ran TypeScript — the transform has leaked into production"
    );
}

/// The specifiers a bundler answers and a strict resolver does not: no
/// extension, a directory, and TypeScript's own `./x.js`-means-`./x.ts`.
///
/// This is what lets `esdev` be pointed at a source tree that was written for a
/// build step, which is most of them — and every one of these is a *miss* under
/// the strict rules, so the file could not be loaded at all before.
#[test]
fn esdev_resolves_the_specifiers_a_bundler_would() {
    let dir = build_dir("t_resolve");
    write_in(&dir, "util.ts", "export const two = 2;\n");
    std::fs::create_dir_all(dir.join("nested")).expect("mkdir");
    write_in(&dir, "nested/index.ts", "export const three = 3;\n");
    write_in(&dir, "helper.ts", "export const four = 4;\n");
    let app = write_in(
        &dir,
        "app.ts",
        "import { two } from './util';\n\
         import { three } from './nested';\n\
         import { four } from './helper.js';\n\
         console.log(two + three + four);\n",
    );
    let out = esdev_in(&dir)
        .arg(app.file_name().expect("entry"))
        .output()
        .expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "9");
}

/// …and the other half of that boundary: `esrun` still refuses them.
///
/// Deployment resolution stays exactly what the module spec says, because the
/// artifact `esrun` runs has been through a build — a production binary that
/// guessed at filenames would be reading the disk to decide what a program
/// means.
#[test]
fn esrun_still_refuses_the_specifiers_esdev_resolves() {
    let dir = build_dir("t_resolve_prod");
    write_in(&dir, "util.ts", "export const two = 2;\n");
    let app = write_in(
        &dir,
        "app.mjs",
        "import { two } from './util';\nconsole.log(two);\n",
    );
    let Some(esrun) = sibling_binary("esrun") else {
        eprintln!("skipping: esrun is not built beside esdev");
        return;
    };
    let out = Command::new(esrun)
        .current_dir(&dir)
        .arg(app.file_name().expect("entry"))
        .output()
        .expect("spawn esrun");
    assert!(
        !out.status.success(),
        "esrun resolved an extensionless specifier — bundler-style resolution \
         has leaked into production"
    );
}

/// A miss still reports the specifier that was written, not the last spelling
/// tried: "cannot resolve ./nope.ts" for `import "./nope"` sends the reader
/// looking for a file they never mentioned.
#[test]
fn an_unresolvable_import_names_what_the_file_said() {
    let dir = build_dir("t_resolve_miss");
    let app = write_in(&dir, "app.ts", "import './nope';\n");
    let out = esdev_in(&dir)
        .arg(app.file_name().expect("entry"))
        .output()
        .expect("spawn esdev");
    assert!(!out.status.success());
    let text = stderr(&out);
    assert!(text.contains("nope"), "{text}");
    assert!(
        !text.contains("nope.mjs") && !text.contains("nope/index"),
        "the error named a spelling the file never wrote:\n{text}"
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

// ---------------------------------------------------------------------------
// `esdev build` (DECISIONS D59)
//
// The property worth testing is not "a bundler bundles" — rolldown has its own
// suite for that. It is the four settings that make this a command rather than
// a note telling people which flags to pass, each of which fails *silently*
// when wrong.
// ---------------------------------------------------------------------------

/// A library is describable in `esdev.json`, and the file and the flags build
/// the same thing.
///
/// This is the property, not a convenience: everything else about a build lives
/// in that file, so a library that could only be described on a command line
/// could describe only *part* of itself. `assets` is the case that proves it —
/// the README and LICENSE a package ships are a target key, and a `--lib` build
/// driven by flags alone had no way to name them.
#[test]
fn a_library_target_builds_the_same_thing_the_flags_do() {
    let dir = build_dir("b_libtarget");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(&dir, "src/index.ts", "export const two: number = 2;\n");
    write_in(&dir, "README.md", "# a package\n");
    write_in(
        &dir,
        "esdev.json",
        r#"{ "targets": { "lib": {
             "entry": "src", "lib": true, "outdir": "dist",
             "format": ["esm", "cjs"], "assets": ["README.md"] } } }"#,
    );

    let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    // The verb a library gets from the flags, not an application's.
    assert!(stdout(&out).contains("built"), "{}", stdout(&out));

    for name in ["index.js", "index.cjs", "index.d.ts", "index.d.cts"] {
        assert!(
            dir.join("dist").join(name).is_file(),
            "dist/{name} is missing:\n{}",
            stdout(&out)
        );
    }
    // The half the flags could never do.
    assert!(
        dir.join("dist/README.md").is_file(),
        "a library target did not copy its assets:\n{}",
        stdout(&out)
    );

    // …and the same build, asked for the other way, writes the same tree.
    let flags = esdev_in(&dir)
        .args(["build", "--lib", "src", "--out=dist2", "--format=esm,cjs"])
        .output()
        .expect("spawn esdev");
    assert!(flags.status.success(), "{}", stderr(&flags));
    for name in ["index.js", "index.cjs", "index.d.ts", "index.d.cts"] {
        assert_eq!(
            std::fs::read(dir.join("dist").join(name)).ok(),
            std::fs::read(dir.join("dist2").join(name)).ok(),
            "the file and the flags disagree about {name}"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// The keys that mean nothing off a library are refused by the file, in the
/// words the flags use — so a reader who moved a build from a script into
/// `esdev.json` gets the same answer either way.
#[test]
fn the_library_keys_are_refused_on_an_application_target() {
    let dir = build_dir("b_libkeys");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(&dir, "src/app.ts", "export const x = 1;\n");
    for (key, value) in [("format", "\"cjs\""), ("types", "false")] {
        write_in(
            &dir,
            "esdev.json",
            &format!(
                r#"{{ "targets": {{ "app": {{ "entry": "src/app.ts",
                     "out": "dist/app.js", "{key}": {value} }} }} }}"#
            ),
        );
        let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
        assert!(!out.status.success(), "{key} was accepted");
        let err = stderr(&out);
        assert!(err.contains(key) && err.contains("lib"), "{err}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A directory of its own per test: `build` writes files, and two tests sharing
/// `dist/` would race.
fn build_dir(name: &str) -> PathBuf {
    let dir = temp(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create build dir");
    dir
}

fn write_in(dir: &Path, name: &str, contents: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, contents).expect("write file");
    path
}

fn esdev_in(dir: &Path) -> Command {
    let mut cmd = esdev();
    cmd.current_dir(dir);
    // Snapshot setup runs model a developer's local invocation. GitHub sets
    // CI for the test harness too, which would make those runs refuse to write
    // the baselines they are about to verify. CI-specific cases set `CI`
    // explicitly below, overriding this removal.
    cmd.env_remove("CI");
    cmd
}

#[test]
fn build_bundles_a_graph_into_one_file() {
    let dir = build_dir("b_graph");
    write_in(&dir, "dep.mjs", "export const answer = 42;\n");
    write_in(
        &dir,
        "app.mjs",
        "import { answer } from './dep.mjs';\nconsole.log(answer);\n",
    );

    let out = esdev_in(&dir)
        .args(["build", "app.mjs"])
        .output()
        .expect("spawn esdev build");
    assert!(out.status.success(), "{}", stderr(&out));

    // Default output location, stated in the help.
    let bundle = dir.join("dist/app.js");
    assert!(bundle.exists(), "{}", stdout(&out));
    let text = std::fs::read_to_string(&bundle).expect("read bundle");
    assert!(!text.contains("./dep.mjs"), "the import survived:\n{text}");

    // And it runs — under esrun, which is the only audience a bundle has.
    let Some(esrun) = sibling_binary("esrun") else {
        return;
    };
    let ran = Command::new(esrun)
        .current_dir(bundle.parent().expect("bundle dir"))
        .arg(bundle.file_name().expect("bundle"))
        .output()
        .expect("spawn esrun");
    assert!(ran.status.success(), "{}", stderr(&ran));
    assert_eq!(stdout(&ran).trim(), "42");
}

/// The setting a hand-written bundler config gets wrong, and the failure is not
/// at build time — it is an artifact that dies on its first import.
#[test]
fn build_leaves_runtime_modules_for_the_runtime_to_serve() {
    let dir = build_dir("b_external");
    write_in(
        &dir,
        "app.mjs",
        "import { join } from 'runtime:path';\nconsole.log(join('a', 'b'));\n",
    );

    let out = esdev_in(&dir)
        .args(["build", "app.mjs"])
        .output()
        .expect("spawn esdev build");
    assert!(out.status.success(), "{}", stderr(&out));

    let text = std::fs::read_to_string(dir.join("dist/app.js")).expect("read bundle");
    assert!(
        text.contains("runtime:path"),
        "runtime:path was inlined instead of left external:\n{text}"
    );

    let Some(esrun) = sibling_binary("esrun") else {
        return;
    };
    let ran = Command::new(esrun)
        .current_dir(&dir)
        .arg("dist/app.js")
        .output()
        .expect("spawn esrun");
    assert!(ran.status.success(), "{}", stderr(&ran));
    assert!(stdout(&ran).contains("a"), "{}", stdout(&ran));
}

/// Packages branch on `process.env.NODE_ENV` before doing anything, and there is
/// no `process` global on this runtime — so an undefined one is a crash, not a
/// missing optimisation.
#[test]
fn build_defines_node_env_and_an_explicit_define_wins() {
    let dir = build_dir("b_define");
    write_in(
        &dir,
        "app.mjs",
        "console.log('env:', process.env.NODE_ENV);\n",
    );

    let out = esdev_in(&dir)
        .args(["build", "app.mjs"])
        .output()
        .expect("spawn esdev build");
    assert!(out.status.success(), "{}", stderr(&out));
    let text = std::fs::read_to_string(dir.join("dist/app.js")).expect("read bundle");
    assert!(text.contains("production"), "{text}");
    assert!(!text.contains("process.env"), "process survived:\n{text}");

    // An explicit --define overrides the default rather than colliding with it.
    let out = esdev_in(&dir)
        .args([
            "build",
            "app.mjs",
            "--out=dist/dev.js",
            "--define=process.env.NODE_ENV=\"development\"",
        ])
        .output()
        .expect("spawn esdev build");
    assert!(out.status.success(), "{}", stderr(&out));
    let text = std::fs::read_to_string(dir.join("dist/dev.js")).expect("read bundle");
    assert!(text.contains("development"), "{text}");
}

/// The condition that decides whether a package hands over its Web-API build or
/// its `node:` one. Getting it wrong builds cleanly and fails at runtime.
#[test]
fn build_asserts_the_worker_condition() {
    let dir = build_dir("b_conditions");
    std::fs::create_dir_all(dir.join("node_modules/two-faced")).expect("mkdir");
    write_in(
        &dir.join("node_modules/two-faced"),
        "package.json",
        r#"{"name":"two-faced","version":"1.0.0","type":"module",
            "exports":{".":{"worker":"./worker.js","default":"./default.js"}}}"#,
    );
    write_in(
        &dir.join("node_modules/two-faced"),
        "worker.js",
        "export const which = 'worker';\n",
    );
    write_in(
        &dir.join("node_modules/two-faced"),
        "default.js",
        "export const which = 'default';\n",
    );
    write_in(
        &dir,
        "app.mjs",
        "import { which } from 'two-faced';\nconsole.log(which);\n",
    );

    let out = esdev_in(&dir)
        .args(["build", "app.mjs"])
        .output()
        .expect("spawn esdev build");
    assert!(out.status.success(), "{}", stderr(&out));
    let text = std::fs::read_to_string(dir.join("dist/app.js")).expect("read bundle");
    assert!(
        text.contains("worker"),
        "the worker branch was not taken:\n{text}"
    );
}

/// `--conditions` adds to the defaults rather than replacing them: a user asking
/// for one more must not silently lose `worker`.
#[test]
fn extra_conditions_add_rather_than_replace() {
    let dir = build_dir("b_extra_conditions");
    std::fs::create_dir_all(dir.join("node_modules/three-faced")).expect("mkdir");
    write_in(
        &dir.join("node_modules/three-faced"),
        "package.json",
        r#"{"name":"three-faced","version":"1.0.0","type":"module",
            "exports":{".":{"custom":"./custom.js","worker":"./worker.js","default":"./default.js"}}}"#,
    );
    for (file, value) in [
        ("custom.js", "custom"),
        ("worker.js", "worker"),
        ("default.js", "default"),
    ] {
        write_in(
            &dir.join("node_modules/three-faced"),
            file,
            &format!("export const which = '{value}';\n"),
        );
    }
    write_in(
        &dir,
        "app.mjs",
        "import { which } from 'three-faced';\nconsole.log(which);\n",
    );

    // Asking for `custom` must not cost `worker`; the manifest's own key order
    // decides between them (D40), and `custom` is first here.
    let out = esdev_in(&dir)
        .args(["build", "app.mjs", "--conditions=custom"])
        .output()
        .expect("spawn esdev build");
    assert!(out.status.success(), "{}", stderr(&out));
    let text = std::fs::read_to_string(dir.join("dist/app.js")).expect("read bundle");
    assert!(text.contains("custom"), "{text}");
}

#[test]
fn a_commonjs_dependency_is_converted_rather_than_refused() {
    let dir = build_dir("b_cjs");
    std::fs::create_dir_all(dir.join("node_modules/old-school")).expect("mkdir");
    write_in(
        &dir.join("node_modules/old-school"),
        "package.json",
        r#"{"name":"old-school","version":"1.0.0","main":"index.js"}"#,
    );
    write_in(
        &dir.join("node_modules/old-school"),
        "index.js",
        "module.exports = { greet: () => 'from cjs' };\n",
    );
    write_in(
        &dir,
        "app.mjs",
        "import pkg from 'old-school';\nconsole.log(pkg.greet());\n",
    );

    // esrun refuses this package unbundled — that is D22, and it stays true.
    if let Some(esrun) = sibling_binary("esrun") {
        let refused = Command::new(esrun)
            .current_dir(&dir)
            .arg("--allow-imports")
            .arg("app.mjs")
            .output()
            .expect("spawn esrun");
        assert!(!refused.status.success());
        assert!(
            stderr(&refused).contains("CommonJS"),
            "{}",
            stderr(&refused)
        );
    }

    let out = esdev_in(&dir)
        .args(["build", "app.mjs"])
        .output()
        .expect("spawn esdev build");
    assert!(out.status.success(), "{}", stderr(&out));

    // Bundled, it runs — the conversion happened here, not in the runtime.
    let Some(esrun) = sibling_binary("esrun") else {
        return;
    };
    let ran = Command::new(esrun)
        .current_dir(&dir)
        .arg("dist/app.js")
        .output()
        .expect("spawn esrun");
    assert!(ran.status.success(), "{}", stderr(&ran));
    assert_eq!(stdout(&ran).trim(), "from cjs");
}

/// The claim the help makes, asserted: a bundle needs no `imports` grant,
/// because it has no imports left to resolve.
#[test]
fn a_bundle_runs_without_the_imports_capability() {
    let dir = build_dir("b_caps");
    write_in(&dir, "dep.mjs", "export const n = 7;\n");
    write_in(
        &dir,
        "app.mjs",
        "import { n } from './dep.mjs';\nconsole.log('n =', n);\n",
    );

    let Some(esrun) = sibling_binary("esrun") else {
        return;
    };
    // Unbundled under --deny-all: the loader cannot run.
    let unbundled = Command::new(&esrun)
        .current_dir(&dir)
        .arg("--deny-all")
        .arg("app.mjs")
        .output()
        .expect("spawn esrun");
    assert!(!unbundled.status.success());

    let out = esdev_in(&dir)
        .args(["build", "app.mjs"])
        .output()
        .expect("spawn esdev build");
    assert!(out.status.success(), "{}", stderr(&out));

    // Bundled under the same --deny-all: nothing left to import.
    let bundled = Command::new(&esrun)
        .current_dir(&dir)
        .arg("--deny-all")
        .arg("dist/app.js")
        .output()
        .expect("spawn esrun");
    assert!(bundled.status.success(), "{}", stderr(&bundled));
    assert_eq!(stdout(&bundled).trim(), "n = 7");
}

#[test]
fn build_writes_where_out_says_and_minify_shrinks_it() {
    let dir = build_dir("b_out");
    write_in(
        &dir,
        "app.mjs",
        "export function aLonglyNamedHelper(someArgument) {\n  return someArgument + 1;\n}\n\
         console.log(aLonglyNamedHelper(1));\n",
    );

    let plain = esdev_in(&dir)
        .args(["build", "app.mjs", "--out=out/plain.js"])
        .output()
        .expect("spawn esdev build");
    assert!(plain.status.success(), "{}", stderr(&plain));
    assert!(dir.join("out/plain.js").exists());

    let small = esdev_in(&dir)
        .args(["build", "app.mjs", "--out=out/small.js", "--minify"])
        .output()
        .expect("spawn esdev build");
    assert!(small.status.success(), "{}", stderr(&small));

    let plain_len = std::fs::metadata(dir.join("out/plain.js")).unwrap().len();
    let small_len = std::fs::metadata(dir.join("out/small.js")).unwrap().len();
    assert!(small_len < plain_len, "{small_len} !< {plain_len}");
}

#[test]
fn build_rejects_a_missing_entry_and_a_second_one() {
    let dir = build_dir("b_args");
    write_in(&dir, "app.mjs", "console.log(1);\n");

    let none = esdev_in(&dir).arg("build").output().expect("spawn esdev");
    assert!(!none.status.success());
    assert!(stderr(&none).contains("missing entry"), "{}", stderr(&none));

    let two = esdev_in(&dir)
        .args(["build", "app.mjs", "app.mjs"])
        .output()
        .expect("spawn esdev");
    assert!(!two.status.success());
    assert!(stderr(&two).contains("one entry"), "{}", stderr(&two));

    let absent = esdev_in(&dir)
        .args(["build", "nope.mjs"])
        .output()
        .expect("spawn esdev");
    assert!(!absent.status.success());
    assert!(
        stderr(&absent).contains("cannot read"),
        "{}",
        stderr(&absent)
    );
}

// ---------------------------------------------------------------------------
// Source maps, and the frames they put back
// ---------------------------------------------------------------------------

/// A stack trace from a deployed bundle names the bundle, which is a true
/// statement and a useless one. This is both halves: the build writes the map,
/// and the binary that runs the bundle reads it when it prints the stack.
#[test]
fn a_source_map_puts_a_stack_trace_back_in_the_source() {
    let dir = build_dir("b_sourcemap");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(
        &dir,
        "src/util.ts",
        "export function boom(n: number): number {\n  \
         if (n > 1) throw new Error('too big');\n  return n;\n}\n",
    );
    write_in(
        &dir,
        "src/app.ts",
        "import { boom } from './util.js';\nconsole.log(boom(5));\n",
    );

    // Off unless asked: a .map beside a deployment costs bytes and discloses
    // the source, and neither is the toolchain's decision.
    let plain = esdev_in(&dir)
        .args(["build", "src/app.ts", "--out=plain/app.js"])
        .output()
        .expect("spawn esdev build");
    assert!(plain.status.success(), "{}", stderr(&plain));
    assert!(!dir.join("plain/app.js.map").exists());

    let out = esdev_in(&dir)
        .args(["build", "src/app.ts", "--out=dist/app.js", "--sourcemap"])
        .output()
        .expect("spawn esdev build");
    assert!(out.status.success(), "{}", stderr(&out));
    let map = std::fs::read_to_string(dir.join("dist/app.js.map")).expect("read the map");
    assert!(
        std::fs::read_to_string(dir.join("dist/app.js"))
            .expect("read the bundle")
            .contains("//# sourceMappingURL=app.js.map")
    );
    // Absolute, and naming the real source rather than a path relative to the
    // staging directory the build wrote it in and then moved out of.
    let sources: serde_json::Value = serde_json::from_str(&map).expect("valid JSON");
    let first = sources["sources"][0].as_str().expect("a source");
    assert!(first.ends_with("src/util.ts"), "{first}");
    assert!(
        std::path::Path::new(first).is_file(),
        "the map names a file that is not there: {first}"
    );

    // The half only the runtime can do.
    let Some(esrun) = sibling_binary("esrun") else {
        return;
    };
    let ran = Command::new(esrun)
        .current_dir(&dir)
        .arg("dist/app.js")
        .output()
        .expect("spawn esrun");
    assert!(!ran.status.success());
    let trace = stderr(&ran);
    assert!(trace.contains("src/util.ts:2:"), "unmapped:\n{trace}");
    assert!(trace.contains("src/app.ts:2:"), "unmapped:\n{trace}");
    assert!(
        trace.contains("file:///"),
        "mapped frames must retain canonical file URLs:\n{trace}"
    );
    assert!(
        !trace.contains("dist/app.js:"),
        "still the bundle:\n{trace}"
    );
}

/// The dev loop maps itself: nothing is deployed, so neither half of the
/// decision a release build makes applies, and an unmapped dev bundle is a
/// stack trace pointing into generated code on every save.
#[test]
fn the_dev_loop_maps_without_being_asked() {
    let dir = build_dir("b_sourcemap_dev");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(&dir, "src/main.ts", "export const x: number = 1;\n");
    write_in(
        &dir,
        "index.html",
        "<!doctype html><html><body>\
         <script type=\"module\" src=\"./src/main.ts\"></script></body></html>\n",
    );
    write_in(
        &dir,
        "esdev.json",
        "{ \"targets\": { \"web\": { \"entry\": \"index.html\", \"outdir\": \"dist\" } } }\n",
    );

    let release = esdev_in(&dir)
        .arg("build")
        .output()
        .expect("spawn esdev build");
    assert!(release.status.success(), "{}", stderr(&release));
    let bundle = std::fs::read_dir(dir.join("dist/assets"))
        .expect("read assets")
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.extension().is_some_and(|e| e == "js"))
        .expect("a bundle");
    let released = std::fs::read_to_string(&bundle).expect("read the bundle");
    assert!(!released.contains("sourceMappingURL"), "{released}");

    // A `sourcemap` a target names is what it gets, in either mode.
    write_in(
        &dir,
        "esdev.json",
        "{ \"targets\": { \"web\": { \"entry\": \"index.html\", \"outdir\": \"dist\", \
         \"sourcemap\": \"inline\" } } }\n",
    );
    let mapped = esdev_in(&dir)
        .arg("build")
        .output()
        .expect("spawn esdev build");
    assert!(mapped.status.success(), "{}", stderr(&mapped));
    let bundle = std::fs::read_dir(dir.join("dist/assets"))
        .expect("read assets")
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.extension().is_some_and(|e| e == "js"))
        .expect("a bundle");
    let text = std::fs::read_to_string(&bundle).expect("read the bundle");
    assert!(
        text.contains("sourceMappingURL=data:application/json"),
        "no inline map in {}: {}",
        bundle.display(),
        &text[text.len().saturating_sub(200)..]
    );
}

// ---------------------------------------------------------------------------
// Assets a module imports
// ---------------------------------------------------------------------------

/// `import logo from "./logo.png"` used to stop the build — the bundler read
/// the image as source and reported that it was not valid UTF-8. The `assets`
/// copy could not answer it either: it cannot hash a name, and it does not know
/// the file was referenced at all.
#[test]
fn an_imported_asset_is_emitted_hashed_and_referenced_by_url() {
    let dir = build_dir("b_asset_import");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    std::fs::write(dir.join("src/logo.png"), b"\x89PNG\r\n\x1a\nfirst").expect("write png");
    write_in(
        &dir,
        "src/main.ts",
        "import logo from './logo.png';\n\
         document.body.innerHTML = `<img src=\"${logo}\">`;\n",
    );
    write_in(
        &dir,
        "index.html",
        "<!doctype html><html><body>\
         <script type=\"module\" src=\"./src/main.ts\"></script></body></html>\n",
    );
    write_in(
        &dir,
        "esdev.json",
        "{ \"targets\": { \"web\": { \"entry\": \"index.html\", \"outdir\": \"dist\" } } }\n",
    );

    let out = esdev_in(&dir)
        .arg("build")
        .output()
        .expect("spawn esdev build");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("1 asset"), "{}", stdout(&out));

    let emitted: Vec<_> = std::fs::read_dir(dir.join("dist/assets"))
        .expect("read assets")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".png"))
        .collect();
    assert_eq!(emitted.len(), 1, "{emitted:?}");
    let name = &emitted[0];
    assert!(
        name.starts_with("logo-") && name.len() > "logo-.png".len(),
        "{name}"
    );

    // The module exports the URL the file is served from, and it is the name
    // that was written.
    let bundle = std::fs::read_dir(dir.join("dist/assets"))
        .expect("read assets")
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.extension().is_some_and(|e| e == "js"))
        .expect("a bundle");
    let text = std::fs::read_to_string(bundle).expect("read the bundle");
    assert!(text.contains(&format!("/assets/{name}")), "{text}");

    // Changed bytes, changed name — which is the whole reason the copy was not
    // enough: a cache cannot serve last week's image under this week's URL.
    std::fs::write(dir.join("src/logo.png"), b"\x89PNG\r\n\x1a\nsecond").expect("write png");
    let again = esdev_in(&dir)
        .arg("build")
        .output()
        .expect("spawn esdev build");
    assert!(again.status.success(), "{}", stderr(&again));
    let second: Vec<_> = std::fs::read_dir(dir.join("dist/assets"))
        .expect("read assets")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".png"))
        .collect();
    assert_eq!(second.len(), 1, "{second:?}");
    assert_ne!(&second[0], name, "the URL did not change with the file");
}

/// A server bundle and a browser bundle contain the same component, so they
/// have to agree about the URL: the markup one sends names the file the other
/// fetches.
#[test]
fn a_server_build_emits_the_same_asset_url() {
    let dir = build_dir("b_asset_server");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    std::fs::write(dir.join("src/logo.png"), b"\x89PNG\r\n\x1a\n").expect("write png");
    write_in(
        &dir,
        "src/server.ts",
        "import logo from './logo.png';\nexport default `<img src=\"${logo}\">`;\n",
    );

    let out = esdev_in(&dir)
        .args(["build", "src/server.ts", "--out=dist/server.js"])
        .output()
        .expect("spawn esdev build");
    assert!(out.status.success(), "{}", stderr(&out));

    let bundle = std::fs::read_to_string(dir.join("dist/server.js")).expect("read the bundle");
    let emitted: Vec<_> = std::fs::read_dir(dir.join("dist/assets"))
        .expect("read assets")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(emitted.len(), 1, "{emitted:?}");
    assert!(
        bundle.contains(&format!("/assets/{}", emitted[0])),
        "{bundle}"
    );
}

/// A library is an input to a build that has not run yet, and where that build
/// serves a file from is its own decision — so the import is refused, with the
/// reason, rather than emitting a URL that is only correct here.
#[test]
fn a_library_refuses_an_asset_import_and_says_why() {
    let dir = build_dir("b_asset_lib");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    std::fs::write(dir.join("src/logo.png"), b"\x89PNG\r\n\x1a\n").expect("write png");
    write_in(
        &dir,
        "src/index.ts",
        "import logo from './logo.png';\nexport const url: string = logo;\n",
    );

    let out = esdev_in(&dir)
        .args(["build", "--lib", "src"])
        .output()
        .expect("spawn esdev build --lib");
    assert!(!out.status.success(), "{}", stdout(&out));
    let message = slash_paths(&stderr(&out));
    // The frame points at the import, and the explanation says what to do.
    assert!(message.contains("src/index.ts"), "{message}");
    assert!(message.contains("--lib cannot know one"), "{message}");
    assert!(message.contains("data:"), "{message}");
}

/// A `?raw` import used to die in the resolver naming a file that is really
/// there. The suffix is refused by name instead — after every other plugin,
/// so one that claims it keeps it.
#[test]
fn a_raw_suffix_is_refused_by_name() {
    let dir = build_dir("b_raw_suffix");
    write_in(&dir, "data.txt", "hello\n");
    write_in(
        &dir,
        "app.mjs",
        "import text from './data.txt?raw';\nconsole.log(text);\n",
    );
    write_in(
        &dir,
        "esdev.json",
        "{ \"targets\": { \"app\": { \"entry\": \"app.mjs\", \"out\": \"dist/app.js\" } } }",
    );

    let out = esdev_in(&dir)
        .arg("build")
        .output()
        .expect("spawn esdev build");
    assert!(!out.status.success(), "{}", stdout(&out));
    let message = slash_paths(&stderr(&out));
    assert!(message.contains("?raw"), "{message}");
    assert!(message.contains("does not support yet"), "{message}");
    assert!(!message.contains("No such file"), "{message}");
}

// ---------------------------------------------------------------------------
// `alias` and `import.meta.env`: the two things a project configures that a
// bundler has to be told, and that a hand-written `--define` was the only way
// to say.
// ---------------------------------------------------------------------------

/// `@/db` is how most of the ecosystem's source is written, and without this it
/// was an unresolved import.
#[test]
fn an_alias_is_rewritten_before_it_is_resolved() {
    let dir = build_dir("b_alias");
    std::fs::create_dir_all(dir.join("src/lib")).expect("create src");
    write_in(
        &dir,
        "src/lib/util.ts",
        "export const help = (): string => 'helped';\n",
    );
    write_in(
        &dir,
        "src/server.ts",
        "import { help } from '@/lib/util.js';\nexport default help();\n",
    );
    write_in(
        &dir,
        "esdev.json",
        "{ \"alias\": { \"@\": \"./src\" },\n  \
         \"targets\": { \"server\": { \"entry\": \"src/server.ts\", \
         \"out\": \"dist/server.js\" } } }\n",
    );

    let out = esdev_in(&dir)
        .arg("build")
        .output()
        .expect("spawn esdev build");
    assert!(out.status.success(), "{}", stderr(&out));
    let bundle = std::fs::read_to_string(dir.join("dist/server.js")).expect("read bundle");
    assert!(bundle.contains("helped"), "{bundle}");

    // …and the same rewrite off the command line, for an entry with no project
    // around it.
    let flagged = esdev_in(&dir)
        .args([
            "build",
            "src/server.ts",
            "--out=flag/server.js",
            "--alias=@=./src",
        ])
        .output()
        .expect("spawn esdev build");
    assert!(flagged.status.success(), "{}", stderr(&flagged));
    assert!(
        std::fs::read_to_string(dir.join("flag/server.js"))
            .expect("read bundle")
            .contains("helped")
    );

    // A published library keeps the specifier its source wrote — the consuming
    // build resolves it, and it has never heard of this alias.
    let lib = esdev_in(&dir)
        .args(["build", "--lib", "src", "--out=libout", "--alias=@=./src"])
        .output()
        .expect("spawn esdev build --lib");
    assert!(!lib.status.success(), "{}", stdout(&lib));
    assert!(
        stderr(&lib).contains("--lib does not bundle"),
        "{}",
        stderr(&lib)
    );
}

/// A browser bundle cannot read the environment at run time, so what it is
/// configured with has to be compiled in — and what is compiled in is public by
/// definition, which is what the prefix says out loud.
#[test]
fn import_meta_env_carries_the_public_variables_and_no_others() {
    let dir = build_dir("b_env");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(
        &dir,
        ".env",
        "PUBLIC_API=https://api.example.com\nSECRET_KEY=do-not-ship\n",
    );
    write_in(
        &dir,
        "src/app.ts",
        "export const api = import.meta.env.PUBLIC_API;\n\
         export const mode = import.meta.env.MODE;\n\
         export const dev = import.meta.env.DEV;\n\
         export const all = import.meta.env;\n",
    );

    let out = esdev_in(&dir)
        .args(["build", "src/app.ts", "--out=dist/app.js"])
        .output()
        .expect("spawn esdev build");
    assert!(out.status.success(), "{}", stderr(&out));
    let bundle = std::fs::read_to_string(dir.join("dist/app.js")).expect("read bundle");

    assert!(bundle.contains("https://api.example.com"), "{bundle}");
    assert!(bundle.contains("\"production\""), "{bundle}");
    // A boolean, not the string "false": `if (import.meta.env.DEV)` on a string
    // is a branch that always runs.
    assert!(bundle.contains("dev = false"), "{bundle}");
    // The object itself is replaced too, so destructuring it works.
    assert!(bundle.contains("\"PROD\": true"), "{bundle}");
    // The one that matters: what was not marked public did not travel.
    assert!(!bundle.contains("do-not-ship"), "{bundle}");
    assert!(!bundle.contains("SECRET_KEY"), "{bundle}");
}

/// The file is what the machine has checked out; the environment is what that
/// machine was told. The second wins.
#[test]
fn the_environment_beats_the_env_file() {
    let dir = build_dir("b_env_order");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(&dir, ".env", "PUBLIC_API=from-the-file\n");
    write_in(
        &dir,
        "src/app.ts",
        "export const api = import.meta.env.PUBLIC_API;\n",
    );

    let out = esdev_in(&dir)
        .args(["build", "src/app.ts", "--out=dist/app.js"])
        .env("PUBLIC_API", "from-the-environment")
        .output()
        .expect("spawn esdev build");
    assert!(out.status.success(), "{}", stderr(&out));
    let bundle = std::fs::read_to_string(dir.join("dist/app.js")).expect("read bundle");
    assert!(bundle.contains("from-the-environment"), "{bundle}");
    assert!(!bundle.contains("from-the-file"), "{bundle}");
}

/// A library is an input to somebody else's build, and which environment the
/// code runs in is that build's to decide — the same rule that keeps `--lib`
/// from defining `NODE_ENV`.
#[test]
fn a_library_compiles_in_no_environment() {
    let dir = build_dir("b_env_lib");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(&dir, ".env", "PUBLIC_API=https://api.example.com\n");
    write_in(
        &dir,
        "src/index.ts",
        "export const api: unknown = import.meta.env.PUBLIC_API;\n",
    );

    let out = esdev_in(&dir)
        .args(["build", "--lib", "src"])
        .output()
        .expect("spawn esdev build --lib");
    assert!(out.status.success(), "{}", stderr(&out));
    let module = std::fs::read_to_string(dir.join("dist/index.js")).expect("read module");
    assert!(module.contains("import.meta.env.PUBLIC_API"), "{module}");
    assert!(!module.contains("api.example.com"), "{module}");
}

// ---------------------------------------------------------------------------
// `esdev build --lib` (DECISIONS D59)
//
// The same command, for an artifact that is not deployed but published — so
// every default that is right for an application is wrong here, and each of
// these tests is one of those defaults not being applied. Every failure they
// guard is silent at build time and loud in somebody else's project.
// ---------------------------------------------------------------------------

/// A source tree with a subdirectory, an internal module, and an export that
/// only an outside caller would ever reach for.
fn lib_project(name: &str) -> PathBuf {
    let dir = build_dir(name);
    std::fs::create_dir_all(dir.join("src/protocol")).expect("create src");
    write_in(
        &dir,
        "src/protocol/codec.ts",
        // `UNUSED_BY_THE_ENTRY` is the point: no other module in this library
        // touches it, and it is still part of what the library exports.
        "export const UNUSED_BY_THE_ENTRY: readonly string[] = ['a', 'b'];\n\
         export function encode(value: string): string {\n  return `<${value}>`;\n}\n",
    );
    write_in(
        &dir,
        "src/index.ts",
        "import { encode } from './protocol/codec.js';\n\
         import { version } from 'some-dependency';\n\
         export function greet(name: string): string {\n  \
         return encode(name) + version;\n}\n",
    );
    dir
}

/// The layout `tsc` gives and a package's `exports` map is written against.
#[test]
fn lib_mirrors_the_source_tree_rather_than_bundling_it() {
    let dir = lib_project("l_tree");

    let out = esdev_in(&dir)
        .args(["build", "--lib", "src"])
        .output()
        .expect("spawn esdev build --lib");
    assert!(out.status.success(), "{}", stderr(&out));

    assert!(dir.join("dist/index.js").exists(), "{}", stdout(&out));
    assert!(
        dir.join("dist/protocol/codec.js").exists(),
        "{}",
        stdout(&out)
    );
    assert!(dir.join("dist/index.d.ts").exists(), "{}", stdout(&out));
    assert!(
        dir.join("dist/protocol/codec.d.ts").exists(),
        "{}",
        stdout(&out)
    );

    // The module boundary survives: `index.js` imports `codec.js` instead of
    // containing it, which is what makes a subpath export a real file.
    let index = std::fs::read_to_string(dir.join("dist/index.js")).expect("read index");
    assert!(index.contains("./protocol/codec.js"), "inlined:\n{index}");
}

/// The one found by building this repository's own Redis driver: shaking took
/// an export that only a *future* caller uses, and the failure surfaced as a
/// SyntaxError in the consumer rather than anything the build said.
#[test]
fn lib_keeps_an_export_no_other_module_uses() {
    let dir = lib_project("l_exports");

    let out = esdev_in(&dir)
        .args(["build", "--lib", "src"])
        .output()
        .expect("spawn esdev build --lib");
    assert!(out.status.success(), "{}", stderr(&out));

    let codec = std::fs::read_to_string(dir.join("dist/protocol/codec.js")).expect("read codec");
    assert!(
        codec.contains("UNUSED_BY_THE_ENTRY"),
        "an export was shaken out of a published module:\n{codec}"
    );

    // And it is genuinely importable, not merely present in the text.
    let Some(esrun) = sibling_binary("esrun") else {
        return;
    };
    write_in(
        &dir,
        "consumer.mjs",
        "import { UNUSED_BY_THE_ENTRY } from './dist/protocol/codec.js';\n\
         console.log(UNUSED_BY_THE_ENTRY.join(','));\n",
    );
    let ran = Command::new(esrun)
        .current_dir(&dir)
        .arg("--allow-imports")
        .arg("consumer.mjs")
        .output()
        .expect("spawn esrun");
    assert!(ran.status.success(), "{}", stderr(&ran));
    assert_eq!(stdout(&ran).trim(), "a,b");
}

/// Inlining a dependency publishes a private copy of it that no consumer can
/// dedupe, override or patch.
#[test]
fn lib_leaves_dependencies_external() {
    let dir = lib_project("l_external");

    let out = esdev_in(&dir)
        .args(["build", "--lib", "src"])
        .output()
        .expect("spawn esdev build --lib");
    // It resolves nothing, so a dependency that is not even installed is fine.
    assert!(out.status.success(), "{}", stderr(&out));

    let index = std::fs::read_to_string(dir.join("dist/index.js")).expect("read index");
    assert!(index.contains("some-dependency"), "{index}");
}

/// `NODE_ENV` and `worker` are the consuming build's decisions. Baking either
/// one in freezes somebody else's environment into your package.
#[test]
fn lib_defines_nothing_and_asserts_no_condition() {
    let dir = build_dir("l_neutral");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(
        &dir,
        "src/index.js",
        "export const mode = process.env.NODE_ENV;\n",
    );

    let out = esdev_in(&dir)
        .args(["build", "--lib", "src"])
        .output()
        .expect("spawn esdev build --lib");
    assert!(out.status.success(), "{}", stderr(&out));

    let index = std::fs::read_to_string(dir.join("dist/index.js")).expect("read index");
    assert!(
        index.contains("process.env.NODE_ENV"),
        "the consumer's decision was made for them:\n{index}"
    );
    assert!(!index.contains("production"), "{index}");
}

/// A `.d.ts` is what makes the package a typed contract, and it is derived from
/// the annotations the source already carries.
#[test]
fn lib_emits_declarations_and_no_types_skips_them() {
    let dir = lib_project("l_types");

    let out = esdev_in(&dir)
        .args(["build", "--lib", "src"])
        .output()
        .expect("spawn esdev build --lib");
    assert!(out.status.success(), "{}", stderr(&out));
    let declaration = std::fs::read_to_string(dir.join("dist/index.d.ts")).expect("read d.ts");
    assert!(
        declaration.contains("declare function greet"),
        "{declaration}"
    );
    assert!(declaration.contains("string"), "{declaration}");
    // The contract, not the implementation.
    assert!(!declaration.contains("encode(name)"), "{declaration}");
    assert!(stdout(&out).contains("declaration"), "{}", stdout(&out));

    let skipped = esdev_in(&dir)
        .args(["build", "--lib", "src", "--out=nodts", "--no-types"])
        .output()
        .expect("spawn esdev build --lib");
    assert!(skipped.status.success(), "{}", stderr(&skipped));
    assert!(dir.join("nodts/index.js").exists());
    assert!(!dir.join("nodts/index.d.ts").exists());
}

// ---------------------------------------------------------------------------
// `--format=cjs`: the same library, for consumers who are not on this runtime
//
// The runtime loads ES modules and nothing else, and that does not move: this
// is an *output*, for the Node programs that will `require()` a package built
// here. What each of these guards is a package that installs cleanly and then
// fails in somebody else's project — an unloadable `require`, or types that
// resolve to the wrong half of the package.
// ---------------------------------------------------------------------------

/// Node, for the one question only it can answer: whether what was written for
/// a `require()` actually loads under one. Absent on a machine without it, and
/// the test then checks the text alone.
fn node_binary() -> Option<&'static str> {
    Command::new("node")
        .arg("--version")
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|_| "node")
}

/// Both trees in one directory, because that is what a dual `exports` map
/// names: `./dist/index.js` for an `import` and `./dist/index.cjs` for a
/// `require`, with the module structure preserved in both.
#[test]
fn lib_writes_both_module_systems_into_one_tree() {
    let dir = lib_project("l_dual");

    let out = esdev_in(&dir)
        .args(["build", "--lib", "src", "--format=esm,cjs"])
        .output()
        .expect("spawn esdev build --lib");
    assert!(out.status.success(), "{}", stderr(&out));

    assert!(dir.join("dist/index.js").exists(), "{}", stdout(&out));
    assert!(dir.join("dist/index.cjs").exists(), "{}", stdout(&out));
    assert!(dir.join("dist/protocol/codec.cjs").exists());

    // The module boundary survives the conversion, pointed at the CommonJS
    // sibling rather than the ES one — a `require` of `./protocol/codec.js`
    // would load the module the other half of the package is made of.
    let index = std::fs::read_to_string(dir.join("dist/index.cjs")).expect("read index.cjs");
    assert!(
        index.contains("require(\"./protocol/codec.cjs\")"),
        "{index}"
    );
    // …and a dependency is still a dependency, now spelled as a require.
    assert!(index.contains("require(\"some-dependency\")"), "{index}");

    // A module is one module however many spellings of it were written: the
    // count is of modules, and the formats are named because they are not the
    // default.
    let report = stdout(&out);
    assert!(report.contains("2 modules"), "{report}");
    assert!(report.contains("esm + cjs"), "{report}");
}

/// The half that text cannot prove: `require()` of the output returns the
/// library's exports, under the runtime it was built for.
#[test]
fn a_commonjs_output_loads_under_node() {
    let Some(node) = node_binary() else {
        return;
    };
    let dir = build_dir("l_cjs_runs");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(
        &dir,
        "src/greeting.ts",
        "export const greet = (name: string): string => `hi ${name}`;\n",
    );
    write_in(
        &dir,
        "src/index.ts",
        "import { greet } from './greeting.js';\n\
         export const version: string = '1';\n\
         export function hello(name: string): string {\n  return greet(name);\n}\n\
         export default hello;\n",
    );

    let out = esdev_in(&dir)
        .args(["build", "--lib", "src", "--format=cjs"])
        .output()
        .expect("spawn esdev build --lib");
    assert!(out.status.success(), "{}", stderr(&out));

    let ran = Command::new(node)
        .current_dir(&dir)
        .arg("-e")
        .arg(
            "const m = require('./dist/index.cjs');\n\
             console.log(m.hello('you'), m.version, typeof m.default);",
        )
        .output()
        .expect("spawn node");
    assert!(ran.status.success(), "{}", stderr(&ran));
    // `default` is a property rather than the module: named exports and a
    // default one cannot both be the value `require` returns.
    assert_eq!(stdout(&ran).trim(), "hi you 1 function");
}

/// A `.cjs` is typed by the `.d.cts` beside it and by nothing else, and that
/// declaration has to import its siblings by the names *it* can load.
#[test]
fn lib_types_the_commonjs_output_with_declarations_of_its_own() {
    let dir = build_dir("l_dcts");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(
        &dir,
        "src/row.ts",
        "export type Row = { readonly id: string };\n",
    );
    write_in(
        &dir,
        "src/index.ts",
        "export type { Row } from './row.js';\n\
         export declare const table: string;\n",
    );

    let out = esdev_in(&dir)
        .args(["build", "--lib", "src", "--format=esm,cjs"])
        .output()
        .expect("spawn esdev build --lib");
    assert!(out.status.success(), "{}", stderr(&out));

    let esm = std::fs::read_to_string(dir.join("dist/index.d.ts")).expect("read d.ts");
    assert!(esm.contains("\"./row.js\""), "{esm}");

    let cjs = std::fs::read_to_string(dir.join("dist/index.d.cts")).expect("read d.cts");
    assert!(cjs.contains("\"./row.cjs\""), "{cjs}");
    assert!(dir.join("dist/row.d.cts").exists());

    // Two module systems, so two declarations per module rather than two
    // modules — the count says which.
    let report = stdout(&out);
    assert!(report.contains("2 modules"), "{report}");
    assert!(report.contains("4 declarations"), "{report}");
}

/// A library written for a bundler spells a sibling `./pool`, with no
/// extension — and a declaration that repeats that spelling is TS2835 in the ESM
/// half and resolves to the *ESM* declaration in the CommonJS half. Both are
/// errors only the consumer sees. Found by building `@opentf/std`: 717 errors
/// across its 334 modules, none of them visible to the build that wrote them.
#[test]
fn a_declaration_names_the_files_the_build_wrote() {
    let dir = build_dir("l_specifiers");
    std::fs::create_dir_all(dir.join("src/colors")).expect("create src");
    write_in(
        &dir,
        "src/pool.ts",
        "export type Pool = { size: number };\n",
    );
    write_in(
        &dir,
        "src/colors/index.ts",
        "export type Rgb = readonly [number, number, number];\n",
    );
    write_in(
        &dir,
        "src/index.ts",
        // Extensionless: a file, and a directory reached through its index.
        "export type { Pool } from './pool';\n\
         export type { Rgb } from './colors';\n\
         export interface Options {\n  \
         readonly pool?: import('./pool').Pool;\n}\n",
    );

    let out = esdev_in(&dir)
        .args(["build", "--lib", "src", "--format=esm,cjs"])
        .output()
        .expect("spawn esdev build --lib");
    assert!(out.status.success(), "{}", stderr(&out));

    let esm = std::fs::read_to_string(dir.join("dist/index.d.ts")).expect("read d.ts");
    assert!(esm.contains("\"./pool.js\""), "{esm}");
    assert!(esm.contains("\"./colors/index.js\""), "{esm}");
    assert!(esm.contains("import(\"./pool.js\")"), "{esm}");

    let cjs = std::fs::read_to_string(dir.join("dist/index.d.cts")).expect("read d.cts");
    assert!(cjs.contains("\"./pool.cjs\""), "{cjs}");
    assert!(cjs.contains("\"./colors/index.cjs\""), "{cjs}");
    assert!(cjs.contains("import(\"./pool.cjs\")"), "{cjs}");

    // Which is what the JavaScript beside them says: the declarations were the
    // only half of the output still spelling a module the source's way.
    let module = std::fs::read_to_string(dir.join("dist/index.js")).expect("read index.js");
    assert!(!module.contains("\"./pool\""), "{module}");
}

/// CommonJS alone is a package for consumers who are only on Node. There is no
/// `.js` in it, and the declarations follow the output rather than the default.
#[test]
fn lib_can_write_commonjs_alone() {
    let dir = lib_project("l_cjs_only");

    let out = esdev_in(&dir)
        .args(["build", "--lib", "src", "--format=cjs"])
        .output()
        .expect("spawn esdev build --lib");
    assert!(out.status.success(), "{}", stderr(&out));

    assert!(dir.join("dist/index.cjs").exists(), "{}", stdout(&out));
    assert!(dir.join("dist/index.d.cts").exists());
    assert!(!dir.join("dist/index.js").exists(), "{}", stdout(&out));
    assert!(!dir.join("dist/index.d.ts").exists());
    let report = stdout(&out);
    assert!(report.contains("2 modules"), "{report}");
    assert!(report.contains("2 declarations"), "{report}");
}

/// `--dts-bundle` collapses the tree into one declaration — one per module
/// system, since a `require` resolves types by the extension too.
#[test]
fn dts_bundle_writes_one_declaration_per_module_system() {
    let dir = lib_project("l_dual_bundle");

    let out = esdev_in(&dir)
        .args(["build", "--lib", "src", "--format=esm,cjs", "--dts-bundle"])
        .output()
        .expect("spawn esdev build --lib");
    assert!(out.status.success(), "{}", stderr(&out));

    assert!(dir.join("dist/index.d.ts").exists(), "{}", stdout(&out));
    assert!(dir.join("dist/index.d.cts").exists(), "{}", stdout(&out));
    // The per-module declarations are what a bundle replaces, in both.
    assert!(!dir.join("dist/protocol/codec.d.ts").exists());
    assert!(!dir.join("dist/protocol/codec.d.cts").exists());
}

/// A `runtime:` import is served by esrun and by nothing else. The CommonJS
/// output keeps it — it has no file behind it to inline — so the build says so
/// rather than leaving a `require` nobody can resolve to be found on install.
#[test]
fn lib_says_when_a_commonjs_output_imports_a_runtime_module() {
    let dir = build_dir("l_cjs_runtime");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(
        &dir,
        "src/index.ts",
        "import { readFile } from 'runtime:fs';\n\
         export async function read(path: string): Promise<string> {\n  \
         return readFile(path);\n}\n",
    );

    let out = esdev_in(&dir)
        .args(["build", "--lib", "src", "--format=esm,cjs"])
        .output()
        .expect("spawn esdev build --lib");
    assert!(out.status.success(), "{}", stderr(&out));
    let report = stdout(&out);
    assert!(report.contains("runtime:fs"), "{report}");

    // The ES half says nothing, because there is nothing wrong with it.
    let esm_only = esdev_in(&dir)
        .args(["build", "--lib", "src", "--out=esmonly"])
        .output()
        .expect("spawn esdev build --lib");
    assert!(esm_only.status.success(), "{}", stderr(&esm_only));
    assert!(
        !stdout(&esm_only).contains("runtime:fs"),
        "{}",
        stdout(&esm_only)
    );
}

/// CommonJS has no asynchronous module, so a top-level `await` cannot be
/// converted. The build stops with the position, names the pass it stopped in —
/// the two write the same source and their diagnostics are otherwise identical
/// — and lands nothing, the ES half included.
#[test]
fn lib_refuses_a_top_level_await_in_the_commonjs_output() {
    let dir = build_dir("l_cjs_tla");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(
        &dir,
        "src/index.ts",
        "export const ready: number = await Promise.resolve(1);\n",
    );
    std::fs::create_dir_all(dir.join("dist")).expect("create dist");
    write_in(&dir, "dist/previous.js", "export const old = 1;\n");

    let out = esdev_in(&dir)
        .args(["build", "--lib", "src", "--format=esm,cjs"])
        .output()
        .expect("spawn esdev build --lib");
    assert!(!out.status.success(), "{}", stdout(&out));
    let message = slash_paths(&stderr(&out));
    assert!(message.contains("Top-level await"), "{message}");
    assert!(message.contains("src/index.ts"), "{message}");
    assert!(message.contains("cjs output"), "{message}");

    // Nothing landed: not the ES module the first pass built, and not over the
    // output that was already there.
    assert!(dir.join("dist/previous.js").exists(), "{message}");
    assert!(!dir.join("dist/index.js").exists(), "{message}");
    assert!(!dir.join("dist/index.cjs").exists(), "{message}");

    // …and ESM alone is still buildable, which is what makes the message
    // actionable rather than a dead end.
    let esm_only = esdev_in(&dir)
        .args(["build", "--lib", "src"])
        .output()
        .expect("spawn esdev build --lib");
    assert!(esm_only.status.success(), "{}", stderr(&esm_only));
}

/// An application's output is loaded by esrun, which loads ES modules and
/// nothing else. `--format` there would be a flag with one legal value.
#[test]
fn format_is_refused_without_lib_and_for_a_module_system_this_does_not_write() {
    let dir = build_dir("l_format_args");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(&dir, "src/index.ts", "export const x: number = 1;\n");

    let application = esdev_in(&dir)
        .args(["build", "src/index.ts", "--format=cjs"])
        .output()
        .expect("spawn esdev build");
    assert!(!application.status.success(), "{}", stdout(&application));
    assert!(
        stderr(&application).contains("--lib"),
        "{}",
        stderr(&application)
    );

    let unknown = esdev_in(&dir)
        .args(["build", "--lib", "src", "--format=umd"])
        .output()
        .expect("spawn esdev build --lib");
    assert!(!unknown.status.success(), "{}", stdout(&unknown));
    let message = stderr(&unknown);
    assert!(message.contains("esm or cjs"), "{message}");
}

/// A guessed declaration would be believed. The build stops instead, and names
/// every signature that has to say its type rather than only the first.
#[test]
fn lib_refuses_to_guess_a_declaration_it_cannot_derive() {
    let dir = build_dir("l_underivable");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(
        &dir,
        "src/index.ts",
        "export const a = (() => 1)();\nexport const b = (() => 2)();\n",
    );

    let out = esdev_in(&dir)
        .args(["build", "--lib", "src"])
        .output()
        .expect("spawn esdev build --lib");
    assert!(!out.status.success(), "{}", stdout(&out));
    let message = slash_paths(&stderr(&out));
    assert!(message.contains("src/index.ts:1:"), "{message}");
    assert!(message.contains("src/index.ts:2:"), "{message}");
    assert!(message.contains("--no-types"), "{message}");
    // The flag named is one this command line actually has.
    assert!(!message.contains("isolatedDeclarations"), "{message}");

    // …and --no-types is genuinely the way past it.
    let skipped = esdev_in(&dir)
        .args(["build", "--lib", "src", "--no-types"])
        .output()
        .expect("spawn esdev build --lib");
    assert!(skipped.status.success(), "{}", stderr(&skipped));
}

/// A stale file in a library's output is a **published** file: `"files":
/// ["dist"]` puts it in the tarball, where a consumer can still import a module
/// the library no longer has.
#[test]
fn lib_empties_its_output_so_a_deleted_module_stops_shipping() {
    let dir = lib_project("l_clean");

    write_in(&dir, "src/dropped.ts", "export const old: number = 1;\n");
    let first = esdev_in(&dir)
        .args(["build", "--lib", "src"])
        .output()
        .expect("spawn esdev build --lib");
    assert!(first.status.success(), "{}", stderr(&first));
    assert!(dir.join("dist/dropped.js").exists());
    assert!(dir.join("dist/dropped.d.ts").exists());

    std::fs::remove_file(dir.join("src/dropped.ts")).expect("remove source");
    // Something that never had a source at all, which is the other half of the
    // problem: without a clean it is published for ever.
    write_in(
        &dir,
        "dist/never-had-a-source.js",
        "export const junk = 1;\n",
    );

    let second = esdev_in(&dir)
        .args(["build", "--lib", "src"])
        .output()
        .expect("spawn esdev build --lib");
    assert!(second.status.success(), "{}", stderr(&second));
    assert!(!dir.join("dist/dropped.js").exists(), "{}", stdout(&second));
    assert!(!dir.join("dist/dropped.d.ts").exists());
    assert!(!dir.join("dist/never-had-a-source.js").exists());
    // Still built what it should have.
    assert!(dir.join("dist/index.js").exists());
    assert!(dir.join("dist/protocol/codec.js").exists());

    // And the count is the build's, not the directory's leftovers — the same
    // clean is what makes reading the modules back off disk honest.
    assert!(stdout(&second).contains("2 modules"), "{}", stdout(&second));
}

/// `--out=src` is one keystroke from `--out=dist`, and a build that empties its
/// output first would delete the library rather than build it.
#[test]
fn lib_refuses_to_empty_a_directory_holding_the_source() {
    let dir = lib_project("l_clean_guard");

    let onto_source = esdev_in(&dir)
        .args(["build", "--lib", "src", "--out=src"])
        .output()
        .expect("spawn esdev");
    assert!(!onto_source.status.success(), "{}", stdout(&onto_source));
    assert!(
        stderr(&onto_source).contains("holds the source"),
        "{}",
        stderr(&onto_source)
    );

    let onto_project = esdev_in(&dir)
        .args(["build", "--lib", "src", "--out=."])
        .output()
        .expect("spawn esdev");
    assert!(!onto_project.status.success(), "{}", stdout(&onto_project));

    // Nothing was deleted on the way to either refusal.
    assert!(dir.join("src/index.ts").exists());
    assert!(dir.join("src/protocol/codec.ts").exists());
}

/// An application build's `--out` names a file, in a directory that may hold
/// other builds and other people's files. Emptying it would be a surprise with
/// no upside, since the one file is overwritten anyway.
#[test]
fn an_application_build_leaves_the_rest_of_the_directory_alone() {
    let dir = build_dir("b_no_clean");
    write_in(&dir, "app.mjs", "console.log(1);\n");
    std::fs::create_dir_all(dir.join("dist")).expect("create dist");
    write_in(&dir, "dist/keep-me.txt", "not the build's to delete\n");

    let out = esdev_in(&dir)
        .args(["build", "app.mjs"])
        .output()
        .expect("spawn esdev build");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(dir.join("dist/app.js").exists());
    assert!(dir.join("dist/keep-me.txt").exists(), "{}", stdout(&out));
}

/// Whether a build left its staging directory behind — one is created inside
/// the project and removed whether the build worked or not.
fn staging_left(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|entries| {
        entries.flatten().any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".esdev-build-")
        })
    })
}

/// **A failed build changes nothing.** The report that prompted this: a react
/// static project with no `node_modules` wrote its browser assets, its
/// `index.html` and its prerender bundle, and *then* failed in the prerender
/// step — leaving a `dist` that is a site whose pages were never rendered, and
/// leaving it where the previous, working deployment had been. In CI that is
/// what gets uploaded.
#[test]
fn a_failed_build_leaves_the_previous_output_where_it_was() {
    let dir = build_dir("b_fail_keeps");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(&dir, "src/app.ts", "console.log('app');\n");
    write_in(
        &dir,
        "src/prerender.ts",
        "throw new Error('the step that fails after everything is written');\n",
    );
    write_in(
        &dir,
        "esdev.json",
        r#"{ "targets": {
              "web": { "entry": "src/app.ts", "outdir": "dist" },
              "prerender": { "entry": "src/prerender.ts", "out": "dist/prerender.js",
                             "then": "run" } } }"#,
    );
    std::fs::create_dir_all(dir.join("dist")).expect("create dist");
    write_in(&dir, "dist/index.html", "the deployment that works\n");

    let out = esdev_in(&dir)
        .arg("build")
        .output()
        .expect("spawn esdev build");
    assert!(!out.status.success(), "{}", stdout(&out));

    // What was deployed is still deployed, whole …
    assert_eq!(
        std::fs::read_to_string(dir.join("dist/index.html")).expect("read"),
        "the deployment that works\n"
    );
    // … and nothing of the build that failed is anywhere near it.
    for partial in ["dist/app.js", "dist/prerender.js"] {
        assert!(
            !dir.join(partial).exists(),
            "{partial} was left behind by a failed build:\n{}{}",
            stdout(&out),
            stderr(&out)
        );
    }
    assert!(
        !staging_left(&dir),
        "the staging directory outlived the build"
    );

    // Said outright, because a `dist` that was left alone looks exactly like one
    // that was rebuilt — and the paths in whatever the failing step printed name
    // a directory that no longer exists.
    assert!(
        stderr(&out).contains("Nothing was written"),
        "{}",
        stderr(&out)
    );
}

/// The same property one step earlier: a target that cannot be bundled at all.
/// The first target had already succeeded, and its output must not be visible
/// either — half a deployment is not a deployment.
#[test]
fn a_target_that_fails_to_bundle_leaves_nothing_of_the_run() {
    let dir = build_dir("b_fail_target");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(&dir, "src/first.ts", "console.log('fine');\n");
    write_in(&dir, "src/second.ts", "const broken = ;\n");
    write_in(
        &dir,
        "esdev.json",
        r#"{ "targets": {
              "first": { "entry": "src/first.ts", "out": "dist/first.js" },
              "second": { "entry": "src/second.ts", "out": "dist/second.js" } } }"#,
    );

    let out = esdev_in(&dir)
        .arg("build")
        .output()
        .expect("spawn esdev build");
    assert!(!out.status.success(), "{}", stdout(&out));
    assert!(
        !dir.join("dist/first.js").exists(),
        "the target that worked was left behind by the run that did not:\n{}",
        stdout(&out)
    );
    assert!(
        !staging_left(&dir),
        "the staging directory outlived the build"
    );
}

/// A hashed filename changes when its contents do, and the old one has nothing
/// to overwrite it. Without this, what gets deployed is every build the
/// directory has ever seen — plus whatever `esdev start` left, which is not
/// hashed and so is never replaced either.
#[test]
fn a_whole_project_build_clears_the_directories_it_owns() {
    let dir = build_dir("b_clean_targets");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(&dir, "src/server.ts", "console.log('server');\n");
    write_in(&dir, "src/app.ts", "console.log('app');\n");
    write_in(
        &dir,
        "esdev.json",
        r#"{ "targets": {
              "server": { "entry": "src/server.ts", "out": "dist/server.js" },
              "web": { "entry": "src/app.ts", "outdir": "dist" } } }"#,
    );
    std::fs::create_dir_all(dir.join("dist")).expect("create dist");
    write_in(&dir, "dist/app-0000dead.js", "a build from last week\n");

    let out = esdev_in(&dir)
        .arg("build")
        .output()
        .expect("spawn esdev build");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        !dir.join("dist/app-0000dead.js").exists(),
        "the stale bundle is still there: {}",
        stdout(&out)
    );
    // And everything this build wrote is present, including the `out` file that
    // sits inside the directory the `outdir` target cleared.
    assert!(dir.join("dist/server.js").exists(), "{}", stdout(&out));
    assert!(dir.join("dist/app.js").exists(), "{}", stdout(&out));
}

/// Building one target does not own the directory it shares with another, so
/// clearing it would delete a bundle this run is not going to write again.
#[test]
fn building_one_target_leaves_the_other_s_output_alone() {
    let dir = build_dir("b_clean_one_target");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(&dir, "src/server.ts", "console.log('server');\n");
    write_in(&dir, "src/app.ts", "console.log('app');\n");
    write_in(
        &dir,
        "esdev.json",
        r#"{ "targets": {
              "server": { "entry": "src/server.ts", "out": "dist/server.js" },
              "web": { "entry": "src/app.ts", "outdir": "dist" } } }"#,
    );

    assert!(
        esdev_in(&dir)
            .arg("build")
            .output()
            .expect("spawn esdev build")
            .status
            .success()
    );
    let out = esdev_in(&dir)
        .args(["build", "--target=web"])
        .output()
        .expect("spawn esdev build");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        dir.join("dist/server.js").exists(),
        "--target=web deleted the server bundle: {}",
        stdout(&out)
    );
}

/// The keystroke `--lib` guards against, arriving through the config instead.
#[test]
fn an_outdir_that_holds_the_project_is_refused_rather_than_emptied() {
    let dir = build_dir("b_clean_refuses_root");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(&dir, "src/app.ts", "console.log('app');\n");
    write_in(
        &dir,
        "esdev.json",
        r#"{ "targets": { "web": { "entry": "src/app.ts", "outdir": "." } } }"#,
    );

    let out = esdev_in(&dir)
        .arg("build")
        .output()
        .expect("spawn esdev build");
    assert!(!out.status.success(), "{}", stdout(&out));
    assert!(
        stderr(&out).contains("holds the project"),
        "{}",
        stderr(&out)
    );
    // Nothing was deleted on the way to the refusal.
    assert!(dir.join("src/app.ts").exists());
    assert!(dir.join("esdev.json").exists());
}

// ---------------------------------------------------------------------------
// `esdev build --lib --dts-bundle` (DECISIONS D59)
//
// One declaration file, linked from many. Neither tsc nor rolldown can do this
// — tsc has no declaration-bundling mode and rolldown's Rust crates have no
// .d.ts support — so every property below is one this bundler has to hold up on
// its own, and each is checked against output rather than against intent.
// ---------------------------------------------------------------------------

/// A library whose declarations only link correctly if collisions, cycles,
/// re-exports and externals are all handled.
fn dts_project(name: &str) -> PathBuf {
    let dir = build_dir(name);
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    // Two modules, one name. Only one can keep it.
    write_in(
        &dir,
        "src/a.ts",
        "/** A's own Options. */\n\
         export interface Options {\n\ta: string;\n}\n\
         export interface Wrap {\n\to: Options;\n}\n",
    );
    write_in(
        &dir,
        "src/b.ts",
        "export interface Options {\n\tb: number;\n}\n\
         export type Boxed = {\n\tinner: Options;\n\tlist: Options[];\n};\n",
    );
    // A type cycle, which is ordinary in a tree structure and must not recurse
    // for ever.
    write_in(
        &dir,
        "src/tree.ts",
        "import type { Leaf } from './leaf.js';\n\
         export interface Tree {\n\tchildren: Leaf[];\n}\n",
    );
    write_in(
        &dir,
        "src/leaf.ts",
        "import type { Tree } from './tree.js';\n\
         export interface Leaf {\n\tparent: Tree | null;\n}\n",
    );
    // Reachable only *through* a public type — it has to be inlined, and it
    // must not become part of the package's surface.
    write_in(
        &dir,
        "src/internal.ts",
        "export interface Hidden {\n\th: boolean;\n}\n",
    );
    write_in(
        &dir,
        "src/index.ts",
        "import type { Options as AOptions, Wrap } from './a.js';\n\
         import type { Boxed } from './b.js';\n\
         import type { Tree } from './tree.js';\n\
         import type { Hidden } from './internal.js';\n\
         import type { Outside } from 'a-package';\n\
         export type { Wrap, Boxed, Tree };\n\
         export interface Everything {\n\ta: AOptions;\n\tboxed: Boxed;\n\t\
         tree: Tree;\n\thidden: Hidden;\n\toutside: Outside;\n}\n",
    );
    dir
}

/// `export default function f(): string;` carries no `declare` — the export
/// modifier is what made it a declaration — so inlining it as
/// `function f(): string;` is TS1046, a top-level declaration in a `.d.ts` with
/// neither modifier. Found by bundling `@opentf/std`, where every utility is a
/// default export: 317 of them in one file, and `tsc` rejecting the package.
#[test]
fn dts_bundle_keeps_declare_on_an_inlined_default_export() {
    let dir = build_dir("l_dts_declare");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(
        &dir,
        "src/splice.ts",
        "export default function splice(s: string): string {\n  return s;\n}\n",
    );
    write_in(
        &dir,
        "src/box.ts",
        "export default class Box {\n  readonly size: number = 1;\n}\n",
    );
    write_in(
        &dir,
        "src/index.ts",
        "export { default as splice } from './splice.js';\n\
         export { default as Box } from './box.js';\n",
    );

    let out = esdev_in(&dir)
        .args(["build", "--lib", "src", "--dts-bundle"])
        .output()
        .expect("spawn esdev build --lib");
    assert!(out.status.success(), "{}", stderr(&out));

    let text = bundled(&dir);
    assert!(text.contains("declare function splice"), "{text}");
    assert!(text.contains("declare class Box"), "{text}");
    // The modifier is added, not doubled: a named export already has one.
    assert!(!text.contains("declare declare"), "{text}");
}

/// An inline `import("./x")` is a reference to another module of the library,
/// and linking resolves import *statements* — so it would survive into a single
/// `index.d.ts` with nothing beside it to resolve to (TS2307, in the consumer's
/// editor, from a build that said it succeeded). It is refused instead, which is
/// what this linker does with every construct it cannot link.
#[test]
fn dts_bundle_refuses_an_import_type_it_cannot_link() {
    let dir = build_dir("l_dts_import_type");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(&dir, "src/clock.ts", "export default class Clock {}\n");
    write_in(
        &dir,
        "src/index.ts",
        "export interface Options {\n  \
         readonly at?: import('./clock.js').default;\n}\n",
    );

    let out = esdev_in(&dir)
        .args(["build", "--lib", "src", "--dts-bundle"])
        .output()
        .expect("spawn esdev build --lib");
    assert!(!out.status.success(), "{}", stdout(&out));
    let message = slash_paths(&stderr(&out));
    assert!(message.contains("import(\"./clock.js\")"), "{message}");
    // The module is named as it is spelled, not as a specifier joined onto a
    // directory: `src/./index.ts` is what that produces.
    assert!(message.contains("src/index.ts"), "{message}");
    assert!(!message.contains("/./"), "{message}");
    // Surface-neutral: the same refusal reaches a reader who wrote
    // `--dts-bundle` and one who wrote `"dts-bundle": true`.
    assert!(message.contains("declaration bundle"), "{message}");

    // A package is not this library's to link, and is left alone.
    write_in(
        &dir,
        "src/index.ts",
        "export interface Options {\n  \
         readonly at?: import('a-package').Clock;\n}\n",
    );
    let external = esdev_in(&dir)
        .args(["build", "--lib", "src", "--dts-bundle"])
        .output()
        .expect("spawn esdev build --lib");
    assert!(external.status.success(), "{}", stderr(&external));
    assert!(
        bundled(&dir).contains("import(\"a-package\")"),
        "{}",
        bundled(&dir)
    );
}

fn bundled(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("dist/index.d.ts")).expect("read bundled declarations")
}

#[test]
fn dts_bundle_writes_one_declaration_instead_of_a_tree_of_them() {
    let dir = dts_project("d_one");

    let out = esdev_in(&dir)
        .args(["build", "--lib", "src", "--dts-bundle"])
        .output()
        .expect("spawn esdev build --lib --dts-bundle");
    assert!(out.status.success(), "{}", stderr(&out));

    assert!(dir.join("dist/index.d.ts").exists(), "{}", stdout(&out));
    // The per-module declarations are what it replaces, not what it joins.
    assert!(!dir.join("dist/a.d.ts").exists());
    assert!(!dir.join("dist/b.d.ts").exists());
    // The JavaScript tree is untouched: only the declarations were linked.
    assert!(dir.join("dist/a.js").exists());
    assert!(dir.join("dist/index.js").exists());

    let text = bundled(&dir);
    // Nothing relative survives — a bundle that still imported `./a.js` would
    // be a declaration file pointing at declarations that are no longer there.
    assert!(!text.contains("./a.js"), "{text}");
    assert!(!text.contains("./b.js"), "{text}");
}

/// The pass that is easy to get subtly wrong. A missed site leaves a name
/// bound to the wrong declaration, in a file no test of the library runs.
#[test]
fn dts_bundle_renames_a_collision_and_rewrites_every_site_of_it() {
    let dir = dts_project("d_collide");
    let out = esdev_in(&dir)
        .args(["build", "--lib", "src", "--dts-bundle"])
        .output()
        .expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));

    let text = bundled(&dir);
    // One `Options` keeps the name and the other is suffixed…
    assert!(text.contains("interface Options {"), "{text}");
    assert!(text.contains("interface Options$1 {"), "{text}");
    // …and B's type refers to the renamed one in *both* of its positions, not
    // just the first.
    assert!(text.contains("inner: Options$1;"), "{text}");
    assert!(text.contains("list: Options$1[];"), "{text}");
    // A's `Wrap` still names A's `Options`, unrenamed.
    assert!(text.contains("o: Options;"), "{text}");
}

/// A type only reachable through a public one has to be present, or the public
/// type means nothing — but exporting it would widen the package's surface past
/// what its author wrote.
#[test]
fn dts_bundle_inlines_what_is_reachable_and_exports_only_what_the_entry_did() {
    let dir = dts_project("d_surface");
    let out = esdev_in(&dir)
        .args(["build", "--lib", "src", "--dts-bundle"])
        .output()
        .expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));

    let text = bundled(&dir);
    assert!(text.contains("interface Hidden {"), "inlined:\n{text}");

    let exports = text
        .lines()
        .find(|line| line.starts_with("export {"))
        .unwrap_or_default();
    for public in ["Wrap", "Boxed", "Tree", "Everything"] {
        assert!(exports.contains(public), "{public} missing from {exports}");
    }
    assert!(!exports.contains("Hidden"), "{exports}");
    assert!(!exports.contains("Options"), "{exports}");
}

/// A tree whose nodes point at their parent is ordinary, and a bundler that
/// followed it naively would not terminate.
#[test]
fn dts_bundle_follows_a_cycle_without_recursing_for_ever() {
    let dir = dts_project("d_cycle");
    let out = esdev_in(&dir)
        .args(["build", "--lib", "src", "--dts-bundle"])
        .output()
        .expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));

    let text = bundled(&dir);
    assert!(text.contains("interface Tree {"), "{text}");
    assert!(text.contains("interface Leaf {"), "{text}");
    assert!(text.contains("children: Leaf[];"), "{text}");
    assert!(text.contains("parent: Tree | null;"), "{text}");
}

/// The same line `--lib` draws for JavaScript: a dependency stays a dependency.
/// Inlining a package's types would publish a private copy of them.
#[test]
fn dts_bundle_leaves_a_package_as_an_import() {
    let dir = dts_project("d_external");
    let out = esdev_in(&dir)
        .args(["build", "--lib", "src", "--dts-bundle"])
        .output()
        .expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));

    let text = bundled(&dir);
    // `import type`, because that is how the source asked for it.
    assert!(
        text.contains("import type { Outside } from \"a-package\";"),
        "{text}"
    );
    assert!(text.contains("outside: Outside;"), "{text}");
}

/// The comments in a declaration file are its documentation — an editor shows
/// them on hover. Carrying declarations as text rather than as an AST is what
/// keeps them, and this is the test that says so.
#[test]
fn dts_bundle_keeps_jsdoc_byte_for_byte() {
    let dir = dts_project("d_jsdoc");
    let out = esdev_in(&dir)
        .args(["build", "--lib", "src", "--dts-bundle"])
        .output()
        .expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));

    assert!(
        bundled(&dir).contains("/** A's own Options. */"),
        "{}",
        bundled(&dir)
    );
}

#[test]
fn dts_bundle_rejects_what_it_cannot_be_asked_for() {
    let dir = dts_project("d_args");

    // Without --lib there are no declarations to link.
    let no_lib = esdev_in(&dir)
        .args(["build", "src/index.ts", "--dts-bundle"])
        .output()
        .expect("spawn esdev");
    assert!(!no_lib.status.success());
    assert!(stderr(&no_lib).contains("--lib"), "{}", stderr(&no_lib));

    // …and with --no-types there are none either.
    let contradiction = esdev_in(&dir)
        .args(["build", "--lib", "src", "--dts-bundle", "--no-types"])
        .output()
        .expect("spawn esdev");
    assert!(!contradiction.status.success());
    assert!(
        stderr(&contradiction).contains("opposite"),
        "{}",
        stderr(&contradiction)
    );

    // A default entry that is not there names itself rather than failing later.
    let empty = build_dir("d_args_empty");
    std::fs::create_dir_all(empty.join("src")).expect("create src");
    write_in(&empty, "src/other.ts", "export const x: number = 1;\n");
    let missing = esdev_in(&empty)
        .args(["build", "--lib", "src", "--dts-bundle"])
        .output()
        .expect("spawn esdev");
    assert!(!missing.status.success());
    assert!(
        stderr(&missing).contains("no index.ts"),
        "{}",
        stderr(&missing)
    );
}

/// The honest half. Each of these needs a synthesised namespace to mean the
/// same thing in one file, and a `.d.ts` that is wrong is believed.
#[test]
fn dts_bundle_refuses_a_construct_it_cannot_link_rather_than_guessing() {
    let dir = build_dir("d_unsupported");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(&dir, "src/dep.ts", "export const value: number = 1;\n");
    write_in(
        &dir,
        "src/index.ts",
        "import * as everything from './dep.js';\n\
         export const re: typeof everything = everything;\n",
    );

    let out = esdev_in(&dir)
        .args(["build", "--lib", "src", "--dts-bundle"])
        .output()
        .expect("spawn esdev");
    assert!(!out.status.success(), "{}", stdout(&out));
    let message = stderr(&out);
    assert!(message.contains("import * as everything"), "{message}");
    // Surface-neutral: the same refusal reaches a reader who wrote
    // `--dts-bundle` and one who wrote `"dts-bundle": true`.
    assert!(message.contains("declaration bundle"), "{message}");

    // …and the per-module build, which the message points at, still works.
    let per_module = esdev_in(&dir)
        .args(["build", "--lib", "src"])
        .output()
        .expect("spawn esdev");
    assert!(per_module.status.success(), "{}", stderr(&per_module));
    assert!(dir.join("dist/index.d.ts").exists());
    assert!(dir.join("dist/dep.d.ts").exists());
}

/// The two shapes are different enough that guessing between them would be
/// worse than saying so: a file to `--lib` would silently drop every module the
/// entry does not import.
#[test]
fn lib_rejects_the_argument_shapes_that_belong_to_the_other_mode() {
    let dir = lib_project("l_args");

    let file = esdev_in(&dir)
        .args(["build", "--lib", "src/index.ts"])
        .output()
        .expect("spawn esdev");
    assert!(!file.status.success());
    assert!(stderr(&file).contains("--lib src"), "{}", stderr(&file));

    let out_file = esdev_in(&dir)
        .args(["build", "--lib", "src", "--out=dist/index.js"])
        .output()
        .expect("spawn esdev");
    assert!(!out_file.status.success());
    assert!(
        stderr(&out_file).contains("directory"),
        "{}",
        stderr(&out_file)
    );

    let no_types_alone = esdev_in(&dir)
        .args(["build", "src/index.ts", "--no-types"])
        .output()
        .expect("spawn esdev");
    assert!(!no_types_alone.status.success());
    assert!(
        stderr(&no_types_alone).contains("--lib"),
        "{}",
        stderr(&no_types_alone)
    );
}

// ---------------------------------------------------------------------------
// `--watch` (DECISIONS D59)
//
// The unit tests in `watch.rs` cover the two filters. This covers the loop:
// that a change actually reruns the program, and — the part a filter test
// cannot see — that it reruns it *once* rather than restarting because it
// restarted.
// ---------------------------------------------------------------------------

/// A scratch directory for watch tests, deliberately **not** under
/// `CARGO_TARGET_TMPDIR`.
///
/// That lives inside `target/`, which the watcher ignores on purpose — machine
/// output is not a reason to restart. A watch test staged there watches nothing
/// and passes for the wrong reason.
fn watch_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("esdev-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create watch dir");
    dir
}

/// Polls `path` until it satisfies `done`, or gives up. Watch tests are about
/// timing, so they wait for a condition rather than for a duration.
fn wait_for_file(path: &Path, timeout: Duration, done: impl Fn(&str) -> bool) -> String {
    let deadline = std::time::Instant::now() + timeout;
    let mut last = String::new();
    while std::time::Instant::now() < deadline {
        last = std::fs::read_to_string(path).unwrap_or_default();
        if done(&last) {
            return last;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    last
}

#[test]
fn watch_reruns_the_program_when_a_file_changes() {
    let dir = watch_dir("w_rerun");
    // A `.txt` sink: not a watched extension, so the program's own output
    // cannot be what triggers the next run.
    let sink = dir.join("runs.txt");
    let app = dir.join("app.mjs");
    let program = |marker: &str| {
        format!(
            "import {{ write }} from 'runtime:fs';\n\
             await write({:?}, '{marker}\\n', {{ append: true }});\n",
            sink.to_string_lossy()
        )
    };
    std::fs::write(&app, program("FIRST")).expect("write app");

    let mut child = esdev_in(&dir)
        .args(["--watch", "app.mjs"])
        .spawn()
        .expect("spawn esdev --watch");

    let first = wait_for_file(&sink, Duration::from_secs(20), |s| s.contains("FIRST"));
    assert!(
        first.contains("FIRST"),
        "first run never happened: {first:?}"
    );

    std::fs::write(&app, program("SECOND")).expect("rewrite app");
    let both = wait_for_file(&sink, Duration::from_secs(20), |s| s.contains("SECOND"));

    let _ = child.kill();
    let _ = child.wait();

    let _ = std::fs::remove_dir_all(&dir);
    assert!(both.contains("FIRST"), "{both:?}");
    assert!(
        both.contains("SECOND"),
        "the change did not rerun it: {both:?}"
    );
}

/// The regression that matters: `inotify` reports reads, so the child *loading*
/// its entry raises an event on a watched file. A watcher that treats that as a
/// change restarts forever with nobody touching anything.
#[test]
fn watch_does_not_restart_because_it_restarted() {
    let dir = watch_dir("w_no_loop");
    let sink = dir.join("runs.txt");
    let app = dir.join("app.mjs");
    std::fs::write(
        &app,
        format!(
            "import {{ write }} from 'runtime:fs';\n\
             await write({:?}, 'x', {{ append: true }});\n",
            sink.to_string_lossy()
        ),
    )
    .expect("write app");

    let mut child = esdev_in(&dir)
        .args(["--watch", "app.mjs"])
        .spawn()
        .expect("spawn esdev --watch");

    // One run, then nothing — no edits are made after this point.
    wait_for_file(&sink, Duration::from_secs(20), |s| !s.is_empty());
    std::thread::sleep(Duration::from_secs(3));
    let settled = std::fs::read_to_string(&sink).unwrap_or_default();

    let _ = child.kill();
    let _ = child.wait();

    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(
        settled.len(),
        1,
        "ran {} times with no edit — the watcher is retriggering itself",
        settled.len()
    );
}

#[test]
fn watch_needs_a_file_to_watch() {
    let out = esdev()
        .args(["--watch", "-e=console.log(1)"])
        .output()
        .expect("spawn esdev");
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("needs a file to watch"),
        "{}",
        stderr(&out)
    );
}

// ---------------------------------------------------------------------------
// `esdev test` (DECISIONS D59)
// ---------------------------------------------------------------------------

#[test]
fn test_dom_declarations_know_which_properties_exist_and_cookies_round_trip() {
    let dir = build_dir("t_test_dom_properties_cookies");
    write_in(
        &dir,
        "properties.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('an unknown property reads undefined, a known one reads empty', () => {\n\
           const element = document.createElement('div');\n\
           document.body.append(element);\n\
           element.style.color = 'red';\n\
           const computed = getComputedStyle(element);\n\
           assertEquals([element.style.color, element.style.transform, element.style.nonsenseProp], ['red', '', undefined]);\n\
           // `transform`'s initial value is a keyword, so it is answered.\n\
           assertEquals([computed.transform, computed.nonsenseProp], ['none', undefined]);\n\
           assertEquals(['transform' in computed, 'nonsenseProp' in computed, 'gridTemplateAreas' in computed], [true, false, true]);\n\
           element.style.setProperty('--x', '1px');\n\
           assertEquals(getComputedStyle(element).getPropertyValue('--x'), '1px');\n\
         });\n\
         test('supports answers from the known properties', () => {\n\
           assertEquals([CSS.supports('color', 'red'), CSS.supports('display', 'grid')], [true, true]);\n\
           assertEquals([CSS.supports('nonsense-prop', '1'), CSS.supports('(nonsense-prop: 1)')], [false, false]);\n\
           assertEquals(CSS.supports('not (nonsense-prop: 1)'), true);\n\
           const style = document.createElement('style');\n\
           style.textContent = '@supports (display: grid) { .s { font-style: italic } } @supports (nonsense: 1) { .s { font-style: oblique } }';\n\
           document.head.append(style);\n\
           const element = document.createElement('p');\n\
           element.className = 's';\n\
           document.body.append(element);\n\
           assertEquals(getComputedStyle(element).fontStyle, 'italic');\n\
           style.remove();\n\
         });\n\
         test('cookies are a document-level string store', () => {\n\
           assertEquals([document.cookie, 'cookie' in document], ['', true]);\n\
           document.cookie = 'a=1';\n\
           document.cookie = 'b=2; Path=/; Secure';\n\
           assertEquals(document.cookie, 'a=1; b=2');\n\
           document.cookie = 'a=updated';\n\
           assertEquals(document.cookie, 'a=updated; b=2');\n\
           document.cookie = 'a=; Max-Age=0';\n\
           assertEquals(document.cookie, 'b=2');\n\
           document.cookie = 'c=3; Expires=Thu, 01 Jan 1970 00:00:00 GMT';\n\
           assertEquals(document.cookie, 'b=2');\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom properties and cookies");
    assert!(
        ran.status.success(),
        "DOM property and cookie test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_jsx_compiles_the_way_the_project_configured_it() {
    let dir = build_dir("t_jsx_classic");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(
        &dir,
        "esdev.json",
        r#"{ "jsx": { "factory": "h", "fragment": "Fragment" } }"#,
    );
    write_in(
        &dir,
        "src/runtime.js",
        "export function h(type, props, ...children) { return { type, props: props ?? {}, children: children.flat() }; }
         export const Fragment = 'fragment';
",
    );
    write_in(
        &dir,
        "src/view.test.jsx",
        "import { test, assertEquals } from 'runtime:test';
         import { h, Fragment } from './runtime.js';
         test('the project names the factory', () => {
           const el = <ul class=\'list\'><li>one</li><>two</></ul>;
           assertEquals([el.type, el.props.class], ['ul', 'list']);
           assertEquals([el.children[0].type, el.children[1].type], ['li', Fragment]);
         });
",
    );
    let ran = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test with classic jsx");
    assert!(
        ran.status.success(),
        "classic JSX did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_jsx_with_nothing_configured_is_refused() {
    // The alternative is a default, and a default is a framework chosen for the
    // project by its build tool. The message has to be the whole answer,
    // because there is nowhere else to look it up.
    let dir = build_dir("t_jsx_unconfigured");
    write_in(&dir, "esdev.json", r#"{ "test": { "jobs": 1 } }"#);
    write_in(
        &dir,
        "view.test.jsx",
        "import { test } from 'runtime:test';
         test('never runs', () => (<div/>));
",
    );
    let ran = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    assert!(
        !ran.status.success(),
        "unconfigured JSX ran:\n{}",
        stdout(&ran)
    );
    let said = format!("{}{}", stdout(&ran), stderr(&ran));
    assert!(said.contains("nothing has said how JSX compiles"), "{said}");
    assert!(said.contains("importSource"), "{said}");
    assert!(said.contains("factory"), "{said}");
    assert!(said.contains("@jsxImportSource"), "{said}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_a_files_jsx_pragma_overrides_the_project() {
    let dir = build_dir("t_jsx_pragma");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    // The project imports the function from a package; the file names one it
    // defines itself.
    write_in(
        &dir,
        "esdev.json",
        r#"{ "jsx": { "importSource": "nothing" } }"#,
    );
    write_in(
        &dir,
        "src/pragma.test.jsx",
        "/** @jsx h */
         import { test, assertEquals } from 'runtime:test';
         function h(type) { return { type, made: 'by the pragma' }; }
         test('the file decides', () => {
           assertEquals((<span/>).made, 'by the pragma');
         });
",
    );
    let ran = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test with a jsx pragma");
    assert!(
        ran.status.success(),
        "a pragma did not override the project:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_a_config_with_no_targets_tests_but_does_not_build() {
    let dir = build_dir("t_jsx_no_targets");
    write_in(&dir, "esdev.json", r#"{ "test": { "jobs": 1 } }"#);
    write_in(
        &dir,
        "one.test.mjs",
        "import { test } from 'runtime:test';
test('runs', () => {});
",
    );
    let tested = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test without targets");
    assert!(
        tested.status.success(),
        "a config with no targets could not test:\n{}{}",
        stdout(&tested),
        stderr(&tested)
    );

    let built = esdev_in(&dir)
        .arg("build")
        .output()
        .expect("spawn esdev build");
    assert!(!built.status.success());
    let text = format!("{}{}", stdout(&built), stderr(&built));
    assert!(text.contains("names no targets"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_cascades_stylesheets_into_computed_styles() {
    let dir = build_dir("t_test_dom_cascade");
    write_in(
        &dir,
        "cascade.test.mjs",
        "import { test, assertEquals, assertThrows } from 'runtime:test';\n\
         function sheet(text) {\n\
           const element = document.createElement('style');\n\
           element.textContent = text;\n\
           document.head.append(element);\n\
           return element;\n\
         }\n\
         // One realm per file, so each case starts from an empty document.\n\
         function reset() {\n\
           document.head.replaceChildren();\n\
           document.body.replaceChildren();\n\
           document.adoptedStyleSheets = [];\n\
         }\n\
         test('origin, importance, specificity and order decide the winner', () => {\n\
           reset();\n\
           sheet('div { color: one } .a { color: two } #b { color: three } .a { color: four } .imp { color: five !important }');\n\
           const element = document.createElement('div');\n\
           document.body.append(element);\n\
           assertEquals(getComputedStyle(element).color, 'one');\n\
           element.className = 'a';\n\
           assertEquals(getComputedStyle(element).color, 'four');\n\
           element.id = 'b';\n\
           assertEquals(getComputedStyle(element).color, 'three');\n\
           element.style.color = 'inline';\n\
           assertEquals(getComputedStyle(element).color, 'inline');\n\
           element.classList.add('imp');\n\
           assertEquals(getComputedStyle(element).color, 'five');\n\
           element.style.setProperty('color', 'inline', 'important');\n\
           assertEquals(getComputedStyle(element).color, 'inline');\n\
           assertThrows(() => { getComputedStyle(element).color = 'x'; }, TypeError);\n\
         });\n\
         test('inheritance and the user-agent defaults fill the rest in', () => {\n\
           reset();\n\
           sheet('.parent { color: purple; border-color: red; --brand: cyan }');\n\
           const parent = document.createElement('div');\n\
           parent.className = 'parent';\n\
           const child = document.createElement('span');\n\
           parent.append(child);\n\
           document.body.append(parent);\n\
           const computed = getComputedStyle(child);\n\
           assertEquals([computed.color, computed.getPropertyValue('border-color'), computed.getPropertyValue('--brand')], ['rgb(128, 0, 128)', '', 'cyan']);\n\
           assertEquals([computed.display, computed.fontWeight, computed.visibility, computed.textAlign], ['inline', '400', 'visible', 'start']);\n\
           assertEquals(getComputedStyle(parent).display, 'block');\n\
           const strong = document.createElement('strong');\n\
           document.body.append(strong);\n\
           assertEquals(getComputedStyle(strong).fontWeight, '700');\n\
           const hidden = document.createElement('div');\n\
           hidden.setAttribute('hidden', '');\n\
           document.body.append(hidden);\n\
           assertEquals(getComputedStyle(hidden).display, 'none');\n\
         });\n\
         test('a media query is answered from the declared viewport', () => {\n\
           reset();\n\
           sheet('@media (min-width: 100px) { .m { color: wide } } @media (min-width: 99999px) { .m { color: wider } } @media print { .m { color: printed } }');\n\
           const element = document.createElement('p');\n\
           element.className = 'm';\n\
           document.body.append(element);\n\
           assertEquals(getComputedStyle(element).color, 'wide');\n\
           window.innerWidth = 50;\n\
           assertEquals(getComputedStyle(element).color, 'rgb(0, 0, 0)');\n\
           window.innerWidth = 1024;\n\
           assertEquals(getComputedStyle(element).color, 'wide');\n\
         });\n\
         test('a constructed sheet applies while it is adopted', () => {\n\
           reset();\n\
           const constructed = new CSSStyleSheet();\n\
           constructed.replaceSync('.c { color: adopted }');\n\
           const element = document.createElement('div');\n\
           element.className = 'c';\n\
           document.body.append(element);\n\
           assertEquals([constructed.cssRules.length, constructed.cssRules[0].selectorText], [1, '.c']);\n\
           assertEquals(getComputedStyle(element).color, 'rgb(0, 0, 0)');\n\
           document.adoptedStyleSheets = [constructed];\n\
           assertEquals(getComputedStyle(element).color, 'adopted');\n\
           document.adoptedStyleSheets = [];\n\
           assertEquals(getComputedStyle(element).color, 'rgb(0, 0, 0)');\n\
         });\n\
         test('a shadow root is styled by its own sheets only', () => {\n\
           reset();\n\
           const host = document.createElement('div');\n\
           document.body.append(host);\n\
           const root = host.attachShadow({ mode: 'open' });\n\
           root.innerHTML = '<style>.in { color: maroon }</style><p class=in>x</p>';\n\
           const inside = root.lastElementChild;\n\
           const outside = document.createElement('p');\n\
           outside.className = 'in';\n\
           document.body.append(outside);\n\
           assertEquals([getComputedStyle(inside).color, getComputedStyle(outside).color], ['rgb(128, 0, 0)', 'rgb(0, 0, 0)']);\n\
           const scoped = new CSSStyleSheet();\n\
           scoped.replaceSync('.in { font-weight: 700 }');\n\
           root.adoptedStyleSheets = [scoped];\n\
           assertEquals(getComputedStyle(inside).fontWeight, '700');\n\
         });\n\
         test('nesting resolves through :is() and reports its own rules', () => {\n\
           reset();\n\
           const element = sheet('.card { color: olive; & a { color: coral } b { color: lime } }');\n\
           const card = document.createElement('div');\n\
           card.className = 'card';\n\
           card.innerHTML = '<a>l</a><b>bold</b>';\n\
           document.body.append(card);\n\
           assertEquals(getComputedStyle(card).color, 'rgb(128, 128, 0)');\n\
           assertEquals(getComputedStyle(card.firstElementChild).color, 'rgb(255, 127, 80)');\n\
           assertEquals(getComputedStyle(card.lastElementChild).color, 'rgb(0, 255, 0)');\n\
           assertEquals(Array.from(element.sheet.cssRules, rule => rule.selectorText), ['.card', ':is(.card) a', ':is(.card) b']);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom cascade");
    assert!(
        ran.status.success(),
        "DOM cascade test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_element_internals_associate_custom_elements_with_forms() {
    let dir = build_dir("t_test_dom_element_internals");
    write_in(
        &dir,
        "internals.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         class Field extends HTMLElement {\n\
           static formAssociated = true;\n\
           constructor() { super(); this.internals = this.attachInternals(); }\n\
         }\n\
         customElements.define('x-field', Field);\n\
         customElements.define('x-plain', class extends HTMLElement {});\n\
         function form() {\n\
           const element = document.createElement('form');\n\
           const field = document.createElement('x-field');\n\
           field.setAttribute('name', 'chosen');\n\
           element.append(field);\n\
           document.body.append(element);\n\
           return [element, field];\n\
         }\n\
         test('a form-associated custom element joins its form and its data', () => {\n\
           const [owner, field] = form();\n\
           assertEquals([field.internals.form === owner, owner.elements.length], [true, 1]);\n\
           field.internals.setFormValue('picked');\n\
           assertEquals(Array.from(new FormData(owner).entries()), [['chosen', 'picked']]);\n\
           field.internals.setFormValue(null);\n\
           assertEquals(Array.from(new FormData(owner).entries()), []);\n\
         });\n\
         test('the element decides its own validity', () => {\n\
           const [owner, field] = form();\n\
           let invalid = 0;\n\
           field.addEventListener('invalid', () => invalid++);\n\
           assertEquals([field.internals.validity.valid, owner.checkValidity()], [true, true]);\n\
           field.internals.setValidity({ valueMissing: true }, 'pick something');\n\
           assertEquals([field.internals.validity.valid, field.internals.validity.valueMissing, field.internals.validationMessage], [false, true, 'pick something']);\n\
           assertEquals([field.internals.checkValidity(), invalid, owner.checkValidity()], [false, 1, false]);\n\
           field.internals.setValidity({});\n\
           assertEquals([field.internals.validity.valid, owner.checkValidity()], [true, true]);\n\
           assertEquals(field.internals.validity === field.internals.validity, true);\n\
         });\n\
         test('custom states drive the :state() selector', () => {\n\
           const [owner, field] = form();\n\
           field.internals.states.add('loading');\n\
           assertEquals([field.internals.states.size, owner.querySelectorAll('x-field:state(loading)').length], [1, 1]);\n\
           field.internals.states.delete('loading');\n\
           assertEquals([owner.querySelectorAll('x-field:state(loading)').length, owner.querySelectorAll('x-field:not(:state(loading))').length], [0, 1]);\n\
         });\n\
         test('internals reach a closed root and refuse the wrong element', () => {\n\
           const host = document.createElement('x-field');\n\
           const shadow = host.attachShadow({ mode: 'closed' });\n\
           assertEquals([host.shadowRoot, host.internals.shadowRoot === shadow], [null, true]);\n\
           const name = callback => { try { callback(); return 'ok'; } catch (error) { return error.name; } };\n\
           assertEquals(name(() => document.createElement('x-plain').attachInternals().form), 'NotSupportedError');\n\
           assertEquals(name(() => document.createElement('div').attachInternals()), 'NotSupportedError');\n\
           assertEquals(name(() => host.attachInternals()), 'NotSupportedError');\n\
           assertEquals(name(() => host.internals.setValidity({ valueMissing: true })), 'TypeError');\n\
         });\n\
         test('a form reset reaches the element', () => {\n\
           const [owner, field] = form();\n\
           let resets = 0;\n\
           field.formResetCallback = () => resets++;\n\
           owner.reset();\n\
           assertEquals(resets, 1);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom element internals");
    assert!(
        ran.status.success(),
        "DOM element-internals test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_selectors_count_an_of_list_and_follow_custom_definitions() {
    let dir = build_dir("t_test_dom_selector_of_and_defined");
    write_in(
        &dir,
        "selectors.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('nth-child counts only the siblings in its of list', () => {\n\
           document.body.innerHTML = \"<ul><li class='x'>1</li><li>2</li><li class='x'>3</li><li class='x'>4</li><li>5</li><li class='x'>6</li></ul>\";\n\
           const list = document.body.firstElementChild;\n\
           const texts = selector => Array.from(list.querySelectorAll(selector), item => item.textContent);\n\
           assertEquals(texts('li:nth-child(2n + 1 of .x)'), ['1', '4']);\n\
           assertEquals(texts('li:nth-child(odd of .x)'), ['1', '4']);\n\
           assertEquals(texts('li:nth-last-child(1 of .x)'), ['6']);\n\
           assertEquals(texts('li:nth-child(2n + 1)'), ['1', '3', '5']);\n\
           assertEquals(texts('li:nth-child(1 of .x, :not(.x))'), ['1']);\n\
         });\n\
         test('an of list is refused where it does not belong', () => {\n\
           const name = callback => { try { callback(); return 'ok'; } catch (error) { return error.name; } };\n\
           assertEquals(name(() => document.querySelectorAll('li:nth-child(of .x)')), 'SyntaxError');\n\
           assertEquals(name(() => document.querySelectorAll('li:nth-of-type(2n of .x)')), 'SyntaxError');\n\
           assertEquals(name(() => document.querySelectorAll(':defined(x)')), 'SyntaxError');\n\
         });\n\
         test(':defined answers before and after the definition', () => {\n\
           const host = document.createElement('div');\n\
           document.body.append(host);\n\
           host.innerHTML = '<my-widget></my-widget><p></p><other-thing></other-thing>';\n\
           const names = selector => Array.from(host.querySelectorAll(selector), element => element.localName);\n\
           assertEquals([names(':defined'), host.firstElementChild.matches(':defined')], [['p'], false]);\n\
           customElements.define('my-widget', class extends HTMLElement {});\n\
           assertEquals([names(':defined'), host.firstElementChild.matches(':defined')], [['my-widget', 'p'], true]);\n\
           assertEquals(names(':not(:defined)'), ['other-thing']);\n\
           assertEquals([document.createElement('my-widget').matches(':defined'), document.createElement('late-one').matches(':defined')], [true, false]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom of-list and defined selectors");
    assert!(
        ran.status.success(),
        "DOM selector test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_parser_builds_whole_documents_from_html_only() {
    let dir = build_dir("t_test_dom_domparser");
    write_in(
        &dir,
        "domparser.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('a bare document gets a synthesized html, head and body', () => {\n\
           const parsed = new DOMParser().parseFromString('<p>one</p><style>a{}</style>', 'text/html');\n\
           assertEquals(parsed.doctype, null);\n\
           assertEquals(parsed.documentElement.tagName, 'HTML');\n\
           assertEquals([parsed.head.innerHTML, parsed.body.innerHTML], ['<style>a{}</style>', '<p>one</p>']);\n\
           assertEquals(parsed.firstElementChild === parsed.documentElement, true);\n\
         });\n\
         test('a document that supplies its own structure keeps it', () => {\n\
           const source = '<!doctype html><html><head><title>T</title></head><body><main id=\"x\">y</main></body></html>';\n\
           const parsed = new DOMParser().parseFromString(source, 'text/html');\n\
           assertEquals([parsed.doctype.name, parsed.title, parsed.getElementById('x').textContent], ['html', 'T', 'y']);\n\
           assertEquals([parsed !== document, parsed.body.ownerDocument === parsed], [true, true]);\n\
         });\n\
         test('XML is refused by name and malformed HTML by the parser', () => {\n\
           const parser = new DOMParser();\n\
           const name = callback => { try { callback(); return 'parsed'; } catch (error) { return error.name; } };\n\
           assertEquals(name(() => parser.parseFromString('<p/>', 'text/xml')), 'NotSupportedError');\n\
           assertEquals(name(() => parser.parseFromString('<p></p>', 'text/plain')), 'TypeError');\n\
           assertEquals(name(() => parser.parseFromString('<p><i>unclosed', 'text/html')), 'SyntaxError');\n\
           assertEquals(name(() => parser.parseFromString('<p>x</p><!doctype html>', 'text/html')), 'SyntaxError');\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom DOMParser");
    assert!(
        ran.status.success(),
        "DOM DOMParser test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_inserts_adjacent_markup_nodes_and_text() {
    let dir = build_dir("t_test_dom_insert_adjacent");
    write_in(
        &dir,
        "adjacent.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('the four insertion positions land where they say', () => {\n\
           const root = document.createElement('main');\n\
           const target = document.createElement('p');\n\
           root.append(target);\n\
           document.body.append(root);\n\
           target.insertAdjacentHTML('beforebegin', '<i>bb</i>');\n\
           target.insertAdjacentHTML('afterbegin', '<b>ab</b>');\n\
           target.insertAdjacentHTML('beforeend', '<u>be</u>');\n\
           target.insertAdjacentHTML('afterend', '<s>ae</s>');\n\
           assertEquals(root.innerHTML, '<i>bb</i><p><b>ab</b><u>be</u></p><s>ae</s>');\n\
           const returned = target.insertAdjacentElement('AfterBegin', document.createElement('em'));\n\
           target.insertAdjacentText('beforeend', 'text&');\n\
           assertEquals([returned.localName, target.innerHTML], ['em', '<em></em><b>ab</b><u>be</u>text&amp;']);\n\
         });\n\
         test('a position with no parent or no name is refused', () => {\n\
           const orphan = document.createElement('div');\n\
           const errors = [];\n\
           const name = callback => { try { callback(); return 'inserted'; } catch (error) { return error.name; } };\n\
           errors.push(name(() => orphan.insertAdjacentHTML('beforebegin', 'x')));\n\
           errors.push(name(() => document.documentElement.insertAdjacentHTML('afterend', 'x')));\n\
           errors.push(name(() => orphan.insertAdjacentHTML('nowhere', 'x')));\n\
           errors.push(name(() => orphan.insertAdjacentElement('beforeend', 'nope')));\n\
           assertEquals(errors, ['NoModificationAllowedError', 'NoModificationAllowedError', 'SyntaxError', 'TypeError']);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom insertAdjacent");
    assert!(
        ran.status.success(),
        "DOM adjacent-insertion test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_declarative_shadow_roots_attach_only_through_set_html_unsafe() {
    let dir = build_dir("t_test_dom_declarative_shadow");
    write_in(
        &dir,
        "declarative.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('innerHTML keeps the template, setHTMLUnsafe attaches it', () => {\n\
           const kept = document.createElement('div');\n\
           kept.innerHTML = '<span><template shadowrootmode=\"open\"><i>inner</i></template></span>';\n\
           assertEquals([kept.firstElementChild.shadowRoot, kept.firstElementChild.firstElementChild.localName], [null, 'template']);\n\
           const attached = document.createElement('div');\n\
           attached.setHTMLUnsafe('<span><template shadowrootmode=\"open\" shadowrootserializable=\"\"><i>inner</i></template>light</span>');\n\
           const host = attached.firstElementChild;\n\
           assertEquals([host.shadowRoot.mode, host.shadowRoot.serializable, host.shadowRoot.innerHTML, host.textContent], ['open', true, '<i>inner</i>', 'light']);\n\
         });\n\
         test('getHTML serializes a shadow root only when asked', () => {\n\
           const root = document.createElement('div');\n\
           root.setHTMLUnsafe('<span><template shadowrootmode=\"open\" shadowrootserializable=\"\"><i>inner</i></template>light</span>');\n\
           const shadow = root.firstElementChild.shadowRoot;\n\
           const serialized = '<span><template shadowrootmode=\"open\" shadowrootserializable=\"\"><i>inner</i></template>light</span>';\n\
           assertEquals(root.getHTML(), '<span>light</span>');\n\
           assertEquals(root.innerHTML, '<span>light</span>');\n\
           assertEquals(root.getHTML({ serializableShadowRoots: true }), serialized);\n\
           assertEquals(root.getHTML({ shadowRoots: [shadow] }), serialized);\n\
           const closed = document.createElement('div');\n\
           closed.setHTMLUnsafe('<p><template shadowrootmode=\"closed\"><b>x</b></template></p>');\n\
           assertEquals([closed.firstElementChild.shadowRoot, closed.getHTML({ serializableShadowRoots: true })], [null, '<p></p>']);\n\
         });\n\
         test('nested declarative roots attach, a repeated one is dropped', () => {\n\
           const nested = document.createElement('div');\n\
           // Only an element that may host a root is used: `<a>` and `<b>` may\n\
           // not, here or in a browser.\n\
           nested.setHTMLUnsafe('<section><template shadowrootmode=\"open\"><div><template shadowrootmode=\"open\"><u>deep</u></template></div></template></section>');\n\
           const inner = nested.firstElementChild.shadowRoot.firstElementChild;\n\
           assertEquals([inner.localName, inner.shadowRoot.innerHTML], ['div', '<u>deep</u>']);\n\
           const twice = document.createElement('div');\n\
           twice.setHTMLUnsafe('<span><template shadowrootmode=\"open\"><i>1</i></template><template shadowrootmode=\"open\"><i>2</i></template></span>');\n\
           assertEquals([twice.firstElementChild.shadowRoot.innerHTML, twice.firstElementChild.childNodes.length], ['<i>1</i>', 0]);\n\
         });\n\
         test('a manual-assignment root holds only what it was assigned', () => {\n\
           const host = document.createElement('div');\n\
           const first = document.createElement('i');\n\
           const second = document.createElement('b');\n\
           host.append(first, second);\n\
           document.body.append(host);\n\
           const root = host.attachShadow({ mode: 'open', slotAssignment: 'manual' });\n\
           root.innerHTML = '<slot></slot>';\n\
           const slot = root.firstElementChild;\n\
           assertEquals([root.slotAssignment, slot.assignedNodes().length], ['manual', 0]);\n\
           slot.assign(second, document.createElement('u'));\n\
           assertEquals(slot.assignedNodes().map(node => node.localName), ['b']);\n\
           const named = document.createElement('div');\n\
           named.append(document.createElement('i'));\n\
           const namedRoot = named.attachShadow({ mode: 'open' });\n\
           namedRoot.innerHTML = '<slot></slot>';\n\
           assertEquals([namedRoot.slotAssignment, namedRoot.firstElementChild.assignedNodes().length, namedRoot.clonable], ['named', 1, false]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom declarative shadow roots");
    assert!(
        ran.status.success(),
        "DOM declarative shadow-root test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_events_propagate_through_the_window() {
    let dir = build_dir("t_test_dom_window_propagation");
    write_in(
        &dir,
        "window-path.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('the window is the last target in the path', () => {\n\
           const target = document.createElement('i');\n\
           document.body.append(target);\n\
           const order = [];\n\
           window.addEventListener('z', () => order.push('window-capture'), true);\n\
           document.addEventListener('z', () => order.push('document-capture'), true);\n\
           target.addEventListener('z', event => order.push(event.currentTarget === target ? 'target' : 'wrong'));\n\
           window.addEventListener('z', event => order.push(event.currentTarget === window && event.target === target ? 'window-bubble' : 'wrong'));\n\
           const event = new Event('z', { bubbles: true });\n\
           target.dispatchEvent(event);\n\
           assertEquals(order, ['window-capture', 'document-capture', 'target', 'window-bubble']);\n\
         });\n\
         test('composedPath crosses the shadow boundary and ends at the window', () => {\n\
           const host = document.createElement('div');\n\
           const root = host.attachShadow({ mode: 'open' });\n\
           const inner = document.createElement('span');\n\
           root.append(inner);\n\
           document.body.append(host);\n\
           let path = [];\n\
           host.addEventListener('x', event => { path = event.composedPath(); });\n\
           inner.dispatchEvent(new Event('x', { bubbles: true, composed: true }));\n\
           assertEquals([path.length, path.at(-1) === window, path.at(-2) === document], [7, true, true]);\n\
         });\n\
         test('a load event at the document stops there', () => {\n\
           const seen = [];\n\
           window.addEventListener('load', () => seen.push('window'));\n\
           document.addEventListener('load', () => seen.push('document'));\n\
           document.dispatchEvent(new Event('load', { bubbles: true }));\n\
           assertEquals(seen, ['document']);\n\
         });\n\
         test('an unqualified listener call reaches the window', () => {\n\
           let hits = 0;\n\
           globalThis.__probe = () => hits++;\n\
           (0, eval)(\"addEventListener('bare', globalThis.__probe)\");\n\
           window.dispatchEvent(new Event('bare'));\n\
           assertEquals(hits, 1);\n\
         });\n\
         test('the window dispatches its own events', () => {\n\
           let seen = null;\n\
           window.addEventListener('own', event => { seen = [event.currentTarget === window, event.target === window, event.composedPath().length]; });\n\
           assertEquals(window.dispatchEvent(new Event('own')), true);\n\
           assertEquals(seen, [true, true, 1]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom window propagation");
    assert!(
        ran.status.success(),
        "DOM window propagation test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_abort_signals_dispatch_in_their_own_event_realm() {
    let dir = build_dir("t_test_dom_abort_signal_events");
    write_in(
        &dir,
        "abort.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('abort signals notify DOM event targets', () => {\n\
           const controller = new AbortController();\n\
           const target = document.createElement('button');\n\
           let abort = null; let calls = 0;\n\
           controller.signal.addEventListener('abort', event => { abort = [event.type, event.isTrusted]; });\n\
           target.addEventListener('go', () => calls++, { signal: controller.signal });\n\
           controller.abort('finished');\n\
           target.dispatchEvent(new Event('go'));\n\
           assertEquals([controller.signal.aborted, controller.signal.reason, abort, calls], [true, 'finished', ['abort', true], 0]);\n\
           const root = new AbortController();\n\
           const signals = [root.signal, AbortSignal.any([root.signal]), AbortSignal.any([root.signal])];\n\
           signals.push(AbortSignal.any([signals[0]]), AbortSignal.any([signals[1]]));\n\
           const order = []; signals.forEach((signal, index) => signal.addEventListener('abort', () => order.push(index)));\n\
           root.abort();\n\
           assertEquals(order, [0, 1, 2, 3, 4]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom abort signal events");
    assert!(
        ran.status.success(),
        "DOM abort-signal test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_event_targets_honor_listener_options_and_event_state() {
    let dir = build_dir("t_test_dom_event_target_options");
    write_in(
        &dir,
        "events.test.mjs",
        "import { test, assertEquals, assertThrows } from 'runtime:test';\n\
         test('event target listener options and state', () => {\n\
           const target = new EventTarget();\n\
           const passive = new Event('passive', { cancelable: true });\n\
           target.addEventListener('passive', event => { event.returnValue = false; }, { passive: true });\n\
           assertEquals(target.dispatchEvent(passive), true); assertEquals(passive.defaultPrevented, false);\n\
           assertThrows(() => target.addEventListener('x', null, { signal: null }), TypeError);\n\
           let duringDispatch; target.addEventListener('state', { get handleEvent() { duringDispatch = window.event; return () => {}; } });\n\
           const state = new Event('state'); target.dispatchEvent(state);\n\
           assertEquals([duringDispatch, state.srcElement, state.composedPath().length, typeof Object.getOwnPropertyDescriptor(state, 'isTrusted').get], [state, target, 0, 'function']);\n\
           assertEquals(state.timeStamp > 0, true);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom event target options");
    assert!(
        ran.status.success(),
        "DOM event-target-options test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_installs_a_fresh_document_and_uses_the_strict_parser() {
    let dir = build_dir("t_test_dom");
    write_in(
        &dir,
        "dom.test.mjs",
        "import { test, assertEquals, assertThrows } from 'runtime:test';\n\
         test('tree and parser', () => {\n\
           assertEquals(document.body.localName, 'body');\n\
           document.body.innerHTML = '<p id=x>hello<!-- note --></p>';\n\
           assertEquals(document.body.firstChild.textContent, 'hello');\n\
           assertEquals(document.body.innerHTML, '<p id=\"x\">hello<!-- note --></p>');\n\
           document.body.innerHTML = '<p>';\n\
           assertEquals(document.body.innerHTML, '<p></p>');\n\
           assertThrows(() => { document.body.innerHTML = '<div>'; }, SyntaxError);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom");
    assert!(
        ran.status.success(),
        "DOM test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_serializes_text_attributes_raw_text_voids_and_replacements() {
    let dir = build_dir("t_test_dom_serialize");
    write_in(
        &dir,
        "serialize.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('serialization contexts', () => {\n\
           const text = document.createTextNode('&<>');\n\
           const image = document.createElement('img');\n\
           image.setAttribute('title', 'a&\\\"b\\u00a0');\n\
           const script = document.createElement('script');\n\
           script.textContent = 'a < b && c > d';\n\
           document.body.append(text, image, script);\n\
           assertEquals(document.body.innerHTML, '&amp;&lt;&gt;<img title=\"a&amp;&quot;b&nbsp;\"><script>a < b && c > d</script>');\n\
         });\n\
         test('outerHTML replacement', () => {\n\
           document.body.innerHTML = '<p>old</p><br>';\n\
           document.body.firstChild.outerHTML = '<em>new</em><!--tail-->';
           assertEquals(document.body.innerHTML, '<em>new</em><!--tail--><br>');\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom serializer cases");
    assert!(
        ran.status.success(),
        "DOM serializer cases did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_selectors_match_strict_compounds_and_combinators() {
    let dir = build_dir("t_test_dom_selectors");
    write_in(
        &dir,
        "selectors.test.mjs",
        "import { test, assertEquals, assertThrows } from 'runtime:test';\n\
         test('selector core', () => {\n\
           document.body.innerHTML = '<section id=panel><article class=\"card selected\" data-tags=\"new hot\"><span class=label data-state=\"Ready-item\">one</span></article><article class=card data-tags=old>two</article></section>';\n\
           const panel = document.querySelector('#panel');\n\
           const label = panel.querySelector('article.card.selected > span.label[data-state|=ready i]');\n\
           assertEquals(label.textContent, 'one');\n\
           assertEquals(document.querySelectorAll('#panel > article').length, 2);\n\
           assertEquals(document.querySelector('#panel > article + article').textContent, 'two');\n\
           assertEquals(document.querySelectorAll('#panel > article ~ article').length, 1);\n\
           assertEquals(document.querySelector('[data-tags~=hot]').textContent, 'one');\n\
           assertEquals(document.querySelector('[data-state^=Ready][data-state$=item][data-state*=dy]').textContent, 'one');\n\
           assertEquals(document.querySelector('.missing, .label'), label);\n\
           assertEquals(label.matches('span.label[data-state=Ready-item]'), true);\n\
           assertEquals(label.closest('section#panel'), panel);\n\
           assertThrows(() => document.querySelector(':nonsense'), SyntaxError);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom selectors");
    assert!(
        ran.status.success(),
        "DOM selector test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_selectors_match_logical_pseudo_classes_and_nested_lists() {
    let dir = build_dir("t_test_dom_selector_logical");
    write_in(
        &dir,
        "logical.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('is where and not compose selector lists', () => {\n\
           document.body.innerHTML = '<main><button class=\"primary selected\">save</button><a class=link href=\"/docs\">docs</a><p class=muted>note</p></main>';
           const [button, link, note] = document.querySelectorAll('main > *');\n\
           assertEquals(document.querySelectorAll('main > :is(button, a.link)').length, 2);\n\
           assertEquals(document.querySelectorAll('main > :where(button.primary, a[href])').length, 2);\n\
           assertEquals(Array.from(document.querySelectorAll('main > :not(:is(.muted, a))')), [button]);\n\
           assertEquals(button.matches(':not(.muted):is(.primary, .secondary)'), true);\n\
           assertEquals(link.matches(':where(.link, .missing)'), true); assertEquals(note.matches(':not(button, a)'), true);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM logical selector test");
    assert!(
        ran.status.success(),
        "logical selector test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_selectors_match_has_relative_descendant_and_sibling_forms() {
    let dir = build_dir("t_test_dom_selector_has");
    write_in(
        &dir,
        "has.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('has evaluates relative selectors from each candidate', () => {\n\
           document.body.innerHTML = '<section id=one><span class=direct></span><div><i class=deep></i></div></section><section id=two></section><aside class=notice></aside><article id=three></article>';
           const one = document.querySelector('#one'); const two = document.querySelector('#two'); const three = document.querySelector('#three');\n\
           assertEquals(Array.from(document.querySelectorAll('section:has(> .direct)')), [one]);\n\
           assertEquals(Array.from(document.querySelectorAll('section:has(.deep)')), [one]);\n\
           assertEquals(Array.from(document.querySelectorAll('section:has(> .missing, .deep)')), [one]);\n\
           assertEquals(Array.from(document.querySelectorAll('section:has(+ aside.notice)')), [two]);\n\
           assertEquals(Array.from(document.querySelectorAll('section:has(~ article)')), [one, two]);\n\
           two.appendChild(document.createElement('i')).className = 'deep';\n\
           assertEquals(Array.from(document.querySelectorAll('section:has(.deep)')), [one, two]);\n\
           assertEquals(three.matches(':has(+ .missing)'), false);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM has selector test");
    assert!(
        ran.status.success(),
        "has selector test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_selectors_match_root_empty_and_child_positions() {
    let dir = build_dir("t_test_dom_selector_structure");
    write_in(
        &dir,
        "structure.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('root empty and child positions count elements', () => {\n\
           document.body.innerHTML = '<section id=parent><i id=one></i>text<b id=two></b><em id=three></em></section><aside id=empty></aside>';\n\
           const parent = document.querySelector('#parent'); const [one, two, three] = parent.children; const empty = document.querySelector('#empty');\n\
           assertEquals(document.documentElement.matches(':root'), true); assertEquals(parent.matches(':root'), false);\n\
           assertEquals(empty.matches(':empty'), true); assertEquals(parent.matches(':empty'), false);\n\
           assertEquals(Array.from(parent.querySelectorAll(':first-child')), [one]);\n\
           assertEquals(Array.from(parent.querySelectorAll(':last-child')), [three]);\n\
           assertEquals(two.matches(':only-child'), false);\n\
           parent.innerHTML = '<strong id=single></strong>'; assertEquals(parent.firstChild.matches(':only-child'), true);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM structural selector test");
    assert!(
        ran.status.success(),
        "structural selector test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_selectors_match_type_positions_and_nth_formulas() {
    let dir = build_dir("t_test_dom_selector_nth");
    write_in(
        &dir,
        "nth.test.mjs",
        "import { test, assertEquals, assertThrows } from 'runtime:test';\n\
         test('type positions and An+B formulas', () => {\n\
           document.body.innerHTML = '<ul><li id=one></li><li id=two></li><div></div><li id=three></li><li id=four></li><li id=five></li></ul>';\n\
           const list = document.querySelector('ul'); const one = document.querySelector('#one'); const three = document.querySelector('#three'); const five = document.querySelector('#five');\n\
           assertEquals(Array.from(list.querySelectorAll('li:first-of-type')), [one]);\n\
           assertEquals(Array.from(list.querySelectorAll('li:last-of-type')), [five]);\n\
           assertEquals(Array.from(list.querySelectorAll('li:nth-child(2n + 1)')), [one, document.querySelector('#four')]);\n\
           assertEquals(Array.from(list.querySelectorAll('li:nth-of-type(2n + 1)')), [one, three, five]);\n\
           assertEquals(Array.from(list.querySelectorAll('li:nth-of-type(even)')), [document.querySelector('#two'), document.querySelector('#four')]);\n\
           assertEquals(Array.from(list.querySelectorAll('li:nth-last-of-type(2)')), [document.querySelector('#four')]);\n\
           assertThrows(() => list.querySelector(':nth-child(two)'), SyntaxError);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM nth selector test");
    assert!(
        ran.status.success(),
        "nth selector test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_focus_selector_follows_focus_transitions() {
    let dir = build_dir("t_test_dom_selector_focus");
    write_in(
        &dir,
        "focus.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('focus selection follows the active element', () => {\n\
           const first = document.createElement('input'); const second = document.createElement('button'); document.body.append(first, second);\n\
           assertEquals(document.querySelector(':focus'), document.body);\n\
           first.focus(); assertEquals(document.querySelector(':focus'), first); assertEquals(second.matches(':focus'), false);\n\
           second.focus(); assertEquals(document.querySelector(':focus'), second);\n\
           second.blur(); assertEquals(document.querySelector(':focus'), document.body);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM focus selector test");
    assert!(
        ran.status.success(),
        "focus selector test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_selectors_match_dynamic_form_state() {
    let dir = build_dir("t_test_dom_selector_form_state");
    write_in(
        &dir,
        "form-state.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('form state selectors follow reflected attributes and defaults', () => {\n\
           const checked = document.createElement('input'); checked.type = 'checkbox'; checked.checked = true;\n\
           const disabled = document.createElement('button'); disabled.disabled = true;\n\
           const required = document.createElement('input'); required.required = true;\n\
           const optional = document.createElement('input'); const plain = document.createElement('div');\n\
           document.body.append(checked, disabled, required, optional, plain);\n\
           assertEquals(Array.from(document.querySelectorAll(':checked')), [checked]);\n\
           assertEquals(Array.from(document.querySelectorAll(':disabled')), [disabled]);\n\
           assertEquals(Array.from(document.querySelectorAll(':enabled')), [checked, required, optional]);\n\
           assertEquals(Array.from(document.querySelectorAll(':required')), [required]);\n\
           assertEquals(Array.from(document.querySelectorAll(':optional')), [checked, optional]);\n\
           checked.checked = false; disabled.disabled = false; required.required = false;\n\
           assertEquals(document.querySelectorAll(':checked').length, 0);\n\
           assertEquals(document.querySelectorAll(':disabled').length, 0);\n\
           assertEquals(Array.from(document.querySelectorAll(':optional')), [checked, required, optional]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM form-state selector test");
    assert!(
        ran.status.success(),
        "form-state selector test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_selectors_match_links_without_treating_plain_anchors_as_links() {
    let dir = build_dir("t_test_dom_selector_links");
    write_in(
        &dir,
        "links.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('link state is driven by href', () => {\n\
           const anchor = document.createElement('a'); const area = document.createElement('area'); const plain = document.createElement('a');\n\
           anchor.setAttribute('href', '/docs'); area.setAttribute('href', '/map'); document.body.append(anchor, area, plain);\n\
           assertEquals(Array.from(document.querySelectorAll(':link')), [anchor, area]);\n\
           anchor.removeAttribute('href'); assertEquals(Array.from(document.querySelectorAll(':link')), [area]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM link selector test");
    assert!(
        ran.status.success(),
        "link selector test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_scope_selector_respects_the_query_root() {
    let dir = build_dir("t_test_dom_selector_scope");
    write_in(
        &dir,
        "scope.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('scope includes the element root only when requested', () => {\n\
           const root = document.createElement('div'); const child = document.createElement('div'); const grandchild = document.createElement('span');\n\
           child.className = 'child'; grandchild.className = 'grandchild'; child.appendChild(grandchild); root.appendChild(child); document.body.appendChild(root);\n\
           assertEquals(Array.from(root.querySelectorAll(':scope')), [root]);\n\
           assertEquals(Array.from(root.querySelectorAll(':scope > .child')), [child]);\n\
           assertEquals(Array.from(root.querySelectorAll(':scope .grandchild')), [grandchild]);\n\
           assertEquals(Array.from(root.querySelectorAll('div')), [child]);\n\
           assertEquals(root.matches(':scope'), true);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM scope selector test");
    assert!(
        ran.status.success(),
        "scope selector test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_selector_node_lists_support_for_each() {
    let dir = build_dir("t_test_dom_selector_node_list_for_each");
    write_in(
        &dir,
        "selector-list.test.mjs",
        "import { test, assertEquals, assertThrows } from 'runtime:test';\n\
         test('selector node lists expose indexed forEach iteration', () => {\n\
           const root = document.createElement('div'); root.innerHTML = '<i>one</i><b>two</b>'; document.body.appendChild(root);\n\
           const list = root.querySelectorAll('*'); const seen = [];\n\
           list.forEach(function(node, index, received) { seen.push([this.prefix, node.localName, index, received === list]); }, { prefix: 'node' });\n\
           assertEquals(seen, [['node', 'i', 0, true], ['node', 'b', 1, true]]);\n\
           assertThrows(() => list.forEach(null), TypeError);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM selector NodeList forEach test");
    assert!(
        ran.status.success(),
        "selector NodeList forEach test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_attributes_are_exposed_by_an_element_prototype_getter() {
    let dir = build_dir("t_test_dom_attributes_accessor");
    write_in(
        &dir,
        "attributes-accessor.test.mjs",
        "import { test, expect, mock } from 'runtime:test';\n\
         test('attributes has one inherited, spyable named-node-map getter', () => {\n\
           const element = document.createElement('article'); element.setAttribute('data-id', 'one');\n\
           const read = mock.spyOn(Element.prototype, 'attributes', 'get');\n\
           expect(Object.hasOwn(element, 'attributes')).toBe(false);\n\
           expect(element.attributes.item(0).value).toBe('one');\n\
           expect(read).toHaveBeenCalledOnce();\n\
           read.mockRestore();\n\
           expect(element.attributes.getNamedItem('data-id').value).toBe('one');\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom attributes accessor");
    assert!(
        ran.status.success(),
        "DOM attributes-accessor test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_text_inputs_keep_and_clamp_selection_ranges() {
    let dir = build_dir("t_test_dom_input_selection");
    write_in(
        &dir,
        "selection.test.mjs",
        "import { test, assertEquals, assertThrows } from 'runtime:test';\n\
         test('text inputs expose browser selection state', () => {\n\
           const input = document.createElement('input'); input.value = 'hello';\n\
           input.setSelectionRange(1, 4, 'backward');\n\
           assertEquals([input.selectionStart, input.selectionEnd, input.selectionDirection], [1, 4, 'backward']);\n\
           input.value = 'hi';\n\
           assertEquals([input.selectionStart, input.selectionEnd, input.selectionDirection], [2, 2, 'none']);\n\
           input.setSelectionRange(-1, 99); assertEquals([input.selectionStart, input.selectionEnd], [0, 2]);\n\
           input.type = 'number';\n\
           assertEquals([input.selectionStart, input.selectionEnd, input.selectionDirection], [null, null, null]);\n\
           assertThrows(() => input.setSelectionRange(0, 1), DOMException);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom input selection");
    assert!(
        ran.status.success(),
        "DOM input-selection test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_selector_pseudo_classes_reject_malformed_and_unsupported_syntax() {
    let dir = build_dir("t_test_dom_selector_pseudo_errors");
    write_in(
        &dir,
        "pseudo-errors.test.mjs",
        "import { test, assertEquals, assertThrows } from 'runtime:test';\n\
         test('pseudo parsing stays strict', () => {\n\
           assertThrows(() => document.querySelector(':is()'), SyntaxError);\n\
           assertThrows(() => document.querySelector(':has(>)'), SyntaxError);\n\
           assertThrows(() => document.querySelector(':not(.one'), SyntaxError);\n\
           assertThrows(() => document.querySelector(':nonsense'), SyntaxError);\n\
           assertThrows(() => document.querySelector('::before'), SyntaxError);\n\
           assertThrows(() => document.querySelector(':dir(ltr)'), SyntaxError);\n\
         });\n\
         test('a pseudo-class with no answer here parses and matches nothing', () => {\n\
           document.body.innerHTML = '<a href=\"#x\">link</a><input placeholder=p><input value=v>';\n\
           const names = selector => Array.from(document.body.querySelectorAll(selector), element => element.localName);\n\
           assertEquals([names(':hover'), names(':active'), names(':visited'), names(':focus-visible')], [[], [], [], []]);\n\
           assertEquals(names(':any-link'), ['a']);\n\
           assertEquals(names('[placeholder]:placeholder-shown'), ['input']);\n\
           assertEquals(names('input:read-write').length, 2);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM pseudo selector error test");
    assert!(
        ran.status.success(),
        "pseudo selector error test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_reflects_common_attributes_and_form_defaults() {
    let dir = build_dir("t_test_dom_elements");
    write_in(
        &dir,
        "elements.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('reflection', () => {\n\
           const input = document.createElement('input');\n\
           input.id = 'email'; input.className = 'field'; input.disabled = true; input.value = 42;\n\
           let refused = null; try { input.size = 0; } catch (error) { refused = error.name; }\n\
           assertEquals(input instanceof HTMLElement, true);\n\
           assertEquals(input instanceof HTMLInputElement, true);\n\
           assertEquals(input.getAttribute('id'), 'email');\n\
           assertEquals(input.getAttribute('class'), 'field');\n\
           assertEquals(input.disabled, true);\n\
           assertEquals(input.hasAttribute('disabled'), true);\n\
           assertEquals([refused, input.size], ['IndexSizeError', 20]);\n\
           assertEquals(input.value, '42');\n\
           input.disabled = false;\n\
           assertEquals(input.hasAttribute('disabled'), false);\n\
           const label = document.createElement('label'); label.htmlFor = 'email';\n\
           assertEquals(label.getAttribute('for'), 'email');\n\
           const dialog = document.createElement('dialog'); dialog.open = true;\n\
           assertEquals([dialog instanceof HTMLDialogElement, dialog.open, dialog.getAttribute('open')], [true, true, '']);\n\
           dialog.open = false; assertEquals([dialog.open, dialog.hasAttribute('open')], [false, false]);\n\
         });\n\
         test('click defaults', () => {\n\
           const form = document.createElement('form');\n\
           const checkbox = document.createElement('input'); checkbox.type = 'checkbox';\n\
           const radioOne = document.createElement('input'); radioOne.type = 'radio'; radioOne.name = 'choice';\n\
           const radioTwo = document.createElement('input'); radioTwo.type = 'radio'; radioTwo.name = 'choice';\n\
           const submit = document.createElement('button');\n\
           form.append(checkbox, radioOne, radioTwo, submit); document.body.appendChild(form);\n\
           checkbox.click(); assertEquals(checkbox.checked, true);\n\
           radioOne.click(); radioTwo.click(); assertEquals(radioOne.checked, false); assertEquals(radioTwo.checked, true);\n\
           let submitted = 0; form.addEventListener('submit', () => { submitted += 1; }); submit.click();\n\
           assertEquals(submitted, 1);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom elements");
    assert!(
        ran.status.success(),
        "DOM element test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_select_controls_choose_values_and_expose_live_options() {
    let dir = build_dir("t_test_dom_select_controls");
    write_in(
        &dir,
        "select.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('single selects expose fallback values and selection state', () => {\n\
           const select = document.createElement('select'); const one = document.createElement('option'); const two = document.createElement('option');\n\
           one.textContent = 'one'; two.value = 'second'; two.textContent = 'two'; select.append(one, two); document.body.appendChild(select);\n\
           const options = select.options; assertEquals(select instanceof HTMLSelectElement, true); assertEquals(one instanceof HTMLOptionElement, true);\n\
           assertEquals([select.length, select.selectedIndex, select.value, Array.from(select.selectedOptions)], [2, 0, 'one', [one]]);\n\
           select.value = 'second'; assertEquals([select.selectedIndex, select.value, one.selected, two.selected], [1, 'second', false, true]);\n\
           select.selectedIndex = -1; assertEquals([select.selectedIndex, select.value, Array.from(select.selectedOptions)], [-1, '', []]);\n\
           const three = document.createElement('option'); three.value = 'third'; select.add(three, 1); assertEquals([options.length, options.item(1)], [3, three]);\n\
           select.remove(1); assertEquals(options.length, 2);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM select-control test");
    assert!(
        ran.status.success(),
        "select-control test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_multiple_selects_keep_each_selected_option() {
    let dir = build_dir("t_test_dom_multiple_select");
    write_in(
        &dir,
        "multiple.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('multiple select does not deselect sibling options', () => {\n\
           const select = document.createElement('select'); select.multiple = true;\n\
           const first = document.createElement('option'); first.value = 'first'; const second = document.createElement('option'); second.value = 'second'; const third = document.createElement('option'); third.value = 'third';\n\
           select.append(first, second, third); document.body.appendChild(select); first.selected = true; third.selected = true;\n\
           assertEquals([select.selectedIndex, select.value, Array.from(select.selectedOptions)], [0, 'first', [first, third]]);\n\
           second.selected = true; assertEquals(Array.from(select.selectedOptions), [first, second, third]);\n\
           select.length = 2; assertEquals([select.options.length, Array.from(select.selectedOptions)], [2, [first, second]]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM multiple-select test");
    assert!(
        ran.status.success(),
        "multiple-select test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_form_reset_restores_select_and_textarea_defaults() {
    let dir = build_dir("t_test_dom_form_reset_controls");
    write_in(
        &dir,
        "reset.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('form controls retain defaults and reset dirty values', () => {\n\
           document.body.innerHTML = '<form><select name=choice><option value=one>one</option><option value=two selected>two</option></select><textarea name=note>initial</textarea></form>';\n\
           const form = document.querySelector('form'); const select = document.querySelector('select'); const textarea = document.querySelector('textarea'); const controls = form.elements;\n\
           assertEquals([form instanceof HTMLFormElement, textarea instanceof HTMLTextAreaElement, select.value, textarea.defaultValue, textarea.value, controls.length], [true, true, 'two', 'initial', 'initial', 2]);\n\
           select.value = 'one'; textarea.value = 'changed'; assertEquals([select.value, textarea.value], ['one', 'changed']);\n\
           form.reset(); assertEquals([select.value, textarea.value, Array.from(controls)], ['two', 'initial', [select, textarea]]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM form reset controls test");
    assert!(
        ran.status.success(),
        "form reset controls test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_form_controls_associate_by_ancestor_or_form_attribute() {
    let dir = build_dir("t_test_dom_form_ownership");
    write_in(
        &dir,
        "ownership.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('form ownership includes associated controls outside the form', () => {\n\
           const form = document.createElement('form'); form.id = 'profile'; const inside = document.createElement('input'); inside.name = 'inside';\n\
           const outside = document.createElement('textarea'); outside.name = 'outside'; outside.setAttribute('form', 'profile');\n\
           const foreign = document.createElement('input'); foreign.name = 'foreign'; foreign.setAttribute('form', 'other');\n\
           form.appendChild(inside); document.body.append(form, outside, foreign); const controls = form.elements;\n\
           assertEquals([inside.form, outside.form, foreign.form], [form, form, null]);\n\
           assertEquals(Array.from(controls), [inside, outside]);\n\
           const button = document.createElement('button'); button.setAttribute('form', 'profile'); let submitted = 0; form.addEventListener('submit', (event) => { submitted += 1; assertEquals(event.submitter, button); });\n\
           document.body.appendChild(button); button.click(); assertEquals(submitted, 1);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM form ownership test");
    assert!(
        ran.status.success(),
        "form ownership test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_form_data_serializes_successful_controls_in_tree_order() {
    let dir = build_dir("t_test_dom_form_data");
    write_in(
        &dir,
        "form-data.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('FormData(form) includes successful controls only', () => {\n\
           const form = document.createElement('form'); form.id = 'survey';\n\
           const text = document.createElement('input'); text.name = 'name'; text.value = 'Ada';\n\
           const checked = document.createElement('input'); checked.type = 'checkbox'; checked.name = 'newsletter'; checked.value = 'yes'; checked.checked = true;\n\
           const unchecked = document.createElement('input'); unchecked.type = 'checkbox'; unchecked.name = 'skip'; unchecked.value = 'no';\n\
           const select = document.createElement('select'); select.multiple = true; select.name = 'tag'; const one = document.createElement('option'); one.value = 'one'; one.selected = true; const two = document.createElement('option'); two.value = 'two'; two.selected = true; select.append(one, two);\n\
           const note = document.createElement('textarea'); note.name = 'note'; note.value = 'hello'; const disabled = document.createElement('input'); disabled.name = 'disabled'; disabled.value = 'never'; disabled.disabled = true;\n\
           form.append(text, checked, unchecked, select, note, disabled); document.body.appendChild(form);\n\
           assertEquals(Array.from(new FormData(form)), [['name', 'Ada'], ['newsletter', 'yes'], ['tag', 'one'], ['tag', 'two'], ['note', 'hello']]);\n\
           assertEquals(Array.from(new FormData()), []);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM FormData test");
    assert!(
        ran.status.success(),
        "FormData test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_form_data_includes_externally_associated_controls() {
    let dir = build_dir("t_test_dom_form_data_external");
    write_in(
        &dir,
        "form-data-external.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('FormData follows form ownership rather than subtree only', () => {\n\
           const form = document.createElement('form'); form.id = 'checkout'; const local = document.createElement('input'); local.name = 'local'; local.value = 'one'; form.appendChild(local);\n\
           const external = document.createElement('select'); external.name = 'external'; external.setAttribute('form', 'checkout'); const option = document.createElement('option'); option.value = 'two'; external.appendChild(option);\n\
           document.body.append(form, external); assertEquals(Array.from(new FormData(form)), [['local', 'one'], ['external', 'two']]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn external DOM FormData test");
    assert!(
        ran.status.success(),
        "external FormData test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_controls_report_common_constraint_validity_flags() {
    let dir = build_dir("t_test_dom_constraint_flags");
    write_in(
        &dir,
        "validity.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('required pattern type range and length constraints are distinct', () => {\n\
           const required = document.createElement('input'); required.required = true;\n\
           const pattern = document.createElement('input'); pattern.pattern = '[0-9]{3}'; pattern.value = 'no';\n\
           const email = document.createElement('input'); email.type = 'email'; email.value = 'not-an-email';\n\
           const number = document.createElement('input'); number.type = 'number'; number.min = '2'; number.max = '4'; number.value = '5';\n\
           const length = document.createElement('input'); length.minLength = 3; length.maxLength = 4; length.value = 'ab';\n\
           assertEquals([required.validity.valueMissing, pattern.validity.patternMismatch, email.validity.typeMismatch, number.validity.rangeOverflow, length.validity.tooShort], [true, true, true, true, true]);\n\
           assertEquals([required.checkValidity(), required.validationMessage], [false, 'Constraints not satisfied']);\n\
           required.value = 'ok'; pattern.value = '123'; email.value = 'ada@example.test'; number.value = '3'; length.value = 'abcd';\n\
           assertEquals([required.validity.valid, pattern.validity.valid, email.validity.valid, number.validity.valid, length.validity.valid], [true, true, true, true, true]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM constraint flag test");
    assert!(
        ran.status.success(),
        "constraint flag test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_constraint_validation_dispatches_invalid_and_honors_custom_errors() {
    let dir = build_dir("t_test_dom_constraint_events");
    write_in(
        &dir,
        "invalid.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('invalid events and custom messages follow checkValidity', () => {\n\
           const input = document.createElement('input'); input.required = true; document.body.appendChild(input); let invalid = 0; let bubbled = 0;\n\
           input.addEventListener('invalid', (event) => { invalid += 1; assertEquals(event.bubbles, false); }); document.body.addEventListener('invalid', () => { bubbled += 1; });\n\
           assertEquals(input.reportValidity(), false); assertEquals([invalid, bubbled], [1, 0]);\n\
           input.value = 'present'; input.setCustomValidity('server rejected it');\n\
           assertEquals([input.validity.customError, input.validity.valid, input.validationMessage, input.checkValidity()], [true, false, 'server rejected it', false]);\n\
           input.setCustomValidity(''); assertEquals(input.checkValidity(), true);\n\
           input.disabled = true; input.value = ''; assertEquals([input.willValidate, input.checkValidity()], [false, true]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM constraint event test");
    assert!(
        ran.status.success(),
        "constraint event test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_request_submit_validates_before_emitting_submit() {
    let dir = build_dir("t_test_dom_request_submit");
    write_in(
        &dir,
        "request-submit.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('requestSubmit stops on invalid controls then names its submitter', () => {\n\
           const form = document.createElement('form'); const input = document.createElement('input'); input.required = true; const button = document.createElement('button'); form.append(input, button); document.body.appendChild(form);\n\
           let invalid = 0; let submitted = 0; input.addEventListener('invalid', () => { invalid += 1; }); form.addEventListener('submit', (event) => { submitted += 1; assertEquals(event.submitter, button); });\n\
           form.requestSubmit(button); assertEquals([invalid, submitted], [1, 0]);\n\
           input.value = 'ready'; form.requestSubmit(button); assertEquals([invalid, submitted], [1, 1]);\n\
           input.setCustomValidity('no'); assertEquals(form.checkValidity(), false); input.setCustomValidity(''); assertEquals(form.reportValidity(), true);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM request-submit test");
    assert!(
        ran.status.success(),
        "request-submit test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_disabled_fieldsets_exclude_descendants_from_validation_and_form_data() {
    let dir = build_dir("t_test_dom_fieldset_disabled");
    write_in(
        &dir,
        "fieldset.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('disabled fieldsets suppress descendant controls', () => {\n\
           const form = document.createElement('form'); const fieldset = document.createElement('fieldset'); fieldset.disabled = true; const blocked = document.createElement('input'); blocked.name = 'blocked'; blocked.required = true; blocked.value = '';\n\
           const active = document.createElement('input'); active.name = 'active'; active.value = 'yes'; fieldset.appendChild(blocked); form.append(fieldset, active); document.body.appendChild(form);\n\
           assertEquals([fieldset instanceof HTMLFieldSetElement, blocked.willValidate, blocked.checkValidity(), form.checkValidity(), Array.from(new FormData(form))], [true, false, true, true, [['active', 'yes']]]);\n\
           fieldset.disabled = false; blocked.value = 'now'; assertEquals([blocked.willValidate, form.checkValidity(), Array.from(new FormData(form))], [true, true, [['blocked', 'now'], ['active', 'yes']]]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM fieldset disabled test");
    assert!(
        ran.status.success(),
        "fieldset disabled test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_disabled_optgroups_exclude_selected_options_from_form_data() {
    let dir = build_dir("t_test_dom_optgroup_disabled");
    write_in(
        &dir,
        "optgroup.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('disabled optgroups leave selected options visible but unsuccessful', () => {\n\
           const form = document.createElement('form'); const select = document.createElement('select'); select.name = 'choice'; select.multiple = true;\n\
           const blockedGroup = document.createElement('optgroup'); blockedGroup.disabled = true; blockedGroup.label = 'blocked'; const blocked = document.createElement('option'); blocked.value = 'blocked'; blocked.selected = true; blockedGroup.appendChild(blocked);\n\
           const activeGroup = document.createElement('optgroup'); const active = document.createElement('option'); active.value = 'active'; active.selected = true; activeGroup.appendChild(active);\n\
           select.append(blockedGroup, activeGroup); form.appendChild(select); document.body.appendChild(form);\n\
           assertEquals([blockedGroup instanceof HTMLOptGroupElement, Array.from(select.selectedOptions), Array.from(new FormData(form))], [true, [blocked, active], [['choice', 'active']]]);\n\
           blockedGroup.disabled = false; assertEquals(Array.from(new FormData(form)), [['choice', 'blocked'], ['choice', 'active']]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM optgroup disabled test");
    assert!(
        ran.status.success(),
        "optgroup disabled test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_form_elements_include_live_fieldsets_and_associated_controls() {
    let dir = build_dir("t_test_dom_fieldset_collections");
    write_in(
        &dir,
        "fieldset-collections.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('form elements remain live across fieldset mutations', () => {\n\
           const form = document.createElement('form'); form.id = 'settings'; const fieldset = document.createElement('fieldset'); const inside = document.createElement('input'); fieldset.appendChild(inside); form.appendChild(fieldset);\n\
           const external = document.createElement('textarea'); external.setAttribute('form', 'settings'); document.body.append(form, external); const elements = form.elements;\n\
           assertEquals(Array.from(elements), [fieldset, inside, external]);\n\
           inside.remove(); assertEquals(Array.from(elements), [fieldset, external]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM fieldset collection test");
    assert!(
        ran.status.success(),
        "fieldset collection test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_radio_clicks_are_exclusive_within_one_form_only() {
    let dir = build_dir("t_test_dom_radio_form_scope");
    write_in(
        &dir,
        "radios.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('same-name radios do not cross form ownership', () => {\n\
           const one = document.createElement('form'); const two = document.createElement('form');\n\
           const first = document.createElement('input'); first.type = 'radio'; first.name = 'choice'; const second = document.createElement('input'); second.type = 'radio'; second.name = 'choice'; const other = document.createElement('input'); other.type = 'radio'; other.name = 'choice';\n\
           one.append(first, second); two.appendChild(other); document.body.append(one, two);\n\
           first.click(); other.click(); assertEquals([first.checked, second.checked, other.checked], [true, false, true]);\n\
           second.click(); assertEquals([first.checked, second.checked, other.checked], [false, true, true]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM radio form-scope test");
    assert!(
        ran.status.success(),
        "radio form-scope test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_labels_resolve_controls_and_expose_live_control_labels() {
    let dir = build_dir("t_test_dom_label_association");
    write_in(
        &dir,
        "labels.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('explicit and nested labels share control association', () => {\n\
           const input = document.createElement('input'); input.id = 'email'; const explicit = document.createElement('label'); explicit.htmlFor = 'email'; const nested = document.createElement('label'); nested.appendChild(input); document.body.append(explicit, nested);\n\
           assertEquals([explicit.control, nested.control, Array.from(input.labels)], [input, input, [explicit, nested]]);\n\
           let clicks = 0; input.addEventListener('click', () => { clicks += 1; }); explicit.click(); nested.click(); assertEquals(clicks, 2);\n\
           const extra = document.createElement('label'); extra.htmlFor = 'email'; document.body.appendChild(extra); assertEquals(Array.from(input.labels), [explicit, nested, extra]);\n\
           extra.remove(); assertEquals(Array.from(input.labels), [explicit, nested]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM label association test");
    assert!(
        ran.status.success(),
        "label association test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_checkbox_and_radio_form_data_default_to_on() {
    let dir = build_dir("t_test_dom_checkable_default_value");
    write_in(
        &dir,
        "checkable-values.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('successful checkable controls default their value to on', () => {\n\
           const form = document.createElement('form'); const checkbox = document.createElement('input'); checkbox.type = 'checkbox'; checkbox.name = 'check'; checkbox.checked = true; const radio = document.createElement('input'); radio.type = 'radio'; radio.name = 'radio'; radio.checked = true;\n\
           form.append(checkbox, radio); document.body.appendChild(form);\n\
           assertEquals([checkbox.value, radio.value, Array.from(new FormData(form))], ['on', 'on', [['check', 'on'], ['radio', 'on']]]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM checkable value test");
    assert!(
        ran.status.success(),
        "checkable value test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_form_reset_restores_dirty_input_values_to_default_values() {
    let dir = build_dir("t_test_dom_input_value_reset");
    write_in(
        &dir,
        "input-values.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('value changes are dirty state rather than attribute writes', () => {\n\
           const form = document.createElement('form'); const input = document.createElement('input'); input.defaultValue = 'markup'; form.appendChild(input); document.body.appendChild(form);\n\
           input.value = 'typed'; assertEquals([input.value, input.defaultValue, input.getAttribute('value')], ['typed', 'markup', 'markup']);\n\
           form.reset(); assertEquals(input.value, 'markup');\n\
           input.defaultValue = 'revised'; assertEquals([input.value, input.defaultValue], ['revised', 'revised']);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM input value reset test");
    assert!(
        ran.status.success(),
        "input value reset test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_form_reset_restores_checkable_defaults_without_rewriting_attributes() {
    let dir = build_dir("t_test_dom_checked_reset");
    write_in(
        &dir,
        "checked-reset.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('checked state is independent from defaultChecked', () => {\n\
           const form = document.createElement('form'); const checked = document.createElement('input'); checked.type = 'checkbox'; checked.defaultChecked = true; const clear = document.createElement('input'); clear.type = 'checkbox'; form.append(checked, clear); document.body.appendChild(form);\n\
           checked.checked = false; clear.checked = true; assertEquals([checked.checked, checked.defaultChecked, clear.checked, clear.defaultChecked, checked.hasAttribute('checked')], [false, true, true, false, true]);\n\
           form.reset(); assertEquals([checked.checked, clear.checked], [true, false]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM checked reset test");
    assert!(
        ran.status.success(),
        "checked reset test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_form_data_uses_dirty_input_values_before_and_after_reset() {
    let dir = build_dir("t_test_dom_dirty_form_data");
    write_in(
        &dir,
        "dirty-form-data.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('FormData reads current dirty values', () => {\n\
           const form = document.createElement('form'); const input = document.createElement('input'); input.name = 'message'; input.defaultValue = 'initial'; form.appendChild(input); document.body.appendChild(form);\n\
           input.value = 'edited'; assertEquals(Array.from(new FormData(form)), [['message', 'edited']]);\n\
           form.reset(); assertEquals(Array.from(new FormData(form)), [['message', 'initial']]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM dirty FormData test");
    assert!(
        ran.status.success(),
        "dirty FormData test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_number_inputs_normalize_values_and_expose_numeric_state() {
    let dir = build_dir("t_test_dom_number_inputs");
    write_in(
        &dir,
        "number.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('number inputs sanitize and convert values', () => {\n\
           const input = document.createElement('input'); input.type = 'number';\n\
           input.value = '004.50'; assertEquals([input.value, input.valueAsNumber], ['004.50', 4.5]);\n\
           input.value = 'not-a-number'; assertEquals([input.value, Number.isNaN(input.valueAsNumber)], ['', true]);\n\
           input.valueAsNumber = 12.25; assertEquals([input.value, input.valueAsNumber], ['12.25', 12.25]);\n\
           input.valueAsNumber = NaN; assertEquals(input.value, '');\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM number input test");
    assert!(
        ran.status.success(),
        "number input test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_date_inputs_validate_calendar_values_and_dates() {
    let dir = build_dir("t_test_dom_date_inputs");
    write_in(
        &dir,
        "date.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('date inputs reject impossible dates and use UTC dates', () => {\n\
           const input = document.createElement('input'); input.type = 'date'; input.value = '2024-02-29';\n\
           assertEquals([input.value, input.valueAsDate.toISOString(), input.valueAsNumber], ['2024-02-29', '2024-02-29T00:00:00.000Z', Date.UTC(2024, 1, 29)]);\n\
           input.value = '2023-02-29'; assertEquals([input.value, input.valueAsDate, Number.isNaN(input.valueAsNumber)], ['', null, true]);\n\
           input.valueAsDate = new Date('2025-03-04T12:00:00Z'); assertEquals(input.value, '2025-03-04');\n\
           input.valueAsDate = null; assertEquals(input.value, '');\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM date input test");
    assert!(
        ran.status.success(),
        "date input test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_value_as_accessors_reject_unsupported_input_types() {
    let dir = build_dir("t_test_dom_value_as_type_errors");
    write_in(
        &dir,
        "value-as-errors.test.mjs",
        "import { test, assertEquals, assertThrows } from 'runtime:test';\n\
         test('valueAs accessors are strict outside number and date types', () => {\n\
           const text = document.createElement('input'); text.type = 'text'; text.value = 'plain';\n\
           assertEquals([Number.isNaN(text.valueAsNumber), text.valueAsDate], [true, null]);\n\
           assertThrows(() => { text.valueAsNumber = 1; }, DOMException); assertThrows(() => { text.valueAsDate = new Date(); }, DOMException);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM valueAs type error test");
    assert!(
        ran.status.success(),
        "valueAs type error test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_replace_children_coerces_strings_and_moves_fragments() {
    let dir = build_dir("t_test_dom_replace_children");
    write_in(
        &dir,
        "replace-children.test.mjs",
        r#"import { test, assertEquals } from 'runtime:test';
test('replaceChildren coerces strings and empties fragments', () => {
  const host = document.createElement('div'); host.append('old', document.createElement('i'));
  const fragment = document.createDocumentFragment(); fragment.append(document.createElement('b'), 'tail');
  host.replaceChildren('head', fragment);
  assertEquals([host.innerHTML, fragment.childNodes.length], ['head<b></b>tail', 0]);
});
"#,
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM replaceChildren test");
    assert!(
        ran.status.success(),
        "replaceChildren test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_element_child_accessors_skip_non_elements() {
    let dir = build_dir("t_test_dom_element_child_accessors");
    write_in(
        &dir,
        "element-children.test.mjs",
        r#"import { test, assertEquals } from 'runtime:test';
test('element child accessors skip text and comments', () => {
  const parent = document.createElement('div'); const first = document.createElement('i'); const last = document.createElement('b');
  parent.append('before', first, document.createComment('note'), last, 'after');
  assertEquals([parent.childElementCount, parent.firstElementChild, parent.lastElementChild], [2, first, last]);
  assertEquals(Array.prototype.slice.call(parent.childNodes), Array.from(parent.childNodes));
});
"#,
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM element child accessors test");
    assert!(
        ran.status.success(),
        "element child accessors test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_element_sibling_accessors_follow_tree_mutations() {
    let dir = build_dir("t_test_dom_element_sibling_accessors");
    write_in(
        &dir,
        "element-siblings.test.mjs",
        r#"import { test, assertEquals } from 'runtime:test';
test('element sibling accessors follow insertions and removals', () => {
  const parent = document.createElement('div'); const first = document.createElement('i'); const middle = document.createElement('b'); const last = document.createElement('em');
  parent.append(first, 'text', middle, document.createComment('note'), last);
  assertEquals([middle.previousElementSibling, middle.nextElementSibling], [first, last]);
  middle.remove(); assertEquals([first.nextElementSibling, last.previousElementSibling], [last, first]);
});
"#,
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM element sibling accessors test");
    assert!(
        ran.status.success(),
        "element sibling accessors test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_submit_buttons_honor_form_settings_and_validation_bypasses() {
    let dir = build_dir("t_test_dom_submit_settings");
    write_in(
        &dir,
        "submit.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
      test('submit settings and bypasses', () => {\n\
        const form = document.createElement('form'); form.action = '/send'; form.method = 'POST'; form.enctype = 'multipart/form-data'; form.target = '_blank';\n\
        const input = document.createElement('input'); input.required = true; const button = document.createElement('button'); form.append(input, button); document.body.appendChild(form); let submits = 0; form.addEventListener('submit', () => { submits += 1; });\n\
        button.click(); assertEquals(submits, 0); button.formNoValidate = true; button.click(); assertEquals(submits, 1);\n\
        button.formNoValidate = false; form.noValidate = true; form.requestSubmit(); assertEquals(submits, 2);\n\
        assertEquals([form.action, form.method, form.enctype, form.target], ['http://localhost/send', 'post', 'multipart/form-data', '_blank']);\n\
      });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM submit settings test");
    assert!(
        ran.status.success(),
        "submit settings test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_form_action_resolves_against_the_current_location() {
    let dir = build_dir("t_test_dom_form_action_resolution");
    write_in(
        &dir,
        "form-action.test.mjs",
        r#"import { test, assertEquals } from 'runtime:test';
test('form actions resolve from the in-memory location', () => {
  const form = document.createElement('form'); form.action = '/send';
  assertEquals([form.getAttribute('action'), form.action], ['/send', 'http://localhost/send']);
  history.pushState(null, '', '/nested/page'); form.action = 'next';
  assertEquals(form.action, 'http://localhost/nested/next');
});
"#,
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM form action test");
    assert!(
        ran.status.success(),
        "form action test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_submitter_actions_resolve_and_preserve_their_attributes() {
    let dir = build_dir("t_test_dom_submitter_action_resolution");
    write_in(
        &dir,
        "submitter-action.test.mjs",
        r#"import { test, assertEquals } from 'runtime:test';
test('submitter actions resolve without rewriting attributes', () => {
  const button = document.createElement('button'); const input = document.createElement('input'); input.type = 'submit';
  button.formAction = '/button'; input.formAction = 'input';
  assertEquals([button.getAttribute('formaction'), button.formAction], ['/button', 'http://localhost/button']);
  assertEquals([input.getAttribute('formaction'), input.formAction], ['input', 'http://localhost/input']);
});
"#,
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM submitter action test");
    assert!(
        ran.status.success(),
        "submitter action test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_form_action_defaults_to_the_current_location() {
    let dir = build_dir("t_test_dom_form_action_default");
    write_in(
        &dir,
        "form-action-default.test.mjs",
        r#"import { test, assertEquals } from 'runtime:test';
test('missing form action uses the current location', () => {
  const form = document.createElement('form'); const submit = document.createElement('button');
  assertEquals([form.action, submit.formAction], ['http://localhost/', 'http://localhost/']);
  history.replaceState(null, '', '/current'); assertEquals([form.action, submit.formAction], ['http://localhost/current', 'http://localhost/current']);
});
"#,
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM form action default test");
    assert!(
        ran.status.success(),
        "form action default test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_request_submit_uses_default_submitter_and_rejects_foreign_button() {
    let dir = build_dir("t_test_dom_submit_default");
    write_in(
        &dir,
        "submit-default.test.mjs",
        r#"import { test, assertEquals, assertThrows } from 'runtime:test';
test('requestSubmit selects the default submitter', () => {
  const form = document.createElement('form'); const first = document.createElement('button'); const second = document.createElement('button'); form.append(first, second); document.body.appendChild(form);
  let submitter = null; form.addEventListener('submit', (event) => { submitter = event.submitter; }); form.requestSubmit(); assertEquals(submitter, first);
  const foreign = document.createElement('button'); assertThrows(() => form.requestSubmit(foreign), TypeError);
});
"#,
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM default submitter test");
    assert!(
        ran.status.success(),
        "default submitter test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_submit_button_respects_cancelled_clicks_and_disabled_state() {
    let dir = build_dir("t_test_dom_submit_click");
    write_in(
        &dir,
        "submit-click.test.mjs",
        r#"import { test, assertEquals } from 'runtime:test';
test('submit button activation is cancelable and disabled-aware', () => {
  const form = document.createElement('form'); const button = document.createElement('button'); form.appendChild(button); document.body.appendChild(form);
  let submissions = 0; form.addEventListener('submit', () => { submissions += 1; }); button.addEventListener('click', (event) => event.preventDefault(), { once: true });
  button.click(); assertEquals(submissions, 0); button.disabled = true; button.click(); assertEquals(submissions, 0);
  button.disabled = false; button.click(); assertEquals(submissions, 1);
});
"#,
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM submit click test");
    assert!(
        ran.status.success(),
        "submit click test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_submit_inputs_validate_and_can_bypass_validation() {
    let dir = build_dir("t_test_dom_submit_input_validation");
    write_in(
        &dir,
        "submit-input-validation.test.mjs",
        r#"import { test, assertEquals } from 'runtime:test';
test('submit inputs validate and honor formNoValidate', () => {
  const form = document.createElement('form'); const required = document.createElement('input'); required.required = true; const submit = document.createElement('input'); submit.type = 'submit'; form.append(required, submit); document.body.appendChild(form);
  let submitter = null; form.addEventListener('submit', (event) => { submitter = event.submitter; });
  submit.click(); assertEquals(submitter, null);
  submit.formNoValidate = true; submit.click(); assertEquals(submitter, submit);
});
"#,
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM submit input validation test");
    assert!(
        ran.status.success(),
        "submit input validation test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_submitters_reflect_form_override_settings() {
    let dir = build_dir("t_test_dom_submitter_overrides");
    write_in(
        &dir,
        "submitter-overrides.test.mjs",
        r#"import { test, assertEquals } from 'runtime:test';
test('submitters reflect their form overrides', () => {
  const button = document.createElement('button'); const input = document.createElement('input'); input.type = 'submit';
  button.formAction = '/button'; button.formMethod = 'post'; button.formEnctype = 'text/plain'; button.formTarget = '_blank'; button.formNoValidate = true;
  input.formAction = '/input'; input.formMethod = 'dialog'; input.formEnctype = 'multipart/form-data'; input.formTarget = 'result'; input.formNoValidate = true;
  assertEquals([button.formAction, button.formMethod, button.formEnctype, button.formTarget, button.formNoValidate], ['http://localhost/button', 'post', 'text/plain', '_blank', true]);
  assertEquals([input.formAction, input.formMethod, input.formEnctype, input.formTarget, input.formNoValidate], ['http://localhost/input', 'dialog', 'multipart/form-data', 'result', true]);
});
"#,
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM submitter override test");
    assert!(
        ran.status.success(),
        "submitter override test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_request_submit_rejects_non_submit_inputs() {
    let dir = build_dir("t_test_dom_non_submit_request");
    write_in(
        &dir,
        "non-submitter.test.mjs",
        r#"import { test, assertThrows } from 'runtime:test';
test('requestSubmit rejects a non-submit input', () => {
  const form = document.createElement('form'); const text = document.createElement('input'); form.appendChild(text); document.body.appendChild(form);
  assertThrows(() => form.requestSubmit(text), TypeError);
});
"#,
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM non-submitter test");
    assert!(
        ran.status.success(),
        "non-submitter test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_reset_button_restores_dirty_control_state() {
    let dir = build_dir("t_test_dom_reset_button");
    write_in(
        &dir,
        "reset-button.test.mjs",
        r#"import { test, assertEquals } from 'runtime:test';
test('reset button restores dirty input state', () => {
  const form = document.createElement('form'); const text = document.createElement('input'); text.defaultValue = 'markup'; const check = document.createElement('input'); check.type = 'checkbox'; check.defaultChecked = true; const reset = document.createElement('button'); reset.type = 'reset'; form.append(text, check, reset); document.body.appendChild(form);
  text.value = 'changed'; check.checked = false; reset.click();
  assertEquals([text.value, check.checked], ['markup', true]);
});
"#,
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM reset button test");
    assert!(
        ran.status.success(),
        "reset button test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_reset_input_restores_select_and_textarea_state() {
    let dir = build_dir("t_test_dom_reset_input");
    write_in(
        &dir,
        "reset-input.test.mjs",
        r#"import { test, assertEquals } from 'runtime:test';
test('reset input restores select and textarea state', () => {
  const form = document.createElement('form'); const select = document.createElement('select'); const first = document.createElement('option'); first.value = 'one'; const second = document.createElement('option'); second.value = 'two'; second.defaultSelected = true; select.append(first, second); const area = document.createElement('textarea'); area.defaultValue = 'markup'; const reset = document.createElement('input'); reset.type = 'reset'; form.append(select, area, reset); document.body.appendChild(form);
  select.value = 'one'; area.value = 'changed'; reset.click();
  assertEquals([select.value, area.value], ['two', 'markup']);
});
"#,
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM reset input test");
    assert!(
        ran.status.success(),
        "reset input test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_reset_control_honors_cancelled_events_and_disabled_state() {
    let dir = build_dir("t_test_dom_reset_cancellation");
    write_in(
        &dir,
        "reset-cancellation.test.mjs",
        r#"import { test, assertEquals } from 'runtime:test';
test('reset activation respects cancellation and disabled controls', () => {
  const form = document.createElement('form'); const text = document.createElement('input'); text.defaultValue = 'markup'; const reset = document.createElement('button'); reset.type = 'reset'; form.append(text, reset); document.body.appendChild(form);
  text.value = 'changed'; form.addEventListener('reset', (event) => event.preventDefault(), { once: true }); reset.click(); assertEquals(text.value, 'changed');
  reset.disabled = true; reset.click(); assertEquals(text.value, 'changed');
  reset.disabled = false; reset.click(); assertEquals(text.value, 'markup');
});
"#,
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM reset cancellation test");
    assert!(
        ran.status.success(),
        "reset cancellation test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_range_tracks_and_validates_boundary_points() {
    let dir = build_dir("t_test_dom_range_boundaries");
    write_in(
        &dir,
        "boundaries.test.mjs",
        "import { test, assertEquals, assertThrows } from 'runtime:test';\n\
         test('boundary setters collapse and reject invalid offsets', () => {\n\
           const parent = document.createElement('div'); const first = document.createElement('i'); const second = document.createElement('b'); parent.append(first, second); document.body.appendChild(parent);\n\
           const range = document.createRange(); assertEquals(range instanceof Range, true); assertEquals(range.collapsed, true);\n\
           range.setStart(parent, 1); range.setEnd(parent, 2); assertEquals([range.startOffset, range.endOffset, range.collapsed], [1, 2, false]);\n\
           range.setStart(parent, 2); assertEquals([range.startOffset, range.endOffset, range.collapsed], [2, 2, true]);\n\
           range.setEnd(parent, 0); assertEquals([range.startOffset, range.endOffset, range.collapsed], [0, 0, true]);\n\
           range.setStartBefore(second); range.setEndAfter(second); assertEquals([range.startOffset, range.endOffset], [1, 2]);\n\
           assertThrows(() => range.setStart(parent, 3), DOMException); assertThrows(() => range.setEnd(parent, -1), DOMException);\n\
           const other = new Document().createElement('div'); assertThrows(() => range.setStart(other, 0), DOMException);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM range boundary test");
    assert!(
        ran.status.success(),
        "range boundary test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_range_extracts_text_across_tree_boundaries() {
    let dir = build_dir("t_test_dom_range_text");
    write_in(
        &dir,
        "text.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('range text walks text nodes between boundaries', () => {\n\
           document.body.innerHTML = '<p>one <b>two</b> three</p>'; const paragraph = document.querySelector('p');\n\
           const first = paragraph.firstChild; const last = paragraph.lastChild; const range = document.createRange();\n\
           range.setStart(first, 1); range.setEnd(last, 2); assertEquals(range.toString(), 'ne two t');\n\
           const selected = document.createRange(); selected.selectNode(paragraph.querySelector('b'));\n\
           assertEquals(selected.toString(), 'two'); assertEquals(selected.commonAncestorContainer, paragraph);\n\
           assertEquals(range.compareBoundaryPoints(Range.START_TO_START, selected), -1);\n\
           assertEquals(range.compareBoundaryPoints(Range.END_TO_END, selected), 1);\n\
           range.collapse(true); assertEquals(range.toString(), '');\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM range text test");
    assert!(
        ran.status.success(),
        "range text test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_range_creates_contextual_fragments() {
    let dir = build_dir("t_test_dom_range_fragment");
    write_in(
        &dir,
        "fragment.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('contextual fragments use the start container context', () => {\n\
           const host = document.createElement('section'); const range = document.createRange(); document.body.appendChild(host); range.selectNodeContents(host);\n\
           const fragment = range.createContextualFragment('<em class=note>new</em><!--tail-->');\n\
           assertEquals(fragment.nodeType, Node.DOCUMENT_FRAGMENT_NODE); assertEquals(fragment.firstChild.localName, 'em'); assertEquals(fragment.firstChild.className, 'note');\n\
           host.appendChild(fragment); assertEquals(host.innerHTML, '<em class=\"note\">new</em><!--tail-->');\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM range fragment test");
    assert!(
        ran.status.success(),
        "range fragment test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_range_inserts_nodes_at_text_and_element_boundaries() {
    let dir = build_dir("t_test_dom_range_insert");
    write_in(
        &dir,
        "insert.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('insertNode splits text and honors child offsets', () => {\n\
           const paragraph = document.createElement('p'); const text = document.createTextNode('ab'); paragraph.appendChild(text); document.body.appendChild(paragraph);\n\
           const inside = document.createRange(); inside.setStart(text, 1); const emphasis = document.createElement('em'); emphasis.textContent = 'x'; inside.insertNode(emphasis);\n\
           assertEquals(paragraph.innerHTML, 'a<em>x</em>b');\n\
           const edge = document.createRange(); edge.setStart(paragraph, 0); const strong = document.createElement('strong'); strong.textContent = 'first'; edge.insertNode(strong);\n\
           assertEquals(paragraph.innerHTML, '<strong>first</strong>a<em>x</em>b');\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM range insertion test");
    assert!(
        ran.status.success(),
        "range insertion test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_range_deletes_same_text_and_child_boundaries() {
    let dir = build_dir("t_test_dom_range_delete_simple");
    write_in(
        &dir,
        "delete-simple.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('deleteContents removes text slices and complete child nodes', () => {\n\
           const text = document.createTextNode('abcdef'); document.body.appendChild(text); const characters = document.createRange();\n\
           characters.setStart(text, 2); characters.setEnd(text, 4); characters.deleteContents();\n\
           assertEquals(text.data, 'abef'); assertEquals([characters.startOffset, characters.endOffset, characters.collapsed], [2, 2, true]);\n\
           const parent = document.createElement('div'); parent.innerHTML = '<i>one</i><b>two</b><em>three</em>'; document.body.appendChild(parent); const children = document.createRange();\n\
           children.setStart(parent, 1); children.setEnd(parent, 3); children.deleteContents();\n\
           assertEquals(parent.innerHTML, '<i>one</i>'); assertEquals([children.startOffset, children.endOffset], [1, 1]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM simple range deletion test");
    assert!(
        ran.status.success(),
        "simple range deletion test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_range_deletes_across_nested_text_boundaries() {
    let dir = build_dir("t_test_dom_range_delete_nested");
    write_in(
        &dir,
        "delete-nested.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('deleteContents removes covered descendants between partial text nodes', () => {\n\
           document.body.innerHTML = '<p>one <b>two</b> three</p>'; const paragraph = document.querySelector('p'); const range = document.createRange();\n\
           range.setStart(paragraph.firstChild, 2); range.setEnd(paragraph.lastChild, 2); range.deleteContents();\n\
           assertEquals(paragraph.innerHTML, 'onhree'); assertEquals(range.collapsed, true); assertEquals(range.toString(), '');\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM nested range deletion test");
    assert!(
        ran.status.success(),
        "nested range deletion test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_range_clones_and_extracts_partial_nested_contents() {
    let dir = build_dir("t_test_dom_range_clone_extract");
    write_in(
        &dir,
        "clone-extract.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('cloneContents preserves selected structure without mutation', () => {\n\
           document.body.innerHTML = '<p>one <b>two</b> three</p>'; const paragraph = document.querySelector('p'); const range = document.createRange();\n\
           range.setStart(paragraph.firstChild, 1); range.setEnd(paragraph.lastChild, 2);\n\
           const copy = range.cloneContents(); const holder = document.createElement('div'); holder.appendChild(copy);\n\
           assertEquals(holder.innerHTML, 'ne <b>two</b> t'); assertEquals(paragraph.innerHTML, 'one <b>two</b> three');\n\
         });\n\
         test('extractContents returns the same structure and removes it', () => {\n\
           const paragraph = document.querySelector('p'); const range = document.createRange(); range.setStart(paragraph.firstChild, 1); range.setEnd(paragraph.lastChild, 2);\n\
           const extracted = range.extractContents(); const holder = document.createElement('div'); holder.appendChild(extracted);\n\
           assertEquals(holder.innerHTML, 'ne <b>two</b> t'); assertEquals(paragraph.innerHTML, 'ohree'); assertEquals(range.collapsed, true);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM range clone/extract test");
    assert!(
        ran.status.success(),
        "range clone/extract test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_range_surrounds_complete_contents_and_rejects_partial_elements() {
    let dir = build_dir("t_test_dom_range_surround");
    write_in(
        &dir,
        "surround.test.mjs",
        "import { test, assertEquals, assertThrows } from 'runtime:test';\n\
         test('surroundContents wraps complete nodes', () => {\n\
           document.body.innerHTML = '<p>one <b>two</b> three</p>'; const paragraph = document.querySelector('p'); const bold = paragraph.querySelector('b'); const range = document.createRange();\n\
           range.selectNode(bold); const mark = document.createElement('mark'); range.surroundContents(mark);\n\
           assertEquals(paragraph.innerHTML, 'one <mark><b>two</b></mark> three'); assertEquals(range.toString(), 'two'); assertEquals(range.commonAncestorContainer, paragraph);\n\
         });\n\
         test('partial non-text nodes refuse wrapping', () => {\n\
           const paragraph = document.querySelector('p'); const range = document.createRange(); range.setStart(paragraph.firstChild, 1); range.setEnd(paragraph.querySelector('b').firstChild, 1);\n\
           assertThrows(() => range.surroundContents(document.createElement('mark')), DOMException);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM range surround test");
    assert!(
        ran.status.success(),
        "range surround test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_ranges_adjust_boundaries_for_tree_and_text_mutations() {
    let dir = build_dir("t_test_dom_range_adjustment");
    write_in(
        &dir,
        "adjustment.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('unrelated inserts, removals, and shortening text retain valid boundaries', () => {\n\
           const parent = document.createElement('div'); const first = document.createElement('i'); const second = document.createElement('b'); const third = document.createElement('em'); parent.append(first, second, third); document.body.appendChild(parent);\n\
           const children = document.createRange(); children.setStart(parent, 2); children.setEnd(parent, 3);\n\
           parent.insertBefore(document.createElement('strong'), second); assertEquals([children.startOffset, children.endOffset], [3, 4]);\n\
           first.remove(); assertEquals([children.startOffset, children.endOffset], [2, 3]);\n\
           const inside = document.createRange(); inside.selectNodeContents(second); second.remove(); assertEquals([inside.startContainer, inside.startOffset, inside.endContainer, inside.endOffset], [parent, 1, parent, 1]);\n\
           const text = document.createTextNode('abcdef'); parent.appendChild(text); const characters = document.createRange(); characters.setStart(text, 1); characters.setEnd(text, 5); text.data = 'xy';\n\
           assertEquals([characters.startOffset, characters.endOffset], [1, 2]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM range adjustment test");
    assert!(
        ran.status.success(),
        "range adjustment test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_document_ids_and_indexed_attributes_follow_browser_collections() {
    let dir = build_dir("t_test_dom_document_ids_and_attributes");
    write_in(
        &dir,
        "document-ids.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('document IDs and named attributes follow document order and mutations', () => {\n\
           const first = document.createElement('article'); const second = document.createElement('aside');\n\
           first.id = 'duplicate'; second.id = 'duplicate'; second.setAttribute('data-state', 'ready'); second.setAttribute('title', 'second');\n\
           document.body.append(first, second); const attributes = second.attributes;\n\
           assertEquals(document.getElementById('duplicate'), first); assertEquals(attributes[0].name, 'id'); assertEquals(attributes[1].value, 'ready'); assertEquals(attributes[3], undefined); assertEquals(attributes.item(3), null);\n\
           first.remove(); second.removeAttribute('id');\n\
           assertEquals(document.getElementById('duplicate'), null); assertEquals(attributes[0].name, 'data-state'); assertEquals(attributes.length, 2);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom document IDs and attributes");
    assert!(
        ran.status.success(),
        "DOM document ID and attribute test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_namespace_attribute_operations_follow_named_node_maps() {
    let dir = build_dir("t_test_dom_namespace_attributes");
    write_in(
        &dir,
        "namespace-attributes.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('namespace attribute methods keep ordinary and namespaced attributes distinct', () => {\n\
           const use = document.createElement('use'); const xlink = 'http://www.w3.org/1999/xlink';\n\
           use.setAttribute('href', 'plain'); use.setAttributeNS(xlink, 'xlink:href', '#first');\n\
           assertEquals([use.getAttributeNS(null, 'href'), use.getAttributeNS(xlink, 'href'), use.hasAttributeNS(xlink, 'href')], ['plain', '#first', true]);\n\
           use.setAttributeNS(xlink, 'xlink:href', '#next'); assertEquals(use.attributes.length, 2);\n\
           const clone = use.cloneNode(); assertEquals(clone.getAttributeNS(xlink, 'href'), '#next');\n\
           use.removeAttributeNS(xlink, 'href'); assertEquals([use.hasAttribute('href'), use.hasAttributeNS(xlink, 'href')], [true, false]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom namespace attributes");
    assert!(
        ran.status.success(),
        "DOM namespace attribute test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_datasets_follow_live_data_attributes_and_html_name_conversion() {
    let dir = build_dir("t_test_dom_datasets");
    write_in(
        &dir,
        "datasets.test.mjs",
        "import { test, assertEquals, assertThrows } from 'runtime:test';\n\
         test('dataset properties reflect data attributes, enumeration, deletion, and conversion', () => {\n\
           const element = document.createElement('article'); const dataset = element.dataset;\n\
           element.setAttribute('data-user-id', 'first'); element.setAttribute('data-ready', '');\n\
           assertEquals([dataset === element.dataset, dataset.userId, Object.keys(dataset)], [true, 'first', ['userId', 'ready']]);\n\
           dataset.userId = 42; delete dataset.ready; dataset.recordId = 'next';\n\
           assertEquals([element.getAttribute('data-user-id'), element.hasAttribute('data-ready'), element.getAttribute('data-record-id')], ['42', false, 'next']);\n\
           assertThrows(() => { dataset['record-id'] = 'no'; }, DOMException);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom datasets");
    assert!(
        ran.status.success(),
        "DOM dataset test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_input_indeterminate_is_boolean_non_reflecting_state() {
    let dir = build_dir("t_test_dom_input_indeterminate");
    write_in(
        &dir,
        "indeterminate.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('input indeterminate defaults false and remains independent of checked and attributes', () => {\n\
           const input = document.createElement('input'); input.type = 'checkbox';\n\
           assertEquals([input.indeterminate, input.hasAttribute('indeterminate')], [false, false]);\n\
           input.indeterminate = 1; input.checked = true;\n\
           assertEquals([input.indeterminate, input.checked, input.hasAttribute('indeterminate')], [true, true, false]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom input indeterminate");
    assert!(
        ran.status.success(),
        "DOM input indeterminate test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_inline_handlers_run_for_modern_events() {
    let dir = build_dir("t_test_dom_inline_events");
    write_in(
        &dir,
        "inline-events.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('modern Event dispatch invokes inline handlers', () => {\n\
           const button = document.createElement('button'); const calls = [];\n\
           button.onclick = (event) => { calls.push(event.type); event.preventDefault(); };\n\
           const event = new Event('click', { bubbles: true, cancelable: true });\n\
           assertEquals([button.dispatchEvent(event), calls], [false, ['click']]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom inline events");
    assert!(
        ran.status.success(),
        "DOM inline event test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_create_element_ns_preserves_modern_namespace_identity() {
    let dir = build_dir("t_test_dom_create_element_ns");
    write_in(
        &dir,
        "element-ns.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('createElementNS installs namespace and specialized interfaces', () => {\n\
           const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');\n\
           const use = document.createElementNS('http://www.w3.org/2000/svg', 'xlink:use');\n\
           const input = document.createElementNS('http://www.w3.org/1999/xhtml', 'input'); const div = document.createElement('div');\n\
           const math = document.createElementNS('http://www.w3.org/1998/Math/MathML', 'math');\n\
           const inputCopy = input.cloneNode(); const svgCopy = svg.cloneNode();\n\
           input.value = 'modern';\n\
           assertEquals([svg instanceof SVGElement, svg.namespaceURI, svg.nodeName, use.prefix, use.localName, input instanceof HTMLInputElement, input.value, div instanceof HTMLDivElement, math instanceof MathMLElement, inputCopy instanceof HTMLInputElement, svgCopy instanceof SVGElement], [true, 'http://www.w3.org/2000/svg', 'svg', 'xlink', 'use', true, 'modern', true, true, true, true]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom createElementNS");
    assert!(
        ran.status.success(),
        "DOM createElementNS test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_shadow_roots_keep_content_scoped_and_cross_only_composed_events() {
    let dir = build_dir("t_test_dom_shadow_root");
    write_in(
        &dir,
        "shadow.test.mjs",
        "import { test, assertEquals, assertThrows } from 'runtime:test';\n\
         test('open and closed shadow roots preserve tree boundaries', () => {\n\
           const host = document.createElement('section'); document.body.appendChild(host);\n\
           const root = host.attachShadow({ mode: 'open' }); root.innerHTML = '<button>inside</button>';\n\
           const button = root.querySelector('button'); const events = [];\n\
           host.addEventListener('go', () => events.push('host')); button.addEventListener('go', () => events.push('button'));\n\
           button.dispatchEvent(new Event('go', { bubbles: true }));\n\
           assertEquals([host.shadowRoot, root.host, root.mode, root.innerHTML, document.querySelector('button'), button.getRootNode(), button.getRootNode({ composed: true }), events], [root, host, 'open', '<button>inside</button>', null, root, document, ['button']]);\n\
           button.dispatchEvent(new Event('go', { bubbles: true, composed: true })); assertEquals(events, ['button', 'button', 'host']);\n\
           assertThrows(() => host.attachShadow({ mode: 'open' }), DOMException);\n\
           const closedHost = document.createElement('div'); const closed = closedHost.attachShadow({ mode: 'closed' });\n\
           assertEquals([closed instanceof ShadowRoot, closedHost.shadowRoot], [true, null]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM shadow-root test");
    assert!(
        ran.status.success(),
        "shadow-root test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_shadow_slots_assign_named_default_and_fallback_content() {
    let dir = build_dir("t_test_dom_shadow_slots");
    write_in(
        &dir,
        "slots.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('slots discover distributed light children and fallback content', () => {\n\
           const host = document.createElement('section');\n\
           host.innerHTML = '<h1 slot=title>Title</h1>light text<p>default</p>';\n\
           const root = host.attachShadow({ mode: 'open' });\n\
           root.innerHTML = '<header><slot name=title><i>title fallback</i></slot></header><main><slot><em>default fallback</em></slot></main><footer><slot name=missing><b>missing fallback</b></slot></footer>';\n\
           const [title, standard, missing] = root.querySelectorAll('slot');\n\
           assertEquals([title instanceof HTMLSlotElement, title.assignedElements().map((node) => node.localName), standard.assignedNodes().map((node) => node.nodeName), missing.assignedNodes().length, missing.assignedElements({ flatten: true }).map((node) => node.localName)], [true, ['h1'], ['#text', 'P'], 0, ['b']]);\n\
           title.name = 'other';\n\
           assertEquals([title.assignedNodes().length, title.assignedElements({ flatten: true }).map((node) => node.localName)], [0, ['i']]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom shadow slots");
    assert!(
        ran.status.success(),
        "DOM shadow-slot test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_composed_shadow_events_retarget_at_host_boundaries() {
    let dir = build_dir("t_test_dom_shadow_event_retargeting");
    write_in(
        &dir,
        "retarget.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('composed events expose the host outside a shadow root', () => {\n\
           const host = document.createElement('section'); document.body.appendChild(host);\n\
           const root = host.attachShadow({ mode: 'open' }); root.innerHTML = '<button>inside</button>';\n\
           const button = root.querySelector('button'); const seen = [];\n\
           button.addEventListener('go', (event) => seen.push(['button', event.target.localName]));\n\
           root.addEventListener('go', (event) => seen.push(['root', event.target.localName]));\n\
           host.addEventListener('go', (event) => seen.push(['host', event.target.localName]));\n\
           document.addEventListener('go', (event) => seen.push(['document', event.target.localName]));\n\
           button.dispatchEvent(new Event('go', { bubbles: true, composed: true }));\n\
           assertEquals(seen, [['button', 'button'], ['root', 'button'], ['host', 'section'], ['document', 'section']]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom shadow event retargeting");
    assert!(
        ran.status.success(),
        "DOM shadow-event retargeting test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_deep_template_clones_preserve_inert_content() {
    let dir = build_dir("t_test_dom_template_clone");
    write_in(
        &dir,
        "template-clone.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('deep template clones copy content without populating the host', () => {\n\
           const template = document.createElement('template');\n\
           template.innerHTML = '<article data-kind=card><strong>hello</strong></article>';\n\
           const clone = template.cloneNode(true);\n\
           const imported = document.importNode(template, true);\n\
           assertEquals([template.childNodes.length, clone.childNodes.length, clone.content.childNodes.length, clone.content.firstElementChild.outerHTML, imported.content.firstElementChild.outerHTML], [0, 0, 1, '<article data-kind=\"card\"><strong>hello</strong></article>', '<article data-kind=\"card\"><strong>hello</strong></article>']);\n\
           clone.content.querySelector('strong').textContent = 'changed';\n\
           assertEquals([template.content.querySelector('strong').textContent, clone.content.querySelector('strong').textContent], ['hello', 'changed']);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom template clone");
    assert!(
        ran.status.success(),
        "DOM template-clone test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_tree_walker_filters_depth_first_nodes() {
    let dir = build_dir("t_test_dom_tree_walker");
    write_in(
        &dir,
        "walker.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
      test('TreeWalker walks accepted nodes and rejects subtrees', () => {\n\
        const root = document.createElement('div'); root.innerHTML = '<i>one</i><!--note--><b><em>two</em></b>';\n\
        const all = document.createTreeWalker(root, NodeFilter.SHOW_ELEMENT | NodeFilter.SHOW_COMMENT);\n\
        const names = []; for (let node; node = all.nextNode();) names.push(node.nodeName);\n\
        const filtered = document.createTreeWalker(root, NodeFilter.SHOW_ELEMENT, node => node.localName === 'b' ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT);\n\
        const kept = []; for (let node; node = filtered.nextNode();) kept.push(node.nodeName);\n\
        assertEquals([names, kept], [['I', '#comment', 'B', 'EM'], ['I']]);\n\
      });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM tree-walker test");
    assert!(
        ran.status.success(),
        "tree-walker test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_class_lists_track_attributes_and_validate_tokens() {
    let dir = build_dir("t_test_dom_class_lists");
    write_in(
        &dir,
        "class-list.test.mjs",
        "import { test, assertEquals, assertThrows } from 'runtime:test';\n\
         test('class list mutations preserve ordered unique tokens', () => {\n\
           const element = document.createElement('div'); element.className = 'one one two'; const classes = element.classList;\n\
           assertEquals(classes, element.classList); assertEquals([...classes], ['one', 'two']); assertEquals(classes[1], 'two'); assertEquals(classes.item(2), null);\n\
           classes.add('three', 'one'); classes.remove('two'); assertEquals(element.className, 'one three');\n\
           assertEquals(classes.replace('one', 'first'), true); assertEquals(classes.toggle('three'), false); assertEquals(classes.toggle('four', true), true);\n\
           assertEquals(element.getAttribute('class'), 'first four');\n\
         });\n\
         test('class list rejects invalid tokens without partial changes', () => {\n\
           const element = document.createElement('div'); element.className = 'ready';\n\
           assertThrows(() => element.classList.add('next', 'bad token'), DOMException); assertThrows(() => element.classList.contains(''), DOMException);\n\
           assertEquals(element.className, 'ready');\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom class lists");
    assert!(
        ran.status.success(),
        "DOM class list test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_parser_does_not_call_overridden_element_append_child() {
    let dir = build_dir("t_test_dom_parser_public_append");
    write_in(
        &dir,
        "parser-append.test.mjs",
        r#"import { test, assertEquals } from 'runtime:test';
           test('parser construction bypasses public appendChild overrides', () => {
             const appendChild = Element.prototype.appendChild;
             Element.prototype.appendChild = function() { throw new Error('public append'); };
             try {
               const template = document.createElement('template');
               template.innerHTML = '<ul><li>one</li><li>two</li></ul>';
               assertEquals(template.content.querySelectorAll('li').length, 2);
             } finally {
               Element.prototype.appendChild = appendChild;
             }
           });
"#,
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn parser public append test");
    assert!(
        ran.status.success(),
        "parser public append test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_parser_accepts_well_formed_svg_foreign_content() {
    let dir = build_dir("t_test_dom_svg_foreign_content");
    write_in(
        &dir,
        "svg.test.mjs",
        "import { test, assertEquals, assertThrows } from 'runtime:test';\n\
         test('SVG attributes and self-closing descendants parse without HTML recovery', () => {\n\
           document.body.innerHTML = '<svg viewbox=\"0 0 10 10\" gradientUnits=\"userSpaceOnUse\"><circle cx=\"5\" cy=\"5\" r=\"4\"/></svg>';\n\
           const svg = document.body.firstElementChild; const circle = svg.firstElementChild;\n\
           assertEquals(svg.getAttribute('viewBox'), '0 0 10 10'); assertEquals(svg.getAttribute('gradientUnits'), 'userSpaceOnUse'); assertEquals(circle.localName, 'circle'); assertEquals(circle.getAttribute('r'), '4');\n\
           document.body.innerHTML = '<div><svg:svg><svg:circle cx=\"1\"/></svg:svg></div>';\n\
           assertEquals(document.body.firstElementChild.firstElementChild.firstElementChild.getAttribute('cx'), '1');\n\
           assertThrows(() => { document.body.innerHTML = '<div/>'; }, SyntaxError);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom SVG foreign content");
    assert!(
        ran.status.success(),
        "DOM SVG foreign-content test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_parser_keeps_svg_foreign_object_namespaces() {
    let dir = build_dir("t_test_dom_svg_foreign_object_namespaces");
    write_in(
        &dir,
        "foreign-object.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('parsed SVG foreignObject enters and exits the HTML namespace', () => {\n\
           const createElementNS = document.createElementNS; let htmlNamespaceCalls = 0; document.createElementNS = function(namespace, name) { if (namespace === 'http://www.w3.org/1999/xhtml') htmlNamespaceCalls += 1; return createElementNS.call(this, namespace, name); };\n\
           document.body.innerHTML = '<svg><foreignObject><div id=html-child>ok</div></foreignObject><clipPath id=clip/></svg>';\n\
           document.createElementNS = createElementNS; assertEquals(htmlNamespaceCalls, 0);\n\
           const foreignObject = document.body.querySelector('foreignObject'); const htmlChild = foreignObject.querySelector('#html-child');\n\
           assertEquals([foreignObject.tagName, foreignObject.namespaceURI, htmlChild.namespaceURI], ['foreignObject', 'http://www.w3.org/2000/svg', 'http://www.w3.org/1999/xhtml']);\n\
           assertEquals(document.body.querySelector('clipPath').tagName, 'clipPath');\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom SVG foreignObject namespaces");
    assert!(
        ran.status.success(),
        "DOM SVG foreignObject namespace test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_inline_styles_track_the_style_attribute() {
    let dir = build_dir("t_test_dom_css");
    write_in(
        &dir,
        "styles.test.mjs",
        "import { test, assertEquals, assertThrows } from 'runtime:test';\n\
         test('properties and attributes stay synchronized', () => {\n\
           const element = document.createElement('div');\n\
           element.style.width = '10px';\n\
           element.style.setProperty('--tone', 'red', 'important');\n\
           assertEquals(element.getAttribute('style'), 'width: 10px; --tone: red !important;');\n\
           assertEquals(element.style.length, 2);\n\
           assertEquals(element.style[0], 'width');\n\
           assertEquals(element.style.getPropertyPriority('--tone'), 'important');\n\
           assertEquals(element.style.removeProperty('width'), '10px');\n\
           assertEquals(element.style.width, '');\n\
           element.setAttribute('style', 'background-color: rgb(1, 2, 3); padding: 1px !important;');\n\
           assertEquals(element.style.backgroundColor, 'rgb(1, 2, 3)');\n\
           assertEquals(element.style.padding, '1px');\n\
           assertEquals(element.style.getPropertyPriority('padding'), 'important');\n\
         });\n\
         test('invalid declarations fail loudly', () => {\n\
           const element = document.createElement('div');\n\
           assertThrows(() => { element.style.cssText = 'width'; }, SyntaxError);\n\
           assertThrows(() => element.style.setProperty('width', '1px', 'urgent'), SyntaxError);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom styles");
    assert!(
        ran.status.success(),
        "DOM style test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_window_selection_keeps_logical_range_boundaries() {
    let dir = build_dir("t_test_dom_selection");
    write_in(
        &dir,
        "selection.test.mjs",
        r#"import { test, assertEquals } from 'runtime:test';
           test('window and document expose the realm selection', () => {
             const text = document.createTextNode('hello'); document.body.appendChild(text);
             const range = document.createRange(); range.setStart(text, 2); range.collapse(true);
             const selection = window.getSelection(); selection.removeAllRanges(); selection.addRange(range);
             assertEquals([selection === document.getSelection(), selection.rangeCount, selection.isCollapsed, selection.getRangeAt(0).startOffset], [true, 1, true, 2]);
             selection.removeAllRanges(); assertEquals(selection.rangeCount, 0);
           });
"#,
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM selection test");
    assert!(
        ran.status.success(),
        "DOM selection test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_style_properties_and_progress_values_are_observable() {
    let dir = build_dir("t_test_dom_style_and_progress");
    write_in(
        &dir,
        "style-progress.test.mjs",
        r#"import { test, assertEquals } from 'runtime:test';
           test('style property checks and common HTML reflectors work', () => {
             const div = document.createElement('div');
             div.style.opacity = 0; div.style.animationIterationCount = 2.5;
             const progress = document.createElement('progress');
             const table = document.createElement('table');
             progress.value = 0; table.border = false;
             assertEquals(['opacity' in div.style, 'animationIterationCount' in div.style, div.style.opacity, div.style.animationIterationCount, progress instanceof HTMLProgressElement, progress.value, table.getAttribute('border')], [true, true, '0', '2.5', true, 0, 'false']);
           });
"#,
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn style and progress DOM test");
    assert!(
        ran.status.success(),
        "style and progress DOM test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_html_element_reflects_editing_and_interaction_state() {
    let dir = build_dir("t_test_dom_html_interaction_reflection");
    write_in(
        &dir,
        "interaction.test.mjs",
        r#"import { test, assertEquals } from 'runtime:test';
           test('common HTMLElement state matches browser defaults and reflection', () => {
             const div = document.createElement('div');
             const anchor = document.createElement('a'); anchor.href = '#target';
             const input = document.createElement('input');
             assertEquals([div.contentEditable, div.isContentEditable, div.translate, div.draggable, div.spellcheck, div.tabIndex, anchor.draggable, anchor.tabIndex, input.tabIndex], ['inherit', false, true, false, true, -1, true, 0, 0]);
             div.contentEditable = 'plaintext-only'; div.translate = false; div.draggable = true; div.spellcheck = false; div.tabIndex = 3;
             assertEquals([div.isContentEditable, div.getAttribute('contenteditable'), div.getAttribute('translate'), div.getAttribute('draggable'), div.getAttribute('spellcheck'), div.getAttribute('tabindex')], [true, 'plaintext-only', 'no', 'true', 'false', '3']);
             const child = document.createElement('span'); div.appendChild(child);
             assertEquals(child.isContentEditable, true);
           });
"#,
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn HTMLElement interaction reflection test");
    assert!(
        ran.status.success(),
        "HTMLElement interaction reflection test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_canvas_elements_reflect_bitmap_dimensions() {
    let dir = build_dir("t_test_dom_canvas_dimensions");
    write_in(
        &dir,
        "canvas.test.mjs",
        r#"import { test, assertEquals } from 'runtime:test';
           test('canvas dimensions are reflected integer properties', () => {
             const canvas = document.createElement('canvas');
             assertEquals(canvas instanceof HTMLCanvasElement, true);
             assertEquals([canvas.width, canvas.height], [300, 150]);
             canvas.width = 80; canvas.height = 40;
             assertEquals([canvas.getAttribute('width'), canvas.getAttribute('height'), canvas.width, canvas.height], ['80', '40', 80, 40]);
           });
"#,
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn canvas dimension reflection test");
    assert!(
        ran.status.success(),
        "canvas dimension reflection test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_custom_elements_upgrade_and_react_to_tree_changes() {
    let dir = build_dir("t_test_dom_custom_elements");
    write_in(
        &dir,
        "custom-elements.test.mjs",
        "import { test, assertEquals, assertThrows } from 'runtime:test';\n\
         test('definition upgrades existing elements and runs reactions', async () => {\n\
           const calls = [];\n\
           const prior = document.createElement('x-probe'); prior.setAttribute('state', 'ready'); document.body.appendChild(prior);\n\
           const pending = customElements.whenDefined('x-probe');\n\
           class Probe extends HTMLElement {\n\
             static observedAttributes = ['state'];\n\
             constructor() { super(); calls.push('constructed'); }\n\
             connectedCallback() { calls.push('connected'); }\n\
             disconnectedCallback() { calls.push('disconnected'); }\n\
             adoptedCallback() { calls.push('adopted'); }\n\
             attributeChangedCallback(name, oldValue, newValue) { calls.push(`${name}:${oldValue}:${newValue}`); }\n\
           }\n\
           customElements.define('x-probe', Probe);\n\
           assertEquals(await pending, Probe);\n\
           assertEquals(prior instanceof Probe, true);\n\
           assertEquals(calls, ['constructed', 'state:null:ready', 'connected']);\n\
           prior.setAttribute('state', 'next'); prior.remove();\n\
           const secondDocument = new Document(); secondDocument.adoptNode(prior);\n\
           assertEquals(calls, ['constructed', 'state:null:ready', 'connected', 'state:ready:next', 'disconnected', 'adopted']);\n\
         });\n\
         test('the registry validates names and duplicate definitions', () => {\n\
           class Good extends HTMLElement {}\n\
           customElements.define('x-good', Good);\n\
           assertEquals(document.createElement('x-good') instanceof Good, true);\n\
           assertThrows(() => customElements.define('plain', Good), DOMException);\n\
           assertThrows(() => customElements.define('x-good', Good), DOMException);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom custom elements");
    assert!(
        ran.status.success(),
        "DOM custom-element test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_connected_callbacks_do_not_repeat_for_new_descendants() {
    let dir = build_dir("t_test_dom_connected_callback_descendant");
    write_in(
        &dir,
        "connected.test.mjs",
        r#"import { test, assertEquals } from 'runtime:test';
           test('a descendant inserted by a connected callback reacts once', () => {
             let childConnections = 0;
             class Child extends HTMLElement { connectedCallback() { childConnections += 1; } }
             class Parent extends HTMLElement { connectedCallback() { this.appendChild(document.createElement('x-connected-child')); } }
             customElements.define('x-connected-child', Child); customElements.define('x-connected-parent', Parent);
             document.body.appendChild(document.createElement('x-connected-parent'));
             assertEquals(childConnections, 1);
           });
"#,
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn connected custom-element descendant test");
    assert!(
        ran.status.success(),
        "connected custom-element descendant test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_custom_element_registry_validates_and_settles_waiters() {
    let dir = build_dir("t_test_dom_custom_registry");
    write_in(
        &dir,
        "registry.test.mjs",
        "import { test, assertEquals, assertThrows, assertRejects } from 'runtime:test';\n\
         test('registry contracts', async () => {\n\
           assertEquals(customElements.get('x-later'), undefined);\n\
           assertRejects(() => customElements.whenDefined('plain'), DOMException);\n\
           assertThrows(() => customElements.define('plain', class Plain extends HTMLElement {}), DOMException);\n\
           assertThrows(() => customElements.define('x-invalid', class Invalid {}), TypeError);\n\
           const waiting = customElements.whenDefined('x-later');\n\
           class Later extends HTMLElement {}\n\
           customElements.define('x-later', Later);\n\
           assertEquals(await waiting, Later);\n\
           assertEquals(customElements.get('x-later'), Later);\n\
           assertThrows(() => customElements.define('x-later', Later), DOMException);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn custom registry test");
    assert!(
        ran.status.success(),
        "registry test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_custom_elements_upgrade_detached_and_parser_nodes() {
    let dir = build_dir("t_test_dom_custom_upgrade");
    write_in(
        &dir,
        "upgrade.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('detached and parsed upgrades', () => {\n\
           const calls = [];\n\
           const detached = document.createElement('x-detached');\n\
           class Detached extends HTMLElement { constructor() { super(); calls.push('detached constructed'); } connectedCallback() { calls.push('detached connected'); } }\n\
           customElements.define('x-detached', Detached);\n\
           assertEquals(detached instanceof Detached, false);\n\
           customElements.upgrade(detached);\n\
           assertEquals(detached instanceof Detached, true);\n\
           document.body.appendChild(detached);\n\
           class Parsed extends HTMLElement {\n\
             static observedAttributes = ['state'];\n\
             constructor() { super(); calls.push('parsed constructed'); }\n\
             connectedCallback() { calls.push('parsed connected'); }\n\
             attributeChangedCallback(name, oldValue, newValue) { calls.push(`${name}:${oldValue}:${newValue}`); }\n\
           }\n\
           customElements.define('x-parsed', Parsed);\n\
           document.body.innerHTML = '<x-parsed state=ready></x-parsed>';\n\
           assertEquals(document.body.firstChild instanceof Parsed, true);\n\
           assertEquals(calls, ['detached constructed', 'detached connected', 'parsed constructed', 'state:null:ready', 'parsed connected']);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn custom upgrade test");
    assert!(
        ran.status.success(),
        "upgrade test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_custom_element_reactions_filter_attributes_and_track_moves() {
    let dir = build_dir("t_test_dom_custom_reactions");
    write_in(
        &dir,
        "reactions.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('attribute filtering and tree reaction order', () => {\n\
           const calls = [];\n\
           class Life extends HTMLElement {\n\
             static observedAttributes = ['state'];\n\
             connectedCallback() { calls.push(`connected:${this.id}`); }\n\
             disconnectedCallback() { calls.push(`disconnected:${this.id}`); }\n\
             attributeChangedCallback(name, oldValue, newValue) { calls.push(`${name}:${oldValue}:${newValue}`); }\n\
           }\n\
           customElements.define('x-life', Life);\n\
           const one = document.createElement('x-life'); one.id = 'one';\n\
           const two = document.createElement('x-life'); two.id = 'two';\n\
           const fragment = document.createDocumentFragment(); fragment.append(one, two);\n\
           document.body.appendChild(fragment);\n\
           one.setAttribute('other', 'ignored'); one.setAttribute('state', 'one'); one.setAttribute('state', 'one'); one.removeAttribute('state');\n\
           document.body.appendChild(one); two.remove();\n\
           assertEquals(calls, ['connected:one', 'connected:two', 'state:null:one', 'state:one:null', 'disconnected:one', 'connected:one', 'disconnected:two']);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn custom reaction test");
    assert!(
        ran.status.success(),
        "reaction test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_focus_tracks_active_element_and_event_order() {
    let dir = build_dir("t_test_dom_focus_events");
    write_in(
        &dir,
        "focus-events.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('focus transfer has browser event ordering', () => {\n\
           const first = document.createElement('input'); const second = document.createElement('button');\n\
           document.body.append(first, second);\n\
           const calls = [];\n\
           for (const [element, name] of [[first, 'first'], [second, 'second']]) {\n\
             for (const type of ['blur', 'focusout', 'focus', 'focusin']) element.addEventListener(type, (event) => calls.push(`${name}:${type}:${event.relatedTarget === first ? 'first' : event.relatedTarget === second ? 'second' : 'body'}`));\n\
           }\n\
           assertEquals(document.activeElement, document.body);\n\
           first.focus(); assertEquals(document.activeElement, first);\n\
           second.focus(); assertEquals(document.activeElement, second);\n\
           assertEquals(calls, ['first:focus:body', 'first:focusin:body', 'first:blur:second', 'first:focusout:second', 'second:focus:first', 'second:focusin:first']);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM focus event test");
    assert!(
        ran.status.success(),
        "focus event test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_focusability_is_connection_and_attribute_driven() {
    let dir = build_dir("t_test_dom_focusability");
    write_in(
        &dir,
        "focusability.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('only connected enabled native or tabindex elements focus', () => {\n\
           const plain = document.createElement('div'); const indexed = document.createElement('div'); indexed.tabIndex = -1;\n\
           const disabled = document.createElement('button'); disabled.disabled = true;\n\
           const hidden = document.createElement('input'); hidden.type = 'hidden';\n\
           const link = document.createElement('a'); const detached = document.createElement('input');\n\
           document.body.append(plain, indexed, disabled, hidden, link);\n\
           plain.focus(); assertEquals(document.activeElement, document.body);\n\
           disabled.focus(); hidden.focus(); detached.focus(); assertEquals(document.activeElement, document.body);\n\
           indexed.focus(); assertEquals(document.activeElement, indexed);\n\
           link.focus(); assertEquals(document.activeElement, indexed);\n\
           link.setAttribute('href', '/target'); link.focus(); assertEquals(document.activeElement, link);\n\
           link.blur(); assertEquals(document.activeElement, document.body);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM focusability test");
    assert!(
        ran.status.success(),
        "focusability test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_focus_returns_to_body_when_the_active_subtree_is_removed() {
    let dir = build_dir("t_test_dom_focus_removal");
    write_in(
        &dir,
        "focus-removal.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('removing a focused descendant clears focus silently', () => {\n\
           const wrapper = document.createElement('section'); const input = document.createElement('input');\n\
           let events = [];\n\
           for (const type of ['blur', 'focusout', 'focus', 'focusin']) {\n\
             input.addEventListener(type, () => events.push(type));\n\
             document.body.addEventListener(type, () => events.push(`body:${type}`), true);\n\
           }\n\
           wrapper.appendChild(input); document.body.appendChild(wrapper); input.focus();\n\
           events = [];\n\
           wrapper.remove();\n\
           // A removal resets the focus and fires nothing: the element that would\n\
           // receive the event is no longer in the document. Verified upstream by\n\
           // dom/nodes/insertion-removing-steps/blur-event.window.js.\n\
           assertEquals(document.activeElement, document.body); assertEquals(events, []);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM focus removal test");
    assert!(
        ran.status.success(),
        "focus removal test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_mutation_observer_delivers_child_list_records_at_microtasks() {
    let dir = build_dir("t_test_dom_mutation_child_list");
    write_in(
        &dir,
        "child-list.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('child-list records batch until the microtask checkpoint', async () => {\n\
           const parent = document.createElement('section'); document.body.appendChild(parent);\n\
           const first = document.createElement('i'); const second = document.createElement('b');\n\
           const deliveries = []; const observer = new MutationObserver((records, same) => deliveries.push([same === observer, records]));\n\
           observer.observe(parent, { childList: true });\n\
           parent.append(first, second); parent.removeChild(first);\n\
           assertEquals(deliveries.length, 0); await Promise.resolve();\n\
           assertEquals(deliveries.length, 1);\n\
           const records = deliveries[0][1]; assertEquals(deliveries[0][0], true); assertEquals(records.length, 3);\n\
           assertEquals(records.map((record) => [record.type, record.target, record.addedNodes.item(0), record.removedNodes.item(0), record.previousSibling, record.nextSibling]), [\n\
             ['childList', parent, first, null, null, null],\n\
             ['childList', parent, second, null, first, null],\n\
             ['childList', parent, null, first, null, second],\n\
           ]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM mutation child-list test");
    assert!(
        ran.status.success(),
        "mutation child-list test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_mutation_observer_filters_attributes_and_preserves_old_values() {
    let dir = build_dir("t_test_dom_mutation_attributes");
    write_in(
        &dir,
        "attributes.test.mjs",
        "import { test, assertEquals, assertThrows } from 'runtime:test';\n\
         test('attribute filters and old values', async () => {\n\
           const element = document.createElement('div'); document.body.appendChild(element);\n\
           const observer = new MutationObserver(() => {});\n\
           observer.observe(element, { attributes: true, attributeFilter: ['state'], attributeOldValue: true });\n\
           element.setAttribute('other', 'ignored'); element.setAttribute('state', 'first'); element.setAttribute('state', 'second'); element.removeAttribute('state');\n\
           const records = observer.takeRecords();\n\
           assertEquals(records.map((record) => [record.attributeName, record.oldValue, record.attributeNamespace]), [['state', null, null], ['state', 'first', null], ['state', 'second', null]]);\n\
           assertThrows(() => observer.observe(element, {}), TypeError);\n\
           assertThrows(() => observer.observe(element, { attributes: false, attributeOldValue: true }), TypeError);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM mutation attribute test");
    assert!(
        ran.status.success(),
        "mutation attribute test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_mutation_observer_tracks_subtree_character_data_and_disconnects() {
    let dir = build_dir("t_test_dom_mutation_character_data");
    write_in(
        &dir,
        "character-data.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('subtree character data and observer lifecycle', async () => {\n\
           const parent = document.createElement('div'); const child = document.createElement('span'); const text = document.createTextNode('before');\n\
           child.appendChild(text); parent.appendChild(child); document.body.appendChild(parent);\n\
           const calls = []; const observer = new MutationObserver((records) => calls.push(records));\n\
           observer.observe(parent, { characterData: true, characterDataOldValue: true, subtree: true });\n\
           text.data = 'after'; await Promise.resolve();\n\
           assertEquals(calls.length, 1); assertEquals(calls[0].map((record) => [record.type, record.target, record.oldValue]), [['characterData', text, 'before']]);\n\
           observer.disconnect(); text.data = 'final'; await Promise.resolve();\n\
           assertEquals(calls.length, 1); assertEquals(observer.takeRecords(), []);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM mutation character-data test");
    assert!(
        ran.status.success(),
        "mutation character-data test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_mutation_observer_reschedules_changes_made_by_its_callback() {
    let dir = build_dir("t_test_dom_mutation_reschedule");
    write_in(
        &dir,
        "reschedule.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('callback changes are delivered in a later microtask', async () => {\n\
           const element = document.createElement('div'); document.body.appendChild(element); const batches = [];\n\
           const observer = new MutationObserver((records) => { batches.push(records.map((record) => record.attributeName)); if (batches.length === 1) element.setAttribute('second', 'yes'); });\n\
           observer.observe(element, { attributes: true }); element.setAttribute('first', 'yes');\n\
           await Promise.resolve(); assertEquals(batches, [['first']]);\n\
           await Promise.resolve(); assertEquals(batches, [['first'], ['second']]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM mutation reschedule test");
    assert!(
        ran.status.success(),
        "mutation reschedule test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_window_console_methods_are_writable() {
    let dir = build_dir("t_test_dom_browser_console");
    write_in(
        &dir,
        "console.test.mjs",
        r#"import { test, assertEquals } from 'runtime:test';
           test('console methods are writable own properties', () => {
             const descriptor = Object.getOwnPropertyDescriptor(console, 'warn');
             const previous = console.warn; let called = false;
             console.warn = () => { called = true; };
             console.warn('test');
             console.warn = previous;
             assertEquals([descriptor.writable, descriptor.configurable, called], [true, true, true]);
           });
"#,
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn browser console DOM test");
    assert!(
        ran.status.success(),
        "browser console DOM test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_window_location_and_history_are_in_memory() {
    let dir = build_dir("t_test_dom_history");
    write_in(
        &dir,
        "history.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('location and history', () => {\n\
           const popped = []; window.addEventListener('popstate', (event) => popped.push(event.state.page));\n\
           history.pushState({ page: 1 }, '', '/one?x=1#top');\n\
           history.pushState({ page: 2 }, '', '/two');\n\
           assertEquals(location.pathname, '/two'); assertEquals(history.length, 3); assertEquals(history.state.page, 2);\n\
           history.back(); assertEquals(location.href, 'http://localhost/one?x=1#top'); assertEquals(history.state.page, 1);\n\
           history.forward(); assertEquals(popped, [1, 2]);\n\
           history.replaceState({ page: 3 }, '', '/three');\n\
           assertEquals(location.pathname, '/three'); assertEquals(history.state.page, 3);\n\
           location.assign('/recorded'); assertEquals(location.pathname, '/recorded');\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM history test");
    assert!(
        ran.status.success(),
        "history test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_window_storage_and_navigator_are_realm_local() {
    let dir = build_dir("t_test_dom_storage");
    write_in(
        &dir,
        "storage.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('storage and navigator', () => {\n\
           localStorage.setItem('answer', 42); sessionStorage.setItem('answer', 'session');\n\
           assertEquals(localStorage.getItem('answer'), '42'); assertEquals(sessionStorage.getItem('answer'), 'session');\n\
           assertEquals(localStorage.key(0), 'answer'); assertEquals(localStorage.key(1), null);\n\
           localStorage.removeItem('answer'); sessionStorage.clear();\n\
           assertEquals(localStorage.length, 0); assertEquals(sessionStorage.length, 0);\n\
           assertEquals(navigator.userAgent, 'esdev DOM'); assertEquals(navigator.language, 'en-US'); assertEquals(navigator.languages, ['en-US']);\n\
           assertEquals(window, globalThis);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM storage test");
    assert!(
        ran.status.success(),
        "storage test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_storage_is_not_shared_by_isolated_test_files() {
    let dir = build_dir("t_test_dom_storage_isolation");
    write_in(
        &dir,
        "first.test.mjs",
        "import { test } from 'runtime:test';\n\
         test('writes storage', () => { localStorage.setItem('leak', 'no'); });\n",
    );
    write_in(
        &dir,
        "second.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('starts with clean storage', () => { assertEquals(localStorage.getItem('leak'), null); });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom", "--jobs=1"])
        .output()
        .expect("spawn DOM storage isolation test");
    assert!(
        ran.status.success(),
        "storage isolation test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_window_media_queries_read_the_viewport_and_observers_never_deliver() {
    let dir = build_dir("t_test_dom_window_stubs");
    write_in(
        &dir,
        "stubs.test.mjs",
        "import { test, assertEquals, assertThrows } from 'runtime:test';\n\
         test('computed styles and stubs', () => {\n\
           const element = document.createElement('div'); element.style.width = '10px';\n\
           // Outside the tree there is no computed style at all, as in a browser.\n\
           assertEquals(getComputedStyle(element).length, 0);\n\
           document.body.append(element);\n\
           const computed = getComputedStyle(element);\n\
           assertEquals(computed.width, '10px'); assertEquals(computed.getPropertyValue('width'), '10px');\n\
           assertThrows(() => { computed.width = '20px'; }, TypeError);\n\
           const query = matchMedia('(min-width: 1px)');\n\
           assertEquals([query.matches, query.media], [true, '(min-width: 1px)']);\n\
           assertEquals([window.innerWidth, window.innerHeight], [1024, 768]);\n\
           assertEquals(matchMedia('(min-width: 99999px)').matches, false);\n\
           window.innerWidth = 320;\n\
           assertEquals([query.matches, matchMedia('(min-width: 400px)').matches], [true, false]);\n\
           window.innerWidth = 1024;\n\
           let delivered = 0; const resize = new ResizeObserver(() => { delivered += 1; }); resize.observe(element);\n\
           const intersection = new IntersectionObserver(() => { delivered += 1; }); intersection.observe(element);\n\
           assertEquals(resize.takeRecords(), []); assertEquals(intersection.takeRecords(), []); assertEquals(delivered, 0);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM window stub test");
    assert!(
        ran.status.success(),
        "window stub test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_animation_frames_follow_the_test_clock() {
    let dir = build_dir("t_test_dom_raf");
    write_in(
        &dir,
        "raf.test.mjs",
        "import { test, assertEquals, clock } from 'runtime:test';\n\
         test('animation frames', () => {\n\
           clock.freeze(new Date('2020-01-01T00:00:00Z'));\n\
           const frames = []; const cancelled = requestAnimationFrame(() => frames.push('cancelled'));\n\
           cancelAnimationFrame(cancelled); requestAnimationFrame((at) => frames.push(at));\n\
           clock.advance(15); assertEquals(frames, []);\n\
           clock.advance(1); assertEquals(frames, [Date.now()]);\n\
           clock.release();\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM rAF test");
    assert!(
        ran.status.success(),
        "rAF test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_runs_discovered_files_and_reports_failures() {
    let dir = build_dir("t_run");
    write_in(
        &dir,
        "ok.test.mjs",
        "import { test, assert } from 'runtime:test';\n\
         test('passes', () => assert(true));\n",
    );
    write_in(
        &dir,
        "bad.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('fails', () => assertEquals(1, 2));\n",
    );
    // Not a test file: discovery must not sweep in ordinary source.
    write_in(&dir, "helper.mjs", "export const x = 1;\n");

    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    let text = format!("{}{}", stdout(&out), stderr(&out));

    assert!(!out.status.success(), "a failing suite must exit non-zero");
    assert!(text.contains("ok.test.mjs"), "{text}");
    assert!(text.contains("bad.test.mjs"), "{text}");
    assert!(!text.contains("helper.mjs"), "{text}");
    assert!(text.contains("1 of 2 files failed"), "{text}");
}

#[test]
fn a_passing_suite_exits_zero() {
    let dir = build_dir("t_pass");
    write_in(
        &dir,
        "a.test.mjs",
        "import { test, assert, assertThrows, assertRejects } from 'runtime:test';\n\
         test('sync', () => assert(true));\n\
         test('async', async () => { const v = await Promise.resolve(1); assert(v === 1); });\n\
         test('throws', () => assertThrows(() => { throw new Error('x'); }));\n\
         test('rejects', async () => await assertRejects(async () => { throw new Error('x'); }));\n",
    );
    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    assert!(stdout(&out).contains("4 passed"), "{}", stdout(&out));
}

#[test]
fn snapshots_are_versioned_explicitly_updated_and_checked() {
    let dir = build_dir("t_snapshots");
    write_in(
        &dir,
        "value.test.mjs",
        "import { test, expect } from 'runtime:test';\n\
         test('records supported values', () => {\n\
           const shared = { answer: 42 };\n\
           expect({ b: undefined, a: [BigInt(2), shared, shared], when: new Date('2020-01-01T00:00:00.000Z') }).toMatchSnapshot();\n\
         });\n",
    );

    let written = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    assert!(
        written.status.success(),
        "{}{}",
        stdout(&written),
        stderr(&written)
    );
    assert!(
        stdout(&written).contains("snapshots: 0 matched, 0 failed, 1 written"),
        "{}",
        stdout(&written)
    );

    let updated = esdev_in(&dir)
        .args(["test", "--update-snapshots"])
        .output()
        .expect("spawn esdev test --update-snapshots");
    assert!(
        updated.status.success(),
        "{}{}",
        stdout(&updated),
        stderr(&updated)
    );
    let snapshot = std::fs::read_to_string(dir.join("__snapshots__/value.test.mjs.snap"))
        .expect("read snapshot");
    assert!(snapshot.contains("// esdev snapshot v1"), "{snapshot}");
    assert!(
        snapshot.contains("=== records supported values: snapshot 1 [value]"),
        "{snapshot}"
    );
    assert!(snapshot.contains("[Circular]"), "{snapshot}");

    let checked = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    assert!(
        checked.status.success(),
        "{}{}",
        stdout(&checked),
        stderr(&checked)
    );

    write_in(
        &dir,
        "value.test.mjs",
        "import { test, expect } from 'runtime:test';\n\
         test('records supported values', () => expect({ answer: false }).toMatchSnapshot());\n",
    );
    let changed = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    let text = stdout(&changed);
    assert!(!changed.status.success(), "a changed snapshot must fail");
    assert!(text.contains("--- snapshot"), "{text}");
    assert!(text.contains("+++ received"), "{text}");
    assert!(text.contains("- "), "{text}");
    assert!(text.contains("+ "), "{text}");

    let ci_changed = esdev_in(&dir)
        .arg("test")
        .env("CI", "1")
        .output()
        .expect("run changed snapshot in CI");
    let ci_text = stdout(&ci_changed);
    assert!(
        !ci_changed.status.success(),
        "changed CI snapshot must fail"
    );
    assert!(ci_text.contains("snapshot changed"), "{ci_text}");
    assert!(
        !ci_text.contains("accept with:"),
        "CI must not suggest accepting a snapshot:\n{ci_text}"
    );
}

#[test]
fn ci_never_creates_a_missing_snapshot() {
    let dir = build_dir("t_snapshot_ci");
    write_in(
        &dir,
        "ci.test.mjs",
        "import { test, expect } from 'runtime:test';\n\
         test('requires a committed snapshot', () => expect({ ok: true }).toMatchSnapshot());\n",
    );
    let out = esdev_in(&dir)
        .arg("test")
        .arg("--ci")
        .output()
        .expect("spawn esdev test --ci");
    let text = stdout(&out);
    assert!(!out.status.success(), "CI must reject missing snapshots");
    assert!(text.contains("--ci does not write them"), "{text}");
    assert!(
        !dir.join("__snapshots__/ci.test.mjs.snap").exists(),
        "CI wrote a snapshot:\n{text}"
    );
}

#[test]
fn snapshots_print_stable_values_errors_and_files() {
    let dir = build_dir("t_snapshot_printer");
    write_in(
        &dir,
        "printer.test.mjs",
        "import { test, expect } from 'runtime:test';\n\
         test('prints values', () => {\n\
           const cycle = {}; cycle.self = cycle;\n\
           expect({ text: 'one\\ntwo', big: 9n, bytes: new Uint8Array([3, 1]), map: new Map([['z', 1], ['a', 2]]), set: new Set(['z', 'a']), objectMap: new Map([[{ z: 1 }, 'z'], [{ a: 1 }, 'a']]), cycle }).toMatchSnapshot();\n\
           expect('<main>ada</main>\\n').toMatchFileSnapshot('home.html');\n\
           expect(new Uint8Array([0, 255, 3])).toMatchFileSnapshot('chart.bin');\n\
         });\n\
         test('prints errors', () => expect(() => { const error = new Error('broken', { cause: 'network' }); error.code = 'E_BROKEN'; throw error; }).toThrowErrorMatchingSnapshot());\n",
    );
    let first = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    assert!(
        first.status.success(),
        "{}{}",
        stdout(&first),
        stderr(&first)
    );
    let snapshot = std::fs::read_to_string(dir.join("__snapshots__/printer.test.mjs.snap"))
        .expect("read snapshot");
    assert!(snapshot.contains("9n"), "{snapshot}");
    assert!(snapshot.contains("Uint8Array [3, 1]"), "{snapshot}");
    assert!(snapshot.contains("Map {\n    \"a\" => 2,"), "{snapshot}");
    assert!(
        snapshot.contains("\"objectMap\": Map {\n    {\n      \"a\": 1,"),
        "{snapshot}"
    );
    assert!(snapshot.contains("Set {\n    \"a\","), "{snapshot}");
    assert!(snapshot.contains("[Circular]"), "{snapshot}");
    assert!(snapshot.contains("[error]"), "{snapshot}");
    assert!(snapshot.contains("E_BROKEN"), "{snapshot}");
    assert!(snapshot.contains("\"cause\": \"network\""), "{snapshot}");
    assert_eq!(
        std::fs::read(dir.join("__snapshots__/printer.test.mjs/chart.bin")).expect("read bytes"),
        vec![0, 255, 3]
    );
    let second = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("rerun esdev test");
    assert!(
        second.status.success(),
        "{}{}",
        stdout(&second),
        stderr(&second)
    );
    assert!(
        stdout(&second).contains("snapshots: 4 matched"),
        "{}",
        stdout(&second)
    );
}

#[test]
fn a_snapshot_property_matcher_fails_before_snapshot_comparison() {
    let dir = build_dir("t_snapshot_matcher_failure");
    write_in(
        &dir,
        "matcher.test.mjs",
        "import { test, expect } from 'runtime:test';\n\
         test('checks volatile fields', () => expect({ id: 1 }).toMatchSnapshot({ id: expect.any(String) }));\n",
    );
    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    let text = stdout(&out);
    assert!(!out.status.success(), "a bad matcher must fail");
    assert!(
        text.contains("snapshot value did not satisfy its property matchers"),
        "{text}"
    );
    assert!(
        !dir.join("__snapshots__/matcher.test.mjs.snap").exists(),
        "{text}"
    );
}

#[test]
fn snapshots_number_hints_and_honor_the_ci_environment() {
    let dir = build_dir("t_snapshot_keys");
    write_in(
        &dir,
        "keys.test.mjs",
        "import { describe, test, assertSnapshot } from 'runtime:test';\n\
         describe('outer', () => describe('inner', () => test('keys', () => {\n\
           assertSnapshot({ first: true }, 'after lowering');\n\
           assertSnapshot({ second: true }, 'after lowering');\n\
         })));\n",
    );
    let first = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("write snapshots");
    assert!(
        first.status.success(),
        "{}{}",
        stdout(&first),
        stderr(&first)
    );
    let snapshot = std::fs::read_to_string(dir.join("__snapshots__/keys.test.mjs.snap"))
        .expect("read snapshot");
    assert!(
        snapshot.contains("outer > inner > keys: after lowering 1"),
        "{snapshot}"
    );
    assert!(
        snapshot.contains("outer > inner > keys: after lowering 2"),
        "{snapshot}"
    );
    assert!(
        stdout(&first).contains("written:\n  "),
        "{}",
        stdout(&first)
    );

    write_in(
        &dir,
        "missing.test.mjs",
        "import { test, expect } from 'runtime:test'; test('CI', () => expect(1).toMatchSnapshot());\n",
    );
    let ci = esdev_in(&dir)
        .arg("test")
        .env("CI", "1")
        .output()
        .expect("run CI test");
    assert!(!ci.status.success(), "CI must reject a missing snapshot");
    assert!(
        stdout(&ci).contains("--ci does not write them"),
        "{}",
        stdout(&ci)
    );
    assert!(!dir.join("__snapshots__/missing.test.mjs.snap").exists());
}

#[test]
fn file_snapshot_diffs_and_filtered_updates_keep_other_entries() {
    let dir = build_dir("t_snapshot_file_diff");
    write_in(
        &dir,
        "files.test.mjs",
        "import { test, expect } from 'runtime:test';\n\
         test('text', () => expect('before\\n').toMatchFileSnapshot('page.html'));\n\
         test('binary', () => expect(new Uint8Array([1, 2, 3])).toMatchFileSnapshot('bytes.bin'));\n\
         test('kept', () => expect({ kept: true }).toMatchSnapshot());\n",
    );
    let initial = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("write snapshots");
    assert!(
        initial.status.success(),
        "{}{}",
        stdout(&initial),
        stderr(&initial)
    );
    write_in(
        &dir,
        "files.test.mjs",
        "import { test, expect } from 'runtime:test';\n\
         test('text', () => expect('after\\n').toMatchFileSnapshot('page.html'));\n\
         test('binary', () => expect(new Uint8Array([1, 9, 3])).toMatchFileSnapshot('bytes.bin'));\n",
    );
    let changed = esdev_in(&dir).arg("test").output().expect("diff snapshots");
    let text = stdout(&changed);
    assert!(!changed.status.success(), "file changes must fail");
    assert!(text.contains("file snapshot differs:"), "{text}");
    assert!(text.contains("--- snapshot"), "{text}");
    assert!(text.contains("first differing byte 1"), "{text}");
    assert!(text.contains("--file=files.test.mjs"), "{text}");

    let filtered = esdev_in(&dir)
        .args(["test", "--update-snapshots", "files.test.mjs"])
        .output()
        .expect("update filtered test");
    assert!(
        filtered.status.success(),
        "{}{}",
        stdout(&filtered),
        stderr(&filtered)
    );
    let values = std::fs::read_to_string(dir.join("__snapshots__/files.test.mjs.snap"))
        .expect("read retained snapshot");
    assert!(
        values.contains("kept: snapshot 1"),
        "filtered update pruned an entry:\n{values}"
    );

    let pruned = esdev_in(&dir)
        .args(["test", "--update-snapshots"])
        .output()
        .expect("prune complete test file");
    assert!(
        pruned.status.success(),
        "{}{}",
        stdout(&pruned),
        stderr(&pruned)
    );
    assert!(stdout(&pruned).contains("1 removed"), "{}", stdout(&pruned));
    let values = std::fs::read_to_string(dir.join("__snapshots__/files.test.mjs.snap"))
        .expect("read pruned snapshot");
    assert!(!values.contains("kept: snapshot 1"), "{values}");

    write_in(
        &dir,
        "files.test.mjs",
        "import { test } from 'runtime:test'; test('no snapshots remain', () => {});\n",
    );
    let remove_files = esdev_in(&dir)
        .args(["test", "--update-snapshots"])
        .output()
        .expect("remove orphaned file snapshots");
    assert!(
        remove_files.status.success(),
        "{}{}",
        stdout(&remove_files),
        stderr(&remove_files)
    );
    assert!(
        stdout(&remove_files).contains("2 removed"),
        "{}",
        stdout(&remove_files)
    );
    assert!(!dir.join("__snapshots__/files.test.mjs/page.html").exists());
    assert!(!dir.join("__snapshots__/files.test.mjs/bytes.bin").exists());
}

/// A `.test.ts` file is the ordinary case: it must be stripped like any other,
/// and its relative imports must resolve from its own directory.
#[test]
fn a_typescript_test_file_runs_with_its_imports() {
    let dir = build_dir("t_ts");
    write_in(
        &dir,
        "math.ts",
        "export const add = (a: number, b: number): number => a + b;\n",
    );
    write_in(
        &dir,
        "math.test.ts",
        "import { test, assertEquals } from 'runtime:test';\n\
         import { add } from './math.ts';\n\
         interface Case { a: number; b: number; want: number }\n\
         test('adds', () => {\n\
         \x20 const c: Case = { a: 2, b: 3, want: 5 };\n\
         \x20 assertEquals(add(c.a, c.b), c.want);\n\
         });\n",
    );
    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    assert!(stdout(&out).contains("1 passed"), "{}", stdout(&out));
}

/// `expect` is a second spelling of the assertions, not a second
/// implementation — so this drives the whole vocabulary through one file and
/// checks the tally, rather than trusting that a matcher nobody ran works.
///
/// Every matcher appears, in both directions where negation means anything,
/// because a matcher that forgot to consult `not` passes its positive test
/// perfectly.
#[test]
fn the_expect_vocabulary_holds_in_both_directions() {
    let dir = build_dir("t_expect");
    write_in(
        &dir,
        "expect.test.ts",
        r#"
import { test, expect } from "runtime:test";

test("values", () => {
  expect(2 + 2).toBe(4);
  expect({ a: [1, { b: 2 }] }).toEqual({ a: [1, { b: 2 }] });
  expect({ a: 1 }).toStrictEqual({ a: 1 });
  expect(1).not.toBe(2);
  expect("").toBeFalsy();
  expect("x").toBeTruthy();
  expect(null).toBeNull();
  expect(undefined).toBeUndefined();
  expect(0).toBeDefined();
  expect(Number.NaN).toBeNaN();
  expect(new Error("x")).toBeInstanceOf(Error);
  expect("x").toBeTypeOf("string");
  expect(0.1 + 0.2).toBeCloseTo(0.3);
  expect(3).toBeGreaterThan(2);
  expect(3).toBeGreaterThanOrEqual(3);
  expect(2).toBeLessThan(3);
  expect(2).toBeLessThanOrEqual(2);
});

test("collections and text", () => {
  expect([1, 2, 3]).toContain(2);
  expect([{ a: 1 }]).toContainEqual({ a: 1 });
  expect([1, 2]).toHaveLength(2);
  expect("hello").toHaveLength(5);
  expect({ a: { b: 1 } }).toHaveProperty("a.b", 1);
  expect("hello").toMatch(/ell/);
  expect("hello").toMatch("ell");
  expect({ a: 1, b: 2 }).toMatchObject({ a: 1 });
  expect([1, 2]).not.toContain(9);
});

test("throwing, and the alias", () => {
  const boom = () => {
    throw new TypeError("bad input");
  };
  expect(boom).toThrow();
  expect(boom).toThrow("bad input");
  expect(boom).toThrow(/bad/);
  expect(boom).toThrow(TypeError);
  expect(boom).toThrowError("bad input");
  expect(() => {}).not.toThrow();
});

test("promises", async () => {
  await expect(Promise.resolve(7)).resolves.toBe(7);
  await expect(Promise.reject(new Error("no"))).rejects.toThrow("no");
  await expect(Promise.resolve(7)).resolves.not.toBe(8);
});

test("asymmetric matchers, including nested", () => {
  expect({ id: 1, name: "a", at: new Date() }).toEqual({
    id: expect.any(Number),
    name: expect.stringContaining("a"),
    at: expect.anything(),
  });
  expect([1, 2, 3]).toEqual(expect.arrayContaining([3, 1]));
  expect({ a: 1, b: 2 }).toEqual(expect.objectContaining({ a: 1 }));
  expect("hello").toEqual(expect.stringMatching(/^he/));
});

// A matcher that does not fail when it should is the failure this whole file
// is about, so each direction is provoked once.
test("a matcher that should fail, does", () => {
  const refused = (fn: () => void) => {
    let threw = false;
    try {
      fn();
    } catch {
      threw = true;
    }
    if (!threw) throw new Error("that assertion should have failed");
  };
  refused(() => expect(1).toBe(2));
  refused(() => expect(1).not.toBe(1));
  refused(() => expect([1]).toHaveLength(2));
  refused(() => expect("a").toMatch(/b/));
  refused(() => expect(() => {}).toThrow());
});
"#,
    );
    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(out.status.success(), "{text}");
    assert!(text.contains("6 passed, 0 failed"), "{text}");
}

/// `mock` and `clock` — the two subsystems that stand in for something real.
///
/// The clock is the half worth driving end to end: it replaces `setTimeout`,
/// `setInterval` and `Date` on `globalThis` for the whole process, and the
/// property that makes that safe is that a test file is a process. Freezing it
/// here and never releasing it in the last case is deliberate — the runner
/// drains on microtasks, so it must still print a tally.
#[test]
fn mocks_record_calls_and_the_clock_moves_only_when_told() {
    let dir = build_dir("t_mock");
    write_in(
        &dir,
        "mock.test.ts",
        r#"
import { test, expect, mock, clock } from "runtime:test";

test("a mock records what it was asked", () => {
  const add = mock.fn((a: number, b: number) => a + b);
  expect(add(1, 2)).toBe(3);
  add(4, 5);
  expect(add).toHaveBeenCalled();
  expect(add).toHaveBeenCalledTimes(2);
  expect(add).toHaveBeenCalledWith(1, 2);
  expect(add).toHaveBeenLastCalledWith(4, 5);
  expect(add).toHaveBeenNthCalledWith(1, 1, 2);
  expect(add).toHaveReturnedWith(9);
  expect(add).toHaveReturnedTimes(2);
  expect(add).not.toHaveBeenCalledWith(9, 9);
  // The short spellings are the same matchers.
  expect(add).toBeCalledTimes(2);
  expect(add).lastCalledWith(4, 5);
  expect(add.mock.calls).toEqual([[1, 2], [4, 5]]);
  const once = mock.fn(); once();
  expect(once).toHaveBeenCalledOnce();
});

test("it answers however it was told, once and then always", () => {
  const f = mock.fn().mockReturnValueOnce("first").mockReturnValue("rest");
  expect([f(), f(), f()]).toEqual(["first", "rest", "rest"]);
  const g = mock.fn().mockResolvedValue("ok");
  expect(g()).resolves;
  const boom = mock.fn(() => {
    throw new Error("boom");
  });
  expect(boom).toThrow("boom");
  expect(boom.mock.results[0].type).toBe("throw");
});

test("clear, reset, restore", () => {
  const f = mock.fn(() => 1).mockReturnValue(2);
  f();
  f.mockClear();
  expect(f).not.toHaveBeenCalled();
  f.mockReset();
  expect(f()).toBe(1);
});

test("a call matcher refuses anything that is not a mock", () => {
  expect(() => expect(() => {}).toHaveBeenCalled()).toThrow("needs a mock");
});

test("the failure names the mock", () => {
  const send = mock.fn().mockName("send");
  expect(() => expect(send).toHaveBeenCalled()).toThrow("send");
});

test("a spy calls through, and gives the method back", () => {
  const object = { double: (n: number) => n * 2 };
  const spy = mock.spyOn(object, "double");
  expect(object.double(4)).toBe(8);
  expect(spy).toHaveBeenCalledWith(4);
  spy.mockRestore();
  expect(mock.is(object.double)).toBe(false);
  expect(object.double(4)).toBe(8);
});

test("an inherited method goes back to the prototype", () => {
  class Greeter {
    hello() {
      return "hi";
    }
  }
  const greeter = new Greeter();
  const spy = mock.spyOn(greeter, "hello");
  expect(greeter.hello()).toBe("hi");
  spy.mockRestore();
  expect(Object.hasOwn(greeter, "hello")).toBe(false);
});

test("accessor spies call through and restore inherited descriptors", () => {
  class Meter {
    #value = 2;
    get value() { return this.#value; }
    set value(next: number) { this.#value = next; }
  }
  const meter = new Meter();
  const read = mock.spyOn(meter, "value", "get");
  expect(meter.value).toBe(2);
  expect(read).toHaveBeenCalledOnce();
  read.mockRestore();
  expect(Object.hasOwn(meter, "value")).toBe(false);
  const write = mock.spyOn(meter, "value", "set");
  meter.value = 7;
  expect(write).toHaveBeenCalledWith(7);
  write.mockRestore();
  expect(Object.hasOwn(meter, "value")).toBe(false);
  expect(meter.value).toBe(7);
});

test("a replaced global is put back by restoreAll", () => {
  const before = globalThis.fetch;
  mock.global("fetch", mock.fn());
  expect(mock.is(globalThis.fetch)).toBe(true);
  mock.restoreAll();
  expect(globalThis.fetch).toBe(before);
});

test("nothing fires until the clock is moved", () => {
  clock.freeze();
  const ran = mock.fn();
  setTimeout(ran, 100);
  expect(clock.pending()).toBe(1);
  clock.advance(99);
  expect(ran).not.toHaveBeenCalled();
  clock.advance(1);
  expect(ran).toHaveBeenCalledTimes(1);
  clock.release();
});

test("an interval repeats, and clearing it from inside stops it", () => {
  clock.freeze();
  let seen = 0;
  const id = setInterval(() => {
    seen += 1;
    if (seen === 3) clearInterval(id);
  }, 10);
  clock.advance(100);
  expect(seen).toBe(3);
  expect(clock.pending()).toBe(0);
  clock.release();
});

test("Date moves with the clock", () => {
  clock.freeze(new Date("2020-01-01T00:00:00Z"));
  expect(Date.now()).toBe(1577836800000);
  expect(new Date().toISOString()).toBe("2020-01-01T00:00:00.000Z");
  clock.advance(1000);
  expect(Date.now()).toBe(1577836801000);
  clock.release();
});

// The distinction the async form exists for.
test("the async form resumes a chain of awaits; the sync form does not", async () => {
  clock.freeze();
  const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
  const twice = () => {
    let done = false;
    void (async () => {
      await sleep(50);
      await sleep(50);
      done = true;
    })();
    return () => done;
  };

  const synchronous = twice();
  clock.advance(100);
  expect(synchronous()).toBe(false);

  const asynchronous = twice();
  await clock.advanceAsync(100);
  expect(asynchronous()).toBe(true);
  clock.release();
});

test("runAll drains, and refuses a queue that never will", () => {
  clock.freeze();
  const ran = mock.fn();
  setTimeout(() => setTimeout(ran, 10), 10);
  clock.runAll();
  expect(ran).toHaveBeenCalled();
  setInterval(() => {}, 1);
  expect(() => clock.runAll()).toThrow("not draining");
  clock.clear();
  clock.release();
});

test("runPending fires an interval once rather than for ever", () => {
  clock.freeze();
  const tick = mock.fn();
  setInterval(tick, 10);
  clock.runPending();
  expect(tick).toHaveBeenCalledTimes(1);
  clock.release();
});

test("advancing a clock that is not frozen says which verb to call", () => {
  expect(() => clock.advance(1)).toThrow("clock.freeze()");
});

// Left frozen on purpose: the runner drains on microtasks, so a file that
// forgets to release the clock must still report.
test("a file may end with time stopped", () => {
  clock.freeze();
  expect(clock.isFrozen()).toBe(true);
});
"#,
    );
    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(out.status.success(), "{text}");
    assert!(text.contains("17 passed, 0 failed"), "{text}");
}

/// The vocabulary a suite written elsewhere reaches for: `it`/`suite`, a table,
/// a to-do, and the conditional forms.
///
/// Driven end to end rather than unit-tested, because what matters is the
/// **tally** — a table that registered one case instead of three, or a to-do
/// that vanished rather than being counted as skipped, both look like a passing
/// file from the inside.
#[test]
fn the_table_and_alias_forms_register_what_they_say() {
    let dir = build_dir("t_vocab");
    write_in(
        &dir,
        "vocab.test.ts",
        r#"
import { it, suite, test, describe, expect } from "runtime:test";

suite("aliases", () => {
  it("it is test", () => expect(1).toBe(1));
});

test.each([
  [1, 1, 2],
  [2, 3, 5],
])("adds %d + %d = %d", (a: number, b: number, want: number) => {
  expect(a + b).toBe(want);
});

test.each([{ name: "ada" }, { name: "alan" }])("$name is named", (row: { name: string }) => {
  expect(row.name.length).toBeGreaterThan(0);
});

// Rows whose name does not vary still get distinct identities.
test.each([1, 2, 3])("same name", (n: number) => expect(n).toBeGreaterThan(0));

describe.each([["a"], ["b"]])("group %s", (letter: string) => {
  test("has a letter", () => expect(letter).toHaveLength(1));
});

test.todo("written later");
test.skipIf(true)("not here", () => expect(1).toBe(2));
test.runIf(true)("but here", () => expect(1).toBe(1));
"#,
    );
    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(out.status.success(), "{text}");
    // 1 alias + 2 table + 2 object table + 3 same-name + 2 groups + 1 runIf.
    assert!(text.contains("11 passed"), "{text}");
    // The to-do and the skipIf, counted rather than missing from the report.
    assert!(text.contains("2 skipped"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// `--setup` imports a module before the file, and **does not move a line**.
///
/// The second half is the one that could regress silently. A prelude prepended
/// to the *source* goes through the printer, which lays a statement on a line
/// of its own — so the file runs one line lower than it was written, and every
/// frame in every failure points one line off (D71). It is applied to the
/// printer's output instead.
#[test]
fn a_setup_module_runs_first_and_costs_no_line_numbers() {
    let dir = build_dir("t_setup");
    write_in(
        &dir,
        "setup.ts",
        "(globalThis as unknown as { __ready: string }).__ready = \"yes\";\n",
    );
    write_in(
        &dir,
        "s.test.ts",
        r#"import { test, expect } from "runtime:test";
test("the setup ran first", () => {
  expect((globalThis as unknown as { __ready?: string }).__ready).toBe("yes");
});
test("and the line is the one written", () => {
  const err = new Error("x");
  expect(String(err.stack)).toContain("s.test.ts:6");
});
"#,
    );

    // Without it the first case fails: nothing put the global there.
    let bare = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    assert!(!bare.status.success(), "the setup was not needed");

    let out = esdev_in(&dir)
        .args(["test", "--setup=./setup.ts"])
        .output()
        .expect("spawn esdev test");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(out.status.success(), "{text}");
    assert!(text.contains("2 passed"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A file that never finishes is **ended**, not waited on.
///
/// The failure this pins is subtle: cancelling the future that waits on a child
/// does not end the child. The first version of the budget did exactly that and
/// left the process running, holding the inherited stdout — so the run appeared
/// to hang after the timeout had already fired.
#[test]
fn a_file_that_runs_too_long_is_stopped_and_failed() {
    let dir = build_dir("t_timeout");
    write_in(
        &dir,
        "hang.test.mjs",
        "import { test } from 'runtime:test';\n\
         setInterval(() => {}, 1000);\n\
         test('passes, but the file never exits', () => {});\n",
    );
    for jobs in ["--jobs=1", "--jobs=2"] {
        let started = std::time::Instant::now();
        let out = esdev_in(&dir)
            .args(["test", "--timeout=1500", jobs])
            .output()
            .expect("spawn esdev test");
        let text = format!("{}{}", stdout(&out), stderr(&out));
        assert!(!out.status.success(), "{jobs}: {text}");
        assert!(text.contains("longer than 1500ms"), "{jobs}: {text}");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(20),
            "{jobs}: the child outlived its budget — it was abandoned, not ended"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// `--reporter=json` is one object per line, and every line parses.
#[test]
fn the_json_reporter_emits_one_object_per_line() {
    let dir = build_dir("t_reporter");
    write_in(
        &dir,
        "ok.test.mjs",
        "import { test, expect } from 'runtime:test';\n\
         test('passes', () => expect(1).toBe(1));\n\
         test.skip('skipped', () => {});\n",
    );
    write_in(
        &dir,
        "bad.test.mjs",
        "import { test, expect } from 'runtime:test';\n\
         test('fails \"quoted\"', () => expect(1).toBe(2));\n",
    );
    let out = esdev_in(&dir)
        .args(["test", "--reporter=json"])
        .output()
        .expect("spawn esdev test");
    assert!(!out.status.success());
    let text = stdout(&out);
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    assert!(lines.len() >= 4, "{lines:?}");
    for line in &lines {
        // A quoted test name and a multi-line stack both have to survive.
        assert!(
            serde_json::from_str::<serde_json::Value>(line).is_ok(),
            "not JSON: {line}"
        );
    }
    let last = lines.last().expect("a summary");
    assert!(last.contains("\"type\":\"summary\""), "{last}");
    assert!(last.contains("\"failed\":1"), "{last}");
    // No human chrome: a filename printed above the objects would have to be
    // skipped by whatever is parsing them.
    assert!(!text.contains("files passed"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The `test` section says the same things the flags say, and a flag wins.
#[test]
fn the_test_section_configures_the_run_and_a_flag_beats_it() {
    let dir = build_dir("t_testcfg");
    write_in(
        &dir,
        "setup.ts",
        "(globalThis as unknown as { __cfg: boolean }).__cfg = true;\n",
    );
    write_in(
        &dir,
        "c.test.ts",
        r#"import { test, expect } from "runtime:test";
test("the file's setup ran", () => {
  expect((globalThis as unknown as { __cfg?: boolean }).__cfg).toBe(true);
});
"#,
    );
    write_in(
        &dir,
        "esdev.json",
        r#"{ "targets": { "app": { "entry": "c.test.ts", "out": "dist/app.js" } },
             "test": { "setup": "./setup.ts", "reporter": "json", "jobs": 1 } }"#,
    );

    let from_file = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    assert!(
        from_file.status.success(),
        "{}{}",
        stdout(&from_file),
        stderr(&from_file)
    );
    assert!(
        stdout(&from_file).contains("\"type\":\"summary\""),
        "the file's reporter was ignored:\n{}",
        stdout(&from_file)
    );

    let overridden = esdev_in(&dir)
        .args(["test", "--reporter=human"])
        .output()
        .expect("spawn esdev test");
    assert!(overridden.status.success(), "{}", stderr(&overridden));
    assert!(
        stdout(&overridden).contains("1 file passed"),
        "a flag did not beat the file:\n{}",
        stdout(&overridden)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The property that makes a failure actionable: the frame names the line the
/// developer wrote. It used to be the interesting test, because a harness was
/// prepended to the file and had to be folded onto one line to avoid moving
/// every line number. Nothing is injected now — the test API is imported — so
/// this asserts the property still holds with the mechanism gone.
#[test]
fn a_failure_names_the_line_the_developer_wrote() {
    let dir = build_dir("t_lines");
    write_in(
        &dir,
        "lines.test.mjs",
        "import { test, assert } from 'runtime:test';\n\
         test('fails on line four', () => {\n  const x = 1;\n  assert(x === 2, 'nope');\n});\n",
    );
    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(!out.status.success());
    assert!(
        text.contains("lines.test.mjs:4:"),
        "the failing line was renumbered:\n{text}"
    );
}

/// The `.ts` counterpart, and the one that has always been at risk: a typed
/// file goes through oxc's codegen, which re-prints it. The old harness was
/// prepended *before* that step and came back out unfolded, reporting line 44
/// for an assertion on line 3 — a bug the `.mjs` sibling above could never
/// catch, because `.mjs` never reaches the printer. There is no harness to
/// unfold now; what is left under test is the stripper itself.
#[test]
fn a_typescript_failure_names_the_line_the_developer_wrote() {
    let dir = build_dir("t_lines_ts");
    write_in(
        &dir,
        "lines.test.ts",
        "import { test, assert } from 'runtime:test';\n\
         test('fails on line four', () => {\n  const x: number = 1;\n  assert(x === 2, 'nope');\n});\n",
    );
    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(!out.status.success());
    assert!(
        text.contains("lines.test.ts:4:"),
        "the typed file was renumbered:\n{text}"
    );
}

/// `assertEquals` walks the values rather than stringifying them. The `BigInt`
/// case is the one that forced this: `JSON.stringify` throws on one, so on a
/// runtime with int64 the assertion could not be written at all.
#[test]
fn assert_equals_compares_structurally() {
    let dir = build_dir("t_deep_equal");
    write_in(
        &dir,
        "eq.test.mjs",
        r#"
import { test, assert, assertEquals } from "runtime:test";

const no = (fn, label) => {
  let threw = false;
  try { fn(); } catch { threw = true; }
  assert(threw, label + ": should have failed");
};
test('holds', () => {
  assertEquals(-9223372036854775808n, -9223372036854775808n);
  assertEquals({ id: 1n }, { id: 1n });
  assertEquals(new Uint8Array([1, 2, 3]), new Uint8Array([1, 2, 3]));
  assertEquals(new Uint8Array([9, 1, 2]).subarray(1), new Uint8Array([1, 2]));
  assertEquals(NaN, NaN);
  assertEquals({ a: 1, b: 2 }, { b: 2, a: 1 });
  assertEquals(new Map([['k', 1n]]), new Map([['k', 1n]]));
  assertEquals(new Set([1, 2]), new Set([2, 1]));
  const cyclic = { name: 'x' }; cyclic.self = cyclic;
  const twin = { name: 'x' }; twin.self = twin;
  assertEquals(cyclic, twin);

  no(() => assertEquals(1n, 2n), 'unequal bigint');
  no(() => assertEquals(1n, 1), 'bigint vs number');
  no(() => assertEquals(new Uint8Array([1]), new Uint8Array([2])), 'bytes');
  no(() => assertEquals(new Uint8Array([1]), new Int8Array([1])), 'view type');
  no(() => assertEquals({ a: 1 }, { a: 1, b: 2 }), 'extra key');
  no(() => assertEquals(new Map([['k', 1]]), new Map([['k', 2]])), 'map value');
});
"#,
    );
    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
}

/// The second argument is the expectation, not a label. It used to be the
/// latter, which meant `assertThrows(fn, "TypeError")` asserted nothing at all.
#[test]
fn assert_throws_checks_the_error_it_was_given() {
    let dir = build_dir("t_throws");
    write_in(
        &dir,
        "throws.test.mjs",
        r#"
import { test, assert, assertThrows, assertRejects } from "runtime:test";

const no = (fn, label) => {
  let threw = false;
  try { fn(); } catch { threw = true; }
  assert(threw, label + ": should have failed");
};
const boom = () => { throw new TypeError('field number 0 is not allowed'); };
test('holds', () => {
  assertThrows(boom);
  assertThrows(boom, 'TypeError');
  assertThrows(boom, 'field number 0');
  assertThrows(boom, /number 0 is not/);
  assertThrows(boom, TypeError);

  no(() => assertThrows(boom, 'RangeError'), 'wrong name');
  no(() => assertThrows(boom, /depth/), 'wrong pattern');
  no(() => assertThrows(boom, RangeError), 'wrong constructor');
  no(() => assertThrows(() => 1), 'never threw');
  no(() => assertThrows(() => 1, 'TypeError'), 'never threw, name wanted');
});
test('async', async () => {
  const rejects = async () => { throw new RangeError('depth limit'); };
  await assertRejects(rejects);
  await assertRejects(rejects, 'RangeError');
  await assertRejects(rejects, /depth/);
  await assertRejects(rejects, RangeError);

  let threw = false;
  try { await assertRejects(rejects, 'TypeError'); } catch { threw = true; }
  assert(threw, 'a wrong name should have failed');
  threw = false;
  try { await assertRejects(async () => 1); } catch { threw = true; }
  assert(threw, 'a resolving promise should have failed');
});
"#,
    );
    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
}

/// What the globals could not do: a **helper module** beside the test file can
/// use the assertions. The harness was injected into the entry only, so a
/// shared `test-helpers.ts` — the one place a suite most wants to share code —
/// had no `assertEquals` to call.
#[test]
fn a_helper_module_can_import_the_assertions() {
    let dir = build_dir("t_helper");
    write_in(
        &dir,
        "helper.mjs",
        "import { assertEquals } from 'runtime:test';\n\
         export const assertSorted = (xs) => assertEquals(xs, [...xs].sort());\n",
    );
    write_in(
        &dir,
        "use.test.mjs",
        "import { test } from 'runtime:test';\n\
         import { assertSorted } from './helper.mjs';\n\
         test('a helper asserts', () => assertSorted([1, 2, 3]));\n",
    );

    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    assert!(stdout(&out).contains("1 passed"), "{}", stdout(&out));
}

/// A test that never settles is a **failure**, not a hang and not an omission.
/// The old epilogue awaited every pending promise, so this file hung forever;
/// with the tally in the host, the case is simply never finished, and a run
/// that reported "1 passed" and exited zero would be lying about the other one.
#[test]
fn a_test_that_never_finishes_fails_the_run() {
    let dir = build_dir("t_unfinished");
    write_in(
        &dir,
        "hangs.test.mjs",
        "import { test, assert } from 'runtime:test';\n\
         test('finishes', () => assert(true));\n\
         test('never finishes', async () => { await new Promise(() => {}); });\n",
    );

    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(!out.status.success(), "{text}");
    assert!(text.contains("FAIL never finishes"), "{text}");
    assert!(text.contains("never finished"), "{text}");
    assert!(text.contains("1 passed, 1 failed"), "{text}");
}

/// Cases run one at a time, in the order they were written, with the lifecycle
/// hooks around each. Before this, `test()` *called* the function, so two async
/// cases sharing a variable ran at once and a `beforeEach` had no "before" to
/// happen in.
#[test]
fn tests_run_one_at_a_time_with_their_lifecycle_hooks() {
    let dir = build_dir("t_serial");
    write_in(
        &dir,
        "order.test.mjs",
        r#"
import { test, beforeAll, afterAll, beforeEach, afterEach, assertEquals } from "runtime:test";
const order = [];
let shared = 0;
beforeAll(() => order.push("beforeAll"));
afterAll(() => { order.push("afterAll"); console.log(order.join(" ")); });
beforeEach(() => { shared = 0; order.push("beforeEach"); });
afterEach(() => order.push("afterEach"));
test("slow", async () => {
  await new Promise((r) => setTimeout(r, 30));
  shared += 1;
  order.push("slow");
  assertEquals(shared, 1);
});
test("fast", () => {
  shared += 5;
  order.push("fast");
  assertEquals(shared, 5, "the slow test was still running");
});
"#,
    );

    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(out.status.success(), "{text}");
    assert!(
        text.contains("beforeAll beforeEach slow afterEach beforeEach fast afterEach afterAll"),
        "{text}"
    );
    assert!(text.contains("2 passed"), "{text}");
}

/// A group is a **scope**, not a naming convention: a hook written inside one
/// belongs to the tests inside it. Without that, `describe` is a template
/// string, and a file that sets up a database for six of its twenty cases sets
/// it up for the other fourteen too.
#[test]
fn a_group_scopes_its_hooks_and_composes_its_names() {
    let dir = build_dir("t_group");
    write_in(
        &dir,
        "group.test.mjs",
        r#"
import { test, describe, beforeAll, afterAll, beforeEach, afterEach } from "runtime:test";
const order = [];
beforeAll(() => order.push("file:all"));
afterAll(() => { order.push("file:/all"); console.log(order.join(" ")); });
beforeEach(() => order.push("file:each"));
afterEach(() => order.push("file:/each"));

test("top", () => order.push("top"));

describe("db", () => {
  beforeAll(() => order.push("db:all"));
  afterAll(() => order.push("db:/all"));
  beforeEach(() => order.push("db:each"));
  test("inserts", () => order.push("inserts"));
  describe("nested", () => {
    beforeEach(() => order.push("nested:each"));
    test("deep", () => order.push("deep"));
  });
});

test("last", () => order.push("last"));
"#,
    );

    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(out.status.success(), "{text}");
    // The file's hooks wrap every case; the group's wrap only its own, and its
    // `afterAll` runs when its last case has run rather than at the end of the
    // file. `beforeEach` runs outermost-first and `afterEach` innermost-first.
    assert!(
        text.contains(
            "file:all file:each top file:/each \
             db:all file:each db:each inserts file:/each \
             file:each db:each nested:each deep file:/each \
             db:/all file:each last file:/each file:/all"
        ),
        "{text}"
    );
    assert!(text.contains("4 passed"), "{text}");

    // A case is reported under its groups, outermost first, so a failure names
    // where it lives rather than only what it is called.
    write_in(
        &dir,
        "named.test.mjs",
        "import { test, describe, assert } from 'runtime:test';\n         describe('db', () => describe('inserts', () => test('rejects a null', () => assert(false))));\n",
    );
    let out = esdev_in(&dir)
        .arg("test")
        .arg("named")
        .output()
        .expect("spawn esdev test");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(
        text.contains("FAIL db > inserts > rejects a null"),
        "{text}"
    );
}

/// A skipped case is **in the report**, and one held back by somebody else's
/// `.only` is named apart from it. A green run that quietly ran fewer tests
/// than it printed is the failure this whole runner is arranged against.
#[test]
fn a_skipped_case_is_reported_rather_than_left_out() {
    let dir = build_dir("t_skip");
    write_in(
        &dir,
        "skip.test.mjs",
        r#"
import { test, describe } from "runtime:test";
test("runs", () => {});
test.skip("not this one", () => { throw new Error("MUST NOT RUN"); });
describe.skip("nor this group", () => {
  test("nor this", () => { throw new Error("MUST NOT RUN"); });
});
"#,
    );

    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(out.status.success(), "{text}");
    assert!(text.contains("1 passed, 0 failed, 2 skipped"), "{text}");

    // `only` runs one thing and says how much it did not run — a `.only` left
    // in a commit otherwise looks exactly like a suite that got faster. The
    // hooks the one case needs still run.
    write_in(
        &dir,
        "only.test.mjs",
        r#"
import { test, describe, beforeAll } from "runtime:test";
beforeAll(() => console.log("SETUP"));
test("one", () => { throw new Error("MUST NOT RUN"); });
describe("group", () => {
  test.only("the one being worked on", () => {});
  test("two", () => { throw new Error("MUST NOT RUN"); });
});
"#,
    );
    let out = esdev_in(&dir)
        .arg("test")
        .arg("only")
        .output()
        .expect("spawn esdev test");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(out.status.success(), "{text}");
    assert!(text.contains("SETUP"), "{text}");
    assert!(text.contains("only: 2 other tests did not run"), "{text}");
    assert!(text.contains("1 passed, 0 failed, 2 skipped"), "{text}");
}

/// A `describe` body registers and returns. An `async` one would register the
/// part before its first `await` and land the rest after the queue had already
/// drained, so it is refused rather than half-honoured.
#[test]
fn an_async_group_body_is_refused() {
    let dir = build_dir("t_async_group");
    write_in(
        &dir,
        "async.test.mjs",
        "import { test, describe } from 'runtime:test';\n         describe('slow', async () => { test('a', () => {}); });\n",
    );

    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(!out.status.success(), "{text}");
    assert!(text.contains("it cannot be async"), "{text}");
}

/// The other half of running in a queue: a case that never got a turn is still
/// in the report, and says so in its own words rather than borrowing the
/// message of the test that is actually stuck.
#[test]
fn a_case_behind_one_that_hangs_says_it_never_started() {
    let dir = build_dir("t_queued");
    write_in(
        &dir,
        "queued.test.mjs",
        "import { test } from 'runtime:test';\n\
         test('hangs', () => new Promise(() => {}));\n\
         test('behind it', () => {});\n",
    );

    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(!out.status.success(), "{text}");
    assert!(text.contains("FAIL hangs"), "{text}");
    assert!(text.contains("the test never finished"), "{text}");
    assert!(text.contains("FAIL behind it"), "{text}");
    assert!(text.contains("the test never started"), "{text}");
}

/// A test file is a module like any other, so running one directly is running
/// a module — no subcommand needed, and the same report either way.
#[test]
fn a_test_file_runs_on_its_own() {
    let dir = build_dir("t_direct");
    write_in(
        &dir,
        "direct.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('adds', () => assertEquals(2 + 3, 5));\n",
    );

    let out = esdev_in(&dir)
        .arg("direct.test.mjs")
        .output()
        .expect("spawn esdev");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    assert!(
        stdout(&out).contains("1 passed, 0 failed"),
        "{}",
        stdout(&out)
    );
}

/// And `runtime:test` is `esdev`'s, like the other two: a test file is never a
/// production artifact.
#[test]
fn runtime_test_does_not_exist_under_esrun() {
    let Some(esrun) = sibling_binary("esrun") else {
        eprintln!("skipping: esrun is not built in this target dir");
        return;
    };
    let dir = build_dir("t_esrun");
    let app = write_in(&dir, "app.mjs", "import 'runtime:test';\n");

    let out = Command::new(esrun)
        .current_dir(app.parent().expect("fixture dir"))
        .arg(app.file_name().expect("entry"))
        .output()
        .expect("spawn esrun");
    assert!(!out.status.success(), "{}", stdout(&out));
    assert!(
        stderr(&out).contains("unknown built-in module"),
        "{}",
        stderr(&out)
    );
}

/// One process per file: a file that exits must not take the run with it, and
/// the others must still be reported.
#[test]
fn a_file_that_exits_does_not_end_the_run() {
    let dir = build_dir("t_isolation");
    write_in(
        &dir,
        "a_exits.test.mjs",
        "import { test } from 'runtime:test';\n\
         import { exit } from 'runtime:process';\n\
         test('bails', () => exit(3));\n",
    );
    write_in(
        &dir,
        "b_fine.test.mjs",
        "import { test, assert } from 'runtime:test';\n\
         test('fine', () => assert(true));\n",
    );

    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(text.contains("b_fine.test.mjs"), "{text}");
    assert!(text.contains("a_exits.test.mjs"), "{text}");
}

#[test]
fn a_filter_selects_by_path() {
    let dir = build_dir("t_filter");
    write_in(
        &dir,
        "alpha.test.mjs",
        "import { test, assert } from 'runtime:test';\ntest('a', () => assert(true));\n",
    );
    write_in(
        &dir,
        "beta.test.mjs",
        "import { test, assert } from 'runtime:test';\ntest('b', () => assert(true));\n",
    );

    let out = esdev_in(&dir)
        .args(["test", "alpha"])
        .output()
        .expect("spawn esdev test");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    assert!(stdout(&out).contains("alpha.test.mjs"), "{}", stdout(&out));
    assert!(!stdout(&out).contains("beta.test.mjs"), "{}", stdout(&out));
}

/// `esdev check` runs the project's TypeScript through its package manager:
/// a fake `npm` records the invocation, so no network and no real install is
/// involved. Unix-only: the fake is a shell script.
#[cfg(unix)]
#[test]
fn check_runs_tsc_through_the_projects_package_manager() {
    use std::os::unix::fs::PermissionsExt;

    let dir = build_dir("t_check");
    write_in(
        &dir,
        "package.json",
        r#"{ "packageManager": "npm@1.0.0", "devDependencies": { "typescript": "^5" } }"#,
    );
    std::fs::create_dir_all(dir.join("node_modules/typescript")).expect("create ts");
    write_in(&dir, "node_modules/typescript/package.json", "{}");
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).expect("create bin");
    let npm = bin.join("npm");
    std::fs::write(
        &npm,
        "#!/bin/sh\necho \"$@\" >> \"$ESDEV_CHECK_CAPTURE\"\nexit \"${ESDEV_CHECK_EXIT:-0}\"\n",
    )
    .expect("write fake npm");
    std::fs::set_permissions(&npm, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    let capture = dir.join("args.txt");
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let out = esdev_in(&dir)
        .arg("check")
        .env("PATH", &path)
        .env("ESDEV_CHECK_CAPTURE", &capture)
        .env("ESDEV_CHECK_EXIT", "0")
        .output()
        .expect("spawn esdev check");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    let invoked = std::fs::read_to_string(&capture).expect("read capture");
    assert!(
        invoked.contains("exec -- tsc --noEmit"),
        "tsc was not invoked through npm exec: {invoked}"
    );

    // The exit code is tsc's own: a failing check fails the command.
    let out = esdev_in(&dir)
        .arg("check")
        .env("PATH", &path)
        .env("ESDEV_CHECK_CAPTURE", &capture)
        .env("ESDEV_CHECK_EXIT", "3")
        .output()
        .expect("spawn esdev check");
    assert!(!out.status.success());
    assert!(stderr(&out).contains("tsc failed"), "{}", stderr(&out));

    let _ = std::fs::remove_dir_all(&dir);
}

/// Without TypeScript installed there is nothing to run: the error names the
/// install rather than failing inside the package manager.
#[test]
fn check_without_typescript_names_the_install() {
    let dir = build_dir("t_check_missing");
    write_in(
        &dir,
        "package.json",
        r#"{ "packageManager": "npm@1.0.0", "devDependencies": { "typescript": "^5" } }"#,
    );

    let out = esdev_in(&dir)
        .arg("check")
        .output()
        .expect("spawn esdev check");
    assert!(!out.status.success());
    assert!(stderr(&out).contains("npm install"), "{}", stderr(&out));

    let _ = std::fs::remove_dir_all(&dir);
}

/// `esdev init` in an empty directory scaffolds the bare project: the files,
/// the package named for the directory, and JavaScript unless asked.
#[test]
fn init_starts_a_bare_project() {
    let parent = temp("t_init_new");
    let _ = std::fs::remove_dir_all(&parent);
    std::fs::create_dir_all(&parent).expect("create parent");
    let out = esdev()
        .args(["init", "hello", "-y", "--no-install"])
        .current_dir(&parent)
        .output()
        .expect("spawn esdev init");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    let dir = parent.join("hello");
    for expected in [
        "package.json",
        "esdev.json",
        "src/index.js",
        ".gitignore",
        "README.md",
    ] {
        assert!(dir.join(expected).is_file(), "{expected} missing");
    }
    let manifest = std::fs::read_to_string(dir.join("package.json")).expect("read manifest");
    assert!(manifest.contains("\"name\": \"hello\""), "{manifest}");
    assert!(
        !dir.join("node_modules").exists(),
        "installed despite --no-install"
    );

    let _ = std::fs::remove_dir_all(&parent);
}

/// `--language=ts` scaffolds the TypeScript overlay instead.
#[test]
fn init_bare_typescript_has_its_config() {
    let parent = temp("t_init_ts");
    let _ = std::fs::remove_dir_all(&parent);
    std::fs::create_dir_all(&parent).expect("create parent");
    let out = esdev()
        .args(["init", "tsapp", "--language=ts", "-y", "--no-install"])
        .current_dir(&parent)
        .output()
        .expect("spawn esdev init");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    let dir = parent.join("tsapp");
    assert!(dir.join("src/index.ts").is_file());
    assert!(dir.join("tsconfig.json").is_file());
    assert!(!dir.join("src/index.js").exists());

    let _ = std::fs::remove_dir_all(&parent);
}

/// Adopting writes the one missing file: a server entry becomes a run
/// server target. Without a manifest the types step warns rather than
/// failing, and offline-friendly.
#[test]
fn init_adopts_an_existing_project() {
    let dir = build_dir("t_init_adopt");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(&dir, "src/server.js", "console.log(\"legacy\");\n");

    let out = esdev_in(&dir)
        .args(["init", "-y"])
        .output()
        .expect("spawn esdev init");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    let manifest = std::fs::read_to_string(dir.join("esdev.json")).expect("read esdev.json");
    assert!(
        manifest.contains("\"entry\": \"src/server.js\""),
        "{manifest}"
    );
    assert!(manifest.contains("\"run\": \"server\""), "{manifest}");
    assert!(
        stdout(&out).contains("types not installed"),
        "{}",
        stdout(&out)
    );

    // Twice is a refusal, never an overwrite.
    let again = esdev_in(&dir)
        .args(["init", "-y"])
        .output()
        .expect("spawn esdev init");
    assert!(!again.status.success());
    assert!(
        stderr(&again).contains("already exists"),
        "{}",
        stderr(&again)
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// An OTF Web project is built and run by `otfw`, not esdev — so `esdev
/// build` and `esdev start` name that toolchain rather than refusing with
/// the missing `esdev.json`, which reads as a misconfiguration.
#[test]
fn an_otfw_project_is_pointed_at_otfw() {
    let dir = build_dir("t_otfw");
    write_in(
        &dir,
        "package.json",
        r#"{ "scripts": { "dev": "otfw dev", "build": "otfw build" } }"#,
    );

    let build = esdev_in(&dir).arg("build").output().expect("spawn esdev");
    assert!(!build.status.success());
    let message = stderr(&build);
    assert!(message.contains("otfw"), "{message}");
    assert!(message.contains("npm run build"), "{message}");

    let start = esdev_in(&dir).arg("start").output().expect("spawn esdev");
    assert!(!start.status.success());
    let message = stderr(&start);
    assert!(message.contains("otfw"), "{message}");
    assert!(message.contains("npm run dev"), "{message}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// A rehearsal runs each file under the named grants: `--deny-all` takes
/// everything, and `--allow-*` grants back what the suite needs — the same
/// flags, resolved the same way, as a run. A path the suite covers this way
/// meets its deployment's capabilities before deployment.
#[test]
fn test_permission_flags_rehearse_the_production_grant() {
    let dir = build_dir("t_rehearse");
    write_in(
        &dir,
        "env.test.mjs",
        "import { test, expect } from \"runtime:test\";\n\
         import { env } from \"runtime:process\";\n\
         test(\"reads its grant\", () => { expect(env.GREETING).toBe(\"hi\"); });\n",
    );

    // The grant the suite needs: green.
    let ok = esdev_in(&dir)
        .args(["test", "--deny-all", "--allow-env=GREETING"])
        .env("GREETING", "hi")
        .output()
        .expect("spawn esdev test");
    assert!(ok.status.success(), "{}{}", stdout(&ok), stderr(&ok));

    // Without it: the file fails on the denied capability, not on its logic.
    // The failure detail is the child's harness report on stdout; the
    // parent's own stderr carries only the tally.
    let denied = esdev_in(&dir)
        .args(["test", "--deny-all"])
        .env("GREETING", "hi")
        .output()
        .expect("spawn esdev test");
    assert!(!denied.status.success());
    let denied_output = format!("{}{}", stdout(&denied), stderr(&denied));
    assert!(denied_output.contains("NotAllowedError"), "{denied_output}");

    // A grant without the denial it narrows is refused up front, not once per
    // file in every child.
    let bad = esdev_in(&dir)
        .args(["test", "--allow-env=GREETING"])
        .env("GREETING", "hi")
        .output()
        .expect("spawn esdev test");
    assert!(!bad.status.success());
    assert!(stderr(&bad).contains("--deny-all"), "{}", stderr(&bad));

    let _ = std::fs::remove_dir_all(&dir);
}

/// Filesystem access rehearses the same way: denied by default under
/// `--deny-all`, granted back narrowed to the directory the suite reads.
#[test]
fn test_deny_all_covers_filesystem_access() {
    let dir = build_dir("t_rehearse_fs");
    std::fs::create_dir_all(dir.join("data")).expect("create data");
    write_in(&dir, "data/msg.txt", "hello\n");
    write_in(
        &dir,
        "read.test.mjs",
        "import { test, expect } from \"runtime:test\";\n\
         import { file } from \"runtime:fs\";\n\
         test(\"reads its file\", async () => {\n\
         \x20 const text = await (await file(\"data/msg.txt\")).text();\n\
         \x20 expect(text).toBe(\"hello\\n\");\n\
         });\n",
    );

    let denied = esdev_in(&dir)
        .args(["test", "--deny-all"])
        .output()
        .expect("spawn esdev test");
    assert!(!denied.status.success());
    let denied_output = format!("{}{}", stdout(&denied), stderr(&denied));
    assert!(denied_output.contains("NotAllowedError"), "{denied_output}");

    let ok = esdev_in(&dir)
        .args(["test", "--deny-all", "--allow-read=./data"])
        .output()
        .expect("spawn esdev test");
    assert!(ok.status.success(), "{}{}", stdout(&ok), stderr(&ok));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn no_test_files_is_an_error_rather_than_a_silent_pass() {
    let dir = build_dir("t_empty");
    write_in(&dir, "notatest.mjs", "export const x = 1;\n");
    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    assert!(
        !out.status.success(),
        "an empty run must not look like success"
    );
    let message = stderr(&out);
    assert!(message.contains("no test files"), "{message}");
    // Both discovery conventions, since 0.6.0 looks for either.
    assert!(message.contains("*.test."), "{message}");
    assert!(message.contains("*.spec."), "{message}");
}

// ---------------------------------------------------------------------------
// `esdev.json` — what a project builds, in a file
//
// A command line describes one bundle. An application that renders on the
// server and hydrates in the browser is two, from two entries, with two shapes
// of output — and the site it prerenders is a third that has to *run*. These
// tests are that whole shape reaching disk from one `esdev build`, plus the
// refusals that keep a mistyped key from being a setting that silently does
// nothing.
// ---------------------------------------------------------------------------

/// A project with all three kinds of target: one file, one directory, one that
/// runs when it is built. No dependencies — what is under test is the config,
/// not the bundler.
fn project_dir(name: &str) -> PathBuf {
    let dir = build_dir(name);
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    std::fs::create_dir_all(dir.join("public/nested")).expect("create public");
    write_in(&dir, "src/server.mjs", "console.log('server');\n");
    write_in(&dir, "src/client.mjs", "console.log('client');\n");
    write_in(
        &dir,
        "src/prerender.mjs",
        "import { write } from 'runtime:fs';\n\
         await write('about.html', '<h1>about</h1>');\n",
    );
    write_in(&dir, "index.html", "<!doctype html><div id=root></div>\n");
    write_in(&dir, "public/styles.css", "body{color:red}\n");
    write_in(&dir, "public/nested/deep.txt", "deep\n");
    write_in(
        &dir,
        "esdev.json",
        r#"{
          "targets": {
            "server":    { "entry": "src/server.mjs", "out": "dist/server.js",
                           "assets": ["index.html", "public"] },
            "browser":   { "entry": "src/client.mjs", "outdir": "dist/client",
                           "platform": "browser" },
            "prerender": { "entry": "src/prerender.mjs", "out": "dist/prerender.js",
                           "then": "run" }
          },
          "start": { "run": "server", "watch": ["server", "browser"] },
          "permissions": { "deny": ["all"], "allow": { "read": ["./dist"], "listen": ["8080"] } }
        }"#,
    );
    dir
}

#[test]
fn build_with_no_entry_builds_every_target_in_the_project() {
    let dir = project_dir("p_all");
    let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));

    assert!(dir.join("dist/server.js").exists(), "{}", stdout(&out));
    assert!(
        dir.join("dist/client/client.js").exists(),
        "{}",
        stdout(&out)
    );
    assert!(dir.join("dist/prerender.js").exists(), "{}", stdout(&out));

    // `then: run` executed the bundle it just built, and the file that step
    // wrote landed beside it — the runtime resolves a relative path against the
    // entry module's directory, which is what makes `dist/` the deployment.
    assert!(dir.join("dist/about.html").exists(), "{}", stdout(&out));
    assert!(stdout(&out).contains("ran → "), "{}", stdout(&out));
}

/// A file is copied by name and a directory by its *contents*, so
/// `public/styles.css` is served at `/styles.css` without anything having to
/// rewrite an href.
#[test]
fn assets_are_copied_by_name_and_directories_by_their_contents() {
    let dir = project_dir("p_assets");
    let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));

    assert!(dir.join("dist/index.html").exists(), "the named file");
    assert!(
        dir.join("dist/styles.css").exists(),
        "the directory's contents"
    );
    assert!(dir.join("dist/nested/deep.txt").exists(), "recursively");
    assert!(
        !dir.join("dist/public").exists(),
        "the directory itself was copied, so every href would need to know it"
    );
}

/// The condition that decides which build of a dependency a client bundle gets.
/// Conditions match in the order the *package author* wrote them, so `worker`
/// being asserted at all is enough to win — and the failure is not here, it is
/// in somebody's browser.
#[test]
fn a_browser_target_takes_the_browser_build_of_a_dependency() {
    let dir = build_dir("p_platform");
    let package = dir.join("node_modules/dual");
    std::fs::create_dir_all(&package).expect("create package");
    std::fs::write(
        package.join("package.json"),
        r#"{ "name": "dual", "version": "1.0.0", "type": "module",
             "exports": { ".": { "worker": "./worker.js", "browser": "./browser.js",
                                 "default": "./default.js" } } }"#,
    )
    .expect("write manifest");
    for build in ["worker", "browser", "default"] {
        std::fs::write(
            package.join(format!("{build}.js")),
            format!("export const who = '{}_BUILD';\n", build.to_uppercase()),
        )
        .expect("write build");
    }
    write_in(
        &dir,
        "app.mjs",
        "import { who } from 'dual';\nconsole.log(who);\n",
    );
    write_in(
        &dir,
        "esdev.json",
        r#"{ "targets": {
               "web": { "entry": "app.mjs", "outdir": "out/web", "platform": "browser" },
               "srv": { "entry": "app.mjs", "outdir": "out/srv" } } }"#,
    );

    let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));

    let web = std::fs::read_to_string(dir.join("out/web/app.js")).expect("read web bundle");
    let srv = std::fs::read_to_string(dir.join("out/srv/app.js")).expect("read server bundle");
    assert!(web.contains("BROWSER_BUILD"), "{web}");
    assert!(srv.contains("WORKER_BUILD"), "{srv}");
}

#[test]
fn target_builds_one_of_them_and_names_the_others_when_it_is_not_there() {
    let dir = project_dir("p_target");
    let one = esdev_in(&dir)
        .args(["build", "--target=browser"])
        .output()
        .expect("spawn esdev");
    assert!(one.status.success(), "{}{}", stdout(&one), stderr(&one));
    assert!(dir.join("dist/client/client.js").exists());
    assert!(
        !dir.join("dist/server.js").exists(),
        "--target built more than the one it named"
    );

    let missing = esdev_in(&dir)
        .args(["build", "--target=brower"])
        .output()
        .expect("spawn esdev");
    assert!(!missing.status.success());
    assert!(
        stderr(&missing).contains("is not a target"),
        "{}",
        stderr(&missing)
    );
    assert!(stderr(&missing).contains("browser"), "{}", stderr(&missing));
}

/// Naming an entry ignores the file entirely — a project that has a config can
/// still build a scratch entry — but asking for both leaves no answer to which
/// one named the entry.
#[test]
fn an_entry_on_the_command_line_and_a_target_together_are_refused() {
    let dir = project_dir("p_conflict");

    let scratch = esdev_in(&dir)
        .args(["build", "src/client.mjs", "--out=scratch.js"])
        .output()
        .expect("spawn esdev");
    assert!(scratch.status.success(), "{}", stderr(&scratch));
    assert!(dir.join("scratch.js").exists());
    assert!(
        !dir.join("dist/server.js").exists(),
        "naming an entry built the project's targets as well"
    );

    let both = esdev_in(&dir)
        .args(["build", "src/client.mjs", "--target=browser"])
        .output()
        .expect("spawn esdev");
    assert!(!both.status.success());
    assert!(
        stderr(&both).contains("--target=browser"),
        "{}",
        stderr(&both)
    );

    // `--out` names one file, and a project build writes what its targets say.
    let out = esdev_in(&dir)
        .args(["build", "--out=everything.js"])
        .output()
        .expect("spawn esdev");
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("--out=everything.js"),
        "{}",
        stderr(&out)
    );
}

/// A mistyped key is otherwise a setting that silently does nothing, which for
/// `platform` is the wrong build of a dependency.
#[test]
fn a_config_error_names_the_key_and_the_one_it_was_nearly() {
    let dir = build_dir("p_typo");
    write_in(&dir, "app.mjs", "console.log(1);\n");
    write_in(
        &dir,
        "esdev.json",
        r#"{ "targets": { "app": { "entry": "app.mjs", "outDir": "dist" } } }"#,
    );
    let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("unknown key `outDir`"),
        "{}",
        stderr(&out)
    );
    assert!(stderr(&out).contains("`outdir`"), "{}", stderr(&out));
}

/// The permissions in the file go through the same parser the flags do, so the
/// file cannot mean anything a command line could not — and it is wrong when it
/// is read, not when a run is finally attempted with it.
#[test]
fn permissions_in_the_file_are_checked_by_the_flag_parser() {
    let dir = build_dir("p_perms");
    write_in(&dir, "app.mjs", "console.log(1);\n");
    write_in(
        &dir,
        "esdev.json",
        r#"{ "targets": { "app": { "entry": "app.mjs" } },
             "permissions": { "deny": ["all"], "allow": { "filesystem": true } } }"#,
    );
    let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
    assert!(!out.status.success());
    assert!(stderr(&out).contains("filesystem"), "{}", stderr(&out));
}

/// `--config` points at a file elsewhere, and every path in it is relative to
/// *that* file rather than to the working directory.
#[test]
fn a_config_elsewhere_resolves_its_paths_against_itself() {
    let dir = project_dir("p_elsewhere");
    let outside = build_dir("p_elsewhere_cwd");
    let config = dir.join("esdev.json");

    let out = esdev_in(&outside)
        .args([
            "build",
            &format!("--config={}", config.display()),
            "--target=server",
        ])
        .output()
        .expect("spawn esdev");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    assert!(
        dir.join("dist/server.js").exists(),
        "built beside the config"
    );
    assert!(!outside.join("dist").exists(), "built beside the caller");
}

// ---------------------------------------------------------------------------
// An `index.html` target (DECISIONS D61)
//
// A server bundle starts at a module because the runtime does; the browser
// starts at a document. So the script and link tags in an HTML file are the
// build's inputs, and what is written out is the same document pointing at what
// was built — with everything the author wrote between those tags untouched.
// ---------------------------------------------------------------------------

/// A document that references one module, one stylesheet, one image, one CDN
/// URL it does not own, and an inline script nobody should touch.
fn html_project(name: &str) -> PathBuf {
    let dir = build_dir(name);
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(&dir, "src/dep.mjs", "export const answer = 42;\n");
    write_in(
        &dir,
        "src/entry.client.mjs",
        "import { answer } from './dep.mjs';\nconsole.log(answer);\n",
    );
    write_in(&dir, "styles.css", "body{color:red}\n");
    write_in(&dir, "logo.svg", "<svg/>\n");
    write_in(
        &dir,
        "index.html",
        r#"<!doctype html>
<html lang="en"><head>
<meta charset="utf-8">
<title>My App</title>
<link rel="stylesheet" href="./styles.css">
<link rel="icon" href="./logo.svg">
<script>window.__EARLY__ = 1;</script>
<script type="module" src="./src/entry.client.mjs"></script>
<script src="https://cdn.example.com/analytics.js"></script>
</head><body><div id="root"></div></body></html>
"#,
    );
    write_in(
        &dir,
        "esdev.json",
        r#"{ "targets": { "web": { "entry": "index.html", "outdir": "dist" } } }"#,
    );
    dir
}

#[test]
fn an_html_target_builds_what_it_references_and_rewrites_it() {
    let dir = html_project("h_build");
    let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));

    let document = std::fs::read_to_string(dir.join("dist/index.html")).expect("read the document");

    // The module script became a hashed bundle under /assets, and the document
    // points at it. The name is the bundler's, so it is read back out of the
    // document rather than guessed.
    let script = document
        .split_once(r#"<script type="module" src=""#)
        .and_then(|(_, rest)| rest.split_once('"'))
        .map(|(url, _)| url.to_string())
        .expect("a module script survived");
    assert!(script.starts_with("/assets/entry.client-"), "{script}");
    assert!(script.ends_with(".js"), "{script}");
    // The URL is rooted at the deployment, which is the output directory.
    let bundle = dir.join("dist").join(script.trim_start_matches('/'));
    assert!(bundle.exists(), "{script} was not written");
    let code = std::fs::read_to_string(&bundle).expect("read bundle");
    assert!(!code.contains("./dep.mjs"), "the import survived:\n{code}");

    // The stylesheet and the icon were copied and hashed.
    for (attribute, prefix, suffix) in [
        ("href=\"/assets/styles-", "styles-", ".css"),
        ("href=\"/assets/logo-", "logo-", ".svg"),
    ] {
        assert!(document.contains(attribute), "{document}");
        let copied = std::fs::read_dir(dir.join("dist/assets"))
            .expect("read assets")
            .flatten()
            .any(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                name.starts_with(prefix) && name.ends_with(suffix)
            });
        assert!(copied, "no {prefix}…{suffix} in dist/assets");
    }

    // Everything else is the author's.
    assert!(document.contains("<title>My App</title>"), "{document}");
    assert!(document.contains("window.__EARLY__ = 1;"), "{document}");
    assert!(document.contains(r#"<html lang="en">"#), "{document}");
    assert!(
        document.contains(r#"src="https://cdn.example.com/analytics.js""#),
        "a URL this build does not own was rewritten:\n{document}"
    );
}

/// The hash follows the content, which is the whole reason it is there — a
/// deployment caches `/assets` immutably, and a file whose name did not change
/// is a file the browser will not fetch again.
#[test]
fn a_changed_file_gets_a_changed_name() {
    let dir = html_project("h_hash");
    let build = || {
        let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
        assert!(out.status.success(), "{}", stderr(&out));
        std::fs::read_to_string(dir.join("dist/index.html")).expect("read the document")
    };

    let before = build();
    write_in(&dir, "styles.css", "body{color:blue}\n");
    let after = build();
    assert_ne!(before, after, "the stylesheet changed and its URL did not");
}

/// A stylesheet is an entry, not a file to copy: what the document ends up
/// pointing at is the whole tree, with every `url()` aimed at where the file it
/// named actually landed.
///
/// The unit tests in `css.rs` cover the bundling; what is worth an end-to-end
/// test is the wiring around it, because both halves fail silently. A
/// placeholder that is never substituted is a stylesheet full of opaque hashes,
/// and a hash computed before substitution is a URL that never changes.
#[test]
fn a_stylesheet_is_bundled_with_what_it_imports_and_references() {
    let dir = build_dir("h_css");
    std::fs::create_dir_all(dir.join("theme")).expect("create theme");
    write_in(
        &dir,
        "styles.css",
        "@import \"./theme/dark.css\";\nbody { color: var(--ink) }\n",
    );
    write_in(
        &dir,
        "theme/dark.css",
        ":root { --ink: #eee }\nbody { background: url(./grain.png) }\n",
    );
    write_in(&dir, "theme/grain.png", "not really a png\n");
    write_in(
        &dir,
        "index.html",
        r#"<!doctype html><html><head><link rel="stylesheet" href="./styles.css"></head><body></body></html>"#,
    );
    write_in(
        &dir,
        "esdev.json",
        r#"{ "targets": { "web": { "entry": "index.html", "outdir": "dist" } } }"#,
    );

    let build = || {
        let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
        assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
        let document =
            std::fs::read_to_string(dir.join("dist/index.html")).expect("read the document");
        let url = document
            .split_once(r#"<link rel="stylesheet" href=""#)
            .and_then(|(_, rest)| rest.split_once('"'))
            .map(|(url, _)| url.to_string())
            .expect("the stylesheet survived");
        let css = std::fs::read_to_string(dir.join("dist").join(url.trim_start_matches('/')))
            .expect("read the stylesheet");
        (url, css)
    };

    let (url, css) = build();
    assert!(url.starts_with("/assets/styles-"), "{url}");

    // The import is gone because its contents are here.
    assert!(!css.contains("@import"), "the import survived:\n{css}");
    assert!(css.contains("--ink"), "the import was not inlined:\n{css}");

    // The `url()` names where the file landed — rooted, hashed, and written.
    let referenced = css
        .split_once("url(")
        .and_then(|(_, rest)| rest.split_once(')'))
        .map(|(url, _)| url.trim_matches(['"', '\'']).to_string())
        .expect("a url() survived");
    assert!(
        referenced.starts_with("/assets/grain-") && referenced.ends_with(".png"),
        "the placeholder was never substituted: {referenced}"
    );
    assert!(
        dir.join("dist")
            .join(referenced.trim_start_matches('/'))
            .is_file(),
        "{referenced} was not written"
    );

    // The name follows the content of the *bundle*, so editing an imported file
    // — which the entry's own bytes know nothing about — still busts the cache.
    write_in(&dir, "theme/dark.css", ":root { --ink: #111 }\n");
    let (changed, _) = build();
    assert_ne!(url, changed, "an imported file changed and the URL did not");
}

/// A stylesheet can be the command-line entry too. Previously it travelled
/// through the JavaScript CSS-import adapter and `rolldown` faithfully wrote
/// the adapter's empty module as a zero-byte `.js` file.
#[test]
fn a_css_entry_bundles_its_imports_to_a_stylesheet() {
    let dir = build_dir("css_entry");
    write_in(
        &dir,
        "styles.css",
        "@import './theme.css';\nbody { color: #ffffff }",
    );
    write_in(&dir, "theme.css", ":root { margin: 0px 0px 0px 0px }");

    let out = esdev_in(&dir)
        .args(["build", "styles.css", "--minify"])
        .output()
        .expect("spawn esdev");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    let css = std::fs::read_to_string(dir.join("dist/styles.css")).expect("read stylesheet");
    assert!(!css.contains("@import"), "{css}");
    assert_eq!(css, ":root{margin:0}body{color:#fff}");
    assert!(
        !dir.join("dist/styles.js").exists(),
        "wrote a JavaScript shell"
    );
}

#[test]
fn a_library_keeps_css_at_its_exports_map_path() {
    let dir = build_dir("lib_css");
    std::fs::create_dir_all(dir.join("src")).expect("create source");
    write_in(&dir, "src/index.ts", "export const answer: number = 42;\n");
    write_in(
        &dir,
        "src/styles.css",
        "@import './theme.css';\n.button { color: red }\n",
    );
    write_in(&dir, "src/theme.css", ":root { --brand: #123456 }\n");

    let out = esdev_in(&dir)
        .args(["build", "--lib", "src"])
        .output()
        .expect("spawn esdev");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    assert_eq!(
        std::fs::read_to_string(dir.join("dist/styles.css")).expect("fixed stylesheet"),
        "@import './theme.css';\n.button { color: red }\n"
    );
    assert!(
        dir.join("dist/theme.css").is_file(),
        "keeps imported sibling"
    );
}

/// CSS Modules: a stylesheet the *JavaScript* imports, rather than one the
/// document links.
///
/// The end-to-end property is the one the unit tests cannot reach — that the
/// name the bundle uses and the name the stylesheet declares are the same
/// string, and that two files declaring the same class do not collide.
#[test]
fn a_css_module_is_scoped_and_reaches_both_the_bundle_and_a_stylesheet() {
    let dir = build_dir("h_cssmod");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(
        &dir,
        "src/Button.module.css",
        ".button { color: red }
:global(.no-js) .button { color: grey }
",
    );
    // A second file with the *same* local name: the whole point of scoping.
    write_in(
        &dir,
        "src/Card.module.css",
        ".button { color: blue }
",
    );
    write_in(
        &dir,
        "src/main.js",
        "import button from './Button.module.css';
         import card from './Card.module.css';
         document.body.className = button.button + ' ' + card.button;
",
    );
    write_in(
        &dir,
        "index.html",
        r#"<!doctype html><html><head><title>t</title><script type="module" src="./src/main.js"></script></head><body></body></html>"#,
    );
    write_in(
        &dir,
        "esdev.json",
        r#"{ "targets": { "web": { "entry": "index.html", "outdir": "dist" } } }"#,
    );

    let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));

    // The document links a stylesheet nothing in it referenced — the build
    // wrote it from what the JavaScript imported.
    let document = std::fs::read_to_string(dir.join("dist/index.html")).expect("read document");
    let href = document
        .split_once(r#"<link rel="stylesheet" href=""#)
        .and_then(|(_, rest)| rest.split_once('"'))
        .map(|(url, _)| url.to_string())
        .expect("a stylesheet was linked");
    assert!(href.starts_with("/assets/modules-"), "{href}");
    let css = std::fs::read_to_string(dir.join("dist").join(href.trim_start_matches('/')))
        .expect("read the stylesheet");

    // Two files, one local name, two scoped names — and both are in the CSS.
    let unique: std::collections::BTreeSet<&str> = css
        .match_indices(".button_")
        .map(|(at, _)| {
            let from = at + 1;
            let len = css[from..]
                .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .unwrap_or(css.len() - from);
            &css[from..from + len]
        })
        .collect();
    assert_eq!(unique.len(), 2, "expected two scoped names, got {unique:?}");

    // …and the bundle uses exactly those strings, or the markup would name a
    // class the stylesheet never declared.
    let bundle = std::fs::read_dir(dir.join("dist/assets"))
        .expect("read assets")
        .flatten()
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("main") && n.ends_with(".js"))
        })
        .expect("a bundle");
    let code = std::fs::read_to_string(&bundle).expect("read bundle");
    for name in &unique {
        assert!(
            code.contains(name),
            "{name} is not in the bundle:
{code}"
        );
    }

    // `:global()` is a convention of this build, not a selector any browser
    // knows: the wrapper has to be gone and its contents left alone.
    assert!(
        !css.contains(":global"),
        "the wrapper survived:
{css}"
    );
    assert!(css.contains(".no-js"), "{css}");
}

/// `composes`, and a plain stylesheet imported from JavaScript.
///
/// The properties worth an end-to-end test are the two that need the module
/// graph: that a composed module's rules reach the output even though nothing
/// imported it, and that composition is transitive — a class only styles an
/// element that actually carries it, so a chain that stops halfway loses the
/// middle link's styling and nothing reports it.
#[test]
fn composes_is_transitive_and_a_plain_stylesheet_is_imported_whole() {
    let dir = build_dir("h_composes");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    std::fs::create_dir_all(dir.join("vendor")).expect("create vendor");

    // Nothing imports this module; only `composes` names it.
    write_in(
        &dir,
        "src/base.module.css",
        ".rounded { border-radius: 8px }\n",
    );
    write_in(
        &dir,
        "src/Button.module.css",
        ".button { composes: rounded from \"./base.module.css\"; color: white }\n         .big { composes: button; font-size: 2rem }\n",
    );
    // A third-party stylesheet: its own JS emits these names, so scoping them
    // would rename half of a contract the library has with itself.
    write_in(
        &dir,
        "vendor/lib.css",
        ".lib-widget { outline: 2px solid green }\n",
    );
    write_in(
        &dir,
        "src/main.js",
        "import '../vendor/lib.css';\n         import styles from './Button.module.css';\n         document.body.className = styles.big;\n",
    );
    write_in(
        &dir,
        "index.html",
        r#"<!doctype html><html><head><title>t</title><script type="module" src="./src/main.js"></script></head><body></body></html>"#,
    );
    write_in(
        &dir,
        "esdev.json",
        r#"{ "targets": { "web": { "entry": "index.html", "outdir": "dist" } } }"#,
    );

    let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));

    let document = std::fs::read_to_string(dir.join("dist/index.html")).expect("read document");
    let href = document
        .split_once(r#"<link rel="stylesheet" href=""#)
        .and_then(|(_, rest)| rest.split_once('"'))
        .map(|(url, _)| url.to_string())
        .expect("a stylesheet was linked");
    let css = std::fs::read_to_string(dir.join("dist").join(href.trim_start_matches('/')))
        .expect("read the stylesheet");

    // The composed module's rules are there even though no JavaScript imported
    // it — without them, `composes` hands out a class name that styles nothing.
    assert!(
        css.contains("border-radius"),
        "the composed module is missing:\n{css}"
    );
    // `composes` is not a property any browser knows; it must be gone.
    assert!(!css.contains("composes"), "{css}");
    // A vendor stylesheet is emitted unscoped, or its own JS stops matching it.
    assert!(css.contains(".lib-widget"), "{css}");

    let bundle = std::fs::read_dir(dir.join("dist/assets"))
        .expect("read assets")
        .flatten()
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("main") && n.ends_with(".js"))
        })
        .expect("a bundle");
    let code = std::fs::read_to_string(&bundle).expect("read bundle");

    // `.big` composes `.button`, which composes `.rounded` — three names, and
    // every one of them declared in the stylesheet.
    let big = code
        .split_once(r#""big": ""#)
        .and_then(|(_, rest)| rest.split_once('"'))
        .map(|(value, _)| value.to_string())
        .expect("a mapping for `big`");
    let names: Vec<&str> = big.split(' ').collect();
    assert_eq!(names.len(), 3, "not transitive: {big}");
    for name in names {
        assert!(css.contains(name), "{name} is not declared in:\n{css}");
    }
}

/// A relative path names a file in the project. If it is not there, that is a
/// broken page, and the build is where it should be found — not the browser.
#[test]
fn a_reference_that_is_not_there_stops_the_build() {
    let dir = build_dir("h_missing");
    write_in(
        &dir,
        "index.html",
        r#"<html><head><script type="module" src="./src/gone.mjs"></script></head></html>"#,
    );
    write_in(
        &dir,
        "esdev.json",
        r#"{ "targets": { "web": { "entry": "index.html", "outdir": "dist" } } }"#,
    );
    let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
    assert!(!out.status.success());
    assert!(stderr(&out).contains("./src/gone.mjs"), "{}", stderr(&out));
    assert!(stderr(&out).contains("not there"), "{}", stderr(&out));
}

/// Two entries built to one name is a build that silently ships half of what
/// the page asked for.
#[test]
fn two_module_scripts_that_would_collide_are_refused() {
    let dir = build_dir("h_collide");
    std::fs::create_dir_all(dir.join("a")).expect("create a");
    std::fs::create_dir_all(dir.join("b")).expect("create b");
    write_in(&dir, "a/main.mjs", "console.log('a');\n");
    write_in(&dir, "b/main.mjs", "console.log('b');\n");
    write_in(
        &dir,
        "index.html",
        r#"<html><head>
           <script type="module" src="./a/main.mjs"></script>
           <script type="module" src="./b/main.mjs"></script>
           </head></html>"#,
    );
    write_in(
        &dir,
        "esdev.json",
        r#"{ "targets": { "web": { "entry": "index.html", "outdir": "dist" } } }"#,
    );
    let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("both called main"),
        "{}",
        stderr(&out)
    );
}

// ---------------------------------------------------------------------------
// `esdev start` (DECISIONS D62)
//
// The dev loop: build, run, rebuild, reload. What is worth testing is not that
// a bundler bundles or that a socket accepts — it is the three promises the
// loop makes. The app's own server is what runs. A build that fails leaves
// what was working alone. And the browser is told, once, after the restart.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// `esdev preview`: the release build, before it is a deployment
// ---------------------------------------------------------------------------

/// The dev loop serves a build that is not the one that ships — NODE_ENV is
/// "development" and nothing is hashed — so the last look before deploying has
/// to be at the release output itself.
#[test]
fn preview_serves_the_build_with_a_route_fallback() {
    let dir = build_dir("b_preview");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(
        &dir,
        "src/main.ts",
        "document.body.innerHTML = `<h1>${import.meta.env.MODE}</h1>`;\n",
    );
    write_in(
        &dir,
        "index.html",
        "<!doctype html><html><body>\
         <script type=\"module\" src=\"./src/main.ts\"></script></body></html>\n",
    );
    write_in(
        &dir,
        "esdev.json",
        "{ \"targets\": { \"web\": { \"entry\": \"index.html\", \"outdir\": \"dist\" } } }\n",
    );

    // Nothing built yet: a preview serves what a build wrote, and says so
    // rather than opening a port onto an empty directory.
    let unbuilt = esdev_in(&dir)
        .args(["preview", "--port=0"])
        .output()
        .expect("spawn esdev preview");
    assert!(!unbuilt.status.success(), "{}", stdout(&unbuilt));
    assert!(
        stderr(&unbuilt).contains("esdev build"),
        "{}",
        stderr(&unbuilt)
    );

    let built = esdev_in(&dir)
        .arg("build")
        .output()
        .expect("spawn esdev build");
    assert!(built.status.success(), "{}", stderr(&built));

    let port = test_port("preview");
    let mut command = esdev_in(&dir);
    command
        .args(["preview", &format!("--port={port}")])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let _server = Supervisor(Some(command.spawn().expect("spawn esdev preview")));

    let index = wait_for_http(port, "/", |response| response.contains("<!doctype html"));
    assert!(index.contains("<script"), "{index}");
    // The document points at the hashed bundle a release build wrote, which is
    // the half `esdev start` cannot show.
    assert!(index.contains("/assets/main-"), "{index}");

    // A reload on a route the router owns has to reach the router.
    let route = http_get(port, "/about").expect("a response");
    assert!(route.contains("200 OK"), "{route}");
    assert!(route.contains("<script"), "{route}");

    // …and a missing file that is not a route is still missing.
    let missing = http_get(port, "/nope.js").expect("a response");
    assert!(missing.contains("404"), "{missing}");
}

/// A server bundle is run, not served — and by the binary that will run it in
/// production.
#[test]
fn preview_says_a_server_project_is_run_rather_than_served() {
    let dir = build_dir("b_preview_server");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(&dir, "src/api.ts", "export default 1;\n");
    write_in(
        &dir,
        "esdev.json",
        "{ \"targets\": { \"api\": { \"entry\": \"src/api.ts\", \
         \"out\": \"dist/api.js\" } } }\n",
    );

    let out = esdev_in(&dir)
        .args(["preview", "--port=0"])
        .output()
        .expect("spawn esdev preview");
    assert!(!out.status.success(), "{}", stdout(&out));
    assert!(stderr(&out).contains("esrun"), "{}", stderr(&out));
}

// ---------------------------------------------------------------------------
// `esdev test`: what runs at once, and what happens on a save
// ---------------------------------------------------------------------------

/// `--isolation=none` is intentionally the opposite of the default: test
/// files share one module map and global object, which is useful for a large
/// suite whose common dependency graph costs more than its tests.
#[test]
fn tests_can_share_one_process_and_module_cache() {
    let dir = build_dir("b_test_no_isolation");
    write_in(
        &dir,
        "shared.mjs",
        "globalThis.shared_loads = (globalThis.shared_loads ?? 0) + 1;\n\
         export const loads = globalThis.shared_loads;\n",
    );
    write_in(
        &dir,
        "a.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         import { loads } from './shared.mjs';\n\
         globalThis.from_first_file = loads;\n\
         test('loads the shared module once', () => assertEquals(loads, 1));\n",
    );
    write_in(
        &dir,
        "b.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         import { loads } from './shared.mjs';\n\
         test('keeps the first file and shared module alive', () => {\n\
           assertEquals(loads, 1);\n\
           assertEquals(globalThis.from_first_file, 1);\n\
         });\n",
    );

    let out = esdev_in(&dir)
        .args(["test", "--isolation=none"])
        .output()
        .expect("spawn esdev test");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    assert!(stdout(&out).contains("2 passed"), "{}", stdout(&out));
}

/// A case's own trailing promise chain finishes before its cleanup runs. A
/// suite written against a browser runner leaves `resolve().then(assert)`
/// un-awaited and expects `assert` to see the DOM the case rendered; tearing
/// down after a single microtask turn hands it a dismantled one. Found porting
/// Preact's suspense suite, where it read as a scheduler difference.
#[test]
fn test_a_cases_trailing_promises_settle_before_its_cleanup() {
    let dir = build_dir("b_test_trailing_chain");
    write_in(
        &dir,
        "a.test.mjs",
        "import { test, afterEach, assertEquals } from 'runtime:test';\n\
         const seq = [];\n\
         afterEach(() => seq.push('afterEach'));\n\
         test('leaves a chain', () => {\n\
           Promise.resolve().then(() => seq.push('one')).then(() => seq.push('two'))\n\
             .then(() => seq.push('three'));\n\
         });\n\
         test('the chain ran first', () => {\n\
           assertEquals(seq, ['one', 'two', 'three', 'afterEach']);\n\
         });\n",
    );
    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(out.status.success(), "{text}");
    assert!(text.contains("2 passed"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A stray promise costs one test, not the file's whole report. Before this,
/// the process died where the rejection surfaced and a suite of a hundred
/// passing tests printed nothing at all — the one thing a test runner must
/// never do.
#[test]
fn test_an_unhandled_rejection_fails_its_case_and_the_rest_still_report() {
    let dir = build_dir("b_test_unhandled_rejection");
    write_in(
        &dir,
        "a.test.mjs",
        "import { test } from 'runtime:test';\n\
         test('one passes', () => {});\n\
         test('two leaks a rejection', async () => {\n\
           Promise.reject(new Error('boom'));\n\
           await new Promise((resolve) => setTimeout(resolve, 1));\n\
         });\n\
         test('three passes', () => {});\n",
    );
    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(!out.status.success(), "a leaked rejection passed:\n{text}");
    assert!(text.contains("2 passed, 1 failed"), "{text}");
    assert!(text.contains("two leaks a rejection"), "{text}");
    assert!(text.contains("boom"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// And when something kills the run outright — a rejection after the last case,
/// with no test left to blame — what already ran is still printed.
#[test]
fn test_a_run_that_dies_still_reports_what_ran() {
    let dir = build_dir("b_test_dies_after_running");
    write_in(
        &dir,
        "a.test.mjs",
        "import { test } from 'runtime:test';\n\
         test('one passes', () => {});\n\
         setTimeout(() => { Promise.reject(new Error('after the run')); }, 300);\n",
    );
    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(!out.status.success(), "{text}");
    assert!(text.contains("after the run"), "{text}");
    // Late enough that it lands after the case, but a loaded machine could
    // still deliver it inside one — either way the tally is printed, which is
    // what this is about.
    assert!(text.contains("passed"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_isolation_can_come_from_project_config() {
    let dir = build_dir("b_test_no_isolation_config");
    write_in(
        &dir,
        "esdev.json",
        r#"{ "targets": { "app": { "entry": "app.mjs", "out": "dist/app.mjs" } },
             "test": { "isolation": "none" } }"#,
    );
    write_in(&dir, "app.mjs", "export {};\n");
    write_in(
        &dir,
        "a.test.mjs",
        "import { test } from 'runtime:test';\nglobalThis.config_shared = true;\ntest('a', () => {});\n",
    );
    write_in(
        &dir,
        "b.test.mjs",
        "import { test, assert } from 'runtime:test';\ntest('b', () => assert(globalThis.config_shared));\n",
    );

    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    assert!(stdout(&out).contains("2 passed"), "{}", stdout(&out));
}

#[test]
fn unisolated_tests_refuse_file_scoped_options() {
    let dir = build_dir("b_test_no_isolation_options");
    write_in(
        &dir,
        "one.test.mjs",
        "import { test } from 'runtime:test';\ntest('ok', () => {});\n",
    );
    for flag in ["--jobs=1", "--timeout=100", "--reporter=json"] {
        let out = esdev_in(&dir)
            .args(["test", "--isolation=none", flag])
            .output()
            .expect("spawn esdev test");
        assert!(!out.status.success(), "{flag}: {}", stdout(&out));
        assert!(
            stderr(&out).contains("--isolation=none"),
            "{flag}: {}",
            stderr(&out)
        );
    }
}

/// A file is a process, so the machine's cores are the runner's to use — and
/// with more than one running, output is held and printed whole rather than
/// interleaved line by line with another file's.
#[test]
fn tests_run_in_parallel_and_keep_each_files_output_together() {
    let dir = build_dir("b_test_jobs");
    for i in 1..=4 {
        write_in(
            &dir,
            &format!("slow{i}.test.ts"),
            &format!(
                "import {{ test, assertEquals }} from \"runtime:test\";\n\
                 test(\"slow {i}\", async () => {{\n  \
                 await new Promise((r) => setTimeout(r, 300));\n  \
                 console.log(\"line one of {i}\");\n  \
                 console.log(\"line two of {i}\");\n  \
                 assertEquals(1 + 1, 2);\n}});\n"
            ),
        );
    }
    write_in(
        &dir,
        "broken.test.ts",
        "import { test, assertEquals } from \"runtime:test\";\n\
         test(\"fails\", () => { assertEquals(1, 2); });\n",
    );

    let started = std::time::Instant::now();
    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    let elapsed = started.elapsed();
    assert!(
        !out.status.success(),
        "a failing file passed: {}",
        stdout(&out)
    );

    let report = stdout(&out);
    assert!(report.contains("1 of 5 files failed"), "{report}");
    // Four 300ms files, so anything near 1.2s means they ran one after another.
    assert!(
        elapsed < Duration::from_millis(1100),
        "ran serially: {elapsed:?}\n{report}"
    );
    // Each file's lines stayed together: nothing came between them.
    for i in 1..=4 {
        let together = format!("line one of {i}\nline two of {i}");
        assert!(
            report.replace("\r\n", "\n").contains(&together),
            "output interleaved:\n{report}"
        );
    }

    // --jobs=1 is the other half of the deal: one at a time, writing straight
    // through, for the run where a test is hanging and you want to see it.
    let serial = esdev_in(&dir)
        .args(["test", "--jobs=1", "slow1"])
        .output()
        .expect("spawn esdev test");
    assert!(serial.status.success(), "{}", stderr(&serial));
    assert!(
        stdout(&serial).contains("1 file passed"),
        "{}",
        stdout(&serial)
    );
}

/// `--file` is one run of one file — the shape the parent invokes for each
/// child — so there is nothing to schedule and nothing to re-run.
#[test]
fn test_refuses_to_schedule_a_single_file() {
    let dir = build_dir("b_test_args");
    write_in(
        &dir,
        "one.test.ts",
        "import { test } from \"runtime:test\";\ntest(\"ok\", () => {});\n",
    );

    for flag in ["--watch", "--jobs=2"] {
        let out = esdev_in(&dir)
            .args(["test", "--file=one.test.ts", flag])
            .output()
            .expect("spawn esdev test");
        assert!(!out.status.success(), "{flag}: {}", stdout(&out));
        assert!(stderr(&out).contains("--file"), "{flag}: {}", stderr(&out));
    }

    let bad = esdev_in(&dir)
        .args(["test", "--jobs=0"])
        .output()
        .expect("spawn esdev test");
    assert!(!bad.status.success(), "{}", stdout(&bad));
    assert!(stderr(&bad).contains("--jobs=0"), "{}", stderr(&bad));
}

/// `esdev start` runs the application as a child of its own, so killing only
/// the supervisor leaves that child holding a port — and, having inherited the
/// test harness's stdout, holding the harness open too. The whole group goes.
/// An `esdev start` that stops when it goes out of scope.
///
/// On `Drop` rather than at the end of a passing test, and the difference is not
/// tidiness. An assertion that fires unwinds past any explicit stop, leaving a
/// supervisor running and holding the port this test derived from its own name.
/// The *next* run then talks to the previous run's dev server, serving the
/// previous run's `dist`, and fails for a reason that has nothing to do with the
/// code — on a machine where the first failure has already been fixed. This cost
/// an afternoon once, and it cost it twice.
struct Supervisor(Option<std::process::Child>);

impl Drop for Supervisor {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            stop_supervisor(&mut child);
        }
    }
}

fn stop_supervisor(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let group = child.id();
        let _ = Command::new("kill")
            .arg("-TERM")
            .arg(format!("-{group}"))
            .status();
        std::thread::sleep(Duration::from_millis(300));
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// A supervisor in a process group of its own, with its output discarded.
fn start_in(dir: &Path) -> Supervisor {
    let mut command = esdev_in(dir);
    command
        .arg("start")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    Supervisor(Some(command.spawn().expect("spawn esdev start")))
}

/// A port unlikely to collide with anything else on the machine, derived from
/// the test's own name so two tests never pick the same one.
/// A port derived from the test's own name, **below the ephemeral range**.
///
/// The upper bound is the load-bearing part. Linux hands out source ports for
/// outbound connections from `net.ipv4.ip_local_port_range`, 32768–60999 by
/// default, so a fixed port chosen from inside that window is one the kernel
/// may already have given to somebody else's socket — and `bind` then fails
/// with `AddrInUse`. This range was 20000–39999, whose top third overlapped it;
/// under `cargo test --workspace`, where hundreds of short-lived connections
/// are opened at once, that came back as an occasional unexplained failure in
/// whichever test happened to draw the collided number.
///
/// 20000–29999 sits entirely below the floor, and is disjoint from the range
/// `tests/hot.rs` takes for the same reason.
fn test_port(name: &str) -> u16 {
    let hash = name.bytes().fold(0u32, |acc, b| {
        acc.wrapping_mul(31).wrapping_add(u32::from(b))
    });
    20000 + u16::try_from(hash % 10000).unwrap_or(0)
}

/// The port scheme's own invariant — see [`test_port`].
///
/// Two names hashing together is a *permanent* failure in whichever test binds
/// second, and it reads as that test being broken rather than as a name clash.
/// Asserted here so a new one fails with the reason.
#[test]
fn every_named_port_is_its_own_and_below_the_ephemeral_range() {
    // Every name passed to `test_port` in this file.
    const NAMED: &[&str] = &[
        "preview",
        "s_asset",
        "s_broken_app",
        "s_frontend",
        "s_norestart_app",
        "s_plugins",
        "s_refresh",
        "s_takenport",
    ];

    let mut taken: Vec<(u16, &str)> = Vec::new();
    for name in NAMED {
        let port = test_port(name);
        assert!(
            port < 32768,
            "{name}'s port {port} is inside the ephemeral range, where the \
             kernel may already have handed it out to an outbound socket"
        );
        if let Some((_, other)) = taken.iter().find(|(held, _)| *held == port) {
            panic!("{name} and {other} both want port {port} — change the range or a name");
        }
        taken.push((port, name));
    }
}

/// One HTTP GET, spoken by hand — the same shape the dev server answers.
fn http_get(port: u16, path: &str) -> Option<String> {
    use std::io::{Read, Write};

    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("set read timeout");
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )
    .ok()?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response).ok()?;
    Some(String::from_utf8_lossy(&response).into_owned())
}

/// Polls until the server answers, or gives up — a build and a process start
/// have to happen first, and how long that takes is the machine's business.
fn wait_for_http(port: u16, path: &str, done: impl Fn(&str) -> bool) -> String {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let mut last = String::new();
    while std::time::Instant::now() < deadline {
        if let Some(response) = http_get(port, path) {
            last = response;
            if done(&last) {
                return last;
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    last
}

/// Runs `esdev start` with extra flags, keeping its stderr in a file so a test
/// can read what it announced. The port is in there and nowhere else when
/// esdev picked it.
fn start_in_logging(dir: &Path, args: &[&str]) -> (Supervisor, PathBuf) {
    let log = dir.join("esdev.log");
    let file = std::fs::File::create(&log).expect("create the log");
    let mut command = esdev_in(dir);
    command
        .arg("start")
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::from(file));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    (
        Supervisor(Some(command.spawn().expect("spawn esdev start"))),
        log,
    )
}

/// Waits for the `http://127.0.0.1:<port>` esdev printed, and returns the port.
fn announced_port(log: &Path) -> u16 {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        let text = std::fs::read_to_string(log).unwrap_or_default();
        if let Some(at) = text.find("http://127.0.0.1:") {
            let rest = &text[at + "http://127.0.0.1:".len()..];
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            if !digits.is_empty()
                && rest.len() > digits.len()
                && let Ok(port) = digits.parse::<u16>()
            {
                return port;
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!(
        "esdev never announced a port:\n{}",
        std::fs::read_to_string(log).unwrap_or_default()
    );
}

/// **A port nobody named is a convenience, and esdev finds another when the
/// usual one is taken.** Two projects open in two terminals is an ordinary
/// afternoon; refusing to start over a number the developer never chose is the
/// tool inventing a problem.
#[test]
fn start_finds_a_free_port_when_none_was_named() {
    let dir = watch_dir("s_freeport");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(&dir, "src/main.mjs", "document.title = 'FREE';\n");
    write_in(
        &dir,
        "index.html",
        "<!doctype html><html><head>\
         <script type=\"module\" src=\"./src/main.mjs\"></script></head>\
         <body><div id=root></div></body></html>\n",
    );
    // No `port` key, so nothing named one.
    write_in(
        &dir,
        "esdev.json",
        r#"{ "targets": { "web": { "entry": "index.html", "outdir": "dist" } } }"#,
    );

    // The default, held for the length of the test. If this fails, something
    // else on the machine already has it — which is the same precondition.
    let blocker = std::net::TcpListener::bind(("127.0.0.1", 5173));

    let (_supervisor, log) = start_in_logging(&dir, &[]);
    let port = announced_port(&log);
    assert_ne!(port, 5173, "esdev bound the port that was taken");

    let document = wait_for_http(port, "/", |body| body.contains("<div id=root>"));
    assert!(document.contains("200 OK"), "{document}");
    assert!(
        std::fs::read_to_string(&log)
            .unwrap_or_default()
            .contains("5173 was taken"),
        "esdev moved without saying so"
    );

    drop(blocker);
    let _ = std::fs::remove_dir_all(&dir);
}

/// **A port that *was* named is a promise.** Moving quietly off it would leave
/// a bookmark, a proxy rule or a second terminal pointing at whatever is
/// already there, so this fails and says what to do instead.
#[test]
fn start_refuses_a_named_port_that_is_taken() {
    let dir = watch_dir("s_takenport");
    let port = test_port("s_takenport");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(&dir, "src/main.mjs", "document.title = 'X';\n");
    write_in(
        &dir,
        "index.html",
        "<!doctype html><html><body><div id=root></div></body></html>\n",
    );
    write_in(
        &dir,
        "esdev.json",
        r#"{ "targets": { "web": { "entry": "index.html", "outdir": "dist" } } }"#,
    );

    let _held = std::net::TcpListener::bind(("127.0.0.1", port)).expect("hold the port");
    let out = esdev_in(&dir)
        .args(["start", &format!("--port={port}")])
        .output()
        .expect("spawn esdev start");

    assert!(!out.status.success(), "{}", stdout(&out));
    let err = stderr(&out);
    assert!(err.contains(&format!("127.0.0.1:{port}")), "{err}");
    assert!(err.contains("--port"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn start_needs_a_project_to_start() {
    let dir = build_dir("s_noconfig");
    let out = esdev_in(&dir).arg("start").output().expect("spawn esdev");
    assert!(!out.status.success());
    assert!(stderr(&out).contains("esdev.json"), "{}", stderr(&out));
}

/// The dev loop holds one bundler across rebuilds, and the pass that emits an
/// imported file is inside it — so the collector has to be held too, or a
/// rebuild the bundler answered from its cache forgets a file that is still
/// imported and stops writing it.
#[test]
fn the_dev_loop_serves_an_imported_asset_and_maps_its_bundle() {
    let dir = watch_dir("s_asset");
    let port = test_port("s_asset");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    std::fs::write(dir.join("src/logo.png"), b"\x89PNG\r\n\x1a\ndev").expect("write png");
    write_in(
        &dir,
        "src/main.ts",
        "import logo from './logo.png';\n\
         document.body.innerHTML = `<img id=logo src=\"${logo}\">`;\n",
    );
    write_in(
        &dir,
        "index.html",
        "<!doctype html><html><body>\
         <script type=\"module\" src=\"./src/main.ts\"></script></body></html>\n",
    );
    write_in(
        &dir,
        "esdev.json",
        &format!(
            r#"{{ "targets": {{ "web": {{ "entry": "index.html", "outdir": "dist" }} }},
                 "start": {{ "port": {port} }} }}"#
        ),
    );

    let _supervisor = start_in(&dir);
    let document = wait_for_http(port, "/", |body| body.contains("<script"));
    assert!(document.contains("200 OK"), "{document}");

    // The bundle names the file, and the file is there to be fetched.
    let bundle = http_get(port, "/assets/main.js").expect("a bundle");
    let url = bundle
        .split("/assets/logo-")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .map(|name| format!("/assets/logo-{name}"))
        .expect("an asset URL in the bundle");
    let asset = http_get(port, &url).expect("the asset");
    assert!(asset.contains("200 OK"), "{url} was not served:\n{asset}");

    // …and the dev bundle carries its map, so what breaks points at the source.
    assert!(
        bundle.contains("sourceMappingURL=data:application/json"),
        "the dev bundle is unmapped"
    );
}

/// A project with no server of its own: esdev serves the output, falls back to
/// index.html for a client-side route, and reloads on a change.
#[test]
fn start_serves_a_frontend_project_and_reloads_it() {
    let dir = watch_dir("s_frontend");
    let port = test_port("s_frontend");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(&dir, "src/main.mjs", "document.title = 'FIRST';\n");
    write_in(&dir, "styles.css", "body{color:red}\n");
    write_in(
        &dir,
        "index.html",
        "<!doctype html><html><head><link rel=\"stylesheet\" href=\"./styles.css\">\
         <script type=\"module\" src=\"./src/main.mjs\"></script></head>\
         <body><div id=root></div></body></html>\n",
    );
    write_in(
        &dir,
        "esdev.json",
        &format!(
            r#"{{ "targets": {{ "web": {{ "entry": "index.html", "outdir": "dist" }} }},
                 "start": {{ "port": {port} }} }}"#
        ),
    );

    let _supervisor = start_in(&dir);

    let document = wait_for_http(port, "/", |body| body.contains("<div id=root>"));
    assert!(document.contains("200 OK"), "{document}");

    // Dev names are stable — no hash — so a reload keeps its cache and a stack
    // trace stays readable.
    assert!(document.contains(r#"src="/assets/main.js""#), "{document}");
    assert!(
        document.contains(r#"href="/assets/styles.css""#),
        "{document}"
    );
    // The update client is esdev's, and it is in the output only.
    assert!(document.contains("WebSocket"), "{document}");
    assert!(document.contains("/@esdev/hmr"), "{document}");
    assert!(
        !std::fs::read_to_string(dir.join("index.html"))
            .expect("read source")
            .contains("WebSocket"),
        "the source document was written to"
    );

    // The bundle is served, and a client-side route falls back to the document.
    let bundle = http_get(port, "/assets/main.js").unwrap_or_default();
    assert!(bundle.contains("FIRST"), "{bundle}");
    assert!(
        bundle.contains("text/javascript"),
        "served with the wrong type: {bundle}"
    );
    let route = http_get(port, "/about").unwrap_or_default();
    assert!(route.contains("<div id=root>"), "{route}");
    // …but a missing file is missing. HTML answered for a .js is a syntax
    // error three steps from its cause.
    let missing = http_get(port, "/assets/nope.js").unwrap_or_default();
    assert!(missing.contains("404"), "{missing}");

    // A change rebuilds, and the new bundle is what is served.
    write_in(&dir, "src/main.mjs", "document.title = 'SECOND';\n");
    let rebuilt = wait_for_http(port, "/assets/main.js", |body| body.contains("SECOND"));
    assert!(rebuilt.contains("SECOND"), "the change never landed");

    let _ = std::fs::remove_dir_all(&dir);
}

/// **A browser-only edit does not restart the server.** Every rebuild used to
/// SIGTERM the child and start it again, so editing a stylesheet cost every
/// open connection and every warm cache the process had, to deliver a server
/// byte for byte identical to the one just stopped.
///
/// The server is restarted when the build changed something it reads, and the
/// browser is told to reload either way. Proved by a nonce fixed at startup: if
/// the process is the same one, the nonce is the same one.
#[test]
fn a_browser_only_change_reloads_without_restarting_the_server() {
    let dir = watch_dir("s_norestart");
    let served = test_port("s_norestart_app");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    // The nonce is created once, when the module is evaluated. A restart is the
    // only thing that can change it.
    write_in(
        &dir,
        "src/server.mjs",
        &format!(
            "import {{ serve }} from 'runtime:http';\n\
             const nonce = String(Math.random());\n\
             serve({{ port: {served} }}, () => new Response('SERVER-A ' + nonce));\n"
        ),
    );
    write_in(&dir, "src/main.mjs", "document.title = 'CLIENT-ONE';\n");
    write_in(
        &dir,
        "index.html",
        "<!doctype html><html><head>\
         <script type=\"module\" src=\"./src/main.mjs\"></script></head>\
         <body><div id=root></div></body></html>\n",
    );
    write_in(
        &dir,
        "esdev.json",
        &format!(
            r#"{{ "targets": {{
                   "server": {{ "entry": "src/server.mjs", "out": "dist/server.js" }},
                   "web": {{ "entry": "index.html", "outdir": "dist" }} }},
                 "start": {{ "run": "server" }},
                 "permissions": {{ "deny": ["all"], "allow": {{ "listen": ["{served}"] }} }} }}"#
        ),
    );

    let (_supervisor, log) = start_in_logging(&dir, &[]);
    let first = wait_for_http(served, "/", |body| body.contains("SERVER-A"));
    assert!(first.contains("SERVER-A"), "never came up: {first}");
    let nonce = nonce_of(&first);

    // A browser-only edit. The client bundle is rebuilt, under the dev
    // directory like every other development build…
    write_in(&dir, "src/main.mjs", "document.title = 'CLIENT-TWO';\n");
    let bundle = dir.join(".dev/dist/assets/main.js");
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        if std::fs::read_to_string(&bundle)
            .unwrap_or_default()
            .contains("CLIENT-TWO")
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        std::fs::read_to_string(&bundle)
            .unwrap_or_default()
            .contains("CLIENT-TWO"),
        "the client change never rebuilt\n{}",
        std::fs::read_to_string(&log).unwrap_or_default()
    );
    // …and the same process is still answering. Given a moment, because a
    // restart would take one.
    std::thread::sleep(Duration::from_secs(2));
    let after = http_get(served, "/").unwrap_or_default();
    assert!(after.contains("SERVER-A"), "{after}");
    assert_eq!(
        nonce_of(&after),
        nonce,
        "a browser-only change restarted the server"
    );

    // A server edit still restarts it, which is the other half of the promise.
    write_in(
        &dir,
        "src/server.mjs",
        &format!(
            "import {{ serve }} from 'runtime:http';\n\
             const nonce = String(Math.random());\n\
             serve({{ port: {served} }}, () => new Response('SERVER-B ' + nonce));\n"
        ),
    );
    let restarted = wait_for_http(served, "/", |body| body.contains("SERVER-B"));
    assert!(
        restarted.contains("SERVER-B"),
        "a server change did not restart it: {restarted}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The digits after `SERVER-x ` in a response body.
fn nonce_of(response: &str) -> String {
    let at = response.rfind("SERVER-").expect("a marked body");
    response[at..]
        .split_whitespace()
        .nth(1)
        .unwrap_or("")
        .to_string()
}

/// The promise that makes the loop usable: a syntax error mid-edit costs a
/// message, not the server you were about to fix it on.
#[test]
fn a_failed_build_leaves_the_running_server_alone() {
    let dir = watch_dir("s_broken");
    let served = test_port("s_broken_app");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(
        &dir,
        "src/server.mjs",
        &format!(
            "import {{ serve }} from 'runtime:http';\n\
             serve({{ port: {served} }}, () => new Response('ALIVE'));\n"
        ),
    );
    write_in(
        &dir,
        "esdev.json",
        &format!(
            r#"{{ "targets": {{ "server": {{ "entry": "src/server.mjs", "out": "dist/server.js" }} }},
                 "start": {{ "run": "server" }},
                 "permissions": {{ "deny": ["all"], "allow": {{ "listen": ["{served}"] }} }} }}"#
        ),
    );

    let _supervisor = start_in(&dir);
    let alive = wait_for_http(served, "/", |body| body.contains("ALIVE"));
    assert!(alive.contains("ALIVE"), "the server never came up: {alive}");

    // Break it, and give the watcher long enough to have acted.
    write_in(
        &dir,
        "src/server.mjs",
        "import { serve } from 'runtime:http'; serve({\n",
    );
    std::thread::sleep(Duration::from_secs(3));
    let still = http_get(served, "/").unwrap_or_default();
    assert!(
        still.contains("ALIVE"),
        "a failed build took the server down: {still:?}"
    );

    // Fix it, and the fix is what is running.
    write_in(
        &dir,
        "src/server.mjs",
        &format!(
            "import {{ serve }} from 'runtime:http';\n\
             serve({{ port: {served} }}, () => new Response('FIXED'));\n"
        ),
    );
    let fixed = wait_for_http(served, "/", |body| body.contains("FIXED"));
    assert!(fixed.contains("FIXED"), "the fix never landed: {fixed}");

    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// `esdev create` (DECISIONS D65)
//
// The command whose output is somebody else's starting point, so what is worth
// testing is that the project it writes actually works — and that the command
// cannot damage a directory somebody already had something in.
// ---------------------------------------------------------------------------

#[test]
fn create_writes_a_project_that_builds_and_runs() {
    let parent = watch_dir("c_project");
    let dir = parent.join("weather-app");

    let out = esdev_in(&parent)
        .args(["create", "weather-app"])
        .output()
        .expect("spawn esdev create");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("npm install"), "{}", stdout(&out));

    // The name comes from the directory, into the manifest and the document.
    let manifest = std::fs::read_to_string(dir.join("package.json")).expect("read package.json");
    assert!(manifest.contains(r#""name": "weather-app""#), "{manifest}");
    assert!(!manifest.contains("{{name}}"), "a placeholder survived");
    let document = std::fs::read_to_string(dir.join("index.html")).expect("read index.html");
    assert!(
        document.contains("<title>weather-app</title>"),
        "{document}"
    );

    // `_gitignore` is written under the name it has to have — as itself, it
    // would apply to the template in this repository.
    assert!(dir.join(".gitignore").is_file(), "no .gitignore");
    assert!(
        !dir.join("_gitignore").exists(),
        "_gitignore was written as-is"
    );

    // Nothing a local build or install left behind is in the binary.
    assert!(!dir.join("node_modules").exists());
    assert!(!dir.join("dist").exists());

    // The tests it ships pass, which is the smallest end-to-end claim that
    // does not need a package registry.
    let tested = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    assert!(
        tested.status.success(),
        "the template's own tests failed:\n{}{}",
        stdout(&tested),
        stderr(&tested)
    );

    let _ = std::fs::remove_dir_all(&parent);
}

/// It owns nothing it writes into, so a directory with anything in it is
/// refused — and `--force` means "write among what is there", never over it.
#[test]
fn create_refuses_a_directory_that_holds_something() {
    let parent = watch_dir("c_refuse");
    let dir = parent.join("taken");
    std::fs::create_dir_all(&dir).expect("create dir");
    write_in(&dir, "package.json", "{ \"name\": \"mine\" }\n");

    let refused = esdev_in(&parent)
        .args(["create", "taken"])
        .output()
        .expect("spawn esdev create");
    assert!(!refused.status.success());
    assert!(
        stderr(&refused).contains("not empty"),
        "{}",
        stderr(&refused)
    );

    let forced = esdev_in(&parent)
        .args(["create", "taken", "--force"])
        .output()
        .expect("spawn esdev create");
    assert!(forced.status.success(), "{}", stderr(&forced));
    assert!(
        stdout(&forced).contains("left alone"),
        "{}",
        stdout(&forced)
    );
    // The file that was there is the file that is there.
    assert_eq!(
        std::fs::read_to_string(dir.join("package.json")).expect("read"),
        "{ \"name\": \"mine\" }\n"
    );
    // …and the rest of the project was written around it.
    assert!(dir.join("src/routes.tsx").is_file());

    let _ = std::fs::remove_dir_all(&parent);
}

/// Every template, scaffolded and put through its own test suite.
///
/// The one property that matters for a scaffolder and cannot be checked by
/// looking at the files: that what it writes *works*. A template is a project
/// nobody builds until somebody depends on it, which is exactly the kind of
/// thing that rots quietly.
///
/// `react` is the exception, and deliberately: its tests need `node_modules`,
/// and installing from a registry is not something a unit test should do. It is
/// covered by `create_writes_a_project_that_builds_and_runs` instead.
#[test]
fn every_dependency_free_template_passes_its_own_tests() {
    let parent = watch_dir("c_all");

    for template in ["api", "lib", "vanilla"] {
        let dir = parent.join(template);
        let created = esdev_in(&parent)
            .args(["create", template, &format!("--template={template}")])
            .output()
            .expect("spawn esdev create");
        assert!(
            created.status.success(),
            "{}: {}",
            template,
            stderr(&created)
        );

        let tested = esdev_in(&dir)
            .arg("test")
            .output()
            .expect("spawn esdev test");
        assert!(
            tested.status.success(),
            "the {template} template's own tests failed:\n{}{}",
            stdout(&tested),
            stderr(&tested)
        );

        // …and it builds. A template that tests clean and does not build is
        // still a broken starting point.
        let mut build = esdev_in(&dir);
        build.arg("build");
        if template == "lib" {
            // `--lib` is a flag rather than an esdev.json key, so the template
            // carries it in its `build` script rather than its config.
            build.args(["--lib", "src"]);
        }
        let built = build.output().expect("spawn esdev build");
        assert!(
            built.status.success(),
            "the {template} template does not build:\n{}{}",
            stdout(&built),
            stderr(&built)
        );
    }

    let _ = std::fs::remove_dir_all(&parent);
}

/// A mode is the whole project, not a preset on top of one. What you get is the
/// files that mode needs and none of the other's — otherwise a starter ships a
/// server nobody runs and a permission nobody needs, and the person scaffolding
/// is left deleting half of it.
#[test]
fn each_mode_writes_its_own_project_and_none_of_the_other() {
    let parent = watch_dir("c_modes");

    for (mode, mine, theirs) in [
        ("static", "src/prerender.tsx", "src/server.tsx"),
        ("fullstack", "src/server.tsx", "src/prerender.tsx"),
    ] {
        let dir = parent.join(mode);
        let out = esdev_in(&parent)
            .args([
                "create",
                mode,
                "--template=react",
                &format!("--mode={mode}"),
            ])
            .stdin(std::process::Stdio::null())
            .output()
            .expect("spawn esdev create");
        assert!(out.status.success(), "{mode}: {}", stderr(&out));
        assert!(
            stdout(&out).contains(&format!("react ({mode}) template")),
            "{mode}: {}",
            stdout(&out)
        );

        assert!(dir.join(mine).is_file(), "{mode} has no {mine}");
        assert!(
            !dir.join(theirs).exists(),
            "{mode} was written with {theirs}, which belongs to the other mode"
        );
        // The shared half is in both, rather than duplicated into each.
        assert!(dir.join("src/routes.tsx").is_file(), "{mode} has no routes");

        // And the tests it ships pass — the react template's own suite needs no
        // node_modules, which is what makes this checkable here at all.
        let tested = esdev_in(&dir)
            .arg("test")
            .output()
            .expect("spawn esdev test");
        assert!(
            tested.status.success(),
            "the react ({mode}) template's own tests failed:\n{}{}",
            stdout(&tested),
            stderr(&tested)
        );
    }

    // A static project has nothing to run in production, so it grants nothing
    // and names no server; a fullstack one does both.
    let statik = std::fs::read_to_string(parent.join("static/esdev.json")).expect("read");
    assert!(!statik.contains("permissions"), "{statik}");
    // `"run":` rather than `"run"` — the prerender target says `"then": "run"`,
    // which is a target that is executed after the build, not a server.
    assert!(!statik.contains("\"run\":"), "{statik}");
    let full = std::fs::read_to_string(parent.join("fullstack/esdev.json")).expect("read");
    assert!(full.contains("\"run\": \"server\""), "{full}");
    assert!(
        full.contains("--allow") || full.contains("listen"),
        "{full}"
    );

    // Both ways of getting a static build are scripts on the project, so the
    // SSG/SPA choice is made per deploy rather than at scaffold time.
    let scripts = std::fs::read_to_string(parent.join("static/package.json")).expect("read");
    assert!(
        scripts.contains("\"build\": \"esdev build --minify\""),
        "{scripts}"
    );
    assert!(scripts.contains("build:spa"), "{scripts}");

    let _ = std::fs::remove_dir_all(&parent);
}

/// A mode that is not one is refused rather than ignored, and so is a mode on a
/// template that has only one shape — a flag that silently does nothing is one
/// somebody keeps passing, and keeps believing.
#[test]
fn a_mode_that_does_not_exist_is_refused() {
    let parent = watch_dir("c_bad_mode");

    let unknown = esdev_in(&parent)
        .args(["create", "nope", "--template=react", "--mode=ssr"])
        .stdin(std::process::Stdio::null())
        .output()
        .expect("spawn esdev create");
    assert!(!unknown.status.success(), "{}", stdout(&unknown));
    // The message names the modes there are.
    assert!(stderr(&unknown).contains("static"), "{}", stderr(&unknown));
    assert!(
        stderr(&unknown).contains("fullstack"),
        "{}",
        stderr(&unknown)
    );
    assert!(!parent.join("nope").exists(), "it wrote a project anyway");

    let modeless = esdev_in(&parent)
        .args(["create", "nope", "--template=api", "--mode=static"])
        .stdin(std::process::Stdio::null())
        .output()
        .expect("spawn esdev create");
    assert!(!modeless.status.success(), "{}", stdout(&modeless));
    assert!(
        stderr(&modeless).contains("no modes"),
        "{}",
        stderr(&modeless)
    );

    let _ = std::fs::remove_dir_all(&parent);
}

/// A prompt in a script is a script that hangs, which is the whole reason the
/// interactive path is gated. These run with stdin closed — the shape every CI
/// job has — and must answer without asking anything.
#[test]
fn create_never_asks_when_nobody_is_there() {
    let parent = watch_dir("c_quiet");

    // No flags at all: the default template, and nothing installed.
    let out = esdev_in(&parent)
        .args(["create", "quiet"])
        .stdin(std::process::Stdio::null())
        .output()
        .expect("spawn esdev create");
    assert!(out.status.success(), "{}", stderr(&out));
    // The default mode, named — "the react template" is two projects, and a
    // report that does not say which one cannot be checked.
    assert!(
        stdout(&out).contains("react (static) template"),
        "{}",
        stdout(&out)
    );
    // The next steps tell them to install, because this run did not.
    assert!(stdout(&out).contains("npm install"), "{}", stdout(&out));
    assert!(
        !parent.join("quiet/node_modules").exists(),
        "an unattended run installed something"
    );

    // …and nothing was written to the question stream.
    assert!(
        !stderr(&out).contains("Which Template"),
        "it asked anyway:\n{}",
        stderr(&out)
    );

    let _ = std::fs::remove_dir_all(&parent);
}

/// Every question has a flag, so the interactive path is a convenience over the
/// scriptable one rather than the only way to an answer.
#[test]
fn every_question_has_a_flag() {
    let parent = watch_dir("c_flags");

    let out = esdev_in(&parent)
        .args(["create", "flagged", "--template=lib", "--no-install"])
        .stdin(std::process::Stdio::null())
        .output()
        .expect("spawn esdev create");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        parent.join("flagged/src/index.ts").is_file(),
        "not the lib template"
    );

    // The next step is what that template actually has: a library has nothing
    // to `run dev`.
    assert!(stdout(&out).contains("run test"), "{}", stdout(&out));

    // `--yes` is the conventional "take every default", and must not ask
    // either — even where a terminal would have been available.
    let yes = esdev_in(&parent)
        .args(["create", "defaulted", "--yes"])
        .stdin(std::process::Stdio::null())
        .output()
        .expect("spawn esdev create");
    assert!(yes.status.success(), "{}", stderr(&yes));
    assert!(
        stdout(&yes).contains("react (static) template"),
        "{}",
        stdout(&yes)
    );

    // A package manager that is not one is named as such, before anything runs.
    let unknown = esdev_in(&parent)
        .args(["create", "nope", "--install=cargo"])
        .stdin(std::process::Stdio::null())
        .output()
        .expect("spawn esdev create");
    assert!(!unknown.status.success());
    assert!(
        stderr(&unknown).contains("npm, bun, pnpm, yarn"),
        "{}",
        stderr(&unknown)
    );

    let _ = std::fs::remove_dir_all(&parent);
}

#[test]
fn create_lists_its_templates_and_names_one_it_does_not_have() {
    let dir = watch_dir("c_list");

    let listed = esdev_in(&dir)
        .args(["create", "--list"])
        .output()
        .expect("spawn esdev create");
    assert!(listed.status.success(), "{}", stderr(&listed));
    assert!(stdout(&listed).contains("react"), "{}", stdout(&listed));
    assert!(stdout(&listed).contains("micro-ui"), "{}", stdout(&listed));

    let unknown = esdev_in(&dir)
        .args(["create", "app", "--template=svelte"])
        .output()
        .expect("spawn esdev create");
    assert!(!unknown.status.success());
    assert!(
        stderr(&unknown).contains("no svelte template"),
        "{}",
        stderr(&unknown)
    );
    // The error is also the list, so the next command is obvious.
    assert!(stderr(&unknown).contains("react"), "{}", stderr(&unknown));

    let _ = std::fs::remove_dir_all(&dir);
}

/// The OTF Web starter set scaffolds through `esdev create`: four templates,
/// each with the axes `create-web` asks about, as flags. stdin is closed
/// throughout, so every answer here comes from a flag.
#[test]
fn otf_templates_scaffold_from_flags() {
    let parent = watch_dir("c_otf");

    // TypeScript + plain CSS: renames, config, and no Tailwind prepend.
    let ts = esdev_in(&parent)
        .args([
            "create",
            "shop-ts",
            "--template=spa",
            "--language=ts",
            "--styling=css",
        ])
        .stdin(std::process::Stdio::null())
        .output()
        .expect("spawn esdev create");
    assert!(ts.status.success(), "{}", stderr(&ts));
    let dir = parent.join("shop-ts");
    assert!(dir.join("app/page.tsx").is_file());
    assert!(dir.join("tsconfig.json").is_file());
    assert!(dir.join("app/otfw-env.d.ts").is_file());
    assert!(!dir.join("app/page.jsx").exists());
    assert!(!dir.join("jsconfig.json").exists());
    let css = std::fs::read_to_string(dir.join("app/global.css")).expect("read");
    assert!(!css.contains("@import"), "{css}");
    let manifest = std::fs::read_to_string(dir.join("package.json")).expect("read");
    assert!(manifest.contains(r#""name": "shop-ts""#), "{manifest}");
    assert!(!manifest.contains("{{name}}"), "a placeholder survived");

    // Defaults are JavaScript with Tailwind, mirroring `create-web`.
    let js = esdev_in(&parent)
        .args(["create", "shop-js", "--template=fullstack"])
        .stdin(std::process::Stdio::null())
        .output()
        .expect("spawn esdev create");
    assert!(js.status.success(), "{}", stderr(&js));
    let dir = parent.join("shop-js");
    assert!(dir.join("app/page.jsx").is_file());
    assert!(dir.join("app/api/hello/route.js").is_file());
    let css = std::fs::read_to_string(dir.join("app/global.css")).expect("read");
    assert!(
        css.starts_with("@import \"tailwindcss\";"),
        "the Tailwind default prepends: {css}"
    );

    // The blog is files plus two patches — or neither.
    let bare = esdev_in(&parent)
        .args(["create", "manual", "--template=docs", "--no-blog"])
        .stdin(std::process::Stdio::null())
        .output()
        .expect("spawn esdev create");
    assert!(bare.status.success(), "{}", stderr(&bare));
    let dir = parent.join("manual");
    assert!(!dir.join("app/blog").exists(), "the blog was not withheld");
    let config = std::fs::read_to_string(dir.join("otfw.config.js")).expect("read");
    assert!(!config.contains("blog:"), "{config}");

    let blogged = esdev_in(&parent)
        .args(["create", "journal", "--template=docs", "--blog"])
        .stdin(std::process::Stdio::null())
        .output()
        .expect("spawn esdev create");
    assert!(blogged.status.success(), "{}", stderr(&blogged));
    let dir = parent.join("journal");
    assert!(dir.join("app/blog/hello-world/page.mdx").is_file());
    let config = std::fs::read_to_string(dir.join("otfw.config.js")).expect("read");
    assert!(config.contains("dir: \"blog\""), "{config}");

    // The library ships an `esdev test` suite, not a Bun one — and it passes
    // with no dependencies installed.
    let lib = esdev_in(&parent)
        .args(["create", "widgets", "--template=library", "--language=ts"])
        .stdin(std::process::Stdio::null())
        .output()
        .expect("spawn esdev create");
    assert!(lib.status.success(), "{}", stderr(&lib));
    let dir = parent.join("widgets");
    assert!(dir.join("index.ts").is_file());
    assert!(dir.join("src/Counter.tsx").is_file());
    assert!(!dir.join("bunfig.toml").exists(), "Bun-only file shipped");
    assert!(
        stdout(&lib).contains("run test"),
        "a library has nothing to run dev: {}",
        stdout(&lib)
    );
    let tested = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    assert!(
        tested.status.success(),
        "the library template's own tests failed:\n{}{}",
        stdout(&tested),
        stderr(&tested)
    );

    let _ = std::fs::remove_dir_all(&parent);
}

/// An axis flag where it does not apply is refused, like a stray `--mode` —
/// and a value that is not one is refused with the values that are.
#[test]
fn otf_axes_are_refused_where_they_do_not_apply() {
    let parent = watch_dir("c_otf_axes");

    for (template, flag, message) in [
        ("api", "--language=ts", "has no language"),
        ("docs", "--styling=css", "has no styling"),
        ("spa", "--blog", "means nothing for spa"),
        ("spa", "--language=elm", "no language elm"),
        ("spa", "--styling=sass", "no styling sass"),
    ] {
        let dir = parent.join(format!("refused-{template}"));
        let refused = esdev_in(&parent)
            .args([
                "create",
                dir.file_name().unwrap().to_str().unwrap(),
                &format!("--template={template}"),
                flag,
            ])
            .stdin(std::process::Stdio::null())
            .output()
            .expect("spawn esdev create");
        assert!(!refused.status.success(), "{template} {flag} was accepted");
        assert!(
            stderr(&refused).contains(message),
            "{template} {flag}: {}",
            stderr(&refused)
        );
        assert!(!dir.exists(), "it wrote a project anyway");
    }

    // The list names the new templates, so the error above is actionable.
    let listed = esdev_in(&parent)
        .args(["create", "--list"])
        .output()
        .expect("spawn esdev create");
    for template in ["spa", "fullstack", "docs", "library"] {
        assert!(
            stdout(&listed).contains(template),
            "{template} is not listed:\n{}",
            stdout(&listed)
        );
    }

    let _ = std::fs::remove_dir_all(&parent);
}

// ---------------------------------------------------------------------------
// `runtime:watch` — file events in guest JS (the esdev-only module).
// ---------------------------------------------------------------------------

/// The whole point of the module: the program **stays up** across the change
/// and is told what changed, rather than being restarted like `--watch` does.
#[test]
fn runtime_watch_delivers_changes_to_the_program() {
    let dir = watch_dir("w_events");
    write_in(
        &dir,
        "app.mjs",
        r#"
import { watch } from "runtime:watch";
import { write } from "runtime:fs";

const changes = watch(["."], { recursive: true });
setTimeout(() => write("new.txt", "hello"), 300);

for await (const change of changes) {
  if (change.path.endsWith("new.txt")) {
    console.log(change.kind, "seen");
    break;
  }
}
console.log("still running");
"#,
    );

    let out = esdev_in(&dir).arg("app.mjs").output().expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));
    // "created", not "modified": a save of a new file is a create followed by a
    // write, and the burst has to add up to the first of those.
    assert!(stdout(&out).contains("created seen"), "{}", stdout(&out));
    assert!(stdout(&out).contains("still running"), "{}", stdout(&out));

    let _ = std::fs::remove_dir_all(&dir);
}

/// The watch set grows while the watcher runs — the case a dev server needs,
/// because which files a bundle depends on is known only after it is built.
#[test]
fn runtime_watch_takes_new_paths_while_it_runs() {
    let dir = watch_dir("w_add");
    std::fs::create_dir_all(dir.join("lib")).expect("create lib");
    write_in(
        &dir,
        "app.mjs",
        r#"
import { watch } from "runtime:watch";
import { write } from "runtime:fs";

// Opened on the app directory only; `lib` is not watched yet.
const changes = watch(["."]);
await changes.add("lib");
setTimeout(() => write("lib/dep.js", "export const x = 1;"), 300);

for await (const change of changes) {
  if (change.path.endsWith("dep.js")) {
    console.log("saw the added path");
    break;
  }
}
"#,
    );

    let out = esdev_in(&dir).arg("app.mjs").output().expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("saw the added path"),
        "{}",
        stdout(&out)
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Watching is scoped by the same `--allow-read` list as reading, because it
/// answers the same questions: which files exist, and when they change.
#[test]
fn runtime_watch_is_bounded_by_allow_read() {
    let dir = watch_dir("w_scope");
    std::fs::create_dir_all(dir.join("app")).expect("create app");
    std::fs::create_dir_all(dir.join("secrets")).expect("create secrets");
    write_in(
        &dir,
        "app.mjs",
        r#"
import { watch } from "runtime:watch";
try {
  const changes = watch(["secrets"]);
  await changes.next();
  console.log("watched it");
} catch (err) {
  console.log("refused:", err.name);
}
"#,
    );

    let out = esdev_in(&dir)
        .args(["--deny-all", "--allow-read=app", "app.mjs"])
        .output()
        .expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).starts_with("refused:"), "{}", stdout(&out));

    let _ = std::fs::remove_dir_all(&dir);
}

/// `add()` refuses a tree that is already being watched — **including one a
/// recursive watch already covers**, which is the spelling that happens: a dev
/// server watches `app/`, then adds the package `app/` lives in as a
/// dependency. Comparing paths for equality caught the exact repeat and let the
/// overlap through, and an overlap is every event delivered twice on the
/// backends that allow it.
#[test]
fn runtime_watch_does_not_watch_one_tree_twice() {
    let dir = watch_dir("w_overlap");
    std::fs::create_dir_all(dir.join("pkg/src/app/deeper")).expect("create tree");
    write_in(
        &dir,
        "app.mjs",
        r#"
import { watch } from "runtime:watch";
const w = await watch(["pkg/src/app"], { recursive: true });
// Inside what is already watched, and the same path again.
console.log("inside:", await w.add("pkg/src/app/deeper"));
console.log("same:", await w.add("pkg/src/app"));
// Around it: the new watch covers the old one, which stops being its own.
console.log("around:", await w.add("pkg"));
// And now everything under it is covered by that.
console.log("inside again:", await w.add("pkg/src"));
await w.close();
"#,
    );

    let out = esdev_in(&dir).arg("app.mjs").output().expect("spawn esdev");
    let printed = stdout(&out);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(printed.contains("inside: false"), "{printed}");
    assert!(printed.contains("same: false"), "{printed}");
    assert!(printed.contains("around: true"), "{printed}");
    assert!(printed.contains("inside again: false"), "{printed}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// And it is `esdev`'s, not the runtime's: the same program under `esrun` must
/// fail at the import rather than run with a watcher that never fires.
#[test]
fn runtime_watch_does_not_exist_under_esrun() {
    let Some(esrun) = sibling_binary("esrun") else {
        eprintln!("skipping: esrun is not built in this target dir");
        return;
    };
    let dir = watch_dir("w_esrun");
    write_in(&dir, "app.mjs", "import 'runtime:watch';\n");

    // From the project, as a program is run: the root is the working
    // directory's project (D79), and an entry outside it is refused.
    let out = Command::new(esrun)
        .current_dir(&dir)
        .arg("app.mjs")
        .output()
        .expect("spawn esrun");
    assert!(!out.status.success(), "{}", stdout(&out));
    assert!(
        stderr(&out).contains("unknown built-in module"),
        "{}",
        stderr(&out)
    );

    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// `esdev.json`'s `plugins` — a project's own passes, in the subcommand's build
//
// Until these, a project that compiled anything the toolchain does not know
// about could only be built by a *program* that called `build()` itself:
// `esdev build` and `esdev start` had nowhere to put a plugin. What is under
// test here is the whole path — the file names a module, esdev evaluates it in
// an isolate of its own, and the hooks it declared run against the bundle the
// subcommand is making.
// ---------------------------------------------------------------------------

/// The plugin the tests below load: a virtual module whose contents come from
/// the options the config passed to the factory. A factory rather than a plain
/// object on purpose — the call is the thing a JSON config cannot make for
/// itself.
fn banner_plugin(dir: &Path) {
    write_in(
        dir,
        "plugin.mjs",
        r#"
export default function banner(options) {
  const text = options?.text ?? "NO OPTIONS";
  return {
    name: "banner",
    resolve: {
      filter: { id: "virtual:banner" },
      handler: () => ({ id: "virtual:banner", virtual: true }),
    },
    load: {
      filter: { id: "virtual:banner" },
      handler: () => ({ code: `export default ${JSON.stringify(text)};` }),
    },
  };
}
"#,
    );
}

/// The whole feature: the config names a module, esdev imports it, calls the
/// factory with the options the file carried, and the plugin serves a module
/// that exists on no disk — for **every** target, because a project's plugins
/// are the project's.
#[test]
fn build_loads_the_plugins_the_project_config_names() {
    let dir = build_dir("p_plugins");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    banner_plugin(&dir);
    write_in(
        &dir,
        "src/server.mjs",
        "import banner from 'virtual:banner';\nconsole.log('server', banner);\n",
    );
    write_in(
        &dir,
        "src/client.mjs",
        "import banner from 'virtual:banner';\nconsole.log('client', banner);\n",
    );
    write_in(
        &dir,
        "esdev.json",
        r#"{
          "plugins": [{ "module": "./plugin.mjs", "options": { "text": "FROM THE CONFIG" } }],
          "targets": {
            "server":  { "entry": "src/server.mjs", "out": "dist/server.js" },
            "browser": { "entry": "src/client.mjs", "outdir": "dist/client",
                         "platform": "browser" }
          }
        }"#,
    );

    let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));

    let server = std::fs::read_to_string(dir.join("dist/server.js")).expect("the server bundle");
    assert!(server.contains("FROM THE CONFIG"), "{server}");
    let client =
        std::fs::read_to_string(dir.join("dist/client/client.js")).expect("the browser bundle");
    assert!(client.contains("FROM THE CONFIG"), "{client}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// A target's own `plugins` add to the project's rather than replacing them,
/// and only that target gets them.
#[test]
fn a_targets_own_plugins_are_added_to_the_projects() {
    let dir = build_dir("p_plugins_target");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    banner_plugin(&dir);
    write_in(
        &dir,
        "plugin-extra.mjs",
        r#"
export default {
  name: "extra",
  resolve: {
    filter: { id: "virtual:extra" },
    handler: () => ({ id: "virtual:extra", virtual: true }),
  },
  load: {
    filter: { id: "virtual:extra" },
    handler: () => ({ code: 'export default "EXTRA";' }),
  },
};
"#,
    );
    write_in(
        &dir,
        "src/one.mjs",
        "import banner from 'virtual:banner';\nimport extra from 'virtual:extra';\n\
         console.log(banner, extra);\n",
    );
    // The second target gets the project's plugin only, and reaching for the
    // first target's would be a build that fails — which is the assertion.
    write_in(
        &dir,
        "src/two.mjs",
        "import banner from 'virtual:banner';\nconsole.log(banner);\n",
    );
    write_in(
        &dir,
        "esdev.json",
        r#"{
          "plugins": [{ "module": "./plugin.mjs", "options": { "text": "SHARED" } }],
          "targets": {
            "one": { "entry": "src/one.mjs", "out": "dist/one.js",
                     "plugins": ["./plugin-extra.mjs"] },
            "two": { "entry": "src/two.mjs", "out": "dist/two.js" }
          }
        }"#,
    );

    let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));

    let one = std::fs::read_to_string(dir.join("dist/one.js")).expect("one");
    assert!(one.contains("SHARED"), "{one}");
    assert!(one.contains("EXTRA"), "{one}");
    let two = std::fs::read_to_string(dir.join("dist/two.js")).expect("two");
    assert!(two.contains("SHARED"), "{two}");
    assert!(
        !two.contains("EXTRA"),
        "a target's plugin reached another: {two}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// **`order: "pre"` is ordered against the built-in passes, not only against
/// the other plugins.** A Tailwind compiler claims `.css` so that
/// `@import "tailwindcss"` never reaches `esdev:css-modules`, which would treat
/// it as a file to fetch and fail with "which is not there".
///
/// Two things have to hold for that, and this is a regression test for both:
/// the plugin's transform runs first, and the built-in pass then **steps
/// aside** — it filters on the id, which is still `.css`, and would otherwise
/// re-read the stylesheet off disk and undo the whole thing.
#[test]
fn a_pre_ordered_plugin_claims_css_ahead_of_the_built_in_pass() {
    let dir = build_dir("p_plugins_pre");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(
        &dir,
        "plugin.mjs",
        r#"
export default {
  name: "tailwind-ish",
  transform: {
    order: "pre",
    filter: { id: /\.css$/ },
    handler: (code, id, ctx) => ({
      code: `export default { button: "COMPILED-${ctx.type}" };`,
      type: "js",
    }),
  },
};
"#,
    );
    // The `@import` names a package, which is exactly what the built-in CSS
    // pass cannot resolve — so if it ever sees this file, the build fails.
    write_in(
        &dir,
        "src/ui.css",
        "@import \"tailwindcss\";\n.button { color: red }\n",
    );
    write_in(
        &dir,
        "src/app.mjs",
        "import styles from './ui.css';\nconsole.log(styles.button);\n",
    );
    write_in(
        &dir,
        "esdev.json",
        r#"{
          "plugins": ["./plugin.mjs"],
          "targets": { "app": { "entry": "src/app.mjs", "out": "dist/app.js" } }
        }"#,
    );

    let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    let bundle = std::fs::read_to_string(dir.join("dist/app.js")).expect("the bundle");
    // `ctx.type` is what the module was when the hook was called — the extension
    // said `.css`, and nothing had changed it yet.
    assert!(bundle.contains("COMPILED-css"), "{bundle}");
    assert!(
        !bundle.contains("color: red"),
        "the built-in pass re-read the stylesheet: {bundle}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A stylesheet nobody claimed is still the built-in pass's, so stepping aside
/// for a plugin did not quietly turn CSS Modules off.
#[test]
fn css_modules_still_scopes_a_stylesheet_no_plugin_claimed() {
    let dir = build_dir("p_plugins_css_intact");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    banner_plugin(&dir);
    write_in(&dir, "src/ui.module.css", ".button { color: red }\n");
    write_in(
        &dir,
        "src/app.mjs",
        "import banner from 'virtual:banner';\nimport styles from './ui.module.css';\n\
         console.log(banner, styles.button);\n",
    );
    write_in(
        &dir,
        "esdev.json",
        r#"{
          "plugins": [{ "module": "./plugin.mjs", "options": { "text": "B" } }],
          "targets": { "app": { "entry": "src/app.mjs", "out": "dist/app.js" } }
        }"#,
    );

    let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    let bundle = std::fs::read_to_string(dir.join("dist/app.js")).expect("the bundle");
    // The scoped name the CSS Modules pass mints — the source class mapped to a
    // name derived from the file's path, which is neither the class alone nor
    // anything a plugin here produced.
    assert!(
        bundle.contains("\"button\": \"button_"),
        "the class was not scoped: {bundle}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A plugin that cannot be loaded fails the build **where it was named**, not
/// as a module-not-found inside a program nobody typed.
#[test]
fn a_plugin_that_cannot_be_loaded_is_reported_against_the_config() {
    let dir = build_dir("p_plugins_missing");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(&dir, "src/app.mjs", "console.log('hi');\n");
    write_in(
        &dir,
        "esdev.json",
        r#"{
          "plugins": ["./plugins/not-here.mjs"],
          "targets": { "app": { "entry": "src/app.mjs", "out": "dist/app.js" } }
        }"#,
    );

    let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
    assert!(!out.status.success(), "{}{}", stdout(&out), stderr(&out));
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(text.contains("./plugins/not-here.mjs"), "{text}");
    assert!(!dir.join("dist/app.js").exists(), "a failed build wrote");

    let _ = std::fs::remove_dir_all(&dir);
}

/// A module that exports no plugin is named too — the module, and which export
/// was looked for.
#[test]
fn a_plugin_module_with_no_export_says_so() {
    let dir = build_dir("p_plugins_noexport");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(&dir, "plugin.mjs", "export const other = 1;\n");
    write_in(&dir, "src/app.mjs", "console.log('hi');\n");
    write_in(
        &dir,
        "esdev.json",
        r#"{
          "plugins": ["./plugin.mjs"],
          "targets": { "app": { "entry": "src/app.mjs", "out": "dist/app.js" } }
        }"#,
    );

    let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
    assert!(!out.status.success(), "{}{}", stdout(&out), stderr(&out));
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(text.contains("no default export"), "{text}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// A project with no `plugins` starts no isolate and builds exactly as it
/// always did — the cost of the feature is paid only by projects that use it.
#[test]
fn a_project_without_plugins_is_unchanged() {
    let dir = build_dir("p_plugins_none");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(&dir, "src/app.mjs", "console.log('plain');\n");
    write_in(
        &dir,
        "esdev.json",
        r#"{ "targets": { "app": { "entry": "src/app.mjs", "out": "dist/app.js" } } }"#,
    );

    let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    let bundle = std::fs::read_to_string(dir.join("dist/app.js")).expect("the bundle");
    assert!(bundle.contains("plain"), "{bundle}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// The case the whole feature was asked for: a configured plugin reading the
/// **chunk graph** and writing a route manifest. `end` fires before there are
/// chunks, and until `esdev.json` had a plugin slot the only way to reach the
/// graph was for the framework to call `build()` itself.
#[test]
fn a_configured_plugin_reads_the_chunk_graph_and_writes_a_manifest() {
    let dir = build_dir("p_plugins_bundle");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(&dir, "src/shared.mjs", "export const shared = 1;\n");
    write_in(
        &dir,
        "src/app.mjs",
        "import { shared } from './shared.mjs';\nconsole.log(shared);\n",
    );
    write_in(
        &dir,
        "plugin.mjs",
        r#"
import { write } from "runtime:fs";
export default {
  name: "manifest",
  bundle: {
    handler: async (output) => {
      const entry = output.find((o) => o.type === "chunk" && o.isEntry);
      // `facadeModuleId` is the module the chunk *is*, and `imports` are the
      // chunks a preload would have to fetch beside it.
      await write(
        "manifest.json",
        JSON.stringify({
          file: entry.fileName,
          module: entry.facadeModuleId.endsWith("app.mjs"),
          modules: entry.moduleIds.length,
          imports: entry.imports,
        }),
      );
    },
  },
};
"#,
    );
    write_in(
        &dir,
        "esdev.json",
        r#"{
          "plugins": ["./plugin.mjs"],
          "targets": { "app": { "entry": "src/app.mjs", "out": "dist/app.js" } }
        }"#,
    );

    let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    let manifest = std::fs::read_to_string(dir.join("manifest.json")).expect("the manifest");
    assert!(manifest.contains("\"module\":true"), "{manifest}");
    assert!(manifest.contains("\"file\":\"app.js\""), "{manifest}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// `host()` is esdev's seam, not an API a program that bundles should reach
/// for. Called with nothing driving a build from outside, it says which one is
/// which rather than waiting for ever on a build nobody started.
#[test]
fn runtime_build_host_is_refused_when_nothing_is_driving_a_build() {
    let dir = build_dir("rb_host_alone");
    write_in(
        &dir,
        "app.mjs",
        "import { host } from 'runtime:build';\n\
         try { await host([]); console.log('RESOLVED'); }\n\
         catch (err) { console.log('refused:', err.message); }\n",
    );

    let out = esdev_in(&dir).arg("app.mjs").output().expect("spawn esdev");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(out.status.success(), "{text}");
    assert!(text.contains("refused:"), "{text}");
    assert!(text.contains("build() instead"), "{text}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// A hot-reload scheme, written the way esdev expects one: **as a plugin**.
///
/// esdev implements none and knows the name of none. It provides the generic
/// half — `import.meta.hot`, the update channel, and the compiler's component
/// registrations on request — and a framework's scheme is this.
fn refresh_plugin(dir: &Path) {
    std::fs::create_dir_all(dir.join("plugins")).expect("create plugins");
    write_in(
        dir,
        "plugins/refresh.mjs",
        r#"
export default {
  name: "otfw-refresh",
  // The half a plugin cannot do for itself: finding the components to register
  // needs the syntax tree the compiler already has.
  jsx: { refresh: true },
  transform: {
    filter: { id: /\.[jt]sx?$/ },
    handler(code, id, ctx) {
      // A wrapper that makes every module a hot boundary is exactly wrong in
      // anything shipped, so it goes in only where a scheme was named and the
      // loop is running it hot.
      if (ctx.refresh !== "otfw") return null;
      return { code: `globalThis.__otfw_hot = ${JSON.stringify(ctx.refresh)};\n${code}` };
    },
  },
};
"#,
    );
}

fn refresh_project(dir: &Path, port: Option<u16>) {
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    refresh_plugin(dir);
    // JSX, so the compiler's half has a component to register.
    write_in(
        dir,
        "src/Home.jsx",
        "export function Home() {\n  return <h1>hi</h1>;\n}\n",
    );
    write_in(
        dir,
        "src/main.jsx",
        "import { Home } from './Home.jsx';\nglobalThis.Home = Home;\n",
    );
    write_in(
        dir,
        "index.html",
        "<!doctype html><html><head>\
         <script type=\"module\" src=\"./src/main.jsx\"></script></head>\
         <body><div id=root></div></body></html>\n",
    );
    let start = port.map_or_else(String::new, |port| {
        format!(r#", "start": {{ "port": {port} }}"#)
    });
    write_in(
        dir,
        "esdev.json",
        &format!(
            r#"{{ "jsx": {{ "factory": "h" }},
                 "targets": {{ "web": {{ "entry": "index.html", "outdir": "dist",
                                        "refresh": "otfw",
                                        "plugins": ["./plugins/refresh.mjs"] }} }}{start} }}"#
        ),
    );
}

/// A release build names no scheme, so the wrapper is not installed. Without
/// this half, `refresh` would be a config key a plugin had to ignore — it would
/// have to inject its wrapper into everything, including what you ship.
#[test]
fn a_release_build_tells_a_plugin_no_refresh_scheme() {
    let dir = build_dir("p_refresh_release");
    refresh_project(&dir, None);

    let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));

    let bundle = std::fs::read_dir(dir.join("dist/assets"))
        .expect("the assets")
        .filter_map(Result::ok)
        .map(|entry| std::fs::read_to_string(entry.path()).unwrap_or_default())
        .collect::<String>();
    assert!(bundle.contains("Home"), "nothing was built");
    assert!(
        !bundle.contains("__otfw_hot"),
        "a release build installed a hot-reload wrapper: {bundle}"
    );
    // The compiler's half is gated on the same thing, and has to be: the
    // registrations reach globals only a hot loop installs, so shipping them
    // would ship calls to something undefined.
    assert!(
        !bundle.contains("$RefreshReg$"),
        "a release build got the compiler's registrations: {bundle}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// **A hot-reload scheme is a plugin, and esdev knows nothing about any of
/// them.** `"react"` was for a while the only name the config would take *and*
/// the only implementation, both inside esdev — so every other framework took a
/// full page reload on each edit. Both halves a scheme needs are generic now:
/// the plugin is told which scheme (`ctx.refresh`) and the compiler is asked
/// for the component registrations by the plugin's own declaration.
#[test]
fn the_hot_dev_loop_tells_a_plugin_which_refresh_scheme() {
    let dir = watch_dir("s_refresh");
    let port = test_port("s_refresh");
    refresh_project(&dir, Some(port));

    let _supervisor = start_in(&dir);

    let bundle = wait_for_http(port, "/assets/main.js", |body| body.contains("__otfw_hot"));
    // The plugin's half: it was told which scheme, so it installed its wrapper.
    assert!(
        bundle.contains(r#"__otfw_hot = "otfw""#),
        "the scheme never reached the plugin: {bundle}"
    );
    // The compiler's half: `jsx: { refresh: true }` on the plugin's own
    // declaration asked for the registrations, and nothing in esdev knows what
    // framework asked or why.
    assert!(
        bundle.contains("$RefreshReg$(_c,"),
        "the compiler never inserted the registrations: {bundle}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The dev loop, with a plugin. Two things are under test that a single build
/// cannot show:
///
/// * the plugin still applies on a **rebuild** — the isolate holding it is
///   started once and kept, because evaluating a plugin's module (and whatever
///   it initialises) on every save would be a startup cost per keystroke;
/// * a plugin holding **state across builds** works, which is what every
///   incremental compiler is. The counter here stands in for one: it is the
///   plugin's own module scope, and a fresh isolate per build would reset it.
#[test]
fn start_keeps_the_projects_plugins_across_rebuilds() {
    let dir = watch_dir("s_plugins");
    let port = test_port("s_plugins");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(
        &dir,
        "plugin.mjs",
        r#"
// Module scope: it survives for as long as the isolate does, and a host that
// reloaded the plugin per build would hand back 1 every time.
let builds = 0;
export default {
  name: "counter",
  resolve: {
    filter: { id: "virtual:builds" },
    handler: () => ({ id: "virtual:builds", virtual: true }),
  },
  load: {
    filter: { id: "virtual:builds" },
    handler: () => ({ code: `export default ${++builds};` }),
  },
};
"#,
    );
    write_in(
        &dir,
        "src/main.mjs",
        "import builds from 'virtual:builds';\ndocument.title = 'FIRST build ' + builds;\n",
    );
    write_in(
        &dir,
        "index.html",
        "<!doctype html><html><head>\
         <script type=\"module\" src=\"./src/main.mjs\"></script></head>\
         <body><div id=root></div></body></html>\n",
    );
    write_in(
        &dir,
        "esdev.json",
        &format!(
            r#"{{ "plugins": ["./plugin.mjs"],
                 "targets": {{ "web": {{ "entry": "index.html", "outdir": "dist" }} }},
                 "start": {{ "port": {port} }} }}"#
        ),
    );

    let _supervisor = start_in(&dir);

    let bundle = wait_for_http(port, "/assets/main.js", |body| body.contains("FIRST build"));
    assert!(bundle.contains("FIRST build"), "{bundle}");
    assert!(bundle.contains("build 1"), "the plugin never ran: {bundle}");

    // A save rebuilds. The plugin applies again — and its counter has moved,
    // which it could not have done if the isolate holding it were new.
    write_in(
        &dir,
        "src/main.mjs",
        "import builds from 'virtual:builds';\ndocument.title = 'SECOND build ' + builds;\n",
    );
    let rebuilt = wait_for_http(port, "/assets/main.js", |body| {
        body.contains("SECOND build")
    });
    assert!(rebuilt.contains("SECOND build"), "the change never landed");
    assert!(
        !rebuilt.contains("build 1"),
        "the plugin was reloaded for the rebuild: {rebuilt}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// `runtime:build` — the bundler, from guest JS (the other esdev-only module).
// ---------------------------------------------------------------------------

/// A plugin can see what the build **produced**. `end` fires when the module
/// graph is finished, before there are chunks at all — which is why it is
/// handed `null` — so route-level modulepreload had no seam to hang on unless
/// the program itself called `build()`.
#[test]
fn runtime_build_hands_a_plugin_the_chunks_it_produced() {
    let dir = build_dir("rb_bundle");
    write_in(&dir, "dep.js", "export const x = 1;\n");
    write_in(
        &dir,
        "main.js",
        "import { x } from './dep.js';\nglobalThis.lazy = () => import('./dep.js');\nconsole.log(x);\n",
    );
    write_in(
        &dir,
        "app.mjs",
        r#"
import { build } from "runtime:build";
let seen = null;
const plugin = {
  name: "preload",
  bundle: { handler: (output) => { seen = output; } },
};
const bundle = await build({ input: "main.js", plugins: [plugin] });
await bundle.generate({ format: "esm" });
const entry = seen.find((o) => o.type === "chunk" && o.isEntry);
console.log(
  seen.length > 0,
  entry.fileName,
  entry.facadeModuleId.endsWith("main.js"),
  entry.moduleIds.some((id) => id.endsWith("dep.js")),
  entry.code === undefined,
);
"#,
    );

    let out = esdev_in(&dir).arg("app.mjs").output().expect("spawn esdev");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(out.status.success(), "{text}");
    assert!(text.contains("true main.js true true true"), "{text}");
}

/// A failed build says **where**. It used to be a string — the module id, a
/// colon and "Unexpected token" — so an overlay could name the file and then
/// had to stop; the bundler computed the line, the column and the frame all
/// along for its own terminal output.
#[test]
fn runtime_build_reports_where_a_build_failed() {
    let dir = build_dir("rb_failure");
    write_in(&dir, "broken.js", "export const a = 1;\nconst b = ;\n");
    write_in(
        &dir,
        "app.mjs",
        r#"
import { build } from "runtime:build";
const bundle = await build({ input: "broken.js" });
try {
  await bundle.generate({ format: "esm" });
  console.log("NO ERROR");
} catch (err) {
  const [first] = err.errors;
  console.log(err.name, err.errors.length, first.kind, first.line, first.column);
  console.log(first.id.endsWith("broken.js"), first.frame.includes("const b = ;"));
}
"#,
    );

    let out = esdev_in(&dir).arg("app.mjs").output().expect("spawn esdev");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(out.status.success(), "{text}");
    assert!(text.contains("BuildError 1 PARSE_ERROR 2 10"), "{text}");
    assert!(text.contains("true true"), "{text}");
}

/// The whole feature in one test: a plugin serving a **virtual module**
/// (`resolveId` + `load`), a `transform`, an `external` **predicate**, output
/// held in memory, and `watchFiles` covering both what was imported and what a
/// plugin declared. None of that is reachable through a subprocess protocol,
/// which is why the bridge exists.
#[test]
fn runtime_build_bundles_with_a_js_plugin() {
    let dir = build_dir("rb_plugin");
    write_in(&dir, "dep.js", "export const answer = 42;\n");
    write_in(
        &dir,
        "main.js",
        "import { answer } from './dep.js';\nimport hello from 'virtual:greeting';\nconsole.log(hello, answer);\n",
    );
    write_in(
        &dir,
        "app.mjs",
        r#"
import { build } from "runtime:build";

let transformCalls = 0;

const plugin = {
  name: "greeting",
  // A module that exists on no disk. `virtual: true` rather than the NUL-byte
  // prefix every bundler inherited from rollup.
  resolve: {
    filter: { id: "virtual:greeting" },
    handler: () => ({ id: "virtual:greeting", virtual: true }),
  },
  load: {
    filter: { id: "virtual:greeting" },
    // A dependency the graph cannot discover: nothing imports dep.js *from
    // here*, but this module is built from it. Returned, not declared by a
    // call that can be forgotten.
    handler: () => ({ code: 'export default "hello";', dependsOn: ["dep.js"] }),
  },
  transform: {
    // The filter is matched on the host side, so this handler is entered once
    // — not once per module in the graph.
    filter: { id: /dep\.js$/ },
    handler(code, id, ctx) {
      transformCalls++;
      return { code: code.replace("42", "43") };
    },
  },
};

const bundle = await build({
  input: "main.js",
  plugins: [plugin],
  external: (id) => id.startsWith("runtime:"),
});

const { output, watchFiles } = await bundle.generate({ format: "esm", codeSplitting: false });
console.log("chunks", output.length);
console.log("crossings", transformCalls);
console.log(output[0].code.includes('console.log("hello", 43)') ? "transformed" : output[0].code);
console.log("watched", watchFiles.some((f) => f.endsWith("dep.js")));
await bundle.close();
"#,
    );

    let out = esdev_in(&dir).arg("app.mjs").output().expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));
    let printed = stdout(&out);
    assert!(printed.contains("chunks 1"), "{printed}");
    // The filter is the difference between one crossing into the isolate and
    // one per module in the graph.
    assert!(printed.contains("crossings 1"), "{printed}");
    assert!(printed.contains("transformed"), "{printed}");
    assert!(printed.contains("watched true"), "{printed}");
}

/// A hook's `this` is the bundler's own context, mid-build: `this.resolve()`
/// asks its resolver, `this.emitFile()` adds to a build already running, and
/// `this.warn()` reaches the caller instead of vanishing into a worker thread.
#[test]
fn runtime_build_hooks_get_the_bundlers_context() {
    let dir = build_dir("rb_context");
    write_in(&dir, "dep.js", "export const answer = 42;\n");
    write_in(
        &dir,
        "app.mjs",
        r#"
import { build } from "runtime:build";

// Arrow functions throughout: the context is the last argument, not `this`,
// so an arrow cannot silently lose it.
const plugin = {
  name: "probe",
  resolve: {
    filter: { id: "virtual:entry" },
    handler: async (source, importer, ctx) => {
      const found = await ctx.resolve("./dep.js", importer ?? undefined);
      console.log("resolved", found !== null && found.id.endsWith("dep.js"));
      ctx.warn("a warning from the plugin");
      return { id: "virtual:entry", virtual: true };
    },
  },
  load: {
    filter: { id: "virtual:entry" },
    handler: (id, ctx) => {
      const ref = ctx.emit({ type: "asset", name: "meta.json", source: '{"ok":true}' });
      console.log("emitted", typeof ref === "string");
      return { code: "export default 1;" };
    },
  },
};

const bundle = await build({ input: "virtual:entry", plugins: [plugin] });
const { output, warnings } = await bundle.generate({});
console.log("assets", output.some((o) => o.type === "asset"));
console.log("warned", warnings.some((w) => w.includes("a warning from the plugin")));
await bundle.close();
"#,
    );

    let out = esdev_in(&dir).arg("app.mjs").output().expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));
    let printed = stdout(&out);
    for expected in [
        "resolved true",
        "emitted true",
        "assets true",
        "warned true",
    ] {
        assert!(
            printed.contains(expected),
            "{expected} missing from {printed}"
        );
    }
}

/// The declaration is checked where it is written. There is **one** way to
/// declare a hook, and rollup's bare-function shorthand is refused rather than
/// quietly accepted — accepting it would make the filter, the order and the
/// context argument optional extras on somebody else's design.
#[test]
fn runtime_build_refuses_a_malformed_plugin() {
    let dir = build_dir("rb_strict");
    write_in(&dir, "main.js", "console.log(1);\n");
    write_in(
        &dir,
        "app.mjs",
        r#"
import { build } from "runtime:build";

const refuse = async (plugin, what) => {
  try {
    await build({ input: "main.js", plugins: [plugin] });
    console.log("NOT REFUSED", what);
  } catch (err) {
    console.log(what, "|", err.message);
  }
};

await refuse({ name: "legacy", transform(code, id) {} }, "bare");
await refuse({ name: "typo", tranform: { handler() {} } }, "typo");
await refuse({ name: "wide", start: { filter: { id: /x/ }, handler() {} } }, "filtered-start");
await refuse({ name: "codef", load: { filter: { code: /x/ }, handler() {} } }, "code-on-load");
await refuse({ name: "ord", transform: { order: "first", handler() {} } }, "order");
await refuse({ name: "none", transform: { filter: { id: /x/ } } }, "no-handler");
// A filter with no key this contract knows would be handed the whole graph,
// so it is refused where it was written rather than honoured as a catch-all.
await refuse({ name: "bare", transform: { filter: /x/, handler() {} } }, "bare-pattern");
await refuse({ name: "typo", transform: { filter: { ID: /x/ }, handler() {} } }, "filter-typo");
await refuse(
  { name: "rollup", transform: { filter: { include: /x/ }, handler() {} } },
  "rollup-key",
);
await refuse({ name: "empty", transform: { filter: {}, handler() {} } }, "empty-filter");
await refuse({ name: "none", transform: { filter: { id: [] }, handler() {} } }, "empty-list");
"#,
    );

    let out = esdev_in(&dir).arg("app.mjs").output().expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));
    let printed = stdout(&out);
    assert!(!printed.contains("NOT REFUSED"), "{printed}");
    // Each rejection says what to write instead, and a misspelling names the
    // hook it was nearly.
    assert!(
        printed.contains("a hook is an object, not a function"),
        "{printed}"
    );
    assert!(printed.contains(r#"Did you mean "transform""#), "{printed}");
    assert!(printed.contains("cannot be filtered"), "{printed}");
    assert!(
        printed.contains("only transform can filter on code"),
        "{printed}"
    );
    assert!(printed.contains(r#""pre" or "post""#), "{printed}");
    assert!(printed.contains("handler must be a function"), "{printed}");
    // The 0.4 spelling names the field it is missing, rather than becoming a
    // filter that matches everything.
    assert!(printed.contains("write { id: /x/ }"), "{printed}");
    assert!(
        printed.contains(r#"unknown key "ID". Did you mean "id"?"#),
        "{printed}"
    );
    assert!(printed.contains(r#"unknown key "include""#), "{printed}");
    assert!(printed.contains("must say what it matches"), "{printed}");
    assert!(printed.contains("empty list"), "{printed}");
}

/// `order` decides which plugin sees a module first — the thing a framework
/// needs when one pass has to run before another.
#[test]
fn runtime_build_runs_hooks_in_the_order_they_asked_for() {
    let dir = build_dir("rb_order");
    write_in(&dir, "main.js", "console.log(1);\n");
    write_in(
        &dir,
        "app.mjs",
        r#"
import { build } from "runtime:build";

const seen = [];
const at = (name, order) => ({
  name,
  transform: {
    filter: { id: /main\.js$/ },
    order,
    handler: () => { seen.push(name); return null; },
  },
});

const bundle = await build({
  input: "main.js",
  plugins: [at("normal"), at("pre", "pre"), at("post", "post")],
});
await bundle.generate({});
console.log("order", seen.join(","));
"#,
    );

    let out = esdev_in(&dir).arg("app.mjs").output().expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("order pre,normal,post"),
        "{}",
        stdout(&out)
    );
}

/// A `dependsOn` written the way every other path in a run is written —
/// relative — has to land in `watchFiles` as the same absolute path the graph
/// reports for it. Otherwise the same file appears twice, once as the graph
/// found it and once as the plugin spelled it, and a consumer matching a change
/// against its dependency set misses half the time.
#[test]
fn runtime_build_resolves_a_relative_dependency() {
    let dir = build_dir("rb_deps");
    write_in(&dir, "dep.js", "export const x = 1;\n");
    write_in(&dir, "main.js", "console.log(1);\n");
    write_in(
        &dir,
        "app.mjs",
        r#"
import { build } from "runtime:build";

const bundle = await build({
  input: "main.js",
  plugins: [{
    name: "deps",
    transform: {
      filter: { id: /main\.js$/ },
      handler: (code) => ({ code, dependsOn: ["dep.js"] }),
    },
  }],
});
const { watchFiles } = await bundle.generate({});
const dep = watchFiles.filter((f) => f.endsWith("dep.js"));
console.log("absolute", dep.length === 1 && /^(?:[A-Za-z]:[\\/]|\/)/.test(dep[0]));
"#,
    );

    let out = esdev_in(&dir).arg("app.mjs").output().expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("absolute true"), "{}", stdout(&out));
}

/// The passes this toolchain owns are installed in a guest build too, and this
/// is the regression test for the day they were not: `esdev build` scoped
/// `styles.button` and `runtime:build` did not, so the same project produced
/// markup that did not match its own stylesheet depending on which path built
/// it. Both must arrive at the identical scoped name.
#[test]
fn runtime_build_runs_the_same_owned_passes_as_the_subcommand() {
    let dir = build_dir("rb_css");
    write_in(&dir, "s.module.css", ".button { color: red; }\n");
    write_in(
        &dir,
        "app-entry.js",
        "import styles from './s.module.css';\nconsole.log(styles.button);\n",
    );
    write_in(
        &dir,
        "app.mjs",
        r#"
import { build } from "runtime:build";
const bundle = await build({ input: "app-entry.js" });
const { output } = await bundle.generate({});
const name = /button_[a-f0-9]+/.exec(output[0].code);
console.log("scoped", name === null ? "none" : name[0]);
"#,
    );

    let from_module = esdev_in(&dir).arg("app.mjs").output().expect("spawn esdev");
    assert!(from_module.status.success(), "{}", stderr(&from_module));

    let from_subcommand = esdev_in(&dir)
        .args(["build", "app-entry.js", "--out=out/x.js"])
        .output()
        .expect("spawn esdev build");
    assert!(
        from_subcommand.status.success(),
        "{}",
        stderr(&from_subcommand)
    );

    let written = std::fs::read_to_string(dir.join("out/x.js")).expect("read the bundle");
    let subcommand_name = regex_find(&written);
    let module_name = stdout(&from_module)
        .lines()
        .find_map(|l| l.strip_prefix("scoped ").map(str::to_string))
        .expect("the module build printed a name");
    assert_eq!(
        module_name, subcommand_name,
        "the two build paths disagree about the scoped name"
    );
}

/// The first `button_<hash>` in some text. Written by hand rather than with a
/// regex crate: this file drives a binary, and a test dependency to find eight
/// hex digits is not worth the graph.
fn regex_find(text: &str) -> String {
    let at = text.find("button_").expect("a scoped name in the bundle");
    let rest = &text[at..];
    let end = rest
        .char_indices()
        .find(|(i, c)| *i > 7 && !c.is_ascii_hexdigit())
        .map_or(rest.len(), |(i, _)| i);
    rest[..end].to_string()
}

/// Writes a project with two packages a bundler can only resolve correctly if
/// it asserts something about where the output runs: one that offers three
/// builds of itself behind `exports` conditions, and one old enough to have no
/// `exports` map at all.
fn write_resolution_fixture(dir: &Path) {
    let dual = dir.join("node_modules/dual");
    std::fs::create_dir_all(&dual).expect("create the dual package");
    write_in(
        &dual,
        "package.json",
        r#"{
  "name": "dual",
  "version": "1.0.0",
  "type": "module",
  "exports": {
    ".": {
      "worker": "./worker.js",
      "browser": "./browser.js",
      "default": "./node.js"
    }
  }
}
"#,
    );
    write_in(&dual, "worker.js", "export const where = 'worker-build';\n");
    write_in(
        &dual,
        "browser.js",
        "export const where = 'browser-build';\n",
    );
    write_in(&dual, "node.js", "export const where = 'node-build';\n");

    let legacy = dir.join("node_modules/legacy");
    std::fs::create_dir_all(&legacy).expect("create the legacy package");
    write_in(
        &legacy,
        "package.json",
        r#"{
  "name": "legacy",
  "version": "1.0.0",
  "module": "./index.mjs",
  "main": "./index.cjs"
}
"#,
    );
    write_in(
        &legacy,
        "index.mjs",
        "export const legacy = 'legacy-esm';\n",
    );
    write_in(
        &legacy,
        "index.cjs",
        "module.exports = { legacy: 'legacy-cjs' };\n",
    );

    write_in(
        dir,
        "app-entry.js",
        "import { where } from 'dual';\nimport { legacy } from 'legacy';\nconsole.log(where, legacy);\n",
    );
}

/// **A guest build asserts what the subcommand asserts.** The two used to
/// disagree: `esdev build` names the `worker` condition and the `module`/`main`
/// fields, and `runtime:build` named neither unless the caller did — so the
/// same project resolved to a package's `node:` build one way and its Web build
/// the other. Nothing fails at build time when that happens; the bundle is
/// produced and dies later on an import this runtime does not have.
#[test]
fn runtime_build_asserts_the_same_conditions_as_the_subcommand() {
    let dir = build_dir("rb_conditions");
    write_resolution_fixture(&dir);
    write_in(
        &dir,
        "app.mjs",
        r#"
import { build } from "runtime:build";
const bundle = await build({ input: "app-entry.js" });
const { output } = await bundle.generate({});
console.log("code", JSON.stringify(output[0].code));
"#,
    );

    let from_module = esdev_in(&dir).arg("app.mjs").output().expect("spawn esdev");
    assert!(from_module.status.success(), "{}", stderr(&from_module));
    let from_module = stdout(&from_module);

    let from_subcommand = esdev_in(&dir)
        .args(["build", "app-entry.js", "--out=out/x.js"])
        .output()
        .expect("spawn esdev build");
    assert!(
        from_subcommand.status.success(),
        "{}",
        stderr(&from_subcommand)
    );
    let written = std::fs::read_to_string(dir.join("out/x.js")).expect("read the bundle");

    for (what, code) in [("runtime:build", &from_module), ("esdev build", &written)] {
        assert!(
            code.contains("worker-build"),
            "{what} resolved `dual` to the wrong build: {code}"
        );
        assert!(
            !code.contains("node-build"),
            "{what} took the package's Node build: {code}"
        );
        assert!(
            code.contains("legacy-esm"),
            "{what} could not resolve a package with no `exports` map: {code}"
        );
    }
}

/// A browser build takes the `browser` key rather than the `worker` one, on
/// both paths. Asserting `worker` for a browser hands over a build written for
/// somewhere with no `document`, and the failure is in someone's browser.
#[test]
fn runtime_build_asserts_browser_for_a_browser_platform() {
    let dir = build_dir("rb_conditions_browser");
    write_resolution_fixture(&dir);
    write_in(
        &dir,
        "app.mjs",
        r#"
import { build } from "runtime:build";
const bundle = await build({ input: "app-entry.js", platform: "browser" });
const { output } = await bundle.generate({});
console.log("code", JSON.stringify(output[0].code));
"#,
    );

    let run = esdev_in(&dir).arg("app.mjs").output().expect("spawn esdev");
    assert!(run.status.success(), "{}", stderr(&run));
    let code = stdout(&run);
    assert!(code.contains("browser-build"), "{code}");
    assert!(!code.contains("worker-build"), "{code}");
}

/// Naming a condition adds to what we assert rather than replacing it. A caller
/// that wants `development` should not lose the condition that decides which
/// half of React it gets.
#[test]
fn runtime_build_appends_the_callers_conditions() {
    let dir = build_dir("rb_conditions_extra");
    write_resolution_fixture(&dir);
    write_in(
        &dir,
        "app.mjs",
        r#"
import { build } from "runtime:build";
const bundle = await build({
  input: "app-entry.js",
  resolve: { conditionNames: ["development"] },
});
const { output } = await bundle.generate({});
console.log("code", JSON.stringify(output[0].code));
"#,
    );

    let run = esdev_in(&dir).arg("app.mjs").output().expect("spawn esdev");
    assert!(run.status.success(), "{}", stderr(&run));
    let code = stdout(&run);
    assert!(code.contains("worker-build"), "{code}");
}

/// **This toolchain's own passes go through the same contract.** The CSS
/// Modules pass used to be written against the bundler's trait, which meant the
/// contract had one implementation and no way to check that it was a contract
/// at all. It is a `Pass` now, and this is the observable consequence: it
/// returns the stylesheets it read through `dependsOn`, so they arrive in the
/// guest's `watchFiles` beside everything the module graph found by itself.
///
/// Nothing imports an `@import`ed stylesheet or a `composes … from` target —
/// the reference is inside the CSS — so before this a save to either rebuilt
/// nothing and the page kept the rules it had.
#[test]
fn runtime_build_watches_the_stylesheets_a_css_module_read() {
    let dir = build_dir("rb_css_deps");
    write_in(&dir, "base.css", ".shared { padding: 4px; }\n");
    write_in(
        &dir,
        "shared.module.css",
        ".pill { border-radius: 999px; }\n",
    );
    write_in(
        &dir,
        "s.module.css",
        "@import \"./base.css\";\n.button { composes: pill from \"./shared.module.css\"; color: red; }\n",
    );
    write_in(
        &dir,
        "app-entry.js",
        "import styles from './s.module.css';\nconsole.log(styles.button);\n",
    );
    write_in(
        &dir,
        "app.mjs",
        r#"
import { build } from "runtime:build";
const bundle = await build({ input: "app-entry.js" });
const { watchFiles } = await bundle.generate({});
for (const file of watchFiles) console.log("watch", file);
"#,
    );

    let run = esdev_in(&dir).arg("app.mjs").output().expect("spawn esdev");
    assert!(run.status.success(), "{}", stderr(&run));
    let watched = stdout(&run);
    assert!(
        watched.contains("base.css"),
        "the @import'ed stylesheet is not watched: {watched}"
    );
    assert!(
        watched.contains("shared.module.css"),
        "the composed module is not watched: {watched}"
    );
}

/// A plugin that throws fails the build **with its own message**. The hook ran
/// on a different thread from the bundler; an error that arrived as "build
/// failed" would be the worst possible outcome of that.
#[test]
fn runtime_build_reports_what_a_plugin_threw() {
    let dir = build_dir("rb_throw");
    write_in(&dir, "main.js", "console.log(1);\n");
    write_in(
        &dir,
        "app.mjs",
        r#"
import { build } from "runtime:build";
const bundle = await build({
  input: "main.js",
  plugins: [
    { name: "boom", transform: { handler() { throw new Error("plugin exploded"); } } },
  ],
});
try {
  await bundle.generate({});
  console.log("NOT REACHED");
} catch (err) {
  console.log(String(err.message).includes("plugin exploded") ? "reported" : err.message);
}
"#,
    );

    let out = esdev_in(&dir).arg("app.mjs").output().expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("reported"), "{}", stdout(&out));
}

/// A plugin failure says **where**: the module the hook was called about, and
/// the plugin that reported it.
///
/// The backend carries neither. A hook returns an `anyhow::Error` — that is the
/// whole of the error type its signatures have — and what comes back out is a
/// diagnostic with `id: null`, `plugin: null`, a message that is the anyhow
/// chain (`plugin `x` threw an error / Caused by: …`) around a stack through
/// `runtime:build`'s own dispatcher, and a "frame" that is the same sentence a
/// second time behind a banner. An editor overlay had nothing to point at.
#[test]
fn runtime_build_says_which_module_a_plugin_failed_in() {
    let dir = build_dir("rb_blame");
    write_in(&dir, "main.js", "import \"./dep.js\";\n");
    write_in(&dir, "dep.js", "export const x = 1;\n");
    write_in(
        &dir,
        "app.mjs",
        r#"
import { build } from "runtime:build";

const reported = {
  name: "otfw",
  transform: {
    filter: { id: /dep\.js$/ },
    handler(code, id, ctx) {
      ctx.error("cannot compile this route");
    },
  },
};
const crashed = {
  name: "otfw",
  transform: { filter: { id: /dep\.js$/ }, handler() { return nope.missing; } },
};

for (const [what, plugin] of [["reported", reported], ["crashed", crashed]]) {
  const bundle = await build({ input: "main.js", plugins: [plugin] });
  try {
    await bundle.generate({});
    console.log("NOT REACHED");
  } catch (err) {
    for (const e of err.errors) {
      const first = e.message.split(String.fromCharCode(10))[0];
      console.log(what, "|", e.plugin, "|", (e.id ?? "").split(/[\\\\/]/).pop(), "|", e.frame, "|", first);
    }
  }
  await bundle.close();
}
"#,
    );

    let out = esdev_in(&dir).arg("app.mjs").output().expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));
    let printed = stdout(&out);
    // What ctx.error() said, attributed — and nothing else. Not a stack through
    // the dispatcher that carried it, and not a frame that is the message again.
    assert!(
        printed.contains("reported | otfw | dep.js | null | cannot compile this route"),
        "{printed}"
    );
    // A plugin that *crashed* is a different failure and keeps its stack: the
    // first frame is the line in the plugin, which is the whole of what helps.
    assert!(
        printed.contains("crashed | otfw | dep.js | null | ReferenceError: nope is not defined"),
        "{printed}"
    );
    assert!(!printed.contains("Caused by"), "{printed}");
}

/// Building reads, so it needs `FileRead`; writing the result out needs
/// `FileWrite` as well, and refusing one must not refuse the other.
#[test]
fn runtime_build_is_gated_on_the_filesystem_capabilities() {
    let dir = build_dir("rb_caps");
    write_in(&dir, "main.js", "console.log(1);\n");
    write_in(
        &dir,
        "app.mjs",
        r#"
import { build } from "runtime:build";
const bundle = await build({ input: "main.js" });
try {
  await bundle.write({ dir: "out" });
  console.log("wrote");
} catch (err) {
  console.log("refused", err.name);
}
const { output } = await bundle.generate({});
console.log("generated", output.length === 1);
"#,
    );

    let denied = esdev_in(&dir)
        .args(["--deny-write", "app.mjs"])
        .output()
        .expect("spawn esdev");
    assert!(denied.status.success(), "{}", stderr(&denied));
    assert!(
        stdout(&denied).contains("refused NotAllowedError"),
        "{}",
        stdout(&denied)
    );
    // The same run still builds: the two grants are separate.
    assert!(
        stdout(&denied).contains("generated true"),
        "{}",
        stdout(&denied)
    );

    let all_denied = esdev_in(&dir)
        .args(["--deny-all", "app.mjs"])
        .output()
        .expect("spawn esdev");
    assert!(!all_denied.status.success(), "{}", stdout(&all_denied));
    assert!(
        stderr(&all_denied).contains("capability denied: FileRead"),
        "{}",
        stderr(&all_denied)
    );
}

/// And, like the watcher, it is `esdev`'s: a production binary that could
/// bundle would have to carry a bundler.
#[test]
fn runtime_build_does_not_exist_under_esrun() {
    let Some(esrun) = sibling_binary("esrun") else {
        eprintln!("skipping: esrun is not built in this target dir");
        return;
    };
    let dir = build_dir("rb_esrun");
    let app = write_in(&dir, "app.mjs", "import 'runtime:build';\n");

    let out = Command::new(esrun)
        .current_dir(app.parent().expect("fixture dir"))
        .arg(app.file_name().expect("entry"))
        .output()
        .expect("spawn esrun");
    assert!(!out.status.success(), "{}", stdout(&out));
    assert!(
        stderr(&out).contains("unknown built-in module"),
        "{}",
        stderr(&out)
    );
}

/// The two modules are one feature used from two sides, and this is the shape
/// they exist for: a server that bundles a route on demand, keeps the chunk,
/// serves it, and on a save drops **only** the routes that used the changed
/// file — all while staying up. Every part of that is happening at once here,
/// which is the integration the design is really making a claim about: the
/// bundler's hooks run in the same isolate that is answering the requests.
#[test]
fn a_dev_server_can_bundle_watch_and_serve_at_once() {
    let dir = watch_dir("rb_devserver");
    write_in(&dir, "dep.js", "export const answer = 42;\n");
    write_in(
        &dir,
        "main.js",
        "import { answer } from './dep.js';\nconsole.log(answer);\n",
    );
    write_in(
        &dir,
        "app.mjs",
        r#"
import { build } from "runtime:build";
import { watch } from "runtime:watch";
import { serve } from "runtime:http";
import { write } from "runtime:fs";

const cache = new Map();

// A hook that awaits: the bundler must wait for this isolate, and this isolate
// must go on answering requests while it does.
const slow = {
  name: "slow",
  transform: {
    filter: { id: /\.js$/ },
    handler: async () => {
      await new Promise((r) => setTimeout(r, 5));
      return null;
    },
  },
};

async function bundleRoute(route) {
  const bundle = await build({ input: route, plugins: [slow] });
  const { output, watchFiles } = await bundle.generate({ codeSplitting: false });
  await bundle.close();
  const entry = { code: output[0].code, deps: new Set(watchFiles) };
  cache.set(route, entry);
  return entry;
}

const server = serve({ port: 0 }, async (req) => {
  const route = new URL(req.url).pathname === "/dep" ? "dep.js" : "main.js";
  const entry = cache.get(route) ?? (await bundleRoute(route));
  return new Response(entry.code);
});
const { port } = await server.addr;

const changes = watch(["."], { recursive: true });
(async () => {
  for await (const { path } of changes) {
    for (const [route, entry] of cache) {
      if (entry.deps.has(path)) cache.delete(route);
    }
  }
})();

const one = await (await fetch(`http://127.0.0.1:${port}/`)).text();
const two = await (await fetch(`http://127.0.0.1:${port}/dep`)).text();
console.log("served", one.includes("console.log") && two.includes("answer"));
console.log("cached", cache.size === 2);

await write("dep.js", "export const answer = 44;\n");
for (let i = 0; i < 40 && cache.size === 2; i++) {
  await new Promise((r) => setTimeout(r, 100));
}
// Only what depended on dep.js was dropped — dep.js's own route and main.js's
// both did, but nothing cleared the map wholesale.
console.log("invalidated", cache.size < 2);

const rebuilt = await (await fetch(`http://127.0.0.1:${port}/`)).text();
console.log("rebuilt", rebuilt.includes("44"));

await changes.close();
await server.stop();
"#,
    );

    let out = esdev_in(&dir).arg("app.mjs").output().expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));
    let printed = stdout(&out);
    for expected in [
        "served true",
        "cached true",
        "invalidated true",
        "rebuilt true",
    ] {
        assert!(
            printed.contains(expected),
            "{expected} missing from {printed}"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}
