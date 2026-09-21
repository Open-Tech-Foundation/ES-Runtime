//! `esdev check` — typecheck the project with its own `tsc`.
//!
//! `esdev` strips types without checking them, so a type error is invisible
//! until something runs `tsc --noEmit` — usually an npm script somebody has
//! to know about. This is that invocation, as a command: run `tsc --noEmit`
//! through the project's package manager, and hand back its output and exit
//! code unchanged.
//!
//! Through the manager, not around it: it resolves the project's own
//! TypeScript (the version the tsconfig was written against, which is what
//! `npm run typecheck` would use) and speaks Windows' `.cmd` shims, so this
//! does not reimplement either. What runs is found the way `--install-types`
//! finds it — the `packageManager` field, then the lockfile, then what is
//! installed — and everything past `check` is `tsc`'s own. Nothing is added:
//! not a checker, not an opinion about flags, not a rewrite of the
//! diagnostics. A convenience needs to stay one.

use std::path::Path;

use crate::types::PackageManager;

/// Runs `tsc --noEmit` through the project's package manager, passing `args`
/// through untouched.
pub async fn check(dir: &Path, args: &[String]) -> Result<(), String> {
    let manager = PackageManager::detect();
    // Before anything runs: the managers fetch a missing binary from the
    // registry when asked (`pnpm exec` just demonstrated it on this
    // repository), so an uninstalled TypeScript would check with whatever
    // version the network handed over — slowly, the first time, and never the
    // project's. Refuse with the install instead.
    if let Some(err) = missing_typescript(dir, manager) {
        return Err(err);
    }
    let (program, prefix) = exec_command(manager, yarn_is_berry());
    let status = tokio::process::Command::new(&program)
        .args(&prefix)
        .arg("--noEmit")
        .args(args)
        .current_dir(dir)
        .status()
        .await
        .map_err(|e| {
            // A declared manager that is not installed is the project's answer
            // anyway: say that rather than leaking the spawn failure.
            if e.kind() == std::io::ErrorKind::NotFound {
                format!(
                    "this project uses {name}, which is not installed here.\n\n\
                     Install it and run `esdev check` again.",
                    name = manager.name(),
                )
            } else {
                format!("cannot run {program}: {e}")
            }
        })?;
    if status.success() {
        return Ok(());
    }
    // tsc already said what is wrong, at length: the exit code is the whole
    // of what this adds.
    Err(format!(
        "tsc failed{}",
        status
            .code()
            .map_or_else(String::new, |code| format!(" (exit code {code})"))
    ))
}

/// Why `tsc` cannot run here, if it cannot: nothing installed, TypeScript
/// not among it, or neither.
fn missing_typescript(dir: &Path, manager: PackageManager) -> Option<String> {
    // Yarn Plug'n'Play keeps no `node_modules`: the manager resolves, so
    // there is nothing to verify up front.
    for marker in [".pnp.cjs", ".pnp.loader.mjs", ".pnp.js"] {
        if dir.join(marker).is_file() {
            return None;
        }
    }
    let typescript = dir.join("node_modules").join("typescript");
    if typescript.is_dir() {
        return None;
    }
    if !dir.join("node_modules").is_dir() {
        return Some(format!(
            "the dependencies are not installed here.\n\n\
             Run `{name} install`, then `esdev check` again.",
            name = manager.name(),
        ));
    }
    if !manifest_depends_on(dir, "typescript") {
        return Some(
            "this project does not depend on TypeScript.\n\n\
             Add it to the dev dependencies and install, then `esdev check` again."
                .to_string(),
        );
    }
    Some(format!(
        "TypeScript is not installed here.\n\n\
         Run `{name} install`, then `esdev check` again.",
        name = manager.name(),
    ))
}

/// Whether `package.json` names `name` in its dependencies of either kind.
/// An unreadable manifest is not evidence: absence of proof either way.
fn manifest_depends_on(dir: &Path, name: &str) -> bool {
    let Ok(manifest) = std::fs::read_to_string(dir.join("package.json")) else {
        return true;
    };
    let Ok(manifest) = serde_json::from_str::<serde_json::Value>(&manifest) else {
        return true;
    };
    ["dependencies", "devDependencies"].iter().any(|key| {
        manifest
            .get(key)
            .and_then(|deps| deps.as_object())
            .is_some_and(|deps| deps.contains_key(name))
    })
}

