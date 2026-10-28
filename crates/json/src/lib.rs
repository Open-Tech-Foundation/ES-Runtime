//! JSONC and JSON5, as DECISIONS D191 defines them.
//!
//! This crate is the text half of `runtime:serialization`'s `JSONC` and
//! `JSON5`: one lexer with a [`Dialect`], and a parser that reports what it
//! reads to a [`Sink`] instead of building a value. `engine` implements the
//! sink to build JavaScript values straight from the text, as BSON's decoder
//! does, and esdev's tsconfig edit reads the same tokens. It knows no engine
//! type and does no I/O.
//!
//! # The dialects
//!
//! - [`Dialect::Jsonc`] is RFC 8259 JSON plus `//` and `/* */` comments and
//!   one trailing comma in an object or array. A leading byte order mark is
//!   ignored. Nothing else is accepted.
//! - [`Dialect::Json5`] is JSON5 1.0.0, its section 9 grammar, with the
//!   reference implementation deciding where the prose and the grammar
//!   disagree (U+2028 and U+2029 are allowed in strings).
//!
//! # Untrusted input
//!
//! Nesting is bounded by [`MAX_DEPTH`], so the recursive parser's stack is
//! bounded; every other cost is linear in the text.

#![forbid(unsafe_code)]

mod lexer;
mod number;
mod parser;

use std::borrow::Cow;
use std::fmt;

pub use parser::{parse, validate};

/// How deep objects and arrays may nest. The same bound as YAML's and BSON's
/// (DECISIONS D189).
pub const MAX_DEPTH: usize = 256;

/// Which format the text is read as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    /// JSON with comments and trailing commas.
    Jsonc,
    /// JSON5 1.0.0.
    Json5,
}

/// A string or a property name as read.
///
/// JSON lets an escape spell a lone surrogate (`"\ud800"`), which a Rust
/// `str` cannot hold but a JavaScript string can. Such a string is carried as
/// UTF-16 code units, so it reaches the engine as `JSON.parse` would make it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Text<'a> {
    /// Well-formed text, borrowed from the input when it had no escapes.
    Str(Cow<'a, str>),
    /// Text with a lone surrogate, as UTF-16 code units.
    Utf16(Vec<u16>),
}

impl Text<'_> {
    /// The text with each lone surrogate replaced by U+FFFD.
    pub fn to_string_lossy(&self) -> Cow<'_, str> {
        match self {
            Text::Str(s) => Cow::Borrowed(s),
            Text::Utf16(units) => Cow::Owned(String::from_utf16_lossy(units)),
        }
    }
}

/// What the parser reports, in document order.
///
/// A value inside an object follows its [`key`](Sink::key). Any method may
/// stop the parse by returning `Err`, which [`parse`] hands back as
/// [`Failure::Abort`].
#[allow(missing_docs)]
pub trait Sink {
    type Abort;
    fn null(&mut self) -> Result<(), Self::Abort>;
    fn bool(&mut self, value: bool) -> Result<(), Self::Abort>;
    fn number(&mut self, value: f64) -> Result<(), Self::Abort>;
    fn string(&mut self, value: Text<'_>) -> Result<(), Self::Abort>;
    fn begin_array(&mut self) -> Result<(), Self::Abort>;
    fn end_array(&mut self) -> Result<(), Self::Abort>;
    fn begin_object(&mut self) -> Result<(), Self::Abort>;
    fn key(&mut self, key: Text<'_>) -> Result<(), Self::Abort>;
    fn end_object(&mut self) -> Result<(), Self::Abort>;
}

/// Why a parse stopped.
#[derive(Debug)]
pub enum Failure<A> {
    /// The text is not in the dialect, or nests too deep. Boxed, so the
    /// results the parser returns for every token stay small.
    Error(Box<Error>),
    /// The sink stopped it.
    Abort(A),
}

impl<A> From<Box<Error>> for Failure<A> {
    fn from(error: Box<Error>) -> Self {
        Failure::Error(error)
    }
}

/// The kind of an [`Error`], which decides the JavaScript error class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// The text is not in the dialect: a `SyntaxError`.
    Syntax,
    /// The text nests deeper than [`MAX_DEPTH`]: a `RangeError`.
    Depth,
}

/// A refused text, with where in it the problem is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    /// Which class of error this is.
    pub kind: ErrorKind,
    /// What is wrong, without the position.
    pub message: String,
    /// The byte offset of the problem.
    pub offset: usize,
    /// The line of the problem, from 1.
    pub line: usize,
    /// The column of the problem, in characters from 1.
    pub column: usize,
}

impl Error {
    pub(crate) fn new(
        kind: ErrorKind,
        message: impl Into<String>,
        text: &str,
        offset: usize,
        dialect: Dialect,
    ) -> Box<Error> {
        let (line, column) = position(text, offset, dialect);
        Box::new(Error {
            kind,
            message: message.into(),
            offset,
            line,
            column,
        })
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} at line {} column {}",
            self.message, self.line, self.column
        )
    }
}

impl std::error::Error for Error {}

/// The line and column, both from 1, of the byte at `offset`. A line ends at
/// LF, CR or CRLF, and in JSON5 also at U+2028 and U+2029, as its comments do.
fn position(text: &str, offset: usize, dialect: Dialect) -> (usize, usize) {
    let (mut line, mut column) = (1, 1);
    let mut chars = text[..offset.min(text.len())].chars().peekable();
    while let Some(c) = chars.next() {
        let ends_line = match c {
            '\n' => true,
            '\r' => chars.peek() != Some(&'\n'),
            '\u{2028}' | '\u{2029}' => dialect == Dialect::Json5,
            _ => false,
        };
        if ends_line {
            line += 1;
            column = 1;
        } else if c != '\r' {
            column += 1;
        }
    }
    (line, column)
}

#[cfg(test)]
mod tests;
