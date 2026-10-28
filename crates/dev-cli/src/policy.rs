//! The import policy applied to what a build ships (DECISIONS D193).
//!
//! The runtime loader applies a policy as it resolves each module (D39), and a
//! build inlines every package, so a deployed bundle never asks the loader for
//! one. The same policy file is applied here instead, to every module the
//! bundler put into an output: what the build *ships*, not what is installed
//! and not the build tools that ran — none of their code is in the output, so
//! there is nothing of theirs to approve.
//!
//! What is judged is the bundle's **module graph** — every module parsed into
//! it, recorded by [`Graph`] — not the list of modules an output chunk
//! reports. The bundler drops a module from that list when it folds the
//! module away entirely: a package exporting a constant is inlined into its
//! importer and no longer "in" a chunk, though its code decided what shipped.
//! Judging the rendered list let a denied package through that way.
//!
//! A module the bundler made up rather than read — a plugin's virtual module —
//! is not a file and is skipped: it is the plugin's output, and the plugin is
//! a build tool.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

use es_runtime_default_providers::{ImportPolicy, Unlisted, Verdict, package_of};
use rolldown::plugin::{HookNoopReturn, HookUsage, Plugin, PluginContext};

/// The modules parsed into a bundle's graph, collected by a bundler plugin.
///
/// A held dev bundler re-parses only what changed, so its graph accumulates
/// across rebuilds: an import removed in the loop stays judged until the
/// bundler is rebuilt, which can over-report and never misses.
#[derive(Clone, Debug, Default)]
pub struct Graph(Arc<Mutex<BTreeSet<String>>>);

impl Graph {
    /// The bundler plugin that records into this graph.
    pub fn plugin(&self) -> Arc<dyn rolldown::plugin::Pluginable> {
        Arc::new(Recorder(self.clone()))
    }

    fn ids(&self) -> Vec<String> {
        self.0
            .lock()
            .map(|ids| ids.iter().cloned().collect())
            .unwrap_or_default()
    }
}

#[derive(Debug)]
struct Recorder(Graph);

impl Plugin for Recorder {
    fn name(&self) -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("esdev:import-policy")
    }

    fn register_hook_usage(&self) -> HookUsage {
        HookUsage::ModuleParsed
    }

    async fn module_parsed(
        &self,
        _ctx: &PluginContext,
        module_info: Arc<rolldown_common::ModuleInfo>,
        _normal_module: &rolldown_common::NormalModule,
    ) -> HookNoopReturn {
        if let Ok(mut ids) = (self.0).0.lock() {
            ids.insert(crate::adapter::guest_id(module_info.id.as_ref()).to_string());
        }
        Ok(())
    }
}

/// The policy at `path`, applied to what `graph` read into a bundle. `Ok`
/// carries the warnings to print when the policy reports rather than refuses
/// (`"unlisted": "warn"`); a refusal names every module refused, at once.
pub fn check(path: &Path, graph: &Graph, root: &Path) -> Result<Vec<String>, String> {
    let shown = shown_path(path, root);
    let policy = ImportPolicy::from_file(path)?;
    let mut denied = BTreeSet::new();
    let mut unlisted = BTreeSet::new();
    // Each installed copy of a package the policy pins, by its directory.
    let mut pinned: BTreeMap<PathBuf, String> = BTreeMap::new();
    for id in &graph.ids() {
        // A query (`?raw`, `?url`) names a way of loading a file, not a
        // different file.
        let file = Path::new(id.split('?').next().unwrap_or(id));
        if !file.is_absolute() {
            continue;
        }
        let Ok(real) = es_runtime_default_providers::path::canonicalize(file) else {
            continue;
        };
        let name = package_of(&real).unwrap_or_else(|| shown_path(&real, root));
        match policy.judge(&real) {
            Verdict::Allowed => {
                if let Some(dir) = package_dir(&real)
                    && !policy.pins(&name).is_empty()
                {
                    pinned.insert(dir, name);
                }
            }
            Verdict::Denied => {
                denied.insert(name);
            }
            Verdict::Unlisted => {
                unlisted.insert(name);
            }
        }
    }
    let mut mismatched = BTreeSet::new();
    for (dir, name) in &pinned {
        if let Some(why) = mismatch(&policy, name, dir)? {
            mismatched.insert(why);
        }
    }
    let strict = policy.unlisted() == Unlisted::Error;
    if denied.is_empty() && mismatched.is_empty() && (unlisted.is_empty() || !strict) {
        return Ok(unlisted
            .into_iter()
            .map(|name| {
                format!(
                    "{name} is not in the import policy's allow list ({shown}); \
                     it shipped because the policy's \"unlisted\" is \"warn\""
                )
            })
            .collect());
    }
    let mut message = format!("the import policy ({shown}) refuses what this build ships:");
    if !denied.is_empty() {
        message.push_str(&format!("\n  denied: {}", join(&denied)));
    }
    if !mismatched.is_empty() {
        message.push_str(&format!("\n  not the approved copy: {}", join(&mismatched)));
    }
    if strict && !unlisted.is_empty() {
        message.push_str(&format!("\n  not in the allow list: {}", join(&unlisted)));
    }
    message.push_str(
        "\n\nA denied package has to go. One that is not listed is approved by adding it \
         to \"allow\"; `npm explain <package>` (or `pnpm why`) shows what brought it in. \
         One that is not the approved copy was installed at another version, or its files \
         changed since its hash was taken.",
    );
    Err(message)
}

