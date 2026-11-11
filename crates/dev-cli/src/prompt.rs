//! Asking a question on a terminal.
//!
//! Arrow-key menus, a yes/no toggle and a line of text, drawn with ratatui into
//! an **inline viewport**, in Clack's look (`@clack/prompts`, which create-vite,
//! create-svelte and create-afw use): one gutter down the left joining a run's
//! questions, `◆` on the one being asked, `◇` on each answered one, `●`/`○` for
//! the options.
//!
//! ```text
//! ┌  esdev create
//! │
//! ◇  Which Template?
//! │  AFW
//! │
//! ◆  Add a blog? (posts with RSS and Atom feeds)
//! │  ○ Yes / ● No
//! └  ←/→ choose · enter confirm · esc back
//! ```
//!
//! The look is Clack's and the keys are this module's own: Esc steps back to
//! the question before, as it did before the restyle, and a digit jumps to an
//! option. So the closing line names the keys, which Clack leaves unsaid.
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
use std::sync::Mutex;

use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
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
///
/// Never in a unit test. `cargo test` run from a terminal inherits it, and a
/// test that reached a menu put that terminal into raw mode and waited for a
/// key nobody would press — CI never saw it, because CI sets `CI`. A test of
/// the questions drives them with a scripted answer instead.
pub fn interactive() -> bool {
    if cfg!(test) {
        return false;
    }
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
    /// One line, for somebody choosing. Shown beside the option the cursor is
    /// on, as Clack shows a hint.
    pub description: &'a str,
    /// Shown but not choosable — e.g. a package manager that is not installed.
    /// Arrow keys skip it, Enter on it does nothing, and typed answers naming
    /// it are re-asked rather than taken.
    pub disabled: bool,
}

/// What Esc means. The key always ends the question with no answer; what that
/// amounts to is the caller's flow to know, and the transcript says which.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OnEsc {
    /// There is nowhere back to go: the run stops, and the gutter closes on
    /// `Cancelled.`
    Cancel,
    /// An earlier question showed a menu: Esc returns to it.
    Back,
    /// The question is optional — installing a project already written — so
    /// Esc declines it, and the run goes on.
    Skip,
}

// --- the transcript ----------------------------------------------------------

const BAR: &str = "│";
const START: &str = "┌";
const END: &str = "└";
const ACTIVE: &str = "◆";
const DONE: &str = "◇";
const STOPPED: &str = "■";
const ON: &str = "●";
const OFF: &str = "○";

/// One command's run of questions: the title its gutter opens with, and
/// whether it has been opened. Opened by the first question actually drawn,
/// so a run answered entirely by flags prints none of it.
struct Session {
    title: Option<&'static str>,
    open: bool,
}

static SESSION: Mutex<Session> = Mutex::new(Session {
    title: None,
    open: false,
});

/// Names the run whose questions follow: `┌  esdev create`. Nothing is printed
/// until a question is.
pub fn intro(title: &'static str) {
    if let Ok(mut session) = SESSION.lock() {
        session.title = Some(title);
        session.open = false;
    }
}

/// Starts the gutter, once.
fn open() {
    let Ok(mut session) = SESSION.lock() else {
        return;
    };
    if session.open {
        return;
    }
    session.open = true;
    let paint = crate::style::Palette::stderr();
    match session.title {
        Some(title) => eprintln!("{}  {title}", paint.dim(START)),
        None => eprintln!("{}", paint.dim(START)),
    }
}

/// Whether a run's questions have drawn their gutter and not yet closed it.
pub fn is_open() -> bool {
    SESSION.lock().is_ok_and(|session| session.open)
}

