//! TypeScript and JSX, stripped to JavaScript before the engine sees it.
//!
//! `esrun` runs JavaScript and nothing else — that is D22's "ES module packages
//! only", and it is why a deployed artifact is exactly the text that was
//! reviewed. But a developer writes `.ts` and `.tsx`, so something has to do the
//! stripping, and the only safe place for it is the machine they are working on.
//! This is that something.
//!
//! **Types are erased, never checked.** The same choice Node's
//! `--experimental-strip-types` and Bun make: a type error is your editor's job
//! and `tsc --noEmit`'s job, and doing it here would put a typechecker on the
//! critical path of every run for a diagnostic you have already seen. What
//! arrives at the engine is the same program with the annotations removed.
//!
//! **This transform rewrites no specifiers.** Resolution is the loader's
//! contract (D21/D40), and a transform that rewrote an import would be deciding
//! what a program means from inside the step that is only supposed to erase
//! types. Where `esdev` is wider than `esrun` — extensionless imports, a
//! directory's index, `./x.js` meaning `x.ts` — that widening lives in the
//! loader `esdev` installs and nowhere else, so this file's output is the same
//! program with the annotations removed and not a byte more
//! ([`es_runtime_cli_common::run::BundlerStyleLoader`]).

use std::path::Path;

use es_runtime_cli_common::run::SourceTransform;
use oxc::allocator::Allocator;
use oxc::codegen::Codegen;
use oxc::parser::Parser;
use oxc::semantic::SemanticBuilder;
use oxc::span::SourceType;
use oxc::transformer::{TransformOptions, Transformer};

/// Strips TypeScript types and compiles JSX, leaving everything else alone.
///
/// It may also carry a **prelude** for one named module — the setup files
/// `esdev test --setup` was given. See [`TypeStripper::before`].
#[derive(Default)]
pub struct TypeStripper {
    /// The module the prelude belongs to, as a `file:` URL, and the specifiers
    /// to import ahead of it.
    prelude: Option<(String, Vec<String>)>,
    /// How JSX compiles. The project's `jsx` section, which a file's own pragma
    /// comments may still override.
    jsx: JsxSettings,
}

/// How JSX compiles, for a project or for one file.
///
/// The default is the automatic runtime with React's import source, because
/// that is what an unannotated `.jsx` means today. Everything else — Preact,
/// Solid, a classic `h`/`Fragment` pair — is a project saying so, and a
/// runtime that is framework-agnostic has to let it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct JsxSettings {
    /// `React.createElement`-style calls rather than imports from a runtime.
    pub classic: bool,
    /// The package the automatic runtime imports `jsx`/`jsxs` from.
    pub import_source: Option<String>,
    /// The classic runtime's element factory.
    pub factory: Option<String>,
    /// The classic runtime's fragment.
    pub fragment: Option<String>,
    /// `__source` and `__self` on every element, which a dev-only renderer
    /// reads to say where a component came from.
    pub development: bool,
}

impl JsxSettings {
    /// The settings with this file's pragma comments applied.
    ///
    /// `@jsx`, `@jsxFrag`, `@jsxRuntime` and `@jsxImportSource` are how a file
    /// says which JSX it is written in, and a file borrowed from another
    /// project carries them. oxc takes options rather than reading comments, so
    /// they are read here.
    fn with_pragmas(&self, source: &str) -> Self {
        let mut settings = self.clone();
        for (tag, value) in pragmas(source) {
            match tag {
                "jsx" => {
                    settings.factory = Some(value);
                    settings.classic = true;
                }
                "jsxFrag" => {
                    settings.fragment = Some(value);
                    settings.classic = true;
                }
                "jsxImportSource" => {
                    settings.import_source = Some(value);
                    settings.classic = false;
                }
                "jsxRuntime" => settings.classic = value == "classic",
                _ => {}
            }
        }
        settings
    }

