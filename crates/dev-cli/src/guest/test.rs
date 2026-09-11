//! `runtime:test` — the test API, and the host that keeps the score.
//!
//! **`esdev` only.** A test file is never a production artifact (D59), so the
//! binary that serves production has no reason to be able to run one.
//!
//! # Why the results live here and not in the module
//!
//! The obvious design is for `runtime:test` to hold its own array of results
//! and print them at the end. There is no "at the end" available to it: a
//! module's body finishes long before the tests it started, and JavaScript has
//! no hook for "the program is about to stop".
//!
//! That is what the old harness solved by *appending* an epilogue —
//! `await Promise.all(pending)` plus a report — to the test file's own source.
//! It worked, and it cost the file its own shape: the harness had to be one
//! physical line so line 1 stayed line 1, which meant it could carry no `//`
//! comments, and the file that ran was not the file the developer wrote.
//!
//! Keeping the score in the host removes all of it. `test()` says a case was
//! registered, later that it started, later still how it ended; `esdev` reads
//! the tally after the program reaches quiescence, prints it, and picks the
//! exit code. Nothing is injected into anybody's source.
//!
//! It also fixes a failure the old design could not see. A test whose promise
//! never settles used to hang the program at the epilogue's `Promise.all`;
//! here, the case is simply never finished, and a started-but-unfinished case
//! is reported as a **failure** rather than silently left out of a green run.
//!
//! # Three states, because the cases are a queue
//!
//! Cases run one at a time ([`runtime:test`](../test.js) explains why), so
//! "never settled" splits in two. A case that *started* and hung is stuck on
//! something of its own; a case that never started is behind one that hung, and
//! reporting the two identically points a reader at twelve innocent tests. So
//! the host is told at registration, told again when the queue reaches the
//! case, and the report names which of the two happened.
//!
//! # Why a thread-local
//!
//! The tally belongs to the *process*: one program, one report, printed after
//! the run by code that is nowhere near the ops. Threading a handle out through
//! `Config` and the argument parser to reach `main` would be plumbing for a
//! value that can only ever have one instance.
//!
//! It is per-thread rather than global, and that is exactly right: extensions
//! are registered on the main agent only, so a worker cannot import
//! `runtime:test` at all and can never have a tally of its own to confuse with
//! this one.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::process::ExitCode;

use es_runtime_cli_common::{ExtensionContext, HostExtension, HostModule, OpDecl, Value};

/// One test case, from `test()` to whatever became of it.
struct Case {
    name: String,
    file: Option<PathBuf>,
    /// Whether the case ever got as far as running. Cases are queued and run
    /// one at a time, so a case that never started is not the same failure as
    /// one that started and hung — the first says an *earlier* test never
    /// finished, and pointing at the wrong one costs an afternoon.
    started: bool,
    /// `None` while it is still running — and still `None` at the end if it
    /// never settled, which is a failure with a name of its own.
    outcome: Option<Outcome>,
}

enum Outcome {
    Passed,
    /// The stack, when the error had one; the error's text otherwise.
    Failed(String),
    /// Never run, and **said so**. A skipped case is counted in the tally
    /// rather than left out of it, for the same reason an unfinished one is a
    /// failure: a green run that quietly ran fewer tests than it printed is the
    /// worst thing a runner can do.
    Skipped(Skip),
}

/// Why a case did not run.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Skip {
    /// `test.skip` / `describe.skip` — the file said so.
    Asked,
    /// Something else asked to be the only thing that runs. Counted apart,
    /// because a `.only` left in a commit turns a suite green in a tenth of the
    /// time and the tally is the only place that shows it.
    Only,
}

thread_local! {
    /// Every case this agent registered, in the order `test()` was called.
    static CASES: RefCell<Vec<Case>> = const { RefCell::new(Vec::new()) };
    static SNAPSHOTS: RefCell<SnapshotState> = RefCell::new(SnapshotState::default());
}

const SNAPSHOT_VERSION: u32 = 1;

#[derive(Default)]
struct SnapshotState {
    update: bool,
    ci: bool,
    full_diff: bool,
    current_file: Option<PathBuf>,
    files: BTreeMap<PathBuf, SnapshotFile>,
    file_writes: BTreeMap<PathBuf, Vec<u8>>,
    matched: usize,
    failed: usize,
    written: usize,
    updated: usize,
    removed: usize,
}

struct SnapshotFile {
    snapshots: BTreeMap<String, String>,
    used: BTreeSet<String>,
    dirty: bool,
}

