//! Package `exports` / `imports` resolution: condition matching in author
//! order, subpath patterns, array fallbacks, `null` (withdrawn) targets, and
//! target validation (DECISIONS.md D40, completing D22's deferral).
//!
//! This module is **pure** — it maps a parsed `package.json` and a subpath to a
//! *target string*; touching the filesystem (probing, canonicalizing, jailing)
//! is [`node_modules`](crate::node_modules)'s job. That split is what makes the
//! algorithm testable without a temp tree.
//!
//! The conditions asserted here are **standards-only**: `import` and `default`.
//! There is deliberately no runtime-branded key (no `node`, `deno`, `bun`, or an
//! `es-runtime` of our own) — a package that needs to know which runtime it is
//! on is a package this runtime is not trying to run — and no `require`, since
//! CommonJS is permanently out (D24). A manifest whose only ESM answer sits
//! behind an unmatched condition falls through to `default`, exactly as it would
//! in any other spec-shaped loader.

use std::fmt;

use serde::de::{Deserializer, MapAccess, SeqAccess, Visitor};

/// The conditions this runtime asserts, matched against `exports`/`imports`
/// condition keys. Order here is *not* precedence — precedence is the order the
/// package author wrote the keys in (see [`Json`]) — this is only the set of
/// keys that may match at all.
pub(crate) const CONDITIONS: &[&str] = &["import", "default"];

/// A `package.json` parsed with **object key order preserved**.
///
/// `serde_json::Value` cannot be used for this: its map is a `BTreeMap`, so it
/// sorts keys, and the `preserve_order` feature is workspace-wide — it would
/// change how *user* data is serialized by the TOML/MessagePack builders in the
/// `runtime` crate. Condition matching is defined to walk keys in the order they
/// were written (`{"import":…,"default":…}` and `{"default":…,"import":…}` are
/// different manifests), so resolution parses into this small ordered tree of
/// its own. Numbers are kept only so the shape parses; no target is a number.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Json {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl Json {
    /// Parses JSON text, preserving object key order.
    pub(crate) fn parse(text: &str) -> Result<Json, serde_json::Error> {
        serde_json::from_str(text)
    }

    /// The value of an object key (first occurrence — duplicate keys in JSON are
    /// resolved last-wins by most parsers, but a `package.json` with duplicates
    /// is malformed either way).
    pub(crate) fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub(crate) fn as_str(&self) -> Option<&str> {
        match self {
            Json::String(s) => Some(s),
            _ => None,
        }
    }
}

impl<'de> serde::Deserialize<'de> for Json {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Json, D::Error> {
        struct JsonVisitor;

        impl<'de> Visitor<'de> for JsonVisitor {
            type Value = Json;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("any JSON value")
            }

