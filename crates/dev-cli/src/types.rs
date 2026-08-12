//! `esdev --install-types` — the `runtime:*` type definitions, in the editor.
//!
//! The definitions themselves are published to npm as `@opentf/esrun-types`
//! (`packages/types/`), so this does not carry a copy of them and does not write
//! one: it adds the package as a dev dependency with whatever package manager
//! the project already uses, and points `tsconfig.json` at it. That is the whole
//! job — TypeScript will not load a package's ambient declarations just because
//! it is installed, so the `compilerOptions.types` entry is what makes an editor
//! resolve `import { file } from "runtime:fs"`.
//!
//! **Why it lives in `esdev`.** It used to be `esrun types --install`, in the
//! binary that serves production — a command whose entire effect is to write
//! into `node_modules` and rewrite `tsconfig.json` (D59). It also meant `esrun`
//! carried every `.d.ts` baked into it, for a command a deployment never runs.

use std::path::Path;

/// The npm package the definitions are published as.
const PACKAGE: &str = "@opentf/esrun-types";

/// What `--install-types` did.
pub struct Outcome {
    /// The lines to print: one per half of the job.
    pub report: String,
    /// Whether the package is now installed. `false` when the package manager
    /// refused or is not there — the `tsconfig.json` half still happened, and
    /// the report says what to run, but the exit status has to be honest about
    /// it or a setup script would carry on as though it had worked.
    pub installed: bool,
}

/// Adds the type package to the project and wires `tsconfig.json` to it.
///
/// Errors only for the things a user must fix themselves — everything else is
/// reported and worked around, because a setup command that half-succeeds
/// should say which half.
pub fn install() -> Result<Outcome, String> {
    if !Path::new("package.json").exists() {
        return Err(format!(
            "no package.json here — run this from the project root, where {PACKAGE} \
             would be installed.\n\n\
             If the project has no package.json yet, create one first (npm init -y)."
        ));
    }

    let (message, installed) = add_dependency();
    let mut report = message;
    report.push('\n');
    report.push_str(&wire_tsconfig()?);
    report.push('\n');
    Ok(Outcome { report, installed })
}

/// Installs the package, unless it is already there, and says whether it is.
fn add_dependency() -> (String, bool) {
    if Path::new("node_modules").join(PACKAGE).exists() {
        return (format!("{PACKAGE} is already installed."), true);
    }
    let manager = PackageManager::detect();
    let (program, args) = manager.add_command();
    match std::process::Command::new(program).args(&args).status() {
        Ok(status) if status.success() => (
            format!("Installed {PACKAGE} ({program} {}).", args.join(" ")),
            true,
        ),
        // The package manager ran and refused, or is not installed at all.
        // Neither is something to guess at on the user's behalf: say what to run
        // and carry on with the half that is ours.
        Ok(_) => (
            format!(
                "{program} could not add {PACKAGE}. Install it yourself:\n  {program} {}",
                args.join(" ")
            ),
            false,
        ),
        Err(err) => (
            format!(
                "cannot run {program} ({err}). Install {PACKAGE} yourself:\n  {program} {}",
                args.join(" ")
            ),
            false,
        ),
    }
}

/// Which package manager this project uses.
///
/// Decided by the lockfile, which is the only durable evidence: a `packageManager`
/// field is often absent, and asking would make a one-command setup a two-step
/// one. npm is the fallback because it is the one that is always there.
#[derive(Debug, PartialEq, Eq)]
enum PackageManager {
    Bun,
    Pnpm,
    Yarn,
    Npm,
}

impl PackageManager {
    fn detect() -> PackageManager {
        Self::from_lockfiles(|name| Path::new(name).exists())
    }

    /// The detection itself, over a predicate, so a test can ask it about a
    /// project that does not exist on disk.
    fn from_lockfiles(exists: impl Fn(&str) -> bool) -> PackageManager {
        // Ordered by how specific the evidence is. A repository that has more
        // than one lockfile has a problem this command cannot solve; picking the
        // first match is at least deterministic.
        for (lockfile, manager) in [
            ("bun.lock", PackageManager::Bun),
            ("bun.lockb", PackageManager::Bun),
            ("pnpm-lock.yaml", PackageManager::Pnpm),
            ("yarn.lock", PackageManager::Yarn),
            ("package-lock.json", PackageManager::Npm),
        ] {
            if exists(lockfile) {
                return manager;
            }
        }
        PackageManager::Npm
    }

