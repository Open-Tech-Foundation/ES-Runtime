//! The process's own standard input (DECISIONS D143): **one reader, one
//! buffer**.
//!
//! A process has one standard input, and it is read by three kinds of caller:
//! `stdin.readable` wants whatever has arrived, `stdin.lines()` and
//! `question()` want a line and may wait for it, and the web's `prompt()` wants
//! a line and blocks the agent until it comes. If each of them read the
//! descriptor itself, a chunk read by one would be gone for the others — a
//! `question()` that read two lines would silently eat the second. So a single
//! thread reads, into a buffer every caller takes from, and bytes read past a
//! line end stay there for whoever asks next.
//!
//! The thread starts on the first read, not before: a program that never reads
//! its input leaves it untouched for a child it hands the terminal to. It is a
//! thread of its own rather than tokio's blocking pool, because a read that
//! never returns — the terminal just sits there — must not hold up the
//! runtime's shutdown, and the blocking pool is joined at shutdown.

use std::io::Read;
use std::sync::{Condvar, Mutex, Once, OnceLock};

/// What has arrived and not been taken.
#[derive(Default)]
struct Buffer {
    bytes: Vec<u8>,
    /// The reader saw the end of input. What is left in `bytes` is still
    /// handed out before the end is reported.
    ended: bool,
    /// The reader failed. Reported to every caller after what was buffered.
    failed: Option<String>,
    /// Raw mode, where Enter sends `\r` and no `\n` follows it.
    raw: bool,
}

/// One attempt to take from the buffer.
#[derive(Debug, PartialEq, Eq)]
enum Taken {
    Bytes(Vec<u8>),
    End,
    Failed(String),
    /// Nothing to hand out yet: wait for the reader.
    Wait,
}

impl Buffer {
    /// Everything buffered.
    fn take_chunk(&mut self) -> Taken {
        if !self.bytes.is_empty() {
            return Taken::Bytes(std::mem::take(&mut self.bytes));
        }
        self.finished()
    }

    /// One line, terminator included — `\n`, or in raw mode `\r` as well. At
    /// the end of input, whatever is left is the last line.
    fn take_line(&mut self) -> Taken {
        let raw = self.raw;
        if let Some(at) = self
            .bytes
            .iter()
            .position(|&b| b == b'\n' || (raw && b == b'\r'))
        {
            return Taken::Bytes(self.bytes.drain(..=at).collect());
        }
        if (self.ended || self.failed.is_some()) && !self.bytes.is_empty() {
            return Taken::Bytes(std::mem::take(&mut self.bytes));
        }
        self.finished()
    }

    fn finished(&self) -> Taken {
        if let Some(failed) = &self.failed {
            return Taken::Failed(failed.clone());
        }
        if self.ended {
            return Taken::End;
        }
        Taken::Wait
    }
}

struct Hub {
    buffer: Mutex<Buffer>,
    /// For the blocking reader: `prompt()` waits on the agent's own thread.
    arrived: Condvar,
    /// For the async readers.
    notify: tokio::sync::Notify,
    started: Once,
}

fn hub() -> &'static Hub {
    static HUB: OnceLock<Hub> = OnceLock::new();
    HUB.get_or_init(|| Hub {
        buffer: Mutex::new(Buffer::default()),
        arrived: Condvar::new(),
        notify: tokio::sync::Notify::new(),
        started: Once::new(),
    })
}

fn lock(hub: &Hub) -> std::sync::MutexGuard<'_, Buffer> {
    hub.buffer.lock().unwrap_or_else(|e| e.into_inner())
}

/// Starts the reader thread, once.
fn start(hub: &'static Hub) {
    hub.started.call_once(|| {
        let spawned = std::thread::Builder::new()
            .name("stdin".to_string())
            .spawn(move || read_until_end(hub));
        if let Err(e) = spawned {
            lock(hub).failed = Some(format!("cannot read standard input: {e}"));
        }
    });
}

fn read_until_end(hub: &'static Hub) {
    let mut stdin = std::io::stdin();
    let mut chunk = vec![0u8; 8192];
    loop {
        let read = stdin.read(&mut chunk);
        {
            let mut buffer = lock(hub);
            match read {
                Ok(0) => buffer.ended = true,
                Ok(n) => buffer.bytes.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => buffer.failed = Some(format!("cannot read standard input: {e}")),
            }
        }
        hub.arrived.notify_all();
        hub.notify.notify_waiters();
        let buffer = lock(hub);
        if buffer.ended || buffer.failed.is_some() {
            return;
        }
    }
}

/// The next chunk, or with `line` the next line; `None` at the end of input.
pub(crate) async fn read(line: bool) -> Result<Option<Vec<u8>>, String> {
    let hub = hub();
    start(hub);
    loop {
        // Registered before the buffer is looked at, so bytes that arrive
        // between the look and the wait still wake this caller.
        let notified = hub.notify.notified();
        let mut notified = std::pin::pin!(notified);
        notified.as_mut().enable();
        let taken = {
            let mut buffer = lock(hub);
            if line {
                buffer.take_line()
            } else {
                buffer.take_chunk()
            }
        };
        match taken {
            Taken::Bytes(bytes) => return Ok(Some(bytes)),
            Taken::End => return Ok(None),
            Taken::Failed(e) => return Err(e),
            Taken::Wait => notified.await,
        }
    }
}

