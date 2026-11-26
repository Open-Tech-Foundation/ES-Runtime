//! `esdev init` and `esdev typecheck`.

use super::*;

/// `esdev typecheck` runs the project's TypeScript through its package manager:
/// a fake `npm` records the invocation, so no network and no real install is
/// involved. Unix-only: the fake is a shell script.
#[cfg(unix)]
#[test]
fn typecheck_runs_tsc_through_the_projects_package_manager() {
    use std::os::unix::fs::PermissionsExt;

    let dir = build_dir("t_typecheck_cmd");
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
        "#!/bin/sh\necho \"$@\" >> \"$ESDEV_TYPECHECK_CAPTURE\"\nexit \"${ESDEV_TYPECHECK_EXIT:-0}\"\n",
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
        .arg("typecheck")
        .env("PATH", &path)
        .env("ESDEV_TYPECHECK_CAPTURE", &capture)
        .env("ESDEV_TYPECHECK_EXIT", "0")
        .output()
        .expect("spawn esdev typecheck");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    let invoked = std::fs::read_to_string(&capture).expect("read capture");
    assert!(
        invoked.contains("exec -- tsc --noEmit"),
        "tsc was not invoked through npm exec: {invoked}"
    );

    // The exit code is tsc's own: a failing check fails the command.
    let out = esdev_in(&dir)
        .arg("typecheck")
        .env("PATH", &path)
        .env("ESDEV_TYPECHECK_CAPTURE", &capture)
        .env("ESDEV_TYPECHECK_EXIT", "3")
        .output()
        .expect("spawn esdev typecheck");
    assert!(!out.status.success());
    assert!(stderr(&out).contains("tsc failed"), "{}", stderr(&out));

    let _ = std::fs::remove_dir_all(&dir);
}

/// Without TypeScript installed there is nothing to run: the error names the
/// install rather than failing inside the package manager.
#[test]
fn typecheck_without_typescript_names_the_install() {
    let dir = build_dir("t_typecheck_cmd_missing");
    write_in(
        &dir,
        "package.json",
        r#"{ "packageManager": "npm@1.0.0", "devDependencies": { "typescript": "^5" } }"#,
    );

    let out = esdev_in(&dir)
        .arg("typecheck")
        .output()
        .expect("spawn esdev typecheck");
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

/// The bare docs name the chosen manager: `--package-manager` renders, the
/// default is npm, and an unknown name fails before writing.
#[test]
fn init_names_the_chosen_package_manager() {
    let parent = temp("t_init_pm");
    let _ = std::fs::remove_dir_all(&parent);
    std::fs::create_dir_all(&parent).expect("create parent");

    let out = esdev()
        .args([
            "init",
            "yarned",
            "--package-manager=yarn",
            "-y",
            "--no-install",
        ])
        .current_dir(&parent)
        .output()
        .expect("spawn esdev init");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    let dir = parent.join("yarned");
    let readme = std::fs::read_to_string(dir.join("README.md")).expect("read readme");
    assert!(readme.contains("yarn run dev"), "{readme}");
    assert!(!readme.contains("{{pm}}"), "a placeholder survived");
    assert!(
        stdout(&out).contains("yarn install"),
        "the next steps name it too: {}",
        stdout(&out)
    );

    // Unsaid: npm, matching the install default.
    let out = esdev()
        .args(["init", "plain", "-y", "--no-install"])
        .current_dir(&parent)
        .output()
        .expect("spawn esdev init");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    let readme = std::fs::read_to_string(parent.join("plain").join("README.md")).expect("read");
    assert!(readme.contains("npm run dev"), "{readme}");
    assert!(!readme.contains("{{pm}}"), "a placeholder survived");

    // An unknown name fails before writing.
    let unknown = esdev()
        .args(["init", "nope", "--package-manager=cargo", "-y"])
        .current_dir(&parent)
        .output()
        .expect("spawn esdev init");
    assert!(!unknown.status.success());
    assert!(
        stderr(&unknown).contains("there is no cargo package manager"),
        "{}",
        stderr(&unknown)
    );
    // The directory itself is created on the way in; nothing is written into it.
    let dir = parent.join("nope");
    assert!(
        dir.read_dir()
            .map(|mut r| r.next().is_none())
            .unwrap_or(true),
        "it wrote a project anyway"
    );

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
    assert!(manifest.contains("\"servers\""), "{manifest}");
    assert!(manifest.contains("\"server\": {"), "{manifest}");
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

/// The Node convention — `main.js` at the root, named by `package.json` — is an
/// entry like any other: adopting detects it instead of reporting that there is
/// nothing to adopt.
#[test]
fn init_adopts_a_root_main() {
    let dir = build_dir("t_init_main");
    write_in(&dir, "package.json", "{\"type\":\"module\"}\n");
    write_in(&dir, "main.js", "console.log(\"legacy\");\n");

    let out = esdev_in(&dir)
        .args(["init", "-y"])
        .output()
        .expect("spawn esdev init");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    let manifest = std::fs::read_to_string(dir.join("esdev.json")).expect("read esdev.json");
    assert!(manifest.contains("\"entry\": \"main.js\""), "{manifest}");
    assert!(manifest.contains("\"servers\""), "{manifest}");
    assert!(manifest.contains("\"server\": {"), "{manifest}");

    let _ = std::fs::remove_dir_all(&dir);
}
