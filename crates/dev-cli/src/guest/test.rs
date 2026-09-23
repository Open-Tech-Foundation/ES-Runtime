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
    /// Its name did not match `--test-name-pattern`, or matched
    /// `--test-skip-pattern`. Counted apart for the same reason as `Only`.
    Filtered,
    /// `--bail`'s limit of failed tests was reached before it ran.
    Bailed,
}

/// What the command line asked of the tests in one file, read by
/// `runtime:test` as it loads. Set before the file runs, in the process that
/// runs it — or handed to the page, in a browser run.
#[derive(Clone, Default)]
pub struct RunOptions {
    /// Run only the tests whose full name matches this pattern.
    pub name_pattern: Option<String>,
    /// Skip the tests whose full name matches this pattern.
    pub skip_pattern: Option<String>,
    /// How many more tests may fail before the rest are not run.
    pub bail: Option<usize>,
    /// Shuffle the order tests run in, from this seed.
    pub seed: Option<u32>,
}

impl RunOptions {
    /// As `runtime:test` reads it.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "namePattern": self.name_pattern,
            "skipPattern": self.skip_pattern,
            "bail": self.bail,
            "seed": self.seed,
        })
    }
}

thread_local! {
    static RUN_OPTIONS: RefCell<RunOptions> = RefCell::new(RunOptions::default());
}

/// Sets what the next file run in this thread is asked to do.
pub fn configure_run(options: RunOptions) {
    RUN_OPTIONS.with_borrow_mut(|held| *held = options);
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
    prune: bool,
    current_file: Option<PathBuf>,
    files: BTreeMap<PathBuf, SnapshotFile>,
    file_writes: BTreeMap<PathBuf, Vec<u8>>,
    file_used: BTreeSet<PathBuf>,
    file_removals: Vec<PathBuf>,
    counts: Counts,
    written: usize,
    written_entries: Vec<String>,
    obsolete_entries: Vec<String>,
    obsolete_reason: Option<&'static str>,
    updated: usize,
    removed: usize,
    /// Inline snapshots to write into source files, by file: the line and
    /// column the matcher ran at, and the value.
    inline_writes: BTreeMap<PathBuf, Vec<crate::inline_snapshot::Write>>,
    /// Which positions already have a value to write, so a call reached twice
    /// with different values is refused rather than written with either.
    inline_seen: BTreeMap<(PathBuf, u32, u32), String>,
    /// Which case first took a snapshot under each test name, per file. Keys
    /// are made from the test's name, so a second case with the same name
    /// would read and write the first one's snapshots.
    owners: BTreeMap<(PathBuf, String), usize>,
}

/// Snapshots matched and failed, in the run and per case.
#[derive(Default)]
struct Counts {
    matched: usize,
    failed: usize,
    /// What each case's current attempt has matched and failed, so a retry
    /// can take back what an earlier attempt counted.
    by_case: BTreeMap<usize, (usize, usize)>,
}

#[derive(Default)]
struct SnapshotFile {
    snapshots: BTreeMap<String, SnapshotEntry>,
    used: BTreeSet<String>,
    dirty: bool,
}

struct SnapshotEntry {
    kind: String,
    body: String,
}

fn snapshot_path(test_file: &std::path::Path) -> PathBuf {
    let name = test_file.file_name().unwrap_or_default();
    test_file
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("__snapshots__")
        .join(format!("{}.snap", name.to_string_lossy()))
}

/// The manual update command belongs in an interactive failure only. CI has no
/// person to act on it, and its snapshots must remain committed state.
fn snapshot_accept_hint(ci: bool, file: &std::path::Path) -> String {
    if ci {
        String::new()
    } else {
        format!(
            "\naccept with: esdev test --update-snapshots --file={}",
            file.file_name()
                .unwrap_or(file.as_os_str())
                .to_string_lossy()
        )
    }
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
    let mut key: Option<(String, String)> = None;
    let mut body = Vec::new();
    for line in lines {
        if let Some(rest) = line.strip_prefix("=== ") {
            let (name, tagged_kind) = rest.rsplit_once(" [").ok_or_else(|| {
                format!("cannot read {}: malformed snapshot heading", path.display())
            })?;
            let kind = tagged_kind.strip_suffix(']').ok_or_else(|| {
                format!("cannot read {}: malformed snapshot heading", path.display())
            })?;
            if !matches!(kind, "value" | "error") {
                return Err(format!(
                    "cannot read {}: unknown snapshot kind {kind}",
                    path.display()
                ));
            }
            if let Some((previous, previous_kind)) =
                key.replace((name.to_string(), kind.to_string()))
            {
                if body.last().is_some_and(String::is_empty) {
                    body.pop();
                }
                file.snapshots.insert(
                    previous,
                    SnapshotEntry {
                        kind: previous_kind,
                        body: body.join("\n"),
                    },
                );
                body.clear();
            }
        } else if key.is_some() {
            body.push(line.strip_prefix("\\===").unwrap_or(line).to_string());
        }
    }
    if let Some((previous, kind)) = key {
        if body.last().is_some_and(String::is_empty) {
            body.pop();
        }
        file.snapshots.insert(
            previous,
            SnapshotEntry {
                kind,
                body: body.join("\n"),
            },
        );
    }
    Ok(file)
}

