//! The console sink every ES-Runtime binary writes through.

use es_runtime_providers::{Console, ConsoleLevel};

/// Writes `console.*` output to the process's stdout/stderr.
///
/// `warn`/`error` go to stderr, everything else to stdout — the split Node and
/// Deno both make, so a shell redirect separates diagnostics from output.
pub struct StdoutConsole;

impl Console for StdoutConsole {
    fn write(&self, level: ConsoleLevel, message: &str) {
        use std::io::Write;

        // One `write_all` of message-plus-newline, under one lock, because
        // more than one agent writes here now: `writeln!` on the unlocked
        // handle can take the lock once for the message and again for the
        // newline, which is a window for another worker's line to land in the
        // middle of this one.
        let mut line = String::with_capacity(message.len() + 1);
        line.push_str(message);
        line.push('\n');

        if matches!(level, ConsoleLevel::Warn | ConsoleLevel::Error) {
            // Nothing useful to do if stderr itself is gone.
            let _ = std::io::stderr().lock().write_all(line.as_bytes());
            return;
        }

        if let Err(err) = std::io::stdout().lock().write_all(line.as_bytes()) {
            // A closed pipe is how `esrun script.js | head` ends, not a failure:
            // the reader took what it wanted and left. `println!` panics on it,
            // which turned an everyday shell idiom into a Rust backtrace on
            // stderr and an exit code of 1 — leaking internals for something
            // that is not the guest's fault and not ours. Node and Deno both
            // stop quietly here, so do the same.
            //
            // Any other write failure is equally unactionable from inside a
            // console sink, so it is dropped rather than raised into guest code.
            if err.kind() == std::io::ErrorKind::BrokenPipe {
                std::process::exit(0);
            }
        }
    }
}
