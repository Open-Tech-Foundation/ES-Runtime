//! What `esdev test --reporter` writes for a machine to read.
//!
//! **One set of results, several shapes.** Each file's run ends as a
//! [`FileResult`] — its cases by full name, each passed, failed or skipped,
//! with how long it took and why it failed — however it ran: in a child
//! process, which hands it to the parent in a file, or in a browser page. The
//! formats here are drawn from that and nothing else, so a format never sees
//! what a test printed and what a test printed never lands in a format.
//!
//! The formats are the ones CI systems already read, in the shapes the other
//! runners write them:
//!
//! - `json` — one object per line: a `case` for each failure (or each listed
//!   test), a `bench` for each benchmark measured, a `file` for each file, a
//!   `summary` at the end.
//! - `junit` — `<testsuites>` / `<testsuite>` per file / `<testcase>`, with the
//!   file as `classname` and the full name as `name`, as Vitest writes it.
//! - `tap` — TAP version 13, one line per test, failures with a YAML block.
//! - `dots` — a character per test as files finish, then the failures.

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

/// The reporters `--reporter` accepts, and what each is for.
pub const REPORTERS: &[(&str, &str)] = &[
    ("human", "what a person reads, the default"),
    ("json", "one JSON object per line"),
    ("junit", "JUnit XML, for CI systems"),
    ("tap", "TAP version 13"),
    ("dots", "a character per test, then the failures"),
];

/// One test's result.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CaseResult {
    /// The full name, groups included.
    pub name: String,
    /// `passed`, `failed`, `skipped` or `listed`.
    pub status: String,
    /// Why it failed; empty otherwise.
    pub detail: String,
    /// How long it ran, in milliseconds.
    pub duration_ms: f64,
    /// What its benchmarks measured, under `esdev bench`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub benchmarks: Vec<serde_json::Value>,
}

/// One file's results.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FileResult {
    /// The path the file was run from.
    pub file: String,
    /// The path as a report names it, inside the project.
    pub name: String,
    pub cases: Vec<CaseResult>,
    /// The browser it ran in, when a run used several.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser: Option<String>,
}

impl FileResult {
    fn count(&self, status: &str) -> usize {
        self.cases
            .iter()
            .filter(|case| case.status == status)
            .count()
    }

    pub fn failed(&self) -> usize {
        self.count("failed")
    }

    fn seconds(&self) -> f64 {
        // A fold from 0, not `sum`: an empty float sum is -0, printed "-0.000".
        self.cases
            .iter()
            .fold(0.0, |total, case| total + case.duration_ms)
            / 1000.0
    }

    /// A file that did not report its own results — it timed out, crashed, or
    /// could not be started — as one failed case saying so, so a report never
    /// shows a file that failed as one that simply had no tests.
    pub fn broken(file: String, name: String, why: &str) -> FileResult {
        FileResult {
            browser: None,
            file,
            name,
            cases: vec![CaseResult {
                name: "(file)".to_string(),
                status: "failed".to_string(),
                detail: why.to_string(),
                duration_ms: 0.0,
                benchmarks: Vec::new(),
            }],
        }
    }
}