            fn visit_unit<E>(self) -> Result<Json, E> {
                Ok(Json::Null)
            }
            fn visit_none<E>(self) -> Result<Json, E> {
                Ok(Json::Null)
            }
            fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Json, D::Error> {
                d.deserialize_any(self)
            }
            fn visit_bool<E>(self, v: bool) -> Result<Json, E> {
                Ok(Json::Bool(v))
            }
            fn visit_i64<E>(self, v: i64) -> Result<Json, E> {
                Ok(Json::Number(v as f64))
            }
            fn visit_u64<E>(self, v: u64) -> Result<Json, E> {
                Ok(Json::Number(v as f64))
            }
            fn visit_f64<E>(self, v: f64) -> Result<Json, E> {
                Ok(Json::Number(v))
            }
            fn visit_str<E>(self, v: &str) -> Result<Json, E> {
                Ok(Json::String(v.to_string()))
            }
            fn visit_string<E>(self, v: String) -> Result<Json, E> {
                Ok(Json::String(v))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Json, A::Error> {
                let mut items = Vec::with_capacity(seq.size_hint().unwrap_or(0));
                while let Some(item) = seq.next_element()? {
                    items.push(item);
                }
                Ok(Json::Array(items))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Json, A::Error> {
                let mut entries = Vec::with_capacity(map.size_hint().unwrap_or(0));
                while let Some((k, v)) = map.next_entry::<String, Json>()? {
                    entries.push((k, v));
                }
                Ok(Json::Object(entries))
            }
        }

        deserializer.deserialize_any(JsonVisitor)
    }
}

/// What resolving a subpath against `exports`/`imports` yielded.
#[derive(Debug, PartialEq)]
pub(crate) enum Resolved {
    /// A target path relative to the package directory (`"./index.mjs"`).
    Target(String),
    /// A bare specifier to resolve afresh — only an `"imports"` target may name
    /// another package (`"#dep": "lodash-es"`).
    Specifier(String),
    /// The subpath exists only as CommonJS (a `require`-only condition).
    CommonJs,
    /// The subpath is mapped to `null`: the author deliberately withdrew it.
    /// Distinct from [`NotFound`](Resolved::NotFound) so the error can say so.
    Blocked,
    /// No key matched — the subpath is not exported / there is no entry.
    NotFound,
}

/// Resolves `subpath` (`"."` or `"./sub"`) against a package's `exports` value.
///
/// `Err` is a *malformed manifest* (an invalid target, or a subpath map mixing
/// `"."` keys with condition keys), which the caller reports as an error naming
/// the package — never silently as "not found".
pub(crate) fn package_exports_resolve(exports: &Json, subpath: &str) -> Result<Resolved, String> {
    match exports {
        // `"exports": "./x.mjs"` / `["./a.mjs", "./b.mjs"]` are shorthand for ".".
        Json::String(_) | Json::Array(_) => {
            if subpath == "." {
                resolve_target(exports, None, false)
            } else {
                Ok(Resolved::NotFound)
            }
        }
        Json::Object(entries) => {
            let subpath_keys = entries.iter().filter(|(k, _)| k.starts_with('.')).count();
            if subpath_keys > 0 && subpath_keys < entries.len() {
                return Err(
                    "\"exports\" cannot mix subpath keys (\".\", \"./sub\") with \
                     condition keys in one object"
                        .to_string(),
                );
            }
            if subpath_keys == 0 {
                // A bare conditions object applies to ".".
                return if subpath == "." {
                    resolve_target(exports, None, false)
                } else {
                    Ok(Resolved::NotFound)
                };
            }
            resolve_subpath_map(entries, subpath, false)
        }
        // `"exports": null` withdraws the whole package.
        Json::Null => Ok(Resolved::Blocked),
        _ => Err("\"exports\" must be a string, array, or object".to_string()),
    }
}

/// Resolves a `#specifier` against the owning package's `imports` value.
///
/// Unlike `exports`, an `imports` target may be a **bare specifier**, which the
/// caller then resolves through `node_modules` from the owning package.
pub(crate) fn package_imports_resolve(imports: &Json, specifier: &str) -> Result<Resolved, String> {
    // `#` alone, and anything starting `#/`, are not valid private specifiers.
    if specifier == "#" || specifier.starts_with("#/") {
        return Err(format!("invalid private specifier {specifier:?}"));
    }
    let Json::Object(entries) = imports else {
        return Err("\"imports\" must be an object whose keys begin with \"#\"".to_string());
    };
    if entries.iter().any(|(k, _)| !k.starts_with('#')) {
        return Err("every \"imports\" key must begin with \"#\"".to_string());
    }
    resolve_subpath_map(entries, specifier, true)
}

/// Matches `key` against a subpath/imports map: an exact key first, then the
/// longest-prefix `*` pattern.
fn resolve_subpath_map(
    entries: &[(String, Json)],
    key: &str,
    allow_bare: bool,
) -> Result<Resolved, String> {
    if let Some((_, node)) = entries.iter().find(|(k, _)| k == key) {
        return resolve_target(node, None, allow_bare);
    }
    match match_pattern(entries, key) {
        Some((node, matched)) => resolve_target(node, Some(&matched), allow_bare),
        None => Ok(Resolved::NotFound),
    }
}

/// Finds the **pattern** key matching `key` — a key with a single `*` whose
/// prefix/suffix bracket it — preferring the longest prefix, then the longest
/// suffix (the documented pattern precedence). Returns the node and the portion
/// captured by `*`.
fn match_pattern<'a>(entries: &'a [(String, Json)], key: &str) -> Option<(&'a Json, String)> {
    let mut best: Option<(&str, &str, &Json)> = None; // (prefix, suffix, node)
    for (pattern, node) in entries {
        let Some(star) = pattern.find('*') else {
            continue;
        };
        let prefix = &pattern[..star];
        let suffix = &pattern[star + 1..];
        // One `*` per key; a key with a second one is not a pattern.
        if suffix.contains('*') {
            continue;
        }
        if key.len() >= prefix.len() + suffix.len()
            && key.starts_with(prefix)
            && key.ends_with(suffix)
            && best.is_none_or(|(p, s, _)| {
                prefix.len() > p.len() || (prefix.len() == p.len() && suffix.len() > s.len())
            })
        {
            best = Some((prefix, suffix, node));
        }
    }
    best.map(|(prefix, suffix, node)| {
        let matched = key[prefix.len()..key.len() - suffix.len()].to_string();
        (node, matched)
    })
}