impl Default for SnapshotFile {
    fn default() -> Self {
        Self {
            snapshots: BTreeMap::new(),
            used: BTreeSet::new(),
            dirty: false,
        }
    }
}

fn snapshot_path(test_file: &std::path::Path) -> PathBuf {
    let name = test_file.file_name().unwrap_or_default();
    test_file
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("__snapshots__")
        .join(format!("{}.snap", name.to_string_lossy()))
}

fn file_snapshot_path(test_file: &std::path::Path, name: &str) -> Result<PathBuf, String> {
    let path = std::path::Path::new(name);
    if name.is_empty()
        || path.components().count() != 1
        || !matches!(
            path.components().next(),
            Some(std::path::Component::Normal(_))
        )
    {
        return Err("toMatchFileSnapshot(name) needs a filename, not a path".to_string());
    }
    let test_name = test_file.file_name().unwrap_or_default();
    Ok(test_file
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("__snapshots__")
        .join(test_name)
        .join(path))
}

fn load_snapshots(test_file: &std::path::Path) -> Result<SnapshotFile, String> {
    let path = snapshot_path(test_file);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(SnapshotFile::default());
    };
    let mut lines = text.lines();
    if lines.next() != Some("// esdev snapshot v1") {
        return Err(format!(
            "{} is not an esdev snapshot format version {SNAPSHOT_VERSION} file",
            path.display()
        ));
    }
    let mut file = SnapshotFile::default();
    let mut key = None;
    let mut body = Vec::new();
    for line in lines {
        if let Some(rest) = line.strip_prefix("=== ") {
            if let Some(previous) = key.replace(
                rest.strip_suffix(" [value]")
                    .ok_or_else(|| {
                        format!("cannot read {}: malformed snapshot heading", path.display())
                    })?
                    .to_string(),
            ) {
                if body.last().is_some_and(String::is_empty) {
                    body.pop();
                }
                file.snapshots.insert(previous, body.join("\n"));
                body.clear();
            }
        } else if key.is_some() {
            body.push(line.strip_prefix("\\===").unwrap_or(line).to_string());
        }
    }
    if let Some(previous) = key {
        if body.last().is_some_and(String::is_empty) {
            body.pop();
        }
        file.snapshots.insert(previous, body.join("\n"));
    }
    Ok(file)
}

/// Configures snapshot ownership for this runtime before its extensions are
/// constructed. A normal child has one known file; unisolated mode sets it as
/// each module is imported.
pub fn configure_snapshots(file: Option<PathBuf>, update: bool, ci: bool, full_diff: bool) {
    SNAPSHOTS.with_borrow_mut(|state| {
        *state = SnapshotState {
            update,
            ci,
            full_diff,
            current_file: file,
            files: BTreeMap::new(),
            file_writes: BTreeMap::new(),
            matched: 0,
            failed: 0,
            written: 0,
            updated: 0,
            removed: 0,
        };
    });
}

fn set_snapshot_file(text: &str) {
    let file = url::Url::parse(text)
        .ok()
        .and_then(|url| url.to_file_path().ok())
        .unwrap_or_else(|| PathBuf::from(text));
    SNAPSHOTS.with_borrow_mut(|state| state.current_file = Some(file));
}

