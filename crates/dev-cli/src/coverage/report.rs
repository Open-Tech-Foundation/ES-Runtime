//! The coverage reports: Istanbul's measures in Istanbul's formats, so the
//! tools that read Vitest's and Jest's coverage read these too.

use std::fmt::Write as _;
use std::path::Path;

use serde_json::{Map, Value as Json, json};

use super::Thresholds;
use super::map::{FileCoverage, Location};

/// Covered out of total, for one measure.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Totals {
    pub total: usize,
    pub covered: usize,
}

impl Totals {
    /// Istanbul's percentage: rounded down to two places, and 100 when there
    /// is nothing to cover.
    pub fn percent(self) -> f64 {
        if self.total == 0 {
            return 100.0;
        }
        #[expect(clippy::cast_precision_loss, reason = "counts of source constructs")]
        let exact = self.covered as f64 * 100.0 / self.total as f64;
        (exact * 100.0).floor() / 100.0
    }

    fn add(&mut self, other: Totals) {
        self.total += other.total;
        self.covered += other.covered;
    }

    fn json(self) -> Json {
        json!({
            "total": self.total,
            "covered": self.covered,
            "skipped": 0,
            "pct": self.percent(),
        })
    }
}

/// The four measures of a file, or of all of them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub statements: Totals,
    pub branches: Totals,
    pub functions: Totals,
    pub lines: Totals,
}

impl Summary {
    pub fn of(file: &FileCoverage) -> Self {
        let count = |counts: &mut dyn Iterator<Item = u64>| {
            let mut totals = Totals::default();
            for count in counts {
                totals.total += 1;
                totals.covered += usize::from(count > 0);
            }
            totals
        };
        Self {
            statements: count(&mut file.statements.iter().map(|s| s.count)),
            branches: count(
                &mut file
                    .branches
                    .iter()
                    .flat_map(|b| b.paths.iter().map(|p| p.count)),
            ),
            functions: count(&mut file.functions.iter().map(|f| f.count)),
            lines: count(&mut file.lines().into_values()),
        }
    }

    pub fn add(&mut self, other: Summary) {
        self.statements.add(other.statements);
        self.branches.add(other.branches);
        self.functions.add(other.functions);
        self.lines.add(other.lines);
    }

    fn json(self) -> Json {
        json!({
            "lines": self.lines.json(),
            "statements": self.statements.json(),
            "functions": self.functions.json(),
            "branches": self.branches.json(),
        })
    }
}

