//! The **import & loader policy** — what a run is allowed to *load*
//! (DECISIONS D39).
//!
//! This is deliberately not part of the capability model. Capabilities answer
//! "what may executing code reach"; a policy answers "which modules may become
//! executing code in the first place". Two layers, not two alternatives:
//!
//! - the `imports` **capability** decides whether the loader runs at all — it
//!   is what makes `--deny-all` mean "runs only the file you named";
//! - the **policy** decides, given that it runs, what it may resolve.
//!
//! Keeping them apart is what lets the policy grow rules a permission model
//! must never have — a warn-only mode, aliases, replacements — without any of
//! that leaking into a security check, where "warn and continue" would be the
//! sandbox that silently isn't one.
//!
//! **Entries are read the way specifiers are.** An entry beginning with `.` or
//! `/` is a path covering its subtree; anything else is a package name
//! (`lodash`, `@scope/pkg`). That is the split the loader already makes between
//! a bare and a relative specifier, so there is no second grammar to learn. The
//! two kinds are alternatives, not territories: a module is named by a rule if
//! *either* list names it, so a path entry that happens to point inside a
//! `node_modules` tree governs what is there like any other path entry.
//!
//! **Matching runs on the resolved, canonicalized module**, after the sandbox
//! root has been enforced: judging the specifier would let
//! `./src/link-to-node_modules/evil/index.js` through, and resolving first is
//! also what makes package matching work under pnpm, whose `node_modules/pkg`
//! is a symlink into a content store — the real path still ends in
//! `…/node_modules/lodash/…`, so the package is recoverable from the path.

use std::path::{Component, Path};

use es_runtime_common::ErrorCode;
use es_runtime_providers::ProviderError;
use serde_json::Value;

use crate::path_allowlist::PathAllowlist;

/// One side of a policy: the packages and paths a rule names.
#[derive(Clone, Debug, Default)]
struct Rules {
    /// Package names as written in a bare specifier (`lodash`, `@scope/pkg`).
    packages: Vec<String>,
    /// Path entries, canonicalized — the same matching `--allow-read` uses.
    paths: PathAllowlist,
}

impl Rules {
    fn parse<I, S>(entries: I, base: &Path) -> Result<Rules, String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut packages = Vec::new();
        let mut paths = Vec::new();
        for entry in entries {
            let entry = entry.as_ref();
            if entry.is_empty() {
                return Err("an empty entry is neither a package nor a path".to_string());
            }
            if entry.starts_with('.') || entry.starts_with('/') || entry.starts_with("file:") {
                paths.push(entry.to_string());
            } else {
                packages.push(entry.trim_end_matches('/').to_string());
            }
        }
        Ok(Rules {
            packages,
            paths: PathAllowlist::parse(paths, base)?,
        })
    }

    fn is_empty(&self) -> bool {
        self.packages.is_empty() && self.paths.is_empty()
    }

    /// Whether the module at `real` is named by these rules — by either list.
    ///
    /// A module inside a `node_modules` tree is *also* judged as the package it
    /// belongs to, which is the only way a bare `"lodash"` can match a path. But
    /// the path list still applies there: reading a `node_modules` module as a
    /// package *instead of* a path made every path entry pointing into one
    /// silently inert, so a `{"deny": ["./node_modules/aws-sdk"]}` denied
    /// nothing at all and reported nothing to say so — a policy that reads as a
    /// restriction and is not one.
    fn matches(&self, real: &Path) -> bool {
        if let Some(name) = package_of(real)
            && self.packages.contains(&name)
        {
            return true;
        }
        self.paths.permits(real)
    }
}

/// What a run may load. The default permits everything — a policy applies only
/// when one is supplied.
#[derive(Clone, Debug, Default)]
pub struct ImportPolicy {
    /// The allow list. `None` when the file omits `"allow"`, which means
    /// "everything not denied" — the shape for a policy that only wants to
    /// exclude a few packages.
    allow: Option<Rules>,
    /// The deny list. Always applies, and **wins over allow** (see
    /// [`permits`](Self::permits)).
    deny: Rules,
}