/// How `tsc` is invoked under each manager: the program plus the arguments
/// ahead of it. `tsc --noEmit` and whatever was passed to `check` follow.
fn exec_command(manager: PackageManager, yarn_berry: bool) -> (String, Vec<String>) {
    let words = |words: &[&str]| words.iter().map(ToString::to_string).collect();
    match manager {
        PackageManager::Npm => ("npm".to_string(), words(&["exec", "--", "tsc"])),
        // `bunx`, not `bun`: the runner is its own binary.
        PackageManager::Bun => ("bunx".to_string(), words(&["tsc"])),
        PackageManager::Pnpm => ("pnpm".to_string(), words(&["exec", "tsc"])),
        // Berry runs shell commands (`yarn exec`); Classic runs local
        // binaries by name (`yarn tsc`).
        PackageManager::Yarn if yarn_berry => ("yarn".to_string(), words(&["exec", "tsc"])),
        PackageManager::Yarn => ("yarn".to_string(), words(&["tsc"])),
    }
}

/// Whether the `yarn` on this machine is Berry (2+) rather than Classic.
/// Only asked when the project uses yarn: Classic is the common case and the
/// fallback when the version cannot be read.
fn yarn_is_berry() -> bool {
    std::process::Command::new("yarn")
        .arg("--version")
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .and_then(|version| version.trim().split('.').next()?.parse::<u64>().ok())
        .is_some_and(|major| major >= 2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Every manager resolves to an invocation ending in `tsc`, with Berry
    /// and Classic yarn taking their documented shapes.
    #[test]
    fn every_manager_invokes_the_projects_tsc() {
        for (manager, program) in [
            (PackageManager::Npm, "npm"),
            // The runner is its own binary, not the manager.
            (PackageManager::Bun, "bunx"),
            (PackageManager::Pnpm, "pnpm"),
        ] {
            let (invoked, prefix) = exec_command(manager, false);
            assert_eq!(invoked, program);
            assert_eq!(prefix.last().map(String::as_str), Some("tsc"));
        }
        let (_, classic) = exec_command(PackageManager::Yarn, false);
        assert_eq!(classic, vec!["tsc".to_string()]);
        let (_, berry) = exec_command(PackageManager::Yarn, true);
        assert_eq!(berry, vec!["exec".to_string(), "tsc".to_string()]);
    }

    /// A directory holding a fixture project root.
    fn root(name: &str, files: &[(&str, &str)]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("esdev-check-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the fixture");
        for (name, contents) in files {
            let path = dir.join(name);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("create parents");
            }
            std::fs::write(&path, contents).expect("write the fixture");
        }
        dir
    }

    /// Installed TypeScript is never questioned, whatever the manifest says.
    #[test]
    fn installed_typescript_is_never_questioned() {
        let dir = root(
            "installed",
            &[("node_modules/typescript/package.json", "{}")],
        );
        assert!(missing_typescript(&dir, PackageManager::Npm).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Each absence names its own fix: no dependencies, TypeScript not among
    /// them, or not depended on at all.
    #[test]
    fn each_absence_names_its_fix() {
        let bare = root("bare", &[("package.json", "{}")]);
        let err = missing_typescript(&bare, PackageManager::Npm).expect("refused");
        assert!(err.contains("npm install"), "{err}");

        let partial = root(
            "partial",
            &[
                (
                    "package.json",
                    r#"{ "devDependencies": { "typescript": "^5" } }"#,
                ),
                ("node_modules/.keep", ""),
            ],
        );
        let err = missing_typescript(&partial, PackageManager::Bun).expect("refused");
        assert!(err.contains("bun install"), "{err}");

        let other = root(
            "other",
            &[
                (
                    "package.json",
                    r#"{ "devDependencies": { "leftpad": "^1" } }"#,
                ),
                ("node_modules/.keep", ""),
            ],
        );
        let err = missing_typescript(&other, PackageManager::Pnpm).expect("refused");
        assert!(err.contains("does not depend"), "{err}");

        for dir in [&bare, &partial, &other] {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    /// Yarn Plug'n'Play keeps no `node_modules`: the manager resolves, so the
    /// pre-check stands aside rather than misfiring.
    #[test]
    fn a_pnp_project_is_left_to_its_manager() {
        let dir = root("pnp", &[(".pnp.cjs", "/* pnp */")]);
        assert!(missing_typescript(&dir, PackageManager::Yarn).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
