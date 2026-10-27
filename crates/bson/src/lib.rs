//! BSON, as <https://bsonspec.org/spec.html> defines it (DECISIONS D187).
//!
//! This crate is the byte half of the format: a [`Document`] reader that
//! validates as it walks, a [`Writer`], and Decimal128's text conversion. It
//! knows no engine type, so the same code reads a document that becomes a
//! JavaScript object in `engine` and a server reply that `runtime`'s MongoDB
//! driver only inspects. It does no I/O and allocates only what it returns.
//!
//! # Untrusted input
//!
//! Every length is checked against the bytes it claims before it is used, and
//! nesting is bounded by [`MAX_DEPTH`], so a hostile document costs at most its
//! own size and a bounded stack. [`validate`] walks a whole document without
//! building anything, and is what the fuzz target drives.

#![forbid(unsafe_code)]

mod decimal128;
mod reader;
mod writer;

pub use decimal128::{decimal128_from_str, decimal128_to_string};
pub use reader::{Document, Element, Elements, validate};
pub use writer::Writer;

/// How deep documents and arrays may nest, on read and on write.
///
/// The server stores at most 100 levels; a command wraps a document in a few
/// more. The bound is generous for that and small enough that the recursive
/// reader and writer cannot exhaust a thread's stack.
pub const MAX_DEPTH: usize = 256;

/// Element type bytes.
#[allow(missing_docs)]
pub mod kind {
    pub const DOUBLE: u8 = 0x01;
    pub const STRING: u8 = 0x02;
    pub const DOCUMENT: u8 = 0x03;
    pub const ARRAY: u8 = 0x04;
    pub const BINARY: u8 = 0x05;
    pub const UNDEFINED: u8 = 0x06;
    pub const OBJECT_ID: u8 = 0x07;
    pub const BOOLEAN: u8 = 0x08;
    pub const DATETIME: u8 = 0x09;
    pub const NULL: u8 = 0x0A;
    pub const REGEX: u8 = 0x0B;
    pub const DB_POINTER: u8 = 0x0C;
    pub const CODE: u8 = 0x0D;
    pub const SYMBOL: u8 = 0x0E;
    pub const CODE_WITH_SCOPE: u8 = 0x0F;
    pub const INT32: u8 = 0x10;
    pub const TIMESTAMP: u8 = 0x11;
    pub const INT64: u8 = 0x12;
    pub const DECIMAL128: u8 = 0x13;
    pub const MIN_KEY: u8 = 0xFF;
    pub const MAX_KEY: u8 = 0x7F;
}

/// Why a document could not be read or written. The message names the problem
/// and, when reading, the byte offset it was found at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(String);

impl Error {
    /// An error with this message.
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }

    pub(crate) fn at(offset: usize, message: impl std::fmt::Display) -> Self {
        Self(format!("{message} at byte {offset}"))
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// A result whose error is a BSON [`Error`].
pub type Result<T> = std::result::Result<T, Error>;

/// Lowercase hex of `bytes`: how an ObjectId, a UUID and a Decimal128 are
/// carried as text.
pub fn to_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    out
}

/// The bytes of exactly `N * 2` hex digits, either case. `None` for anything
/// else.
pub fn from_hex<const N: usize>(text: &str) -> Option<[u8; N]> {
    let text = text.as_bytes();
    if text.len() != N * 2 {
        return None;
    }
    let mut out = [0u8; N];
    for (i, [high, low]) in text.as_chunks::<2>().0.iter().enumerate() {
        out[i] = (nibble(*high)? << 4) | nibble(*low)?;
    }
    Some(out)
}

fn nibble(digit: u8) -> Option<u8> {
    match digit {
        b'0'..=b'9' => Some(digit - b'0'),
        b'a'..=b'f' => Some(digit - b'a' + 10),
        b'A'..=b'F' => Some(digit - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips_and_refuses_the_wrong_length_or_digit() {
        let bytes = [0x00, 0x7f, 0xff, 0xa5];
        assert_eq!(to_hex(&bytes), "007fffa5");
        assert_eq!(from_hex::<4>("007FFFA5"), Some(bytes));
        assert_eq!(from_hex::<4>("007fffa"), None);
        assert_eq!(from_hex::<4>("007fffag"), None);
    }
}
