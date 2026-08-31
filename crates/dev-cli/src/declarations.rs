//! `.d.ts` files, derived from the source's own annotations.
//!
//! A library is a **typed contract**. The `.js` says what it does and the
//! `.d.ts` says what it promises, and a build that emitted only the first would
//! leave every author of an `esrun` library reaching for a second tool to
//! produce the second — which is exactly what the two drivers in this repository
//! did before this existed.
//!
//! **Declarations are derived, never inferred.** oxc's isolated-declarations
//! transform reads what the source *says* — the annotations already written on
//! the exported signatures — and prints them; it does not run a type checker and
//! does not work out what a return type would be. That is the same contract
//! `esdev` has everywhere else (`transform.rs`: types are erased, never
//! checked), and it is what makes emitting a `.d.ts` cost microseconds per file
//! rather than a typechecker's pass over the program.
//!
//! The price is TypeScript's own `isolatedDeclarations` rule: an exported
//! signature has to state its type rather than leave it to be worked out. When
//! one does not, this **fails the build with the list** rather than emitting a
//! declaration it had to guess at — a wrong `.d.ts` is worse than no `.d.ts`,
//! because it is believed. `--no-types` is the way out for a library that would
//! rather annotate later, or emit declarations with `tsc`.

use std::path::{Path, PathBuf};

use oxc::allocator::Allocator;
use oxc::codegen::Codegen;
use oxc::diagnostics::OxcDiagnostic;
use oxc::isolated_declarations::{IsolatedDeclarations, IsolatedDeclarationsOptions};
use oxc::parser::Parser;
use oxc::span::SourceType;

/// The extensions a declaration can be derived from, in the order a `.js`
/// output is matched back to its source.
///
/// TypeScript only: a `.js` or `.mjs` module has no annotations to derive
/// anything from, so it is not an error to find one — there is simply nothing
/// to emit. A JavaScript library that hand-writes its declarations keeps them
/// in its source tree, not in the output, which the build empties.
const TYPED_EXTENSIONS: &[&str] = &["ts", "mts", "cts", "tsx"];

/// Every emitted module under `dir`, as paths relative to it — one entry per
/// module, whatever module systems it was written in.
///
/// The emitted tree *is* the module list: `--lib` preserves module structure, so
/// one output file is one module. Reading it back from disk rather than from the
/// bundler's result is deliberate — it means a declaration is emitted for
/// exactly the modules that were emitted, with no second opinion about which
/// those were.
///
/// A build emitting both module systems writes `pool.js` **and** `pool.cjs`, and
/// those are one module with two spellings rather than two modules: the `.js` is
/// the one listed, and the `.cjs` is left out so it cannot be counted twice or
/// have a declaration derived for it twice. A CommonJS-only build has no `.js`
/// to list, so its `.cjs` files are the modules.
pub fn emitted_modules(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    collect(dir, dir, &mut found);
    found.sort_by(|a, b| {
        a.with_extension("")
            .cmp(&b.with_extension(""))
            // `.js` before `.cjs`, so the dedup below keeps the ES module.
            .then_with(|| b.cmp(a))
    });
    found.dedup_by_key(|module| module.with_extension(""));
    found
}

fn collect(root: &Path, dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(root, &path, found);
        } else if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("js" | "cjs")
        ) && let Ok(relative) = path.strip_prefix(root)
        {
            found.push(relative.to_path_buf());
        }
    }
}

/// The source file an emitted module came from, or `None` if it was JavaScript
/// to begin with.
fn source_of(module: &Path, root: &Path) -> Option<PathBuf> {
    TYPED_EXTENSIONS.iter().find_map(|extension| {
        let candidate = root.join(module).with_extension(extension);
        candidate.exists().then_some(candidate)
    })
}

