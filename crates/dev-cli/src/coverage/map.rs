//! Coverage of the files as written, from V8's counts of the code as run.
//!
//! V8 counts blocks of the code it compiled — for a TypeScript file, what the
//! transform printed. Each statement, function and branch of the file as
//! written is found in that code through the transform's mappings, and takes
//! the count of the innermost block that holds it there. The measures are
//! Istanbul's, so a report reads as it would from Vitest or Jest.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use oxc::allocator::Allocator;
use oxc::ast::ast::{
    ArrowFunctionExpression, ConditionalExpression, Function, FunctionBody, IfStatement,
    LogicalExpression, MethodDefinition, ObjectProperty, PropertyKey, Statement, SwitchStatement,
    VariableDeclarator,
};
use oxc::ast_visit::{Visit, walk};
use oxc::parser::Parser;
use oxc::semantic::ScopeFlags;
use oxc::span::{GetSpan, SourceType, Span};
use serde_json::Value as Json;

use super::collect::Executed;

/// A position as Istanbul writes it: line from 1, column from 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Position {
    pub line: u32,
    pub column: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Location {
    pub start: Position,
    pub end: Position,
}

#[derive(Clone, Debug)]
pub struct Counted {
    pub location: Location,
    pub count: u64,
}

#[derive(Clone, Debug)]
pub struct FunctionCoverage {
    pub name: String,
    /// Where it is named, or where it starts when it has no name.
    pub declaration: Location,
    pub location: Location,
    pub count: u64,
}

#[derive(Clone, Debug)]
pub struct BranchCoverage {
    /// `if`, `cond-expr`, `binary-expr` or `switch`, as Istanbul names them.
    pub kind: &'static str,
    pub location: Location,
    pub paths: Vec<Counted>,
}

/// One file's coverage.
#[derive(Clone, Debug)]
pub struct FileCoverage {
    pub path: PathBuf,
    pub statements: Vec<Counted>,
    pub functions: Vec<FunctionCoverage>,
    pub branches: Vec<BranchCoverage>,
}

impl FileCoverage {
    /// Each line a statement starts on, and the most times one there ran.
    pub fn lines(&self) -> BTreeMap<u32, u64> {
        let mut lines = BTreeMap::new();
        for statement in &self.statements {
            let count = lines.entry(statement.location.start.line).or_insert(0);
            *count = (*count).max(statement.count);
        }
        lines
    }
}

/// Everything the test processes collected, by module URL: what the module ran
/// as, and each set of counts taken of it.
#[derive(Default)]
pub struct Collected {
    modules: HashMap<String, (Executed, Vec<Vec<Block>>)>,
}

/// One block V8 counted: `[start, end)` in UTF-16 units of the code as run.
#[derive(Clone, Copy, Debug)]
struct Block {
    start: u32,
    end: u32,
    count: u64,
}

impl Collected {
    /// Adds what one test process wrote.
    pub fn add(&mut self, written: &Json) {
        let modules: HashMap<String, Executed> = written
            .get("modules")
            .and_then(|modules| serde_json::from_value(modules.clone()).ok())
            .unwrap_or_default();
        for (url, executed) in modules {
            self.modules
                .entry(url)
                .or_insert_with(|| (executed, Vec::new()));
        }
        for take in written
            .get("takes")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
        {
            for script in take.as_array().into_iter().flatten() {
                let Some(url) = script.get("url").and_then(Json::as_str) else {
                    continue;
                };
                let Some((_, takes)) = self.modules.get_mut(url) else {
                    continue;
                };
                let blocks = script
                    .get("functions")
                    .and_then(Json::as_array)
                    .into_iter()
                    .flatten()
                    .flat_map(|function| {
                        function
                            .get("ranges")
                            .and_then(Json::as_array)
                            .into_iter()
                            .flatten()
                    })
                    .filter_map(|range| {
                        Some(Block {
                            start: u32::try_from(range.get("startOffset")?.as_u64()?).ok()?,
                            end: u32::try_from(range.get("endOffset")?.as_u64()?).ok()?,
                            count: range.get("count")?.as_u64()?,
                        })
                    })
                    .collect();
                takes.push(blocks);
            }
        }
    }

