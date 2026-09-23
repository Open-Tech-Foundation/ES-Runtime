//! `mock.module` — a module replaced where it is loaded (D101).
//!
//! Two halves, both in the source transform every module passes through:
//!
//! - **Serving a mock.** `runtime:test` runs a mock's factory and registers
//!   the module's resolved URL and the names it exports. When that URL is
//!   loaded, [`synthetic`] is its source: a module whose exports read the
//!   factory's values from where `runtime:test` keeps them.
//! - **Rewriting the files that call it.** [`rewrite`] passes each
//!   `mock.module`/`mock.importActual` call the file it is written in, so the
//!   specifier resolves from there, as an `import` in that file would. And in a
//!   file that calls `mock.module` at top level it **hoists** those calls, as
//!   Vitest does: they run first, and the file's other static imports become
//!   dynamic `import()`s on the same lines — static imports are linked before
//!   any code runs, so a mock registered in the body would be too late for them.
//!
//! The real module stays loadable beside its mock under [`ACTUAL`], a query
//! the loader keeps on the module id (`importOriginal`, `mock.importActual`).

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use oxc::allocator::Allocator;
use oxc::ast::ast::{
    Argument, CallExpression, Expression, ImportDeclarationSpecifier, ModuleExportName, Statement,
};
use oxc::ast_visit::Visit;
use oxc::parser::Parser;
use oxc::span::{GetSpan, SourceType, Span};

/// The query under which a mocked module's real source is loaded.
pub const ACTUAL: &str = "esdev-actual";

/// Where `runtime:test` keeps each mock's exports: `Map<url, object>`.
const STORE: &str = r#"globalThis[Symbol.for("runtime:test.moduleMocks")]"#;

fn registry() -> &'static Mutex<HashMap<String, Vec<String>>> {
    static MOCKS: OnceLock<Mutex<HashMap<String, Vec<String>>>> = OnceLock::new();
    MOCKS.get_or_init(Mutex::default)
}

/// `url` is mocked, exporting `names`. Replaces an earlier mock of it.
pub fn register(url: String, names: Vec<String>) {
    if let Ok(mut mocks) = registry().lock() {
        mocks.insert(url, names);
    }
}

/// The source that stands in for `url`, when it is mocked.
pub fn synthetic(url: &str) -> Option<String> {
    let names = registry().lock().ok()?.get(url)?.clone();
    let quoted = serde_json::Value::String(url.to_string());
    let mut source = format!("const __esdev_mock = {STORE}.get({quoted});\n");
    for (index, name) in names.iter().enumerate() {
        let key = serde_json::Value::String(name.clone());
        // String export names: any key a factory returned is exportable,
        // reserved words and all.
        source.push_str(&format!(
            "const __esdev_export_{index} = __esdev_mock[{key}];\nexport {{ __esdev_export_{index} as {key} }};\n"
        ));
    }
    Some(source)
}

