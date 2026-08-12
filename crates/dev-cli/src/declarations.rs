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
/// to emit, and a JavaScript library that hand-writes its `.d.ts` keeps it.
const TYPED_EXTENSIONS: &[&str] = &["ts", "mts", "cts", "tsx"];

/// Every `.js` file under `dir`, as paths relative to it.
///
/// The emitted tree *is* the module list: `--lib` preserves module structure, so
/// one output file is one module. Reading it back from disk rather than from the
/// bundler's result is deliberate — it means a declaration is emitted for
/// exactly the modules that were emitted, with no second opinion about which
/// those were.
pub fn emitted_modules(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    collect(dir, dir, &mut found);
    found.sort();
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
        } else if path.extension().and_then(|e| e.to_str()) == Some("js")
            && let Ok(relative) = path.strip_prefix(root)
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

/// Writes a `.d.ts` beside every emitted module that came from TypeScript, and
/// reports how many were written.
///
/// Every file is attempted before anything is reported: an author fixing
/// annotations wants the whole list, the way `tsc` gives it, not one error per
/// build.
pub fn emit(modules: &[PathBuf], out_dir: &Path, root: &Path) -> Result<usize, String> {
    let mut written = 0usize;
    let mut problems: Vec<String> = Vec::new();
    for module in modules {
        let Some(source_path) = source_of(module, root) else {
            continue;
        };
        let source = std::fs::read_to_string(&source_path)
            .map_err(|e| format!("cannot read {}: {e}", source_path.display()))?;
        match declarations_for(&source_path, &source) {
            Ok(text) => {
                let target = out_dir.join(module).with_extension("d.ts");
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
                }
                std::fs::write(&target, text)
                    .map_err(|e| format!("cannot write {}: {e}", target.display()))?;
                written += 1;
            }
            Err(errors) => problems.extend(errors),
        }
    }
    if problems.is_empty() {
        return Ok(written);
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

/// The `.d.ts` text for one TypeScript source file, or the reasons it has none.
fn declarations_for(path: &Path, source: &str) -> Result<String, Vec<String>> {
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
    Ok(Codegen::new().build(&result.program).code)
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
        declarations_for(Path::new("lib.ts"), source)
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
