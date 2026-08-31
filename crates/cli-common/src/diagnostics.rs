//! The one error block a binary prints, and the buffered failures the drive
//! loop hands over while a program runs.

use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use es_runtime::ModuleEvalState;

/// Prints an error as one coherent block: a bold-red `error` headline, then the
/// body, with `at …` stack frames dimmed (SPEC Phase 13).
///
/// Color is used only when stderr is a terminal and `NO_COLOR` is unset, so a
/// redirected or piped run stays plain text.
pub fn print_error(err: &str) {
    use std::io::IsTerminal;

    // Frames first: what is printed should name the code that was written, not
    // the bundle it was written into ([`crate::sourcemap`]). A build with no
    // map beside it comes back unchanged.
    let err = &crate::sourcemap::remap(err);
    let use_color = std::io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none();
    if !use_color {
        eprintln!("error: {err}");
        return;
    }

    let mut lines = err.lines();
    if let Some(first) = lines.next() {
        eprintln!("\x1b[1;31merror\x1b[0m: {first}");
    }
    for line in lines {
        if line.starts_with("    at ") {
            eprintln!("\x1b[2m{line}\x1b[0m");
        } else {
            eprintln!("{line}");
        }
    }
}

/// One failure the drive handed over, with the body kept separately so the
/// entry module's own rejection can be recognised (see [`flush_failures`]).
pub struct Failure {
    /// The whole message as it will be printed, headline included.
    pub text: String,
    /// The error body alone, compared against the module's evaluation failure.
    pub body: String,
}

/// Prints the buffered failures, dropping the one that *is* the entry module's
/// evaluation failure — that one is reported once, by name, as an uncaught
/// exception. Everything else is a failure the guest left unclaimed while it
/// ran, and is printed at the point it happened rather than at exit, which for
/// a program that never quiesces (a listening server) never came.
pub fn flush_failures(
    pending: &Arc<Mutex<Vec<Failure>>>,
    reported: &Arc<AtomicI32>,
    state: &ModuleEvalState,
) {
    let module_failure = match state {
        ModuleEvalState::Failed(e) => Some(e.to_string()),
        _ => None,
    };
    for failure in pending.lock().unwrap_or_else(|e| e.into_inner()).drain(..) {
        if module_failure.as_deref() == Some(failure.body.as_str()) {
            continue;
        }
        eprintln!("{}", failure.text);
        reported.fetch_add(1, Ordering::SeqCst);
    }
}

/// The message for a run the watchdog stopped.
pub fn timeout_message(timeout: Option<Duration>) -> String {
    match timeout {
        Some(d) => format!("execution timed out after {} ms", d.as_millis()),
        None => "execution timed out".to_string(),
    }
}