    /// The files the tests loaded, as paths.
    pub fn files(&self) -> Vec<PathBuf> {
        let mut files: Vec<PathBuf> = self
            .modules
            .keys()
            .filter_map(|url| url::Url::parse(url).ok()?.to_file_path().ok())
            .collect();
        files.sort();
        files.dedup();
        files
    }

    /// The coverage of the file at `path`: its counts, or none at all when no
    /// test loaded it.
    pub fn measure(&self, path: &Path) -> Option<FileCoverage> {
        let source = std::fs::read_to_string(path).ok()?;
        let url = url::Url::from_file_path(path).ok()?.to_string();
        let run = self.modules.get(&url);
        Some(measure(
            path,
            &source,
            run.map(|(executed, takes)| (executed, takes.as_slice())),
        ))
    }
}

/// Measures `source`, counted by `run` — or all zero when it did not run.
fn measure(path: &Path, source: &str, run: Option<(&Executed, &[Vec<Block>])>) -> FileCoverage {
    let mut file = FileCoverage {
        path: path.to_path_buf(),
        statements: Vec::new(),
        functions: Vec::new(),
        branches: Vec::new(),
    };
    let Ok(source_type) = SourceType::from_path(path) else {
        return file;
    };
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, source_type.with_module(true)).parse();
    let mut found = Found::default();
    found.visit_program(&parsed.program);
    let ignored = ignored_spans(source, &parsed.program.comments, &found);

    let original = Lines::new(source);
    let counter = run.map(|(executed, takes)| Counter::new(executed, takes));
    // The count where `span` starts, reading nothing past its end.
    let count = |span: Span| -> u64 {
        let Some(counter) = &counter else {
            return 0;
        };
        let start = original.position(span.start);
        let end = original.position(span.end);
        counter.at(start, end).unwrap_or(0)
    };
    let location = |span: Span| -> Location {
        let (start_line, start_column) = original.position(span.start);
        let (end_line, end_column) = original.position(span.end);
        Location {
            start: Position {
                line: start_line + 1,
                column: start_column,
            },
            end: Position {
                line: end_line + 1,
                column: end_column,
            },
        }
    };
    let kept = |span: Span| {
        !ignored
            .iter()
            .any(|skip| skip.start <= span.start && span.end <= skip.end)
    };

    for (span, at) in &found.statements {
        if kept(*span) {
            file.statements.push(Counted {
                location: location(*span),
                count: count(*at),
            });
        }
    }
    for function in &found.functions {
        if kept(function.span) {
            file.functions.push(FunctionCoverage {
                name: function.name.clone(),
                declaration: location(function.declaration),
                location: location(function.span),
                count: count(function.span),
            });
        }
    }
    for branch in &found.branches {
        if !kept(branch.span) {
            continue;
        }
        let paths = match &branch.paths {
            Paths::Each(paths) => paths
                .iter()
                .map(|path| Counted {
                    location: location(*path),
                    count: count(*path),
                })
                .collect(),
            // An `if` with no `else`: the times it was reached less the times
            // its consequent ran.
            Paths::ImplicitElse(consequent) => {
                let taken = count(*consequent);
                vec![
                    Counted {
                        location: location(*consequent),
                        count: taken,
                    },
                    Counted {
                        location: location(branch.span),
                        count: count(branch.span).saturating_sub(taken),
                    },
                ]
            }
        };
        file.branches.push(BranchCoverage {
            kind: branch.kind,
            location: location(branch.span),
            paths,
        });
    }
    file
}

/// Line starts of a text, to turn a byte offset into a line and a UTF-16
/// column.
struct Lines<'a> {
    text: &'a str,
    starts: Vec<usize>,
}

impl<'a> Lines<'a> {
    fn new(text: &'a str) -> Self {
        let starts = std::iter::once(0)
            .chain(text.match_indices('\n').map(|(at, _)| at + 1))
            .collect();
        Self { text, starts }
    }