    fn options(&self) -> oxc::transformer::JsxOptions {
        let mut jsx = oxc::transformer::JsxOptions {
            development: self.development,
            ..oxc::transformer::JsxOptions::default()
        };
        if self.classic {
            jsx.runtime = oxc::transformer::JsxRuntime::Classic;
            jsx.pragma = self.factory.clone();
            jsx.pragma_frag = self.fragment.clone();
        } else {
            jsx.runtime = oxc::transformer::JsxRuntime::Automatic;
            jsx.import_source = self.import_source.clone();
        }
        jsx
    }
}

/// The `@jsx…` pragmas in a source's comments, in the order they appear.
///
/// Scanned rather than parsed: a pragma is only ever read out of a comment, and
/// the shapes that matter — `/** @jsx h */`, `// @jsxImportSource preact` — are
/// a tag and the word after it. A match inside a string is possible in
/// principle and has never been seen in practice; the cost of being wrong is
/// that a file compiles the way it asked to.
fn pragmas(source: &str) -> Vec<(&'static str, String)> {
    const TAGS: [&str; 4] = ["jsxImportSource", "jsxRuntime", "jsxFrag", "jsx"];
    let mut found = Vec::new();
    // Only the head of the file: a pragma is a file-level declaration, and
    // scanning a megabyte of bundled output for one is wasted work.
    let head = &source[..source.len().min(4096)];
    let mut at = 0;
    while let Some(index) = head[at..].find('@') {
        let start = at + index + 1;
        at = start;
        let Some(tag) = TAGS.iter().find(|tag| head[start..].starts_with(**tag)) else {
            continue;
        };
        let rest = head[start + tag.len()..].trim_start_matches([' ', '\t']);
        let value: String = rest
            .chars()
            .take_while(|character| !character.is_whitespace() && *character != '*')
            .collect();
        if !value.is_empty() {
            found.push((*tag, value));
        }
    }
    found
}

impl TypeStripper {
    /// The stripper everything but a `--setup` test run uses.
    pub fn new() -> Self {
        Self::default()
    }

    /// The same, compiling JSX the way the project said to.
    pub fn with_jsx(jsx: JsxSettings) -> Self {
        Self {
            jsx,
            ..Self::default()
        }
    }

    /// Names how JSX compiles on a stripper that already exists.
    #[must_use]
    pub fn compiling_jsx(mut self, jsx: JsxSettings) -> Self {
        self.jsx = jsx;
        self
    }

    /// A stripper that imports `modules` before `entry` runs.
    ///
    /// **Prepended, and on the entry's own first line.** A setup file exists to
    /// have happened *before* anything else — a global stubbed, a polyfill
    /// installed — and an import appended at the end evaluates after the test
    /// file's own imports, which is after the module under test has already
    /// read whatever the setup was going to change. Prepending puts it first.
    ///
    /// It costs no line numbers, which is the property D71 is built on: the
    /// prelude carries no newline, so the file's line 1 is still line 1 and a
    /// failing assertion still names the line the developer wrote. Only the
    /// columns on that one line move.
    pub fn before(entry: &Path, modules: Vec<String>) -> Self {
        Self {
            // `file://{path.display()}` is not a file URL on Windows: its
            // drive letter becomes the host or scheme rather than the path.
            // The loader names modules with `Url::from_file_path`, so use that
            // exact spelling for the entry the prelude belongs to.
            prelude: Some((
                url::Url::from_file_path(entry)
                    .map(|url| url.to_string())
                    .unwrap_or_else(|()| format!("file://{}", entry.display())),
                modules,
            )),
            jsx: JsxSettings::default(),
        }
    }

    /// Adds one module before the existing prelude. The DOM has to exist
    /// before a user setup module evaluates, because setup commonly imports
    /// helpers that read `document` at module scope.
    pub fn before_with(entry: &Path, first: Option<String>, modules: Vec<String>) -> Self {
        let mut prelude = first.into_iter().collect::<Vec<_>>();
        prelude.extend(modules);
        Self::before(entry, prelude)
    }

    /// The prelude for this module, or empty for every other module.
    fn prelude_for(&self, specifier: &str, path: &str) -> String {
        let Some((entry, modules)) = &self.prelude else {
            return String::new();
        };
        let mine = entry == specifier
            || entry
                .strip_prefix("file://")
                .is_some_and(|file| file == path);
        if !mine {
            return String::new();
        }
        modules
            .iter()
            .map(|module| format!("import {};", serde_json::Value::String(module.clone())))
            .collect::<Vec<_>>()
            .join("")
    }
}