/// A line of progress between questions: inside the gutter while one is open,
/// plain otherwise. An empty line is a gap in the gutter, and nothing at all
/// when there is no gutter.
pub fn log(text: &str) {
    if !is_open() {
        if !text.is_empty() {
            eprintln!("{text}");
        }
        return;
    }
    let paint = crate::style::Palette::stderr();
    if text.is_empty() {
        eprintln!("{}", paint.dim(BAR));
        return;
    }
    for line in text.lines() {
        if line.is_empty() {
            eprintln!("{}", paint.dim(BAR));
        } else {
            eprintln!("{}  {line}", paint.dim(BAR));
        }
    }
}

/// Closes the gutter, if a question opened it: `└  Done.`
pub fn outro(message: &str) {
    let Ok(mut session) = SESSION.lock() else {
        return;
    };
    if !session.open {
        return;
    }
    session.open = false;
    let paint = crate::style::Palette::stderr();
    eprintln!("{}\n{}  {message}", paint.dim(BAR), paint.dim(END));
}

/// What a question leaves behind once it has been answered.
fn answered(question: &str, answer: &str) {
    let paint = crate::style::Palette::stderr();
    eprintln!(
        "{}\n{}  {question}\n{}  {}",
        paint.dim(BAR),
        paint.green(DONE),
        paint.dim(BAR),
        paint.dim(answer)
    );
}

/// What a question Esc ended leaves behind, which depends on what Esc meant.
/// Stepping back leaves nothing: the earlier question is asked again below.
fn ended(question: &str, esc: OnEsc) {
    match esc {
        OnEsc::Back => {}
        OnEsc::Skip => answered(question, "Skipped"),
        OnEsc::Cancel => {
            if let Ok(mut session) = SESSION.lock() {
                session.open = false;
            }
            let paint = crate::style::Palette::stderr();
            eprintln!(
                "{}\n{STOPPED}  {question}\n{}  Cancelled.",
                paint.dim(BAR),
                paint.dim(END)
            );
        }
    }
}

/// The closing line's key hint.
fn esc_hint(esc: OnEsc) -> &'static str {
    match esc {
        OnEsc::Cancel => "esc cancel",
        OnEsc::Back => "esc back",
        OnEsc::Skip => "esc skip",
    }
}

/// The styles a frame is drawn with, or none at all without colour: the glyphs
/// carry every meaning on their own.
struct Styles {
    bar: Style,
    active: Style,
    on: Style,
    dim: Style,
    bold: Style,
}

impl Styles {
    fn new(colour: bool) -> Self {
        if !colour {
            return Self {
                bar: Style::new(),
                active: Style::new(),
                on: Style::new(),
                dim: Style::new(),
                bold: Style::new(),
            };
        }
        Self {
            bar: Style::new().fg(Color::Cyan),
            active: Style::new().fg(Color::Cyan),
            on: Style::new().fg(Color::Green),
            dim: Style::new().add_modifier(Modifier::DIM),
            bold: Style::new().add_modifier(Modifier::BOLD),
        }
    }
}

/// The first two lines of an asked question: the gutter, then `◆  question`.
fn heading<'a>(question: &'a str, styles: &Styles) -> Vec<Line<'a>> {
    vec![
        Line::from(Span::styled(BAR, styles.dim)),
        Line::from(vec![
            Span::styled(ACTIVE, styles.active),
            Span::raw("  "),
            Span::styled(question, styles.bold),
        ]),
    ]
}

/// The closing line of an asked question: `└` and the keys.
fn closing<'a>(hint: String, styles: &Styles) -> Line<'a> {
    Line::from(vec![
        Span::styled(END, styles.bar),
        Span::raw("  "),
        Span::styled(hint, styles.dim),
    ])
}

// --- select ------------------------------------------------------------------