    /// 0-based line, and the UTF-16 column on it.
    fn position(&self, offset: u32) -> (u32, u32) {
        let offset = (offset as usize).min(self.text.len());
        let line = self
            .starts
            .partition_point(|start| *start <= offset)
            .saturating_sub(1);
        let start = self.starts[line];
        let column = self
            .text
            .get(start..offset)
            .map_or(0, |before| before.encode_utf16().count());
        (
            u32::try_from(line).unwrap_or(u32::MAX),
            u32::try_from(column).unwrap_or(u32::MAX),
        )
    }
}

/// Counts at a position of the file as written.
struct Counter<'a> {
    executed: &'a Executed,
    takes: &'a [Vec<Block>],
    /// Where each line of the code as run starts, in UTF-16 units.
    run_lines: Vec<u32>,
    /// By original line: `(original column, run line, run column)`, sorted.
    mapped: HashMap<u32, Vec<(u32, u32, u32)>>,
}

impl<'a> Counter<'a> {
    fn new(executed: &'a Executed, takes: &'a [Vec<Block>]) -> Self {
        let mut run_lines = vec![0u32];
        let mut at = 0u32;
        for unit in executed.text.encode_utf16() {
            at += 1;
            if unit == u16::from(b'\n') {
                run_lines.push(at);
            }
        }
        let mut mapped: HashMap<u32, Vec<(u32, u32, u32)>> = HashMap::new();
        for [run_line, run_column, line, column] in executed.mappings.iter().flatten() {
            mapped
                .entry(*line)
                .or_default()
                .push((*column, *run_line, *run_column));
        }
        for tokens in mapped.values_mut() {
            tokens.sort_unstable();
        }
        Self {
            executed,
            takes,
            run_lines,
            mapped,
        }
    }

    /// The count at `line`/`column` of the file as written: summed over every
    /// take, each the count of the innermost block holding its position in the
    /// code as run. `None` when the position did not map.
    fn at(&self, (line, column): (u32, u32), end: (u32, u32)) -> Option<u64> {
        let (run_line, run_column) = match &self.executed.mappings {
            None => (line, column),
            Some(_) => {
                // A token where the node starts; else the next one on the line
                // while it is still inside the node; else the one before, which
                // begins what holds it.
                let tokens = self.mapped.get(&line)?;
                let index = tokens.partition_point(|token| token.0 < column);
                let token = match tokens.get(index) {
                    Some(next) if next.0 == column || (line, next.0) < end => next,
                    next => index
                        .checked_sub(1)
                        .and_then(|before| tokens.get(before))
                        .or(next)?,
                };
                (token.1, token.2)
            }
        };
        // The prelude sits in front of the file's first line.
        let run_column = if run_line == 0 {
            run_column + self.executed.prelude
        } else {
            run_column
        };
        let offset = self.run_lines.get(run_line as usize)? + run_column;
        Some(
            self.takes
                .iter()
                .filter_map(|blocks| {
                    blocks
                        .iter()
                        .filter(|block| block.start <= offset && offset < block.end)
                        .min_by_key(|block| block.end - block.start)
                        .map(|block| block.count)
                })
                .sum(),
        )
    }
}

/// What a file holds to be covered, found in its syntax.
#[derive(Default)]
struct Found {
    /// Each statement, and the node its count is read from.
    statements: Vec<(Span, Span)>,
    functions: Vec<FoundFunction>,
    branches: Vec<FoundBranch>,
    /// The name the next function takes from what it is assigned to.
    naming: Option<String>,
    anonymous: usize,
}

struct FoundFunction {
    name: String,
    declaration: Span,
    /// Where the function begins, which is where V8's block for it begins,
    /// so its count — read here — is its calls.
    span: Span,
}

struct FoundBranch {
    kind: &'static str,
    span: Span,
    paths: Paths,
}

enum Paths {
    Each(Vec<Span>),
    ImplicitElse(Span),
}