/// Whether a module id names a file this transform has anything to do with.
///
/// Plain `.js`/`.mjs` is returned untouched rather than round-tripped through
/// the parser and printer: reprinting is not free, and every byte it changed
/// would be a byte the stack traces no longer match.
fn needs_transform(path: &str) -> bool {
    matches!(
        Path::new(path).extension().and_then(|e| e.to_str()),
        Some("ts" | "tsx" | "mts" | "cts" | "jsx")
    )
}

/// The first error-severity diagnostic, formatted, or `None` if the batch holds
/// only advisories. The first is the one that matters — the rest are usually
/// cascades from it.
fn first_error(diagnostics: &[oxc::diagnostics::OxcDiagnostic]) -> Option<String> {
    diagnostics
        .iter()
        .find(|d| d.severity == oxc::diagnostics::Severity::Error)
        .map(|d| format!("{d}"))
}

impl SourceTransform for TypeStripper {
    fn transform(&self, specifier: &str, source: String) -> Result<String, String> {
        // The specifier is a file: URL; oxc wants a path, and only to read the
        // extension off it. A URL that will not convert (there should be none —
        // the loader produces file: URLs) is left alone rather than guessed at.
        let path = match specifier.strip_prefix("file://") {
            Some(rest) => rest.split(['?', '#']).next().unwrap_or(rest),
            None => specifier,
        };
        // Applied to the *output* below, never to the input. Prepending it here
        // would put it through the printer, which lays a statement out on a
        // line of its own — and the file would run one line lower than it was
        // written, which is the property D71 is built on. A plain `.js` is not
        // reprinted at all, so it takes the prelude directly.
        let prelude = self.prelude_for(specifier, path);
        if !needs_transform(path) {
            return Ok(if prelude.is_empty() {
                source
            } else {
                format!("{prelude}{source}")
            });
        }

        let path = Path::new(path);
        let source_type = SourceType::from_path(path)
            .map_err(|e| format!("cannot determine the source type: {e}"))?
            // Always a module. This runtime has no script goal and no CommonJS
            // (D22), so a `.ts` here is an ES module regardless of what the
            // extension would mean to Node.
            .with_module(true);

        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, &source, source_type).parse();
        // Only genuine errors stop the run: the parser also reports advisory
        // diagnostics, and refusing to run a file over one of those would be a
        // typechecker's behaviour, which this deliberately is not.
        if let Some(error) = first_error(&parsed.diagnostics) {
            return Err(error);
        }
        let mut program = parsed.program;

        // The transformer needs scoping information to rename and resolve as it
        // erases; `SemanticBuilder` is what produces it.
        //
        // `with_enum_eval` is not optional despite reading like a tuning knob:
        // a TypeScript `enum` compiles to an IIFE whose member values may refer
        // to earlier members, so the transformer needs their evaluated constants
        // and *panics* without them. Anything short of enabling it turns a
        // one-line `enum` in a user's file into a crash.
        let scoping = SemanticBuilder::new()
            .with_enum_eval(true)
            .build(&program)
            .semantic
            .into_scoping();

        let options = TransformOptions {
            jsx: self.jsx.with_pragmas(&source).options(),
            ..TransformOptions::default()
        };
        let result =
            Transformer::new(&allocator, path, &options).build_with_scoping(scoping, &mut program);
        if let Some(error) = first_error(&result.diagnostics) {
            return Err(error);
        }

        // No newline between them: the prelude shares line 1 with whatever the
        // printer put there, so every line below keeps the number it had.
        Ok(format!("{prelude}{}", Codegen::new().build(&program).code))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip(name: &str, source: &str) -> Result<String, String> {
        TypeStripper::new().transform(&format!("file:///{name}"), source.to_string())
    }

    fn strip_with(jsx: JsxSettings, name: &str, source: &str) -> Result<String, String> {
        TypeStripper::with_jsx(jsx).transform(&format!("file:///{name}"), source.to_string())
    }

