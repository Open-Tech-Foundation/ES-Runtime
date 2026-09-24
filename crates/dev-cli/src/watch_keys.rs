//! Keys for `esdev test --watch`: rerun, rerun what failed, filter, quit
//! (DECISIONS D118).
//!
//! # The terminal keeps printing as it did
//!
//! Keys are read one at a time, unechoed, by turning off only the terminal's
//! line editing (`ICANON`) and echo. Everything else is left as it was:
//! output processing, so what the tests print still starts each line at the
//! left edge; and signals, so ^C and ^Z are still the terminal's. A full raw
//! mode would take both, and a child writing `\n` would staircase across the
//! screen. The mode is given back when the watch ends, however it ends, and
//! taken again after ^Z and `fg`, when the shell has put its own back.
//!
//! # Only on a terminal
//!
//! Stdin that is not a terminal is left alone and the watch has no keys, as
//! before. `--_watch-keys` reads keys from a piped stdin anyway, which is how
//! the end-to-end tests press them.

use std::io::Read;

/// A key pressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Escape,
    Backspace,
}

/// The keys in one read. A terminal delivers an escape sequence — an arrow, a
/// function key — in one read, so a lone ESC is the Escape key and a longer
/// run starting with one is a key this has no use for.
pub fn parse(chunk: &[u8]) -> Vec<Key> {
    if chunk.first() == Some(&0x1b) {
        return if chunk.len() == 1 {
            vec![Key::Escape]
        } else {
            Vec::new()
        };
    }
    String::from_utf8_lossy(chunk)
        .chars()
        .filter_map(|c| match c {
            '\r' | '\n' => Some(Key::Enter),
            '\u{7f}' | '\u{8}' => Some(Key::Backspace),
            '\u{1b}' => Some(Key::Escape),
            c if c.is_control() => None,
            c => Some(Key::Char(c)),
        })
        .collect()
}

/// The terminal's mode before the watch changed it, put back when dropped.
pub struct Terminal {
    #[cfg(unix)]
    saved: Option<rustix::termios::Termios>,
}

impl Terminal {
    /// Reads keys unechoed, one at a time; `None` when stdin is not a terminal.
    #[cfg(unix)]
    fn take() -> Option<Terminal> {
        let stdin = std::io::stdin();
        let saved = rustix::termios::tcgetattr(&stdin).ok()?;
        let terminal = Terminal { saved: Some(saved) };
        terminal.apply().ok()?;
        Some(terminal)
    }

    #[cfg(not(unix))]
    fn take() -> Option<Terminal> {
        None
    }

    /// Keys unechoed and one at a time — again, after ^Z and `fg`.
    #[cfg(unix)]
    pub fn apply(&self) -> std::io::Result<()> {
        use rustix::termios::{LocalModes, OptionalActions, SpecialCodeIndex, tcsetattr};
        let Some(saved) = &self.saved else {
            return Ok(());
        };
        let mut keys = saved.clone();
        keys.local_modes
            .remove(LocalModes::ICANON | LocalModes::ECHO);
        keys.special_codes[SpecialCodeIndex::VMIN] = 1;
        keys.special_codes[SpecialCodeIndex::VTIME] = 0;
        tcsetattr(std::io::stdin(), OptionalActions::Now, &keys)?;
        Ok(())
    }

    #[cfg(not(unix))]
    pub fn apply(&self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(saved) = &self.saved {
            let _ = rustix::termios::tcsetattr(
                std::io::stdin(),
                rustix::termios::OptionalActions::Now,
                saved,
            );
        }
    }
}

/// Where keys come from, and the terminal mode that makes them keys.
pub struct Keys {
    pub rx: tokio::sync::mpsc::UnboundedReceiver<Key>,
    pub terminal: Option<Terminal>,
}

/// Starts reading keys: from a terminal, or with `forced` from any stdin.
/// `None` when there are none to read.
pub fn start(forced: bool) -> Option<Keys> {
    use std::io::IsTerminal;
    let terminal = if std::io::stdin().is_terminal() {
        Some(Terminal::take()?)
    } else if forced {
        None
    } else {
        return None;
    };
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    // A thread of its own, blocked on stdin for the life of the process: a
    // read cannot be cancelled, and nothing else reads this stdin.
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buffer = [0u8; 64];
        while let Ok(n) = stdin.read(&mut buffer) {
            if n == 0 {
                break;
            }
            for key in parse(&buffer[..n]) {
                if tx.send(key).is_err() {
                    return;
                }
            }
        }
    });
    Some(Keys { rx, terminal })
}