impl ImportPolicy {
    /// Reads and parses a policy file. Paths inside it resolve **relative to
    /// the file**, not to the working directory: a policy is committed next to
    /// the project it governs, and should mean the same thing wherever it is
    /// invoked from.
    pub fn from_file(path: &Path) -> Result<ImportPolicy, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read import policy {}: {e}", path.display()))?;
        let base = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        ImportPolicy::from_json(&text, &base)
            .map_err(|e| format!("import policy {}: {e}", path.display()))
    }

    /// Parses a policy from JSON, resolving relative paths against `base`.
    ///
    /// Unknown keys are an **error**, not something to ignore: a misspelled
    /// `"allowed"` that parsed as an empty policy would be read as protection
    /// that is not there — the same reason the argument parser refuses a flag
    /// it does not know.
    pub fn from_json(text: &str, base: &Path) -> Result<ImportPolicy, String> {
        let value: Value = serde_json::from_str(text).map_err(|e| format!("invalid JSON: {e}"))?;
        let object = value
            .as_object()
            .ok_or_else(|| "the policy must be a JSON object".to_string())?;
        for key in object.keys() {
            if key != "allow" && key != "deny" {
                return Err(format!(
                    "unknown key {key:?} (expected \"allow\" and/or \"deny\")"
                ));
            }
        }
        let allow = match object.get("allow") {
            Some(value) => Some(Rules::parse(string_list("allow", value)?, base)?),
            None => None,
        };
        let deny = match object.get("deny") {
            Some(value) => Rules::parse(string_list("deny", value)?, base)?,
            None => Rules::default(),
        };
        if allow.as_ref().is_some_and(Rules::is_empty) {
            // `{"allow": []}` denies every import, which is legitimate to write
            // but almost never what someone means — say so here rather than at
            // the first import, with no explanation.
            return Err(
                "\"allow\" is empty, so nothing may be imported. Omit \"allow\" to permit \
                 everything not denied, or list what may load."
                    .to_string(),
            );
        }
        Ok(ImportPolicy { allow, deny })
    }

    /// Whether the module at `real` (resolved and canonicalized) may load.
    ///
    /// **Deny wins.** With both lists present the file reads top to bottom and
    /// there is one rule to remember; the alternative — "the more specific
    /// entry wins" — makes the answer depend on comparing two entries, which is
    /// exactly the precedence reasoning D38 removed from the flags.
    pub(crate) fn permits(&self, real: &Path) -> bool {
        if self.deny.matches(real) {
            return false;
        }
        match &self.allow {
            Some(allow) => allow.matches(real),
            None => true,
        }
    }

    /// [`permits`](Self::permits), as a provider error naming what was refused
    /// and which list refused it.
    pub(crate) fn check(&self, real: &Path) -> Result<(), ProviderError> {
        if self.permits(real) {
            return Ok(());
        }
        let what = match package_of(real) {
            Some(name) => format!("package {name}"),
            None => real.display().to_string(),
        };
        let why = if self.deny.matches(real) {
            "denied by the import policy"
        } else {
            "not in the import policy's allow list"
        };
        Err(ProviderError::Coded {
            code: ErrorCode::PermissionDenied,
            message: format!("{what} is {why}"),
        })
    }
}

/// Reads a JSON array of strings, naming the key when it is anything else.
fn string_list(key: &str, value: &Value) -> Result<Vec<String>, String> {
    let array = value
        .as_array()
        .ok_or_else(|| format!("{key:?} must be an array of strings"))?;
    array
        .iter()
        .map(|entry| {
            entry
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| format!("{key:?} must contain only strings"))
        })
        .collect()
}