impl Found {
    fn function(&mut self, id: Option<(String, Span)>, span: Span) {
        let (name, declaration) = match id {
            Some((name, at)) => (name, at),
            None => match self.naming.take() {
                Some(name) => (name, span),
                None => {
                    let name = format!("(anonymous_{})", self.anonymous);
                    self.anonymous += 1;
                    (name, span)
                }
            },
        };
        self.naming = None;
        self.functions.push(FoundFunction {
            name,
            declaration,
            span,
        });
    }
}

fn key_name(key: &PropertyKey<'_>) -> Option<String> {
    key.static_name().map(|name| name.to_string())
}

impl<'a> Visit<'a> for Found {
    fn visit_statement(&mut self, statement: &Statement<'a>) {
        let counted = match statement {
            Statement::ExpressionStatement(_)
            | Statement::ReturnStatement(_)
            | Statement::IfStatement(_)
            | Statement::ForStatement(_)
            | Statement::ForInStatement(_)
            | Statement::ForOfStatement(_)
            | Statement::WhileStatement(_)
            | Statement::DoWhileStatement(_)
            | Statement::SwitchStatement(_)
            | Statement::ThrowStatement(_)
            | Statement::TryStatement(_)
            | Statement::BreakStatement(_)
            | Statement::ContinueStatement(_)
            | Statement::DebuggerStatement(_)
            | Statement::WithStatement(_)
            | Statement::TSEnumDeclaration(_) => true,
            Statement::ClassDeclaration(class) => !class.declare,
            Statement::TSNamespaceDeclaration(namespace) => !namespace.declare,
            _ => false,
        };
        if counted {
            let span = statement.span();
            self.statements.push((span, span));
        }
        walk::walk_statement(self, statement);
    }

    // A declaration is counted per declarator that assigns, read where the
    // name is: what it assigns may be a function, whose own block starts there.
    fn visit_variable_declarator(&mut self, declarator: &VariableDeclarator<'a>) {
        if let Some(init) = &declarator.init {
            self.statements.push((declarator.span, declarator.span));
            if matches!(
                init,
                oxc::ast::ast::Expression::ArrowFunctionExpression(_)
                    | oxc::ast::ast::Expression::FunctionExpression(_)
            ) {
                self.naming = declarator
                    .id
                    .get_identifier_name()
                    .map(|name| name.to_string());
            }
        }
        walk::walk_variable_declarator(self, declarator);
    }

    fn visit_method_definition(&mut self, method: &MethodDefinition<'a>) {
        self.naming = key_name(&method.key);
        walk::walk_method_definition(self, method);
    }

    fn visit_object_property(&mut self, property: &ObjectProperty<'a>) {
        if matches!(
            property.value,
            oxc::ast::ast::Expression::ArrowFunctionExpression(_)
                | oxc::ast::ast::Expression::FunctionExpression(_)
        ) {
            self.naming = key_name(&property.key);
        }
        walk::walk_object_property(self, property);
    }

    fn visit_function(&mut self, function: &Function<'a>, flags: ScopeFlags) {
        if function.body.is_some() && !function.declare {
            let id = function
                .id
                .as_ref()
                .map(|id| (id.name.to_string(), id.span));
            self.function(id, function.span);
        } else {
            self.naming = None;
        }
        walk::walk_function(self, function, flags);
    }

    fn visit_arrow_function_expression(&mut self, arrow: &ArrowFunctionExpression<'a>) {
        if !matches!(
            arrow.body,
            oxc::ast::ast::ArrowFunctionBody::FunctionBody(_)
        ) {
            // `x => x * 2`: the expression is the one statement it has.
            let body = arrow.body.span();
            self.statements.push((body, body));
        }
        self.function(None, arrow.span);
        walk::walk_arrow_function_expression(self, arrow);
    }

    fn visit_function_body(&mut self, body: &FunctionBody<'a>) {
        walk::walk_function_body(self, body);
    }

