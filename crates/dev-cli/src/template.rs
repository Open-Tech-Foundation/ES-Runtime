//! The templating primitive `esdev create` applies to a template (D220).
//!
//! A template is a directory of files plus a `template.json`, and everything
//! that makes one template different from another is in those two. Nothing
//! here names a template, an option or a file: what a choice does is data,
//! through four operations this module implements once —
//!
//! 1. **an overlay** — `_option/<name>/<value>/`, whose files are added, or
//!    replace the file at the same path. Overlays nest:
//!    `_option/language/ts/_option/server/yes/app/todos/page.tsx` is written
//!    only when both are chosen. Base files first, then overlays under one
//!    condition, then two, and so on — the most specific file wins a path;
//! 2. **`omit`** — paths, or directory prefixes ending in `/`, withheld;
//! 3. **`devDependencies`** — packages added at the top of `package.json`'s
//!    `devDependencies` as `{{registry:<name>}}`, unless already there;
//! 4. **`scripts`** — scripts added at the top of `scripts`, unless there.
//!
//! In that order: every chosen overlay, first option to last; then every
//! omit, so an option can withhold what another's overlay brought (no blog
//! means no blog, in TypeScript too); then the manifest edits, last option to
//! first, so what the first option adds is written first.
//!
//! What a variant *is* lives in the template as files — the TypeScript half
//! of a starter is its `.tsx` files, not a conversion run on the JavaScript
//! half — so a reviewer reads the output, and the templates' own CI can
//! typecheck it.

use serde::Deserialize;

/// The manifest's name inside a template directory. Never written.
pub const MANIFEST: &str = "template.json";

/// Where a template keeps one choice's files: `_option/<name>/<value>/`.
pub const OPTION_PREFIX: &str = "_option/";

/// A template's `template.json`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// How a menu shows it: `React`, `Component Library`.
    pub label: String,
    /// One line, for `--list` and the menus.
    pub description: String,
    /// The catalog group it is chosen behind, if any.
    #[serde(default)]
    pub group: Option<String>,
    /// The script the next steps tell you to run.
    #[serde(default = "default_run")]
    pub run: String,
    /// The questions it asks, in order.
    #[serde(default)]
    pub options: Vec<OptionSpec>,
}

fn default_run() -> String {
    "dev".to_string()
}

/// One question a template asks.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OptionSpec {
    /// The flag: `--<name>=<value>`, or `--<name>`/`--no-<name>` for a yes/no.
    pub name: String,
    /// What the menu asks.
    pub question: String,
    #[serde(default)]
    pub kind: Kind,
    /// The answer the menu starts on, and the one a run nobody can ask takes.
    #[serde(default)]
    pub default: Option<String>,
    /// For an option with no default: the menu starts on nothing and must be
    /// answered, and a run nobody can ask takes this (D135).
    #[serde(default)]
    pub unattended: Option<String>,
    /// Asked only when every earlier option named here has one of these
    /// values — `{ "render": ["static", "client"] }`. Otherwise the option is
    /// not asked and takes `otherwise`.
    #[serde(default)]
    pub when: Option<std::collections::BTreeMap<String, Vec<String>>>,
    /// The value an option takes when its `when` does not hold.
    #[serde(default)]
    pub otherwise: Option<String>,
    pub choices: Vec<ChoiceSpec>,
}

impl OptionSpec {
    /// The answer an unattended run takes.
    pub fn fallback(&self) -> &str {
        self.default
            .as_deref()
            .or(self.unattended.as_deref())
            .unwrap_or_default()
    }

    /// The choice called `value`, if there is one.
    pub fn choice(&self, value: &str) -> Option<&ChoiceSpec> {
        self.choices.iter().find(|choice| choice.value == value)
    }

    /// Whether this option is asked, given the earlier answers.
    pub fn applies(&self, earlier: &[(String, String)]) -> bool {
        self.when.as_ref().is_none_or(|when| {
            when.iter().all(|(name, values)| {
                earlier
                    .iter()
                    .any(|(answered, value)| answered == name && values.contains(value))
            })
        })
    }
}