/// What a key asks the watch to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// `a`, Enter: clear the filters and run every test.
    RunAll,
    /// `r`: run again with the filters as they are.
    Rerun,
    /// `f`: run the files that failed last time.
    RerunFailed,
    /// `u`: run again, updating the snapshots that no longer match.
    UpdateSnapshots,
    /// `p`: ask for a filename filter.
    FilterFiles,
    /// `t`: ask for a test name pattern.
    FilterNames,
    Help,
    Quit,
}

impl Action {
    pub fn of(key: Key) -> Option<Action> {
        Some(match key {
            Key::Enter | Key::Char('a') => Action::RunAll,
            Key::Char('r') => Action::Rerun,
            Key::Char('f') => Action::RerunFailed,
            Key::Char('u') => Action::UpdateSnapshots,
            Key::Char('p') => Action::FilterFiles,
            Key::Char('t') => Action::FilterNames,
            Key::Char('h' | '?') => Action::Help,
            Key::Char('q') => Action::Quit,
            _ => return None,
        })
    }

    /// Whether it starts a run, and so cancels one in progress.
    pub fn runs(self) -> bool {
        matches!(
            self,
            Action::RunAll | Action::Rerun | Action::RerunFailed | Action::UpdateSnapshots
        )
    }
}

/// The help `h` prints, in Vitest's words where it has them.
pub fn help(paint: &crate::style::Palette) -> String {
    let rows = [
        ("a", "rerun all tests, clearing the filters (also Enter)"),
        ("r", "rerun with the current filters"),
        ("f", "rerun only the files that failed"),
        ("u", "rerun, updating snapshots that no longer match"),
        ("p", "filter by a filename"),
        ("t", "filter by a test name pattern"),
        ("q", "quit"),
    ];
    let mut out = format!("\n{}\n", paint.bold("Watch usage"));
    for (key, what) in rows {
        out.push_str(&format!(
            "  {} {} {}\n",
            paint.dim("press"),
            paint.bold(key),
            paint.dim(format!("to {what}"))
        ));
    }
    out
}

/// A line of input, drawn on stderr as it is typed: `Some` when Enter ends
/// it (empty to clear), `None` when Escape cancels it or input ends. `hint`
/// says what the text so far would select, beside it.
pub async fn prompt(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<Key>,
    label: &str,
    initial: &str,
    hint: impl Fn(&str) -> String,
) -> Option<String> {
    use std::io::Write;
    let paint = crate::style::Palette::stderr();
    let mut text = initial.to_string();
    let draw = |text: &str| {
        let hint = hint(text);
        let mut err = std::io::stderr();
        let _ = write!(err, "\r\x1b[2K{} {text}", paint.bold(label));
        if !hint.is_empty() {
            let shown = format!("  {hint}");
            // The cursor goes back to the end of the text, before the hint.
            let _ = write!(err, "{}\x1b[{}D", paint.dim(&shown), shown.chars().count());
        }
        let _ = err.flush();
    };
    draw(&text);
    let answer = loop {
        match rx.recv().await {
            None | Some(Key::Escape) => break None,
            Some(Key::Enter) => break Some(text.trim().to_string()),
            Some(Key::Backspace) => {
                text.pop();
            }
            Some(Key::Char(c)) => text.push(c),
        }
        draw(&text);
    };
    eprint!("\r\x1b[2K");
    answer
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_read_becomes_keys() {
        assert_eq!(parse(b"ab\n"), [Key::Char('a'), Key::Char('b'), Key::Enter]);
        assert_eq!(parse(b"\r"), [Key::Enter]);
        assert_eq!(parse(b"\x7f"), [Key::Backspace]);
        assert_eq!(parse("é".as_bytes()), [Key::Char('é')]);
        // A lone ESC is Escape; an arrow key is a sequence, and is nothing here.
        assert_eq!(parse(b"\x1b"), [Key::Escape]);
        assert_eq!(parse(b"\x1b[A"), []);
        // Other control characters are dropped.
        assert_eq!(parse(b"\x01x"), [Key::Char('x')]);
    }

    #[test]
    fn keys_are_vitests() {
        assert_eq!(Action::of(Key::Enter), Some(Action::RunAll));
        assert_eq!(Action::of(Key::Char('a')), Some(Action::RunAll));
        assert_eq!(Action::of(Key::Char('f')), Some(Action::RerunFailed));
        assert_eq!(Action::of(Key::Char('p')), Some(Action::FilterFiles));
        assert_eq!(Action::of(Key::Char('t')), Some(Action::FilterNames));
        assert_eq!(Action::of(Key::Char('x')), None);
        assert!(Action::Rerun.runs() && Action::UpdateSnapshots.runs());
        assert!(!Action::FilterFiles.runs() && !Action::Quit.runs());
    }
}