/// A file's name in a report: its path inside the project, with `/`.
pub fn name(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn percent(value: f64) -> String {
    let text = format!("{value:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// Line numbers as runs: `3-5,9`.
fn runs(lines: &[u32]) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let start = lines[index];
        let mut end = start;
        while index + 1 < lines.len() && lines[index + 1] == end + 1 {
            index += 1;
            end = lines[index];
        }
        out.push(if start == end {
            start.to_string()
        } else {
            format!("{start}-{end}")
        });
        index += 1;
    }
    out.join(",")
}

/// `text`: the table a person reads, as Istanbul lays it out.
pub fn text(root: &Path, files: &[FileCoverage]) -> String {
    let mut rows = Vec::new();
    let mut total = Summary::default();
    for file in files {
        let summary = Summary::of(file);
        total.add(summary);
        let uncovered: Vec<u32> = file
            .lines()
            .into_iter()
            .filter(|(_, count)| *count == 0)
            .map(|(line, _)| line)
            .collect();
        rows.push([
            name(root, &file.path),
            percent(summary.statements.percent()),
            percent(summary.branches.percent()),
            percent(summary.functions.percent()),
            percent(summary.lines.percent()),
            runs(&uncovered),
        ]);
    }
    rows.insert(
        0,
        [
            "All files".to_string(),
            percent(total.statements.percent()),
            percent(total.branches.percent()),
            percent(total.functions.percent()),
            percent(total.lines.percent()),
            String::new(),
        ],
    );
    let heading = [
        "File",
        "% Stmts",
        "% Branch",
        "% Funcs",
        "% Lines",
        "Uncovered Line #s",
    ];
    let widths: Vec<usize> = (0..6)
        .map(|column| {
            rows.iter()
                .map(|row| row[column].chars().count())
                .chain(std::iter::once(heading[column].len()))
                .max()
                .unwrap_or(0)
        })
        .collect();
    let rule = widths
        .iter()
        .map(|width| "-".repeat(width + 2))
        .collect::<Vec<_>>()
        .join("|");
    let line = |cells: [&str; 6]| {
        cells
            .iter()
            .enumerate()
            .map(|(column, cell)| {
                if column == 0 || column == 5 {
                    format!(" {cell:<width$} ", width = widths[column])
                } else {
                    format!(" {cell:>width$} ", width = widths[column])
                }
            })
            .collect::<Vec<_>>()
            .join("|")
    };
    let mut out = String::new();
    let _ = writeln!(out, "{rule}");
    let _ = writeln!(out, "{}", line(heading));
    let _ = writeln!(out, "{rule}");
    for row in &rows {
        let _ = writeln!(
            out,
            "{}",
            line([&row[0], &row[1], &row[2], &row[3], &row[4], &row[5]])
        );
    }
    let _ = writeln!(out, "{rule}");
    out
}

/// `lcov`: `lcov.info`, which genhtml, Codecov and most CI services read.
pub fn lcov(root: &Path, files: &[FileCoverage]) -> String {
    let mut out = String::new();
    for file in files {
        let _ = writeln!(out, "TN:");
        let _ = writeln!(out, "SF:{}", name(root, &file.path));
        for function in &file.functions {
            let _ = writeln!(out, "FN:{},{}", function.location.start.line, function.name);
        }
        for function in &file.functions {
            let _ = writeln!(out, "FNDA:{},{}", function.count, function.name);
        }
        let _ = writeln!(out, "FNF:{}", file.functions.len());
        let _ = writeln!(
            out,
            "FNH:{}",
            file.functions.iter().filter(|f| f.count > 0).count()
        );
        for (line, count) in file.lines() {
            let _ = writeln!(out, "DA:{line},{count}");
        }
        let lines = file.lines();
        let _ = writeln!(out, "LF:{}", lines.len());
        let _ = writeln!(
            out,
            "LH:{}",
            lines.values().filter(|count| **count > 0).count()
        );
        let mut found = 0;
        let mut hit = 0;
        for (block, branch) in file.branches.iter().enumerate() {
            // `-` for a branch whose block never ran at all, as lcov spells it.
            let ran = branch.paths.iter().any(|path| path.count > 0);
            for (index, path) in branch.paths.iter().enumerate() {
                let taken = if ran {
                    path.count.to_string()
                } else {
                    "-".to_string()
                };
                let _ = writeln!(
                    out,
                    "BRDA:{},{block},{index},{taken}",
                    branch.location.start.line
                );
                found += 1;
                hit += usize::from(path.count > 0);
            }
        }
        let _ = writeln!(out, "BRF:{found}");
        let _ = writeln!(out, "BRH:{hit}");
        let _ = writeln!(out, "end_of_record");
    }
    out
}

fn location(location: Location) -> Json {
    json!({
        "start": { "line": location.start.line, "column": location.start.column },
        "end": { "line": location.end.line, "column": location.end.column },
    })
}

/// `json`: Istanbul's `coverage-final.json`, every count by file.
pub fn istanbul_json(files: &[FileCoverage]) -> String {
    let mut all = Map::new();
    for file in files {
        let path = file.path.display().to_string();
        let indexed = |items: Vec<Json>| -> Map<String, Json> {
            items
                .into_iter()
                .enumerate()
                .map(|(index, item)| (index.to_string(), item))
                .collect()
        };
        all.insert(
            path.clone(),
            json!({
                "path": path,
                "statementMap": indexed(file.statements.iter().map(|s| location(s.location)).collect()),
                "fnMap": indexed(file.functions.iter().map(|f| json!({
                    "name": f.name,
                    "decl": location(f.declaration),
                    "loc": location(f.location),
                    "line": f.location.start.line,
                })).collect()),
                "branchMap": indexed(file.branches.iter().map(|b| json!({
                    "loc": location(b.location),
                    "type": b.kind,
                    "locations": b.paths.iter().map(|p| location(p.location)).collect::<Vec<_>>(),
                    "line": b.location.start.line,
                })).collect()),
                "s": indexed(file.statements.iter().map(|s| json!(s.count)).collect()),
                "f": indexed(file.functions.iter().map(|f| json!(f.count)).collect()),
                "b": indexed(file.branches.iter().map(|b| json!(b.paths.iter().map(|p| p.count).collect::<Vec<_>>())).collect()),
            }),
        );
    }
    Json::Object(all).to_string()
}

/// `json-summary`: Istanbul's `coverage-summary.json`, the four measures by
/// file and in total.
pub fn json_summary(files: &[FileCoverage]) -> String {
    let mut all = Map::new();
    let mut total = Summary::default();
    for file in files {
        let summary = Summary::of(file);
        total.add(summary);
        all.insert(file.path.display().to_string(), summary.json());
    }
    let mut out = Map::new();
    out.insert("total".to_string(), total.json());
    out.extend(all);
    Json::Object(out).to_string()
}

/// What falls short of `thresholds`, one sentence each.
pub fn short_of(total: &Summary, thresholds: &Thresholds) -> Vec<String> {
    let mut failures = Vec::new();
    for (name, threshold, totals) in [
        ("statements", thresholds.statements, total.statements),
        ("branches", thresholds.branches, total.branches),
        ("functions", thresholds.functions, total.functions),
        ("lines", thresholds.lines, total.lines),
    ] {
        let Some(threshold) = threshold else {
            continue;
        };
        if threshold >= 0.0 {
            if totals.percent() < threshold {
                failures.push(format!(
                    "{name} coverage is {}%, below the {}% threshold",
                    percent(totals.percent()),
                    percent(threshold)
                ));
            }
        } else {
            let uncovered = totals.total - totals.covered;
            #[expect(clippy::cast_precision_loss, reason = "a count of source constructs")]
            if uncovered as f64 > -threshold {
                failures.push(format!(
                    "{uncovered} {name} are not covered, more than the {} allowed",
                    percent(-threshold)
                ));
            }
        }
    }
    failures
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coverage::map::{BranchCoverage, Counted, FunctionCoverage, Position};

    fn at(line: u32) -> Location {
        Location {
            start: Position { line, column: 0 },
            end: Position { line, column: 5 },
        }
    }

    fn sample() -> FileCoverage {
        FileCoverage {
            path: "/p/src/math.ts".into(),
            statements: [(1, 1), (2, 3), (3, 0), (4, 0), (6, 1)]
                .map(|(line, count)| Counted {
                    location: at(line),
                    count,
                })
                .to_vec(),
            functions: vec![
                FunctionCoverage {
                    name: "add".into(),
                    declaration: at(1),
                    location: at(1),
                    count: 3,
                },
                FunctionCoverage {
                    name: "never".into(),
                    declaration: at(3),
                    location: at(3),
                    count: 0,
                },
            ],
            branches: vec![BranchCoverage {
                kind: "if",
                location: at(2),
                paths: vec![
                    Counted {
                        location: at(2),
                        count: 3,
                    },
                    Counted {
                        location: at(2),
                        count: 0,
                    },
                ],
            }],
        }
    }

    #[test]
    fn percentages_round_down_and_nothing_is_all() {
        assert_eq!(
            Totals {
                total: 3,
                covered: 2
            }
            .percent(),
            66.66
        );
        assert_eq!(
            Totals {
                total: 0,
                covered: 0
            }
            .percent(),
            100.0
        );
    }

    #[test]
    fn the_text_table_names_uncovered_lines_as_runs() {
        let table = text(Path::new("/p"), &[sample()]);
        assert!(table.contains("% Stmts"), "{table}");
        assert!(table.contains(" src/math.ts "), "{table}");
        assert!(table.contains(" 60 "), "{table}");
        assert!(table.contains(" 3-4 "), "{table}");
        assert!(
            table.lines().nth(3).unwrap().starts_with(" All files"),
            "{table}"
        );
    }

    #[test]
    fn lcov_holds_functions_lines_and_branches() {
        let info = lcov(Path::new("/p"), &[sample()]);
        for line in [
            "SF:src/math.ts",
            "FN:1,add",
            "FNDA:0,never",
            "FNF:2",
            "FNH:1",
            "DA:3,0",
            "LF:5",
            "LH:3",
            "BRDA:2,0,1,0",
            "BRF:2",
            "BRH:1",
            "end_of_record",
        ] {
            assert!(info.lines().any(|l| l == line), "{line} in\n{info}");
        }
    }

    #[test]
    fn istanbul_json_indexes_every_count() {
        let parsed: Json = serde_json::from_str(&istanbul_json(&[sample()])).unwrap();
        let file = &parsed["/p/src/math.ts"];
        assert_eq!(file["s"]["1"], 3);
        assert_eq!(file["fnMap"]["1"]["name"], "never");
        assert_eq!(file["b"]["0"], json!([3, 0]));
        let summary: Json = serde_json::from_str(&json_summary(&[sample()])).unwrap();
        assert_eq!(summary["total"]["lines"]["pct"], 60.0);
    }

    #[test]
    fn thresholds_are_percentages_or_counts_left_uncovered() {
        let total = Summary::of(&sample());
        let failures = short_of(
            &total,
            &Thresholds {
                lines: Some(80.0),
                functions: Some(50.0),
                branches: Some(-1.0),
                statements: Some(-1.0),
            },
        );
        assert_eq!(
            failures,
            [
                "2 statements are not covered, more than the 1 allowed",
                "lines coverage is 60%, below the 80% threshold",
            ]
        );
    }
}
