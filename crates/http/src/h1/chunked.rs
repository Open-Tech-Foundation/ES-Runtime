//! Decoding a chunked body (RFC 9112 §7.1), incrementally and strictly.
//!
//! The decoder is fed bytes as they arrive and hands back the data inside the
//! framing without copying it. It stops exactly at the end of the message —
//! the blank line after the trailers — so bytes past it (a pipelined request)
//! are left for whoever reads next. Chunk sizes, chunk extensions and the
//! trailer section are bounded; lines end in CRLF only.

use crate::fields::{Field, is_field_value_byte, is_tchar};

use super::request::{Limits, RequestError};

/// The most hex digits a chunk size may have: 16 is every `u64`.
const MAX_SIZE_DIGITS: u8 = 16;

/// The most bytes of chunk extensions one chunk line may carry. Extensions
/// are read and discarded; without a bound, a body of extensions and no data
/// would be a way to make the server read forever for nothing.
const MAX_EXTENSION_BYTES: usize = 4096;

/// Why a chunked body was refused. The connection is answered `400` and
/// closed: past a framing error, nothing after it can be trusted to be where
/// it seems.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkError(pub &'static str);

impl std::fmt::Display for ChunkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "malformed chunked body: {}", self.0)
    }
}

impl std::error::Error for ChunkError {}

/// What one call to [`ChunkedDecoder::decode`] produced.
#[derive(Debug, PartialEq, Eq)]
pub enum Chunk<'a> {
    /// Body bytes, a slice of the input.
    Data(&'a [u8]),
    /// The input ran out mid-framing; call again with more.
    NeedMore,
    /// The body is complete. Trailers, if any, are in
    /// [`trailers`](ChunkedDecoder::trailers).
    End,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Reading the chunk size's hex digits.
    Size {
        value: u64,
        digits: u8,
    },
    /// After the size: chunk extensions up to the CR, by their grammar.
    Extension {
        value: u64,
        bytes: usize,
        at: Ext,
    },
    /// Saw the size line's CR; the LF must follow.
    SizeLf {
        value: u64,
    },
    /// Inside a chunk's data.
    Data {
        remaining: u64,
    },
    /// After a chunk's data: its CR, then its LF.
    DataCr,
    DataLf,
    /// After the last chunk: the trailer section, buffered until it ends.
    Trailers,
    Done,
}

/// Where inside the chunk extensions the decoder is (RFC 9112 §7.1.1):
/// `*( BWS ";" BWS name [ BWS "=" BWS ( token / quoted-string ) ] )`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ext {
    /// Whitespace after the size or after a value: only more of it or `;` may
    /// follow — the grammar has no trailing whitespace (`BWS ";"`).
    BeforeSemicolon,
    /// Just after a quoted value: `;`, the CR, or whitespace before a `;`.
    AfterQuoted,
    /// After `;`: whitespace, then a name.
    BeforeName,
    /// In a name.
    Name,
    /// Whitespace after a name: `=` or `;` next (not the CR).
    AfterName,
    /// After `=`: whitespace, then a token or a quoted string.
    BeforeValue,
    /// In a token value.
    Token,
    /// In a quoted value, and just after a backslash in one.
    Quoted,
    QuotedEscape,
}

/// `qdtext` (RFC 9110 §5.6.4): tab, space, visible ASCII but `"` and `\`,
/// obs-text.
fn is_qdtext(b: u8) -> bool {
    matches!(b, b'\t' | b' ' | 0x21 | 0x23..=0x5b | 0x5d..=0x7e | 0x80..=0xff)
}

