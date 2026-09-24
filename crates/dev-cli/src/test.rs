//! `esdev test` — find the test files, run each in its own process, report.
//!
//! **One process per file.** A test suite is the place where isolation matters
//! most: a file that wedges, exhausts its heap, or calls `process.exit()` must
//! not decide the fate of the others, and a global left behind by one file must
//! not be visible to the next. A child process gives all of that for free, and
//! the prelude snapshot makes starting one cheap. It is the same reasoning
//! `--watch` uses, and the same mechanism — `esdev` re-executing itself.
//!
//! **The test file is the entry, and nothing is added to it.** Module
//! resolution is jailed to the project root detected from the entry's own
//! directory (D25), so a generated driver living in a temp directory could not
//! import a test file in the project at all — the file being run has to be the
//! file the developer wrote.
//!
//! It once had a harness *prepended* to it: five globals — `test`, `assert`,
//! `assertEquals`, `assertThrows`, `assertRejects` — folded onto a single
//! physical line so that the file's line 1 stayed line 1, plus an epilogue
//! appended to await and report. That worked, and three things were wrong with
//! it, all of which an import fixes:
//!
//! * **Ambient globals are what this runtime does not do.** Every other piece
//!   of host functionality is a `runtime:` module, imported by name. A test file
//!   was the one place a program was handed names it never asked for.
//! * **Only the entry got them**, since only the entry was wrapped — so a
//!   shared `test-helpers.ts` beside it could not call `assertEquals`, which is
//!   exactly where a suite most wants to share code.
//! * **They had no types**, because there was nowhere to declare them. A `.ts`
//!   test file referenced five undeclared names, and `tsc --noEmit` failed on a
//!   suite that ran perfectly.
//!
//! So the API moved into [`runtime:test`](crate::guest::test), the score moved
//! into the host, and what runs now is byte for byte the file on disk. This
//! module is what is left: discovery, and a child process per file.
//!
//! The API itself is unchanged, and is the one this repository's own
//! conformance suite uses — a developer reading the runtime's tests and writing
//! their own should not have to learn two vocabularies.

use std::path::{Path, PathBuf};

use crate::config::TestIsolation;

