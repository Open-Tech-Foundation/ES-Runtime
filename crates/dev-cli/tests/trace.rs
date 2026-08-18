//! End-to-end tests for `esdev --trace-permissions`.
//!
//! The claim this feature makes is narrow and checkable: **the line it prints is
//! the line that runs**. So these do not stop at asserting the report's wording
//! — they take the `esrun` command line out of it, run the program under `esrun`
//! with exactly that, and check it works. The stronger half is the other
//! direction: dropping any one grant from the line must make the same program
//! fail, which is what makes the line *minimal* rather than merely sufficient.

// A test reporting why it skipped is talking to whoever reads the run.
#![allow(clippy::print_stderr)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn temp(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name)
}

fn write(name: &str, contents: &str) -> PathBuf {
    let path = temp(name);
    std::fs::write(&path, contents).expect("write temp file");
    path
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Runs `esdev --trace-permissions <entry>` and returns everything it said.
fn trace(entry: &Path, extra: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_esdev"))
        // The sandbox is the working directory (D79): run from where the
        // fixtures live.
        .current_dir(env!("CARGO_TARGET_TMPDIR"))
        .arg("--trace-permissions")
        .args(extra)
        .arg(entry)
        .output()
        .expect("spawn esdev");
    stderr(&out)
}

/// The `esrun …` line out of a report, split into arguments.
fn grant_line(report: &str) -> Vec<String> {
    let line = report
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("esrun "))
        .unwrap_or_else(|| panic!("no grant line in:\n{report}"));
    line.split_whitespace()
        .skip(1)
        .map(str::to_string)
        .collect()
}

/// `esrun` from the same target directory, or `None` if it is not built —
/// cargo exports `CARGO_BIN_EXE_*` only for this package's own binaries.
fn esrun() -> Option<PathBuf> {
    PathBuf::from(env!("CARGO_BIN_EXE_esdev"))
        .parent()
        .map(|dir| dir.join(format!("esrun{}", std::env::consts::EXE_SUFFIX)))
        .filter(|path| path.exists())
}

#[test]
fn the_line_it_prints_is_the_line_that_runs_and_the_smallest_one() {
    let dep = write("trace-dep.mjs", "export const answer = 42;\n");
    let app = write(
        "trace-app.mjs",
        &format!(
            "import {{ file }} from \"runtime:fs\";\n\
             import {{ env }} from \"runtime:process\";\n\
             import {{ answer }} from {:?};\n\
             const text = await file({:?}).text();\n\
             console.log(answer, text.length > 0, typeof env.PATH);\n",
            dep.to_string_lossy(),
            dep.to_string_lossy(),
        ),
    );

    let report = trace(&app, &[]);
    let args = grant_line(&report);
    // No --deny-all: esrun grants nothing on its own (D65), so the grants are
    // the whole line.
    assert!(!args.contains(&"--deny-all".to_string()), "{report}");
    for expected in ["--allow-read", "--allow-imports", "--allow-env"] {
        assert!(
            args.contains(&expected.to_string()),
            "{expected} missing from {args:?}\n{report}"
        );
    }

    let Some(esrun) = esrun() else {
        eprintln!("skipped the esrun half: it is not built in this target directory");
        return;
    };

    // Sufficient: the line runs the program.
    let out = Command::new(&esrun)
        .current_dir(env!("CARGO_TARGET_TMPDIR"))
        .args(&args[..args.len() - 1])
        .arg(&app)
        .output()
        .expect("spawn esrun");
    assert!(
        out.status.success(),
        "the traced line failed: {}",
        stderr(&out)
    );

    // Minimal: take any one grant away and the same program stops working.
    // Without this, a trace that printed every flag there is would pass.
    let grants: Vec<&String> = args.iter().filter(|a| a.starts_with("--allow-")).collect();
    assert!(!grants.is_empty(), "{report}");
    for dropped in &grants {
        let kept: Vec<&String> = args[..args.len() - 1]
            .iter()
            .filter(|a| a != dropped)
            .collect();
        let out = Command::new(&esrun)
            .current_dir(env!("CARGO_TARGET_TMPDIR"))
            .args(&kept)
            .arg(&app)
            .output()
            .expect("spawn esrun");
        assert!(
            !out.status.success(),
            "the program still ran without {dropped}, so the trace over-granted"
        );
    }
}

#[test]
fn a_program_that_reaches_for_nothing_is_told_so() {
    let app = write("trace-pure.mjs", "console.log(6 * 7);\n");
    let report = trace(&app, &[]);
    assert!(report.contains("nothing at all"), "{report}");
    assert_eq!(grant_line(&report), vec![app.to_str().unwrap()]);
}

#[test]
fn what_was_refused_is_reported_and_left_out_of_the_line() {
    // The run this exists for: it failed *because* of a permission, which is
    // exactly when the developer wants to be told which one.
    let dep = write("trace-refused-dep.mjs", "export const x = 1;\n");
    let app = write(
        "trace-refused.mjs",
        &format!(
            "import {{ x }} from {:?};\nconsole.log(x);\n",
            dep.to_string_lossy()
        ),
    );
    let report = trace(&app, &["--deny-all"]);
    assert!(report.contains("imports"), "{report}");
    assert!(report.contains("asked and was refused"), "{report}");
    // A refusal is not a recommendation: whether it was the right answer is the
    // developer's call, so it never becomes an --allow flag.
    assert!(
        !grant_line(&report).iter().any(|a| a == "--allow-imports"),
        "{report}"
    );
}

#[test]
fn a_workers_capabilities_are_in_the_same_report() {
    let child = write(
        "trace-worker-child.mjs",
        "import { env } from \"runtime:process\";\npostMessage(typeof env.PATH);\n",
    );
    let app = write(
        "trace-worker.mjs",
        &format!(
            "const w = new Worker({:?}, {{ permissions: [\"env\", \"imports\"] }});\n\
             w.onmessage = () => w.terminate();\n",
            url_of(&child)
        ),
    );
    let report = trace(&app, &[]);
    // `env` is used only inside the worker, on its own thread in its own
    // isolate. A report that stopped at the main agent would miss it — and a
    // worker's grants, set at the spawn, are the ones hardest to get right.
    assert!(report.contains("env"), "{report}");
    assert!(
        grant_line(&report).iter().any(|a| a == "--allow-env"),
        "{report}"
    );
    assert!(
        grant_line(&report).iter().any(|a| a == "--allow-workers"),
        "{report}"
    );
}

/// A `file:` URL for a path, which is what `new Worker(...)` takes.
fn url_of(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    if text.starts_with('/') {
        format!("file://{text}")
    } else {
        format!("file:///{text}")
    }
}
