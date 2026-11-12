//! The templating primitive `esdev create` applies to a template (D220).
//!
//! A template is a directory of files plus a `template.json`, and everything
//! that makes one template different from another is in those two. Nothing
//! here names a template, an option or a file: what a choice does is data,
//! through four operations this module implements once —
//!
//! 1. **an overlay** — `_option/<name>/<value>/`, whose files are added, or
//!    replace the file at the same path;
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
    for option in &manifest.options {
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

    // 1. Overlays, first option to last: a later one wins a path.
    for (name, value) in chosen {
        let prefix = format!("{OPTION_PREFIX}{name}/{value}/");
        for (path, bytes) in files {
            if let Some(within) = path.strip_prefix(prefix.as_str()) {
                match out.iter_mut().find(|(existing, _)| existing == within) {
                    Some(entry) => entry.1 = bytes.to_vec(),
                    None => out.push((within.to_string(), bytes.to_vec())),
                }
            }
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
    }
}
