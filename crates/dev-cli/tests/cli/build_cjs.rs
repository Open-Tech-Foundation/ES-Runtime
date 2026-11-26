//! `--format=cjs`: the same library, for consumers who are not on this runtime.

use super::*;

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
    let note = stderr(&out);
    assert!(note.contains("runtime:fs"), "{note}");

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
    assert!(message.contains("INVALID_TLA_FORMAT"), "{message}");
    assert!(
        message.to_lowercase().contains("top-level await"),
        "{message}"
    );
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
        r#"{"build": {"targets": {"web": {"entry": "src/app.ts", "outDir": "dist"}, "prerender": {"entry": "src/prerender.ts", "out": "dist/prerender.js", "then": "run"}}}}"#,
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

/// The dev loop writes to its own output directory as it builds, so its error
/// must not promise that nothing was written. The deployment directory remains
/// separate and untouched.
#[test]
fn a_failed_dev_build_says_its_output_may_be_partial() {
    let dir = watch_dir("s_failed_build_note");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(&dir, "src/main.ts", "document.title = 'working';\n");
    write_in(&dir, "src/broken.ts", "const broken = ;\n");
    write_in(
        &dir,
        "index.html",
        "<!doctype html><html><body><script type=\"module\" src=\"./src/main.ts\"></script></body></html>\n",
    );
    write_in(
        &dir,
        "esdev.json",
        r#"{"build":{"targets":{"app":{"entry":"index.html","outDir":"dist"},"z-broken":{"entry":"src/broken.ts","out":"dist/broken.js"}}},"dev":{"servers":{"web":{"target":"app"}}}}"#,
    );

    let (_supervisor, log) = start_in_logging(&dir, &[]);
    let _port = announced_port(&log);
    let err = std::fs::read_to_string(&log).expect("read esdev log");
    assert!(
        err.contains("The dev output directory may contain files"),
        "{err}"
    );
    assert!(
        err.contains("deployment output, which was not changed"),
        "{err}"
    );
    assert!(!err.contains("Nothing was written"), "{err}");
    assert!(dir.join(".dev/dist/index.html").exists(), "{err}");
    assert!(!dir.join("dist/index.html").exists(), "{err}");
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
        r#"{"build": {"targets": {"first": {"entry": "src/first.ts", "out": "dist/first.js"}, "second": {"entry": "src/second.ts", "out": "dist/second.js"}}}}"#,
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
        r#"{"build": {"targets": {"server": {"entry": "src/server.ts", "out": "dist/server.js"}, "web": {"entry": "src/app.ts", "outDir": "dist"}}}}"#,
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
    // sits inside the directory the `outDir` target cleared.
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
        r#"{"build": {"targets": {"server": {"entry": "src/server.ts", "out": "dist/server.js"}, "web": {"entry": "src/app.ts", "outDir": "dist"}}}}"#,
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
        r#"{"build": {"targets": {"web": {"entry": "src/app.ts", "outDir": "."}}}}"#,
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