/// Asks `question` and returns the index of the chosen option.
///
/// `None` is Esc, or ^C. It is not the same as taking the default, and callers
/// are expected to stop rather than to guess: somebody who pressed Esc at
/// "which template?" did not ask for the default template.
///
/// The default is `None` where the question has none: the cursor starts on the
/// first option, and choosing still means pressing enter on it. `Some` is
/// where the cursor starts, and what end of input resolves to.
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
    open();

    // A terminal that will not go into raw mode is not a terminal this can draw
    // on, and the question still has to be asked. The fallback is the plain
    // numbered list, which needs nothing but a line of input.
    let chosen = match menu(question, choices, default, esc) {
        Ok(chosen) => chosen,
        Err(_) => numbered(question, choices, default),
    };
    match chosen {
        Some(chosen) => {
            answered(question, choices[chosen].label);
            Some(chosen)
        }
        None => {
            ended(question, esc);
            None
        }
    }
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
    let mut inline = Inline::new(choices.len() + 3)?;
    let colour = colour();
    // A `None` default still starts somewhere: the first thing that can be
    // chosen. `select` guarantees at least one enabled choice above.
    let mut cursor = default
        .filter(|index| !choices[*index].disabled)
        .or_else(|| choices.iter().position(|choice| !choice.disabled))
        .unwrap_or(0);
    let last = choices.len() - 1;

    let chosen = loop {
        inline.draw(render(question, choices, cursor, esc, colour), None)?;
        let Some(key) = key()? else {
            continue;
        };
        if cancels(&key) {
            break None;
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
            // arrow to it: the digits count the options from the top. A digit
            // naming a disabled entry stays where it is rather than moving
            // somewhere that cannot be chosen.
            KeyCode::Char(digit @ '1'..='9') => {
                let index = digit as usize - '1' as usize;
                if index <= last && !choices[index].disabled {
                    cursor = index;
                }
            }
            // Enter on a disabled entry is not an answer: wait for one that is.
            KeyCode::Enter if !choices[cursor].disabled => break Some(cursor),
            _ => {}
        }
    };
    inline.finish()?;
    Ok(chosen)
}

/// The menu's frame.
///
/// Only the option under the cursor shows its description, as Clack shows a
/// hint: a list of templates reads as names, and the one being considered
/// says what it is. A disabled option always says so, since it is the reason
/// the cursor will not land on it.
fn render<'a>(
    question: &'a str,
    choices: &'a [Choice<'a>],
    cursor: usize,
    esc: OnEsc,
    colour: bool,
) -> Vec<Line<'a>> {
    let styles = Styles::new(colour);
    let mut lines = heading(question, &styles);
    for (index, choice) in choices.iter().enumerate() {
        let mut spans = vec![Span::styled(BAR, styles.bar), Span::raw("  ")];
        if choice.disabled {
            spans.push(Span::styled(
                format!("{OFF} {} (not available)", choice.label),
                styles.dim,
            ));
        } else if index == cursor {
            spans.push(Span::styled(ON, styles.on));
            spans.push(Span::raw(format!(" {}", choice.label)));
            if !choice.description.is_empty() {
                spans.push(Span::styled(
                    format!(" ({})", choice.description),
                    styles.dim,
                ));
            }
        } else {
            spans.push(Span::styled(format!("{OFF} {}", choice.label), styles.dim));
        }
        lines.push(Line::from(spans));
    }
    lines.push(closing(
        format!("↑/↓ move · enter select · {}", esc_hint(esc)),
        &styles,
    ));
    lines
}

// --- confirm -----------------------------------------------------------------

/// Asks a yes/no question, drawn as Clack draws one: `○ Yes / ● No` on a line.
///
/// `None` is Esc, or ^C; end of input is the default. `y` and `n` answer at
/// once, the arrows and Tab move between the two, and Enter takes the one
/// marked.
pub fn confirm(question: &str, default: bool, esc: OnEsc) -> Option<bool> {
    open();
    let answer = match toggle(question, default, esc) {
        Ok(answer) => answer,
        Err(_) => yes_no(question, default),
    };
    match answer {
        Some(yes) => {
            answered(question, if yes { "Yes" } else { "No" });
            Some(yes)
        }
        None => {
            ended(question, esc);
            None
        }
    }
}