/// What `esdev test` was asked to do.
pub struct TestConfig {
    /// Install the esdev-only DOM realm before the test module evaluates.
    pub dom: bool,
    /// Run the files in a real browser over WebDriver BiDi instead, from
    /// `--browser` or the project's `test.browser`.
    pub browser: Option<crate::browser::Choice>,
    /// Show the browser's window rather than running it headless.
    pub headed: bool,
    /// Run only the tests whose full name matches, from `-t`.
    pub name_pattern: Option<String>,
    /// Skip the tests whose full name matches.
    pub skip_pattern: Option<String>,
    /// Stop after this many tests have failed, from `--bail`.
    pub bail: Option<usize>,
    /// How many more times every test runs, from `--repeats`.
    pub repeats: Option<u32>,
    /// Name the tests instead of running them, from `--list`.
    pub list: bool,
    /// Where a machine reporter writes, from `--reporter-outfile`. With it, the
    /// terminal keeps the report a person reads.
    pub reporter_outfile: Option<PathBuf>,
    /// Internal parent-to-child: print no report; the parent writes it.
    pub quiet: bool,
    /// Run only this part of the discovered files, from `--shard`.
    pub shard: Option<Shard>,
    /// Run only the test files these changes reach, from `--changed` or
    /// `--related`.
    pub affected_by: Option<AffectedBy>,
    /// Shuffle the run's order, from `--randomize`.
    pub randomize: bool,
    /// The seed the order is shuffled from, from `--seed` or chosen for a
    /// `--randomize` run. Passed to every file, so the whole run can be
    /// repeated.
    pub seed: Option<u32>,
    /// Internal parent-to-child: where a child writes its pass and fail
    /// counts, for the parent to add up.
    pub summary: Option<PathBuf>,
    /// How JSX in a test file compiles, from the project's `jsx` section. Read
    /// by the parent and by every `--file` child, so a test means the same
    /// thing however it was started.
    pub jsx: crate::transform::JsxSettings,
    /// Run exactly this file, harness installed. This is what the parent
    /// invokes for each child, and it is a supported way to run one file
    /// directly.
    pub file: Option<String>,
    /// Substring filters on the discovered paths; empty means all of them.
    pub filters: Vec<String>,
    /// How many files may run at once, from `--jobs`. `None` is [`jobs`].
    pub jobs: Option<usize>,
    /// The process boundary between files. `None` keeps the default.
    pub isolation: Option<TestIsolation>,
    /// Keep running, re-running the files when a source file changes.
    pub watch: bool,
    /// Modules imported before the file under test, from `--setup` or the
    /// project's `test.setup`. Absolute by the time they get here.
    pub setup: Vec<String>,
    /// Modules run once before the files and torn down after, as file URLs,
    /// from `--global-setup` or the project's `test.globalSetup`.
    pub global_setup: Vec<String>,
    /// Internal parent-to-child: the file holding what global setup provided,
    /// for `inject`. Its presence also says the parent ran global setup.
    pub provided: Option<PathBuf>,
    /// `--coverage`, and the project's `test.coverage` settings.
    pub coverage: Option<crate::coverage::Settings>,
    /// Whether `--coverage` itself was given, rather than `enabled` in the
    /// project: a run that cannot collect it refuses the flag, and quietly goes
    /// without what the project asks for every run.
    pub coverage_flag: bool,
    /// Internal parent-to-child: collect coverage, and write it here.
    pub coverage_out: Option<PathBuf>,
    /// Where each child of a coverage run writes, set by the parent.
    pub coverage_dir: Option<PathBuf>,
    /// `--inspect[=<addr>]` / `--inspect-brk[=<addr>]`: each file serves a
    /// debugger in turn, one file at a time.
    pub inspect: Option<crate::inspect::InspectConfig>,
    /// Internal: this process is the global setup, and writes what it provides
    /// here once its `setup` functions have run.
    pub global_setup_out: Option<PathBuf>,
    /// How long one file may take before it is stopped and failed.
    pub timeout: Option<u64>,
    /// `"json"` for one object per line, or `None` for what a person reads.
    pub reporter: Option<String>,
    /// Rewrite snapshots whose value changed, and create snapshots that do not
    /// exist yet. This is deliberately a command action, not project config:
    /// committing a config that rewrites assertions would be a foot-gun.
    pub update_snapshots: bool,
    /// Refuse every implicit snapshot write. CI is an assertion of committed
    /// state, never a place that can create it.
    pub ci: bool,
    /// Print every changed snapshot line instead of the review-sized default.
    pub full_diff: bool,
    /// Internal parent-to-child signal: only a complete, unfiltered discovery
    /// pass may prune obsolete entries.
    pub snapshot_prune: bool,
    /// Permission flags shaping each test file's run (`--deny-all`,
    /// `--allow-read=…`): a rehearsal of the production grant, so a path the
    /// suite covers meets its deployment's capabilities before deployment.
    /// Forwarded to every child, which re-parses them as its own run; the
    /// parent keeps its full grant for discovery and reporting. Flags only,
    /// never an `esdev.json` key: a rehearsal decides a single run.
    pub permission_args: Vec<String>,
}

impl TestConfig {
    /// Whether the terminal shows the report a person reads: the default, or a
    /// machine reporter that writes to a file.
    pub fn terminal_human(&self) -> bool {
        matches!(self.reporter.as_deref(), None | Some("human")) || self.reporter_outfile.is_some()
    }

    /// What each file's `runtime:test` is told.
    pub fn run_options(&self) -> crate::guest::test::RunOptions {
        crate::guest::test::RunOptions {
            name_pattern: self.name_pattern.clone(),
            skip_pattern: self.skip_pattern.clone(),
            bail: self.bail,
            seed: self.seed,
            repeats: self.repeats,
            list: self.list,
        }
    }
}

/// Puts the files in the order `seed` shuffles them into — the same
/// generator `runtime:test` shuffles tests with, so a seed names one order of
/// the whole run.
pub fn shuffle(files: &mut [PathBuf], seed: Option<u32>) {
    let Some(seed) = seed else {
        return;
    };
    let mut state = seed;
    let mut next = move || {
        state = state.wrapping_add(0x6d2b_79f5);
        let mut t = (state ^ (state >> 15)).wrapping_mul(1 | state);
        t = (t.wrapping_add((t ^ (t >> 7)).wrapping_mul(61 | t))) ^ t;
        f64::from(t ^ (t >> 14)) / 4_294_967_296.0
    };
    for i in (1..files.len()).rev() {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a fraction below one of a small length"
        )]
        let j = (next() * (i + 1) as f64) as usize;
        files.swap(i, j);
    }
}

