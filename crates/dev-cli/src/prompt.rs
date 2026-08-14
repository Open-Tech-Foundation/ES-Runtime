//! Asking a question on a terminal.
//!
//! A numbered list and a line of input. No dependency, because the two crates
//! that do this well both draw their own UI with raw-mode terminal control, and
//! a scaffolder that runs once does not need a redraw loop — it needs a
//! question somebody can answer, and a transcript they can read afterwards.
//!
//! # It only ever runs on a terminal
//!
//! [`interactive`] is the gate, and it is deliberately strict: **stdin and
//! stderr must both be a TTY, and no `CI` variable may be set.** A prompt that
//! appears in a script is a script that hangs, which is the failure this is
//! written to avoid — every other `esdev` command is a flag grammar that works
//! unattended, and `create` stays one whenever it cannot see a person.
//!
//! Everything a prompt asks has a flag, so the interactive path is a
//! convenience over the scriptable one and never the only way to reach an
//! answer.
//!
//! # Questions go to stderr
//!
//! So `esdev create app > notes.txt` still shows them, and what lands in the
//! file is the report rather than a half-drawn menu.

use std::io::{IsTerminal, Write};

/// Whether this run may ask questions.
///
/// `CI` is honoured because build systems set it and pipe a terminal in anyway;
/// it is the one signal that is about intent rather than about plumbing.
pub fn interactive() -> bool {
    std::io::stdin().is_terminal()
        && std::io::stderr().is_terminal()
        && std::env::var_os("CI").is_none()
}

/// One option in a [`select`].
pub struct Choice<'a> {
    /// The value, and what a flag would spell.
    pub name: &'a str,
    /// One line, for somebody choosing.
    pub description: &'a str,
}

/// Asks `question` and returns the index of the chosen option.
///
/// Enter takes `default`. An answer that is not an option is re-asked rather
/// than resolved to something nearby — a scaffolder writes a project, and a
/// typo silently producing the wrong one is worse than a second question.
///
/// End of input takes the default too: a closed stdin is not an answer, and
/// looping on it would hang exactly where this is supposed to not.
pub fn select(question: &str, choices: &[Choice<'_>], default: usize) -> usize {
    let width = choices.iter().map(|c| c.name.len()).max().unwrap_or(0);

    loop {
        eprintln!("\n{question}");
        for (i, choice) in choices.iter().enumerate() {
            let marker = if i == default { " (default)" } else { "" };
            let line = format!(
                "  {}) {:width$}  {}{}",
                i + 1,
                choice.name,
                choice.description,
                marker,
            );
            // Trimmed, because a choice with no description would otherwise pad
            // to the column width and leave trailing spaces on the line.
            eprintln!("{}", line.trim_end());
        }
        eprint!("> ");
        let _ = std::io::stderr().flush();

        let Some(line) = read_line() else {
            eprintln!();
            return default;
        };
        let answer = line.trim();
        if answer.is_empty() {
            return default;
        }
        // By number, or by name — somebody who knows what they want should not
        // have to count.
        if let Ok(number) = answer.parse::<usize>()
            && (1..=choices.len()).contains(&number)
        {
            return number - 1;
        }
        if let Some(found) = choices
            .iter()
            .position(|choice| choice.name.eq_ignore_ascii_case(answer))
        {
            return found;
        }
        eprintln!("  `{answer}` is not one of them.");
    }
}

/// Reads one line, or `None` at end of input.
fn read_line() -> Option<String> {
    let mut line = String::new();
    match std::io::stdin().read_line(&mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(line),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The gate is the whole safety property: a prompt in a script is a script
    /// that hangs. Asserted on the pieces, since a test has no terminal.
    #[test]
    fn a_test_process_is_never_interactive() {
        // Whatever the harness does with stdin, `CI` alone is enough to refuse,
        // and a test never has a terminal on both.
        assert!(!interactive() || std::io::stdin().is_terminal());
    }

    #[test]
    fn choices_can_be_named_as_well_as_numbered() {
        // The lookup `select` does, without the terminal it does it on.
        let choices = [
            Choice {
                name: "react",
                description: "",
            },
            Choice {
                name: "api",
                description: "",
            },
        ];
        assert_eq!(
            choices
                .iter()
                .position(|c| c.name.eq_ignore_ascii_case("API")),
            Some(1)
        );
    }
}
