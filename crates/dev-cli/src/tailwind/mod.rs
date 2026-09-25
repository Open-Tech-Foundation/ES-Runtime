//! Tailwind CSS v4, compiled by the project's own `tailwindcss`.
//!
//! A stylesheet that uses Tailwind — `@import "tailwindcss"`, or any of its
//! directives — is compiled wherever `esdev` reads a stylesheet: one a document
//! links, one a module imports, one named as an entry. Nothing is configured.
//! The project installs `tailwindcss`, as it would for Tailwind's Vite plugin,
//! and the version is the project's choice.
//!
//! # The three halves, and where each runs
//!
//! * **The compiler** is the package's, and it is JavaScript: `compile()` reads
//!   the stylesheet and the theme, `build(candidates)` generates the CSS. It
//!   has no dependencies and imports nothing from Node, so it runs in an
//!   isolate here — started on the first stylesheet that needs it and kept for
//!   the run, because the dev loop compiles on every save (`host.js`, beside
//!   this file).
//! * **The scanner** is not in that package: Tailwind ships it as a native
//!   addon (`@tailwindcss/oxide`), which this runtime does not load. It is
//!   written here, in Rust ([`scan`]).
//! * **The rest** — `@import`, `url()`, minifying — is this toolchain's CSS
//!   pipeline, unchanged. Tailwind runs after local `@import`s are inlined, so
//!   `@apply` and `@theme` see the whole sheet, and before the sheet is
//!   printed, so the output is minified like any other.
//!
//! A stylesheet takes two crossings into the isolate: the compile, which says
//! where to look for class names, and the build, which is handed them.
//!
//! # Not solved here
//!
//! Tailwind v3, whose configuration is a JavaScript file run through PostCSS.
//! And a candidate, once seen, stays generated for as long as the stylesheet's
//! own text is unchanged — Tailwind's `build()` only ever adds — so the dev
//! loop can keep a class a save removed until the stylesheet is next edited.
//! A release build is one process and starts clean.

pub mod scan;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use es_runtime_cli_common::Value;

use crate::css::ast::{Block, BlockItem, ComponentValue, Item, Rule, Stylesheet};
use crate::css::bundle::Bundled;
use crate::plugins::PluginHost;

/// The at-rules only Tailwind reads. A sheet using any of them means nothing
/// to a browser until it is compiled, so their presence is what claims it.
const DIRECTIVES: &[&str] = &[
    "tailwind",
    "theme",
    "apply",
    "utility",
    "variant",
    "custom-variant",
    "plugin",
    "config",
    "reference",
    "source",
];

/// Whether a bundled stylesheet is one Tailwind compiles.
pub fn uses(sheet: &Stylesheet) -> bool {
    sheet.items.iter().any(|item| match item {
        Item::Rule(rule) => rule_uses(rule),
        _ => false,
    })
}

fn rule_uses(rule: &Rule) -> bool {
    match rule {
        Rule::At(at) => {
            let name = at.name();
            DIRECTIVES.contains(&name.as_str())
                || (name == "import" && imports_tailwind(&at.prelude))
                || at.block.as_ref().is_some_and(block_uses)
        }
        Rule::Qualified(qualified) => block_uses(&qualified.block),
    }
}

fn block_uses(block: &Block) -> bool {
    block.items.iter().any(|item| match item {
        BlockItem::Rule(rule) => rule_uses(rule),
        _ => false,
    })
}

/// `@import "tailwindcss"`, or one of its own stylesheets.
fn imports_tailwind(prelude: &[ComponentValue]) -> bool {
    prelude
        .iter()
        .find(|value| !value.is_trivia())
        .and_then(ComponentValue::token)
        .filter(|token| token.kind == crate::css::token::Kind::String)
        .is_some_and(|token| {
            let url = token.unescape();
            url == "tailwindcss" || url.starts_with("tailwindcss/")
        })
}

