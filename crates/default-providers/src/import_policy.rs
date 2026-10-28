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
//! **A scope entry, `@acme/*`, names every package in the scope**, so an
//! organisation's own packages are one line (D193). It is the only pattern:
//! `*` anywhere else is refused rather than read literally.
//!
//! **What a policy does with a package it does not list** is written in the
//! file (D193): with no `allow`, everything not denied loads; with `allow`,
//! `"unlisted": "error"` (the default) refuses it and `"unlisted": "warn"` lets
//! it load and says so, the way a policy is rolled out. A denied module is
//! refused in every mode. `esdev build` applies the same file to what a build
//! ships, through [`ImportPolicy::judge`].
//!
//! **Matching runs on the resolved, canonicalized module**, after the sandbox
//! root has been enforced: judging the specifier would let
//! `./src/link-to-node_modules/evil/index.js` through, and resolving first is
//! also what makes package matching work under pnpm, whose `node_modules/pkg`
//! is a symlink into a content store — the real path still ends in
//! `…/node_modules/lodash/…`, so the package is recoverable from the path.

use std::collections::HashSet;
use std::path::{Component, Path};
use std::sync::Mutex;

use es_runtime_common::ErrorCode;
use es_runtime_providers::ProviderError;
use serde_json::Value;

use crate::path_allowlist::PathAllowlist;

/// One side of a policy: the packages and paths a rule names.
#[derive(Clone, Debug, Default)]
struct Rules {
    /// Package names as written in a bare specifier (`lodash`, `@scope/pkg`).
    packages: Vec<String>,
    /// Scopes from `@scope/*` entries, as `@scope`.
    scopes: Vec<String>,
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
        let mut scopes = Vec::new();
        let mut paths = Vec::new();
        for entry in entries {
            let entry = entry.as_ref();
            if entry.is_empty() {
                return Err("an empty entry is neither a package nor a path".to_string());
            }
            if entry.starts_with('.') || entry.starts_with('/') || entry.starts_with("file:") {
                paths.push(entry.to_string());
            } else if let Some(scope) = entry.strip_suffix("/*")
                && scope.starts_with('@')
                && scope.len() > 1
                && !scope[1..].contains(['/', '*'])
            {
                scopes.push(scope.to_string());
            } else if entry.contains('*') {
                return Err(format!(
                    "{entry:?}: the only pattern is a whole scope, \"@scope/*\""
                ));
            } else {
                packages.push(entry.trim_end_matches('/').to_string());
            }
        }
        Ok(Rules {
            packages,
            scopes,
            paths: PathAllowlist::parse(paths, base)?,
        })
    }

    fn is_empty(&self) -> bool {
        self.packages.is_empty() && self.scopes.is_empty() && self.paths.is_empty()
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
            && (self.packages.contains(&name)
                || self.scopes.iter().any(|scope| {
                    name.strip_prefix(scope.as_str())
                        .is_some_and(|rest| rest.starts_with('/'))
                }))
        {
            return true;
        }
        self.paths.permits(real)
    }
}

/// What a policy does with a module its `allow` list does not name (D193).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Unlisted {
    /// Refuse it: the strict mode, and the default once `allow` is present.
    #[default]
    Error,
    /// Let it load and report it: how a policy is rolled out.
    Warn,
}

/// An `allow` entry written as an object (D193): a package approved at a
/// version, and optionally with the hash of its installed files.
///
/// Matching by name is all the runtime does with it; the version and the
/// hash are checked by `esdev build`, where a package is chosen into a
/// bundle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pin {
    /// The package, as a bare specifier names it.
    pub package: String,
    /// The `version` its installed `package.json` must carry.
    pub version: Option<String>,
    /// Go's `h1:` tree hash of its installed directory.
    pub integrity: Option<String>,
}

/// What a policy says about one module.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Named by `allow`, or there is no `allow` and `deny` does not name it.
    Allowed,
    /// Named by `deny`. Refused in every mode.
    Denied,
    /// Not named by `allow`: refused or reported, by the policy's
    /// [`Unlisted`] mode.
    Unlisted,
}

/// What a run may load. The default permits everything — a policy applies only
/// when one is supplied.
#[derive(Debug, Default)]
pub struct ImportPolicy {
    /// The allow list. `None` when the file omits `"allow"`, which means
    /// "everything not denied" — the shape for a policy that only wants to
    /// exclude a few packages.
    allow: Option<Rules>,
    /// The deny list. Always applies, and **wins over allow** (see
    /// [`permits`](Self::permits)).
    deny: Rules,
    /// What happens to a module `allow` does not name.
    unlisted: Unlisted,
    /// The `allow` entries written as objects, in file order.
    pins: Vec<Pin>,
    /// The packages a run has already reported under `"unlisted": "warn"`, so
    /// each is reported once rather than per module.
    reported: Mutex<HashSet<String>>,
}