/// The file's source with its `mock.module` calls resolved from it, and
/// hoisted when they are at top level — or `None` when it has nothing to
/// rewrite.
pub fn rewrite(source: &str, path: &Path) -> Option<String> {
    if !source.contains("runtime:test")
        || !(source.contains(".module(") || source.contains(".importActual("))
    {
        return None;
    }
    let source_type = SourceType::from_path(path).ok()?.with_module(true);
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, source_type).parse();
    if parsed
        .diagnostics
        .iter()
        .any(|d| d.severity == oxc::diagnostics::Severity::Error)
    {
        // The transform proper reports the error.
        return None;
    }
    let program = &parsed.program;

    // What `runtime:test`'s `mock` is called in this file.
    let mut mock = None;
    let mut test_import = None;
    for statement in &program.body {
        if let Statement::ImportDeclaration(import) = statement
            && import.source.value == "runtime:test"
        {
            test_import = Some(import.span);
            for specifier in import.specifiers.iter().flatten() {
                if let ImportDeclarationSpecifier::ImportSpecifier(named) = specifier
                    && export_name(&named.imported) == "mock"
                {
                    mock = Some(named.local.name.to_string());
                }
            }
        }
    }
    let mock = mock?;

    // Every call, anywhere: the file it is written in, as a last argument.
    let mut calls = Calls {
        mock: &mock,
        edits: Vec::new(),
    };
    calls.visit_program(program);
    if calls.edits.is_empty() {
        return None;
    }
    let edits = calls.edits;

    // Top-level `mock.module(...)` statements, to run before the imports.
    let hoisted: Vec<Span> = program
        .body
        .iter()
        .filter_map(|statement| match statement {
            Statement::ExpressionStatement(expression) => {
                let call = match &expression.expression {
                    Expression::AwaitExpression(awaited) => &awaited.argument,
                    other => other,
                };
                match call {
                    Expression::CallExpression(call) if is_mock_call(call, &mock, "module") => {
                        Some(expression.span)
                    }
                    _ => None,
                }
            }
            _ => None,
        })
        .collect();

    let mut replacements: Vec<(u32, u32, String)> = Vec::new();
    if !hoisted.is_empty() {
        let mut first = true;
        for statement in &program.body {
            let Statement::ImportDeclaration(import) = statement else {
                continue;
            };
            if Some(import.span) == test_import
                || import.import_kind.is_type()
                || import.with_clause.is_some()
            {
                continue;
            }
            let Some(dynamic) = dynamic_import(import) else {
                continue;
            };
            // The calls run first, from the first import's line; each stays
            // where it was written as a function, which a module declares
            // before any of its code runs.
            let prefix = if first {
                first = false;
                (0..hoisted.len())
                    .map(|index| format!("await __esdev_mock_{index}(); "))
                    .collect::<String>()
            } else {
                String::new()
            };
            // Every line where it was: a multi-line import keeps its breaks.
            let span = &source[import.span.start as usize..import.span.end as usize];
            let breaks = "\n".repeat(span.matches('\n').count());
            replacements.push((
                import.span.start,
                import.span.end,
                format!("{prefix}{dynamic}{breaks}"),
            ));
        }
        if !first {
            for (index, span) in hoisted.iter().enumerate() {
                let text = apply(source, *span, &edits);
                // An async factory is finished before the imports load.
                let wait = if text.starts_with("await ") {
                    ""
                } else {
                    "await "
                };
                replacements.push((
                    span.start,
                    span.end,
                    format!("async function __esdev_mock_{index}() {{ {wait}{text} }}"),
                ));
            }
        }
    }
    // The argument edits outside anything replaced whole.
    for (at, text) in &edits {
        if !replacements
            .iter()
            .any(|(start, end, _)| start <= at && at < end)
        {
            replacements.push((*at, *at, text.clone()));
        }
    }
    replacements.sort_by_key(|(start, end, _)| std::cmp::Reverse((*start, *end)));
    let mut out = source.to_string();
    for (start, end, text) in replacements {
        out.replace_range(start as usize..end as usize, &text);
    }
    Some(out)
}

/// `source[span]` with the edits inside it applied.
fn apply(source: &str, span: Span, edits: &[(u32, String)]) -> String {
    let mut inside: Vec<&(u32, String)> = edits
        .iter()
        .filter(|(at, _)| *at >= span.start && *at <= span.end)
        .collect();
    inside.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
    let mut text = source[span.start as usize..span.end as usize].to_string();
    for (at, insert) in inside {
        text.insert_str((at - span.start) as usize, insert);
    }
    text
}

