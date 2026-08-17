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

// ---------------------------------------------------------------------------
// `esdev build` (DECISIONS D59)
//
// The property worth testing is not "a bundler bundles" — rolldown has its own
// suite for that. It is the four settings that make this a command rather than
// a note telling people which flags to pass, each of which fails *silently*
// when wrong.
// ---------------------------------------------------------------------------

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
        .arg(&bundle)
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
        .arg(dir.join("dist/app.js"))
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
            .arg("--allow-imports")
            .arg(dir.join("app.mjs"))
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
        .arg(dir.join("dist/app.js"))
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
        .arg("--deny-all")
        .arg(dir.join("app.mjs"))
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
        .arg("--deny-all")
        .arg(dir.join("dist/app.js"))
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
        .arg("--allow-imports")
        .arg(dir.join("consumer.mjs"))
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
    let message = stderr(&out);
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
    assert!(message.contains("--dts-bundle"), "{message}");

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

    let out = Command::new(esrun).arg(&app).output().expect("spawn esrun");
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
    assert!(stderr(&out).contains("no test files"), "{}", stderr(&out));
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

/// `esdev start` runs the application as a child of its own, so killing only
/// the supervisor leaves that child holding a port — and, having inherited the
/// test harness's stdout, holding the harness open too. The whole group goes.
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
fn start_in(dir: &Path) -> std::process::Child {
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
    command.spawn().expect("spawn esdev start")
}

/// A port unlikely to collide with anything else on the machine, derived from
/// the test's own name so two tests never pick the same one.
fn test_port(name: &str) -> u16 {
    let hash = name.bytes().fold(0u32, |acc, b| {
        acc.wrapping_mul(31).wrapping_add(u32::from(b))
    });
    20000 + u16::try_from(hash % 20000).unwrap_or(0)
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
fn start_in_logging(dir: &Path, args: &[&str]) -> (std::process::Child, PathBuf) {
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
    (command.spawn().expect("spawn esdev start"), log)
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

    let (mut child, log) = start_in_logging(&dir, &[]);
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

    stop_supervisor(&mut child);
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

    let mut child = start_in(&dir);

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

    stop_supervisor(&mut child);
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

    let (mut child, log) = start_in_logging(&dir, &[]);
    let first = wait_for_http(served, "/", |body| body.contains("SERVER-A"));
    assert!(first.contains("SERVER-A"), "never came up: {first}");
    let nonce = nonce_of(&first);

    // A browser-only edit. The client bundle is rebuilt…
    write_in(&dir, "src/main.mjs", "document.title = 'CLIENT-TWO';\n");
    let bundle = dir.join("dist/assets/main.js");
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

    stop_supervisor(&mut child);
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

    let mut child = start_in(&dir);
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

    stop_supervisor(&mut child);
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
        !stderr(&out).contains("Which template"),
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

/// And it is `esdev`'s, not the runtime's: the same program under `esrun` must
/// fail at the import rather than run with a watcher that never fires.
#[test]
fn runtime_watch_does_not_exist_under_esrun() {
    let Some(esrun) = sibling_binary("esrun") else {
        eprintln!("skipping: esrun is not built in this target dir");
        return;
    };
    let dir = watch_dir("w_esrun");
    let app = write_in(&dir, "app.mjs", "import 'runtime:watch';\n");

    let out = Command::new(esrun).arg(&app).output().expect("spawn esrun");
    assert!(!out.status.success(), "{}", stdout(&out));
    assert!(
        stderr(&out).contains("unknown built-in module"),
        "{}",
        stderr(&out)
    );

    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// `runtime:build` — the bundler, from guest JS (the other esdev-only module).
// ---------------------------------------------------------------------------

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
console.log("absolute", dep.length === 1 && dep[0].startsWith("/"));
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

    let out = Command::new(esrun).arg(&app).output().expect("spawn esrun");
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