/// Why the copy of `name` installed at `dir` is not one the policy approved,
/// or `None` when it is: its `version` matches an entry's, and so does the
/// hash of its files where that entry gives one. The hash is taken once, and
/// only if an entry asks for it.
fn mismatch(policy: &ImportPolicy, name: &str, dir: &Path) -> Result<Option<String>, String> {
    let version = installed_version(dir)?;
    let mut hash: Option<String> = None;
    let mut version_matched = false;
    for pin in policy.pins(name) {
        if pin
            .version
            .as_ref()
            .is_some_and(|wanted| *wanted != version)
        {
            continue;
        }
        version_matched = true;
        let Some(wanted) = &pin.integrity else {
            return Ok(None);
        };
        if hash.is_none() {
            hash = Some(h1(dir, &format!("{name}@{version}"))?);
        }
        if hash.as_ref() == Some(wanted) {
            return Ok(None);
        }
    }
    Ok(Some(if version_matched {
        format!("{name}@{version} (its files do not match the approved integrity)")
    } else {
        let approved: Vec<&str> = policy
            .pins(name)
            .into_iter()
            .filter_map(|pin| pin.version.as_deref())
            .collect();
        format!("{name}@{version} (approved: {})", approved.join(", "))
    }))
}

/// The `version` in the installed package's `package.json`.
fn installed_version(dir: &Path) -> Result<String, String> {
    let manifest = dir.join("package.json");
    let text = std::fs::read_to_string(&manifest)
        .map_err(|e| format!("cannot read {}: {e}", manifest.display()))?;
    let json: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("{} is not JSON: {e}", manifest.display()))?;
    json.get("version")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("{} has no \"version\"", manifest.display()))
}

/// The installed package directory a module belongs to: `…/node_modules/pkg`
/// or `…/node_modules/@scope/pkg`, after the **last** `node_modules`.
fn package_dir(real: &Path) -> Option<PathBuf> {
    let components: Vec<Component<'_>> = real.components().collect();
    let last = components
        .iter()
        .rposition(|c| c.as_os_str() == "node_modules")?;
    let first = components.get(last + 1)?.as_os_str().to_str()?;
    let len = if first.starts_with('@') {
        last + 3
    } else {
        last + 2
    };
    (components.len() > len).then(|| components[..len].iter().collect())
}

/// Go's `dirhash.Hash1` of the package at `dir`, its files named under
/// `prefix/` (`name@version/`), as `go.sum` pins a module (D193): the SHA-256,
/// in standard base64 after `h1:`, of one line per file — the file's SHA-256
/// in hex, two spaces, its name, a newline — sorted by name.
///
/// Regular files only. A nested `node_modules` is another package, pinned by
/// its own entry, and is left out.
pub fn h1(dir: &Path, prefix: &str) -> Result<String, String> {
    use base64::Engine as _;
    use sha2::{Digest, Sha256};

    let mut names = Vec::new();
    collect_files(dir, dir, prefix, &mut names)?;
    names.sort();
    let mut summary = Sha256::new();
    for (name, path) in &names {
        if name.contains('\n') {
            return Err(format!(
                "{}: a file name with a newline cannot be hashed",
                path.display()
            ));
        }
        let bytes =
            std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let digest = Sha256::digest(&bytes);
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        summary.update(format!("{hex}  {name}\n").as_bytes());
    }
    Ok(format!(
        "h1:{}",
        base64::engine::general_purpose::STANDARD.encode(summary.finalize())
    ))
}