/// Resolves one `exports`/`imports` node — a string target, `null`, an array of
/// fallbacks, or a conditions object — substituting `star` for `*` in the target.
///
/// Conditions are walked **in the order the author wrote them**, and the first
/// key this runtime asserts (or `default`) wins; an unmatched key is skipped, and
/// a matched key whose own target does not resolve falls through to the next.
fn resolve_target(node: &Json, star: Option<&str>, allow_bare: bool) -> Result<Resolved, String> {
    match node {
        Json::String(target) => validate_target(target, star, allow_bare),
        // An explicit `null` withdraws the subpath — deliberate, so it stops here
        // rather than falling through to a later condition.
        Json::Null => Ok(Resolved::Blocked),
        Json::Array(items) => {
            // Fallback array: the first item that resolves wins. An invalid or
            // unmatched item is skipped rather than failing the whole array —
            // that is the point of a fallback list.
            let mut commonjs = false;
            for item in items {
                match resolve_target(item, star, allow_bare) {
                    Ok(hit @ (Resolved::Target(_) | Resolved::Specifier(_))) => return Ok(hit),
                    Ok(Resolved::CommonJs) => commonjs = true,
                    Ok(Resolved::Blocked | Resolved::NotFound) | Err(_) => {}
                }
            }
            Ok(if commonjs {
                Resolved::CommonJs
            } else {
                Resolved::NotFound
            })
        }
        Json::Object(conds) => {
            if conds.iter().any(|(k, _)| k.starts_with('.')) {
                return Err("nested \"exports\" conditions cannot contain subpath keys".to_string());
            }
            let mut commonjs = false;
            for (condition, target) in conds {
                if condition == "require" {
                    // Not asserted (ESM-only, D24) — but remembered, so a
                    // require-only package gets "this is CommonJS", not "not found".
                    commonjs = true;
                    continue;
                }
                if !CONDITIONS.contains(&condition.as_str()) {
                    continue;
                }
                match resolve_target(target, star, allow_bare)? {
                    Resolved::NotFound => {} // keep looking
                    Resolved::CommonJs => commonjs = true,
                    resolved => return Ok(resolved), // Target/Specifier/Blocked
                }
            }
            Ok(if commonjs {
                Resolved::CommonJs
            } else {
                Resolved::NotFound
            })
        }
        _ => Err(format!(
            "invalid target {node:?}: expected a string, array, object, or null"
        )),
    }
}