/// Compiles `bundled` with Tailwind if it uses Tailwind; leaves it alone if
/// not.
///
/// `entry` is the stylesheet the sheet was bundled from: where its relative
/// paths are relative to, and what the project it belongs to is.
pub fn apply(entry: &Path, bundled: &mut Bundled) -> Result<(), String> {
    if !uses(&bundled.sheet) {
        return Ok(());
    }
    let entry = dunce::canonicalize(entry).unwrap_or_else(|_| entry.to_path_buf());
    let dir = entry.parent().unwrap_or(Path::new(".")).to_path_buf();
    let fail = |message: String| {
        format!(
            "cannot compile {} with Tailwind: {message}",
            display(&entry)
        )
    };

    let compiler = locate(&dir).map_err(fail)?;
    let host = host(&compiler).map_err(fail)?;
    let css = crate::css::print::print(&bundled.sheet);
    let id = entry.to_string_lossy().into_owned();

    let compiled = crate::plugins::wait(host.call(
        0,
        "transform",
        vec![Value::String(css.clone()), Value::String(id.clone())],
        Vec::new(),
    ))
    .map_err(fail)?;

    let candidates = if matches!(field(&compiled, "utilities"), Some(Value::Bool(false))) {
        Vec::new()
    } else {
        let auto = root(field(&compiled, "root"), &dir);
        let sources = sources(field(&compiled, "sources"));
        scan::scan(auto.as_deref(), &sources)
            .map_err(fail)?
            .candidates
            .into_iter()
            .map(Value::String)
            .collect()
    };

    let built = crate::plugins::wait(host.call(
        0,
        "transform",
        vec![Value::String(css), Value::String(id)],
        vec![("candidates".to_string(), Value::Array(candidates))],
    ))
    .map_err(fail)?;
    let Some(Value::String(code)) = field(&built, "code") else {
        return Err(fail("the compiler returned no CSS".to_string()));
    };

    bundled.sheet = crate::css::parse::parse(code);
    // Every stylesheet and plugin the compiler loaded: `tailwindcss/index.css`,
    // a `@reference`d file, a `@plugin`. No import reaches them, so without
    // this a save to one rebuilds nothing.
    for path in crate::contract::depends_on(&built) {
        bundled.read_files.push(PathBuf::from(path));
    }
    Ok(())
}

/// The installed `tailwindcss`, as the `file:` URL of its compiler module.
///
/// Found from the stylesheet's directory upward, as an import from it would
/// find it. Read from the manifest rather than assumed, so a layout the
/// package changes in a later release is followed rather than guessed at.
fn locate(dir: &Path) -> Result<String, String> {
    let root = dir
        .ancestors()
        .map(|dir| dir.join("node_modules").join("tailwindcss"))
        .find(|root| root.join("package.json").is_file())
        .ok_or_else(|| {
            "`tailwindcss` is not installed.\n\n\
             The stylesheet uses Tailwind, and esdev compiles it with the \
             project's own copy. Add it:\n\n  npm install -D tailwindcss"
                .to_string()
        })?;
    let manifest_path = root.join("package.json");
    let manifest: serde_json::Value = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("cannot read {}: {e}", manifest_path.display()))
        .and_then(|text| {
            serde_json::from_str(&text)
                .map_err(|e| format!("cannot read {}: {e}", manifest_path.display()))
        })?;

    let version = manifest
        .get("version")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let major: u32 = version
        .split('.')
        .next()
        .and_then(|major| major.parse().ok())
        .unwrap_or(0);
    if major < 4 {
        return Err(format!(
            "the installed `tailwindcss` is {version}, and esdev compiles Tailwind v4.\n\n\
             v3 is configured by a JavaScript file run through PostCSS, which esdev \
             does not run. Upgrade: npm install -D tailwindcss@4"
        ));
    }

    let module = manifest
        .pointer("/exports/.")
        .and_then(|entry| match entry {
            serde_json::Value::String(path) => Some(path.as_str()),
            serde_json::Value::Object(conditions) => conditions
                .get("import")
                .or_else(|| conditions.get("default"))
                .and_then(serde_json::Value::as_str),
            _ => None,
        })
        .ok_or_else(|| {
            format!(
                "{} names no module for `import \"tailwindcss\"`",
                manifest_path.display()
            )
        })?;
    let path = dunce::canonicalize(root.join(module))
        .map_err(|e| format!("cannot read the Tailwind compiler at {module}: {e}"))?;
    url::Url::from_file_path(&path)
        .map(|url| url.to_string())
        .map_err(|()| format!("cannot name {} as a module", path.display()))
}