/// A small unified diff for the JSON a snapshot stores. Snapshot payloads are
/// deliberately bounded by reviewability, so a quadratic LCS is clearer than a
/// dependency and ample for this error path. Unchanged lines remain too: a
/// reader needs the surrounding JSON keys to identify what changed.
fn snapshot_diff(expected: &str, actual: &str, full: bool) -> String {
    let before: Vec<&str> = expected.lines().collect();
    let after: Vec<&str> = actual.lines().collect();
    if !full && before.len() + after.len() > 200 {
        let mut out =
            String::from("--- snapshot\n+++ received\n@@ large diff (use --full-diff) @@\n");
        for line in before.iter().take(50) {
            out.push_str(&format!("- {line}\n"));
        }
        for line in after.iter().take(50) {
            out.push_str(&format!("+ {line}\n"));
        }
        out.push_str(&format!(
            "… {} more changed lines\n",
            before.len() + after.len() - 100
        ));
        return out;
    }
    let mut lcs = vec![vec![0usize; after.len() + 1]; before.len() + 1];
    for i in (0..before.len()).rev() {
        for j in (0..after.len()).rev() {
            lcs[i][j] = if before[i] == after[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let mut out = String::from("--- snapshot\n+++ received\n@@\n");
    let (mut i, mut j) = (0, 0);
    while i < before.len() || j < after.len() {
        if i < before.len() && j < after.len() && before[i] == after[j] {
            out.push_str(&format!("  {}\n", before[i]));
            i += 1;
        } else if j < after.len() && (i == before.len() || lcs[i][j + 1] >= lcs[i + 1][j]) {
            out.push_str(&format!("+ {}\n", after[j]));
            j += 1;
        } else {
            out.push_str(&format!("- {}\n", before[i]));
            i += 1;
        }
    }
    out
}

fn check_snapshot(case_id: usize, key: String, actual: String) -> Result<(), String> {
    let (file, key) = CASES
        .with_borrow(|cases| {
            cases.get(case_id).and_then(|case| {
                case.file
                    .clone()
                    .map(|file| (file, format!("{}: {key}", case.name)))
            })
        })
        .ok_or_else(|| "toMatchSnapshot is available when esdev runs a test file".to_string())?;
    SNAPSHOTS.with_borrow_mut(|state| {
        if !state.files.contains_key(&file) {
            let loaded = load_snapshots(&file)?;
            state.files.insert(file.clone(), loaded);
        }
        let snapshots = state.files.get_mut(&file).expect("inserted above");
        snapshots.used.insert(key.clone());
        match snapshots.snapshots.get(&key) {
            Some(expected) if expected == &actual => {
                state.matched += 1;
                Ok(())
            }
            Some(expected) if !state.update => {
                state.failed += 1;
                Err(format!("snapshot changed: {key}\n\n{}\nRun esdev test --update-snapshots to accept this change.", snapshot_diff(expected, &actual, state.full_diff)))
            }
            None if state.ci => {
                state.failed += 1;
                Err(format!("no stored snapshot: {key}; --ci does not write them"))
            }
            Some(_) => {
                snapshots.snapshots.insert(key, actual);
                snapshots.dirty = true;
                state.updated += 1;
                Ok(())
            }
            None => {
                snapshots.snapshots.insert(key, actual);
                snapshots.dirty = true;
                state.written += 1;
                Ok(())
            }
        }
    })
}

fn check_file_snapshot(case_id: usize, name: &str, actual: Vec<u8>) -> Result<(), String> {
    let file = CASES
        .with_borrow(|cases| cases.get(case_id).and_then(|case| case.file.clone()))
        .ok_or_else(|| {
            "toMatchFileSnapshot is available when esdev runs a test file".to_string()
        })?;
    let path = file_snapshot_path(&file, name)?;
    SNAPSHOTS.with_borrow_mut(|state| {
        let expected = state
            .file_writes
            .get(&path)
            .cloned()
            .or_else(|| std::fs::read(&path).ok());
        match expected {
            Some(expected) if expected == actual => {
                state.matched += 1;
                Ok(())
            }
            Some(expected) if !state.update => {
                state.failed += 1;
                let detail = match (std::str::from_utf8(&expected), std::str::from_utf8(&actual)) {
                    (Ok(before), Ok(after)) => snapshot_diff(before, after, state.full_diff),
                    _ => binary_diff(&expected, &actual),
                };
                Err(format!(
                    "file snapshot differs: {}\n\n{detail}",
                    path.display()
                ))
            }
            None if state.ci => {
                state.failed += 1;
                Err(format!(
                    "no stored file snapshot: {}; --ci does not write them",
                    path.display()
                ))
            }
            Some(_) => {
                state.file_writes.insert(path, actual);
                state.updated += 1;
                Ok(())
            }
            None => {
                state.file_writes.insert(path, actual);
                state.written += 1;
                Ok(())
            }
        }
    })
}

fn binary_diff(expected: &[u8], actual: &[u8]) -> String {
    let offset = expected
        .iter()
        .zip(actual)
        .position(|(a, b)| a != b)
        .unwrap_or(expected.len().min(actual.len()));
    let start = offset.saturating_sub(8);
    let end = (offset + 8).min(expected.len().max(actual.len()));
    let window = |bytes: &[u8]| {
        (start..end)
            .map(|index| {
                bytes
                    .get(index)
                    .map_or("--".to_string(), |byte| format!("{byte:02x}"))
            })
            .collect::<Vec<_>>()
            .join(" ")
    };
    format!(
        "binary snapshot differs: stored {} bytes, received {} bytes; first differing byte {offset}\n  stored: {}\n  received: {}",
        expected.len(),
        actual.len(),
        window(expected),
        window(actual)
    )
}

fn snapshot_tally() -> Option<String> {
    SNAPSHOTS.with_borrow(|state| {
        let used = state.matched + state.failed + state.written + state.updated;
        (used > 0).then(|| {
            if state.update {
                format!(
                    "snapshots: {} updated, {} written, {} removed, {} unchanged",
                    state.updated, state.written, state.removed, state.matched
                )
            } else {
                format!(
                    "snapshots: {} matched, {} failed, {} written",
                    state.matched, state.failed, state.written
                )
            }
        })
    })
}

fn flush_snapshots() -> Result<(), String> {
    SNAPSHOTS.with_borrow_mut(|state| {
        // A skipped, exclusive, failed, or unfinished case means this run did
        // not observe the whole file. Keeping unused entries is conservative:
        // deleting an assertion because `.only` hid its test is never safe.
        let complete = CASES.with_borrow(|cases| {
            !cases.is_empty()
                && cases
                    .iter()
                    .all(|case| matches!(case.outcome, Some(Outcome::Passed)))
        });
        if state.update && complete {
            for snapshots in state.files.values_mut() {
                let before = snapshots.snapshots.len();
                snapshots
                    .snapshots
                    .retain(|key, _| snapshots.used.contains(key));
                let removed = before - snapshots.snapshots.len();
                if removed > 0 {
                    snapshots.dirty = true;
                    state.removed += removed;
                }
            }
        }
        for (file, snapshots) in &mut state.files {
            if !snapshots.dirty {
                continue;
            }
            let path = snapshot_path(file);
            let mut text = String::from("// esdev snapshot v1\n");
            for (key, body) in &snapshots.snapshots {
                text.push_str(&format!("\n=== {key} [value]\n"));
                for line in body.lines() {
                    text.push_str(if line.starts_with("===") { "\\" } else { "" });
                    text.push_str(line);
                    text.push('\n');
                }
            }
            let dir = path.parent().expect("snapshot has a parent");
            std::fs::create_dir_all(dir)
                .map_err(|err| format!("cannot create {}: {err}", dir.display()))?;
            let temp = path.with_extension("snap.tmp");
            std::fs::write(&temp, text)
                .map_err(|err| format!("cannot write {}: {err}", temp.display()))?;
            std::fs::rename(&temp, &path)
                .map_err(|err| format!("cannot replace {}: {err}", path.display()))?;
            snapshots.dirty = false;
        }
        for (path, bytes) in &state.file_writes {
            let dir = path.parent().expect("file snapshot has a parent");
            std::fs::create_dir_all(dir)
                .map_err(|err| format!("cannot create {}: {err}", dir.display()))?;
            let temp = path.with_extension("tmp");
            std::fs::write(&temp, bytes)
                .map_err(|err| format!("cannot write {}: {err}", temp.display()))?;
            std::fs::rename(&temp, path)
                .map_err(|err| format!("cannot replace {}: {err}", path.display()))?;
        }
        Ok(())
    })
}

/// The `runtime:test` extension.
pub struct TestExtension;

/// Starts a fresh test-run tally in this host thread.
///
/// An ordinary `esdev test` child exits after one file, but unisolated watch
/// mode constructs a fresh runtime for each pass in the same host process.
/// The runtime is new; this thread-local bookkeeping must be too.
pub fn reset() {
    CASES.with_borrow_mut(Vec::clear);
    configure_snapshots(None, false, false, false);
}

const MODULES: &[HostModule] = &[HostModule {
    specifier: "runtime:test",
    source: include_str!("test.js"),
}];

impl HostExtension for TestExtension {
    fn modules(&self) -> &[HostModule] {
        MODULES
    }

    fn ops(&self, _ctx: &ExtensionContext<'_>) -> Vec<OpDecl> {
        vec![
            // registered(name) -> id
            //
            // At registration rather than at the start, so a case that never
            // got to run is still in the report. No capability, and nothing to
            // gate: an assertion computes, and a tally is bookkeeping this
            // process keeps about itself. The same reasoning as
            // `runtime:hashing`.
            OpDecl::sync("test_registered", |args| {
                let name = args
                    .first()
                    .and_then(Value::as_str)
                    .unwrap_or("(unnamed)")
                    .to_string();
                let id = CASES.with_borrow_mut(|cases| {
                    cases.push(Case {
                        name,
                        file: SNAPSHOTS.with_borrow(|state| state.current_file.clone()),
                        started: false,
                        outcome: None,
                    });
                    cases.len() - 1
                });
                Ok(Value::Number(id as f64))
            }),
            // running(id) — the queue reached this case.
            OpDecl::sync("test_running", |args| {
                let id = args.first().and_then(Value::as_number).unwrap_or(-1.0);
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "the id came from `registered`, which handed out an index"
                )]
                let index = id as usize;
                CASES.with_borrow_mut(|cases| {
                    if let Some(case) = cases.get_mut(index) {
                        case.started = true;
                    }
                });
                Ok(Value::Undefined)
            }),
            // skipped(id, because) — the case will not run, and is in the
            // report saying so rather than missing from it.
            OpDecl::sync("test_skipped", |args| {
                let id = args.first().and_then(Value::as_number).unwrap_or(-1.0);
                let because = match args.get(1).and_then(Value::as_str) {
                    Some("only") => Skip::Only,
                    _ => Skip::Asked,
                };
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "the id came from `registered`, which handed out an index"
                )]
                let index = id as usize;
                CASES.with_borrow_mut(|cases| {
                    if let Some(case) = cases.get_mut(index) {
                        case.outcome = Some(Outcome::Skipped(because));
                    }
                });
                Ok(Value::Undefined)
            }),
            // finished(id, ok, detail)
            OpDecl::sync("test_finished", |args| {
                let id = args.first().and_then(Value::as_number).unwrap_or(-1.0);
                let passed = matches!(args.get(1), Some(Value::Bool(true)));
                let detail = args
                    .get(2)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "the id came from `registered`, which handed out an index"
                )]
                let index = id as usize;
                CASES.with_borrow_mut(|cases| {
                    if let Some(case) = cases.get_mut(index) {
                        case.outcome = Some(if passed {
                            Outcome::Passed
                        } else {
                            Outcome::Failed(detail)
                        });
                    }
                });
                Ok(Value::Undefined)
            }),
            OpDecl::sync("test_set_file", |args| {
                if let Some(file) = args.first().and_then(Value::as_str) {
                    set_snapshot_file(file);
                }
                Ok(Value::Undefined)
            }),
            OpDecl::sync("test_snapshot", |args| {
                let id = args.first().and_then(Value::as_number).unwrap_or(-1.0) as usize;
                let key = args
                    .get(1)
                    .and_then(Value::as_str)
                    .unwrap_or("(unnamed)")
                    .to_string();
                let actual = args
                    .get(2)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                match check_snapshot(id, key, actual) {
                    Ok(()) => Ok(Value::Undefined),
                    Err(message) => Ok(Value::String(message)),
                }
            }),
            OpDecl::sync("test_file_snapshot", |args| {
                let id = args.first().and_then(Value::as_number).unwrap_or(-1.0) as usize;
                let name = args.get(1).and_then(Value::as_str).unwrap_or_default();
                let actual = match args.get(2) {
                    Some(Value::String(text)) => text.as_bytes().to_vec(),
                    Some(Value::Bytes(bytes)) => bytes.clone(),
                    _ => {
                        return Ok(Value::String(
                            "toMatchFileSnapshot accepts a string or byte buffer".to_string(),
                        ));
                    }
                };
                match check_file_snapshot(id, name, actual) {
                    Ok(()) => Ok(Value::Undefined),
                    Err(message) => Ok(Value::String(message)),
                }
            }),
        ]
    }
}

