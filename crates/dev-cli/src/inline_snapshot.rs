//! Writing an inline snapshot into the test file that took it.
//!
//! `expect(value).toMatchInlineSnapshot()` keeps its snapshot in the call
//! itself, as a template literal argument. Taking one for the first time, or
//! updating one, means rewriting that argument in the source — in the format
//! the rest of the ecosystem writes, so a suite moved here from Jest or Vitest
//! keeps its snapshots readable and its diffs small:
//!
//! ```text
//! expect(1).toMatchInlineSnapshot(`1`);
//! expect(user).toMatchInlineSnapshot(`
//!   {
//!     "name": "ada",
//!   }
//! `);
//! ```
//!
//! A one-line value sits in a one-line literal; a longer one starts on the next
//! line, indented one step past the line the call starts on, and the closing
//! backtick lines up with that line. Property matchers passed ahead of the
//! snapshot are kept as they were written.
//!
//! **The call is found by where it ran.** The test reports the line and column
//! of its caller's frame, already mapped back to the source; this file parses
//! the source and takes the matcher call whose property name is on that line,
//! closest to that column. Nothing is found by text search, so a comment or a
//! string that happens to spell the matcher's name is never rewritten.

use std::path::Path;

use oxc::allocator::Allocator;
use oxc::ast::ast::{Argument, CallExpression, Expression};
use oxc::ast_visit::Visit;
use oxc::parser::Parser;
use oxc::span::{GetSpan, SourceType};

/// The matchers whose last argument is an inline snapshot.
const MATCHERS: &[&str] = &[
    "toMatchInlineSnapshot",
    "toThrowErrorMatchingInlineSnapshot",
];

/// One snapshot to write: the 1-based line and column the call ran at, and
/// the serialized value.
pub struct Write {
    pub line: u32,
    pub column: u32,
    pub value: String,
}

/// `source` with each write applied, or why one could not be.
pub fn rewrite(source: &str, path: &Path, writes: &[Write]) -> Result<String, String> {
    let source_type = SourceType::from_path(path)
        .map_err(|e| {
            format!(
                "cannot determine the source type of {}: {e}",
                path.display()
            )
        })?
        .with_module(true);
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, source_type).parse();
    if parsed
        .diagnostics
        .iter()
        .any(|d| d.severity == oxc::diagnostics::Severity::Error)
    {
        return Err(format!(
            "cannot parse {} to write its inline snapshots",
            path.display()
        ));
    }
    let mut calls = Calls {
        source,
        found: Vec::new(),
    };
    calls.visit_program(&parsed.program);

    let starts = line_starts(source);
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    for write in writes {
        let Some(call) = calls
            .found
            .iter()
            .filter(|call| position(&starts, call.property).0 == write.line)
            .min_by_key(|call| position(&starts, call.property).1.abs_diff(write.column))
        else {
            return Err(format!(
                "cannot find the inline snapshot call at {}:{}:{}",
                path.display(),
                write.line,
                write.column
            ));
        };
        let indent = indentation(source, starts[position(&starts, call.start).0 as usize - 1]);
        let literal = literal(&write.value, indent);
        let arguments = match &call.kept {
            Some(kept) => format!("{kept}, {literal}"),
            None => literal,
        };
        if edits.iter().any(|(start, _, _)| *start == call.open) {
            return Err(format!(
                "{}:{}: an inline snapshot ran more than once with different values — \
                 inline snapshots cannot be taken in a loop; use toMatchSnapshot",
                path.display(),
                write.line
            ));
        }
        edits.push((call.open, call.close, arguments));
    }
    // Last first, so an edit never moves the offsets of the ones still to make.
    edits.sort_by_key(|edit| std::cmp::Reverse(edit.0));
    let mut out = source.to_string();
    for (open, close, arguments) in edits {
        out.replace_range(open..close, &arguments);
    }
    Ok(out)
}