    #[test]
    fn a_javascript_file_is_returned_untouched() {
        // Byte-identical, not merely equivalent: reprinting a file nobody asked
        // to change would move every stack frame in it.
        let source = "const   x =  1;\n\n// a comment\nexport default x;\n";
        assert_eq!(strip("app.js", source).unwrap(), source);
        assert_eq!(strip("app.mjs", source).unwrap(), source);
    }

    #[test]
    fn type_annotations_are_erased() {
        let out = strip(
            "app.ts",
            "const n: number = 1;\nfunction f(a: string): string { return a; }\nexport { f, n };",
        )
        .unwrap();
        assert!(!out.contains(": number"), "{out}");
        assert!(!out.contains(": string"), "{out}");
        assert!(out.contains("function f(a)"), "{out}");
        assert!(out.contains("export"), "{out}");
    }

    #[test]
    fn type_only_constructs_disappear_entirely() {
        let out = strip(
            "app.ts",
            "interface Point { x: number }\ntype Id = string;\nexport const p = { x: 1 };",
        )
        .unwrap();
        assert!(!out.contains("interface"), "{out}");
        assert!(!out.contains("type Id"), "{out}");
        assert!(out.contains("x: 1"), "{out}");
    }

    #[test]
    fn a_setup_prelude_matches_the_loader_file_url() {
        let entry = std::env::temp_dir().join("esdev_setup_prelude.ts");
        let specifier = url::Url::from_file_path(&entry)
            .expect("an absolute temporary path has a file URL")
            .to_string();
        let out = TypeStripper::before(&entry, vec!["./setup.ts".into()])
            .transform(&specifier, "export const value: number = 1;".into())
            .expect("transform");
        assert!(out.starts_with("import \"./setup.ts\";"), "{out}");
    }

    #[test]
    fn a_type_only_import_is_dropped_but_a_value_import_is_kept() {
        let out = strip(
            "app.ts",
            "import type { A } from './a.ts';\nimport { b } from './b.ts';\nexport const c = b;",
        )
        .unwrap();
        assert!(!out.contains("'./a.ts'"), "{out}");
        assert!(
            out.contains("'./b.ts'") || out.contains("\"./b.ts\""),
            "{out}"
        );
    }

    /// The transform must not touch specifiers: what a module imports has to
    /// resolve identically under `esdev` and `esrun`, or the two disagree about
    /// which file a program is.
    #[test]
    fn specifiers_are_left_exactly_as_written() {
        let out = strip(
            "app.ts",
            "import { x } from './dep.ts';\nimport { y } from 'some-pkg';\nexport const z = [x, y];",
        )
        .unwrap();
        assert!(out.contains("./dep.ts"), "{out}");
        assert!(out.contains("some-pkg"), "{out}");
    }

    #[test]
    fn jsx_compiles_to_calls() {
        let out = strip("app.jsx", "export const el = <div id=\"a\">hi</div>;").unwrap();
        assert!(!out.contains('<'), "{out}");
        assert!(out.contains("jsx"), "{out}");
    }

    #[test]
    fn tsx_gets_both_treatments() {
        let out = strip(
            "app.tsx",
            "const n: number = 1;\nexport const el = <p>{n}</p>;",
        )
        .unwrap();
        assert!(!out.contains(": number"), "{out}");
        assert!(!out.contains("<p>"), "{out}");
    }

    /// TypeScript's constructs that *emit* code, rather than vanishing. These
    /// are where a stripper stops being a matter of deleting annotations, and
    /// `enum` in particular panicked the transformer until the semantic pass was
    /// told to evaluate enum members.
    #[test]
    fn an_enum_becomes_a_real_object() {
        let out = strip(
            "app.ts",
            "export enum Color { Red, Green }\nexport const g = Color.Green;",
        )
        .unwrap();
        assert!(!out.contains("enum Color"), "{out}");
        assert!(out.contains("Color"), "{out}");
    }