fn toggle(question: &str, default: bool, esc: OnEsc) -> std::io::Result<Option<bool>> {
    let mut inline = Inline::new(4)?;
    let colour = colour();
    let mut yes = default;
    let answer = loop {
        inline.draw(render_confirm(question, yes, esc, colour), None)?;
        let Some(key) = key()? else {
            continue;
        };
        if cancels(&key) {
            break None;
        }
        match key.code {
            KeyCode::Left
            | KeyCode::Right
            | KeyCode::Up
            | KeyCode::Down
            | KeyCode::Tab
            | KeyCode::BackTab
            | KeyCode::Char('h' | 'l' | 'j' | 'k') => yes = !yes,
            KeyCode::Char('y' | 'Y') => break Some(true),
            KeyCode::Char('n' | 'N') => break Some(false),
            KeyCode::Enter => break Some(yes),
            _ => {}
        }
    };
    inline.finish()?;
    Ok(answer)
}

/// The yes/no frame.
fn render_confirm<'a>(question: &'a str, yes: bool, esc: OnEsc, colour: bool) -> Vec<Line<'a>> {
    let styles = Styles::new(colour);
    let option = |label: &'static str, marked: bool| -> Vec<Span<'a>> {
        if marked {
            vec![Span::styled(ON, styles.on), Span::raw(format!(" {label}"))]
        } else {
            vec![Span::styled(format!("{OFF} {label}"), styles.dim)]
        }
    };
    let mut row = vec![Span::styled(BAR, styles.bar), Span::raw("  ")];
    row.extend(option("Yes", yes));
    row.push(Span::styled(" / ", styles.dim));
    row.extend(option("No", !yes));
    let mut lines = heading(question, &styles);
    lines.push(Line::from(row));
    lines.push(closing(
        format!("←/→ choose · enter confirm · {}", esc_hint(esc)),
        &styles,
    ));
    lines
}

/// The yes/no question as a line of input, for a terminal that would not be
/// drawn on. Anything but an answer is re-asked.
fn yes_no(question: &str, default: bool) -> Option<bool> {
    loop {
        eprint!("\n{question} ({}) ", if default { "Y/n" } else { "y/N" });
        let _ = std::io::stderr().flush();
        let Some(line) = read_line() else {
            eprintln!();
            return Some(default);
        };
        match parse_yes_no(&line) {
            Some(answer) => return Some(answer.unwrap_or(default)),
            None => eprintln!("  answer y or n."),
        }
    }
}

/// A typed yes/no: `Some(None)` is an empty line, which takes the default, and
/// `None` is not an answer.
fn parse_yes_no(line: &str) -> Option<Option<bool>> {
    match line.trim().to_ascii_lowercase().as_str() {
        "" => Some(None),
        "y" | "yes" => Some(Some(true)),
        "n" | "no" => Some(Some(false)),
        _ => None,
    }
}

// --- text --------------------------------------------------------------------

/// A free-text answer with a default, shown as the placeholder until something
/// is typed.
///
/// Empty is the default and end of input is too — a closed stdin is not a
/// decision. Anything typed is an answer: the caller validates what needs
/// validating. Ask only when [`interactive`] says somebody is there; away from
/// a terminal the defaults decide.
///
/// Esc or ^C stops the command, as ^C at a line of input always has: the
/// terminal is given back, the gutter closes on `Cancelled.`, and the process
/// exits 130. There is no answer to return that would not be taken for one.
pub fn ask_text(question: &str, default: &str) -> Option<String> {
    open();
    match line_editor(question, default) {
        Ok(Some(answer)) => {
            answered(question, &answer);
            Some(answer)
        }
        Ok(None) => {
            ended(question, OnEsc::Cancel);
            std::process::exit(130);
        }
        Err(_) => plain_text(question, default),
    }
}