impl Clone for ImportPolicy {
    fn clone(&self) -> Self {
        ImportPolicy {
            allow: self.allow.clone(),
            deny: self.deny.clone(),
            unlisted: self.unlisted,
            pins: self.pins.clone(),
            reported: Mutex::new(HashSet::new()),
        }
    }
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
            if !matches!(key.as_str(), "allow" | "deny" | "unlisted") {
                return Err(format!(
                    "unknown key {key:?} (expected \"allow\", \"deny\" and \"unlisted\")"
                ));
            }
        }
        let mut pins = Vec::new();
        let allow = match object.get("allow") {
            Some(value) => Some(Rules::parse(allow_entries(value, &mut pins)?, base)?),
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
        let unlisted = match object.get("unlisted") {
            None => Unlisted::Error,
            Some(Value::String(mode)) if mode == "error" => Unlisted::Error,
            Some(Value::String(mode)) if mode == "warn" => Unlisted::Warn,
            Some(_) => return Err("\"unlisted\" is \"error\" or \"warn\"".to_string()),
        };
        // Without `allow` nothing is unlisted, so a mode for it would be a
        // setting that does nothing — which reads as one that does.
        if allow.is_none() && object.contains_key("unlisted") {
            return Err(
                "\"unlisted\" says what happens to a package \"allow\" does not name, \
                 and there is no \"allow\""
                    .to_string(),
            );
        }
        Ok(ImportPolicy {
            allow,
            deny,
            unlisted,
            pins,
            reported: Mutex::new(HashSet::new()),
        })
    }

    /// The object entries approving `package`: one per approved copy, since an
    /// install can hold two versions of a package. Empty when the package is
    /// approved by name alone, or not at all.
    pub fn pins(&self, package: &str) -> Vec<&Pin> {
        self.pins
            .iter()
            .filter(|pin| pin.package == package)
            .collect()
    }

    /// What this policy does with a module `allow` does not name.
    pub fn unlisted(&self) -> Unlisted {
        self.unlisted
    }

    /// What the policy says about the module at `real` (resolved and
    /// canonicalized), before its mode is applied.
    pub fn judge(&self, real: &Path) -> Verdict {
        if self.deny.matches(real) {
            return Verdict::Denied;
        }
        match &self.allow {
            Some(allow) if !allow.matches(real) => Verdict::Unlisted,
            _ => Verdict::Allowed,
        }
    }

    /// Whether the module at `real` (resolved and canonicalized) may load.
    ///
    /// **Deny wins.** With both lists present the file reads top to bottom and
    /// there is one rule to remember; the alternative — "the more specific
    /// entry wins" — makes the answer depend on comparing two entries, which is
    /// exactly the precedence reasoning D38 removed from the flags.
    #[cfg(test)]
    pub(crate) fn permits(&self, real: &Path) -> bool {
        match self.judge(real) {
            Verdict::Allowed => true,
            Verdict::Denied => false,
            Verdict::Unlisted => self.unlisted == Unlisted::Warn,
        }
    }

    /// [`permits`](Self::permits), as a provider error naming what was refused
    /// and which list refused it.
    ///
    /// What is named is the package, or else the `specifier` the program wrote
    /// — never `real`, which is canonical (D141).
    pub(crate) fn check(&self, real: &Path, specifier: &str) -> Result<(), ProviderError> {
        let verdict = self.judge(real);
        let what = match package_of(real) {
            Some(name) => format!("package {name}"),
            None => format!("module {specifier:?}"),
        };
        match verdict {
            Verdict::Allowed => return Ok(()),
            Verdict::Unlisted if self.unlisted == Unlisted::Warn => {
                // Reported once per package (or module), not per file it loads.
                let first = self
                    .reported
                    .lock()
                    .map(|mut seen| seen.insert(what.clone()))
                    .unwrap_or(false);
                if first {
                    tracing::warn!(
                        target: "import-policy",
                        "{what} is not in the import policy's allow list \
                         (loaded: the policy's \"unlisted\" is \"warn\")"
                    );
                }
                return Ok(());
            }
            _ => {}
        }
        let why = if verdict == Verdict::Denied {
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

/// The `allow` list: strings, and objects approving a package at a version
/// (D193), whose names join the strings and whose pins are collected.
fn allow_entries(value: &Value, pins: &mut Vec<Pin>) -> Result<Vec<String>, String> {
    let array = value
        .as_array()
        .ok_or_else(|| "\"allow\" must be an array".to_string())?;
    let mut names = Vec::new();
    for entry in array {
        match entry {
            Value::String(name) => names.push(name.clone()),
            Value::Object(fields) => {
                let pin = pin(fields)?;
                names.push(pin.package.clone());
                pins.push(pin);
            }
            _ => {
                return Err(
                    "\"allow\" must contain only strings and { \"package\", … } objects"
                        .to_string(),
                );
            }
        }
    }
    Ok(names)
}

fn pin(fields: &serde_json::Map<String, Value>) -> Result<Pin, String> {
    for key in fields.keys() {
        if !matches!(key.as_str(), "package" | "version" | "integrity" | "reason") {
            return Err(format!(
                "unknown key {key:?} in an \"allow\" entry \
                 (expected \"package\", \"version\", \"integrity\" and \"reason\")"
            ));
        }
    }
    let text = |key: &str| -> Result<Option<String>, String> {
        match fields.get(key) {
            None => Ok(None),
            Some(Value::String(value)) if !value.is_empty() => Ok(Some(value.clone())),
            Some(_) => Err(format!(
                "an \"allow\" entry's {key:?} must be a non-empty string"
            )),
        }
    };
    let package =
        text("package")?.ok_or_else(|| "an \"allow\" entry needs a \"package\"".to_string())?;
    if package.starts_with('.') || package.starts_with('/') || package.contains('*') {
        return Err(format!(
            "an \"allow\" entry's \"package\" is one package name, not {package:?}"
        ));
    }
    let integrity = text("integrity")?;
    if let Some(hash) = &integrity
        && !is_h1(hash)
    {
        return Err(format!(
            "{package}'s \"integrity\" is not an h1: hash (\"h1:\" and 44 base64 characters): {hash:?}"
        ));
    }
    // A reason is for the people reviewing the file, and need not be there;
    // it must be text when it is.
    if let Some(reason) = fields.get("reason")
        && !reason.is_string()
    {
        return Err(format!("{package}'s \"reason\" must be a string"));
    }
    Ok(Pin {
        package,
        version: text("version")?,
        integrity,
    })
}

/// Whether `hash` has the shape of Go's `h1:` hash: a SHA-256 in standard
/// base64, 43 characters and one `=`.
fn is_h1(hash: &str) -> bool {
    hash.strip_prefix("h1:").is_some_and(|b64| {
        b64.len() == 44
            && b64.ends_with('=')
            && b64[..43]
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/')
    })
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
pub fn package_of(real: &Path) -> Option<String> {
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
            .check(Path::new("/proj/node_modules/aws-sdk/index.js"), "aws-sdk")
            .unwrap_err();
        assert!(
            denied.to_string().contains("denied by the import policy"),
            "{denied}"
        );
        let unlisted = p
            .check(Path::new("/proj/node_modules/other/index.js"), "other")
            .unwrap_err();
        assert!(
            unlisted
                .to_string()
                .contains("not in the import policy's allow list"),
            "{unlisted}"
        );
        assert_eq!(unlisted.code(), Some(ErrorCode::PermissionDenied));
    }

    /// A refused file that is not a package is named by the specifier the
    /// program wrote, not by where it resolved (D141).
    #[test]
    fn a_refused_file_is_named_as_written() {
        let root = temp_dir("spelling");
        let p = policy(r#"{ "allow": ["./src"] }"#, &root);
        let refused = p
            .check(&root.join("lib/secret.js"), "../lib/secret.js")
            .unwrap_err()
            .to_string();
        assert!(refused.contains("\"../lib/secret.js\""), "{refused}");
        assert!(!refused.contains(root.to_str().unwrap()), "{refused}");
    }

    #[test]
    fn a_scope_entry_names_every_package_in_the_scope_and_nothing_else() {
        let root = temp_dir("scope");
        let p = policy(r#"{ "allow": ["@acme/*"] }"#, &root);
        assert!(p.permits(Path::new("/proj/node_modules/@acme/ui/index.js")));
        assert!(p.permits(Path::new("/proj/node_modules/@acme/payments/lib/a.js")));
        assert!(!p.permits(Path::new("/proj/node_modules/@acmeco/ui/index.js")));
        assert!(!p.permits(Path::new("/proj/node_modules/acme/index.js")));
        let p = policy(r#"{ "deny": ["@evil/*"] }"#, &root);
        assert_eq!(
            p.judge(Path::new("/proj/node_modules/@evil/x/i.js")),
            Verdict::Denied
        );
        assert_eq!(
            p.judge(Path::new("/proj/node_modules/ok/i.js")),
            Verdict::Allowed
        );
    }

    #[test]
    fn the_only_pattern_is_a_whole_scope() {
        let root = temp_dir("patterns");
        for bad in [
            r#"{ "allow": ["lod*"] }"#,
            r#"{ "allow": ["@acme/ui-*"] }"#,
            r#"{ "allow": ["*"] }"#,
            r#"{ "allow": ["@/*"] }"#,
        ] {
            let err = ImportPolicy::from_json(bad, &root).unwrap_err();
            assert!(err.contains("the only pattern"), "{bad}: {err}");
        }
    }

    #[test]
    fn unlisted_decides_what_allow_does_not_name_and_deny_always_refuses() {
        let root = temp_dir("unlisted");
        let strict = policy(r#"{ "allow": ["zod"], "deny": ["aws-sdk"] }"#, &root);
        let report = policy(
            r#"{ "allow": ["zod"], "deny": ["aws-sdk"], "unlisted": "warn" }"#,
            &root,
        );
        let explicit = policy(r#"{ "allow": ["zod"], "unlisted": "error" }"#, &root);
        let zod = Path::new("/proj/node_modules/zod/index.js");
        let tailwind = Path::new("/proj/node_modules/left-pad/index.js");
        let denied = Path::new("/proj/node_modules/aws-sdk/index.js");
        assert_eq!(strict.unlisted(), Unlisted::Error);
        assert_eq!(report.unlisted(), Unlisted::Warn);
        assert_eq!(explicit.unlisted(), Unlisted::Error);
        assert_eq!(strict.judge(tailwind), Verdict::Unlisted);
        assert!(strict.permits(zod) && !strict.permits(tailwind) && !strict.permits(denied));
        assert!(report.permits(zod) && report.permits(tailwind) && !report.permits(denied));
        assert!(report.check(tailwind, "left-pad").is_ok());
        assert!(report.check(denied, "aws-sdk").is_err());
    }

    #[test]
    fn unlisted_needs_an_allow_list_and_a_known_mode() {
        let root = temp_dir("unlisted-errors");
        let err =
            ImportPolicy::from_json(r#"{ "deny": ["x"], "unlisted": "warn" }"#, &root).unwrap_err();
        assert!(err.contains("there is no \"allow\""), "{err}");
        let err = ImportPolicy::from_json(r#"{ "allow": ["x"], "unlisted": "ignore" }"#, &root)
            .unwrap_err();
        assert!(err.contains("\"error\" or \"warn\""), "{err}");
        let err =
            ImportPolicy::from_json(r#"{ "allow": ["x"], "unlisted": true }"#, &root).unwrap_err();
        assert!(err.contains("\"error\" or \"warn\""), "{err}");
    }

    const HASH: &str = "h1:MqlKs+Ul5e3twLZxAsDHxdUENULXjG0nDnnk4B/VuZA=";

    #[test]
    fn an_object_entry_approves_its_package_and_carries_its_pin() {
        let root = temp_dir("pins");
        let json = format!(
            r#"{{ "allow": ["./src", {{ "package": "zod", "version": "3.23.8", "integrity": "{HASH}", "reason": "validation" }},
                {{ "package": "zod", "version": "3.22.4" }}, {{ "package": "@acme/ui" }}] }}"#
        );
        let p = policy(&json, &root);
        assert_eq!(
            p.judge(Path::new("/proj/node_modules/zod/index.js")),
            Verdict::Allowed
        );
        assert_eq!(
            p.judge(Path::new("/proj/node_modules/@acme/ui/index.js")),
            Verdict::Allowed
        );
        let pins = p.pins("zod");
        assert_eq!(pins.len(), 2, "one entry per approved copy");
        assert_eq!(pins[0].version.as_deref(), Some("3.23.8"));
        assert_eq!(pins[0].integrity.as_deref(), Some(HASH));
        assert_eq!(pins[1].integrity, None);
        assert!(p.pins("left-pad").is_empty());
    }

    #[test]
    fn a_malformed_object_entry_is_an_error() {
        let root = temp_dir("pin-errors");
        for (json, says) in [
            (
                r#"{ "allow": [{ "version": "1.0.0" }] }"#,
                "needs a \"package\"",
            ),
            (
                r#"{ "allow": [{ "package": "zod", "versoin": "1" }] }"#,
                "unknown key \"versoin\"",
            ),
            (
                r#"{ "allow": [{ "package": "zod", "integrity": "sha256-abc" }] }"#,
                "not an h1: hash",
            ),
            (
                r#"{ "allow": [{ "package": "zod", "version": 3 }] }"#,
                "must be a non-empty string",
            ),
            (
                r#"{ "allow": [{ "package": "zod", "reason": 1 }] }"#,
                "\"reason\" must be a string",
            ),
            (
                r#"{ "allow": [{ "package": "@acme/*" }] }"#,
                "one package name",
            ),
            (
                r#"{ "allow": [{ "package": "./src" }] }"#,
                "one package name",
            ),
            (r#"{ "allow": [true] }"#, "only strings and"),
            (
                r#"{ "deny": [{ "package": "zod" }] }"#,
                "must contain only strings",
            ),
        ] {
            let err = ImportPolicy::from_json(json, &root).unwrap_err();
            assert!(err.contains(says), "{json}: {err}");
        }
    }
}