    #[test]
    fn an_enum_member_may_refer_to_an_earlier_one() {
        let out = strip(
            "app.ts",
            "enum E { A = 1, B = A + 1, C = B * 2 }\nexport const c = E.C;",
        )
        .unwrap();
        assert!(!out.contains("enum E"), "{out}");
    }

    #[test]
    fn a_parameter_property_becomes_an_assignment() {
        let out = strip(
            "app.ts",
            "export class Box<T> { constructor(private readonly v: T) {}\n get(): T { return this.v; } }",
        )
        .unwrap();
        assert!(!out.contains("private"), "{out}");
        assert!(!out.contains("<T>"), "{out}");
        assert!(out.contains("this.v"), "{out}");
    }

    #[test]
    fn a_namespace_is_emitted_rather_than_dropped() {
        let out = strip(
            "app.ts",
            "export namespace N { export const x = 1; }\nexport const y = N.x;",
        )
        .unwrap();
        assert!(!out.contains("namespace"), "{out}");
        assert!(out.contains("N"), "{out}");
    }

    #[test]
    fn a_syntax_error_is_reported_rather_than_swallowed() {
        let err = strip("app.ts", "const x: = ;").unwrap_err();
        assert!(!err.is_empty());
    }

    #[test]
    fn top_level_await_survives() {
        let out = strip(
            "app.ts",
            "const v: number = await Promise.resolve(1);\nexport { v };",
        )
        .unwrap();
        assert!(out.contains("await"), "{out}");
    }

    #[test]
    fn jsx_compiles_the_way_the_project_said() {
        let classic = JsxSettings {
            classic: true,
            factory: Some("h".to_string()),
            fragment: Some("Fragment".to_string()),
            ..JsxSettings::default()
        };
        let out = strip_with(
            classic,
            "app.jsx",
            "export const el = <div id=\"a\"><>x</></div>;",
        )
        .unwrap();
        assert!(out.contains("h(\"div\""), "{out}");
        assert!(out.contains("h(Fragment"), "{out}");
        assert!(!out.contains("react"), "{out}");

        let automatic = JsxSettings {
            import_source: Some("preact".to_string()),
            ..JsxSettings::default()
        };
        let out = strip_with(automatic, "app.jsx", "export const el = <div/>;").unwrap();
        assert!(out.contains("preact/jsx-runtime"), "{out}");
        assert!(!out.contains("\"react"), "{out}");
    }

    #[test]
    fn a_files_own_pragma_beats_the_project() {
        // The project says automatic-with-preact; the file says classic-with-h.
        let project = JsxSettings {
            import_source: Some("preact".to_string()),
            ..JsxSettings::default()
        };
        let out = strip_with(
            project.clone(),
            "app.jsx",
            "/** @jsx h */\n/** @jsxFrag Frag */\nexport const el = <div><>x</></div>;",
        )
        .unwrap();
        assert!(out.contains("h(\"div\""), "{out}");
        assert!(out.contains("h(Frag"), "{out}");
        assert!(!out.contains("jsx-runtime"), "{out}");

        // And the other way round: a project on the classic runtime, a file that
        // names an import source.
        let classic = JsxSettings {
            classic: true,
            factory: Some("h".to_string()),
            ..JsxSettings::default()
        };
        let out = strip_with(
            classic,
            "app.jsx",
            "// @jsxImportSource solid-js\nexport const el = <div/>;",
        )
        .unwrap();
        assert!(out.contains("solid-js/jsx-runtime"), "{out}");
        assert!(!out.contains("h(\"div\""), "{out}");
    }

    #[test]
    fn a_pragma_is_read_from_the_head_of_the_file() {
        let found = pragmas("/** @jsxImportSource preact */\nconst a = 1;");
        assert_eq!(found, vec![("jsxImportSource", "preact".to_string())]);
        // The longest tag wins, so `@jsxImportSource` is not read as `@jsx`.
        assert_eq!(pragmas("// @jsx h").first().unwrap().0, "jsx");
        assert!(pragmas("const a = 1;").is_empty());
        // A tag with nothing after it says nothing.
        assert!(pragmas("/** @jsx */").is_empty());
    }
}