/// The declaration text for every emitted module that came from TypeScript,
/// keyed by the **source** path it was derived from.
///
/// In memory rather than on disk, because both callers want it that way: the
/// per-module writer below turns each entry into a file, and the bundler
/// ([`crate::dts`]) links them without a round trip through a directory whose
/// layout it would then have to resolve specifiers against.
///
/// Every file is attempted before anything is reported: an author fixing
/// annotations wants the whole list, the way `tsc` gives it, not one error per
/// build.
pub fn generate(
    modules: &[PathBuf],
    root: &Path,
    format: crate::build::Format,
) -> Result<std::collections::HashMap<PathBuf, String>, String> {
    let mut generated = std::collections::HashMap::new();
    let mut problems: Vec<String> = Vec::new();
    for module in modules {
        let Some(source_path) = source_of(module, root) else {
            continue;
        };
        let source = std::fs::read_to_string(&source_path)
            .map_err(|e| format!("cannot read {}: {e}", source_path.display()))?;
        match declarations_for(&source_path, &source, format) {
            Ok(text) => {
                // Both spellings, so a lookup by either the path as written or
                // the canonical one finds it.
                if let Ok(canonical) = source_path.canonicalize() {
                    generated.insert(canonical, text.clone());
                }
                generated.insert(source_path, text);
            }
            Err(errors) => problems.extend(errors),
        }
    }
    if problems.is_empty() {
        return Ok(generated);
    }
    Err(format!(
        "{} declaration{} could not be derived:\n\n{}\n\n\
         A .d.ts is derived from what the source says, never from what a checker \
         infers — so an exported signature has to state its type. Annotate the \
         ones above, or pass --no-types and emit declarations another way.",
        problems.len(),
        if problems.len() == 1 { "" } else { "s" },
        problems
            .iter()
            .map(|p| format!("  {p}"))
            .collect::<Vec<_>>()
            .join("\n")
    ))
}

/// Writes a declaration beside every emitted module that came from TypeScript —
/// one per module system the build emitted — and reports how many were written.
///
/// A `.d.ts` for the `.js` and a `.d.cts` for the `.cjs`, because that is how
/// TypeScript finds them: under `node16`/`nodenext` a declaration types the
/// module file whose extension it mirrors, so a CommonJS output beside an
/// ESM-only declaration is an untyped package to a `require()` of it.
pub fn emit(
    modules: &[PathBuf],
    out_dir: &Path,
    root: &Path,
    formats: &[crate::build::Format],
) -> Result<usize, String> {
    let mut written = 0usize;
    for format in formats {
        let generated = generate(modules, root, *format)?;
        for module in modules {
            let Some(source_path) = source_of(module, root) else {
                continue;
            };
            let Some(text) = generated.get(&source_path) else {
                continue;
            };
            let target = out_dir
                .join(module)
                .with_extension(format.declaration_extension());
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
            }
            std::fs::write(&target, text)
                .map_err(|e| format!("cannot write {}: {e}", target.display()))?;
            written += 1;
        }
    }
    Ok(written)
}

/// The declaration text for one TypeScript source file, in one module system,
/// or the reasons it has none.
fn declarations_for(
    path: &Path,
    source: &str,
    format: crate::build::Format,
) -> Result<String, Vec<String>> {
    let source_type = SourceType::from_path(path)
        .map_err(|e| {
            vec![format!(
                "{}: cannot determine the source type: {e}",
                path.display()
            )]
        })?
        // Always a module, for the same reason the stripper says so: this
        // runtime has no script goal and no CommonJS (D22).
        .with_module(true);

    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, source_type).parse();
    if let Some(error) = parsed
        .diagnostics
        .iter()
        .find(|d| d.severity == oxc::diagnostics::Severity::Error)
    {
        return Err(vec![located(path, source, error)]);
    }

    let result = IsolatedDeclarations::new(&allocator, IsolatedDeclarationsOptions::default())
        .build(&parsed.program);
    let errors: Vec<String> = result
        .diagnostics
        .errors()
        .map(|d| located(path, source, d))
        .collect();
    if !errors.is_empty() {
        return Err(errors);
    }
    let mut program = result.program;
    if format == crate::build::Format::Cjs {
        point_at_cjs(&mut program, &allocator);
    }
    Ok(Codegen::new().build(&program).code)
}