/// Configures snapshot ownership for this runtime before its extensions are
/// constructed. A normal child has one known file; unisolated mode sets it as
/// each module is imported.
pub fn configure_snapshots(
    file: Option<PathBuf>,
    update: bool,
    ci: bool,
    full_diff: bool,
    prune: bool,
) {
    SNAPSHOTS.with_borrow_mut(|state| {
        *state = SnapshotState {
            update,
            ci,
            full_diff,
            prune,
            current_file: file,
            files: BTreeMap::new(),
            file_writes: BTreeMap::new(),
            file_used: BTreeSet::new(),
            file_removals: Vec::new(),
            counts: Counts::default(),
            written: 0,
            written_entries: Vec::new(),
            obsolete_entries: Vec::new(),
            obsolete_reason: None,
            updated: 0,
            removed: 0,
            owners: BTreeMap::new(),
            inline_writes: BTreeMap::new(),
            inline_seen: BTreeMap::new(),
        };
    });
}

/// A test name as it appears in a snapshot key. The file format gives each
/// entry a one-line heading, so line breaks are written as `\n` and `\r`.
fn key_name(name: &str) -> String {
    name.replace('\r', "\\r").replace('\n', "\\n")
}

/// A case is being attempted again: what its previous attempt counted no
/// longer stands.
fn restart_case_snapshots(case_id: usize) {
    SNAPSHOTS.with_borrow_mut(|state| restart_in(state, case_id));
}

fn restart_in(state: &mut SnapshotState, case_id: usize) {
    let counts = &mut state.counts;
    if let Some((matched, failed)) = counts.by_case.remove(&case_id) {
        counts.matched -= matched;
        counts.failed -= failed;
    }
}

