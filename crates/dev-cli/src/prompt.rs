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
    /// The value, and what a flag would spell. Lowercase, always.
    pub name: &'a str,
    /// How the value reads in the menu: `SPA`, not `spa`.
    pub label: &'a str,
    /// One line, for somebody choosing.
    pub description: &'a str,
    /// Shown but not choosable — e.g. a package manager that is not installed.
    /// Arrow keys skip it, Enter on it does nothing, and typed answers naming
    /// it are re-asked rather than taken.
    pub disabled: bool,
}

/// What Esc means in a [`select`]. The key always ends the question with no
/// answer; whether that steps back or cancels is the caller's flow to know.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OnEsc {
    /// There is nowhere back to go: the run stops.
    Cancel,
    /// An earlier question showed a menu: Esc returns to it.
    Back,
}

/// Asks `question` and returns the index of the chosen option.
///
/// `None` is a deliberate cancel — Esc, or ^C. It is not the same as taking the
/// default, and callers are expected to stop rather than to guess: somebody who
/// pressed Esc at "which template?" did not ask for the default template.
///
/// The default is `None` where the question has none: no option is marked, the
/// cursor starts on the first one, and choosing still means pressing enter on
/// it. `Some` marks the option and is what end of input resolves to.
///
/// A default naming a disabled option falls back to the first enabled one, and
/// a question with nothing enabled is a cancel without drawing anything.
///
/// End of input *is* the default: a closed stdin is not a decision, and looping
/// on it would hang exactly where this is written not to. With no default it
/// is a cancel instead, for the same reason.
pub fn select(
    question: &str,
    choices: &[Choice<'_>],
    default: Option<usize>,
    esc: OnEsc,
) -> Option<usize> {
    if choices.is_empty() || choices.iter().all(|choice| choice.disabled) {
        return None;
    }
    let default = normalize_default(choices, default);

    // A terminal that will not go into raw mode is not a terminal this can draw
    // on, and the question still has to be asked. The fallback is the plain
    // numbered list, which needs nothing but a line of input.
    let chosen = match menu(question, choices, default, esc) {
        Ok(chosen) => chosen,
        Err(_) => numbered(question, choices, default),
    }?;

    answered(question, choices[chosen].label);
    Some(chosen)
}

/// The default with a disabled choice moved to the first enabled one, or `None`
/// when the question names no default. Pure, so the fallback tests directly.
fn normalize_default(choices: &[Choice<'_>], default: Option<usize>) -> Option<usize> {
    let default = default.map(|default| default.min(choices.len() - 1));
    match default {
        Some(index) if !choices[index].disabled => Some(index),
        _ => choices.iter().position(|choice| !choice.disabled),
    }
}

/// The next enabled option from `cursor` in `direction` (+1 down, -1 up),
/// wrapping. `None` when nothing is enabled — the caller then has nothing to
/// move to.
fn step_enabled(choices: &[Choice<'_>], cursor: usize, direction: isize) -> Option<usize> {
    if choices.iter().all(|choice| choice.disabled) {
        return None;
    }
    let len = choices.len();
    let mut next = cursor;
    for _ in 0..len {
        next = (next as isize + direction).rem_euclid(len as isize) as usize;
        if !choices[next].disabled {
            return Some(next);
        }
    }
    None
}

/// The menu, drawn and driven. `Err` means the terminal would not cooperate.
fn menu(
    question: &str,
    choices: &[Choice<'_>],
    default: Option<usize>,
    esc: OnEsc,
) -> std::io::Result<Option<usize>> {
    enable_raw_mode()?;
    // Held for the rest of the function so raw mode is given back even if
    // drawing panics — a terminal left in raw mode is one the user's shell
    // stops echoing into, and they have no way to know why.
    let _raw = RawMode;

    // A blank line, the question, the choices, a breath before the key hint.
    let height = u16::try_from(choices.len())
        .unwrap_or(u16::MAX)
        .saturating_add(4);
    let mut terminal = Terminal::with_options(
        CrosstermBackend::new(std::io::stderr()),
        TerminalOptions {
            viewport: Viewport::Inline(height),
        },
    )?;
    terminal.hide_cursor()?;

    let colour = colour();
    // A `None` default still starts somewhere: the first thing that can be
    // chosen. `select` guarantees at least one enabled choice above.
    let mut cursor = default
        .filter(|index| !choices[*index].disabled)
        .or_else(|| choices.iter().position(|choice| !choice.disabled))
        .unwrap_or(0);
    let mut origin = Position::ORIGIN;
    let last = choices.len() - 1;

    let chosen = loop {
        terminal.draw(|frame| {
            origin = frame.area().as_position();
            frame.render_widget(
                Paragraph::new(render(question, choices, cursor, default, esc, colour)),
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
            // Disabled entries are skipped, so choosing means landing on one
            // that can be taken.
            KeyCode::Up | KeyCode::Char('k') => {
                if let Some(next) = step_enabled(choices, cursor, -1) {
                    cursor = next;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let Some(next) = step_enabled(choices, cursor, 1) {
                    cursor = next;
                }
            }
            KeyCode::Home => {
                if let Some(first) = choices.iter().position(|choice| !choice.disabled) {
                    cursor = first;
                }
            }
            KeyCode::End => {
                if let Some(last) = choices.iter().rposition(|choice| !choice.disabled) {
                    cursor = last;
                }
            }
            // Somebody who already knows what they want should not have to
            // arrow to it. The digits are the same ones the list is numbered by.
            // A digit naming a disabled entry stays where it is rather than
            // moving somewhere that cannot be chosen.
            KeyCode::Char(digit @ '1'..='9') => {
                let index = digit as usize - '1' as usize;
                if index <= last && !choices[index].disabled {
                    cursor = index;
                }
            }
            // Enter on a disabled entry is not an answer: wait for one that is.
            KeyCode::Enter => {
                if !choices[cursor].disabled {
                    break Some(cursor);
                }
            }
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
    default: Option<usize>,
    esc: OnEsc,
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

    let width = choices.iter().map(|c| c.label.len()).max().unwrap_or(0);
    let mut lines = vec![
        Line::default(),
        Line::from(vec![
            Span::styled("? ", accent),
            Span::styled(question, Style::new().add_modifier(Modifier::BOLD)),
        ]),
    ];

    for (index, choice) in choices.iter().enumerate() {
        let selected = index == cursor && !choice.disabled;
        // The marker carries the selection on its own, so a terminal with no
        // colour — or somebody who cannot tell cyan from white — still reads it.
        // A disabled entry never takes the marker, even under the cursor: it
        // cannot be chosen, so it must not read as the thing Enter would take.
        let marker = if selected { "❯ " } else { "  " };
        let name = if choice.disabled {
            dim
        } else if selected {
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
            Span::styled(format!("{:width$}", choice.label), name),
        ];
        if !choice.description.is_empty() {
            spans.push(Span::styled(format!("  {}", choice.description), dim));
        }
        if choice.disabled {
            spans.push(Span::styled("  (not available)", dim));
        } else if Some(index) == default {
            spans.push(Span::styled("  (default)", dim));
        }
        lines.push(Line::from(spans));
    }

    lines.push(Line::default());
    let esc_hint = match esc {
        OnEsc::Cancel => "esc cancel",
        OnEsc::Back => "esc back",
    };
    lines.push(Line::from(Span::styled(
        format!("  ↑/↓ move · 1-9 jump · enter select · {esc_hint}"),
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
fn numbered(question: &str, choices: &[Choice<'_>], default: Option<usize>) -> Option<usize> {
    let width = choices.iter().map(|c| c.label.len()).max().unwrap_or(0);

    loop {
        eprintln!("\n{question}");
        for (index, choice) in choices.iter().enumerate() {
            let mut marker = String::new();
            if choice.disabled {
                marker.push_str(" (not available)");
            } else if Some(index) == default {
                marker.push_str(" (default)");
            }
            let line = format!(
                "  {}) {:width$}  {}{}",
                index + 1,
                choice.label,
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
        if let Some(found) = resolve(choices, answer) {
            return Some(found);
        }
        if find(choices, answer).is_some() {
            eprintln!("  `{answer}` is not available.");
        } else {
            eprintln!("  `{answer}` is not one of them.");
        }
    }
}

/// One typed answer as an index, wherever it sits: by number, by flag spelling,
/// or by the label the menu showed — `spa`, `SPA` and `1` all reach the same
/// starter. Disabled entries match here so the caller can tell "not available"
/// from "not one of them"; [`resolve`] filters them back out.
fn find(choices: &[Choice<'_>], answer: &str) -> Option<usize> {
    if let Ok(number) = answer.parse::<usize>()
        && (1..=choices.len()).contains(&number)
    {
        return Some(number - 1);
    }
    choices.iter().position(|choice| {
        choice.name.eq_ignore_ascii_case(answer) || choice.label.eq_ignore_ascii_case(answer)
    })
}

/// One typed answer as an index: by number, by flag spelling, or by the label
/// the menu showed — `spa`, `SPA` and `1` all reach the same starter.
///
/// A disabled entry never resolves: naming one is re-asked, not taken.
fn resolve(choices: &[Choice<'_>], answer: &str) -> Option<usize> {
    find(choices, answer).filter(|index| !choices[*index].disabled)
}

/// A free-text answer with a default, npm-init style.
///
/// Empty is the default and end of input is too — a closed stdin is not a
/// decision. Unlike [`select`], anything typed is an answer: the caller
/// validates what needs validating. Ask only when [`interactive`] says
/// somebody is there; away from a terminal the defaults decide.
pub fn ask_text(question: &str, default: &str) -> Option<String> {
    eprint!("\n{question} ({default}): ");
    let _ = std::io::stderr().flush();
    let line = read_line()?;
    let answer = line.trim();
    Some(if answer.is_empty() {
        default.to_string()
    } else {
        answer.to_string()
    })
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
                label: "ReactJS",
                description: "",
                disabled: false,
            },
            Choice {
                name: "api",
                label: "API",
                description: "",
                disabled: false,
            },
        ];
        assert_eq!(resolve(&choices, "API"), Some(1));
        assert_eq!(resolve(&choices, "api"), Some(1));
        assert_eq!(resolve(&choices, "ReactJS"), Some(0));
        assert_eq!(resolve(&choices, "1"), Some(0));
        assert_eq!(resolve(&choices, "3"), None);
        assert_eq!(resolve(&choices, "svelte"), None);
    }

    /// The frame is what somebody chooses from, so what is on it is worth
    /// asserting: every choice, the marker on exactly one of them, the
    /// default named as such — and a breath between the choices and the keys.
    #[test]
    fn the_menu_draws_every_choice_and_marks_one() {
        let choices = [
            Choice {
                name: "static",
                label: "Static",
                description: "no server",
                disabled: false,
            },
            Choice {
                name: "fullstack",
                label: "FullStack",
                description: "a server",
                disabled: false,
            },
        ];
        let lines = render("Which Mode?", &choices, 1, Some(0), OnEsc::Cancel, false);
        let text: Vec<String> = lines.iter().map(ToString::to_string).collect();

        assert!(text.iter().any(|line| line.contains("Which Mode?")));
        assert!(text.iter().any(|line| line.contains("no server")));
        assert_eq!(text.iter().filter(|line| line.contains('❯')).count(), 1);
        assert!(
            text.iter()
                .find(|line| line.contains("FullStack"))
                .is_some_and(|line| line.contains('❯')),
            "the cursor is on the second choice: {text:?}"
        );
        assert!(
            !text.iter().any(|line| line.contains("fullstack ")),
            "the flag spelling is not what is shown: {text:?}"
        );
        assert!(
            text.iter()
                .find(|line| line.contains("Static"))
                .is_some_and(|line| line.contains("(default)")),
            "the default is named: {text:?}"
        );
        let hint = text
            .iter()
            .position(|line| line.contains("enter select"))
            .expect("the key hint is drawn");
        assert!(
            text[hint - 1].trim().is_empty(),
            "the hint stands apart from the choices: {text:?}"
        );
    }

    /// A question with no default marks nothing and still starts somewhere —
    /// choosing means pressing enter on it, explicitly.
    #[test]
    fn a_menu_without_a_default_marks_nothing() {
        let choices = [
            Choice {
                name: "spa",
                label: "SPA",
                description: "an app",
                disabled: false,
            },
            Choice {
                name: "docs",
                label: "Docs",
                description: "a site",
                disabled: false,
            },
        ];
        let lines = render("Which Template?", &choices, 0, None, OnEsc::Back, false);
        let text: Vec<String> = lines.iter().map(ToString::to_string).collect();

        assert!(
            !text.iter().any(|line| line.contains("(default)")),
            "nothing is the default: {text:?}"
        );
        assert!(
            text.iter()
                .find(|line| line.contains("SPA"))
                .is_some_and(|line| line.contains('❯')),
            "the cursor still starts on the first choice: {text:?}"
        );
        assert!(
            text.iter().any(|line| line.contains("esc back")),
            "going back is what Esc does here: {text:?}"
        );
        assert!(
            !text.iter().any(|line| line.contains("esc cancel")),
            "cancel is not on offer where back is: {text:?}"
        );
    }

    /// Where there is nowhere back to go, Esc says cancel.
    #[test]
    fn the_first_menu_offers_cancel() {
        let choices = [Choice {
            name: "spa",
            label: "SPA",
            description: "an app",
            disabled: false,
        }];
        let lines = render("Which Template?", &choices, 0, None, OnEsc::Cancel, false);
        let text: Vec<String> = lines.iter().map(ToString::to_string).collect();

        assert!(
            text.iter().any(|line| line.contains("esc cancel")),
            "cancel is what Esc does here: {text:?}"
        );
    }

    /// A disabled entry is shown but cannot be taken: the typed answer does not
    /// resolve, while the enabled neighbour still does by number, name and label.
    #[test]
    fn disabled_choices_do_not_resolve() {
        let choices = [
            Choice {
                name: "npm",
                label: "npm",
                description: "installed",
                disabled: false,
            },
            Choice {
                name: "bun",
                label: "bun",
                description: "not installed",
                disabled: true,
            },
        ];
        assert_eq!(resolve(&choices, "npm"), Some(0));
        assert_eq!(resolve(&choices, "1"), Some(0));
        assert_eq!(resolve(&choices, "bun"), None);
        assert_eq!(resolve(&choices, "2"), None);
        // The name still matches, so the numbered fallback can tell "not
        // available" from "not one of them".
        assert_eq!(find(&choices, "bun"), Some(1));
        assert_eq!(find(&choices, "svelte"), None);
    }

    /// A default naming a disabled entry moves to the first enabled one, so
    /// Enter never lands somewhere that cannot be chosen.
    #[test]
    fn a_disabled_default_falls_back_to_the_first_enabled_choice() {
        let choices = [
            Choice {
                name: "npm",
                label: "npm",
                description: "",
                disabled: true,
            },
            Choice {
                name: "bun",
                label: "bun",
                description: "",
                disabled: false,
            },
        ];
        assert_eq!(normalize_default(&choices, Some(0)), Some(1));
        assert_eq!(normalize_default(&choices, Some(1)), Some(1));
        assert_eq!(normalize_default(&choices, None), Some(1));
    }

    /// Arrow keys skip what cannot be chosen, wrapping past the ends.
    #[test]
    fn stepping_skips_disabled_choices() {
        let choices = [
            Choice {
                name: "npm",
                label: "npm",
                description: "",
                disabled: false,
            },
            Choice {
                name: "bun",
                label: "bun",
                description: "",
                disabled: true,
            },
            Choice {
                name: "pnpm",
                label: "pnpm",
                description: "",
                disabled: false,
            },
        ];
        assert_eq!(step_enabled(&choices, 0, 1), Some(2));
        assert_eq!(step_enabled(&choices, 2, 1), Some(0));
        assert_eq!(step_enabled(&choices, 0, -1), Some(2));
    }

    /// The frame marks what cannot be chosen and never hands it the marker or
    /// the default — Enter must read as taking something enabled.
    #[test]
    fn the_menu_marks_disabled_choices_as_not_available() {
        let choices = [
            Choice {
                name: "npm",
                label: "npm",
                description: "installed",
                disabled: false,
            },
            Choice {
                name: "bun",
                label: "bun",
                description: "not installed",
                disabled: true,
            },
        ];
        let lines = render(
            "Which Package Manager?",
            &choices,
            1,
            Some(1),
            OnEsc::Cancel,
            false,
        );
        let text: Vec<String> = lines.iter().map(ToString::to_string).collect();

        let bun = text
            .iter()
            .find(|line| line.contains("bun"))
            .expect("the disabled entry is drawn");
        assert!(
            bun.contains("(not available)"),
            "a disabled entry says so: {text:?}"
        );
        assert!(
            !bun.contains('❯'),
            "a disabled entry never takes the marker: {text:?}"
        );
        assert!(
            !bun.contains("(default)"),
            "a disabled entry is never the default: {text:?}"
        );
    }

    /// Nothing enabled is a cancel without drawing: there is nowhere to move.
    #[test]
    fn a_question_with_nothing_enabled_is_a_cancel() {
        let choices = [Choice {
            name: "npm",
            label: "npm",
            description: "",
            disabled: true,
        }];
        assert_eq!(select("Which?", &choices, Some(0), OnEsc::Cancel), None);
        assert_eq!(normalize_default(&choices, Some(0)), None);
        assert_eq!(step_enabled(&choices, 0, 1), None);
    }
}