/// The compiler isolate for one installed `tailwindcss`, started on first use.
///
/// Keyed by the compiler rather than held as one, because a workspace can
/// install two versions for two packages, and each stylesheet is compiled by
/// the copy its own project resolves.
fn host(compiler: &str) -> Result<Arc<PluginHost>, String> {
    static HOSTS: OnceLock<Mutex<HashMap<String, Arc<PluginHost>>>> = OnceLock::new();
    let hosts = HOSTS.get_or_init(|| Mutex::new(HashMap::new()));
    // Held across the start, so two stylesheets arriving at once start one
    // isolate between them rather than one each.
    let mut hosts = hosts.lock().expect("no panic while holding the lock");
    if let Some(host) = hosts.get(compiler) {
        return Ok(host.clone());
    }
    let source = include_str!("host.js").replace(
        "__ESDEV_TAILWIND__",
        &serde_json::Value::String(compiler.to_string()).to_string(),
    );
    let resolution = crate::settings::Resolution {
        alias: Arc::new(crate::alias::Aliases::new(Vec::new(), None)),
        // A `@plugin` from npm is very often CommonJS.
        converter: crate::commonjs::converter(),
    };
    let host = crate::plugins::wait(crate::plugins::launch(
        resolution,
        source,
        "the Tailwind compiler",
    ))
    .map_err(|reason| match reason {
        Some(reason) => format!("the compiler could not be started: {reason}"),
        None => "the compiler could not be started".to_string(),
    })?;
    let host = Arc::new(host);
    hosts.insert(compiler.to_string(), host.clone());
    Ok(host)
}

/// Where automatic detection starts, from the compiler's `root`: the project,
/// unless the import said `source("…")` (a directory) or `source(none)`.
fn root(value: Option<&Value>, dir: &Path) -> Option<PathBuf> {
    match value {
        Some(Value::String(none)) if none == "none" => None,
        Some(root @ Value::Object(_)) => {
            let base = field(root, "base").and_then(Value::as_str).unwrap_or("");
            let pattern = field(root, "pattern").and_then(Value::as_str).unwrap_or("");
            Some(Path::new(base).join(pattern))
        }
        _ => Some(project(dir)),
    }
}

/// The project a stylesheet belongs to: the nearest directory above it with an
/// `esdev.json` or a `package.json`, which is what Tailwind's own integrations
/// scan from — the directory the build runs in.
fn project(dir: &Path) -> PathBuf {
    dir.ancestors()
        .find(|dir| dir.join("esdev.json").is_file() || dir.join("package.json").is_file())
        .unwrap_or(dir)
        .to_path_buf()
}

fn sources(value: Option<&Value>) -> Vec<scan::Source> {
    let Some(Value::Array(items)) = value else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            Some(scan::Source {
                base: PathBuf::from(field(item, "base")?.as_str()?),
                pattern: field(item, "pattern")?.as_str()?.to_string(),
                negated: matches!(field(item, "negated"), Some(Value::Bool(true))),
            })
        })
        .collect()
}

fn field<'a>(value: &'a Value, name: &str) -> Option<&'a Value> {
    match value {
        Value::Object(pairs) => pairs.iter().find(|(key, _)| key == name).map(|(_, v)| v),
        _ => None,
    }
}

