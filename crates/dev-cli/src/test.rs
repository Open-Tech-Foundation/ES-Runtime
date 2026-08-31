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

/// What `esdev test` was asked to do.
pub struct TestConfig {
    /// Run exactly this file, harness installed. This is what the parent
    /// invokes for each child, and it is a supported way to run one file
    /// directly.
    pub file: Option<String>,
    /// Substring filters on the discovered paths; empty means all of them.
    pub filters: Vec<String>,
    /// How many files may run at once, from `--jobs`. `None` is [`jobs`].
    pub jobs: Option<usize>,
    /// Keep running, re-running the files when a source file changes.
    pub watch: bool,
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
const TEST_SUFFIXES: &[&str] = &[
    ".test.js",
    ".test.mjs",
    ".test.ts",
    ".test.tsx",
    ".test.jsx",
    ".test.mts",
];

/// Directories discovery never descends into.
const SKIP_DIRS: &[&str] = &["node_modules", ".git", "dist", "target", ".cache"];

/// Runs each file in its own process, up to `jobs` at a time, and reports how
/// many failed.
///
/// **Output is buffered per file whenever more than one runs at a time**, and
/// printed whole when that file finishes. Two suites writing to one terminal
/// interleave line by line, which turns a failure's message and the assertion
/// above it into two things a reader has to reassemble. With `--jobs=1` nothing
/// is buffered and the child writes straight through — which is what you want
/// from the run where you are watching a test hang.
pub async fn run_all(exe: &Path, root: &Path, files: &[PathBuf], jobs: usize) -> usize {
    use futures_util::StreamExt;

    let named = |file: &Path| {
        file.strip_prefix(root)
            .unwrap_or(file)
            .display()
            .to_string()
    };

    if jobs <= 1 {
        let mut failed = 0usize;
        for file in files {
            println!("{}", named(file));
            let status = tokio::process::Command::new(exe)
                .arg("test")
                .arg(format!("--file={}", file.display()))
                .status()
                .await;
            match status {
                Ok(status) if status.success() => {}
                Ok(_) => failed += 1,
                Err(e) => {
                    eprintln!("  cannot run it: {e}");
                    failed += 1;
                }
            }
        }
        return failed;
    }

    let runs = files.iter().map(|file| {
        let exe = exe.to_path_buf();
        let name = named(file);
        let file = file.clone();
        async move {
            let output = tokio::process::Command::new(&exe)
                .arg("test")
                .arg(format!("--file={}", file.display()))
                .output()
                .await;
            (name, output)
        }
    });

    let mut failed = 0usize;
    let mut results = futures_util::stream::iter(runs).buffer_unordered(jobs);
    while let Some((name, output)) = results.next().await {
        println!("{name}");
        match output {
            Ok(output) => {
                // The child's two streams, in the order a reader expects them:
                // what the test printed, then what went wrong. Written through
                // rather than re-formatted — a harness's output is its own.
                print!("{}", String::from_utf8_lossy(&output.stdout));
                eprint!("{}", String::from_utf8_lossy(&output.stderr));
                if !output.status.success() {
                    failed += 1;
                }
            }
            Err(e) => {
                eprintln!("  cannot run it: {e}");
                failed += 1;
            }
        }
    }
    failed
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
    use notify::{RecursiveMode, Watcher};

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<()>();
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

    let paint = crate::style::Palette::stderr();
    loop {
        let files = discover(root, &config.filters);
        if files.is_empty() {
            eprintln!("no test files found (looked for *.test.js/.mjs/.ts/.tsx/.jsx)");
        } else {
            let jobs = config.jobs.unwrap_or_else(jobs).min(files.len()).max(1);
            let failed = run_all(exe, root, &files, jobs).await;
            report(files.len(), failed);
        }
        eprintln!("{}", paint.dim("watching for changes — ^C to stop"));

        tokio::select! {
            change = crate::watch::coalesce(&mut rx) => {
                if change.is_none() {
                    return Ok(());
                }
            }
            _ = tokio::signal::ctrl_c() => return Ok(()),
        }
        println!();
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
    found.retain(|p| {
        let text = p.to_string_lossy().into_owned();
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

    #[test]
    fn test_files_are_recognised_by_suffix() {
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

        let _ = std::fs::remove_dir_all(&dir);
    }
}