fn line_editor(question: &str, default: &str) -> std::io::Result<Option<String>> {
    let mut inline = Inline::new(4)?;
    let colour = colour();
    let mut text = String::new();
    let answer = loop {
        let (lines, column) = render_text(question, &text, default, colour);
        inline.draw(lines, Some((column, 2)))?;
        let Some(key) = key()? else {
            continue;
        };
        if cancels(&key) {
            break None;
        }
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('u') if control => text.clear(),
            KeyCode::Char(c) if !control => text.push(c),
            KeyCode::Backspace => {
                text.pop();
            }
            KeyCode::Enter => break Some(typed_or_default(&text, default)),
            _ => {}
        }
    };
    inline.finish()?;
    Ok(answer)
}

fn typed_or_default(text: &str, default: &str) -> String {
    let typed = text.trim();
    if typed.is_empty() {
        default.to_string()
    } else {
        typed.to_string()
    }
}

/// The text frame, and the column the cursor stands in: after what was typed,
/// or on the placeholder's first character while nothing has been.
fn render_text<'a>(
    question: &'a str,
    text: &'a str,
    default: &'a str,
    colour: bool,
) -> (Vec<Line<'a>>, u16) {
    let styles = Styles::new(colour);
    let mut lines = heading(question, &styles);
    let shown = if text.is_empty() {
        Span::styled(default, styles.dim)
    } else {
        Span::raw(text)
    };
    let column = 3 + if text.is_empty() {
        0
    } else {
        u16::try_from(Span::raw(text).width()).unwrap_or(u16::MAX - 3)
    };
    lines.push(Line::from(vec![
        Span::styled(BAR, styles.bar),
        Span::raw("  "),
        shown,
    ]));
    lines.push(closing("enter confirm · esc cancel".to_string(), &styles));
    (lines, column)
}

/// The text question as a line of input, for a terminal that would not be
/// drawn on.
fn plain_text(question: &str, default: &str) -> Option<String> {
    eprint!("\n{question} ({default}): ");
    let _ = std::io::stderr().flush();
    let line = read_line()?;
    Some(typed_or_default(&line, default))
}

// --- the terminal ------------------------------------------------------------

/// An inline viewport in raw mode, cleared away when the question is answered
/// so the answer can be written where it stood.
struct Inline {
    terminal: Terminal<CrosstermBackend<std::io::Stderr>>,
    origin: Position,
    // Held for as long as the viewport, so raw mode is given back even if
    // drawing panics — a terminal left in raw mode is one the user's shell
    // stops echoing into, and they have no way to know why. Declared last so
    // it is dropped last.
    _raw: RawMode,
}

impl Inline {
    fn new(height: usize) -> std::io::Result<Self> {
        enable_raw_mode()?;
        let raw = RawMode;
        let mut terminal = Terminal::with_options(
            CrosstermBackend::new(std::io::stderr()),
            TerminalOptions {
                viewport: Viewport::Inline(u16::try_from(height).unwrap_or(u16::MAX)),
            },
        )?;
        terminal.hide_cursor()?;
        Ok(Self {
            terminal,
            origin: Position::ORIGIN,
            _raw: raw,
        })
    }

    /// One frame. `cursor` is where the text cursor stands, relative to the
    /// viewport, or `None` to hide it.
    fn draw(&mut self, lines: Vec<Line<'_>>, cursor: Option<(u16, u16)>) -> std::io::Result<()> {
        let origin = &mut self.origin;
        self.terminal.draw(|frame| {
            let area = frame.area();
            *origin = area.as_position();
            frame.render_widget(Paragraph::new(lines), area);
            if let Some((x, y)) = cursor {
                frame.set_cursor_position(Position::new(area.x + x, area.y + y));
            }
        })?;
        Ok(())
    }

    /// Clears the viewport from its own origin, which is what keeps the
    /// transcript continuous rather than leaving a hole in it.
    fn finish(mut self) -> std::io::Result<()> {
        self.terminal.clear()?;
        self.terminal.set_cursor_position(self.origin)?;
        self.terminal.show_cursor()?;
        ratatui::backend::Backend::flush(self.terminal.backend_mut())
    }
}

