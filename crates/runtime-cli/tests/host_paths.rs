//! A refusal a program can catch names what the program wrote, and never where
//! the host resolved it (DECISIONS D141).
//!
//! One probe per kind of refusal, run by the real `esrun` in a project with a
//! directory beside it that the program must learn nothing about. The
//! assertion is the same for all of them: no message contains the project's
//! absolute path, the directory above it, or anything outside it — and each
//! still carries the `code` a program branches on.

use std::path::{Path, PathBuf};
use std::process::Command;

/// `base/project` (the root, where the run starts) and `base/outside`, which
/// holds a real file for links and paths to point at.
fn scene() -> (PathBuf, PathBuf) {
    let base = std::env::temp_dir().join(format!("esrun-host-paths-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let project = base.join("project");
    for dir in ["data", "out", "src", "node_modules/noentry"] {
        std::fs::create_dir_all(project.join(dir)).unwrap();
    }
    std::fs::create_dir_all(base.join("outside")).unwrap();
    std::fs::write(base.join("outside/secret.txt"), "s").unwrap();
    std::fs::write(project.join("package.json"), r#"{"type":"module"}"#).unwrap();
    std::fs::write(project.join("data/ok.txt"), "ok").unwrap();
    std::fs::write(project.join("src/denied.js"), "export default 1;").unwrap();
    std::fs::write(
        project.join("node_modules/noentry/package.json"),
        r#"{"name":"noentry"}"#,
    )
    .unwrap();
    std::fs::write(project.join("policy.json"), r#"{"allow":["./app.mjs"]}"#).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(base.join("outside/secret.txt"), project.join("data/link")).unwrap();
    let base = std::fs::canonicalize(&base).unwrap();
    (base.clone(), base.join("project"))
}

const PROBE: &str = r#"
import * as fs from "runtime:fs";
const probes = {
  "escape-relative": () => fs.file("../outside/secret.txt").text(),
  "escape-absolute": () => fs.file("/etc/hostname").text(),
  "escape-link": () => fs.file("data/link").text(),
  "not-allowed": () => fs.file("package.json").text(),
  "missing": () => fs.file("data/missing.txt").text(),
  "root-mutation": () => fs.remove("out/.."),
  "import-missing": () => import("./src/missing.js"),
  "import-outside": () => import("../outside/nothing-here.js"),
  "import-outside-existing": () => import("../outside/secret.txt"),
  "import-policy": () => import("./src/denied.js"),
  "import-no-package": () => import("nopkg"),
  "import-no-entry": () => import("noentry"),
  "resolve-no-package": async () => import.meta.resolve("nopkg"),
};
for (const [name, run] of Object.entries(probes)) {
  try {
    await run();
    console.log([name, "ok", ""].join("\t"));
  } catch (e) {
    // The message JSON-quoted, so a newline in it cannot split the line.
    console.log([name, e.code ?? "none", JSON.stringify(String(e.message))].join("\t"));
  }
}
"#;

#[test]
fn a_caught_refusal_names_no_host_path() {
    let (base, project) = scene();
    std::fs::write(project.join("app.mjs"), PROBE).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_esrun"))
        .current_dir(&project)
        .args([
            "--allow-read=data",
            "--allow-write=out",
            "--allow-imports",
            "--import-policy=policy.json",
            "app.mjs",
        ])
        .output()
        .expect("spawn esrun");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(out.status.success(), "stdout: {stdout}\nstderr: {stderr}");

    let expected: &[(&str, &str)] = &[
        ("escape-relative", "ERR_JAIL_ESCAPE"),
        ("escape-absolute", "ERR_JAIL_ESCAPE"),
        #[cfg(unix)]
        ("escape-link", "ERR_JAIL_ESCAPE"),
        ("not-allowed", "ERR_PERMISSION_DENIED"),
        ("missing", "ERR_NOT_FOUND"),
        ("root-mutation", "ERR_INVALID_PATH"),
        ("import-missing", "ERR_NOT_FOUND"),
        ("import-outside", "ERR_JAIL_ESCAPE"),
        ("import-outside-existing", "ERR_JAIL_ESCAPE"),
        ("import-policy", "ERR_PERMISSION_DENIED"),
        ("import-no-package", "ERR_NOT_FOUND"),
        ("import-no-entry", "ERR_NOT_FOUND"),
        ("resolve-no-package", "ERR_NOT_FOUND"),
    ];
    let lines: Vec<Vec<&str>> = stdout
        .lines()
        .map(|line| line.splitn(3, '\t').collect())
        .collect();
    let host = [
        base.to_string_lossy().into_owned(),
        project.to_string_lossy().into_owned(),
    ];
    for (name, code) in expected {
        let line = lines
            .iter()
            .find(|line| line.first() == Some(name))
            .unwrap_or_else(|| panic!("{name} did not report: {stdout}"));
        let message = line.get(2).copied().unwrap_or_default();
        assert_eq!(line.get(1), Some(code), "{name}: {message}");
        for path in &host {
            assert!(
                !message.contains(path.as_str()),
                "{name} leaked {path}: {message}"
            );
        }
    }
    let _ = std::fs::remove_dir_all(Path::new(&base));
}

/// A failed fetch names the origin and path it tried — not the credentials,
/// query or fragment, which are where a program's secrets travel, in either
/// the error or its cause (D141). Port 1 on loopback refuses without a network.
#[test]
fn a_failed_fetch_names_no_secret_from_its_url() {
    let code = r#"
      try {
        await fetch("http://user:hunter2@127.0.0.1:1/api?token=s3cret#frag");
      } catch (e) {
        console.log([e.code, e.message, e.cause && e.cause.message].join("\t"));
      }
    "#;
    let out = Command::new(env!("CARGO_BIN_EXE_esrun"))
        .args(["--allow-net", &format!("-e={code}")])
        .output()
        .expect("spawn esrun");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(stdout.starts_with("ERR_CONNECTION_REFUSED\t"), "{stdout}");
    assert!(stdout.contains("http://127.0.0.1:1/api"), "{stdout}");
    for secret in ["hunter2", "user", "s3cret", "token", "frag"] {
        assert!(!stdout.contains(secret), "leaked {secret}: {stdout}");
    }
}