/// Where the extension grammar goes from `at` on byte `b`: `Err` breaks the
/// grammar, `Ok(None)` is the CR that ends the line.
fn ext_step(at: Ext, b: u8) -> Result<Option<Ext>, ()> {
    let ws = b == b' ' || b == b'\t';
    Ok(Some(match at {
        Ext::Quoted if b == b'"' => Ext::AfterQuoted,
        Ext::Quoted if b == b'\\' => Ext::QuotedEscape,
        Ext::Quoted if is_qdtext(b) => Ext::Quoted,
        Ext::QuotedEscape if is_field_value_byte(b) => Ext::Quoted,
        Ext::Quoted | Ext::QuotedEscape => return Err(()),
        Ext::BeforeName | Ext::BeforeValue if ws => at,
        Ext::BeforeName if is_tchar(b) => Ext::Name,
        Ext::BeforeValue if b == b'"' => Ext::Quoted,
        Ext::BeforeValue if is_tchar(b) => Ext::Token,
        Ext::BeforeName | Ext::BeforeValue => return Err(()),
        Ext::Name if is_tchar(b) => Ext::Name,
        Ext::Token if is_tchar(b) => Ext::Token,
        Ext::Name | Ext::AfterName if b == b'=' => Ext::BeforeValue,
        Ext::Name | Ext::AfterName if ws => Ext::AfterName,
        Ext::Token | Ext::AfterQuoted | Ext::BeforeSemicolon if ws => Ext::BeforeSemicolon,
        // After whitespace, only `;` may end it.
        Ext::AfterName | Ext::BeforeSemicolon => match b {
            b';' => Ext::BeforeName,
            _ => return Err(()),
        },
        Ext::Name | Ext::Token | Ext::AfterQuoted => match b {
            b';' => Ext::BeforeName,
            b'\r' => return Ok(None),
            _ => return Err(()),
        },
    }))
}

/// A chunked-body decoder for one message.
#[derive(Debug)]
pub struct ChunkedDecoder {
    state: State,
    limits: Limits,
    trailer_bytes: Vec<u8>,
    trailer_fields: Vec<Field>,
}

impl ChunkedDecoder {
    /// A decoder whose trailer section is held to `limits` (its head-size and
    /// field-count limits, as a request head is).
    #[must_use]
    pub fn new(limits: Limits) -> Self {
        ChunkedDecoder {
            state: State::Size {
                value: 0,
                digits: 0,
            },
            limits,
            trailer_bytes: Vec::new(),
            trailer_fields: Vec::new(),
        }
    }

    /// Whether the body has ended.
    #[must_use]
    pub fn is_done(&self) -> bool {
        self.state == State::Done
    }

    /// The trailer fields, once [`Chunk::End`] has been returned: name and
    /// value as bytes, validated as header fields are.
    pub fn trailers(&self) -> impl Iterator<Item = (&[u8], &[u8])> {
        self.trailer_fields.iter().map(|f| {
            (
                f.name.of(&self.trailer_bytes),
                f.value.of(&self.trailer_bytes),
            )
        })
    }