/// A matcher call: where its property name starts, where the statement's
/// expression starts, the span inside its parentheses, and the property
/// matchers argument to keep, if there is one.
struct Found {
    property: usize,
    start: usize,
    open: usize,
    close: usize,
    kept: Option<String>,
}

struct Calls<'s> {
    source: &'s str,
    found: Vec<Found>,
}

impl<'a> Visit<'a> for Calls<'_> {
    fn visit_call_expression(&mut self, call: &CallExpression<'a>) {
        if let Expression::StaticMemberExpression(member) = &call.callee
            && MATCHERS.contains(&member.property.name.as_str())
        {
            let property = member.property.span.start as usize;
            let after_callee = member.span.end as usize;
            // The `(` after the callee, and the `)` that ends the call.
            let open = self.source[after_callee..]
                .find('(')
                .map(|at| after_callee + at + 1);
            let close = (call.span.end as usize).checked_sub(1);
            if let (Some(open), Some(close)) = (open, close) {
                // The first argument is property matchers unless it is the
                // snapshot itself.
                let kept = call.arguments.first().and_then(|first| match first {
                    Argument::StringLiteral(_) | Argument::TemplateLiteral(_) => None,
                    other => Some(
                        self.source[other.span().start as usize..other.span().end as usize]
                            .to_string(),
                    ),
                });
                self.found.push(Found {
                    property,
                    start: statement_start(call),
                    open,
                    close,
                    kept,
                });
            }
        }
        oxc::ast_visit::walk::walk_call_expression(self, call);
    }
}

/// Where the chain a matcher call ends starts — `expect(…)` — which is the
/// line whose indentation a multi-line snapshot is laid out against.
fn statement_start(call: &CallExpression<'_>) -> usize {
    let mut at = &call.callee;
    loop {
        match at {
            Expression::StaticMemberExpression(member) => at = &member.object,
            Expression::CallExpression(inner) => at = &inner.callee,
            Expression::AwaitExpression(awaited) => at = &awaited.argument,
            other => return other.span().start as usize,
        }
    }
}

/// The template literal for `value`, laid out against `indent`.
fn literal(value: &str, indent: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('`', "\\`")
        .replace("${", "\\${");
    if !escaped.contains('\n') {
        return format!("`{escaped}`");
    }
    let mut out = String::from("`\n");
    for line in escaped.split('\n') {
        if !line.is_empty() {
            out.push_str(indent);
            out.push_str("  ");
            out.push_str(line);
        }
        out.push('\n');
    }
    out.push_str(indent);
    out.push('`');
    out
}

fn line_starts(source: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(source.match_indices('\n').map(|(at, _)| at + 1))
        .collect()
}

/// The 1-based line and column of a byte offset.
fn position(starts: &[usize], offset: usize) -> (u32, u32) {
    let line = starts.partition_point(|start| *start <= offset);
    let column = offset - starts[line - 1] + 1;
    (
        u32::try_from(line).unwrap_or(u32::MAX),
        u32::try_from(column).unwrap_or(u32::MAX),
    )
}