/// The options an overlay path is under, and the path it writes; no
/// conditions for a base file.
pub fn conditions(path: &str) -> (Vec<(&str, &str)>, &str) {
    let mut found = Vec::new();
    let mut rest = path;
    while let Some(after) = rest.strip_prefix(OPTION_PREFIX) {
        let mut parts = after.splitn(3, '/');
        match (parts.next(), parts.next(), parts.next()) {
            (Some(name), Some(value), Some(tail)) if !name.is_empty() && !value.is_empty() => {
                found.push((name, value));
                rest = tail;
            }
            _ => break,
        }
    }
    (found, rest)
}

/// Whether an option is a menu of named choices or a yes/no.
#[derive(Debug, Default, Deserialize, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Choice,
    /// Two choices, `yes` and `no`; asked as a confirmation.
    Confirm,
}

/// One answer to an option, and what choosing it does.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChoiceSpec {
    pub value: String,
    pub label: String,
    pub description: String,
    /// Paths, or directory prefixes ending in `/`, withheld.
    #[serde(default)]
    pub omit: Vec<String>,
    /// Packages added to `devDependencies`, resolved from the registry.
    #[serde(default, rename = "devDependencies")]
    pub dev_dependencies: Vec<String>,
    /// Scripts added to `scripts`.
    #[serde(default)]
    pub scripts: Vec<Script>,
}

/// A `package.json` script a choice adds.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Script {
    pub name: String,
    pub run: String,
}

/// The catalog beside the templates: the groups some are chosen behind.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    #[serde(default)]
    pub groups: Vec<Group>,
}

/// A menu entry that stands for several templates, and the menu behind it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Group {
    /// What the first menu returns for it. No template may be called this.
    pub name: String,
    pub label: String,
    pub description: String,
    /// What the second menu asks.
    pub question: String,
    /// Its templates, in menu order.
    pub templates: Vec<String>,
}

/// Reads a template's manifest from its files.
pub fn manifest(template: &str, files: &[(&str, &[u8])]) -> Result<Manifest, String> {
    let bytes = files
        .iter()
        .find(|(path, _)| *path == MANIFEST)
        .map(|(_, bytes)| *bytes)
        .ok_or_else(|| format!("the {template} template has no {MANIFEST}"))?;
    let manifest: Manifest = serde_json::from_slice(bytes)
        .map_err(|e| format!("the {template} template's {MANIFEST}: {e}"))?;
    for (index, option) in manifest.options.iter().enumerate() {
        if let Some(when) = &option.when {
            for (name, values) in when {
                let Some(earlier) = manifest.options[..index].iter().find(|o| &o.name == name)
                else {
                    return Err(format!(
                        "the {template} template's {MANIFEST}: {}'s when names {name}, which is not an earlier option",
                        option.name
                    ));
                };
                if let Some(value) = values.iter().find(|v| earlier.choice(v).is_none()) {
                    return Err(format!(
                        "the {template} template's {MANIFEST}: {}'s when names {name}={value}, which is not one of its choices",
                        option.name
                    ));
                }
            }
            match option.otherwise.as_deref() {
                Some(value) if option.choice(value).is_some() => {}
                _ => {
                    return Err(format!(
                        "the {template} template's {MANIFEST}: {} has a when, and needs an otherwise that is one of its choices",
                        option.name
                    ));
                }
            }
        }
        let fallback = option.fallback();
        if option.choice(fallback).is_none() {
            return Err(format!(
                "the {template} template's {MANIFEST}: {} has no choice {fallback:?} to fall back to",
                option.name
            ));
        }
        if option.kind == Kind::Confirm
            && (option.choices.len() != 2
                || option.choice("yes").is_none()
                || option.choice("no").is_none())
        {
            return Err(format!(
                "the {template} template's {MANIFEST}: {} is a confirm, and has exactly the choices yes and no",
                option.name
            ));
        }
    }
    // Every overlay names options and choices the manifest has, each once: a
    // misspelt directory would otherwise be a file that is silently never
    // written.
    for (path, _) in files {
        let (under, _) = conditions(path);
        if under.is_empty() && path.starts_with(OPTION_PREFIX) {
            return Err(format!(
                "the {template} template: {path} is not under _option/<name>/<value>/"
            ));
        }
        for (position, (name, value)) in under.iter().enumerate() {
            let known = manifest
                .options
                .iter()
                .find(|option| option.name == *name)
                .is_some_and(|option| option.choice(value).is_some());
            if !known {
                return Err(format!(
                    "the {template} template: {path} is under {name}={value}, which is not an option and choice in {MANIFEST}"
                ));
            }
            if under[..position].iter().any(|(earlier, _)| earlier == name) {
                return Err(format!(
                    "the {template} template: {path} names {name} twice"
                ));
            }
        }
    }
    Ok(manifest)
}

