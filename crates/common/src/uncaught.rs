//! A JS failure that escaped every handler, described rather than stringified.

use std::fmt;

/// An exception, or a rejection reason, that reached the host with no guest code
/// left to handle it.
///
/// Carried as fields rather than as one formatted string because the thing most
/// likely to receive one is a supervisor — the parent of a failed worker, a
/// pool deciding whether to retry — and a supervisor branches on the error
/// before it logs it. Formatting at the throw site threw that away and left
/// substring matching as the only way back to it.
///
/// [`Display`](fmt::Display) renders the report exactly as it always read (the
/// stack when there is one), so anything that only prints a failure is
/// unaffected by the extra fields.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct UncaughtError {
    /// The `name` property — `"TypeError"`, `"AbortError"`, … Empty when the
    /// thrown value was not an `Error`: `throw "nope"` has no name.
    pub name: String,
    /// The `message` property alone: no class prefix, no stack. For a value
    /// that is not an `Error`, `String(value)`.
    pub message: String,
    /// The `stack` property, which by V8's format already begins with
    /// `name: message`. Empty when the thrown value carried no stack.
    pub stack: String,
    /// URL of the script the exception came from. Empty when the host had no
    /// location to attach — a rejection reason that is not an `Error`, say.
    pub filename: String,
    /// 1-based line within [`filename`](Self::filename); 0 when unknown.
    pub lineno: u32,
    /// 1-based column within [`filename`](Self::filename); 0 when unknown.
    pub colno: u32,
}

impl UncaughtError {
    /// A failure described from the JS value that caused it, with the location
    /// read out of the stack's top frame.
    ///
    /// `stack` may be empty: only an `Error` carries one, and `throw 3` is
    /// legal JS. When it is, or when its top frame names no source, the
    /// location stays at the zeros that mean "unknown".
    ///
    /// Parsing the string is the only way to this. V8 does keep a structured
    /// trace on the value — but only until the first read of `.stack`, which
    /// formats it and drops the frames, and a failure is described a tick or
    /// more after that has happened.
    pub fn new(
        name: impl Into<String>,
        message: impl Into<String>,
        stack: impl Into<String>,
    ) -> Self {
        let stack = stack.into();
        let (filename, lineno, colno) = top_frame(&stack).unwrap_or_default();
        Self {
            name: name.into(),
            message: message.into(),
            stack,
            filename,
            lineno,
            colno,
        }
    }

    /// A failure the host itself is describing, with only prose to offer — a
    /// worker whose entry module would not load, a thread that panicked.
    ///
    /// There is no JS value behind one of these, so it carries no name, no
    /// stack and no location; it exists so such a failure travels the same
    /// path, and reaches the same `error` handler, as one that does.
    pub fn from_message(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            ..Self::default()
        }
    }
}

/// Splits `source:line:column` off the end of a V8 stack frame.
///
/// The two shapes V8 writes are `    at name (source:1:2)` and, for a frame
/// with no function name, `    at source:1:2`; the trailing parenthesis is the
/// only difference, so both are read from the right. Anything else — `at
/// native`, `at <anonymous>` — has no location and yields `None`.
fn frame_location(frame: &str) -> Option<(String, u32, u32)> {
    let frame = frame.strip_prefix("at ")?;
    let frame = frame.strip_suffix(')').unwrap_or(frame);
    let (rest, column) = frame.rsplit_once(':')?;
    let (source, line) = rest.rsplit_once(':')?;
    // A `file:///…` source contains colons of its own, so the split is only
    // this frame's location if both halves are actually numbers.
    let column = column.parse().ok()?;
    let line = line.parse().ok()?;
    let source = source.rsplit_once('(').map_or(source, |(_, after)| after);
    Some((source.to_string(), line, column))
}

/// The location of the topmost frame of a V8 stack string — the throw site.
///
/// Only the first frame is considered: if *it* has no location, no frame
/// further down is a better answer than none.
fn top_frame(stack: &str) -> Option<(String, u32, u32)> {
    let frame = stack
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("at "))?;
    frame_location(frame)
}

impl fmt::Display for UncaughtError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The stack subsumes the message — it opens with `name: message` — so
        // showing both would repeat the first line.
        if self.stack.is_empty() {
            f.write_str(&self.message)
        } else {
            f.write_str(&self.stack)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// V8 writes a named frame with the location in parentheses.
    #[test]
    fn a_named_frame_yields_its_location() {
        let error = UncaughtError::new(
            "TypeError",
            "boom",
            "TypeError: boom\n    at inner (file:///app/job.js:12:26)\n    at file:///app/job.js:30:1",
        );
        assert_eq!(error.filename, "file:///app/job.js");
        assert_eq!(error.lineno, 12);
        assert_eq!(error.colno, 26);
    }

    /// …and an anonymous one without them. Both are the top frame's job.
    #[test]
    fn an_anonymous_top_frame_yields_its_location_too() {
        let error = UncaughtError::new(
            "Error",
            "boom",
            "Error: boom\n    at file:///app/job.js:3:9",
        );
        assert_eq!(error.filename, "file:///app/job.js");
        assert_eq!(error.lineno, 3);
        assert_eq!(error.colno, 9);
    }

    /// A frame V8 has no source for leaves the location unknown rather than
    /// borrowing the next frame's, which would point at the wrong line.
    #[test]
    fn a_top_frame_without_a_location_yields_none() {
        let error = UncaughtError::new(
            "Error",
            "boom",
            "Error: boom\n    at native\n    at file:///app/job.js:3:9",
        );
        assert_eq!(error.filename, "");
        assert_eq!(error.lineno, 0);
        assert_eq!(error.colno, 0);
    }

    /// `throw "nope"` captures no stack at all.
    #[test]
    fn no_stack_means_no_location() {
        let error = UncaughtError::new("", "nope", "");
        assert_eq!(error.filename, "");
        assert_eq!(error.lineno, 0);
    }

    /// The report reads as it always did: the stack subsumes the message, and
    /// a failure with only prose shows the prose.
    #[test]
    fn display_shows_the_stack_when_there_is_one() {
        let error = UncaughtError::new("Error", "boom", "Error: boom\n    at file:///a.js:1:1");
        assert_eq!(error.to_string(), "Error: boom\n    at file:///a.js:1:1");
        assert_eq!(
            UncaughtError::from_message("worker could not be created").to_string(),
            "worker could not be created"
        );
    }
}
