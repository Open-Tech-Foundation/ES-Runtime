//! `esdev test --coverage` (DECISIONS D108).
//!
//! Each test file's process collects V8's counts ([`collect`]); the parent
//! turns them into statement, branch, function and line coverage of the files
//! as they were written ([`map`]), and reports it ([`report`]).

pub mod collect;
pub mod map;
pub mod report;

/// What `--coverage` measures and writes, from `test.coverage` in esdev.json.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// Globs, relative to the project, of the files to report. Empty: every
    /// file the tests loaded. A matching file no test loaded is reported as
    /// not covered at all.
    pub include: Vec<String>,
    /// Globs of files left out, beside the test files and `node_modules`.
    pub exclude: Vec<String>,
    /// `text`, `lcov`, `json`, `json-summary`.
    pub reporters: Vec<String>,
    /// Where the reports that are files go, relative to the project.
    pub directory: String,
    pub thresholds: Thresholds,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            include: Vec::new(),
            exclude: Vec::new(),
            reporters: vec!["text".to_string(), "lcov".to_string()],
            directory: "coverage".to_string(),
            thresholds: Thresholds::default(),
        }
    }
}

/// The reporters there are.
pub const REPORTERS: &[&str] = &["text", "lcov", "json", "json-summary"];

/// The least coverage a run may have, per measure. A positive number is a
/// percentage; a negative one is how many may go uncovered, as in Vitest.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Thresholds {
    pub lines: Option<f64>,
    pub functions: Option<f64>,
    pub branches: Option<f64>,
    pub statements: Option<f64>,
}

use std::path::{Path, PathBuf};

use globset::{Glob, GlobSet, GlobSetBuilder};

/// Where the test processes write what they collected: a directory of one
/// file per test file, emptied for each run.
pub fn scratch() -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join(format!("esdev-coverage-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)
        .map_err(|err| format!("cannot make a directory for coverage: {err}"))?;
    Ok(dir)
}

/// Why a run cannot collect coverage, if it cannot.
pub fn refused(config: &crate::test::TestConfig) -> Option<String> {
    config.coverage.as_ref()?;
    if !es_runtime_cli_common::HAS_INSPECTOR {
        return Some(format!(
            "--coverage reads V8's counts through the inspector, and {}",
            es_runtime_cli_common::NO_INSPECTOR_MESSAGE
        ));
    }
    if config.browser.is_some() {
        return Some(
            "--coverage measures what ran in this runtime, and a browser run runs elsewhere.\n\n\
             Drop --browser to measure the files here."
                .to_string(),
        );
    }
    if config.isolation == Some(crate::config::TestIsolation::None) {
        return Some(
            "--coverage is collected per test file, and --isolation=none runs them together.\n\n\
             Drop one of them."
                .to_string(),
        );
    }
    None
}

/// Turns what the test processes wrote into the reports, and says whether the
/// thresholds were met. The table goes to stdout, or to stderr when a machine
/// reporter has stdout.
pub fn finish(dir: &Path, root: &Path, config: &crate::test::TestConfig) -> Result<bool, String> {
    let Some(settings) = &config.coverage else {
        return Ok(true);
    };
    let mut collected = map::Collected::default();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        if let Some(written) = std::fs::read_to_string(entry.path())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
        {
            collected.add(&written);
        }
    }
    // Emptied rather than removed: a watch writes the next pass here.
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let _ = std::fs::remove_file(entry.path());
    }

    let files: Vec<map::FileCoverage> = selected(&collected, root, settings, config)?
        .iter()
        .filter_map(|path| collected.measure(path))
        .collect();

    let out = root.join(&settings.directory);
    for reporter in &settings.reporters {
        let (name, text) = match reporter.as_str() {
            "text" => {
                let table = report::text(root, &files);
                if config.terminal_human() {
                    println!("\n{table}");
                } else {
                    eprintln!("\n{table}");
                }
                continue;
            }
            "lcov" => ("lcov.info", report::lcov(root, &files)),
            "json" => ("coverage-final.json", report::istanbul_json(&files)),
            "json-summary" => ("coverage-summary.json", report::json_summary(&files)),
            other => return Err(format!("{other} is not a coverage reporter")),
        };
        std::fs::create_dir_all(&out)
            .and_then(|()| std::fs::write(out.join(name), text))
            .map_err(|err| format!("cannot write the coverage report {name}: {err}"))?;
    }

    let mut total = report::Summary::default();
    for file in &files {
        total.add(report::Summary::of(file));
    }
    let failures = report::short_of(&total, &settings.thresholds);
    for failure in &failures {
        eprintln!("coverage: {failure}");
    }
    Ok(failures.is_empty())
}

/// The files a report covers: those the tests loaded — or, with `include`,
/// those it matches, loaded or not — less the tests, their setup,
/// `node_modules` and `exclude`.
fn selected(
    collected: &map::Collected,
    root: &Path,
    settings: &Settings,
    config: &crate::test::TestConfig,
) -> Result<Vec<PathBuf>, String> {
    let include = globs(&settings.include)?;
    let exclude = globs(&settings.exclude)?;
    let setup: Vec<PathBuf> = config
        .run
        .setup
        .iter()
        .chain(&config.global_setup)
        .filter_map(|module| match url::Url::parse(module) {
            Ok(url) => url.to_file_path().ok(),
            Err(_) => Some(root.join(module)),
        })
        .map(|path| crate::related::canonical(&path))
        .collect();
    let reports = root.join(&settings.directory);
    let wanted = |path: &Path| {
        let Ok(inside) = path.strip_prefix(root) else {
            return false;
        };
        let name = report::name(root, path);
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        !inside
            .components()
            .any(|part| part.as_os_str() == "node_modules")
            && !crate::test::is_test_file(file_name)
            && !path.starts_with(&reports)
            && !setup.contains(&crate::related::canonical(path))
            && (settings.include.is_empty() || include.is_match(&name))
            && !exclude.is_match(&name)
    };
    let mut files: Vec<PathBuf> = collected
        .files()
        .into_iter()
        .filter(|path| wanted(path))
        .collect();
    if !settings.include.is_empty() {
        let mut found = Vec::new();
        sources(root, &mut found);
        files.extend(found.into_iter().filter(|path| wanted(path)));
    }
    files.sort();
    files.dedup();
    Ok(files)
}

/// Globs as Vitest reads them: relative to the project, and a pattern with no
/// wildcard naming a directory and everything in it.
fn globs(patterns: &[String]) -> Result<GlobSet, String> {
    let mut set = GlobSetBuilder::new();
    for pattern in patterns {
        let pattern = pattern.trim_start_matches("./");
        let mut add = |pattern: &str| -> Result<(), String> {
            set.add(Glob::new(pattern).map_err(|err| format!("coverage glob {pattern}: {err}"))?);
            Ok(())
        };
        add(pattern)?;
        if !pattern.contains(['*', '?', '[', '{']) {
            add(&format!("{}/**", pattern.trim_end_matches('/')))?;
        }
    }
    set.build().map_err(|err| format!("coverage globs: {err}"))
}

/// Every module source under `dir`, skipping what discovery skips.
fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if !name.starts_with('.')
                && !["node_modules", "dist", "target"].contains(&name.as_str())
            {
                sources(&path, out);
            }
        } else if ["js", "mjs", "cjs", "ts", "mts", "cts", "jsx", "tsx"]
            .iter()
            .any(|ext| path.extension().is_some_and(|e| e == *ext))
            && !name.ends_with(".d.ts")
        {
            out.push(crate::related::canonical(&path));
        }
    }
}