/// Prints what the run's tests did, and returns the process's exit code.
///
/// Called after **every** `esdev` run, not only `esdev test`: a program that
/// imported `runtime:test` ran tests whatever the command line called it, and
/// one that did not has nothing to print. That is what makes
/// `esdev app.test.ts` work on its own, with the same output the runner gives.
pub fn finish() -> ExitCode {
    let code = report(None);
    if let Some(tally) = snapshot_tally() {
        println!("{tally}");
    }
    if let Err(err) = flush_snapshots() {
        eprintln!("error: {err}");
        return ExitCode::FAILURE;
    }
    code
}

/// The same tally, as one JSON object per line.
///
/// **For a machine, and shaped for one that reads a stream.** A CI job wants
/// each case as it lands rather than a document it can only parse once the run
/// is over — and a run that dies half way through then still leaves behind
/// everything that had happened, which a single trailing object would not.
///
/// `file` is the path the child was given, repeated on every line: the parent
/// runs a process per file and their output interleaves, so a line that does
/// not say which file it belongs to cannot be attributed to one.
pub fn finish_as_json(file: &str) -> ExitCode {
    let code = report(Some(file));
    if let Err(err) = flush_snapshots() {
        eprintln!("error: {err}");
        return ExitCode::FAILURE;
    }
    code
}