/// Validates a string target and substitutes the pattern capture.
///
/// The segment rules are the security-relevant part: neither the literal target
/// nor the captured portion may introduce a `..`, `.` or `node_modules` segment,
/// so `"./*": "./dist/*.js"` cannot be walked out of the package with
/// `pkg/../../etc/passwd`. (The D25 root jail is a second, independent net; this
/// one keeps the *package boundary* meaningful inside the root.)
pub(crate) fn validate_target(
    target: &str,
    star: Option<&str>,
    allow_bare: bool,
) -> Result<Resolved, String> {
    let bare = !target.starts_with("./");
    if bare && !allow_bare {
        return Err(format!(
            "invalid \"exports\" target {target:?}: a target must start with \"./\""
        ));
    }
    if bare && (target.starts_with('#') || target.starts_with('/') || target.starts_with("..")) {
        return Err(format!(
            "invalid \"imports\" target {target:?}: expected \"./path\" or a package specifier"
        ));
    }
    if target.ends_with('/') {
        return Err(format!(
            "invalid target {target:?}: directory (trailing-slash) mappings are not supported \
             — use a subpath pattern such as \"./*\""
        ));
    }
    let expanded = match star {
        Some(captured) => {
            // A capture is a URL path portion: an encoded separator would smuggle
            // a segment past the segment check below once the OS decoded it.
            let lower = captured.to_ascii_lowercase();
            if captured.contains('\\') || lower.contains("%2f") || lower.contains("%5c") {
                return Err(format!(
                    "invalid subpath {captured:?}: encoded path separators are not allowed"
                ));
            }
            target.replace('*', captured)
        }
        None => target.to_string(),
    };
    for segment in expanded.trim_start_matches("./").split('/') {
        if matches!(segment, "" | "." | ".." | "node_modules") {
            return Err(format!(
                "invalid target {expanded:?}: a target may not contain a {segment:?} path segment"
            ));
        }
    }
    Ok(if bare {
        Resolved::Specifier(expanded)
    } else {
        Resolved::Target(expanded)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parses `exports`-shaped JSON text (order preserved) and resolves a subpath.
    fn exports(json: &str, subpath: &str) -> Result<Resolved, String> {
        package_exports_resolve(&Json::parse(json).expect("valid JSON"), subpath)
    }

    fn imports(json: &str, specifier: &str) -> Result<Resolved, String> {
        package_imports_resolve(&Json::parse(json).expect("valid JSON"), specifier)
    }

    fn target(result: Result<Resolved, String>) -> Option<String> {
        match result {
            Ok(Resolved::Target(t)) => Some(t),
            _ => None,
        }
    }

    // ----- object key order is preserved (the premise of the whole module) ---

    #[test]
    fn parsing_preserves_object_key_order() {
        let Json::Object(entries) = Json::parse(r#"{ "zeta": 1, "alpha": 2 }"#).unwrap() else {
            panic!("expected an object");
        };
        let keys: Vec<&str> = entries.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["zeta", "alpha"], "keys must stay in author order");
    }

    // ----- condition precedence --------------------------------------------

    #[test]
    fn the_first_asserted_condition_in_author_order_wins() {
        assert_eq!(
            target(exports(
                r#"{ "import": "./esm.mjs", "default": "./d.mjs" }"#,
                "."
            )),
            Some("./esm.mjs".into())
        );
        // Reversed in the manifest: `default` is written first, so it wins. This
        // is the case a fixed import-then-default preference gets wrong.
        assert_eq!(
            target(exports(
                r#"{ "default": "./d.mjs", "import": "./esm.mjs" }"#,
                "."
            )),
            Some("./d.mjs".into())
        );
    }

    #[test]
    fn unasserted_conditions_are_skipped() {
        // No runtime-branded key is asserted — including our own name.
        let json = r#"{
            "node": "./node.mjs",
            "browser": "./browser.mjs",
            "deno": "./deno.mjs",
            "es-runtime": "./ours.mjs",
            "default": "./d.mjs"
        }"#;
        assert_eq!(target(exports(json, ".")), Some("./d.mjs".into()));
    }

    #[test]
    fn nested_conditions_resolve_and_fall_through_when_unmatched() {
        // `import` matches, and its nested object is walked in turn.
        let json = r#"{ "import": { "node": "./n.mjs", "default": "./i.mjs" } }"#;
        assert_eq!(target(exports(json, ".")), Some("./i.mjs".into()));

        // `import` matches but resolves to nothing (only unasserted keys inside),
        // so the walk continues to the next top-level condition.
        let json = r#"{ "import": { "browser": "./b.mjs" }, "default": "./d.mjs" }"#;
        assert_eq!(target(exports(json, ".")), Some("./d.mjs".into()));
    }

    #[test]
    fn array_targets_fall_back_to_the_first_valid_entry() {
        // A leading invalid entry is skipped, not fatal — that is what a
        // fallback array is for.
        let json = r#"{ ".": ["not-a-package", "./b.mjs"] }"#;
        assert_eq!(target(exports(json, ".")), Some("./b.mjs".into()));

        // Arrays nest inside conditions, and conditions inside arrays.
        let json = r#"{ "import": [{ "browser": "./b.mjs" }, "./i.mjs"] }"#;
        assert_eq!(target(exports(json, ".")), Some("./i.mjs".into()));

        // Nothing usable in the array.
        let json = r#"{ ".": [{ "require": "./c.cjs" }] }"#;
        assert_eq!(exports(json, "."), Ok(Resolved::CommonJs));
    }

    #[test]
    fn a_null_target_withdraws_the_subpath() {
        let json = r#"{ "./public": "./pub.mjs", "./private/*": null }"#;
        assert_eq!(exports(json, "./private/x"), Ok(Resolved::Blocked));
        assert_eq!(target(exports(json, "./public")), Some("./pub.mjs".into()));

        // A `null` under a matched condition stops the walk — it is a deliberate
        // withdrawal, not an absence.
        let json = r#"{ "import": null, "default": "./d.mjs" }"#;
        assert_eq!(exports(json, "."), Ok(Resolved::Blocked));

        // `"exports": null` withdraws the entire package.
        assert_eq!(exports("null", "."), Ok(Resolved::Blocked));
    }

    #[test]
    fn require_only_is_reported_as_commonjs_at_any_depth() {
        assert_eq!(
            exports(r#"{ ".": { "require": "./c.cjs" } }"#, "."),
            Ok(Resolved::CommonJs)
        );
        assert_eq!(
            exports(r#"{ ".": { "node": { "require": "./c.cjs" } } }"#, "."),
            Ok(Resolved::NotFound),
            "an unasserted outer condition is never entered, so its CJS is invisible"
        );
        assert_eq!(
            exports(r#"{ "import": { "require": "./c.cjs" } }"#, "."),
            Ok(Resolved::CommonJs)
        );
    }

    // ----- subpath keys and patterns ----------------------------------------

    #[test]
    fn exact_subpath_keys_win_over_patterns_and_longest_prefix_wins() {
        let json = r#"{ "./special": "./s.mjs", "./feat/*": "./b/*.js", "./*": "./a/*.js" }"#;
        assert_eq!(target(exports(json, "./special")), Some("./s.mjs".into()));
        assert_eq!(target(exports(json, "./feat/x")), Some("./b/x.js".into()));
        assert_eq!(
            target(exports(json, "./other")),
            Some("./a/other.js".into())
        );
    }

    #[test]
    fn the_longest_suffix_breaks_a_prefix_tie() {
        let json = r#"{ "./*": "./any/*.js", "./*.css": "./styles/*.css" }"#;
        assert_eq!(
            target(exports(json, "./main.css")),
            Some("./styles/main.css".into())
        );
    }

    #[test]
    fn mixing_subpath_and_condition_keys_is_a_malformed_manifest() {
        let err = exports(r#"{ ".": "./a.mjs", "import": "./b.mjs" }"#, ".").unwrap_err();
        assert!(err.contains("cannot mix subpath keys"), "{err}");
    }

    // ----- target validation ------------------------------------------------

    #[test]
    fn a_target_may_not_escape_the_package() {
        for bad in [
            r#"{ ".": "../outside.mjs" }"#,
            r#"{ ".": "/etc/passwd" }"#,
            r#"{ ".": "./../outside.mjs" }"#,
            r#"{ ".": "./node_modules/other/x.mjs" }"#,
            r#"{ ".": "./dir/" }"#,
        ] {
            assert!(exports(bad, ".").is_err(), "must be rejected: {bad}");
        }
    }

    #[test]
    fn a_pattern_capture_cannot_inject_an_escape() {
        let json = r#"{ "./*": "./dist/*.mjs" }"#;
        // `..` in the captured portion would climb out of dist/ once joined.
        assert!(exports(json, "./../../etc/passwd").is_err());
        // Percent-encoded and backslash separators are refused before expansion.
        assert!(
            exports(json, "./%2e%2e/x").is_ok(),
            "only separators are barred"
        );
        assert!(exports(json, "./a%2Fb").is_err());
        assert!(exports(json, "./a\\b").is_err());
    }

    #[test]
    fn a_bare_specifier_target_is_rejected_in_exports() {
        let err = exports(r#"{ ".": "lodash-es" }"#, ".").unwrap_err();
        assert!(err.contains("must start with"), "{err}");
    }

    // ----- imports (#internal) ----------------------------------------------

    #[test]
    fn imports_resolve_paths_patterns_and_conditions() {
        let json = r##"{
            "#internal": "./src/internal.mjs",
            "#feat/*": "./src/feat/*.mjs",
            "#env": { "import": "./src/env.mjs", "require": "./src/env.cjs" }
        }"##;
        assert_eq!(
            target(imports(json, "#internal")),
            Some("./src/internal.mjs".into())
        );
        assert_eq!(
            target(imports(json, "#feat/a")),
            Some("./src/feat/a.mjs".into())
        );
        assert_eq!(target(imports(json, "#env")), Some("./src/env.mjs".into()));
        assert_eq!(imports(json, "#missing"), Ok(Resolved::NotFound));
    }

    #[test]
    fn an_imports_target_may_name_a_package() {
        let json = r##"{ "#dep": "lodash-es", "#dep/*": "@scope/pkg/*.mjs" }"##;
        assert_eq!(
            imports(json, "#dep"),
            Ok(Resolved::Specifier("lodash-es".into()))
        );
        assert_eq!(
            imports(json, "#dep/x"),
            Ok(Resolved::Specifier("@scope/pkg/x.mjs".into()))
        );
    }

    #[test]
    fn malformed_imports_are_rejected() {
        assert!(imports(r#"{ "internal": "./x.mjs" }"#, "#internal").is_err());
        assert!(imports(r#""./x.mjs""#, "#internal").is_err());
        assert!(imports(r##"{ "#a": "#b" }"##, "#a").is_err());
        assert!(imports(r##"{ "#a": "./x.mjs" }"##, "#").is_err());
    }
}