/// What selects the test files a change reaches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AffectedBy {
    /// `--changed[=<since>]`: what git says changed — uncommitted, or since a
    /// commit or branch.
    Changed(Option<String>),
    /// `--related <file>…`: these source files.
    Related(Vec<String>),
}

/// One part of a suite split across machines, from `--shard=<index>/<count>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shard {
    /// 1-based.
    pub index: usize,
    pub count: usize,
}

impl Shard {
    /// `<index>/<count>`, both whole numbers from 1, the index at most the
    /// count.
    pub fn parse(text: &str) -> Result<Self, String> {
        let malformed = || {
            format!(
                "--shard={text} is not a shard.\n\n\
                 Write <index>/<count>, counting from 1: --shard=1/3 runs the first of three."
            )
        };
        let (index, count) = text.split_once('/').ok_or_else(malformed)?;
        let index: usize = index.parse().map_err(|_| malformed())?;
        let count: usize = count.parse().map_err(|_| malformed())?;
        if index == 0 || count == 0 || index > count {
            return Err(malformed());
        }
        Ok(Self { index, count })
    }
}

impl std::fmt::Display for Shard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.index, self.count)
    }
}

/// Keeps this shard's files: all of them ordered by the SHA-1 of their path
/// inside the project, then cut into `count` runs of as near equal length as
/// they divide into, the first ones a file longer. As Jest and Vitest shard: a
/// hash rather than the path spreads one directory over every shard, and the
/// order does not depend on anything but the paths.
pub fn shard(files: &mut Vec<PathBuf>, root: &Path, shard: Shard) {
    use sha1::{Digest, Sha1};
    let mut keyed: Vec<_> = files
        .drain(..)
        .map(|path| {
            let inside = path.strip_prefix(root).unwrap_or(&path);
            let relative = inside
                .components()
                .map(|part| part.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            (Sha1::digest(relative.as_bytes()), path)
        })
        .collect();
    keyed.sort_by_key(|(hash, _)| *hash);
    let start = shard_start(keyed.len(), shard.count, shard.index - 1);
    let end = shard_start(keyed.len(), shard.count, shard.index);
    files.extend(keyed.drain(start..end).map(|(_, path)| path));
}

/// Where shard `index` (0-based) begins among `total` files.
fn shard_start(total: usize, count: usize, index: usize) -> usize {
    let (size, longer) = (total / count, total % count);
    index * size + index.min(longer)
}

/// A seed for a `--randomize` run that did not name one.
pub fn fresh_seed() -> u32 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.subsec_nanos());
    nanos ^ std::process::id().rotate_left(16)
}

/// How many test files run at once when `--jobs` did not say.
///
/// **A file is a process, and a process is an isolate.** That is what makes the
/// runner robust — a test that wedges or exhausts its heap takes only itself
/// down — and it is also what makes it expensive: every job holds a V8 heap, so
/// the useful number is bounded by memory as well as by cores. The cap is the
/// conservative half of that trade; a machine with more of both says so with
/// `--jobs`.
pub fn jobs() -> usize {
    std::thread::available_parallelism()
        .map(std::num::NonZero::get)
        .unwrap_or(1)
        .min(8)
}

/// Suffixes that make a file a test.
///
/// `.spec.` as well as `.test.`, because both conventions are everywhere and a
/// runner that knows only one silently runs no tests in half the projects it is
/// pointed at — which looks exactly like a suite that passes.
const TEST_SUFFIXES: &[&str] = &[
    ".test.js",
    ".test.mjs",
    ".test.ts",
    ".test.tsx",
    ".test.jsx",
    ".test.mts",
    ".spec.js",
    ".spec.mjs",
    ".spec.ts",
    ".spec.tsx",
    ".spec.jsx",
    ".spec.mts",
];

/// Directories discovery never descends into.
const SKIP_DIRS: &[&str] = &["node_modules", ".git", "dist", "target", ".cache"];

