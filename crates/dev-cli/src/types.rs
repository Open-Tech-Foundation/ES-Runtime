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
/// Three sources, in the order of how much they mean:
///
/// 1. **`"packageManager"` in package.json** — the project *saying* which one
///    it uses, the field corepack reads and every modern toolchain writes. It
///    is first because it is a statement of intent rather than a trace, and
///    because it is there before a lockfile is: a fresh clone, a scaffolded
///    project, anything CI has not installed yet.
/// 2. **The lockfile** — durable evidence of what actually installed. Still
///    ahead of anything guessed, and the only source most older projects have.
/// 3. **What is on this machine** — when the project says nothing either way,
///    a manager that is installed beats one that is not.
///
/// npm is the last word rather than the first: it is the one usually there, and
/// "usually" is exactly the assumption that produces `npm: command not found`
/// in a container that ships only bun.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum PackageManager {
    Bun,
    Pnpm,
    Yarn,
    Npm,
}

impl PackageManager {
    /// The program's name, which is also the name it is declared under.
    fn name(self) -> &'static str {
        match self {
            PackageManager::Bun => "bun",
            PackageManager::Pnpm => "pnpm",
            PackageManager::Yarn => "yarn",
            PackageManager::Npm => "npm",
        }
    }

    fn from_name(name: &str) -> Option<PackageManager> {
        [
            PackageManager::Bun,
            PackageManager::Pnpm,
            PackageManager::Yarn,
            PackageManager::Npm,
        ]
        .into_iter()
        .find(|manager| manager.name().eq_ignore_ascii_case(name))
    }

    fn detect() -> PackageManager {
        Self::detected(
            || std::fs::read_to_string("package.json").ok(),
            |name| Path::new(name).exists(),
            || {
                crate::install::available()
                    .into_iter()
                    .map(|manager| manager.name)
                    .collect()
            },
        )
    }

    /// The detection itself, over its three sources, so a test can ask it about
    /// a project and a machine that do not exist.
    fn detected(
        manifest: impl Fn() -> Option<String>,
        exists: impl Fn(&str) -> bool,
        installed: impl Fn() -> Vec<&'static str>,
    ) -> PackageManager {
        if let Some(declared) = manifest().as_deref().and_then(Self::declared) {
            return declared;
        }
        if let Some(locked) = Self::from_lockfiles(exists) {
            return locked;
        }
        // Nothing said, so the question stops being "which does this project
        // use" and becomes "which can this machine run".
        if let Some(present) = installed().into_iter().find_map(Self::from_name) {
            return present;
        }
        PackageManager::Npm
    }

    /// The `packageManager` field, as corepack defines it: a name, `@`, and a
    /// version that may carry a hash — `bun@1.3.14`,
    /// `pnpm@9.0.0+sha512.abc…`. Only the name is of any use here; which
    /// *version* to run is corepack's business and not this command's.
    ///
    /// A field naming something this does not know — a manager added later, a
    /// typo — is no answer rather than an error: the lockfile below may well
    /// know, and a command whose whole job is installing one dev dependency
    /// should not refuse over a field it merely failed to recognise.
    fn declared(manifest: &str) -> Option<PackageManager> {
        let manifest: serde_json::Value = serde_json::from_str(manifest).ok()?;
        let declared = manifest.get("packageManager")?.as_str()?;
        Self::from_name(declared.split('@').next()?.trim())
    }

    /// The lockfile, or `None` where there is not one yet.
    fn from_lockfiles(exists: impl Fn(&str) -> bool) -> Option<PackageManager> {
        // Ordered by how specific the evidence is. A repository that has more
        // than one lockfile has a problem this command cannot solve; picking the
        // first match is at least deterministic.
        [
            ("bun.lock", PackageManager::Bun),
            ("bun.lockb", PackageManager::Bun),
            ("pnpm-lock.yaml", PackageManager::Pnpm),
            ("yarn.lock", PackageManager::Yarn),
            ("package-lock.json", PackageManager::Npm),
        ]
        .into_iter()
        .find_map(|(lockfile, manager)| exists(lockfile).then_some(manager))
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

    /// Nothing declared and nothing installed on the machine, so the answer is
    /// whatever the sources under test say.
    fn detect(manifest: &str, lockfile: &'static str) -> PackageManager {
        let manifest = manifest.to_string();
        PackageManager::detected(
            move || Some(manifest.clone()),
            move |name| name == lockfile,
            Vec::new,
        )
    }

    #[test]
    fn a_lockfile_decides_when_nothing_is_declared() {
        assert_eq!(detect("{}", "bun.lock"), PackageManager::Bun);
        assert_eq!(detect("{}", "bun.lockb"), PackageManager::Bun);
        assert_eq!(detect("{}", "pnpm-lock.yaml"), PackageManager::Pnpm);
        assert_eq!(detect("{}", "yarn.lock"), PackageManager::Yarn);
        assert_eq!(detect("{}", "package-lock.json"), PackageManager::Npm);
    }

    /// The field a project writes before it has installed anything: a fresh
    /// clone, a scaffold, a CI job at its first step. It is the project saying
    /// which manager it uses, so it is read ahead of the traces of one.
    #[test]
    fn the_package_manager_field_is_read_first() {
        let declared = |field: &str| {
            detect(
                &format!(r#"{{ "packageManager": "{field}" }}"#),
                "package-lock.json",
            )
        };
        assert_eq!(declared("bun@1.3.14"), PackageManager::Bun);
        assert_eq!(declared("pnpm@9.0.0+sha512.abcdef"), PackageManager::Pnpm);
        assert_eq!(declared("yarn@4.1.0"), PackageManager::Yarn);
        // Corepack's own shape is `name@version`; a bare name is still a name.
        assert_eq!(declared("bun"), PackageManager::Bun);
    }

    /// A field this does not recognise is not an error and not an answer: the
    /// lockfile below may well know, and a command that installs one dev
    /// dependency should not refuse over a field it merely failed to parse.
    #[test]
    fn an_unreadable_field_falls_through_to_the_lockfile() {
        assert_eq!(
            detect(r#"{ "packageManager": "cnpm@1.0.0" }"#, "yarn.lock"),
            PackageManager::Yarn
        );
        assert_eq!(
            detect(r#"{ "packageManager": 7 }"#, "yarn.lock"),
            PackageManager::Yarn
        );
        assert_eq!(
            detect("{ not json at all", "yarn.lock"),
            PackageManager::Yarn
        );
        // …and with no lockfile either, it reaches the machine and then npm.
        assert_eq!(
            detect(r#"{ "packageManager": "@" }"#, ""),
            PackageManager::Npm
        );
    }

    /// With the project silent, the question is no longer which manager it uses
    /// but which one can run at all. npm is the last word rather than the
    /// first: a container that ships only bun has no npm to fall back to.
    #[test]
    fn nothing_declared_or_locked_takes_a_manager_that_is_installed() {
        let on_this_machine = |installed: &'static [&'static str]| {
            PackageManager::detected(
                || Some("{}".to_string()),
                |_| false,
                move || installed.to_vec(),
            )
        };
        assert_eq!(on_this_machine(&["bun", "yarn"]), PackageManager::Bun);
        assert_eq!(on_this_machine(&["npm", "bun"]), PackageManager::Npm);
        // Nothing at all: npm, and the failed spawn reports the line to run.
        assert_eq!(on_this_machine(&[]), PackageManager::Npm);
    }

    /// A project with no package.json never reaches here — `install` refuses
    /// first — but the reader is fallible for other reasons (permissions), and
    /// that must not be a panic.
    #[test]
    fn an_unreadable_manifest_is_not_a_failure() {
        assert_eq!(
            PackageManager::detected(|| None, |name| name == "bun.lock", Vec::new),
            PackageManager::Bun
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