fn report(as_json: Option<&str>) -> ExitCode {
    let (passed, skipped, held, failures) = CASES.with_borrow(|cases| {
        let mut passed = 0usize;
        let mut skipped = 0usize;
        let mut held = 0usize;
        let mut failures: Vec<(String, String)> = Vec::new();
        for case in cases {
            match &case.outcome {
                Some(Outcome::Passed) => passed += 1,
                Some(Outcome::Skipped(Skip::Asked)) => skipped += 1,
                Some(Outcome::Skipped(Skip::Only)) => held += 1,
                Some(Outcome::Failed(detail)) => {
                    failures.push((case.name.clone(), detail.clone()));
                }
                // Registered, never settled. The old harness hung the program
                // here; a test that cannot finish is a failing test, and saying
                // so is the difference between a red run and a green one that
                // quietly ran fewer tests than it printed.
                None if case.started => failures.push((
                    case.name.clone(),
                    "the test never finished — it is waiting on something that never happened"
                        .to_string(),
                )),
                // Never even started: the queue did not reach it, because a
                // case ahead of it never finished. Named separately so the
                // report points at the test that is stuck rather than at the
                // twelve behind it.
                None => failures.push((
                    case.name.clone(),
                    "the test never started — a test before it never finished".to_string(),
                )),
            }
        }
        (passed, skipped, held, failures)
    });

    if passed == 0 && skipped == 0 && held == 0 && failures.is_empty() {
        return ExitCode::SUCCESS;
    }

    if let Some(file) = as_json {
        return json(file, passed, skipped + held, &failures);
    }

    for (name, detail) in &failures {
        println!("  FAIL {name}");
        for line in detail.lines() {
            println!("    {line}");
        }
    }
    // Named on its own line, because it is the one that is easy to leave in a
    // commit: the tally underneath it is otherwise a small green number, and a
    // suite that ran one of its two hundred tests looks exactly like a fast one.
    if held > 0 {
        println!(
            "  only: {held} other test{} did not run",
            if held == 1 { "" } else { "s" }
        );
    }
    let mut tally = format!("  {passed} passed, {} failed", failures.len());
    if skipped + held > 0 {
        tally.push_str(&format!(", {} skipped", skipped + held));
    }
    println!("{tally}");

    if failures.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// One object per case, then one for the file.
///
/// Written by hand rather than through a serializer: the only values that need
/// escaping are a test's name and an error's text, `serde_json` is already in
/// the graph for exactly that, and a schema this small is easier to read as the
/// lines it produces.
fn json(file: &str, passed: usize, skipped: usize, failures: &[(String, String)]) -> ExitCode {
    let string = |text: &str| serde_json::Value::String(text.to_string()).to_string();
    for (name, detail) in failures {
        println!(
            r#"{{"type":"case","file":{},"name":{},"status":"failed","detail":{}}}"#,
            string(file),
            string(name),
            string(detail)
        );
    }
    println!(
        r#"{{"type":"file","file":{},"passed":{passed},"failed":{},"skipped":{skipped}}}"#,
        string(file),
        failures.len()
    );
    if failures.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A run with no tests in it prints nothing and succeeds — `finish()` is
    /// called after every run, including the ones that are not tests at all.
    #[test]
    fn a_run_with_no_tests_is_silent() {
        CASES.with_borrow_mut(Vec::clear);
        assert!(
            matches!(finish(), code if format!("{code:?}") == format!("{:?}", ExitCode::SUCCESS))
        );
    }

    /// A case that was started and never settled fails the run. Nothing else
    /// notices it: the program reached quiescence perfectly happily.
    #[test]
    fn an_unfinished_case_is_a_failure() {
        CASES.with_borrow_mut(|cases| {
            cases.clear();
            cases.push(Case {
                name: "hangs".to_string(),
                file: None,
                started: true,
                outcome: None,
            });
        });
        let unfinished = CASES.with_borrow(|cases| cases.iter().all(|c| c.outcome.is_none()));
        assert!(unfinished);
        assert_eq!(
            format!("{:?}", finish()),
            format!("{:?}", ExitCode::FAILURE),
            "a case that never settled must fail the run"
        );
    }
}