    fn visit_if_statement(&mut self, statement: &IfStatement<'a>) {
        let consequent = statement.consequent.span();
        let paths = match &statement.alternate {
            Some(alternate) => Paths::Each(vec![consequent, alternate.span()]),
            None => Paths::ImplicitElse(consequent),
        };
        self.branches.push(FoundBranch {
            kind: "if",
            span: statement.span,
            paths,
        });
        walk::walk_if_statement(self, statement);
    }

    fn visit_conditional_expression(&mut self, expression: &ConditionalExpression<'a>) {
        self.branches.push(FoundBranch {
            kind: "cond-expr",
            span: expression.span,
            paths: Paths::Each(vec![
                expression.consequent.span(),
                expression.alternate.span(),
            ]),
        });
        walk::walk_conditional_expression(self, expression);
    }

    fn visit_logical_expression(&mut self, expression: &LogicalExpression<'a>) {
        self.branches.push(FoundBranch {
            kind: "binary-expr",
            span: expression.span,
            paths: Paths::Each(vec![expression.left.span(), expression.right.span()]),
        });
        walk::walk_logical_expression(self, expression);
    }

    fn visit_switch_statement(&mut self, statement: &SwitchStatement<'a>) {
        let paths = statement
            .cases
            .iter()
            .map(|case| case.consequent.first().map_or(case.span, GetSpan::span))
            .collect();
        self.branches.push(FoundBranch {
            kind: "switch",
            span: statement.span,
            paths: Paths::Each(paths),
        });
        walk::walk_switch_statement(self, statement);
    }
}