fn collect_files(
    root: &Path,
    dir: &Path,
    prefix: &str,
    out: &mut Vec<(String, PathBuf)>,
) -> Result<(), String> {
    let entries =
        std::fs::read_dir(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
        let path = entry.path();
        let kind = entry
            .file_type()
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        if kind.is_dir() {
            if entry.file_name() != "node_modules" {
                collect_files(root, &path, prefix, out)?;
            }
        } else if kind.is_file() {
            let relative = path.strip_prefix(root).unwrap_or(&path);
            let name = relative
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            out.push((format!("{prefix}/{name}"), path));
        }
    }
    Ok(())
}

fn join(names: &BTreeSet<String>) -> String {
    names.iter().cloned().collect::<Vec<_>>().join(", ")
}

/// `path` as the reader of this project would write it: `./`-relative to the
/// root where it is inside it.
fn shown_path(path: &Path, root: &Path) -> String {
    let root = es_runtime_default_providers::path::canonicalize(root)
        .unwrap_or_else(|_| root.to_path_buf());
    match path.strip_prefix(&root) {
        Ok(relative) => format!("./{}", relative.to_string_lossy().replace('\\', "/")),
        Err(_) => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fixture a Go program hashed with `golang.org/x/mod/sumdb/dirhash`
    /// (v0.24.0, `HashDir(dir, "zod@3.23.8", Hash1)`), and its answer. An
    /// uppercase name sorts first, as Go's byte-wise sort puts it.
    const GO_H1: &str = "h1:MqlKs+Ul5e3twLZxAsDHxdUENULXjG0nDnnk4B/VuZA=";

    fn fixture(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("esdev-h1-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("lib")).unwrap();
        std::fs::create_dir_all(dir.join("z")).unwrap();
        std::fs::write(
            dir.join("package.json"),
            r#"{"name":"zod","version":"3.23.8"}"#,
        )
        .unwrap();
        std::fs::write(dir.join("index.js"), "export const a = 1;\n").unwrap();
        std::fs::write(dir.join("lib/util.js"), "export {};\n").unwrap();
        std::fs::write(dir.join("z/empty.txt"), "").unwrap();
        std::fs::write(dir.join("B.md"), "B").unwrap();
        dir
    }

    #[test]
    fn h1_is_go_s_dirhash() {
        let dir = fixture("go");
        assert_eq!(h1(&dir, "zod@3.23.8").unwrap(), GO_H1);
    }

    #[test]
    fn h1_leaves_out_a_nested_node_modules_and_sees_any_change() {
        let dir = fixture("nested");
        std::fs::create_dir_all(dir.join("node_modules/dep")).unwrap();
        std::fs::write(dir.join("node_modules/dep/index.js"), "x").unwrap();
        assert_eq!(
            h1(&dir, "zod@3.23.8").unwrap(),
            GO_H1,
            "another package's files are not this one's"
        );
        std::fs::write(dir.join("lib/util.js"), "export {}; // changed\n").unwrap();
        assert_ne!(h1(&dir, "zod@3.23.8").unwrap(), GO_H1);
        let dir = fixture("prefix");
        assert_ne!(
            h1(&dir, "zod@3.24.0").unwrap(),
            GO_H1,
            "the version is part of what is hashed"
        );
    }

    #[test]
    fn a_package_dir_is_after_the_last_node_modules() {
        let p = |s: &str| package_dir(Path::new(s)).map(|d| d.to_string_lossy().replace('\\', "/"));
        assert_eq!(
            p("/p/node_modules/zod/lib/a.js").as_deref(),
            Some("/p/node_modules/zod")
        );
        assert_eq!(
            p("/p/node_modules/@acme/ui/a.js").as_deref(),
            Some("/p/node_modules/@acme/ui")
        );
        assert_eq!(
            p("/p/node_modules/a/node_modules/b/i.js").as_deref(),
            Some("/p/node_modules/a/node_modules/b")
        );
        assert_eq!(p("/p/src/a.js"), None);
    }
}
