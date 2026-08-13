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
    write_in(&dir, "ok.test.mjs", "test('passes', () => assert(true));\n");
    write_in(
        &dir,
        "bad.test.mjs",
        "test('fails', () => assertEquals(1, 2));\n",
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
        "test('sync', () => assert(true));\n\
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
        "import { add } from './math.ts';\n\
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
/// developer wrote, not a line the injected harness moved it to.
#[test]
fn a_failure_names_the_line_the_developer_wrote() {
    let dir = build_dir("t_lines");
    write_in(
        &dir,
        "lines.test.mjs",
        "test('fails on line three', () => {\n  const x = 1;\n  assert(x === 2, 'nope');\n});\n",
    );
    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(!out.status.success());
    assert!(
        text.contains("lines.test.mjs:3:"),
        "the harness renumbered the file:\n{text}"
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
        "import { exit } from 'runtime:process';\ntest('bails', () => exit(3));\n",
    );
    write_in(
        &dir,
        "b_fine.test.mjs",
        "test('fine', () => assert(true));\n",
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
    write_in(&dir, "alpha.test.mjs", "test('a', () => assert(true));\n");
    write_in(&dir, "beta.test.mjs", "test('b', () => assert(true));\n");

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