/// What the "no test files found" message claims was looked for.
///
/// Derived from [`TEST_SUFFIXES`] rather than written out, so the two cannot
/// drift apart again: `*.test.js/.mjs/.ts/.tsx/.jsx/.mts and
/// *.spec.js/.mjs/.ts/.tsx/.jsx/.mts`, whatever the list becomes.
pub(crate) fn sought_description() -> String {
    let mut stems: Vec<&str> = Vec::new();
    let mut exts: Vec<Vec<&str>> = Vec::new();
    for suffix in TEST_SUFFIXES {
        // ".test.js" is the stem ".test" plus the extension "js".
        let rest = &suffix[1..];
        let dot = rest.find('.').expect("a test suffix names an extension");
        let (stem, ext) = (&suffix[..dot + 1], &rest[dot + 1..]);
        match stems.iter().position(|s| *s == stem) {
            Some(i) => exts[i].push(ext),
            None => {
                stems.push(stem);
                exts.push(vec![ext]);
            }
        }
    }
    stems
        .iter()
        .zip(exts.iter())
        .map(|(stem, exts)| format!("*.{}.{}", &stem[1..], exts.join("/.")))
        .collect::<Vec<_>>()
        .join(" and ")
}

/// Runs each file in its own process, up to `jobs` at a time, and reports how
/// many failed.
///
/// **Output is buffered per file whenever more than one runs at a time**, and
/// printed whole when that file finishes. Two suites writing to one terminal
/// interleave line by line, which turns a failure's message and the assertion
/// above it into two things a reader has to reassemble. With `--jobs=1` nothing
/// is buffered and the child writes straight through — which is what you want
/// from the run where you are watching a test hang.
pub async fn run_all(
    exe: &Path,
    root: &Path,
    files: &[PathBuf],
    jobs: usize,
    config: &TestConfig,
) -> usize {
    use futures_util::StreamExt;

    // What the parent hands down. A child runs one file and must run it the way
    // the parent was asked to, or the run reports something nobody configured.
    let flags: Vec<String> = config
        .dom
        .then(|| "--dom".to_string())
        .into_iter()
        .chain(
            config
                .setup
                .iter()
                .map(|module| format!("--setup={module}"))
                .chain((!config.terminal_human()).then(|| "--_quiet".to_string()))
                .chain(
                    config
                        .update_snapshots
                        .then(|| "--update-snapshots".to_string()),
                )
                .chain(config.ci.then(|| "--ci".to_string()))
                .chain(config.full_diff.then(|| "--full-diff".to_string()))
                .chain(
                    config
                        .name_pattern
                        .iter()
                        .map(|p| format!("--test-name-pattern={p}")),
                )
                .chain(
                    config
                        .provided
                        .iter()
                        .map(|path| format!("--_provided={}", path.display())),
                )
                .chain(config.inspect.iter().map(|inspect| {
                    let flag = if inspect.wait {
                        "--inspect-brk"
                    } else {
                        "--inspect"
                    };
                    format!("{flag}={}", inspect.address)
                }))
                .chain(config.seed.iter().map(|seed| format!("--seed={seed}")))
                .chain(config.repeats.iter().map(|n| format!("--repeats={n}")))
                .chain(config.list.then(|| "--list".to_string()))
                .chain(
                    config
                        .skip_pattern
                        .iter()
                        .map(|p| format!("--test-skip-pattern={p}")),
                )
                .chain(config.permission_args.iter().cloned())
                .chain(std::iter::once(format!(
                    "--_snapshot-prune={}",
                    u8::from(config.snapshot_prune)
                ))),
        )
        .collect();

    let named = |file: &Path| {
        file.strip_prefix(root)
            .unwrap_or(file)
            .display()
            .to_string()
    };
    // With a machine reporter writing to stdout, the report is the only thing
    // there: a filename above it, or what a test printed, would be something a
    // parser has to step over. What the tests print goes to stderr instead.
    let quiet = !config.terminal_human();
    let mut reporter = Reporter::new(config);

    // `--bail`: tests failed so far, added up from each child's results. A file
    // is not started once the limit is reached, and each one started is told
    // how many more may fail.
    let failed_tests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let not_run = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let bail = config.bail;
    let command = |file: &Path, index: usize| -> Option<(tokio::process::Command, PathBuf)> {
        let failed = failed_tests.load(std::sync::atomic::Ordering::SeqCst);
        let remaining = match bail {
            Some(limit) if failed >= limit => {
                not_run.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                return None;
            }
            Some(limit) => Some(limit - failed),
            None => None,
        };
        let mut child = tokio::process::Command::new(exe);
        child
            .arg("test")
            .arg(format!("--file={}", file.display()))
            .args(&flags);
        if let Some(remaining) = remaining {
            child.arg(format!("--bail={remaining}"));
        }
        // Every child reports its results to the parent in a file: what a
        // machine reporter is written from, and what `--bail` counts.
        let summary = std::env::temp_dir().join(format!(
            "esdev-test-summary-{}-{index}.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&summary);
        child.arg(format!("--_summary={}", summary.display()));
        if let Some(dir) = &config.coverage_dir {
            child.arg(format!(
                "--_coverage={}",
                dir.join(format!("{index}.json")).display()
            ));
        }
        Some((child, summary))
    };
    // A file's results, from its summary — or, when it wrote none because it
    // timed out or died, one failure saying so.
    let take_result = |summary: PathBuf, file: &Path, why: Option<String>| {
        let written = crate::guest::test::read_result(&summary);
        let _ = std::fs::remove_file(&summary);
        let mut result = written.unwrap_or_else(|| {
            crate::report::FileResult::broken(
                file.display().to_string(),
                String::new(),
                why.as_deref()
                    .unwrap_or("the file ended before it reported its results"),
            )
        });
        result.name = named(file);
        if let Some(why) = why.filter(|_| result.failed() == 0) {
            result = crate::report::FileResult::broken(result.file, result.name, &why);
        }
        failed_tests.fetch_add(result.failed(), std::sync::atomic::Ordering::SeqCst);
        result
    };
    // What a finished child printed, and whether it failed and why.
    let settle = |output: std::io::Result<Finished>, failed: &mut usize, echo: bool| {
        match output {
            Ok(done) => {
                if echo {
                    let out = String::from_utf8_lossy(&done.output.stdout);
                    // With a machine reporter on stdout, what the tests printed
                    // goes to stderr.
                    if quiet {
                        eprint!("{out}");
                    } else {
                        print!("{out}");
                    }
                    eprint!("{}", String::from_utf8_lossy(&done.output.stderr));
                }
                if done.expired {
                    // What it managed to print came first: a file that hangs
                    // on its ninth test has eight results worth reading.
                    let message = timed_out(config.timeout);
                    eprintln!("{message}");
                    *failed += 1;
                    Some(message.trim().to_string())
                } else {
                    if !done.output.status.success() {
                        *failed += 1;
                    }
                    None
                }
            }
            Err(e) => {
                eprintln!("  cannot run it: {e}");
                *failed += 1;
                Some(format!("cannot run it: {e}"))
            }
        }
    };

    let mut failed = 0usize;
    if jobs <= 1 {
        for (index, file) in files.iter().enumerate() {
            let Some((child, summary)) = command(file, index) else {
                continue;
            };
            if !quiet {
                println!("{}", named(file));
            }
            // Not captured: with one job the child writes straight through,
            // which is the run to reach for when a test is hanging — unless a
            // machine reporter owns stdout.
            // A child serving a debugger says where on stderr, which has to
            // reach the terminal while it waits rather than after it ends.
            let capture = match (quiet, config.inspect.is_some()) {
                (false, _) => Capture::Nothing,
                (true, true) => Capture::Stdout,
                (true, false) => Capture::Both,
            };
            let output = supervise(child, config.timeout, capture).await;
            let why = settle(output, &mut failed, quiet);
            let result = take_result(summary, file, why);
            reporter.file(&result);
        }
    } else {
        let runs = files.iter().enumerate().map(|(index, file)| {
            let name = named(file);
            let timeout = config.timeout;
            let command = &command;
            async move {
                let (child, summary) = command(file, index)?;
                Some((
                    name,
                    supervise(child, timeout, Capture::Both).await,
                    summary,
                    file,
                ))
            }
        });
        let mut results = futures_util::stream::iter(runs).buffer_unordered(jobs);
        while let Some(result) = results.next().await {
            let Some((name, output, summary, file)) = result else {
                continue;
            };
            if !quiet {
                println!("{name}");
            }
            let why = settle(output, &mut failed, true);
            let result = take_result(summary, file, why);
            reporter.file(&result);
        }
    }
    report_not_run(not_run.load(std::sync::atomic::Ordering::SeqCst), quiet);
    reporter.finish(files.len(), failed);
    failed
}

/// A machine reporter, written as files finish or at the end of the run, to
/// `--reporter-outfile` or stdout. The `human` report is each file's own and
/// needs nothing here.
pub struct Reporter<'a> {
    config: &'a TestConfig,
    format: &'a str,
    /// Everything reported, for the formats written at the end.
    files: Vec<crate::report::FileResult>,
    /// Written so far — the whole report goes to the outfile at the end.
    written: String,
}

impl<'a> Reporter<'a> {
    pub fn new(config: &'a TestConfig) -> Reporter<'a> {
        Reporter {
            config,
            format: config.reporter.as_deref().unwrap_or("human"),
            files: Vec::new(),
            written: String::new(),
        }
    }

    /// One file finished.
    pub fn file(&mut self, result: &crate::report::FileResult) {
        let text = match self.format {
            "json" => crate::report::json_file(result),
            "dots" => crate::report::dots(result),
            _ => String::new(),
        };
        self.emit(&text);
        self.files.push(result.clone());
    }

    /// The run is over.
    pub fn finish(&mut self, total: usize, failed: usize) {
        let text = match self.format {
            "json" => crate::report::json_summary(total, failed),
            "dots" => crate::report::dots_end(&self.files),
            "junit" => crate::report::junit(&self.files),
            "tap" => crate::report::tap(&self.files),
            _ => return,
        };
        self.emit(&text);
        if self.config.reporter_outfile.is_some()
            && let Err(err) = write_report(self.config, &self.written)
        {
            eprintln!("error: {err}");
        }
    }

    fn emit(&mut self, text: &str) {
        if self.config.reporter_outfile.is_some() {
            self.written.push_str(text);
        } else {
            use std::io::Write as _;
            print!("{text}");
            let _ = std::io::stdout().flush();
        }
    }
}

/// Writes a machine report where the run was asked to: the outfile, or stdout.
pub fn write_report(config: &TestConfig, text: &str) -> Result<(), String> {
    match &config.reporter_outfile {
        Some(path) => {
            if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
                std::fs::create_dir_all(dir)
                    .map_err(|err| format!("cannot create {}: {err}", dir.display()))?;
            }
            std::fs::write(path, text)
                .map_err(|err| format!("cannot write {}: {err}", path.display()))
        }
        None => {
            print!("{text}");
            Ok(())
        }
    }
}

/// Says how many files `--bail` kept from starting.
pub fn report_not_run(not_run: usize, quiet: bool) {
    if not_run > 0 && !quiet {
        println!(
            "\nbail: {not_run} file{} did not run after the failure limit",
            if not_run == 1 { "" } else { "s" }
        );
    }
}

/// Runs the child to completion, or **kills it** after `timeout` milliseconds.
///
/// The parent enforces the budget, because the case it exists for is a file
/// that wedges: a limit the child kept for itself is one it never gets round to
/// noticing. `None` is no limit, which stays the default — a test suite is not
/// a place to guess at how long a machine takes.
///
/// **Killing is the part that has to be explicit.** Dropping the future that
/// was waiting on the process does not end the process: the first version of
/// this abandoned the wait and left the child running, holding the inherited
/// stdout, so the run appeared to hang *after* the timeout had already fired.
/// So a watchdog holds the pid and signals it, and the flag it sets is how the
/// caller tells "it finished" from "we ended it".
async fn supervise(
    mut command: tokio::process::Command,
    timeout: Option<u64>,
    capture: Capture,
) -> Result<Finished, std::io::Error> {
    use std::sync::atomic::{AtomicBool, Ordering};

    if capture != Capture::Nothing {
        command.stdout(std::process::Stdio::piped());
    }
    if capture == Capture::Both {
        command.stderr(std::process::Stdio::piped());
    }
    let child = command.spawn()?;
    let pid = child.id();
    let expired = std::sync::Arc::new(AtomicBool::new(false));
    let watchdog = timeout.map(|ms| {
        let expired = expired.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
            expired.store(true, Ordering::SeqCst);
            if let Some(pid) = pid {
                crate::watch::end(pid);
            }
        })
    });

    let output = child.wait_with_output().await;
    if let Some(watchdog) = watchdog {
        watchdog.abort();
    }
    let output = output?;
    Ok(Finished {
        expired: expired.load(Ordering::SeqCst),
        output,
    })
}