/// The files a template writes for `chosen` — `(option, value)` pairs in the
/// manifest's order — with the manifest and every overlay directory left out.
pub fn assemble(
    files: &[(&str, &[u8])],
    manifest: &Manifest,
    chosen: &[(String, String)],
) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut out: Vec<(String, Vec<u8>)> = files
        .iter()
        .filter(|(path, _)| *path != MANIFEST && !path.starts_with(OPTION_PREFIX))
        .map(|(path, bytes)| ((*path).to_string(), bytes.to_vec()))
        .collect();

    let choices: Vec<&ChoiceSpec> = chosen
        .iter()
        .map(|(name, value)| {
            manifest
                .options
                .iter()
                .find(|option| &option.name == name)
                .and_then(|option| option.choice(value))
                .ok_or_else(|| format!("no choice {value:?} for the {name} option"))
        })
        .collect::<Result<_, _>>()?;

    // 1. Overlays whose every condition is chosen: fewer conditions first,
    // then in option order, so the most specific file wins a path.
    let position = |name: &str| {
        manifest
            .options
            .iter()
            .position(|option| option.name == name)
            .unwrap_or(usize::MAX)
    };
    let mut overlays: Vec<(usize, usize, &str, &[u8])> = files
        .iter()
        .filter_map(|(path, bytes)| {
            let (under, within) = conditions(path);
            if under.is_empty() {
                return None;
            }
            let all_chosen = under
                .iter()
                .all(|(name, value)| chosen.iter().any(|(n, v)| n == name && v == value));
            let last = under
                .iter()
                .map(|(name, _)| position(name))
                .max()
                .unwrap_or(0);
            all_chosen.then_some((under.len(), last, within, *bytes))
        })
        .collect();
    overlays.sort_by_key(|(count, last, _, _)| (*count, *last));
    for (_, _, within, bytes) in overlays {
        match out.iter_mut().find(|(existing, _)| existing == within) {
            Some(entry) => entry.1 = bytes.to_vec(),
            None => out.push((within.to_string(), bytes.to_vec())),
        }
    }

    // 2. Omits, after every overlay.
    for choice in &choices {
        out.retain(|(path, _)| {
            !choice
                .omit
                .iter()
                .any(|omitted| match omitted.strip_suffix('/') {
                    Some(_) => path.starts_with(omitted.as_str()),
                    None => path == omitted,
                })
        });
    }

    // 3 and 4. The manifest's additions, last option to first, each at the top
    // of its section: what the first option adds ends up first.
    for choice in choices.iter().rev() {
        if choice.dev_dependencies.is_empty() && choice.scripts.is_empty() {
            continue;
        }
        let Some((_, bytes)) = out.iter_mut().find(|(path, _)| path == "package.json") else {
            return Err(format!(
                "the {} choice adds to package.json, and the template has none",
                choice.value
            ));
        };
        let mut text = String::from_utf8(std::mem::take(bytes))
            .map_err(|_| "package.json is not UTF-8".to_string())?;
        for package in choice.dev_dependencies.iter().rev() {
            text = add_entry(
                &text,
                "devDependencies",
                package,
                &format!("{{{{registry:{package}}}}}"),
            )?;
        }
        for script in choice.scripts.iter().rev() {
            text = add_entry(&text, "scripts", &script.name, &script.run)?;
        }
        *bytes = text.into_bytes();
    }

    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// `"key": "value",` as the first entry of the `section` object, unless the