/// The next line, blocking this thread until it arrives.
pub(crate) fn read_line_blocking() -> Result<Option<Vec<u8>>, String> {
    let hub = hub();
    start(hub);
    let mut buffer = lock(hub);
    loop {
        match buffer.take_line() {
            Taken::Bytes(bytes) => return Ok(Some(bytes)),
            Taken::End => return Ok(None),
            Taken::Failed(e) => return Err(e),
            Taken::Wait => {
                buffer = hub.arrived.wait(buffer).unwrap_or_else(|e| e.into_inner());
            }
        }
    }
}

/// The terminal settings raw mode replaced, to put back.
#[cfg(unix)]
fn saved() -> &'static Mutex<Option<rustix::termios::Termios>> {
    static SAVED: OnceLock<Mutex<Option<rustix::termios::Termios>>> = OnceLock::new();
    SAVED.get_or_init(|| Mutex::new(None))
}

/// Raw mode on or off.
///
/// The settings are libuv's `UV_TTY_MODE_RAW`, which is what Node's
/// `setRawMode` uses, rather than `cfmakeraw`: input is unprocessed and
/// unechoed, one byte at a time, with no signal keys — but **output**
/// processing is left on, so a `console.log` in raw mode still starts its next
/// line at column 0 instead of drawing a staircase.
#[cfg(unix)]
pub(crate) fn set_raw(raw: bool) -> Result<(), String> {
    use rustix::termios::{
        ControlModes, InputModes, LocalModes, OptionalActions, OutputModes, SpecialCodeIndex,
        tcgetattr, tcsetattr,
    };
    let stdin = std::io::stdin();
    let mut saved = saved().lock().unwrap_or_else(|e| e.into_inner());
    if raw {
        if saved.is_none() {
            let original = tcgetattr(&stdin).map_err(|_| {
                "setRawMode needs a terminal, and standard input is not one".to_string()
            })?;
            let mut settings = original.clone();
            settings.input_modes -= InputModes::BRKINT
                | InputModes::ICRNL
                | InputModes::INPCK
                | InputModes::ISTRIP
                | InputModes::IXON;
            settings.output_modes |= OutputModes::ONLCR;
            settings.control_modes |= ControlModes::CS8;
            settings.local_modes -=
                LocalModes::ECHO | LocalModes::ICANON | LocalModes::IEXTEN | LocalModes::ISIG;
            settings.special_codes[SpecialCodeIndex::VMIN] = 1;
            settings.special_codes[SpecialCodeIndex::VTIME] = 0;
            tcsetattr(&stdin, OptionalActions::Now, &settings)
                .map_err(|e| format!("cannot put the terminal into raw mode: {e}"))?;
            *saved = Some(original);
        }
    } else if let Some(original) = saved.take() {
        tcsetattr(&stdin, OptionalActions::Now, &original)
            .map_err(|e| format!("cannot restore the terminal: {e}"))?;
    }
    lock(hub()).raw = raw;
    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn set_raw(raw: bool) -> Result<(), String> {
    if !raw {
        return Ok(());
    }
    Err("setRawMode is not supported on this platform yet".to_string())
}

/// Gives the terminal back, if raw mode took it.
///
/// The CLI calls this on every way out of a run — a normal end, an uncaught
/// error, `exit()`, a signal-driven shutdown — because a shell left in raw mode
/// does not echo what is typed into it, and the person at it has no idea why.
pub fn restore_terminal() {
    #[cfg(unix)]
    {
        let mut saved = saved().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(original) = saved.take() {
            let _ = rustix::termios::tcsetattr(
                std::io::stdin(),
                rustix::termios::OptionalActions::Now,
                &original,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(bytes: &[u8]) -> Buffer {
        Buffer {
            bytes: bytes.to_vec(),
            ..Buffer::default()
        }
    }

    /// A line is taken with its terminator, and what follows stays for the
    /// next caller — the property that lets `question()` and `lines()` share
    /// one stream without losing input.
    #[test]
    fn a_line_leaves_the_rest_for_the_next_reader() {
        let mut b = buffer(b"one\ntwo\nthr");
        assert_eq!(b.take_line(), Taken::Bytes(b"one\n".to_vec()));
        assert_eq!(b.take_chunk(), Taken::Bytes(b"two\nthr".to_vec()));
        assert_eq!(b.take_line(), Taken::Wait);
    }

    /// A partial line waits for the rest, unless the input has ended — then it
    /// is the last line.
    #[test]
    fn the_last_line_needs_no_terminator() {
        let mut b = buffer(b"partial");
        assert_eq!(b.take_line(), Taken::Wait);
        b.ended = true;
        assert_eq!(b.take_line(), Taken::Bytes(b"partial".to_vec()));
        assert_eq!(b.take_line(), Taken::End);
        assert_eq!(b.take_chunk(), Taken::End);
    }

    /// In raw mode Enter sends `\r` and nothing after it.
    #[test]
    fn raw_mode_ends_a_line_at_a_carriage_return() {
        let mut b = buffer(b"yes\rno");
        assert_eq!(b.take_line(), Taken::Wait);
        b.raw = true;
        assert_eq!(b.take_line(), Taken::Bytes(b"yes\r".to_vec()));
    }

    /// A failure is reported after what was buffered, not instead of it.
    #[test]
    fn a_failure_comes_after_the_buffered_bytes() {
        let mut b = buffer(b"x");
        b.failed = Some("broken".into());
        assert_eq!(b.take_chunk(), Taken::Bytes(b"x".to_vec()));
        assert_eq!(b.take_chunk(), Taken::Failed("broken".into()));
    }
}