    /// Consumes framing from the start of `input` and returns how many bytes
    /// it used together with what they held. Call again with the rest of the
    /// input (and then with more of it) until [`Chunk::End`].
    ///
    /// # Errors
    ///
    /// A [`ChunkError`] when the framing breaks the grammar or a limit.
    pub fn decode<'a>(&mut self, input: &'a [u8]) -> Result<(usize, Chunk<'a>), ChunkError> {
        let mut used = 0;
        while used < input.len() {
            let b = input[used];
            match self.state {
                State::Size { value, digits } => match hex(b) {
                    Some(d) => {
                        if digits == MAX_SIZE_DIGITS {
                            return Err(ChunkError("chunk size too long"));
                        }
                        self.state = State::Size {
                            value: (value << 4) | u64::from(d),
                            digits: digits + 1,
                        };
                        used += 1;
                    }
                    None if digits == 0 => return Err(ChunkError("missing chunk size")),
                    None => match b {
                        b'\r' => {
                            self.state = State::SizeLf { value };
                            used += 1;
                        }
                        b';' => {
                            self.state = State::Extension {
                                value,
                                bytes: 1,
                                at: Ext::BeforeName,
                            };
                            used += 1;
                        }
                        b' ' | b'\t' => {
                            self.state = State::Extension {
                                value,
                                bytes: 1,
                                at: Ext::BeforeSemicolon,
                            };
                            used += 1;
                        }
                        _ => return Err(ChunkError("invalid chunk size")),
                    },
                },
                State::Extension { value, bytes, at } => {
                    if bytes == MAX_EXTENSION_BYTES {
                        return Err(ChunkError("chunk extensions too long"));
                    }
                    self.state = match ext_step(at, b) {
                        Ok(Some(next)) => State::Extension {
                            value,
                            bytes: bytes + 1,
                            at: next,
                        },
                        Ok(None) => State::SizeLf { value },
                        Err(()) => return Err(ChunkError("invalid chunk extension")),
                    };
                    used += 1;
                }
                State::SizeLf { value } => {
                    if b != b'\n' {
                        return Err(ChunkError("chunk size line not ended by CRLF"));
                    }
                    used += 1;
                    self.state = if value == 0 {
                        State::Trailers
                    } else {
                        State::Data { remaining: value }
                    };
                }
                State::Data { remaining } => {
                    let available = (input.len() - used) as u64;
                    let n = remaining.min(available) as usize;
                    let data = &input[used..used + n];
                    let left = remaining - n as u64;
                    self.state = if left == 0 {
                        State::DataCr
                    } else {
                        State::Data { remaining: left }
                    };
                    return Ok((used + n, Chunk::Data(data)));
                }
                State::DataCr => {
                    if b != b'\r' {
                        return Err(ChunkError("chunk data longer than its size"));
                    }
                    self.state = State::DataLf;
                    used += 1;
                }
                State::DataLf => {
                    if b != b'\n' {
                        return Err(ChunkError("chunk data not ended by CRLF"));
                    }
                    self.state = State::Size {
                        value: 0,
                        digits: 0,
                    };
                    used += 1;
                }
                State::Trailers => {
                    self.trailer_bytes.push(b);
                    used += 1;
                    if self.trailer_section_ended()? {
                        self.state = State::Done;
                        return Ok((used, Chunk::End));
                    }
                }
                State::Done => return Ok((used, Chunk::End)),
            }
        }
        Ok((
            used,
            if self.is_done() {
                Chunk::End
            } else {
                Chunk::NeedMore
            },
        ))
    }

    /// Whether the buffered trailer section is complete, parsing it when it is.
    fn trailer_section_ended(&mut self) -> Result<bool, ChunkError> {
        let t = &self.trailer_bytes;
        // No trailers: the section is just the blank line.
        if t.as_slice() == b"\r\n" {
            return Ok(true);
        }
        if t.len() > self.limits.max_head_bytes {
            return Err(ChunkError("trailer section too large"));
        }
        if !t.ends_with(b"\r\n\r\n") {
            // Fail early on what can never end well.
            let last = t[t.len() - 1];
            let prev = t.len().checked_sub(2).map(|i| t[i]);
            if last == b'\n' && prev != Some(b'\r') {
                return Err(ChunkError("bare LF in trailers"));
            }
            if prev == Some(b'\r') && last != b'\n' {
                return Err(ChunkError("bare CR in trailers"));
            }
            return Ok(false);
        }
        // The section's field lines follow the request head's grammar, and
        // are parsed by the same code.
        let mut fields = Vec::new();
        let parsed = super::request::check_complete_line_endings(t).and_then(|()| {
            super::request::parse_field_lines(t, 0, t.len(), &self.limits, &mut fields)
        });
        match parsed {
            Ok(()) => {}
            Err(RequestError::TooManyFields) => return Err(ChunkError("too many trailers")),
            Err(_) => return Err(ChunkError("malformed trailer field")),
        }
        self.trailer_fields = fields;
        Ok(true)
    }
}

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}
