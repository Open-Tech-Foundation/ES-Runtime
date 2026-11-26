//! TypeScript and JSX (D59).

use super::*;

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