/// What of a child's output is held back, to print once it ends.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Capture {
    Nothing,
    Stdout,
    Both,
}

/// How a child ended.
struct Finished {
    /// Whether it was still running when its budget ran out.
    expired: bool,
    output: std::process::Output,
}

/// What a file that ran out of time is told.
pub(crate) fn timed_out(timeout: Option<u64>) -> String {
    let ms = timeout.unwrap_or_default();
    format!(
        "  FAIL the file took longer than {ms}ms and was stopped\n    \
         Something in it never finished. Raise the budget with --timeout, or \
         run it alone with --jobs=1 to watch where it stops."
    )
}

/// Runs the files, then runs them again on every change, until interrupted.
///
/// **Discovery happens per run, not once.** A test you are about to write does
/// not exist when the watcher starts, and a watcher that only knew the files it
/// began with would be silent about the one you just created — which is the file
/// you are watching for.
///
/// The exit status of a watched run is nobody's: it ends when the developer ends
/// it, and what they read is the tally printed after each pass.
pub async fn watch(root: &Path, config: &TestConfig, exe: &Path) -> Result<(), String> {
    let (_watcher, mut rx) = change_watcher(root)?;
    let paint = crate::style::Palette::stderr();
    loop {
        let mut files = discover(root, &config.filters);
        shuffle(&mut files, config.seed);
        if files.is_empty() {
            eprintln!("no test files found (looked for {})", sought_description());
        } else {
            if config.isolation == Some(TestIsolation::None) {
                let code = crate::run_tests_unisolated(&files, config).await;
                report(
                    files.len(),
                    usize::from(code != std::process::ExitCode::SUCCESS),
                );
            } else {
                let jobs = config.jobs.unwrap_or_else(jobs).min(files.len()).max(1);
                let failed = run_all(exe, root, &files, jobs, config).await;
                // A machine reporter wrote its own ending.
                if config.terminal_human() {
                    report(files.len(), failed);
                }
                // Coverage for each pass, as Vitest's watch reports it.
                if let Some(dir) = &config.coverage_dir
                    && let Err(err) = crate::coverage::finish(dir, root, config)
                {
                    eprintln!("error: {err}");
                }
            }
        }
        eprintln!("{}", paint.dim("watching for changes — ^C to stop"));

        tokio::select! {
            change = crate::watch::coalesce(&mut rx) => {
                if change.is_none() {
                    return Ok(());
                }
            }
            () = crate::watch::stopped() => return Ok(()),
        }
        println!();
    }
}