/// Points a declaration's own imports at the CommonJS half of the output:
/// `./pool.js` becomes `./pool.cjs`.
///
/// **A `.d.cts` that imports `./pool.js` describes the wrong module.** The
/// specifier is resolved by the consumer's TypeScript, which maps it to
/// `pool.d.ts` — the declaration of the *ES* module — and then reports that a
/// CommonJS file cannot import one (TS1479). The types are right and the package
/// still does not typecheck, which is the worst of the outcomes available.
///
/// Only relative specifiers are touched. A bare one names a package, and which
/// of its builds a `require` resolves to is decided by that package's `exports`
/// map and TypeScript's conditions, not by anything spelled here.
fn point_at_cjs<'a>(program: &mut oxc::ast::ast::Program<'a>, allocator: &'a Allocator) {
    use oxc::ast::ast::Statement;
    for statement in &mut program.body {
        let source = match statement {
            Statement::ImportDeclaration(import) => &mut import.source,
            Statement::ExportFromDeclaration(export) => &mut export.source,
            Statement::ExportAllDeclaration(export) => &mut export.source,
            _ => continue,
        };
        let Some(rewritten) = cjs_specifier(source.value.as_str()) else {
            continue;
        };
        source.value = oxc::str::Str::from_str_in(&rewritten, &allocator);
        // The printer prefers the raw text it was parsed from, which is still
        // the specifier as written.
        source.raw = None;
    }
}

/// `./pool.js` → `./pool.cjs`, and `None` for a specifier that names something
/// other than a sibling module of this library.
///
/// The extension has to be there to be replaced. A source that wrote `./pool`
/// left the resolution to a resolver that guesses, and neither `node16`
/// TypeScript nor an ES module runtime is one — so there is no CommonJS spelling
/// of it to reach for, and rewriting it would invent one.
fn cjs_specifier(specifier: &str) -> Option<String> {
    if !specifier.starts_with("./") && !specifier.starts_with("../") {
        return None;
    }
    for extension in [".js", ".jsx", ".mjs", ".ts", ".tsx", ".mts"] {
        if let Some(stem) = specifier.strip_suffix(extension) {
            return Some(format!("{stem}.cjs"));
        }
    }
    None
}

/// A diagnostic as `path:line:column  message`.
///
/// The location is the whole value of the message here: "Variable must have an
/// explicit type annotation" is only actionable if it says *which*.
///
/// The message keeps its `TSxxxx` code — the same code `tsc` prints, so it is
/// searchable — and loses the trailing "with --isolatedDeclarations", which
/// names a `tsc` flag that has no counterpart on this command line. Pointing a
/// reader at a flag they cannot pass is worse than saying nothing.
fn located(path: &Path, source: &str, diagnostic: &OxcDiagnostic) -> String {
    let offset = diagnostic.labels.first().map_or(0, |label| label.offset());
    let (line, column) = position(source, offset as usize);
    let message = diagnostic.to_string();
    let message = message
        .split_once(" with --isolatedDeclarations")
        .map_or(message.as_str(), |(head, _)| head)
        .trim_end_matches('.');
    format!("{}:{line}:{column}  {message}.", path.display())
}

