//! A YAML 1.2 reader and writer over the runtime's [`Value`] tree.
//!
//! Written here rather than taken from a crate: `serde_yaml` is deprecated and
//! sits on `unsafe-libyaml`, a machine translation of libyaml's C into unsafe
//! Rust whose repository is archived (RUSTSEC-2024-0449). The pipeline is the
//! specification's: [`scanner`] turns characters into tokens, [`parser`]
//! tokens into the event stream the yaml-test-suite specifies, and
//! [`compose`] events into values. [`emit`] goes the other way.
//!
//! Everything is safe Rust, and every structure an untrusted document can
//! grow — nesting, alias expansion — is bounded (see [`compose::MAX_DEPTH`]
//! and [`compose::MAX_ALIAS_NODES`]).

mod compose;
mod emit;
mod parser;
mod scanner;
#[cfg(test)]
mod tests;

use es_runtime_engine::Value;
use std::fmt;

/// A position in the input, counted in characters from zero.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Mark {
    pub index: usize,
    pub line: usize,
    pub column: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ErrorKind {
    /// The input is not well-formed YAML, or uses something this reader refuses.
    Syntax,
    /// The input is well-formed but exceeds a resource bound.
    Limit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Error {
    pub kind: ErrorKind,
    pub message: String,
    pub mark: Mark,
}

impl Error {
    pub(crate) fn syntax(message: impl Into<String>, mark: Mark) -> Error {
        Error {
            kind: ErrorKind::Syntax,
            message: message.into(),
            mark,
        }
    }

    pub(crate) fn limit(message: impl Into<String>, mark: Mark) -> Error {
        Error {
            kind: ErrorKind::Limit,
            message: message.into(),
            mark,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} at line {} column {}",
            self.message,
            self.mark.line + 1,
            self.mark.column + 1
        )
    }
}

/// Parses a single-document YAML stream. An empty stream is `null`.
pub(crate) fn parse(input: &str) -> Result<Value, Error> {
    compose::compose(input)
}

/// Checks that `input` would parse.
pub(crate) fn validate(input: &str) -> Result<(), Error> {
    compose::validate(input)
}

/// Writes `value` as a YAML document ending in a line break.
#[cfg(any(test, feature = "fuzzing"))]
pub(crate) fn emit(value: &Value) -> String {
    emit::emit(value)
}

/// [`emit`] for a value from the guest: refuses what YAML cannot carry (bytes,
/// a function) rather than writing it as `null`.
pub(crate) fn build(value: &Value) -> Result<String, String> {
    emit::check(value)?;
    Ok(emit::emit(value))
}

/// Fuzzing entry point (see [`crate::fuzz`]): any input must produce a value or
/// an error, never a panic, and whatever parses must survive a write and re-read.
#[cfg(any(test, feature = "fuzzing"))]
pub(crate) fn fuzz_round_trip(input: &str) {
    if let Ok(value) = parse(input) {
        let text = emit(&value);
        if let Err(e) = parse(&text) {
            panic!("emitted YAML does not re-parse: {e}\n--- emitted:\n{text}");
        }
    }
}