/// The next key pressed, or `None` for any other event. Windows reports the
/// release too, and acting on both moves twice.
fn key() -> std::io::Result<Option<KeyEvent>> {
    match event::read()? {
        Event::Key(key) if key.kind == KeyEventKind::Press => Ok(Some(key)),
        _ => Ok(None),
    }
}

/// Esc, ^C or ^D: the question ends with no answer.
fn cancels(key: &KeyEvent) -> bool {
    key.code == KeyCode::Esc
        || (key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c' | 'd')))
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

    fn text(lines: &[Line<'_>]) -> Vec<String> {
        lines.iter().map(ToString::to_string).collect()
    }

    /// The frame is what somebody chooses from, in Clack's shape: the gutter,
    /// `◆` and the question, one `●` on the option under the cursor and `○` on
    /// the rest, and `└` closing on the keys.
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
        let text = text(&render("Which Mode?", &choices, 1, OnEsc::Cancel, false));

        assert_eq!(
            text,
            [
                "│",
                "◆  Which Mode?",
                "│  ○ Static",
                "│  ● FullStack (a server)",
                "└  ↑/↓ move · enter select · esc cancel",
            ],
        );
    }

    /// Esc says what it does where it is asked: back, cancel, or skip.
    #[test]
    fn the_closing_line_names_what_esc_does() {
        let choices = [Choice {
            name: "spa",
            label: "SPA",
            description: "",
            disabled: false,
        }];
        for (esc, hint) in [
            (OnEsc::Back, "esc back"),
            (OnEsc::Cancel, "esc cancel"),
            (OnEsc::Skip, "esc skip"),
        ] {
            let text = text(&render("Which Template?", &choices, 0, esc, false));
            assert!(
                text.last().is_some_and(|line| line.ends_with(hint)),
                "{text:?}"
            );
            assert_eq!(text[2], "│  ● SPA", "no description, no hint: {text:?}");
        }
    }

    /// A yes/no is one line, the marked answer filled.
    #[test]
    fn a_confirm_marks_one_answer() {
        let no = text(&render_confirm("Add a blog?", false, OnEsc::Back, false));
        assert_eq!(
            no,
            [
                "│",
                "◆  Add a blog?",
                "│  ○ Yes / ● No",
                "└  ←/→ choose · enter confirm · esc back",
            ],
        );
        let yes = text(&render_confirm("Add a blog?", true, OnEsc::Back, false));
        assert_eq!(yes[2], "│  ● Yes / ○ No");
    }

    /// A typed yes/no on a terminal that cannot be drawn on.
    #[test]
    fn a_typed_yes_or_no() {
        assert_eq!(parse_yes_no("y\n"), Some(Some(true)));
        assert_eq!(parse_yes_no(" YES "), Some(Some(true)));
        assert_eq!(parse_yes_no("n"), Some(Some(false)));
        assert_eq!(parse_yes_no(""), Some(None));
        assert_eq!(parse_yes_no("maybe"), None);
    }

    /// The default is the placeholder until something is typed, and the cursor
    /// stands after what was typed.
    #[test]
    fn a_text_question_shows_its_default_until_typed_over() {
        let (empty, column) = render_text("Project name", "", "my-app", false);
        assert_eq!(text(&empty)[2], "│  my-app");
        assert_eq!(column, 3);
        let (typed, column) = render_text("Project name", "shop", "my-app", false);
        assert_eq!(text(&typed)[2], "│  shop");
        assert_eq!(column, 7);
        assert_eq!(typed_or_default("  ", "my-app"), "my-app");
        assert_eq!(typed_or_default(" shop ", "my-app"), "shop");
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

    /// A disabled option says so, and never takes the marker — even under the
    /// cursor, since Enter would not take it.
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
        let text = text(&render(
            "Which Package Manager?",
            &choices,
            1,
            OnEsc::Cancel,
            false,
        ));
        assert_eq!(text[2], "│  ○ npm");
        assert_eq!(text[3], "│  ○ bun (not available)");
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