    /// The command that adds a dev dependency, as `(program, args)`.
    fn add_command(&self) -> (&'static str, Vec<&'static str>) {
        match self {
            // `npm add` exists as an alias, but `install` is the one every
            // version and every piece of documentation agrees on.
            PackageManager::Npm => ("npm", vec!["install", "--save-dev", PACKAGE]),
            PackageManager::Bun => ("bun", vec!["add", "--dev", PACKAGE]),
            PackageManager::Pnpm => ("pnpm", vec!["add", "--save-dev", PACKAGE]),
            PackageManager::Yarn => ("yarn", vec!["add", "--dev", PACKAGE]),
        }
    }
}

/// The `compilerOptions.types` entry that loads the package's declarations.
///
/// A `types` entry, not a `typeRoots` one: the package is a normal scoped
/// package rather than something under `node_modules/@types`, and TypeScript
/// resolves a type-reference name through node resolution when no type root
/// holds it. It is also the form the package's own `index.d.ts` documents, so
/// there is one answer rather than two.
fn wire_tsconfig() -> Result<String, String> {
    use serde_json::{Value, json};

    let path = Path::new("tsconfig.json");
    let manual = format!("  add to compilerOptions:\n    \"types\": [\"{PACKAGE}\"]");

    if !path.exists() {
        let config = json!({
            "compilerOptions": {
                "target": "ESNext",
                "module": "ESNext",
                "moduleResolution": "bundler",
                "strict": true,
                "types": [PACKAGE]
            },
            "include": ["**/*.ts"]
        });
        let text = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
        std::fs::write(path, format!("{text}\n")).map_err(|e| e.to_string())?;
        return Ok("Created tsconfig.json (compilerOptions.types).".into());
    }

    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    // A tsconfig is very often JSONC — comments and trailing commas — which
    // cannot be re-emitted from a JSON value. Rewriting it would silently delete
    // the comments, so an unparseable file is left exactly as it is and the two
    // lines to add are printed instead.
    let Ok(mut config) = serde_json::from_str::<Value>(&text) else {
        return Ok(format!(
            "tsconfig.json looks like JSONC (comments/trailing commas) — left it untouched.\n{manual}"
        ));
    };
    let Some(root) = config.as_object_mut() else {
        return Ok(format!(
            "tsconfig.json is not a JSON object — left it untouched.\n{manual}"
        ));
    };
    let options = root.entry("compilerOptions").or_insert_with(|| json!({}));
    let Some(options) = options.as_object_mut() else {
        return Ok(format!(
            "tsconfig.json compilerOptions is not an object — left it untouched.\n{manual}"
        ));
    };
    let added = add_type_entry(options);
    let text = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
    std::fs::write(path, format!("{text}\n")).map_err(|e| e.to_string())?;
    Ok(if added {
        "Updated tsconfig.json (compilerOptions.types).".into()
    } else {
        "tsconfig.json already loads the types.".into()
    })
}

