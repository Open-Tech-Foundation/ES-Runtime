//! Writing an HTTP/1.1 response (RFC 9112 §4, §6, §7.1).
//!
//! The encoder owns the framing: it writes `Content-Length` or
//! `Transfer-Encoding: chunked` itself and drops either if the caller passed
//! one, so the framing on the wire is always the one the body is actually
//! sent with. Every name and value it writes is validated here — the guarantee
//! against response splitting does not depend on whoever built the fields.

use crate::date::IMF_FIXDATE_LEN;
use crate::fields::{is_field_value, is_token};

/// How a message's body is framed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Framing {
    /// `Content-Length: n`, then exactly n bytes.
    Length(u64),
    /// `Transfer-Encoding: chunked`; the body follows as chunks
    /// ([`chunk_header`], [`CHUNK_END`], [`encode_last_chunk`]).
    Chunked,
    /// No body and no framing field: `1xx`, `204` and `304`, which RFC 9110
    /// says carry neither.
    NoBody,
}

/// A response field the encoder will not write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodeError {
    /// A status outside 100–999.
    InvalidStatus,
    /// A field name that is not a token.
    InvalidFieldName,
    /// A field value with a control character (CR, LF, NUL, …).
    InvalidFieldValue,
    /// A request method that is not a token.
    InvalidMethod,
    /// A request-target with whitespace, controls or non-ASCII bytes.
    InvalidTarget,
}

impl std::fmt::Display for EncodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            EncodeError::InvalidStatus => "invalid status code",
            EncodeError::InvalidFieldName => "invalid header name",
            EncodeError::InvalidFieldValue => "invalid header value",
            EncodeError::InvalidMethod => "invalid request method",
            EncodeError::InvalidTarget => "invalid request-target",
        })
    }
}

impl std::error::Error for EncodeError {}

/// The interim response a server sends to `Expect: 100-continue`.
pub const CONTINUE: &[u8] = b"HTTP/1.1 100 Continue\r\n\r\n";

/// The CRLF that ends every chunk's data.
pub const CHUNK_END: &[u8] = b"\r\n";

/// Appends a response head to `out`: status line, `fields`, the framing
/// field, `Date` (unless `fields` has one) and `Connection: close` when the
/// connection will not be kept.
///
/// `Content-Length` and `Transfer-Encoding` in `fields` are dropped: the
/// framing is `framing`'s. On error nothing is appended.
///
/// # Errors
///
/// An [`EncodeError`] for an invalid status, name or value.
pub fn encode_response_head<'a>(
    out: &mut Vec<u8>,
    status: u16,
    fields: impl IntoIterator<Item = (&'a [u8], &'a [u8])>,
    framing: Framing,
    date: &[u8; IMF_FIXDATE_LEN],
    keep_alive: bool,
) -> Result<(), EncodeError> {
    if !(100..=999).contains(&status) {
        return Err(EncodeError::InvalidStatus);
    }
    let mark = out.len();
    out.extend_from_slice(b"HTTP/1.1 ");
    push_status(out, status);
    out.push(b' ');
    out.extend_from_slice(reason(status).as_bytes());
    out.extend_from_slice(b"\r\n");

    let mut has_date = false;
    for (name, value) in fields {
        if !is_token(name) {
            out.truncate(mark);
            return Err(EncodeError::InvalidFieldName);
        }
        if !is_field_value(value) {
            out.truncate(mark);
            return Err(EncodeError::InvalidFieldValue);
        }
        if name.eq_ignore_ascii_case(b"content-length")
            || name.eq_ignore_ascii_case(b"transfer-encoding")
        {
            continue;
        }
        has_date |= name.eq_ignore_ascii_case(b"date");
        push_field(out, name, value);
    }
    match framing {
        Framing::Length(n) => {
            out.extend_from_slice(b"content-length: ");
            push_decimal(out, n);
            out.extend_from_slice(b"\r\n");
        }
        Framing::Chunked => out.extend_from_slice(b"transfer-encoding: chunked\r\n"),
        Framing::NoBody => {}
    }
    if !has_date {
        push_field(out, b"date", date);
    }
    if !keep_alive {
        out.extend_from_slice(b"connection: close\r\n");
    }
    out.extend_from_slice(b"\r\n");
    Ok(())
}

/// The line that starts a chunk of `len` bytes: its size in hex and a CRLF.
/// `len` must not be zero — an empty chunk is the end of the body
/// ([`encode_last_chunk`]).
#[must_use]
pub fn chunk_header(len: usize) -> ChunkHeader {
    debug_assert!(len > 0, "a zero-length chunk ends the body");
    let mut bytes = [0u8; 18];
    let digits = (usize::BITS - len.leading_zeros()).div_ceil(4).max(1) as usize;
    for (i, slot) in bytes[..digits].iter_mut().enumerate() {
        let nibble = (len >> (4 * (digits - 1 - i))) & 0xf;
        *slot = b"0123456789abcdef"[nibble];
    }
    bytes[digits] = b'\r';
    bytes[digits + 1] = b'\n';
    ChunkHeader {
        bytes,
        len: digits + 2,
    }
}

/// A chunk-size line, built without allocating.
#[derive(Debug, Clone, Copy)]
pub struct ChunkHeader {
    bytes: [u8; 18],
    len: usize,
}

