//! `esdev build` (D59).

use super::*;

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
        r#"{"build": {"targets": {"lib": {"entry": "src", "lib": true, "outDir": "dist", "format": ["esm", "cjs"], "assets": ["README.md"]}}}}"#,
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
                r#"{{ "build": {{ "targets": {{ "app": {{ "entry": "src/app.ts",
                     "out": "dist/app.js", "{key}": {value} }} }} }} }}"#
            ),
        );
        let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
        assert!(!out.status.success(), "{key} was accepted");
        let err = stderr(&out);
        assert!(err.contains(key) && err.contains("lib"), "{err}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// `new URL("./x.js", import.meta.url)` names a module the build has to emit
/// (D134): a `Worker`'s entry, and the module a durable worker's shards import.
/// Both are built as chunks of their own, the URLs point at them, and the
/// bundle runs under esrun from its own directory.
#[test]
fn build_emits_the_modules_a_new_url_names() {
    let dir = build_dir("b_module_url");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(
        &dir,
        "src/w.js",
        "self.onmessage = (e) => postMessage(`${e.data}!`);\n",
    );
    write_in(
        &dir,
        "src/workers.ts",
        "import { DurableWorker } from \"runtime:workers\";\n\
         export class Room extends DurableWorker {\n\
           static durableName = \"Room\";\n\
           hi(): string { return \"hi\"; }\n\
         }\n",
    );
    write_in(
        &dir,
        "src/server.js",
        "import { configure, shutdown } from \"runtime:workers\";\n\
         import { Room } from \"./workers.ts\";\n\
         const w = new Worker(new URL(\"./w.js\", import.meta.url));\n\
         const echoed = await new Promise((r) => { w.onmessage = (e) => r(e.data); w.postMessage(\"echo\"); });\n\
         configure({ shards: 1, module: new URL(\"./workers.ts\", import.meta.url) });\n\
         console.log(echoed, await Room.get(\"r\").hi());\n\
         w.terminate();\n\
         await shutdown();\n",
    );

    let out = esdev_in(&dir)
        .args(["build", "src/server.js", "--out=dist/server.js"])
        .output()
        .expect("spawn esdev build");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(dir.join("dist/server.js").exists(), "{}", stdout(&out));
    let bundle = std::fs::read_to_string(dir.join("dist/server.js")).expect("read bundle");
    assert!(
        !bundle.contains("./w.js"),
        "the source path survived:\n{bundle}"
    );
    assert!(
        !bundle.contains("./workers.ts"),
        "the source path survived:\n{bundle}"
    );

    let Some(esrun) = sibling_binary("esrun") else {
        eprintln!("skipping: esrun is not built beside esdev");
        return;
    };
    let run = Command::new(esrun)
        .current_dir(&dir)
        .args([
            "--allow-read",
            "--allow-write",
            "--allow-imports",
            "--allow-workers",
            "dist/server.js",
        ])
        .output()
        .expect("spawn esrun");
    assert!(run.status.success(), "{}", stderr(&run));
    assert_eq!(stdout(&run).trim(), "echo! hi");
}

/// `new URL("./x.json", import.meta.url)` naming a file that is not a module
/// copies it into the output (D155): hashed, beside the bundle, the URL
/// rewritten to it — so a server reads its data file from `dist/`. A path that
/// names nothing on disk, and one with a query, are left as written.
#[test]
fn build_copies_the_files_a_new_url_names() {
    let dir = build_dir("b_asset_url");
    std::fs::create_dir_all(dir.join("src/data")).expect("create src");
    write_in(&dir, "src/data/greeting.json", "{\"greeting\":\"hi\"}\n");
    write_in(
        &dir,
        "src/server.js",
        "import { file } from \"runtime:fs\";\n\
         const url = new URL(\"./data/greeting.json\", import.meta.url);\n\
         const later = new URL(\"./made-at-run-time.txt\", import.meta.url);\n\
         const query = new URL(\"./data/greeting.json?v=1\", import.meta.url);\n\
         console.log(JSON.parse(await file(url).text()).greeting, later.pathname.endsWith(\"/made-at-run-time.txt\"), query.search);\n",
    );

    let out = esdev_in(&dir)
        .args(["build", "src/server.js", "--out=dist/server.js"])
        .output()
        .expect("spawn esdev build");
    assert!(out.status.success(), "{}", stderr(&out));
    let bundle = std::fs::read_to_string(dir.join("dist/server.js")).expect("read bundle");
    assert!(
        !bundle.contains("\"./data/greeting.json\""),
        "the source path survived:\n{bundle}"
    );
    assert!(
        bundle.contains("./made-at-run-time.txt"),
        "a path to nothing on disk is left as written:\n{bundle}"
    );
    assert!(
        bundle.contains("./data/greeting.json?v=1"),
        "a path with a query is left as written:\n{bundle}"
    );
    let copied: Vec<String> = std::fs::read_dir(dir.join("dist/assets"))
        .expect("an assets directory beside the bundle")
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .collect();
    assert!(
        copied.len() == 1 && copied[0].starts_with("greeting-") && copied[0].ends_with(".json"),
        "one hashed copy: {copied:?}"
    );

    let Some(esrun) = sibling_binary("esrun") else {
        eprintln!("skipping: esrun is not built beside esdev");
        return;
    };
    let run = Command::new(esrun)
        .current_dir(&dir)
        .args(["--allow-read", "dist/server.js"])
        .output()
        .expect("spawn esrun");
    assert!(run.status.success(), "{}", stderr(&run));
    assert_eq!(stdout(&run).trim(), "hi true ?v=1");
}

/// The same in a page: the file lands beside the chunk that names it, in the
/// document's assets directory rather than a nested one, and the chunk's URL
/// is relative to itself.
#[test]
fn build_copies_a_new_url_file_beside_a_page_chunk() {
    let dir = build_dir("b_asset_url_page");
    std::fs::create_dir_all(dir.join("src/img")).expect("create src");
    write_in(
        &dir,
        "index.html",
        "<!doctype html><html><body><script type=\"module\" src=\"./src/main.js\"></script></body></html>\n",
    );
    write_in(
        &dir,
        "src/img/icon.svg",
        "<svg xmlns=\"http://www.w3.org/2000/svg\"/>\n",
    );
    write_in(
        &dir,
        "src/main.js",
        "const icon = new URL(\"./img/icon.svg\", import.meta.url);\n\
         document.body.append(Object.assign(document.createElement(\"img\"), { src: icon.href }));\n",
    );
    write_in(
        &dir,
        "esdev.json",
        "{ \"build\": { \"targets\": { \"web\": { \"entry\": \"index.html\", \"outDir\": \"dist\" } } } }\n",
    );

    let out = esdev_in(&dir)
        .arg("build")
        .output()
        .expect("spawn esdev build");
    assert!(out.status.success(), "{}", stderr(&out));
    let names: Vec<String> = std::fs::read_dir(dir.join("dist/assets"))
        .expect("dist/assets")
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .collect();
    let icon = names
        .iter()
        .find(|n| n.starts_with("icon-") && n.ends_with(".svg"))
        .unwrap_or_else(|| panic!("no hashed icon beside the chunk: {names:?}"));
    assert!(
        !dir.join("dist/assets/assets").exists(),
        "a nested assets directory"
    );
    let chunk = names
        .iter()
        .find(|n| n.starts_with("main-") && n.ends_with(".js"))
        .expect("the page chunk");
    let code = std::fs::read_to_string(dir.join("dist/assets").join(chunk)).expect("chunk");
    assert!(
        code.contains(&format!("new URL(\"{icon}\", import.meta.url)")),
        "the chunk names the copy relative to itself:\n{code}"
    );
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

/// Four CommonJS packages: one compiled from ES modules, two where one
/// `require`s the other's state, and one that requires a Node builtin on a
/// branch only a call reaches.
fn commonjs_packages(name: &str) -> PathBuf {
    let dir = build_dir(name);
    let package = |name: &str, index: &str| {
        let at = dir.join("node_modules").join(name);
        std::fs::create_dir_all(&at).expect("mkdir");
        write_in(
            &at,
            "package.json",
            &format!(r#"{{"name":"{name}","version":"1.0.0","main":"index.js"}}"#),
        );
        write_in(&at, "index.js", index);
    };
    package(
        "greeter",
        "\"use strict\";\n\
         Object.defineProperty(exports, \"__esModule\", { value: true });\n\
         exports.greet = (name) => 'hi ' + name;\n\
         exports.default = 'the default';\n",
    );
    package("shared-state", "module.exports = { count: 0 };\n");
    package(
        "counter",
        "const state = require('shared-state');\n\
         module.exports = { bump: () => ++state.count };\n",
    );
    package(
        "needs-fs",
        "exports.read = () => require('fs').readFileSync('x');\n",
    );
    write_in(&dir, "package.json", r#"{"name":"app"}"#);
    dir
}

/// A test imports a CommonJS package unbundled: its names link, its default is
/// what a bundler makes it, a package it requires is the same instance the
/// test imports, and a builtin it cannot have fails only when it is reached.
#[test]
fn test_imports_commonjs_packages_unbundled() {
    let dir = commonjs_packages("b_cjs_test");
    write_in(
        &dir,
        "cjs.test.js",
        "import { test, expect } from 'runtime:test';\n\
         import greeting, { greet } from 'greeter';\n\
         import state from 'shared-state';\n\
         import { bump } from 'counter';\n\
         import { read } from 'needs-fs';\n\
         test('names and the default', () => {\n\
           expect(greet('Ada')).toBe('hi Ada');\n\
           expect(greeting).toBe('the default');\n\
         });\n\
         test('one instance of a required package', () => {\n\
           bump();\n\
           expect(state.count).toBe(1);\n\
         });\n\
         test('a builtin fails where it is required', () => {\n\
           expect(read).toThrow('\"fs\"');\n\
         });\n",
    );
    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    assert!(stdout(&out).contains("3 passed"), "{}", stdout(&out));
    // Converted once, into the project's own node_modules — one directory
    // for the install — and reused.
    let install = std::fs::read_dir(dir.join("node_modules/.esdev/deps"))
        .expect("the conversions")
        .flatten()
        .next()
        .expect("an install's directory")
        .path();
    let converted = std::fs::read_dir(&install)
        .expect("the install's conversions")
        .count();
    assert!(converted >= 4, "{converted}");
    let again = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    assert!(
        again.status.success(),
        "{}{}",
        stdout(&again),
        stderr(&again)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// **A package the project links from outside it loads** (D234): `link:`,
/// `file:` or `workspace:` in its `package.json`, installed as a symbolic link
/// out of the project. Its own dependencies load from its own `node_modules`;
/// a walk from inside it stops at it rather than climbing the host; and a link
/// nobody declared is refused as before.
#[cfg(unix)]
#[test]
fn a_linked_package_outside_the_project_loads() {
    use std::os::unix::fs::symlink;
    let base = build_dir("b_linked");
    let app = base.join("app");
    let lib = base.join("lib");
    let package = |dir: &Path, name: &str, index: &str| {
        std::fs::create_dir_all(dir).expect("mkdir");
        write_in(
            dir,
            "package.json",
            &format!(
                r#"{{"name":"{name}","version":"1.0.0","type":"module","exports":"./index.js"}}"#
            ),
        );
        write_in(dir, "index.js", index);
    };
    package(
        &lib,
        "lib",
        "import { twice } from 'helper';\nexport const value = twice(21);\n",
    );
    package(
        &lib.join("node_modules/helper"),
        "helper",
        "export const twice = (n) => n * 2;\n",
    );
    // Above the linked package, where a walk that did not stop at it would
    // find it.
    package(
        &base.join("node_modules/above"),
        "above",
        "export default 'climbed';\n",
    );
    package(&base.join("stray"), "stray", "export default 'stray';\n");
    std::fs::create_dir_all(app.join("node_modules")).expect("mkdir");
    symlink(&lib, app.join("node_modules/lib")).expect("link lib");
    symlink(base.join("stray"), app.join("node_modules/stray")).expect("link stray");
    write_in(
        &app,
        "package.json",
        r#"{"name":"app","type":"module","dependencies":{"lib":"link:../lib","stray":"^1.0.0"}}"#,
    );
    write_in(
        &app,
        "linked.test.js",
        "import { test, expect } from 'runtime:test';\n\
         import { value } from 'lib';\n\
         test('the linked package and its own dependency', () => expect(value).toBe(42));\n",
    );
    let out = esdev_in(&app)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));

    write_in(
        &app,
        "stray.js",
        "import stray from 'stray';\nconsole.log(stray);\n",
    );
    let stray = esdev_in(&app)
        .arg("stray.js")
        .output()
        .expect("spawn esdev");
    assert!(
        !stray.status.success(),
        "an undeclared link loaded: {}",
        stdout(&stray)
    );
    assert!(
        stderr(&stray).contains("resolves outside the project root"),
        "{}",
        stderr(&stray)
    );

    write_in(&lib, "climb.js", "export { default } from 'above';\n");
    write_in(
        &app,
        "climb.js",
        "import x from 'lib/climb.js';\nconsole.log(x);\n",
    );
    write_in(
        &lib,
        "package.json",
        r#"{"name":"lib","version":"1.0.0","type":"module","exports":{".":"./index.js","./climb.js":"./climb.js"}}"#,
    );
    let climbed = esdev_in(&app)
        .arg("climb.js")
        .output()
        .expect("spawn esdev");
    assert!(
        !climbed.status.success(),
        "the walk climbed out: {}",
        stdout(&climbed)
    );
    assert!(
        stderr(&climbed).contains("cannot find package \"above\""),
        "{}",
        stderr(&climbed)
    );
    let _ = std::fs::remove_dir_all(&base);
}

/// **esdev's default is never narrower than a grant it accepts** (D233). Under
/// the development default and `-A` an unscoped grant is the whole filesystem;
/// rehearsing production with `--deny-all --allow-write` is the project, as
/// `esrun` is — and `esrun -A` is the project too, reaching further only by a
/// path typed on its command line. The refusal says how the root was chosen.
#[test]
fn the_development_default_reaches_outside_the_project() {
    let base = build_dir("b_everywhere");
    let project = base.join("project");
    let outside = base.join("outside");
    std::fs::create_dir_all(&project).expect("mkdir project");
    std::fs::create_dir_all(&outside).expect("mkdir outside");
    let target = outside.join("x.txt");
    write_in(
        &project,
        "w.js",
        &format!(
            "import {{ write }} from 'runtime:fs';\nawait write({:?}, 'hi');\nconsole.log('wrote');\n",
            target.to_str().expect("utf-8")
        ),
    );
    for args in [&[][..], &["-A"][..]] {
        let _ = std::fs::remove_file(&target);
        let out = esdev_in(&project)
            .args(args)
            .arg("w.js")
            .output()
            .expect("spawn esdev");
        assert!(out.status.success(), "esdev {args:?}: {}", stderr(&out));
        assert!(target.is_file(), "esdev {args:?} wrote nothing");
    }
    let rehearsed = esdev_in(&project)
        .args(["--deny-all", "--allow-write", "w.js"])
        .output()
        .expect("spawn esdev");
    assert!(!rehearsed.status.success());
    let said = stderr(&rehearsed);
    assert!(said.contains("escapes the filesystem root"), "{said}");
    assert!(
        said.contains("the working directory the program was started in"),
        "{said}"
    );
    assert!(said.contains("--allow-write=<path>"), "{said}");

    let Some(esrun) = sibling_binary("esrun") else {
        return;
    };
    let _ = std::fs::remove_file(&target);
    let refused = Command::new(&esrun)
        .current_dir(&project)
        .args(["-A", "w.js"])
        .output()
        .expect("spawn esrun");
    assert!(
        !refused.status.success(),
        "esrun -A wrote outside the project"
    );
    let named = Command::new(&esrun)
        .current_dir(&project)
        .arg(format!("--allow-write={}", outside.display()))
        .arg("w.js")
        .output()
        .expect("spawn esrun");
    assert!(named.status.success(), "{}", stderr(&named));
    let _ = std::fs::remove_dir_all(&base);
}

/// A package's module is written only once the packages it requires are
/// converted. Another test process takes the module's being there to mean the
/// package is ready and follows it to the modules of what it requires; written
/// first, it was found before they were — a cold run that failed with "No such
/// file" one time in ten — and a conversion that then failed left it there,
/// so every run after failed the same way.
#[test]
fn a_package_is_not_ready_before_what_it_requires() {
    let dir = commonjs_packages("b_cjs_order");
    // `counter` requires `shared-state`, which cannot be converted.
    write_in(
        &dir.join("node_modules/shared-state"),
        "index.js",
        "module.exports = { count: ;\n",
    );
    write_in(
        &dir,
        "count.test.js",
        "import { test, expect } from 'runtime:test';\n\
         import { bump } from 'counter';\n\
         test('bumps', () => expect(bump()).toBe(1));\n",
    );
    let failed = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    assert!(!failed.status.success());
    let modules = |dir: &Path| -> Vec<String> {
        let Ok(installs) = std::fs::read_dir(dir.join("node_modules/.esdev/deps")) else {
            return Vec::new();
        };
        installs
            .flatten()
            .flat_map(|install| std::fs::read_dir(install.path()).into_iter().flatten())
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| {
                name.ends_with(".mjs")
                    && !name.ends_with(".body.mjs")
                    && !name.ends_with(".deps.mjs")
            })
            .collect()
    };
    let left = modules(&dir);
    assert!(
        left.iter().all(|name| !name.starts_with("counter-")),
        "counter's module was written though shared-state was not: {left:?}"
    );

    // Repaired, the next run converts what it needs rather than finding a
    // module that points at nothing.
    write_in(
        &dir.join("node_modules/shared-state"),
        "index.js",
        "module.exports = { count: 0 };\n",
    );
    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    let _ = std::fs::remove_dir_all(&dir);
}

/// An install replaces the conversions: they live in a directory named for
/// the lockfile, and the first conversion of a new install removes the old
/// install's, so the cache holds one install's worth and no more.
#[test]
fn a_new_install_replaces_the_previous_conversions() {
    let dir = commonjs_packages("b_cjs_prune");
    write_in(
        &dir,
        "package-lock.json",
        r#"{ "lockfileVersion": 3, "v": 1 }"#,
    );
    write_in(
        &dir,
        "one.test.js",
        "import { test, expect } from 'runtime:test';\nimport { greet } from 'greeter';\n\
         test('greets', () => expect(greet('Ada')).toBe('hi Ada'));\n",
    );
    let installs = || -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir.join("node_modules/.esdev/deps"))
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    };
    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    let first = installs();
    assert_eq!(first.len(), 1, "{first:?}");

    write_in(
        &dir,
        "package-lock.json",
        r#"{ "lockfileVersion": 3, "v": 2 }"#,
    );
    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    let second = installs();
    assert_eq!(second.len(), 1, "{second:?}");
    assert_ne!(first, second);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A converted package is a module like any other to `mock.module`: the mock
/// replaces it, and `importActual` still reaches the real one.
#[test]
fn test_mocks_a_commonjs_package() {
    let dir = commonjs_packages("b_cjs_mock");
    write_in(
        &dir,
        "mock.test.js",
        "import { test, expect, mock } from 'runtime:test';\n\
         mock.module('greeter', () => ({ greet: () => 'mocked' }));\n\
         import { greet } from 'greeter';\n\
         test('mocked', async () => {\n\
           expect(greet('Ada')).toBe('mocked');\n\
           const real = await mock.importActual('greeter');\n\
           expect(real.greet('Ada')).toBe('hi Ada');\n\
         });\n",
    );
    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    let _ = std::fs::remove_dir_all(&dir);
}

/// `esdev <file>` runs it too, and `import.meta.resolve` names the module an
/// import gets.
#[test]
fn a_module_run_unbundled_imports_a_commonjs_package() {
    let dir = commonjs_packages("b_cjs_run");
    write_in(
        &dir,
        "app.mjs",
        "import { greet } from 'greeter';\n\
         console.log(greet('Ada'));\n\
         console.log(import.meta.resolve('greeter').includes('/.esdev/deps/'));\n",
    );
    let out = esdev_in(&dir).arg("app.mjs").output().expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "hi Ada\ntrue\n");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The bundler's own warnings reach the terminal. It leaves an import it
/// cannot resolve as an import and reports success, and the bundle then fails
/// where it is deployed — so the build says so, where it was written.
#[test]
fn build_prints_what_the_bundler_warns_about() {
    let dir = build_dir("b_warnings");
    write_in(&dir, "package.json", r#"{"name":"w"}"#);
    write_in(
        &dir,
        "app.js",
        "import x from 'not-installed';\nconsole.log(x);\n",
    );
    let out = esdev_in(&dir)
        .args(["build", "app.js", "--out=dist/app.js"])
        .output()
        .expect("spawn esdev build");
    let err = stderr(&out);
    assert!(err.contains("warning:"), "{err}");
    assert!(err.contains("not-installed"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A dev server re-reads a file that changed by importing it under a new
/// query, as it would in a browser or Node: the module is its URL, so the new
/// query evaluates what is on disk now.
#[test]
fn a_changed_file_is_read_afresh_under_a_new_query() {
    let dir = build_dir("b_query_fresh");
    write_in(
        &dir,
        "server.ts",
        "import { write } from 'runtime:fs';\n\
         await write('./data.ts', 'export default 1 as number;');\n\
         const one = (await import('./data?v=1')).default;\n\
         await write('./data.ts', 'export default 2 as number;');\n\
         const two = (await import('./data?v=2')).default;\n\
         const same = (await import('./data?v=1')).default;\n\
         console.log(one, two, same);\n",
    );
    let out = esdev_in(&dir)
        .arg("server.ts")
        .output()
        .expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "1 2 1");
    let _ = std::fs::remove_dir_all(&dir);
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

/// The claim the help makes, asserted: a bundle that stays one file needs no
/// `imports` grant, because it has no imports left to resolve.
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

/// The exception the help names: a bundle that code-splits loads its chunks at
/// run time, and that is an import the runtime has to grant.
#[test]
fn a_code_split_bundle_needs_the_imports_capability() {
    let dir = build_dir("b_caps_split");
    write_in(&dir, "lazy.mjs", "export const n = 41;\n");
    write_in(
        &dir,
        "app.mjs",
        "const m = await import('./lazy.mjs');\nconsole.log('n =', m.n + 1);\n",
    );

    let out = esdev_in(&dir)
        .args(["build", "app.mjs"])
        .output()
        .expect("spawn esdev build");
    assert!(out.status.success(), "{}", stderr(&out));

    let Some(esrun) = sibling_binary("esrun") else {
        return;
    };
    // The chunk is a file the runtime must load, so --deny-all refuses it.
    let denied = Command::new(&esrun)
        .current_dir(&dir)
        .args(["--deny-all", "dist/app.js"])
        .output()
        .expect("spawn esrun");
    assert!(!denied.status.success(), "{}", stdout(&denied));

    // Granted, it runs.
    let granted = Command::new(&esrun)
        .current_dir(&dir)
        .args(["--allow-imports", "dist/app.js"])
        .output()
        .expect("spawn esrun");
    assert!(granted.status.success(), "{}", stderr(&granted));
    assert_eq!(stdout(&granted).trim(), "n = 42");
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

/// A minified build carries no annotation comments, as before rolldown 1.3
/// kept them through minification; a legal comment stays, and the unminified
/// build keeps the annotation for whatever bundles it next.
#[test]
fn a_minified_build_drops_annotations_and_keeps_legal_comments() {
    let dir = build_dir("b_comments");
    write_in(
        &dir,
        "app.mjs",
        "/*! Example licence, kept. */\n\
         function make(n) {\n  return { n };\n}\n\
         export const made = /* @__PURE__ */ make(Date.now());\n\
         console.log(made.n > 0);\n",
    );
    for (out, minify) in [("out/plain.js", false), ("out/small.js", true)] {
        let mut command = esdev_in(&dir);
        command.args(["build", "app.mjs", &format!("--out={out}")]);
        if minify {
            command.arg("--minify");
        }
        let built = command.output().expect("spawn esdev build");
        assert!(built.status.success(), "{}", stderr(&built));
    }
    let plain = std::fs::read_to_string(dir.join("out/plain.js")).unwrap();
    let small = std::fs::read_to_string(dir.join("out/small.js")).unwrap();
    assert!(plain.contains("@__PURE__"), "{plain}");
    assert!(!small.contains("@__PURE__"), "{small}");
    assert!(small.contains("Example licence, kept."), "{small}");
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