/// Watches `root` for the changes a test run cares about. The watcher must be
/// kept alive for as long as changes are wanted; each change arrives as `()`.
pub fn change_watcher(
    root: &Path,
) -> Result<
    (
        notify::RecommendedWatcher,
        tokio::sync::mpsc::UnboundedReceiver<()>,
    ),
    String,
> {
    use notify::{RecursiveMode, Watcher};

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    let scope = root.to_path_buf();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(event) = res
            && crate::watch::is_change(&event.kind)
            && event
                .paths
                .iter()
                .any(|path| crate::watch::is_interesting(path, &scope))
        {
            let _ = tx.send(());
        }
    })
    .map_err(|e| format!("cannot start the file watcher: {e}"))?;
    watcher
        .watch(root, RecursiveMode::Recursive)
        .map_err(|e| format!("cannot watch {}: {e}", root.display()))?;

    Ok((watcher, rx))
}

/// The line a `--list` run ends with: nothing passed, because nothing ran.
pub fn report_listed(total: usize, failed: usize) {
    let files = if total == 1 { "file" } else { "files" };
    if failed == 0 {
        println!("\n{total} {files} listed");
    } else {
        println!("\n{total} {files} listed, {failed} could not be loaded");
    }
}

/// The tally a run ends with.
pub fn report(total: usize, failed: usize) {
    let files = if total == 1 { "file" } else { "files" };
    if failed == 0 {
        println!("\n{total} {files} passed");
    } else {
        println!("\n{failed} of {total} {files} failed");
    }
}

