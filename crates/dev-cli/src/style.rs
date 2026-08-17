//! Colour, for the lines esdev writes about its own work.
//!
//! # Why a module rather than escape codes at each call site
//!
//! Because the rule is one rule and it has to hold everywhere: **colour only
//! when the destination is a terminal and `NO_COLOR` is unset.** Written inline,
//! that rule becomes a condition somebody forgets at one `println!`, and the
//! result is a build log with `\x1b[32m` in it — which is worse than no colour,
//! because it breaks the tools reading that log rather than merely being plain.
//!
//! It is the same rule `es_runtime_cli_common::diagnostics` applies to the error
//! block, and the same escape codes; this is that rule for everything that is
//! not an error.
//!
//! # Per stream, not per process
//!
//! `esdev build` writes its report to stdout and `esdev start` writes its status
//! to stderr, and a run can very reasonably have one of those piped and the
//! other on a terminal — `esdev build > build.log` is a normal thing to do. So
//! the gate is asked about the stream being written to, not about the process.
//!
//! # What the colours mean
//!
//! Four, and each says one thing, so a line can be read at a glance without
//! being read:
//!
//! * **green** — something was produced. `built`, `bundled`, `created`.
//! * **cyan** — somewhere to go or look: a path, a URL, a name.
//! * **bold** — a command to type.
//! * **dim** — true but secondary; the part you skip when skimming.
//!
//! Nothing is red here. Red is the error block's, and a status line that reaches
//! for it is a status line competing with an actual failure.

use std::io::IsTerminal;

/// Whether one stream may carry colour.
#[derive(Clone, Copy)]
pub struct Palette {
    enabled: bool,
}

impl Palette {
    /// For text going to stdout — a build's report.
    pub fn stdout() -> Self {
        Self {
            enabled: std::io::stdout().is_terminal() && allowed(),
        }
    }

    /// For text going to stderr — the dev loop's status.
    pub fn stderr() -> Self {
        Self {
            enabled: std::io::stderr().is_terminal() && allowed(),
        }
    }

    /// Something was produced.
    pub fn green(self, text: impl std::fmt::Display) -> String {
        self.wrap("32", text)
    }

    /// Somewhere to go or look.
    pub fn cyan(self, text: impl std::fmt::Display) -> String {
        self.wrap("36", text)
    }

    /// Something to type.
    pub fn bold(self, text: impl std::fmt::Display) -> String {
        self.wrap("1", text)
    }

    /// True, and secondary.
    pub fn dim(self, text: impl std::fmt::Display) -> String {
        self.wrap("2", text)
    }

    fn wrap(self, code: &str, text: impl std::fmt::Display) -> String {
        if self.enabled {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }
}

/// The half of the rule that is about intent rather than about plumbing.
///
/// [`NO_COLOR`](https://no-color.org) is honoured for its presence, whatever it
/// is set to — that is what the convention says, and an empty value is still
/// somebody having asked.
fn allowed() -> bool {
    std::env::var_os("NO_COLOR").is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A test has no terminal, so every palette it can build is the plain one —
    /// which is the property worth pinning: nothing escapes into a pipe.
    #[test]
    fn a_stream_that_is_not_a_terminal_gets_no_escapes() {
        let plain = Palette { enabled: false };
        assert_eq!(plain.green("built"), "built");
        assert_eq!(plain.cyan("dist/app.js"), "dist/app.js");
        assert_eq!(plain.bold("npm run dev"), "npm run dev");
        assert_eq!(plain.dim("(4 files)"), "(4 files)");
    }

    /// And when it is one, every code is closed — a line that sets a colour and
    /// does not reset it colours the shell prompt after it.
    #[test]
    fn every_colour_is_closed_again() {
        let colour = Palette { enabled: true };
        for painted in [
            colour.green("a"),
            colour.cyan("b"),
            colour.bold("c"),
            colour.dim("d"),
        ] {
            assert!(painted.starts_with("\x1b["), "{painted:?}");
            assert!(painted.ends_with("\x1b[0m"), "{painted:?}");
        }
    }
}