/// Appends the package to `compilerOptions.types`, keeping whatever is there.
///
/// Returns whether anything changed, so re-running says "already" rather than
/// claiming an edit it did not make. Existing entries are preserved: `types` is
/// an allowlist, so dropping one would remove another package's globals.
fn add_type_entry(options: &mut serde_json::Map<String, serde_json::Value>) -> bool {
    use serde_json::Value;

    let entries = options
        .entry("types")
        .or_insert_with(|| Value::Array(Vec::new()));
    let Value::Array(entries) = entries else {
        return false;
    };
    if entries.iter().any(|entry| entry.as_str() == Some(PACKAGE)) {
        return false;
    }
    entries.push(Value::String(PACKAGE.to_string()));
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    use es_runtime_cli_common::Capability;

    #[test]
    fn a_lockfile_decides_the_package_manager() {
        let detect =
            |present: &'static str| PackageManager::from_lockfiles(move |name| name == present);
        assert_eq!(detect("bun.lock"), PackageManager::Bun);
        assert_eq!(detect("bun.lockb"), PackageManager::Bun);
        assert_eq!(detect("pnpm-lock.yaml"), PackageManager::Pnpm);
        assert_eq!(detect("yarn.lock"), PackageManager::Yarn);
        assert_eq!(detect("package-lock.json"), PackageManager::Npm);
    }

    #[test]
    fn a_project_with_no_lockfile_gets_npm() {
        assert_eq!(
            PackageManager::from_lockfiles(|_| false),
            PackageManager::Npm
        );
    }

    #[test]
    fn every_manager_installs_it_as_a_dev_dependency() {
        for manager in [
            PackageManager::Bun,
            PackageManager::Pnpm,
            PackageManager::Yarn,
            PackageManager::Npm,
        ] {
            let (program, args) = manager.add_command();
            assert!(
                args.contains(&PACKAGE),
                "{program} does not name the package"
            );
            assert!(
                args.iter().any(|arg| arg.contains("dev")),
                "{program} would install it as a runtime dependency"
            );
        }
    }

    #[test]
    fn the_types_entry_is_added_once_and_keeps_the_others() {
        let mut options = serde_json::Map::new();
        options.insert(
            "types".into(),
            serde_json::json!(["node", "@opentf/esrun-types"]),
        );
        assert!(!add_type_entry(&mut options), "it was already there");

        let mut options = serde_json::Map::new();
        options.insert("types".into(), serde_json::json!(["node"]));
        assert!(add_type_entry(&mut options));
        assert_eq!(
            options["types"],
            serde_json::json!(["node", "@opentf/esrun-types"]),
            "another package's globals must survive"
        );

        let mut options = serde_json::Map::new();
        assert!(add_type_entry(&mut options));
        assert_eq!(options["types"], serde_json::json!(["@opentf/esrun-types"]));
    }

    /// Every declaration file must actually be published.
    ///
    /// `index.d.ts` references its siblings, so one missing from the npm package
    /// is not a gap — it is an installed package that cannot resolve itself. The
    /// list in `package.json` had drifted twice; it is a glob now, and this
    /// asserts the glob still covers everything.
    #[test]
    fn every_types_file_is_publishable() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../packages/types");
        let manifest =
            std::fs::read_to_string(format!("{dir}/package.json")).expect("read manifest");
        let files: Vec<&str> = manifest
            .split("\"files\"")
            .nth(1)
            .expect("a files list")
            .split(']')
            .next()
            .expect("a closing bracket")
            .split('"')
            .filter(|s| s.ends_with(".d.ts") || s.ends_with(".md"))
            .collect();
        let covers_declarations = files.contains(&"*.d.ts");
        for entry in std::fs::read_dir(dir).expect("read types dir") {
            let name = entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned();
            if !name.ends_with(".d.ts") {
                continue;
            }
            assert!(
                covers_declarations || files.contains(&name.as_str()),
                "{name} is not in the published file list, so an installed \
                 {PACKAGE} could not resolve it"
            );
        }
    }

    /// `index.d.ts` is what the published package loads, so a file missing from
    /// it is invisible to anyone consuming the package.
    #[test]
    fn every_types_file_is_referenced_by_the_index() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../packages/types");
        let index = std::fs::read_to_string(format!("{dir}/index.d.ts")).expect("read index.d.ts");
        let mut checked = 0;
        for entry in std::fs::read_dir(dir).expect("read types dir") {
            let name = entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned();
            if !name.ends_with(".d.ts") || name == "index.d.ts" {
                continue;
            }
            assert!(
                index.contains(&format!("./{name}")),
                "{name} is not referenced by index.d.ts, so installing the \
                 package would not load it"
            );
            checked += 1;
        }
        assert!(checked >= 8, "only found {checked} declaration files");
    }

    /// The TypeScript `PermissionName` union is the last hand-written copy of
    /// the denial vocabulary — Rust owns it, and the two JS readers now ask the
    /// host for it — so this is where it can silently fall behind. An editor
    /// would then reject a name the runtime accepts, or accept one it does not.
    #[test]
    fn the_permission_union_matches_the_capabilities() {
        let source = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../packages/types/runtime-process.d.ts"
        ))
        .expect("read runtime-process.d.ts");
        let union = source
            .split_once("export type PermissionName =")
            .expect("PermissionName union")
            .1
            .split_once(';')
            .expect("union terminator")
            .0;
        let listed: Vec<&str> = union
            .split('|')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(|part| part.trim_matches('"'))
            .collect();
        let expected: Vec<&str> = Capability::HOST_FACING
            .iter()
            .filter_map(|capability| capability.flag_name())
            .collect();
        assert_eq!(
            listed, expected,
            "packages/types/runtime-process.d.ts is out of date"
        );
    }

    /// Every non-standard `WorkerOptions` member has to be described where an
    /// editor will find it. `memory` shipped without types once already.
    #[test]
    fn worker_options_are_typed() {
        let source = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../packages/types/globals.d.ts"
        ))
        .expect("read globals.d.ts");
        for member in [
            "permissions?:",
            "env?:",
            "memory?:",
            "unref(): void",
            "ref(): void",
        ] {
            assert!(
                source.contains(member),
                "packages/types/globals.d.ts does not declare WorkerOptions {member}"
            );
        }
        assert!(
            source.contains(r#""inherit" | readonly import("runtime:process").PermissionName[]"#),
            "permissions should be typed as \"inherit\" or the shared name union"
        );
    }
}