/// manifest already names `key`. Text, not a JSON round trip, so the
/// template's own formatting and order survive.
fn add_entry(manifest: &str, section: &str, key: &str, value: &str) -> Result<String, String> {
    if manifest.contains(&format!("\"{key}\"")) {
        return Ok(manifest.to_string());
    }
    let anchor = format!("\"{section}\": {{\n");
    if !manifest.contains(&anchor) {
        return Err(format!("package.json has no \"{section}\" to add {key} to"));
    }
    Ok(manifest.replacen(
        &anchor,
        &format!("{anchor}    \"{key}\": \"{value}\",\n"),
        1,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest_of(json: &str) -> Manifest {
        manifest("t", &[(MANIFEST, json.as_bytes())]).expect("a valid manifest")
    }

    const PACKAGE: &str = "{\n  \"scripts\": {\n    \"dev\": \"esdev start\"\n  },\n  \"devDependencies\": {\n    \"esdev\": \"1\"\n  }\n}\n";

    /// Overlays first to last, then every omit, then the additions with the
    /// first option's on top — the whole order, in one template.
    #[test]
    fn overlays_then_omits_then_additions_first_option_on_top() {
        let manifest = manifest_of(
            r#"{ "label": "T", "description": "d", "options": [
                 { "name": "language", "question": "?", "default": "js", "choices": [
                   { "value": "js", "label": "JS", "description": "" },
                   { "value": "ts", "label": "TS", "description": "", "omit": ["a.js"],
                     "devDependencies": ["typescript"], "scripts": [{ "name": "typecheck", "run": "esdev typecheck" }] } ] },
                 { "name": "blog", "kind": "confirm", "question": "?", "default": "no", "choices": [
                   { "value": "yes", "label": "Yes", "description": "" },
                   { "value": "no", "label": "No", "description": "", "omit": ["blog/"],
                     "devDependencies": ["feed"] } ] } ] }"#,
        );
        let files: Vec<(&str, &[u8])> = vec![
            (MANIFEST, b"{}"),
            ("package.json", PACKAGE.as_bytes()),
            ("a.js", b"js"),
            ("blog/post.md", b"post"),
            ("_option/language/ts/a.ts", b"ts"),
            ("_option/language/ts/blog/post.ts", b"typed post"),
            ("_option/language/ts/README.md", b"typed readme"),
        ];
        let out = assemble(
            &files,
            &manifest,
            &[
                ("language".into(), "ts".into()),
                ("blog".into(), "no".into()),
            ],
        )
        .unwrap();
        let paths: Vec<&str> = out.iter().map(|(p, _)| p.as_str()).collect();
        // The overlay's blog file is withheld with the blog; the manifest and
        // the overlay directory are never written.
        assert_eq!(paths, ["README.md", "a.ts", "package.json"]);
        let package = String::from_utf8(out[2].1.clone()).unwrap();
        let typescript = package.find("\"typescript\"").unwrap();
        let feed = package.find("\"feed\"").unwrap();
        assert!(
            typescript < feed,
            "the first option's addition is first: {package}"
        );
        assert!(package.contains("\"typescript\": \"{{registry:typescript}}\""));
        assert!(
            package
                .contains("\"scripts\": {\n    \"typecheck\": \"esdev typecheck\",\n    \"dev\"")
        );
    }

    /// A nested overlay is written only when every option on its path is
    /// chosen, and wins over a less specific file at the same path.
    #[test]
    fn a_nested_overlay_needs_every_condition_and_is_the_most_specific() {
        let manifest = manifest_of(
            r#"{ "label": "T", "description": "d", "options": [
                 { "name": "server", "kind": "confirm", "question": "?", "default": "no", "choices": [
                   { "value": "yes", "label": "Yes", "description": "" },
                   { "value": "no", "label": "No", "description": "" } ] },
                 { "name": "language", "question": "?", "default": "js", "choices": [
                   { "value": "js", "label": "JS", "description": "" },
                   { "value": "ts", "label": "TS", "description": "" } ] } ] }"#,
        );
        let files: Vec<(&str, &[u8])> = vec![
            ("page.js", b"base"),
            ("_option/server/yes/todos.js", b"todos"),
            ("_option/language/ts/page.js", b"typed"),
            (
                "_option/language/ts/_option/server/yes/todos.ts",
                b"typed todos",
            ),
            (
                "_option/language/ts/_option/server/yes/page.js",
                b"typed with a server",
            ),
        ];
        let run = |server: &str, language: &str| {
            let out = assemble(
                &files,
                &manifest,
                &[
                    ("server".into(), server.into()),
                    ("language".into(), language.into()),
                ],
            )
            .unwrap();
            out.into_iter()
                .map(|(p, b)| format!("{p}={}", String::from_utf8(b).unwrap()))
                .collect::<Vec<_>>()
        };
        assert_eq!(run("no", "ts"), ["page.js=typed"]);
        assert_eq!(run("yes", "js"), ["page.js=base", "todos.js=todos"]);
        assert_eq!(
            run("yes", "ts"),
            [
                "page.js=typed with a server",
                "todos.js=todos",
                "todos.ts=typed todos"
            ]
        );
    }

    /// An option with `when` is asked only when its earlier options say so.
    #[test]
    fn an_option_applies_only_when_its_condition_holds() {
        let manifest = manifest_of(
            r#"{ "label": "T", "description": "d", "options": [
                 { "name": "render", "question": "?", "default": "client", "choices": [
                   { "value": "server", "label": "Server", "description": "" },
                   { "value": "client", "label": "Client", "description": "" } ] },
                 { "name": "server", "kind": "confirm", "question": "?", "default": "no",
                   "when": { "render": ["client"] }, "otherwise": "yes", "choices": [
                   { "value": "yes", "label": "Yes", "description": "" },
                   { "value": "no", "label": "No", "description": "" } ] } ] }"#,
        );
        let server = &manifest.options[1];
        assert!(server.applies(&[("render".into(), "client".into())]));
        assert!(!server.applies(&[("render".into(), "server".into())]));
    }

    /// An addition the manifest already has is left as it is.
    #[test]
    fn an_addition_already_there_is_not_added_twice() {
        let once = add_entry(PACKAGE, "devDependencies", "esdev", "x").unwrap();
        assert_eq!(once, PACKAGE);
    }

    /// An addition with nowhere to go is the template's mistake, said.
    #[test]
    fn an_addition_with_no_section_is_refused() {
        let err = add_entry("{}\n", "devDependencies", "typescript", "x").unwrap_err();
        assert!(err.contains("devDependencies"), "{err}");
    }

    /// A manifest that cannot fall back, or a confirm that is not yes/no, is
    /// refused when it is read.
    #[test]
    fn a_manifest_is_checked_when_read() {
        let bad = |json: &str| manifest("t", &[(MANIFEST, json.as_bytes())]).unwrap_err();
        let no_fallback = bad(r#"{ "label": "T", "description": "d", "options": [
                 { "name": "x", "question": "?", "default": "c", "choices": [
                   { "value": "a", "label": "A", "description": "" } ] } ] }"#);
        assert!(no_fallback.contains("fall back"), "{no_fallback}");
        let confirm = bad(r#"{ "label": "T", "description": "d", "options": [
                 { "name": "x", "kind": "confirm", "question": "?", "default": "a", "choices": [
                   { "value": "a", "label": "A", "description": "" } ] } ] }"#);
        assert!(confirm.contains("yes and no"), "{confirm}");
        let unknown = bad(r#"{ "label": "T", "description": "d", "colour": "red" }"#);
        assert!(unknown.contains("colour"), "{unknown}");
        let no_otherwise = bad(r#"{ "label": "T", "description": "d", "options": [
                 { "name": "a", "question": "?", "default": "x", "choices": [
                   { "value": "x", "label": "X", "description": "" } ] },
                 { "name": "b", "question": "?", "default": "y", "when": { "a": ["x"] }, "choices": [
                   { "value": "y", "label": "Y", "description": "" } ] } ] }"#);
        assert!(no_otherwise.contains("otherwise"), "{no_otherwise}");
        let later = bad(r#"{ "label": "T", "description": "d", "options": [
                 { "name": "b", "question": "?", "default": "y", "when": { "a": ["x"] }, "otherwise": "y", "choices": [
                   { "value": "y", "label": "Y", "description": "" } ] },
                 { "name": "a", "question": "?", "default": "x", "choices": [
                   { "value": "x", "label": "X", "description": "" } ] } ] }"#);
        assert!(later.contains("not an earlier option"), "{later}");
    }

    /// An overlay under an option or choice the manifest does not have is a
    /// file that would never be written: refused when the template is read.
    #[test]
    fn an_overlay_must_name_a_real_option_and_choice() {
        let json = r#"{ "label": "T", "description": "d", "options": [
             { "name": "language", "question": "?", "default": "js", "choices": [
               { "value": "js", "label": "JS", "description": "" } ] } ] }"#;
        for (path, why) in [
            ("_option/langauge/js/a", "not an option"),
            ("_option/language/tss/a", "not an option"),
            ("_option/language/js/_option/language/js/a", "twice"),
            ("_option/stray", "not under"),
        ] {
            let err = manifest("t", &[(MANIFEST, json.as_bytes()), (path, b"x")]).unwrap_err();
            assert!(err.contains(why), "{path}: {err}");
        }
    }
}