fn display(path: &Path) -> String {
    std::env::current_dir()
        .ok()
        .and_then(|cwd| path.strip_prefix(cwd).ok().map(Path::to_path_buf))
        .unwrap_or_else(|| path.to_path_buf())
        .display()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sheet(css: &str) -> Stylesheet {
        crate::css::parse::parse(css)
    }

    #[test]
    fn the_import_claims_a_sheet() {
        assert!(uses(&sheet("@import \"tailwindcss\";")));
        assert!(uses(&sheet("@import 'tailwindcss' source(\"../src\");")));
        assert!(uses(&sheet(
            "@import \"tailwindcss/theme.css\" layer(theme);"
        )));
    }

    #[test]
    fn a_directive_claims_a_sheet_wherever_it_is() {
        assert!(uses(&sheet("@theme { --color-brand: red; }")));
        assert!(uses(&sheet(
            "@reference \"../app.css\";\n.btn { @apply px-4; }"
        )));
        assert!(uses(&sheet(
            "@media (min-width: 1px) { .a { @apply p-4; } }"
        )));
        assert!(uses(&sheet(
            "@custom-variant dark (&:where(.dark, .dark *));"
        )));
    }

    /// Plain CSS — including an `@import` and at-rules the browser reads — is
    /// left to the pipeline it always went through.
    #[test]
    fn plain_css_is_not_claimed() {
        assert!(!uses(&sheet(
            "@import \"./theme.css\";\n@layer base { a { color: red } }\n\
             @media print { .x { display: none } }\n@font-face { font-family: x }"
        )));
        assert!(!uses(&sheet("@import \"tailwind-like.css\";")));
    }

    #[test]
    fn a_package_without_tailwind_is_reported_with_what_to_do() {
        let dir = std::env::temp_dir().join("esdev-tailwind-missing");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let err = locate(&dir).expect_err("nothing is installed");
        assert!(err.contains("npm install -D tailwindcss"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tailwind_v3_is_refused_by_name() {
        let dir = std::env::temp_dir().join("esdev-tailwind-v3");
        let _ = std::fs::remove_dir_all(&dir);
        let package = dir.join("node_modules/tailwindcss");
        std::fs::create_dir_all(&package).expect("mkdir");
        std::fs::write(
            package.join("package.json"),
            r#"{ "name": "tailwindcss", "version": "3.4.17", "main": "lib/index.js" }"#,
        )
        .expect("write");
        let err = locate(&dir).expect_err("v3 is not compiled");
        assert!(err.contains("3.4.17"), "{err}");
        assert!(err.contains("tailwindcss@4"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_compiler_module_is_read_from_the_manifest() {
        let dir = std::env::temp_dir().join("esdev-tailwind-v4");
        let _ = std::fs::remove_dir_all(&dir);
        let package = dir.join("app/node_modules/tailwindcss");
        std::fs::create_dir_all(package.join("dist")).expect("mkdir");
        std::fs::write(package.join("dist/lib.mjs"), "export {}").expect("write");
        std::fs::write(
            package.join("package.json"),
            r#"{ "version": "4.3.3", "exports": { ".": { "style": "./index.css", "import": "./dist/lib.mjs" } } }"#,
        )
        .expect("write");
        // Found from a directory below the one that installed it.
        std::fs::create_dir_all(dir.join("app/src/styles")).expect("mkdir");
        let url = locate(&dir.join("app/src/styles")).expect("found");
        assert!(url.starts_with("file:"), "{url}");
        assert!(url.ends_with("/dist/lib.mjs"), "{url}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_root_follows_what_the_import_said() {
        let dir = Path::new("/p/src");
        assert_eq!(root(Some(&Value::String("none".into())), dir), None);
        let custom = Value::Object(vec![
            ("base".into(), Value::String("/p/src".into())),
            ("pattern".into(), Value::String("/p/app".into())),
        ]);
        assert_eq!(root(Some(&custom), dir), Some(PathBuf::from("/p/app")));
    }
}
