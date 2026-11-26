//! Source maps, and the frames they put back.

use super::*;

/// A frame in TypeScript run directly names the line it was written on — also
/// in a directory with a space, which its `file:` URL spells `%20`. The map
/// was registered under the encoded spelling and looked up under the decoded
/// one, so it was never found and the frame named a line of the stripped
/// output instead (D165).
#[test]
fn a_stack_trace_names_the_written_line_in_a_directory_with_a_space() {
    let dir = build_dir("b_sourcemap space");
    write_in(
        &dir,
        "app.ts",
        "type T = {\n  a: number;\n};\nconst x: T = { a: 1 };\n\
         function boom(): never {\n  throw new Error('boom ' + x.a);\n}\nboom();\n",
    );
    let out = esdev_in(&dir).arg("app.ts").output().expect("spawn esdev");
    assert!(!out.status.success(), "{}", stdout(&out));
    let err = stderr(&out);
    assert!(err.contains("app.ts:6:9)"), "{err}");
    assert!(err.contains("app.ts:8:1"), "{err}");
}

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
    // Relative to where the map ended up — not to the staging directory the
    // build wrote it in and moved out of — and naming nothing about the
    // machine that built it (D141).
    let sources: serde_json::Value = serde_json::from_str(&map).expect("valid JSON");
    let first = sources["sources"][0].as_str().expect("a source");
    assert_eq!(first, "../src/util.ts", "{map}");
    assert!(
        dir.join("dist").join(first).is_file(),
        "the map names a file that is not there: {first}"
    );
    let built_in =
        es_runtime_default_providers::path::canonicalize(&dir).expect("canonical build dir");
    assert!(!map.contains(built_in.to_str().expect("utf-8")), "{map}");
    assert!(!map.contains(".esdev-build-"), "{map}");

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

    // Deployed somewhere else, the map still finds its sources: they are
    // relative to it, so they move with it.
    let moved = dir.with_file_name(format!(
        "{}-moved",
        dir.file_name().and_then(|n| n.to_str()).expect("a name")
    ));
    let _ = std::fs::remove_dir_all(&moved);
    std::fs::rename(&dir, &moved).expect("move the deployment");
    let ran = Command::new(sibling_binary("esrun").expect("found above"))
        .current_dir(&moved)
        .arg("dist/app.js")
        .output()
        .expect("spawn esrun");
    let trace = stderr(&ran);
    let _ = std::fs::remove_dir_all(&moved);
    assert!(
        trace.contains("-moved/src/util.ts:2:"),
        "unmapped after moving:\n{trace}"
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
        "{ \"build\": { \"targets\": { \"web\": { \"entry\": \"index.html\", \"outDir\": \"dist\" } } } }\n",
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
        "{ \"build\": { \"targets\": { \"web\": { \"entry\": \"index.html\", \"outDir\": \"dist\", \
         \"sourcemap\": \"inline\" } } } }\n",
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

/// esdev's own passes that change a module hand the bundler a map, so a build
/// that asks for one gets no "sourcemap is likely to be incorrect" warning and
/// a stack frame lands on the line that was written. A CSS Module is generated
/// code (an empty map), a `new URL(…, import.meta.url)` is edited in place, and
/// a file whose JSX pragma differs from the project's is recompiled.
#[test]
fn built_in_passes_keep_the_source_map_whole() {
    let dir = build_dir("b_sourcemap_passes");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(&dir, "src/card.module.css", ".card { color: red }\n");
    write_in(&dir, "src/worker.js", "export const worker = 1;\n");
    write_in(
        &dir,
        "src/view.jsx",
        "/** @jsx h */\n\
         const h = (tag) => tag;\n\
         export function view(n) {\n  \
           if (n > 1) throw new Error('too big');\n  \
           return <p />;\n\
         }\n",
    );
    write_in(
        &dir,
        "src/app.ts",
        "import styles from './card.module.css';\n\
         import { view } from './view.jsx';\n\
         const worker: URL = new URL('./worker.js', import.meta.url);\n\
         console.log(styles.card, String(worker).length > 0);\n\
         console.log(view(5));\n",
    );
    write_in(
        &dir,
        "esdev.json",
        r#"{ "jsx": { "factory": "React.createElement" } }"#,
    );
    let out = esdev_in(&dir)
        .args(["build", "src/app.ts", "--out=dist/app.js", "--sourcemap"])
        .output()
        .expect("spawn esdev build");
    assert!(out.status.success(), "{}", stderr(&out));
    let printed = format!("{}{}", stdout(&out), stderr(&out));
    assert!(!printed.contains("SOURCEMAP_BROKEN"), "{printed}");

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
    assert!(trace.contains("src/view.jsx:4:"), "unmapped:\n{trace}");
    assert!(trace.contains("src/app.ts:5:"), "unmapped:\n{trace}");
    let _ = std::fs::remove_dir_all(&dir);
}
