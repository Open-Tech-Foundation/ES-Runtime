//! Asking a question on a terminal.
//!
//! An arrow-key menu, drawn with ratatui into an **inline viewport**.
//!
//! This file used to argue against the dependency, and the argument was that a
//! scaffolder does not need a redraw loop. That was true and it was not the
//! point: what somebody choosing a template needs is to *see* the choices and
//! move through them, and a numbered list read off stdin makes them count lines
//! and then type a digit. The redraw loop is the cheap part of paying for that.
//!
//! # Inline, never the alternate screen
//!
//! The menu is drawn where the cursor already is, and when it is answered it is
//! replaced in place by one line naming the answer. Nothing scrolls away and
//! nothing is restored: what is left in the scrollback afterwards is a
//! transcript of the questions and what was said to them, which is exactly what
//! somebody re-reading their terminal an hour later is looking for.
//!
//! A full-screen TUI would take the terminal over and hand it back empty, and
//! the record of a command that writes a project to disk would be gone.
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
//! file is the report rather than a half-drawn menu. The viewport is drawn on
//! stderr for the same reason.

use std::io::{IsTerminal, Write};

use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use ratatui::layout::Position;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::{Terminal, TerminalOptions, Viewport};

/// Whether this run may ask questions.
///
/// `CI` is honoured because build systems set it and pipe a terminal in anyway;
/// it is the one signal that is about intent rather than about plumbing.
pub fn interactive() -> bool {
    std::io::stdin().is_terminal()
        && std::io::stderr().is_terminal()
        && std::env::var_os("CI").is_none()
}

/// Whether output may carry colour.
///
/// The same rule the rest of the CLI applies (`es_runtime_cli_common::
/// diagnostics`): a terminal, and no `NO_COLOR`. Kept as a function rather than
/// a constant because a caller may write to a pipe in the same process that
/// asked a question on a terminal.
fn colour() -> bool {
    std::io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none()
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
/// `None` is a deliberate cancel — Esc, or ^C. It is not the same as taking the
/// default, and callers are expected to stop rather than to guess: somebody who
/// pressed Esc at "which template?" did not ask for the default template.
///
/// End of input *is* the default: a closed stdin is not a decision, and looping
/// on it would hang exactly where this is written not to.
pub fn select(question: &str, choices: &[Choice<'_>], default: usize) -> Option<usize> {
    if choices.is_empty() {
        return None;
    }
    let default = default.min(choices.len() - 1);

    // A terminal that will not go into raw mode is not a terminal this can draw
    // on, and the question still has to be asked. The fallback is the plain
    // numbered list, which needs nothing but a line of input.
    let chosen = match menu(question, choices, default) {
        Ok(chosen) => chosen,
        Err(_) => numbered(question, choices, default),
    }?;

    answered(question, choices[chosen].name);
    Some(chosen)
}

/// The menu, drawn and driven. `Err` means the terminal would not cooperate.
fn menu(question: &str, choices: &[Choice<'_>], default: usize) -> std::io::Result<Option<usize>> {
    enable_raw_mode()?;
    // Held for the rest of the function so raw mode is given back even if
    // drawing panics — a terminal left in raw mode is one the user's shell
    // stops echoing into, and they have no way to know why.
    let _raw = RawMode;

    // A blank line, the question, the choices, and the key hint.
    let height = u16::try_from(choices.len())
        .unwrap_or(u16::MAX)
        .saturating_add(3);
    let mut terminal = Terminal::with_options(
        CrosstermBackend::new(std::io::stderr()),
        TerminalOptions {
            viewport: Viewport::Inline(height),
        },
    )?;
    terminal.hide_cursor()?;

    let colour = colour();
    let mut cursor = default;
    let mut origin = Position::ORIGIN;
    let last = choices.len() - 1;

    let chosen = loop {
        terminal.draw(|frame| {
            origin = frame.area().as_position();
            frame.render_widget(
                Paragraph::new(render(question, choices, cursor, default, colour)),
                frame.area(),
            );
        })?;

        let Event::Key(key) = event::read()? else {
            continue;
        };
        // Windows reports the release too, and acting on both moves twice.
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match key.code {
            // Wrapping, because a list this short has no scrollback to get lost
            // in and stopping at the end is one keystroke of nothing happening.
            KeyCode::Up | KeyCode::Char('k') => {
                cursor = if cursor == 0 { last } else { cursor - 1 }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                cursor = if cursor == last { 0 } else { cursor + 1 }
            }
            KeyCode::Home => cursor = 0,
            KeyCode::End => cursor = last,
            // Somebody who already knows what they want should not have to
            // arrow to it. The digits are the same ones the list is numbered by.
            KeyCode::Char(digit @ '1'..='9') => {
                let index = digit as usize - '1' as usize;
                if index <= last {
                    cursor = index;
                }
            }
            KeyCode::Enter => break Some(cursor),
            KeyCode::Esc => break None,
            KeyCode::Char('c' | 'd') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                break None;
            }
            _ => {}
        }
    };

    // The menu has served its purpose and the answer is about to be printed
    // where it stood. Clearing from the viewport's own origin is what keeps the
    // transcript continuous rather than leaving a hole in it.
    terminal.clear()?;
    terminal.set_cursor_position(origin)?;
    terminal.show_cursor()?;
    ratatui::backend::Backend::flush(terminal.backend_mut())?;

    Ok(chosen)
}

/// The viewport's contents for one frame.
fn render<'a>(
    question: &'a str,
    choices: &'a [Choice<'a>],
    cursor: usize,
    default: usize,
    colour: bool,
) -> Vec<Line<'a>> {
    let accent = if colour {
        Style::new().fg(Color::Cyan)
    } else {
        Style::new()
    };
    let dim = if colour {
        Style::new().add_modifier(Modifier::DIM)
    } else {
        Style::new()
    };

    let width = choices.iter().map(|c| c.name.len()).max().unwrap_or(0);
    let mut lines = vec![
        Line::default(),
        Line::from(vec![
            Span::styled("? ", accent),
            Span::styled(question, Style::new().add_modifier(Modifier::BOLD)),
        ]),
    ];

    for (index, choice) in choices.iter().enumerate() {
        let selected = index == cursor;
        // The marker carries the selection on its own, so a terminal with no
        // colour — or somebody who cannot tell cyan from white — still reads it.
        let marker = if selected { "❯ " } else { "  " };
        let name = if selected {
            Style::new().add_modifier(Modifier::BOLD).patch(if colour {
                accent
            } else {
                Style::new()
            })
        } else {
            Style::new()
        };
        let mut spans = vec![
            Span::styled(marker, accent),
            Span::styled(format!("{:width$}", choice.name), name),
        ];
        if !choice.description.is_empty() {
            spans.push(Span::styled(format!("  {}", choice.description), dim));
        }
        if index == default {
            spans.push(Span::styled("  (default)", dim));
        }
        lines.push(Line::from(spans));
    }

    lines.push(Line::from(Span::styled(
        "  ↑/↓ move · 1-9 jump · enter select · esc cancel",
        dim,
    )));
    lines
}