/// `import … from "x"` as the `await import("x")` that binds the same names,
/// or `None` when it binds only types.
fn dynamic_import(import: &oxc::ast::ast::ImportDeclaration<'_>) -> Option<String> {
    let from = serde_json::Value::String(import.source.value.to_string());
    let Some(specifiers) = &import.specifiers else {
        return Some(format!("await import({from});"));
    };
    let mut names = Vec::new();
    for specifier in specifiers {
        match specifier {
            ImportDeclarationSpecifier::ImportNamespaceSpecifier(namespace) => {
                return Some(format!(
                    "const {} = await import({from});",
                    namespace.local.name
                ));
            }
            ImportDeclarationSpecifier::ImportDefaultSpecifier(default) => {
                names.push(format!("default: {}", default.local.name));
            }
            ImportDeclarationSpecifier::ImportSpecifier(named) => {
                if named.import_kind.is_type() {
                    continue;
                }
                let imported = export_name(&named.imported);
                let local = named.local.name.as_str();
                let key = if imported
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '_' || c == '$')
                {
                    imported.to_string()
                } else {
                    serde_json::Value::String(imported.to_string()).to_string()
                };
                names.push(if key == local {
                    key
                } else {
                    format!("{key}: {local}")
                });
            }
        }
    }
    if names.is_empty() {
        return None;
    }
    Some(format!(
        "const {{ {} }} = await import({from});",
        names.join(", ")
    ))
}

fn export_name<'a>(name: &'a ModuleExportName<'_>) -> &'a str {
    match name {
        ModuleExportName::IdentifierName(name) => name.name.as_str(),
        ModuleExportName::IdentifierReference(name) => name.name.as_str(),
        ModuleExportName::StringLiteral(name) => name.value.as_str(),
    }
}

/// Whether `call` is `<mock>.<method>(…)`.
fn is_mock_call(call: &CallExpression<'_>, mock: &str, method: &str) -> bool {
    matches!(
        &call.callee,
        Expression::StaticMemberExpression(member)
            if member.property.name == method
                && matches!(&member.object, Expression::Identifier(object) if object.name == mock)
    )
}

/// Finds the calls to pass the calling file to.
struct Calls<'m> {
    mock: &'m str,
    /// Text to insert, and where.
    edits: Vec<(u32, String)>,
}

