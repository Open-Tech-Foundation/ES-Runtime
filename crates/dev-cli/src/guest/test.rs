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
//! Keeping the score in the host removes all of it. `test()` says a case
//! started and, later, how it ended; `esdev` reads the tally after the program
//! reaches quiescence, prints it, and picks the exit code. Nothing is injected
//! into anybody's source.
//!
//! It also fixes a failure the old design could not see. A test whose promise
//! never settles used to hang the program at the epilogue's `Promise.all`;
//! here, the case is simply never finished, and a started-but-unfinished case
//! is reported as a **failure** rather than silently left out of a green run.
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
    /// `None` while it is still running — and still `None` at the end if it
    /// never settled, which is a failure with a name of its own.
    outcome: Option<Outcome>,
}

enum Outcome {
    Passed,
    /// The stack, when the error had one; the error's text otherwise.
    Failed(String),
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
            // started(name) -> id
            //
            // No capability, and nothing to gate: an assertion computes, and a
            // tally is bookkeeping this process keeps about itself. The same
            // reasoning as `runtime:hashing`.
            OpDecl::sync("test_started", |args| {
                let name = args
                    .first()
                    .and_then(Value::as_str)
                    .unwrap_or("(unnamed)")
                    .to_string();
                let id = CASES.with_borrow_mut(|cases| {
                    cases.push(Case {
                        name,
                        outcome: None,
                    });
                    cases.len() - 1
                });
                Ok(Value::Number(id as f64))
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
                    reason = "the id came from `started`, which handed out an index"
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
    let (passed, failures) = CASES.with_borrow(|cases| {
        let mut passed = 0usize;
        let mut failures: Vec<(String, String)> = Vec::new();
        for case in cases {
            match &case.outcome {
                Some(Outcome::Passed) => passed += 1,
                Some(Outcome::Failed(detail)) => {
                    failures.push((case.name.clone(), detail.clone()));
                }
                // Registered, never settled. The old harness hung the program
                // here; a test that cannot finish is a failing test, and saying
                // so is the difference between a red run and a green one that
                // quietly ran fewer tests than it printed.
                None => failures.push((
                    case.name.clone(),
                    "the test never finished — it is waiting on something that never happened"
                        .to_string(),
                )),
            }
        }
        (passed, failures)
    });

    if passed == 0 && failures.is_empty() {
        return ExitCode::SUCCESS;
    }

    for (name, detail) in &failures {
        println!("  FAIL {name}");
        for line in detail.lines() {
            println!("    {line}");
        }
    }
    println!("  {passed} passed, {} failed", failures.len());

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