/// The whitespace a line begins with.
fn indentation(source: &str, line_start: usize) -> &str {
    let rest = &source[line_start..];
    &rest[..rest.len() - rest.trim_start_matches([' ', '\t']).len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(line: u32, column: u32, value: &str) -> Write {
        Write {
            line,
            column,
            value: value.to_string(),
        }
    }

    #[test]
    fn a_one_line_value_is_a_one_line_literal() {
        let source = "test(\"a\", () => {\n  expect(1).toMatchInlineSnapshot();\n});\n";
        let out = rewrite(source, Path::new("a.test.js"), &[write(2, 13, "1")]).unwrap();
        assert_eq!(
            out,
            "test(\"a\", () => {\n  expect(1).toMatchInlineSnapshot(`1`);\n});\n"
        );
    }

    #[test]
    fn a_longer_value_is_indented_against_the_statement() {
        let source = "test(\"a\", () => {\n  expect(user)\n    .toMatchInlineSnapshot();\n});\n";
        let out = rewrite(
            source,
            Path::new("a.test.ts"),
            &[write(3, 6, "{\n  \"name\": \"ada\",\n}")],
        )
        .unwrap();
        assert_eq!(
            out,
            "test(\"a\", () => {\n  expect(user)\n    .toMatchInlineSnapshot(`\n    {\n      \"name\": \"ada\",\n    }\n  `);\n});\n"
        );
    }

    #[test]
    fn an_existing_snapshot_is_replaced_and_property_matchers_kept() {
        let source = "expect(u).toMatchInlineSnapshot({ id: expect.any(Number) }, `old`);\n";
        let out = rewrite(source, Path::new("a.test.js"), &[write(1, 11, "new")]).unwrap();
        assert_eq!(
            out,
            "expect(u).toMatchInlineSnapshot({ id: expect.any(Number) }, `new`);\n"
        );
        let source = "expect(1).toMatchInlineSnapshot(`\n  2\n`);\n";
        let out = rewrite(source, Path::new("a.test.js"), &[write(1, 11, "1")]).unwrap();
        assert_eq!(out, "expect(1).toMatchInlineSnapshot(`1`);\n");
    }

    #[test]
    fn backticks_dollar_braces_and_backslashes_are_escaped() {
        let source = "expect(s).toMatchInlineSnapshot();\n";
        let out = rewrite(
            source,
            Path::new("a.test.js"),
            &[write(1, 11, "\"a`b ${c} \\\\d\"")],
        )
        .unwrap();
        assert_eq!(
            out,
            "expect(s).toMatchInlineSnapshot(`\"a\\`b \\${c} \\\\\\\\d\"`);\n"
        );
    }

    #[test]
    fn the_call_is_found_by_position_not_by_text() {
        // The name in a comment and in a string is not a call; of the two real
        // calls on one line, the column decides.
        let source = "// toMatchInlineSnapshot()\nconst s = \"toMatchInlineSnapshot()\";\nexpect(1).toMatchInlineSnapshot(); expect(2).toMatchInlineSnapshot();\n";
        let out = rewrite(source, Path::new("a.test.js"), &[write(3, 46, "2")]).unwrap();
        assert!(
            out.ends_with(
                "expect(1).toMatchInlineSnapshot(); expect(2).toMatchInlineSnapshot(`2`);\n"
            ),
            "{out}"
        );
        assert!(
            out.starts_with("// toMatchInlineSnapshot()\nconst s = \"toMatchInlineSnapshot()\";\n"),
            "{out}"
        );
    }

    #[test]
    fn several_writes_in_one_file_all_land() {
        let source = "expect(1).toMatchInlineSnapshot();\nexpect(() => f()).toThrowErrorMatchingInlineSnapshot();\n";
        let out = rewrite(
            source,
            Path::new("a.test.js"),
            &[write(1, 11, "1"), write(2, 19, "[Error: boom]")],
        )
        .unwrap();
        assert_eq!(
            out,
            "expect(1).toMatchInlineSnapshot(`1`);\nexpect(() => f()).toThrowErrorMatchingInlineSnapshot(`[Error: boom]`);\n"
        );
    }

    #[test]
    fn a_call_in_a_loop_with_different_values_is_refused() {
        let source = "for (const n of [1, 2]) expect(n).toMatchInlineSnapshot();\n";
        let err = rewrite(
            source,
            Path::new("a.test.js"),
            &[write(1, 35, "1"), write(1, 35, "2")],
        )
        .unwrap_err();
        assert!(err.contains("cannot be taken in a loop"), "{err}");
    }

    #[test]
    fn a_position_with_no_call_says_so() {
        let err = rewrite(
            "expect(1).toBe(1);\n",
            Path::new("a.test.js"),
            &[write(1, 11, "1")],
        )
        .unwrap_err();
        assert!(
            err.contains("cannot find the inline snapshot call"),
            "{err}"
        );
    }
}