/// `json`: the lines one file contributes. The failures, or with `--list` the
/// listed tests, then the file's counts.
pub fn json_file(result: &FileResult) -> String {
    let string = |text: &str| serde_json::Value::String(text.to_string()).to_string();
    let mut out = String::new();
    for case in &result.cases {
        for bench in &case.benchmarks {
            let _ = writeln!(
                out,
                r#"{{"type":"bench","file":{},"test":{},"result":{}}}"#,
                string(&result.file),
                string(&case.name),
                bench
            );
        }
        match case.status.as_str() {
            "failed" => {
                let _ = writeln!(
                    out,
                    r#"{{"type":"case","file":{},"name":{},"status":"failed","detail":{}}}"#,
                    string(&result.file),
                    string(&case.name),
                    string(&case.detail)
                );
            }
            "listed" => {
                let _ = writeln!(
                    out,
                    r#"{{"type":"case","file":{},"name":{},"status":"listed"}}"#,
                    string(&result.file),
                    string(&case.name)
                );
            }
            _ => {}
        }
    }
    if result.cases.iter().any(|case| case.status == "listed") {
        return out;
    }
    let browser = result
        .browser
        .as_deref()
        .map(|browser| format!(r#","browser":{}"#, string(browser)))
        .unwrap_or_default();
    let _ = writeln!(
        out,
        r#"{{"type":"file","file":{}{browser},"passed":{},"failed":{},"skipped":{}}}"#,
        string(&result.file),
        result.count("passed"),
        result.failed(),
        result.count("skipped")
    );
    out
}

/// `json`: the line a run ends with.
pub fn json_summary(files: usize, failed: usize) -> String {
    format!("{{\"type\":\"summary\",\"files\":{files},\"failed\":{failed}}}\n")
}

/// `junit`: the whole run as one document.
pub fn junit(files: &[FileResult]) -> String {
    let tests: usize = files.iter().map(|file| file.cases.len()).sum();
    let failures: usize = files.iter().map(FileResult::failed).sum();
    let skipped: usize = files.iter().map(|file| file.count("skipped")).sum();
    let time = files.iter().fold(0.0, |total, file| total + file.seconds());
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\" ?>\n");
    let _ = writeln!(
        out,
        "<testsuites name=\"esdev test\" tests=\"{tests}\" failures=\"{failures}\" errors=\"0\" skipped=\"{skipped}\" time=\"{time:.3}\">"
    );
    for file in files {
        let _ = writeln!(
            out,
            "  <testsuite name=\"{}\" tests=\"{}\" failures=\"{}\" errors=\"0\" skipped=\"{}\" time=\"{:.3}\">",
            xml(&file.name),
            file.cases.len(),
            file.failed(),
            file.count("skipped"),
            file.seconds()
        );
        for case in &file.cases {
            let _ = write!(
                out,
                "    <testcase classname=\"{}\" name=\"{}\" time=\"{:.3}\"",
                xml(&file.name),
                xml(&case.name),
                case.duration_ms / 1000.0
            );
            match case.status.as_str() {
                "failed" => {
                    let message = case.detail.lines().next().unwrap_or_default();
                    let kind = message
                        .split_once(':')
                        .map_or("Error", |(kind, _)| kind)
                        .trim();
                    let kind = if kind.contains(' ') { "Error" } else { kind };
                    let _ = writeln!(
                        out,
                        ">\n      <failure message=\"{}\" type=\"{}\">{}</failure>\n    </testcase>",
                        xml(message),
                        xml(kind),
                        xml(&case.detail)
                    );
                }
                "skipped" => {
                    let _ = writeln!(out, ">\n      <skipped/>\n    </testcase>");
                }
                _ => {
                    let _ = writeln!(out, "/>");
                }
            }
        }
        let _ = writeln!(out, "  </testsuite>");
    }
    out.push_str("</testsuites>\n");
    out
}

/// Text as XML attribute or element content. Characters XML cannot hold at
/// all — control characters other than tab and line breaks — are dropped
/// rather than making the document one no parser will read.
fn xml(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t' | '\n' | '\r' => out.push(c),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// `tap`: the whole run, TAP version 13.
pub fn tap(files: &[FileResult]) -> String {
    let total: usize = files.iter().map(|file| file.cases.len()).sum();
    let mut out = format!("TAP version 13\n1..{total}\n");
    let mut number = 0;
    for file in files {
        for case in &file.cases {
            number += 1;
            // `#` starts a directive in TAP, so a name cannot carry one.
            let name = format!("{} > {}", file.name, case.name).replace('#', "\\#");
            match case.status.as_str() {
                "failed" => {
                    let _ = writeln!(out, "not ok {number} - {name}");
                    out.push_str("  ---\n  error: |-\n");
                    for line in case.detail.lines() {
                        let _ = writeln!(out, "    {line}");
                    }
                    out.push_str("  ...\n");
                }
                "skipped" => {
                    let _ = writeln!(out, "ok {number} - {name} # SKIP");
                }
                _ => {
                    let _ = writeln!(out, "ok {number} - {name}");
                }
            }
        }
    }
    out
}

/// `dots`: one file's characters — `.` passed, `x` failed, `-` skipped.
pub fn dots(result: &FileResult) -> String {
    result
        .cases
        .iter()
        .map(|case| match case.status.as_str() {
            "passed" => '.',
            "failed" => 'x',
            _ => '-',
        })
        .collect()
}

/// `dots`: what comes after the characters — every failure, then the counts.
pub fn dots_end(files: &[FileResult]) -> String {
    let mut out = String::from("\n");
    for file in files {
        for case in file.cases.iter().filter(|case| case.status == "failed") {
            let _ = writeln!(out, "\nFAIL {} > {}", file.name, case.name);
            for line in case.detail.lines() {
                let _ = writeln!(out, "  {line}");
            }
        }
    }
    let passed: usize = files.iter().map(|file| file.count("passed")).sum();
    let failed: usize = files.iter().map(FileResult::failed).sum();
    let skipped: usize = files.iter().map(|file| file.count("skipped")).sum();
    let _ = write!(out, "\n{passed} passed, {failed} failed");
    if skipped > 0 {
        let _ = write!(out, ", {skipped} skipped");
    }
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn case(name: &str, status: &str, detail: &str, duration_ms: f64) -> CaseResult {
        CaseResult {
            name: name.to_string(),
            status: status.to_string(),
            detail: detail.to_string(),
            duration_ms,
            benchmarks: Vec::new(),
        }
    }

    #[test]
    fn json_names_the_browser_of_a_run_that_used_several() {
        let result = FileResult {
            browser: Some("firefox".to_string()),
            file: "a.test.ts".to_string(),
            name: "a.test.ts [firefox]".to_string(),
            cases: vec![case("t", "passed", "", 1.0)],
        };
        assert_eq!(
            json_file(&result),
            "{\"type\":\"file\",\"file\":\"a.test.ts\",\"browser\":\"firefox\",\"passed\":1,\"failed\":0,\"skipped\":0}\n"
        );
    }

    #[test]
    fn json_reports_each_benchmark_before_its_file() {
        let mut measured = case("parse", "passed", "", 12.0);
        measured.benchmarks = vec![serde_json::json!({ "name": "fast", "samples": 10 })];
        let result = FileResult {
            browser: None,
            file: "a.bench.ts".to_string(),
            name: "a.bench.ts".to_string(),
            cases: vec![measured],
        };
        assert_eq!(
            json_file(&result),
            "{\"type\":\"bench\",\"file\":\"a.bench.ts\",\"test\":\"parse\",\"result\":{\"name\":\"fast\",\"samples\":10}}\n\
             {\"type\":\"file\",\"file\":\"a.bench.ts\",\"passed\":1,\"failed\":0,\"skipped\":0}\n"
        );
        // A case with none carries no field, so older summaries read the same.
        let plain = serde_json::to_value(case("t", "passed", "", 1.0)).unwrap();
        assert!(plain.get("benchmarks").is_none());
    }

    fn run() -> Vec<FileResult> {
        vec![
            FileResult {
                browser: None,
                file: "/p/src/a.test.ts".to_string(),
                name: "src/a.test.ts".to_string(),
                cases: vec![
                    case("adds", "passed", "", 1.5),
                    case(
                        "g > <fails> & \"quotes\"",
                        "failed",
                        "TypeError: bad\n    at a.test.ts:3:1",
                        2.0,
                    ),
                    case("later", "skipped", "", 0.0),
                ],
            },
            FileResult {
                browser: None,
                file: "/p/b.test.js".to_string(),
                name: "b.test.js".to_string(),
                cases: vec![case("b # one", "passed", "", 0.5)],
            },
        ]
    }

    #[test]
    fn junit_has_the_shape_ci_systems_read() {
        let xml = junit(&run());
        assert!(
            xml.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\" ?>\n"),
            "{xml}"
        );
        assert!(
            xml.contains("<testsuites name=\"esdev test\" tests=\"4\" failures=\"1\" errors=\"0\" skipped=\"1\" time=\"0.004\">"),
            "{xml}"
        );
        assert!(
            xml.contains("<testsuite name=\"src/a.test.ts\" tests=\"3\" failures=\"1\" errors=\"0\" skipped=\"1\" time=\"0.004\">"),
            "{xml}"
        );
        assert!(
            xml.contains("<testcase classname=\"src/a.test.ts\" name=\"adds\" time=\"0.002\"/>"),
            "{xml}"
        );
        assert!(
            xml.contains(
                "name=\"g &gt; &lt;fails&gt; &amp; &quot;quotes&quot;\" time=\"0.002\">\n      <failure message=\"TypeError: bad\" type=\"TypeError\">TypeError: bad\n    at a.test.ts:3:1</failure>"
            ),
            "{xml}"
        );
        assert!(
            xml.contains("name=\"later\" time=\"0.000\">\n      <skipped/>"),
            "{xml}"
        );
        assert!(xml.ends_with("</testsuites>\n"), "{xml}");
    }

    #[test]
    fn xml_drops_what_no_parser_accepts() {
        assert_eq!(xml("a\u{1b}[31mb\tc"), "a[31mb\tc");
    }

    #[test]
    fn tap_numbers_each_test_and_explains_failures() {
        let text = tap(&run());
        assert_eq!(
            text,
            "TAP version 13\n1..4\n\
             ok 1 - src/a.test.ts > adds\n\
             not ok 2 - src/a.test.ts > g > <fails> & \"quotes\"\n  ---\n  error: |-\n    TypeError: bad\n        at a.test.ts:3:1\n  ...\n\
             ok 3 - src/a.test.ts > later # SKIP\n\
             ok 4 - b.test.js > b \\# one\n"
        );
    }

    #[test]
    fn dots_mark_each_test_then_list_the_failures() {
        let files = run();
        assert_eq!(dots(&files[0]), ".x-");
        assert_eq!(dots(&files[1]), ".");
        let end = dots_end(&files);
        assert!(
            end.contains("\nFAIL src/a.test.ts > g > <fails> & \"quotes\"\n  TypeError: bad\n"),
            "{end}"
        );
        assert!(end.ends_with("\n2 passed, 1 failed, 1 skipped\n"), "{end}");
    }

    #[test]
    fn json_keeps_its_line_shapes() {
        let files = run();
        assert_eq!(
            json_file(&files[0]),
            "{\"type\":\"case\",\"file\":\"/p/src/a.test.ts\",\"name\":\"g > <fails> & \\\"quotes\\\"\",\"status\":\"failed\",\"detail\":\"TypeError: bad\\n    at a.test.ts:3:1\"}\n\
             {\"type\":\"file\",\"file\":\"/p/src/a.test.ts\",\"passed\":1,\"failed\":1,\"skipped\":1}\n"
        );
        assert_eq!(
            json_summary(2, 1),
            "{\"type\":\"summary\",\"files\":2,\"failed\":1}\n"
        );
    }

    #[test]
    fn a_file_that_reported_nothing_is_one_failure_saying_why() {
        let broken = FileResult::broken("/p/x.test.js".into(), "x.test.js".into(), "timed out");
        assert_eq!(broken.failed(), 1);
        assert!(junit(&[broken]).contains("<failure message=\"timed out\""));
    }

    #[test]
    fn an_empty_run_takes_no_time_rather_than_minus_none() {
        let xml = junit(&[]);
        assert!(
            xml.contains("tests=\"0\"") && xml.contains("time=\"0.000\""),
            "{xml}"
        );
    }
}