/// The package a resolved module belongs to, if it is inside a `node_modules`
/// tree: the one or two components after the **last** `node_modules`, so a
/// scoped package (`@scope/pkg`) keeps its scope and a nested dependency is
/// named as itself rather than as its parent.
fn package_of(real: &Path) -> Option<String> {
    let components: Vec<&std::ffi::OsStr> = real
        .components()
        .filter_map(|c| match c {
            Component::Normal(name) => Some(name),
            _ => None,
        })
        .collect();
    let last = components
        .iter()
        .rposition(|name| *name == std::ffi::OsStr::new("node_modules"))?;
    let first = components.get(last + 1)?.to_str()?;
    if first.starts_with('@') {
        let second = components.get(last + 2)?.to_str()?;
        return Some(format!("{first}/{second}"));
    }
    Some(first.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("esrt-importpolicy-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        crate::path::canonicalize(&dir).unwrap()
    }

    fn policy(json: &str, base: &Path) -> ImportPolicy {
        ImportPolicy::from_json(json, base).expect("parse")
    }

    #[test]
    fn an_allow_list_admits_named_packages_and_refuses_the_rest() {
        let root = temp_dir("allow");
        let p = policy(r#"{ "allow": ["lodash"] }"#, &root);
        assert!(p.permits(Path::new("/proj/node_modules/lodash/index.js")));
        assert!(p.permits(Path::new("/proj/node_modules/lodash/fp/map.js")));
        assert!(!p.permits(Path::new("/proj/node_modules/left-pad/index.js")));
    }

    #[test]
    fn omitting_allow_permits_everything_not_denied() {
        // The deny-only shape: an organisation excluding a handful of packages
        // without having to enumerate the whole graph.
        let root = temp_dir("deny-only");
        let p = policy(r#"{ "deny": ["aws-sdk"] }"#, &root);
        assert!(p.permits(Path::new("/proj/node_modules/express/index.js")));
        assert!(!p.permits(Path::new("/proj/node_modules/aws-sdk/index.js")));
    }

    #[test]
    fn deny_wins_over_allow() {
        // One rule to remember, rather than an answer that depends on comparing
        // two entries for specificity.
        let root = temp_dir("precedence");
        let p = policy(
            r#"{ "allow": ["lodash", "aws-sdk"], "deny": ["aws-sdk"] }"#,
            &root,
        );
        assert!(p.permits(Path::new("/proj/node_modules/lodash/index.js")));
        assert!(!p.permits(Path::new("/proj/node_modules/aws-sdk/index.js")));
    }

    #[test]
    fn a_nested_dependency_is_named_as_itself() {
        // Allowing a package is not allowing what it imports.
        let root = temp_dir("nested");
        let p = policy(r#"{ "allow": ["lodash"] }"#, &root);
        assert!(!p.permits(Path::new(
            "/proj/node_modules/lodash/node_modules/inner/index.js"
        )));
    }

    #[test]
    fn a_scoped_package_keeps_its_scope() {
        let root = temp_dir("scoped");
        let p = policy(r#"{ "allow": ["@acme/ui"] }"#, &root);
        assert!(p.permits(Path::new("/proj/node_modules/@acme/ui/index.js")));
        assert!(!p.permits(Path::new("/proj/node_modules/@acme/other/index.js")));
        assert!(!p.permits(Path::new("/proj/node_modules/ui/index.js")));
    }

    #[test]
    fn a_pnpm_store_path_still_names_its_package() {
        let root = temp_dir("pnpm");
        let p = policy(r#"{ "allow": ["lodash"] }"#, &root);
        assert!(p.permits(Path::new(
            "/proj/node_modules/.pnpm/lodash@4.17.21/node_modules/lodash/index.js"
        )));
    }

    #[test]
    fn path_entries_resolve_against_the_policy_file() {
        let root = temp_dir("paths");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("vendor")).unwrap();
        let p = policy(r#"{ "allow": ["./src"] }"#, &root);
        assert!(p.permits(&root.join("src/app.mjs")));
        assert!(p.permits(&root.join("src/lib/util.mjs")));
        assert!(!p.permits(&root.join("vendor/thing.mjs")));
    }

    #[test]
    fn packages_and_paths_can_be_mixed() {
        let root = temp_dir("mixed");
        std::fs::create_dir_all(root.join("src")).unwrap();
        let p = policy(r#"{ "allow": ["./src", "lodash"] }"#, &root);
        assert!(p.permits(&root.join("src/app.mjs")));
        assert!(p.permits(Path::new("/proj/node_modules/lodash/index.js")));
    }

    #[test]
    fn a_path_entry_governs_a_node_modules_tree_like_any_other() {
        // The rule that was silently inert: reading a `node_modules` module as
        // a package *instead of* a path meant this denied nothing, and said so
        // nowhere.
        let root = temp_dir("nm-path");
        std::fs::create_dir_all(root.join("node_modules/aws-sdk")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/express")).unwrap();
        let p = policy(r#"{ "deny": ["./node_modules/aws-sdk"] }"#, &root);
        assert!(!p.permits(&root.join("node_modules/aws-sdk/index.js")));
        assert!(p.permits(&root.join("node_modules/express/index.js")));
    }

    #[test]
    fn a_path_entry_can_allow_a_vendored_package() {
        // The same rule read the other way: a package under a directory the
        // policy allows loads, without having to name every package in it.
        let root = temp_dir("nm-allow");
        std::fs::create_dir_all(root.join("node_modules/left-pad")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        let p = policy(
            r#"{ "allow": ["./src", "./node_modules/left-pad"] }"#,
            &root,
        );
        assert!(p.permits(&root.join("node_modules/left-pad/index.js")));
        assert!(p.permits(&root.join("src/app.mjs")));
        // And nothing outside either entry.
        assert!(!p.permits(Path::new("/proj/node_modules/lodash/index.js")));
    }

    #[test]
    fn no_policy_permits_everything() {
        assert!(ImportPolicy::default().permits(Path::new("/proj/node_modules/anything/i.js")));
    }

    #[test]
    fn an_unknown_key_is_an_error() {
        // A misspelled "allowed" that parsed as an empty policy would read as
        // protection that is not there.
        let root = temp_dir("unknown-key");
        let err = ImportPolicy::from_json(r#"{ "allowed": ["lodash"] }"#, &root).unwrap_err();
        assert!(err.contains("unknown key"), "{err}");
        assert!(err.contains("allowed"), "{err}");
    }

    #[test]
    fn malformed_input_is_an_error_naming_the_problem() {
        let root = temp_dir("malformed");
        for (json, expected) in [
            ("[]", "must be a JSON object"),
            ("{", "invalid JSON"),
            (r#"{ "allow": "lodash" }"#, "must be an array"),
            (r#"{ "allow": [1] }"#, "must contain only strings"),
            (r#"{ "allow": [""] }"#, "empty entry"),
            (r#"{ "allow": [] }"#, "nothing may be imported"),
        ] {
            let err = ImportPolicy::from_json(json, &root).unwrap_err();
            assert!(err.contains(expected), "{json} → {err}");
        }
    }

    #[test]
    fn check_says_which_list_refused() {
        let root = temp_dir("message");
        let p = policy(r#"{ "allow": ["lodash"], "deny": ["aws-sdk"] }"#, &root);
        let denied = p
            .check(Path::new("/proj/node_modules/aws-sdk/index.js"))
            .unwrap_err();
        assert!(
            denied.to_string().contains("denied by the import policy"),
            "{denied}"
        );
        let unlisted = p
            .check(Path::new("/proj/node_modules/other/index.js"))
            .unwrap_err();
        assert!(
            unlisted
                .to_string()
                .contains("not in the import policy's allow list"),
            "{unlisted}"
        );
        assert_eq!(unlisted.code(), Some(ErrorCode::PermissionDenied));
    }
}