impl ChunkHeader {
    /// The line's bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

/// Appends a request head to `out`: request line, `Host`, `fields` and the
/// framing field. `Content-Length`, `Transfer-Encoding` and `Host` in `fields`
/// are dropped — the framing is `framing`'s and the host is `host`. On error
/// nothing is appended.
///
/// # Errors
///
/// An [`EncodeError`] for a method that is not a token, a target with
/// anything but visible ASCII, a host that is not an authority, or an invalid
/// field name or value.
pub fn encode_request_head<'a>(
    out: &mut Vec<u8>,
    method: &[u8],
    target: &[u8],
    host: &[u8],
    fields: impl IntoIterator<Item = (&'a [u8], &'a [u8])>,
    framing: Framing,
) -> Result<(), EncodeError> {
    if !is_token(method) {
        return Err(EncodeError::InvalidMethod);
    }
    if target.is_empty() || !target.iter().all(|&b| (0x21..=0x7e).contains(&b)) {
        return Err(EncodeError::InvalidTarget);
    }
    if !host.iter().all(|&b| {
        b.is_ascii_alphanumeric()
            || matches!(
                b,
                b'-' | b'.'
                    | b'_'
                    | b'~'
                    | b'!'
                    | b'$'
                    | b'&'
                    | b'\''
                    | b'('
                    | b')'
                    | b'*'
                    | b'+'
                    | b','
                    | b';'
                    | b'='
                    | b'%'
                    | b':'
                    | b'['
                    | b']'
            )
    }) {
        return Err(EncodeError::InvalidFieldValue);
    }
    let mark = out.len();
    out.extend_from_slice(method);
    out.push(b' ');
    out.extend_from_slice(target);
    out.extend_from_slice(b" HTTP/1.1\r\n");
    push_field(out, b"host", host);
    for (name, value) in fields {
        if !is_token(name) {
            out.truncate(mark);
            return Err(EncodeError::InvalidFieldName);
        }
        if !is_field_value(value) {
            out.truncate(mark);
            return Err(EncodeError::InvalidFieldValue);
        }
        if name.eq_ignore_ascii_case(b"content-length")
            || name.eq_ignore_ascii_case(b"transfer-encoding")
            || name.eq_ignore_ascii_case(b"host")
        {
            continue;
        }
        push_field(out, name, value);
    }
    match framing {
        Framing::Length(n) => {
            out.extend_from_slice(b"content-length: ");
            push_decimal(out, n);
            out.extend_from_slice(b"\r\n");
        }
        Framing::Chunked => out.extend_from_slice(b"transfer-encoding: chunked\r\n"),
        Framing::NoBody => {}
    }
    out.extend_from_slice(b"\r\n");
    Ok(())
}

/// Appends the last chunk and the trailer section ending a chunked body.
/// On error nothing is appended.
///
/// # Errors
///
/// An [`EncodeError`] for an invalid trailer name or value.
pub fn encode_last_chunk<'a>(
    out: &mut Vec<u8>,
    trailers: impl IntoIterator<Item = (&'a [u8], &'a [u8])>,
) -> Result<(), EncodeError> {
    let mark = out.len();
    out.extend_from_slice(b"0\r\n");
    for (name, value) in trailers {
        if !is_token(name) {
            out.truncate(mark);
            return Err(EncodeError::InvalidFieldName);
        }
        if !is_field_value(value) {
            out.truncate(mark);
            return Err(EncodeError::InvalidFieldValue);
        }
        push_field(out, name, value);
    }
    out.extend_from_slice(b"\r\n");
    Ok(())
}

fn push_field(out: &mut Vec<u8>, name: &[u8], value: &[u8]) {
    out.extend_from_slice(name);
    out.extend_from_slice(b": ");
    out.extend_from_slice(value);
    out.extend_from_slice(b"\r\n");
}

fn push_status(out: &mut Vec<u8>, status: u16) {
    out.extend_from_slice(&[
        b'0' + (status / 100) as u8,
        b'0' + (status / 10 % 10) as u8,
        b'0' + (status % 10) as u8,
    ]);
}

fn push_decimal(out: &mut Vec<u8>, mut n: u64) {
    let mut digits = [0u8; 20];
    let mut i = digits.len();
    loop {
        i -= 1;
        digits[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    out.extend_from_slice(&digits[i..]);
}

/// The reason phrase for `status` (RFC 9110 §15). A code without a
/// registered phrase gets an empty one, which the grammar allows.
#[must_use]
pub fn reason(status: u16) -> &'static str {
    match status {
        100 => "Continue",
        101 => "Switching Protocols",
        103 => "Early Hints",
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        203 => "Non-Authoritative Information",
        204 => "No Content",
        205 => "Reset Content",
        206 => "Partial Content",
        300 => "Multiple Choices",
        301 => "Moved Permanently",
        302 => "Found",
        303 => "See Other",
        304 => "Not Modified",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        402 => "Payment Required",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        406 => "Not Acceptable",
        407 => "Proxy Authentication Required",
        408 => "Request Timeout",
        409 => "Conflict",
        410 => "Gone",
        411 => "Length Required",
        412 => "Precondition Failed",
        413 => "Content Too Large",
        414 => "URI Too Long",
        415 => "Unsupported Media Type",
        416 => "Range Not Satisfiable",
        417 => "Expectation Failed",
        421 => "Misdirected Request",
        422 => "Unprocessable Content",
        425 => "Too Early",
        426 => "Upgrade Required",
        428 => "Precondition Required",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        451 => "Unavailable For Legal Reasons",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        505 => "HTTP Version Not Supported",
        _ => "",
    }
}