/// The one line a question leaves behind once it has been answered.
fn answered(question: &str, name: &str) {
    if colour() {
        eprintln!("\x1b[36m✓\x1b[0m {question} \x1b[1;36m{name}\x1b[0m");
    } else {
        eprintln!("✓ {question} {name}");
    }
}

/// Raw mode, given back when this goes out of scope.
struct RawMode;

impl Drop for RawMode {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
    }
}

/// The question as a numbered list, for a terminal that would not be drawn on.
///
/// Everything here needs is a line of input, so it works wherever the menu does
/// not. An answer that is not an option is re-asked rather than resolved to
/// something nearby — a scaffolder writes a project, and a typo silently
/// producing the wrong one is worse than a second question.
fn numbered(question: &str, choices: &[Choice<'_>], default: usize) -> Option<usize> {
    let width = choices.iter().map(|c| c.name.len()).max().unwrap_or(0);

    loop {
        eprintln!("\n{question}");
        for (index, choice) in choices.iter().enumerate() {
            let marker = if index == default { " (default)" } else { "" };
            let line = format!(
                "  {}) {:width$}  {}{}",
                index + 1,
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
            return Some(default);
        };
        let answer = line.trim();
        if answer.is_empty() {
            return Some(default);
        }
        if let Some(found) = resolve(choices, answer) {
            return Some(found);
        }
        eprintln!("  `{answer}` is not one of them.");
    }
}

/// One typed answer as an index: by number, or by name.
fn resolve(choices: &[Choice<'_>], answer: &str) -> Option<usize> {
    if let Ok(number) = answer.parse::<usize>()
        && (1..=choices.len()).contains(&number)
    {
        return Some(number - 1);
    }
    choices
        .iter()
        .position(|choice| choice.name.eq_ignore_ascii_case(answer))
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
        assert_eq!(resolve(&choices, "API"), Some(1));
        assert_eq!(resolve(&choices, "1"), Some(0));
        assert_eq!(resolve(&choices, "3"), None);
        assert_eq!(resolve(&choices, "svelte"), None);
    }

    /// The frame is what somebody chooses from, so what is on it is worth
    /// asserting: every choice, the marker on exactly one of them, and the
    /// default named as such.
    #[test]
    fn the_menu_draws_every_choice_and_marks_one() {
        let choices = [
            Choice {
                name: "static",
                description: "no server",
            },
            Choice {
                name: "fullstack",
                description: "a server",
            },
        ];
        let lines = render("Which mode?", &choices, 1, 0, false);
        let text: Vec<String> = lines.iter().map(ToString::to_string).collect();

        assert!(text.iter().any(|line| line.contains("Which mode?")));
        assert!(text.iter().any(|line| line.contains("no server")));
        assert_eq!(text.iter().filter(|line| line.contains('❯')).count(), 1);
        assert!(
            text.iter()
                .find(|line| line.contains("fullstack"))
                .is_some_and(|line| line.contains('❯')),
            "the cursor is on the second choice: {text:?}"
        );
        assert!(
            text.iter()
                .find(|line| line.contains("static"))
                .is_some_and(|line| line.contains("(default)")),
            "the default is named: {text:?}"
        );
    }
}