/// Every test file under `root`, sorted, honouring `filters`.
pub fn discover(root: &Path, filters: &[String]) -> Vec<PathBuf> {
    let mut found = Vec::new();
    collect(root, &mut found);
    // Matched against the path inside the project, not the absolute one: a
    // filter that happens to spell part of where the project lives — `src`
    // for a project under `~/src` — would otherwise select every file.
    found.retain(|p| {
        let text = p
            .strip_prefix(root)
            .unwrap_or(p)
            .to_string_lossy()
            .into_owned();
        filters.is_empty() || filters.iter().any(|f| text.contains(f.as_str()))
    });
    found.sort();
    found
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if !SKIP_DIRS.contains(&name.as_str()) && !name.starts_with('.') {
                collect(&path, out);
            }
        } else if is_test_file(&name) {
            out.push(path);
        }
    }
}

/// Whether a filename names a test.
pub fn is_test_file(name: &str) -> bool {
    TEST_SUFFIXES.iter().any(|s| name.ends_with(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(n: usize) -> Vec<PathBuf> {
        (0..n)
            .map(|i| PathBuf::from(format!("/p/src/t{i}.test.ts")))
            .collect()
    }

    fn shard_of(all: &[PathBuf], index: usize, count: usize) -> Vec<PathBuf> {
        let mut files = all.to_vec();
        shard(&mut files, Path::new("/p"), Shard { index, count });
        files
    }

    #[test]
    fn shards_cover_every_file_once_in_near_equal_parts() {
        let all = files(10);
        let parts: Vec<_> = (1..=3).map(|i| shard_of(&all, i, 3)).collect();
        assert_eq!(parts.iter().map(Vec::len).collect::<Vec<_>>(), [4, 3, 3]);
        let mut seen: Vec<_> = parts.concat();
        seen.sort();
        let mut want = all.clone();
        want.sort();
        assert_eq!(seen, want);
    }

    #[test]
    fn a_shard_depends_on_the_paths_not_their_order() {
        let all = files(7);
        let mut reversed = all.clone();
        reversed.reverse();
        assert_eq!(shard_of(&all, 2, 3), shard_of(&reversed, 2, 3));
    }

    #[test]
    fn a_shard_beyond_the_files_is_empty() {
        let all = files(2);
        assert_eq!(shard_of(&all, 3, 3), Vec::<PathBuf>::new());
        assert_eq!(shard_of(&all, 1, 3).len() + shard_of(&all, 2, 3).len(), 2);
    }

    #[test]
    fn a_shard_is_index_over_count_from_one() {
        assert_eq!(Shard::parse("2/3"), Ok(Shard { index: 2, count: 3 }));
        assert_eq!(Shard::parse("1/1").unwrap().to_string(), "1/1");
        for bad in ["0/3", "4/3", "1/0", "3", "a/b", "1/3/4", "-1/3", ""] {
            let err = Shard::parse(bad).unwrap_err();
            assert!(err.contains("counting from 1"), "{bad}: {err}");
        }
    }

    #[test]
    fn test_files_are_recognised_by_suffix() {
        assert!(is_test_file("app.spec.ts"));
        assert!(is_test_file("app.spec.tsx"));
        assert!(!is_test_file("spec.ts"));
        assert!(is_test_file("app.test.ts"));
        assert!(is_test_file("app.test.mjs"));
        assert!(is_test_file("deep.name.test.tsx"));
        assert!(!is_test_file("app.ts"));
        assert!(!is_test_file("testing.ts"));
        // `test.ts` alone is a module named "test", not a test file: the suffix
        // is `.test.<ext>`, and treating a bare name as a test would sweep in
        // ordinary source.
        assert!(!is_test_file("test.ts"));
    }

    #[test]
    fn discovery_skips_machine_written_directories() {
        let dir = std::env::temp_dir().join(format!("esdev-discover-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).expect("mkdir");
        std::fs::create_dir_all(dir.join("node_modules/pkg")).expect("mkdir");
        std::fs::write(dir.join("src/a.test.mjs"), "").expect("write");
        std::fs::write(dir.join("src/b.mjs"), "").expect("write");
        std::fs::write(dir.join("node_modules/pkg/c.test.mjs"), "").expect("write");

        let found = discover(&dir, &[]);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].ends_with("a.test.mjs"), "{found:?}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The "no test files" message names what discovery looks for, and it is
    /// derived from the suffix list rather than written out — so adding a
    /// suffix updates the message, or fails here.
    #[test]
    fn the_no_tests_message_names_what_discovery_looks_for() {
        assert_eq!(
            sought_description(),
            "*.test.js/.mjs/.ts/.tsx/.jsx/.mts and *.spec.js/.mjs/.ts/.tsx/.jsx/.mts"
        );
        for suffix in TEST_SUFFIXES {
            let ext = suffix.rsplit('.').next().expect("an extension");
            assert!(
                sought_description().contains(ext),
                "the message is missing {suffix}"
            );
        }
    }

    #[test]
    fn filters_match_on_the_path() {
        let dir = std::env::temp_dir().join(format!("esdev-filter-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join("alpha.test.mjs"), "").expect("write");
        std::fs::write(dir.join("beta.test.mjs"), "").expect("write");

        assert_eq!(discover(&dir, &["alpha".to_string()]).len(), 1);
        assert_eq!(discover(&dir, &["nope".to_string()]).len(), 0);
        assert_eq!(discover(&dir, &[]).len(), 2);
        // Part of where the project lives is not part of any file's path in it.
        assert_eq!(discover(&dir, &["esdev-filter".to_string()]).len(), 0);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