/// What `v8 ignore`, `c8 ignore` and `istanbul ignore` comments leave out:
/// from `start` to `stop`, and the one thing that begins after `next`.
fn ignored_spans(source: &str, comments: &[oxc::ast::Comment], found: &Found) -> Vec<Span> {
    let starts: Vec<Span> = found
        .statements
        .iter()
        .map(|(span, _)| *span)
        .chain(found.functions.iter().map(|function| function.span))
        .chain(found.branches.iter().map(|branch| branch.span))
        .collect();
    let mut ignored = Vec::new();
    let mut open: Option<u32> = None;
    for comment in comments {
        let text = comment
            .content_span()
            .source_text(source)
            .trim()
            .trim_start_matches('*')
            .trim();
        let Some(rest) = ["v8 ignore", "c8 ignore", "istanbul ignore"]
            .iter()
            .find_map(|tool| text.strip_prefix(tool))
        else {
            continue;
        };
        let word = rest.split_whitespace().next().unwrap_or("");
        match word {
            "start" => open = Some(comment.span.end),
            "stop" => {
                if let Some(start) = open.take() {
                    ignored.push(Span::new(start, comment.span.start));
                }
            }
            "next" => {
                // The widest thing that starts first after the comment.
                let after = comment.span.end;
                if let Some(first) = starts
                    .iter()
                    .filter(|span| span.start >= after)
                    .map(|span| span.start)
                    .min()
                    && let Some(widest) = starts
                        .iter()
                        .filter(|span| span.start == first)
                        .max_by_key(|span| span.end)
                {
                    ignored.push(*widest);
                }
            }
            _ => {}
        }
    }
    if let Some(start) = open {
        ignored.push(Span::new(
            start,
            u32::try_from(source.len()).unwrap_or(u32::MAX),
        ));
    }
    ignored
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A run of `source` as written (no reprint), counted by `blocks`.
    fn run(source: &str, blocks: &[(u32, u32, u64)]) -> FileCoverage {
        let executed = Executed {
            text: source.to_string(),
            prelude: 0,
            mappings: None,
        };
        let takes = vec![
            blocks
                .iter()
                .map(|&(start, end, count)| Block { start, end, count })
                .collect(),
        ];
        measure(Path::new("m.js"), source, Some((&executed, &takes)))
    }

    #[test]
    fn a_function_never_called_is_uncovered_with_what_it_holds() {
        let source = "export function used() {\n  return 1;\n}\nexport function unused() {\n  return 2;\n}\n";
        let unused = source.find("function unused").unwrap() as u32;
        let file = run(
            source,
            &[
                (0, source.len() as u32, 1),
                (unused, source.len() as u32 - 1, 0),
            ],
        );
        let names: Vec<_> = file
            .functions
            .iter()
            .map(|f| (f.name.as_str(), f.count))
            .collect();
        assert_eq!(names, [("used", 1), ("unused", 0)]);
        let lines = file.lines();
        assert_eq!(lines.get(&2), Some(&1));
        assert_eq!(lines.get(&5), Some(&0));
    }

    #[test]
    fn an_if_without_else_counts_the_times_it_was_not_taken() {
        let source = "function f(x) {\n  if (x) {\n    g();\n  }\n}\n";
        let consequent = source.find('{').unwrap();
        let block = source[consequent + 1..].find('{').unwrap() + consequent + 1;
        let end = source[block..].find('}').unwrap() + block + 1;
        // Called three times, the consequent taken once.
        let file = run(
            source,
            &[
                (0, source.len() as u32, 1),
                (9, source.len() as u32 - 1, 3),
                (block as u32, end as u32, 1),
            ],
        );
        let branch = &file.branches[0];
        assert_eq!(branch.kind, "if");
        assert_eq!(
            branch.paths.iter().map(|p| p.count).collect::<Vec<_>>(),
            [1, 2]
        );
    }

    #[test]
    fn functions_take_their_names_from_where_they_are_assigned() {
        let source = "const add = (a, b) => a + b;\nconst o = { m() {}, n: function () {} };\nsetTimeout(() => {});\n";
        let file = run(source, &[(0, source.len() as u32, 1)]);
        let names: Vec<_> = file.functions.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["add", "m", "n", "(anonymous_0)"]);
    }

    #[test]
    fn ignore_comments_leave_code_out() {
        let source = "/* v8 ignore next */\nfunction a() {}\n/* c8 ignore start */\nb();\nc();\n/* c8 ignore stop */\nd();\n";
        let file = run(source, &[(0, source.len() as u32, 1)]);
        assert!(file.functions.is_empty());
        assert_eq!(file.statements.len(), 1);
        assert_eq!(file.statements[0].location.start.line, 7);
    }

    #[test]
    fn a_file_no_test_loaded_is_all_uncovered() {
        let source = "export const a = 1;\nexport function b() { return 2; }\n";
        let file = measure(Path::new("m.ts"), source, None);
        assert!(!file.statements.is_empty());
        assert!(file.statements.iter().all(|s| s.count == 0));
        assert_eq!(file.functions[0].count, 0);
    }

    #[test]
    fn a_function_whose_start_did_not_map_is_counted_from_inside_it() {
        // oxc maps the parameter, not the `(` that starts the arrow.
        let source = "export const never = (x: number) => x * 2;\n";
        let executed = Executed {
            text: "export const never = (x) => x * 2;\n".to_string(),
            prelude: 0,
            mappings: Some(vec![
                [0, 0, 0, 0],
                [0, 13, 0, 13],
                [0, 22, 0, 22],
                [0, 28, 0, 36],
            ]),
        };
        let takes = vec![vec![
            Block {
                start: 0,
                end: 35,
                count: 1,
            },
            Block {
                start: 21,
                end: 33,
                count: 0,
            },
        ]];
        let file = measure(Path::new("m.ts"), source, Some((&executed, &takes)));
        assert_eq!(file.functions[0].count, 0);
        // The declaration itself ran.
        assert_eq!(file.statements[0].count, 1);
    }

    #[test]
    fn a_reprinted_file_is_counted_where_it_was_written() {
        // Written with a type the run no longer has, which moves the column.
        let source = "let n: number = 1;\n";
        let executed = Executed {
            text: "let n = 1;\n".to_string(),
            prelude: 0,
            mappings: Some(vec![[0, 0, 0, 0], [0, 4, 0, 4]]),
        };
        let takes = vec![vec![Block {
            start: 0,
            end: 11,
            count: 7,
        }]];
        let file = measure(Path::new("m.ts"), source, Some((&executed, &takes)));
        assert_eq!(file.statements[0].count, 7);
    }
}
