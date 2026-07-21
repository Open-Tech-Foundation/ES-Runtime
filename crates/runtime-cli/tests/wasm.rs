//! End-to-end WebAssembly ES-module integration tests: run the real `esrun`
//! binary against a `.wasm` file on disk, so the actual loader, the binary
//! read path, and the generated wrapper are all exercised together.
//!
//! Fixtures are built here rather than committed as binaries: the modules are
//! tiny, and spelling them out in code keeps what is being imported readable.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

// --- a minimal WASM encoder, enough for the fixtures below ---

/// `\0asm` + version 1.
const MAGIC: [u8; 8] = [0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];

/// A length-prefixed vector of pre-encoded items.
fn vec_of(items: &[Vec<u8>]) -> Vec<u8> {
    let mut out = vec![items.len() as u8];
    for item in items {
        out.extend_from_slice(item);
    }
    out
}

/// A section: id, byte length, payload.
fn section(id: u8, payload: Vec<u8>) -> Vec<u8> {
    let mut out = vec![id, payload.len() as u8];
    out.extend(payload);
    out
}

/// A length-prefixed UTF-8 name.
fn name(s: &str) -> Vec<u8> {
    let mut out = vec![s.len() as u8];
    out.extend_from_slice(s.as_bytes());
    out
}

/// A function body: size-prefixed, no locals, terminated by `end`.
fn body(code: &[u8]) -> Vec<u8> {
    let mut out = vec![(code.len() + 2) as u8, 0x00];
    out.extend_from_slice(code);
    out.push(0x0b);
    out
}

/// `(func (export "add") (export "weird-name") (param i32 i32) (result i32)
///    local.get 0 local.get 1 i32.add)`
///
/// The second export deliberately is not a JS identifier, so the generated
/// wrapper has to re-export it under a string alias.
fn add_module() -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    out.extend(section(
        1,
        vec_of(&[vec![0x60, 0x02, 0x7f, 0x7f, 0x01, 0x7f]]),
    ));
    out.extend(section(3, vec_of(&[vec![0x00]])));
    let mut add_export = name("add");
    add_export.extend([0x00, 0x00]);
    let mut weird_export = name("weird-name");
    weird_export.extend([0x00, 0x00]);
    out.extend(section(7, vec_of(&[add_export, weird_export])));
    out.extend(section(
        10,
        vec_of(&[body(&[0x20, 0x00, 0x20, 0x01, 0x6a])]),
    ));
    out
}

/// `(import "./env.js" "log" (func (param i32)))`
/// `(func (export "callLog") (param i32) local.get 0 call 0)`
///
/// The import's *module* half is a real module specifier, so the wasm import
/// must resolve through the ordinary ES module graph.
fn importing_module() -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    out.extend(section(1, vec_of(&[vec![0x60, 0x01, 0x7f, 0x00]])));
    let mut import = name("./env.js");
    import.extend(name("log"));
    import.extend([0x00, 0x00]);
    out.extend(section(2, vec_of(&[import])));
    out.extend(section(3, vec_of(&[vec![0x00]])));
    let mut export = name("callLog");
    export.extend([0x00, 0x01]);
    out.extend(section(7, vec_of(&[export])));
    out.extend(section(10, vec_of(&[body(&[0x20, 0x00, 0x10, 0x00])])));
    out
}

// --- harness ---

/// A fresh directory under the system temp dir, removed and recreated so a rerun
/// never sees a previous run's files.
fn workdir(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("esrun-wasm-esm-{test}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create work dir");
    dir
}

fn write(dir: &Path, rel: &str, bytes: &[u8]) {
    std::fs::write(dir.join(rel), bytes).expect("write fixture");
}

fn run(dir: &Path, entry: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_esrun"))
        .arg(dir.join(entry))
        .output()
        .expect("failed to spawn esrun")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}
fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

// --- tests ---

#[test]
fn imports_a_wasm_module_as_an_es_module() {
    let dir = workdir("basic");
    write(&dir, "add.wasm", &add_module());
    write(
        &dir,
        "main.js",
        br#"import { add } from "./add.wasm";
console.log("SUM:", add(2, 3));
"#,
    );

    let out = run(&dir, "main.js");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("SUM: 5"), "{}", stdout(&out));
}

#[test]
fn exposes_every_export_including_non_identifier_names() {
    let dir = workdir("exports");
    write(&dir, "add.wasm", &add_module());
    write(
        &dir,
        "main.js",
        br#"import * as all from "./add.wasm";
console.log("KEYS:" + Object.keys(all).sort().join(","));
console.log("WEIRD:", all["weird-name"](1, 1));
"#,
    );

    let out = run(&dir, "main.js");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let stdout = stdout(&out);
    // A wasm export name that is not an identifier still round-trips.
    assert!(stdout.contains("KEYS:add,weird-name"), "{stdout}");
    assert!(stdout.contains("WEIRD: 2"), "{stdout}");
}

#[test]
fn a_wasm_import_resolves_through_the_module_graph() {
    let dir = workdir("imports");
    write(&dir, "imported.wasm", &importing_module());
    write(
        &dir,
        "env.js",
        br#"export const log = (v) => console.log("LOGGED:", v);
"#,
    );
    write(
        &dir,
        "main.js",
        br#"import { callLog } from "./imported.wasm";
callLog(7);
"#,
    );

    let out = run(&dir, "main.js");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("LOGGED: 7"), "{}", stdout(&out));
}

#[test]
fn a_wasm_module_is_instantiated_once_per_graph() {
    let dir = workdir("dedup");
    write(&dir, "add.wasm", &add_module());
    write(
        &dir,
        "main.js",
        br#"import { add } from "./add.wasm";
const again = await import("./add.wasm");
// Static and dynamic import of the same file must share one instance.
console.log("SAME:", again.add === add);
"#,
    );

    let out = run(&dir, "main.js");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("SAME: true"), "{}", stdout(&out));
}

#[test]
fn a_malformed_wasm_file_fails_to_load_with_a_clear_error() {
    let dir = workdir("malformed");
    write(&dir, "bad.wasm", &[0x00, 0x01, 0x02, 0x03]);
    write(
        &dir,
        "main.js",
        br#"import "./bad.wasm";
console.log("should not run");
"#,
    );

    let out = run(&dir, "main.js");
    assert!(!out.status.success(), "should exit non-zero");
    let stderr = stderr(&out);
    // V8's own diagnostic is surfaced rather than a generic load failure.
    assert!(
        stderr.contains("magic word") || stderr.to_lowercase().contains("wasm"),
        "{stderr}"
    );
    assert!(!stdout(&out).contains("should not run"), "{}", stdout(&out));
}

#[test]
fn a_missing_wasm_import_dependency_is_a_load_error() {
    let dir = workdir("missing-dep");
    // The wasm imports "./env.js", which is deliberately not written.
    write(&dir, "imported.wasm", &importing_module());
    write(&dir, "main.js", br#"import "./imported.wasm";"#);

    let out = run(&dir, "main.js");
    assert!(!out.status.success(), "should exit non-zero");
    assert!(stderr(&out).contains("env.js"), "{}", stderr(&out));
}