/// Counts a snapshot result against the whole run and against its case.
fn count(counts: &mut Counts, case_id: usize, passed: bool) {
    let entry = counts.by_case.entry(case_id).or_default();
    if passed {
        counts.matched += 1;
        entry.0 += 1;
    } else {
        counts.failed += 1;
        entry.1 += 1;
    }
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
    let before: Vec<&str> = expected.split('\n').collect();
    let after: Vec<&str> = actual.split('\n').collect();
    if !full && before.len() + after.len() > 200 {
        let mut out =
            String::from("--- snapshot\n+++ received\n@@ large diff (use --full-diff) @@\n");
        for line in before.iter().take(50) {
            out.push_str(&format!("- {}\n", visible_line(line)));
        }
        for line in after.iter().take(50) {
            out.push_str(&format!("+ {}\n", visible_line(line)));
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
    let mut rows: Vec<(char, String)> = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < before.len() || j < after.len() {
        if i < before.len() && j < after.len() && before[i] == after[j] {
            rows.push((' ', visible_line(before[i])));
            i += 1;
            j += 1;
        } else if i < before.len() && (j == after.len() || lcs[i + 1][j] >= lcs[i][j + 1]) {
            // A removal before the addition that replaces it, as a unified
            // diff reads.
            rows.push(('-', visible_line(before[i])));
            i += 1;
        } else {
            rows.push(('+', visible_line(after[j])));
            j += 1;
        }
    }
    let mut out = String::from("--- snapshot\n+++ received\n");
    let mut show = vec![full; rows.len()];
    if !full {
        for (index, (kind, _)) in rows.iter().enumerate() {
            if *kind != ' ' {
                let first = index.saturating_sub(3);
                let last = (index + 4).min(rows.len());
                show[first..last].fill(true);
            }
        }
    }
    let mut hidden = false;
    for ((kind, line), visible) in rows.iter().zip(show) {
        if !visible {
            hidden = true;
            continue;
        }
        if hidden {
            out.push_str("… unchanged lines omitted …\n");
            hidden = false;
        }
        out.push(*kind);
        out.push(' ');
        out.push_str(line);
        out.push('\n');
    }
    if expected.ends_with('\n') != actual.ends_with('\n') {
        out.push_str("\\ No newline at end of file\n");
    }
    out
}

fn visible_line(line: &str) -> String {
    let (line, cr) = match line.strip_suffix('\r') {
        Some(line) => (line, true),
        None => (line, false),
    };
    let trimmed = line.trim_end_matches([' ', '\t']);
    let suffix = &line[trimmed.len()..];
    let mut out = trimmed.replace('\t', "⇥");
    for ch in suffix.chars() {
        out.push(if ch == ' ' { '·' } else { '⇥' });
    }
    if cr {
        out.push('␍');
    }
    out
}

fn check_snapshot(case_id: usize, key: String, kind: String, actual: String) -> Result<(), String> {
    let (file, name) = CASES
        .with_borrow(|cases| {
            cases
                .get(case_id)
                .and_then(|case| case.file.clone().map(|file| (file, case.name.clone())))
        })
        .ok_or_else(|| "toMatchSnapshot is available when esdev runs a test file".to_string())?;
    SNAPSHOTS
        .with_borrow_mut(|state| check_snapshot_in(state, file, &name, case_id, &key, kind, actual))
}

/// Checks one value snapshot of the test `name` in `file` against the store.
fn check_snapshot_in(
    state: &mut SnapshotState,
    file: PathBuf,
    name: &str,
    case_id: usize,
    key: &str,
    kind: String,
    actual: String,
) -> Result<(), String> {
    let key = key_name(&format!("{name}: {key}"));
    {
        let owner = *state
            .owners
            .entry((file.clone(), name.to_string()))
            .or_insert(case_id);
        if owner != case_id {
            state.counts.failed += 1;
            return Err(format!(
                "another test in this file is also named {name:?}, and snapshots are \
                 stored by test name — rename one of them"
            ));
        }
        if !state.files.contains_key(&file) {
            let loaded = load_snapshots(&file)?;
            state.files.insert(file.clone(), loaded);
        }
        let snapshots = state.files.get_mut(&file).expect("inserted above");
        snapshots.used.insert(key.clone());
        match snapshots.snapshots.get(&key) {
            Some(expected) if expected.kind == kind && expected.body == actual => {
                count(&mut state.counts, case_id, true);
                Ok(())
            }
            Some(expected) if !state.update => {
                count(&mut state.counts, case_id, false);
                let accept = snapshot_accept_hint(state.ci, &file);
                Err(format!(
                    "snapshot changed — {key}\n{}\n\n{}{}",
                    snapshot_path(&file).display(),
                    snapshot_diff(&expected.body, &actual, state.full_diff),
                    accept
                ))
            }
            None if state.ci => {
                count(&mut state.counts, case_id, false);
                Err(format!(
                    "no stored snapshot: {key}; --ci does not write them"
                ))
            }
            Some(_) => {
                snapshots
                    .snapshots
                    .insert(key, SnapshotEntry { kind, body: actual });
                snapshots.dirty = true;
                state.updated += 1;
                Ok(())
            }
            None => {
                let written_entry = format!("{} › {key}", snapshot_path(&file).display());
                snapshots
                    .snapshots
                    .insert(key, SnapshotEntry { kind, body: actual });
                snapshots.dirty = true;
                state.written += 1;
                state.written_entries.push(written_entry);
                Ok(())
            }
        }
    }
}

fn check_file_snapshot(case_id: usize, name: &str, actual: Vec<u8>) -> Result<(), String> {
    let file = CASES
        .with_borrow(|cases| cases.get(case_id).and_then(|case| case.file.clone()))
        .ok_or_else(|| {
            "toMatchFileSnapshot is available when esdev runs a test file".to_string()
        })?;
    SNAPSHOTS.with_borrow_mut(|state| check_file_snapshot_in(state, &file, case_id, name, actual))
}

/// Checks one file snapshot, `name`, of the test file `file`.
fn check_file_snapshot_in(
    state: &mut SnapshotState,
    file: &std::path::Path,
    case_id: usize,
    name: &str,
    actual: Vec<u8>,
) -> Result<(), String> {
    let path = file_snapshot_path(file, name)?;
    {
        state.file_used.insert(path.clone());
        let expected = state
            .file_writes
            .get(&path)
            .cloned()
            .or_else(|| std::fs::read(&path).ok());
        match expected {
            Some(expected) if expected == actual => {
                count(&mut state.counts, case_id, true);
                Ok(())
            }
            Some(expected) if !state.update => {
                count(&mut state.counts, case_id, false);
                let detail = match (text_snapshot(&expected), text_snapshot(&actual)) {
                    (Some(before), Some(after)) => snapshot_diff(before, after, state.full_diff),
                    _ => binary_diff(&expected, &actual),
                };
                Err(format!(
                    "file snapshot differs: {}\n\n{detail}{}",
                    path.display(),
                    snapshot_accept_hint(state.ci, file)
                ))
            }
            None if state.ci => {
                count(&mut state.counts, case_id, false);
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
                let written_entry = path.display().to_string();
                state.file_writes.insert(path, actual);
                state.written += 1;
                state.written_entries.push(written_entry);
                Ok(())
            }
        }
    }
}

fn check_inline_snapshot(
    case_id: usize,
    stack: &str,
    actual: String,
    existing: Option<String>,
) -> Result<(), String> {
    let stack = es_runtime_cli_common::sourcemap::remap(stack);
    SNAPSHOTS.with_borrow_mut(|state| check_inline_in(state, case_id, &stack, actual, existing))
}

/// Checks an inline snapshot against the value written in the call, and when
/// there is none — or `--update-snapshots` asked — queues the value to be
/// written there. `stack` is the matcher's own, mapped back to the source, so
/// its first frame outside `runtime:test` is the call.
fn check_inline_in(
    state: &mut SnapshotState,
    case_id: usize,
    stack: &str,
    actual: String,
    existing: Option<String>,
) -> Result<(), String> {
    if existing.as_deref() == Some(actual.as_str()) {
        count(&mut state.counts, case_id, true);
        return Ok(());
    }
    let caller = caller(stack);
    let at = caller
        .as_ref()
        .map(|(path, line, column)| format!("{}:{line}:{column}", path.display()))
        .unwrap_or_default();
    match existing {
        Some(expected) if !state.update => {
            count(&mut state.counts, case_id, false);
            let accept = caller
                .as_ref()
                .map(|(path, _, _)| snapshot_accept_hint(state.ci, path))
                .unwrap_or_default();
            Err(format!(
                "inline snapshot changed\n{at}\n\n{}{accept}",
                snapshot_diff(&expected, &actual, state.full_diff)
            ))
        }
        None if state.ci => {
            count(&mut state.counts, case_id, false);
            Err(format!(
                "no inline snapshot; --ci does not write them\n{at}"
            ))
        }
        existing => {
            let Some((path, line, column)) = caller else {
                count(&mut state.counts, case_id, false);
                return Err(
                    "cannot tell where toMatchInlineSnapshot was called, so its snapshot cannot be written"
                        .to_string(),
                );
            };
            let position = (path.clone(), line, column);
            if let Some(queued) = state.inline_seen.get(&position) {
                if *queued == actual {
                    return Ok(());
                }
                count(&mut state.counts, case_id, false);
                return Err(format!(
                    "this inline snapshot ran more than once with different values — inline \
                     snapshots cannot be taken in a loop; use toMatchSnapshot\n{at}"
                ));
            }
            state.inline_seen.insert(position, actual.clone());
            if existing.is_some() {
                state.updated += 1;
            } else {
                state.written += 1;
                state.written_entries.push(at);
            }
            state
                .inline_writes
                .entry(path)
                .or_default()
                .push(crate::inline_snapshot::Write {
                    line,
                    column,
                    value: actual,
                });
            Ok(())
        }
    }
}

/// The first frame of `stack` in a file — not in `runtime:test` — as a path,
/// line and column.
fn caller(stack: &str) -> Option<(PathBuf, u32, u32)> {
    stack.lines().skip(1).find_map(|line| {
        let start = line.find("file://")?;
        let rest = &line[start..];
        let end = rest
            .find(|c: char| c == ')' || c.is_whitespace())
            .unwrap_or(rest.len());
        let (url, column) = rest[..end].rsplit_once(':')?;
        let (url, line) = url.rsplit_once(':')?;
        let path = url::Url::parse(url).ok()?.to_file_path().ok()?;
        Some((path, line.parse().ok()?, column.parse().ok()?))
    })
}

fn text_snapshot(bytes: &[u8]) -> Option<&str> {
    let text = std::str::from_utf8(bytes).ok()?;
    // UTF-8 control bytes other than whitespace make a file snapshot binary.
    // A valid UTF-8 decoding alone would render arbitrary byte buffers as
    // invisible glyphs and hide the byte-level diagnostic the caller needs.
    (!text
        .chars()
        .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\r' | '\t')))
    .then_some(text)
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

fn reconcile_snapshots() -> Result<(), String> {
    CASES.with_borrow(|cases| SNAPSHOTS.with_borrow_mut(|state| reconcile_in(state, cases)))
}

/// Decides which stored snapshots no test used, and — on a complete,
/// unfiltered `--update-snapshots` run — removes them.
fn reconcile_in(state: &mut SnapshotState, cases: &[Case]) -> Result<(), String> {
    let (complete, test_files, unjudged) = {
        (
            !cases.is_empty()
                && cases
                    .iter()
                    .all(|case| matches!(case.outcome, Some(Outcome::Passed))),
            cases
                .iter()
                .filter_map(|case| case.file.clone())
                .collect::<BTreeSet<_>>(),
            // A test that was skipped, left out by `.only`, or did not pass
            // may not have reached its snapshots, so they say nothing about
            // being obsolete: `name: ` is the prefix of every key it owns.
            cases
                .iter()
                .filter(|case| !matches!(case.outcome, Some(Outcome::Passed)))
                .filter_map(|case| {
                    case.file
                        .clone()
                        .map(|file| (file, format!("{}: ", key_name(&case.name))))
                })
                .collect::<Vec<_>>(),
        )
    };
    {
        // Preload every file's value store even when the current source no
        // longer calls a value matcher. Otherwise a deleted last matcher can
        // never make its old entry observable for reporting or pruning.
        for file in &test_files {
            if !state.files.contains_key(file) {
                state.files.insert(file.clone(), load_snapshots(file)?);
            }
        }
        let prune = state.update && state.prune && complete;
        state.obsolete_entries.clear();
        state.file_removals.clear();
        state.obsolete_reason = (!prune).then_some(if state.prune {
            "incomplete test run"
        } else {
            "filter active"
        });

        for (file, snapshots) in &mut state.files {
            let unused = snapshots
                .snapshots
                .keys()
                .filter(|key| !snapshots.used.contains(*key))
                .filter(|key| {
                    !unjudged
                        .iter()
                        .any(|(owner, prefix)| owner == file && key.starts_with(prefix.as_str()))
                })
                .cloned()
                .collect::<Vec<_>>();
            if prune {
                for key in unused {
                    snapshots.snapshots.remove(&key);
                    state.removed += 1;
                    snapshots.dirty = true;
                }
            } else {
                state.obsolete_entries.extend(
                    unused
                        .into_iter()
                        .map(|key| format!("{} › {key}", snapshot_path(file).display())),
                );
            }
        }

        for file in test_files {
            let Ok(dir) = file_snapshot_path(&file, ".probe")
                .map(|path| path.parent().unwrap().to_path_buf())
            else {
                continue;
            };
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_file()
                    || state.file_used.contains(&path)
                    || state.file_writes.contains_key(&path)
                {
                    continue;
                }
                if prune {
                    state.file_removals.push(path);
                    state.removed += 1;
                } else {
                    state.obsolete_entries.push(path.display().to_string());
                }
            }
        }
        Ok(())
    }
}

/// The snapshot lines a report ends with, or nothing when no snapshot was
/// used.
fn snapshot_report(state: &SnapshotState) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    if let Some((tally, written, obsolete, reason)) = snapshot_tally_in(state) {
        let _ = writeln!(out, "{tally}");
        if !written.is_empty() {
            let _ = writeln!(out, "written:");
            for entry in written {
                let _ = writeln!(out, "  {entry}");
            }
        }
        if !obsolete.is_empty() {
            let _ = writeln!(
                out,
                "obsolete — kept ({}):",
                reason.unwrap_or("not updating")
            );
            for entry in obsolete {
                let _ = writeln!(out, "  {entry}");
            }
        }
    }
    out
}

/// The summary text plus the entries that the reporter expands below it.
type SnapshotTally = (String, Vec<String>, Vec<String>, Option<&'static str>);

fn snapshot_tally_in(state: &SnapshotState) -> Option<SnapshotTally> {
    {
        let used = state.counts.matched + state.counts.failed + state.written + state.updated;
        (used > 0 || state.removed > 0 || !state.obsolete_entries.is_empty()).then(|| {
            let mut tally = if state.update {
                format!(
                    "snapshots: {} updated, {} written, {} removed, {} unchanged",
                    state.updated, state.written, state.removed, state.counts.matched
                )
            } else {
                format!(
                    "snapshots: {} matched, {} failed, {} written",
                    state.counts.matched, state.counts.failed, state.written
                )
            };
            if !state.obsolete_entries.is_empty() {
                tally.push_str(&format!(", {} obsolete", state.obsolete_entries.len()));
            }
            (
                tally,
                state.written_entries.clone(),
                state.obsolete_entries.clone(),
                state.obsolete_reason,
            )
        })
    }
}

fn flush_snapshots() -> Result<(), String> {
    SNAPSHOTS.with_borrow_mut(flush_in)
}

/// Writes every changed snapshot, and removes the obsolete ones a prune chose.
fn flush_in(state: &mut SnapshotState) -> Result<(), String> {
    {
        for (file, snapshots) in &mut state.files {
            if !snapshots.dirty {
                continue;
            }
            let path = snapshot_path(file);
            let mut text = String::from("// esdev snapshot v1\n");
            for (key, entry) in &snapshots.snapshots {
                text.push_str(&format!("\n=== {key} [{}]\n", entry.kind));
                for line in entry.body.lines() {
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
        for path in &state.file_removals {
            std::fs::remove_file(path)
                .map_err(|err| format!("cannot remove obsolete {}: {err}", path.display()))?;
        }
        for (path, writes) in std::mem::take(&mut state.inline_writes) {
            let source = std::fs::read_to_string(&path)
                .map_err(|err| format!("cannot read {}: {err}", path.display()))?;
            let written = crate::inline_snapshot::rewrite(&source, &path, &writes)?;
            let temp = path.with_extension("inline-snapshot.tmp");
            std::fs::write(&temp, written)
                .map_err(|err| format!("cannot write {}: {err}", temp.display()))?;
            std::fs::rename(&temp, &path)
                .map_err(|err| format!("cannot replace {}: {err}", path.display()))?;
        }
        Ok(())
    }
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
    configure_snapshots(None, false, false, false, false);
}

/// `runtime:test`'s source — also what a browser run bundles in its place,
/// so a file means the same thing in a page as in this runtime.
pub const SOURCE: &str = include_str!("test.js");

const MODULES: &[HostModule] = &[HostModule {
    specifier: "runtime:test",
    source: SOURCE,
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
                let file = SNAPSHOTS.with_borrow(|state| state.current_file.clone());
                let id = CASES.with_borrow_mut(|cases| register(cases, name, file));
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
                let again =
                    CASES.with_borrow(|cases| cases.get(index).is_some_and(|case| case.started));
                if again {
                    restart_case_snapshots(index);
                }
                CASES.with_borrow_mut(|cases| mark_running(cases, index));
                Ok(Value::Undefined)
            }),
            // skipped(id, because) — the case will not run, and is in the
            // report saying so rather than missing from it.
            OpDecl::sync("test_skipped", |args| {
                let id = args.first().and_then(Value::as_number).unwrap_or(-1.0);
                let because = match args.get(1).and_then(Value::as_str) {
                    Some("only") => Skip::Only,
                    Some("filter") => Skip::Filtered,
                    Some("bail") => Skip::Bailed,
                    _ => Skip::Asked,
                };
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "the id came from `registered`, which handed out an index"
                )]
                let index = id as usize;
                CASES.with_borrow_mut(|cases| mark_skipped(cases, index, because));
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
                // Frames in a file rewritten as it loaded — TypeScript, JSX —
                // name positions in the code that ran; put them back on the
                // lines that were written.
                let detail = es_runtime_cli_common::sourcemap::remap(&detail);
                CASES.with_borrow_mut(|cases| mark_finished(cases, index, passed, detail));
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
                let kind = args
                    .get(3)
                    .and_then(Value::as_str)
                    .unwrap_or("value")
                    .to_string();
                match check_snapshot(id, key, kind, actual) {
                    Ok(()) => Ok(Value::Undefined),
                    Err(message) => Ok(Value::String(message)),
                }
            }),
            // options() -> JSON — what the command line asked of this file.
            OpDecl::sync("test_options", |_| {
                Ok(Value::String(
                    RUN_OPTIONS.with_borrow(RunOptions::to_json).to_string(),
                ))
            }),
            // inline_snapshot(id, stack, actual, existing | null) -> message?
            OpDecl::sync("test_inline_snapshot", |args| {
                let id = args.first().and_then(Value::as_number).unwrap_or(-1.0) as usize;
                let stack = args.get(1).and_then(Value::as_str).unwrap_or_default();
                let actual = args
                    .get(2)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let existing = args.get(3).and_then(Value::as_str).map(str::to_string);
                match check_inline_snapshot(id, stack, actual, existing) {
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
    if let Err(err) = reconcile_snapshots() {
        eprintln!("error: {err}");
        return ExitCode::FAILURE;
    }
    print!("{}", SNAPSHOTS.with_borrow(snapshot_report));
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
    let (text, passed) = CASES.with_borrow(|cases| render(cases, as_json));
    print!("{text}");
    if passed {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// A new case, in registration order; its index is its id.
fn register(cases: &mut Vec<Case>, name: String, file: Option<PathBuf>) -> usize {
    cases.push(Case {
        name,
        file,
        started: false,
        outcome: None,
    });
    cases.len() - 1
}

fn mark_running(cases: &mut [Case], id: usize) {
    if let Some(case) = cases.get_mut(id) {
        case.started = true;
    }
}

fn mark_skipped(cases: &mut [Case], id: usize, because: Skip) {
    if let Some(case) = cases.get_mut(id) {
        case.outcome = Some(Outcome::Skipped(because));
    }
}

fn mark_finished(cases: &mut [Case], id: usize, passed: bool, detail: String) {
    if let Some(case) = cases.get_mut(id) {
        case.outcome = Some(if passed {
            Outcome::Passed
        } else {
            Outcome::Failed(detail)
        });
    }
}

/// The report for a set of cases — what a person reads, or the JSON lines
/// for `file` — and whether it passed. Rendered rather than printed, so a run
/// holding several files at once can print each one's whole.
fn render(cases: &[Case], as_json: Option<&str>) -> (String, bool) {
    use std::fmt::Write as _;
    let mut out = String::new();
    let (passed, skipped, held, filtered, bailed, failures) = {
        let mut passed = 0usize;
        let mut skipped = 0usize;
        let mut held = 0usize;
        let mut filtered = 0usize;
        let mut bailed = 0usize;
        let mut failures: Vec<(String, String)> = Vec::new();
        for case in cases {
            match &case.outcome {
                Some(Outcome::Passed) => passed += 1,
                Some(Outcome::Skipped(Skip::Asked)) => skipped += 1,
                Some(Outcome::Skipped(Skip::Only)) => held += 1,
                Some(Outcome::Skipped(Skip::Filtered)) => filtered += 1,
                Some(Outcome::Skipped(Skip::Bailed)) => bailed += 1,
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
        (passed, skipped + bailed, held, filtered, bailed, failures)
    };

    if passed == 0 && skipped == 0 && held == 0 && filtered == 0 && failures.is_empty() {
        return (out, true);
    }

    if let Some(file) = as_json {
        json(&mut out, file, passed, skipped + held + filtered, &failures);
        return (out, failures.is_empty());
    }

    for (name, detail) in &failures {
        let _ = writeln!(out, "  FAIL {name}");
        for line in detail.lines() {
            let _ = writeln!(out, "    {line}");
        }
    }
    // Named on its own line, because it is the one that is easy to leave in a
    // commit: the tally underneath it is otherwise a small green number, and a
    // suite that ran one of its two hundred tests looks exactly like a fast one.
    if held > 0 {
        let _ = writeln!(
            out,
            "  only: {held} other test{} did not run",
            if held == 1 { "" } else { "s" }
        );
    }
    if filtered > 0 {
        let _ = writeln!(
            out,
            "  filter: {filtered} other test{} did not match",
            if filtered == 1 { "" } else { "s" }
        );
    }
    if bailed > 0 {
        let _ = writeln!(
            out,
            "  bail: {bailed} test{} did not run after the failure limit",
            if bailed == 1 { "" } else { "s" }
        );
    }
    let mut tally = format!("  {passed} passed, {} failed", failures.len());
    if skipped + held + filtered > 0 {
        tally.push_str(&format!(", {} skipped", skipped + held + filtered));
    }
    let _ = writeln!(out, "{tally}");
    (out, failures.is_empty())
}

/// One object per case, then one for the file.
///
/// Written by hand rather than through a serializer: the only values that need
/// escaping are a test's name and an error's text, `serde_json` is already in
/// the graph for exactly that, and a schema this small is easier to read as the
/// lines it produces.
fn json(
    out: &mut String,
    file: &str,
    passed: usize,
    skipped: usize,
    failures: &[(String, String)],
) {
    use std::fmt::Write as _;
    let string = |text: &str| serde_json::Value::String(text.to_string()).to_string();
    for (name, detail) in failures {
        let _ = writeln!(
            out,
            r#"{{"type":"case","file":{},"name":{},"status":"failed","detail":{}}}"#,
            string(file),
            string(name),
            string(detail)
        );
    }
    let _ = writeln!(
        out,
        r#"{{"type":"file","file":{},"passed":{passed},"failed":{},"skipped":{skipped}}}"#,
        string(file),
        failures.len()
    );
}

/// How many cases passed and failed — a case that never settled counts as
/// failed, as the report counts it.
fn counts(cases: &[Case]) -> (usize, usize) {
    cases
        .iter()
        .fold((0, 0), |(passed, failed), case| match &case.outcome {
            Some(Outcome::Passed) => (passed + 1, failed),
            Some(Outcome::Skipped(_)) => (passed, failed),
            Some(Outcome::Failed(_)) | None => (passed, failed + 1),
        })
}

/// Writes this process's pass and fail counts for the parent that ran it,
/// which adds them up — what `--bail` counts across files.
pub fn write_summary(path: &std::path::Path) {
    let (passed, failed) = CASES.with_borrow(|cases| counts(cases));
    let _ = std::fs::write(
        path,
        serde_json::json!({ "passed": passed, "failed": failed }).to_string(),
    );
}

/// The failed count a child wrote with [`write_summary`], or `None` if it
/// wrote none — it died before it could.
pub fn read_summary(path: &std::path::Path) -> Option<usize> {
    let text = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value
        .get("failed")?
        .as_u64()
        .and_then(|n| usize::try_from(n).ok())
}

/// A tally kept outside the host thread's own — for a run whose tests execute
/// somewhere else and report back, as a file in a browser does. The same
/// bookkeeping and the same report as a file run in this runtime, because
/// they are the same functions.
#[derive(Default)]
pub struct Tally {
    cases: Vec<Case>,
    /// The test file every case belongs to, which is what snapshots are
    /// stored beside.
    file: Option<PathBuf>,
}

impl Tally {
    /// A tally for the cases of one test file.
    pub fn for_file(file: PathBuf) -> Tally {
        Tally {
            cases: Vec::new(),
            file: Some(file),
        }
    }

    /// `registered(name)`; the id is the index, as it is for the op.
    pub fn register(&mut self, name: String) -> usize {
        register(&mut self.cases, name, self.file.clone())
    }

    /// The name a case was registered under.
    pub fn name(&self, id: usize) -> Option<&str> {
        self.cases.get(id).map(|case| case.name.as_str())
    }

    /// `running(id)`.
    pub fn running(&mut self, id: usize) {
        mark_running(&mut self.cases, id);
    }

    /// `skipped(id, because)`, where `because` is `"only"` or anything else.
    pub fn skipped(&mut self, id: usize, because: &str) {
        let because = match because {
            "only" => Skip::Only,
            "filter" => Skip::Filtered,
            "bail" => Skip::Bailed,
            _ => Skip::Asked,
        };
        mark_skipped(&mut self.cases, id, because);
    }

    /// `finished(id, ok, detail)`.
    pub fn finished(&mut self, id: usize, passed: bool, detail: String) {
        mark_finished(&mut self.cases, id, passed, detail);
    }

    /// How many cases failed, those that never settled included.
    pub fn failed(&self) -> usize {
        counts(&self.cases).1
    }

    /// Whether every case has an outcome.
    pub fn settled(&self) -> bool {
        self.cases.iter().all(|case| case.outcome.is_some())
    }

    /// The report, and whether the file passed.
    pub fn render(&self, as_json: Option<&str>) -> (String, bool) {
        render(&self.cases, as_json)
    }
}

/// One test file's snapshots, kept for a run whose tests execute elsewhere —
/// a browser page — and report each snapshot back. The same store, checks and
/// report a file run in this runtime uses, over state that belongs to this
/// file rather than to the process.
pub struct FileSnapshots {
    state: SnapshotState,
    file: PathBuf,
}

impl FileSnapshots {
    /// `update`, `ci`, `full_diff` and `prune` mean what the flags do.
    pub fn new(
        file: PathBuf,
        update: bool,
        ci: bool,
        full_diff: bool,
        prune: bool,
    ) -> FileSnapshots {
        let state = SnapshotState {
            update,
            ci,
            full_diff,
            prune,
            current_file: Some(file.clone()),
            ..SnapshotState::default()
        };
        FileSnapshots { state, file }
    }

    /// Every stored value snapshot as `(key, kind, body)`, with keys exactly
    /// as a check builds them.
    pub fn stored(&mut self) -> Result<Vec<(String, String, String)>, String> {
        if !self.state.files.contains_key(&self.file) {
            let loaded = load_snapshots(&self.file)?;
            self.state.files.insert(self.file.clone(), loaded);
        }
        Ok(self.state.files[&self.file]
            .snapshots
            .iter()
            .map(|(key, entry)| (key.clone(), entry.kind.clone(), entry.body.clone()))
            .collect())
    }

    /// Every stored file snapshot, by name, base64-encoded.
    pub fn stored_files(&self) -> Vec<(String, String)> {
        use base64::Engine as _;
        let Some(dir) = file_snapshot_path(&self.file, ".probe")
            .ok()
            .and_then(|path| path.parent().map(std::path::Path::to_path_buf))
        else {
            return Vec::new();
        };
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter(|entry| entry.path().is_file())
            .filter_map(|entry| {
                let bytes = std::fs::read(entry.path()).ok()?;
                let name = entry.file_name().into_string().ok()?;
                Some((
                    name,
                    base64::engine::general_purpose::STANDARD.encode(bytes),
                ))
            })
            .collect()
    }

    /// A value snapshot the page took, for the test `name`.
    pub fn check(
        &mut self,
        case_id: usize,
        name: &str,
        key: &str,
        kind: String,
        actual: String,
    ) -> Result<(), String> {
        check_snapshot_in(
            &mut self.state,
            self.file.clone(),
            name,
            case_id,
            key,
            kind,
            actual,
        )
    }

    /// A file snapshot the page took, its bytes base64-encoded.
    pub fn check_file(&mut self, case_id: usize, name: &str, actual: &str) -> Result<(), String> {
        use base64::Engine as _;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(actual)
            .map_err(|_| "the page sent a file snapshot that is not base64".to_string())?;
        check_file_snapshot_in(&mut self.state, &self.file, case_id, name, bytes)
    }

    /// An inline snapshot the page took. `stack` is already mapped back to
    /// the source.
    pub fn check_inline(
        &mut self,
        case_id: usize,
        stack: &str,
        actual: String,
        existing: Option<String>,
    ) -> Result<(), String> {
        check_inline_in(&mut self.state, case_id, stack, actual, existing)
    }

    /// A case is being attempted again.
    pub fn restart(&mut self, case_id: usize) {
        restart_in(&mut self.state, case_id);
    }

    /// Where a file snapshot called `name` is stored, as a report names it,
    /// or why `name` is not a file snapshot name.
    pub fn file_path(&self, name: &str) -> Result<String, String> {
        file_snapshot_path(&self.file, name).map(|path| path.display().to_string())
    }

    /// Settles the file's snapshots once its tests are done: decides what is
    /// obsolete (pruning it on a complete `--update-snapshots` run), writes what
    /// changed, and returns the lines its report ends with. `summary: false` is
    /// the JSON reporter's form, which writes and says nothing.
    pub fn finish(&mut self, tally: &Tally, summary: bool) -> Result<String, String> {
        if !summary {
            flush_in(&mut self.state)?;
            return Ok(String::new());
        }
        reconcile_in(&mut self.state, &tally.cases)?;
        let report = snapshot_report(&self.state);
        flush_in(&mut self.state)?;
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A changed line is one removal and one addition, with its unchanged
    /// neighbours shown once each as context.
    #[test]
    fn a_snapshot_diff_shows_only_what_changed() {
        let before = "{\n  \"user\": \"ada\",\n  \"ids\": [1, 2],\n}";
        let after = "{\n  \"user\": \"bob\",\n  \"ids\": [1, 2],\n}";
        assert_eq!(
            snapshot_diff(before, after, false),
            "--- snapshot\n+++ received\n  {\n-   \"user\": \"ada\",\n+   \"user\": \"bob\",\n    \"ids\": [1, 2],\n  }\n"
        );
    }

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