/// The one-based line and column of a byte offset.
fn position(source: &str, offset: usize) -> (usize, usize) {
    let offset = offset.min(source.len());
    let before = &source[..offset];
    let line = before.matches('\n').count() + 1;
    let column = before
        .rfind('\n')
        .map_or(before, |newline| &before[newline + 1..])
        .chars()
        .count()
        + 1;
    (line, column)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn declare(source: &str) -> Result<String, Vec<String>> {
        declarations_for(Path::new("lib.ts"), source, crate::build::Format::Esm)
    }

    fn declare_cjs(source: &str) -> Result<String, Vec<String>> {
        declarations_for(Path::new("lib.ts"), source, crate::build::Format::Cjs)
    }

    #[test]
    fn an_annotated_export_becomes_a_declaration() {
        let out = declare("export function add(a: number, b: number): number { return a + b; }")
            .expect("declarations");
        assert!(out.contains("declare function add"), "{out}");
        assert!(out.contains("number"), "{out}");
        // The body is a fact about the implementation, not the contract.
        assert!(!out.contains("a + b"), "{out}");
    }

    #[test]
    fn a_type_only_export_survives() {
        let out = declare("export type Id = string;\nexport interface Row { id: Id }\n")
            .expect("declarations");
        assert!(out.contains("type Id"), "{out}");
        assert!(out.contains("interface Row"), "{out}");
    }

    /// A private helper is not part of the contract and must not appear in it —
    /// a `.d.ts` that named it would make it look importable.
    #[test]
    fn an_unexported_declaration_is_left_out() {
        let out = declare(
            "function secret(): number { return 1; }\nexport const value: number = secret();",
        )
        .expect("declarations");
        assert!(out.contains("value"), "{out}");
        assert!(!out.contains("secret"), "{out}");
    }

    /// The rule this whole approach rests on. Guessing here would produce a
    /// `.d.ts` that is believed and wrong; failing says so.
    #[test]
    fn an_underivable_signature_is_an_error_with_its_location() {
        let errors = declare("export const late = (() => 1)();").expect_err("should not derive");
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].starts_with("lib.ts:1:14"), "{}", errors[0]);
        assert!(errors[0].contains("annotation"), "{}", errors[0]);
    }

    #[test]
    fn every_underivable_signature_is_reported_not_only_the_first() {
        let errors = declare("export const a = (() => 1)();\nexport const b = (() => 2)();")
            .expect_err("should not derive");
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert!(errors[1].starts_with("lib.ts:2:"), "{}", errors[1]);
    }

    #[test]
    fn a_position_counts_lines_and_columns_from_one() {
        assert_eq!(position("abc", 0), (1, 1));
        assert_eq!(position("abc", 2), (1, 3));
        assert_eq!(position("ab\ncd", 3), (2, 1));
        assert_eq!(position("ab\ncd", 4), (2, 2));
        // Past the end rather than panicking: a diagnostic's span is not this
        // module's to validate.
        assert_eq!(position("ab", 99), (1, 3));
    }

    /// The `.d.cts` has to point at the `.cjs` beside it. Pointing at the `.js`
    /// resolves to the ES module's declaration, which a CommonJS file may not
    /// import (TS1479) — types that are right and a package that does not
    /// typecheck.
    #[test]
    fn a_commonjs_declaration_points_at_its_commonjs_siblings() {
        let source = "import type { Pool } from './pool.js';\n\
                      export { Row } from './row.js';\n\
                      export * from './rows.js';\n\
                      export function open(): Pool { return null as never; }\n";
        let esm = declare(source).expect("declarations");
        assert!(esm.contains("\"./pool.js\""), "{esm}");
        assert!(esm.contains("\"./row.js\""), "{esm}");
        assert!(esm.contains("\"./rows.js\""), "{esm}");

        let cjs = declare_cjs(source).expect("declarations");
        assert!(cjs.contains("\"./pool.cjs\""), "{cjs}");
        assert!(cjs.contains("\"./row.cjs\""), "{cjs}");
        assert!(cjs.contains("\"./rows.cjs\""), "{cjs}");
        assert!(!cjs.contains(".js\""), "{cjs}");
    }

    /// A bare specifier names a package, and which of its builds a `require`
    /// arrives at is that package's `exports` map to decide.
    #[test]
    fn a_commonjs_declaration_leaves_a_dependency_alone() {
        let source = "import type { Context } from 'hono';\n\
                      export declare function handler(c: Context): Response;\n";
        let cjs = declare_cjs(source).expect("declarations");
        assert!(cjs.contains("\"hono\""), "{cjs}");
    }

    /// Nothing to replace, so nothing is replaced: an extensionless specifier
    /// was already left to a resolver that guesses, and inventing `.cjs` for it
    /// would name a file the source never referred to.
    #[test]
    fn an_extensionless_specifier_is_not_given_one() {
        assert_eq!(cjs_specifier("./pool"), None);
        assert_eq!(cjs_specifier("./pool.js").as_deref(), Some("./pool.cjs"));
        assert_eq!(cjs_specifier("../a/b.mjs").as_deref(), Some("../a/b.cjs"));
        assert_eq!(cjs_specifier("hono"), None);
        assert_eq!(cjs_specifier("runtime:fs"), None);
    }

    #[test]
    fn a_javascript_module_has_no_source_to_derive_from() {
        let dir = std::env::temp_dir().join("esdev_declarations_source_of");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create dir");
        std::fs::write(dir.join("plain.js"), "export const x = 1;").expect("write");
        std::fs::write(dir.join("typed.ts"), "export const x: number = 1;").expect("write");

        assert_eq!(source_of(Path::new("plain.js"), &dir), None);
        assert_eq!(
            source_of(Path::new("typed.js"), &dir),
            Some(dir.join("typed.ts"))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
