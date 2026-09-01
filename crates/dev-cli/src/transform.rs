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
}

impl TypeStripper {
    /// The stripper everything but a `--setup` test run uses.
    pub fn new() -> Self {
        Self::default()
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
            prelude: Some((format!("file://{}", entry.display()), modules)),
        }
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

        let result = Transformer::new(&allocator, path, &TransformOptions::default())
            .build_with_scoping(scoping, &mut program);
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
}