impl<'a> Visit<'a> for Calls<'_> {
    fn visit_call_expression(&mut self, call: &CallExpression<'a>) {
        let arity = if is_mock_call(call, self.mock, "module") {
            Some(2)
        } else if is_mock_call(call, self.mock, "importActual") {
            Some(1)
        } else {
            None
        };
        if let Some(arity) = arity
            && call.arguments.len() == arity
            && !call
                .arguments
                .iter()
                .any(|argument| matches!(argument, Argument::SpreadElement(_)))
        {
            let last = call.arguments.last().expect("arity is at least one");
            self.edits
                .push((last.span().end, ", import.meta.url".to_string()));
        }
        oxc::ast_visit::walk::walk_call_expression(self, call);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rewritten(source: &str) -> String {
        rewrite(source, Path::new("a.test.ts")).expect("rewritten")
    }

    #[test]
    fn a_top_level_mock_runs_before_the_imports_on_the_same_lines() {
        let source = "import { mock, test } from \"runtime:test\";\n\
                      import { send } from \"./mail.ts\";\n\
                      import * as db from \"./db.ts\";\n\
                      mock.module(\"./mail.ts\", () => ({ send: mock.fn() }));\n\
                      test(\"x\", () => send());\n";
        let out = rewritten(source);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "import { mock, test } from \"runtime:test\";");
        assert_eq!(
            lines[1],
            "await __esdev_mock_0(); const { send } = await import(\"./mail.ts\");"
        );
        assert_eq!(lines[2], "const db = await import(\"./db.ts\");");
        assert_eq!(
            lines[3],
            "async function __esdev_mock_0() { await mock.module(\"./mail.ts\", () => ({ send: mock.fn() }), import.meta.url); }"
        );
        assert_eq!(lines[4], "test(\"x\", () => send());");
        assert_eq!(out.lines().count(), source.lines().count());
    }

    #[test]
    fn a_multi_line_mock_or_import_keeps_every_line_where_it_was() {
        let source = "import { mock, test } from \"runtime:test\";\n\
                      import {\n  send,\n  provider,\n} from \"./mail.ts\";\n\
                      await mock.module(\"./mail.ts\", () => ({\n  send: mock.fn(),\n}));\n\
                      test(\"x\", () => send());\n";
        let out = rewritten(source);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(out.lines().count(), source.lines().count(), "{out}");
        assert_eq!(
            lines[1],
            "await __esdev_mock_0(); const { send, provider } = await import(\"./mail.ts\");"
        );
        assert_eq!(
            lines[5],
            "async function __esdev_mock_0() { await mock.module(\"./mail.ts\", () => ({"
        );
        assert_eq!(lines[7], "}), import.meta.url); }");
        assert_eq!(lines[8], "test(\"x\", () => send());");
    }

    #[test]
    fn default_renamed_type_and_side_effect_imports() {
        let out = rewritten(
            "import { mock as m } from \"runtime:test\";\n\
             import def, { a, b as c, type T } from \"x\";\n\
             import type { U } from \"y\";\n\
             import \"./setup.ts\";\n\
             await m.module(\"x\", async (importOriginal) => ({ ...(await importOriginal()), a: 1 }));\n",
        );
        assert!(
            out.contains("const { default: def, a, b: c } = await import(\"x\");"),
            "{out}"
        );
        assert!(out.contains("import type { U } from \"y\";"), "{out}");
        assert!(out.contains("await import(\"./setup.ts\");"), "{out}");
        assert!(
            out.starts_with(
                "import { mock as m } from \"runtime:test\";\nawait __esdev_mock_0(); const"
            ),
            "{out}"
        );
        // Written awaited, it is not awaited twice.
        assert!(
            out.ends_with("async function __esdev_mock_0() { await m.module(\"x\", async (importOriginal) => ({ ...(await importOriginal()), a: 1 }), import.meta.url); }\n"),
            "{out}"
        );
    }

    #[test]
    fn a_mock_inside_a_test_is_resolved_from_its_file_and_not_hoisted() {
        let source = "import { mock, test } from \"runtime:test\";\n\
                      import { send } from \"./mail.ts\";\n\
                      test(\"x\", async () => {\n  mock.module(\"./db.ts\", () => ({}));\n  const real = await mock.importActual(\"./db.ts\");\n});\n";
        let out = rewritten(source);
        assert!(
            out.contains("import { send } from \"./mail.ts\";"),
            "static import kept: {out}"
        );
        assert!(
            out.contains("mock.module(\"./db.ts\", () => ({}), import.meta.url);"),
            "{out}"
        );
        assert!(
            out.contains("mock.importActual(\"./db.ts\", import.meta.url)"),
            "{out}"
        );
    }

    #[test]
    fn files_that_do_not_mock_are_left_alone() {
        assert_eq!(
            rewrite(
                "import { test } from \"runtime:test\";\ntest(\"x\", () => {});\n",
                Path::new("a.test.js")
            ),
            None
        );
        // `module(` on something else, and a `mock` that is not runtime:test's.
        assert_eq!(
            rewrite(
                "import { mock } from \"./mine.js\";\nmock.module(\"x\", f);\n",
                Path::new("a.js")
            ),
            None
        );
    }

    #[test]
    fn a_mock_serves_its_exports_by_name() {
        register(
            "file:///p/mail.ts".to_string(),
            vec!["send".into(), "default".into(), "not-an-identifier".into()],
        );
        let source = synthetic("file:///p/mail.ts").expect("mocked");
        assert!(source.contains(r#".get("file:///p/mail.ts")"#), "{source}");
        assert!(
            source.contains(r#"export { __esdev_export_0 as "send" };"#),
            "{source}"
        );
        assert!(
            source.contains(r#"export { __esdev_export_1 as "default" };"#),
            "{source}"
        );
        assert!(
            source.contains(r#"__esdev_mock["not-an-identifier"]"#),
            "{source}"
        );
        assert_eq!(synthetic("file:///p/other.ts"), None);
    }
}
