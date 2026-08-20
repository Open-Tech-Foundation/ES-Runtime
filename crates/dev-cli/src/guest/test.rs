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
use std::process::ExitCode;

use es_runtime_cli_common::{ExtensionContext, HostExtension, HostModule, OpDecl, Value};

/// One test case, from `test()` to whatever became of it.
struct Case {
    name: String,
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
}

/// The `runtime:test` extension.
pub struct TestExtension;

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
