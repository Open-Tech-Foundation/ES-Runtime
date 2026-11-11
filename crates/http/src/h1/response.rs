//! Parsing an HTTP/1.1 response head — the client's half (RFC 9112 §4, §6.3).
//!
//! The same line and field grammar as a request head, with the same
//! strictness: CRLF only, no folded lines, HTTP/1.1 only. What differs is how a
//! response's body is framed, which depends on the request it answers — a
//! response to `HEAD` has no body whatever its headers say — and that a
//! response may be delimited by the connection closing. A pooled connection
//! is only reused when the framing left no doubt where the response ended.

use crate::fields::{Field, Span, is_field_value_byte, is_token, list_elements};

use super::request::{
    Limits, RequestError, check_content_lengths, check_incomplete_fields, check_line_endings,
    find_head_end, line_end_at, parse_field_lines,
};

/// Where a response's body ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseBody {
    /// No body: a response to `HEAD`, or a `1xx`, `204` or `304`.
    Empty,
    /// Exactly this many bytes.
    Length(u64),
    /// A chunked body.
    Chunked,
    /// Everything until the server closes the connection.
    UntilClose,
}

/// A parsed response head; its fields are in the `Vec` given to
/// [`parse_response`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResponseHead {
    /// The status code.
    pub status: u16,
    /// The reason phrase, as sent (may be empty).
    pub reason: Span,
    /// Where the body ends.
    pub body: ResponseBody,
    /// Whether the connection may carry another request afterwards.
    pub keep_alive: bool,
    /// How many bytes the head took.
    pub len: usize,
}

impl ResponseHead {
    /// An interim response (`1xx` other than `101`): another head follows on
    /// the same connection before the final one.
    #[must_use]
    pub fn is_interim(&self) -> bool {
        (100..200).contains(&self.status) && self.status != 101
    }
}

/// Why a response head was refused. The connection cannot be trusted past it
/// and is closed; the request fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseError {
    /// The head breaks the grammar.
    Malformed(&'static str),
    /// A version other than HTTP/1.1.
    VersionNotSupported,
    /// More than [`Limits::max_head_bytes`] without a complete head.
    HeadTooLarge,
    /// More field lines than [`Limits::max_fields`].
    TooManyFields,
}

impl std::fmt::Display for ResponseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResponseError::Malformed(why) => write!(f, "malformed response: {why}"),
            ResponseError::VersionNotSupported => {
                f.write_str("the server answered with an HTTP version other than 1.1")
            }
            ResponseError::HeadTooLarge => f.write_str("response head too large"),
            ResponseError::TooManyFields => f.write_str("too many response header fields"),
        }
    }
}

impl std::error::Error for ResponseError {}

impl From<RequestError> for ResponseError {
    fn from(e: RequestError) -> Self {
        match e {
            RequestError::Malformed(why) => ResponseError::Malformed(why),
            RequestError::HeadTooLarge => ResponseError::HeadTooLarge,
            RequestError::TooManyFields => ResponseError::TooManyFields,
            // Request-only refusals cannot come from the shared field code.
            _ => ResponseError::Malformed("invalid response"),
        }
    }
}

/// The outcome of a parse that did not fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParsedResponse {
    /// A complete head.
    Complete(ResponseHead),
    /// Not all here yet; read more and parse again from the start.
    Partial,
}

fn bad(why: &'static str) -> ResponseError {
    ResponseError::Malformed(why)
}

/// Parses the response head at the start of `buf`, filling `fields` (cleared
/// first). `to_head` says whether the request was `HEAD`, whose response has
/// no body whatever its framing fields claim.
///
/// # Errors
///
/// A [`ResponseError`] when the head is refused.
pub fn parse_response(
    buf: &[u8],
    limits: &Limits,
    fields: &mut Vec<Field>,
    to_head: bool,
) -> Result<ParsedResponse, ResponseError> {
    fields.clear();
    let Some(end) = find_head_end(buf) else {
        if buf.len() >= limits.max_head_bytes {
            return Err(ResponseError::HeadTooLarge);
        }
        check_line_endings(buf, false)?;
        if let Some(line_end) = line_end_at(buf)? {
            parse_status_line(&buf[..line_end])?;
            check_incomplete_fields(buf, line_end + 2, limits, fields)?;
            fields.clear();
        } else {
            check_status_line_prefix(buf)?;
        }
        return Ok(ParsedResponse::Partial);
    };
    let len = end + 4;
    if len > limits.max_head_bytes {
        return Err(ResponseError::HeadTooLarge);
    }
    let line_end = line_end_at(buf)?.expect("a head has a line");
    let (status, reason) = parse_status_line(&buf[..line_end])?;
    parse_field_lines(buf, line_end + 2, end + 2, limits, fields)?;

    let mut head = ResponseHead {
        status,
        reason,
        body: ResponseBody::Empty,
        keep_alive: true,
        len,
    };
    let content_length = check_content_lengths(buf, fields)?;
    let mut transfer_encoding = false;
    let mut chunked_seen = false;
    let mut chunked_last = false;
    for field in fields.iter() {
        let name = field.name.of(buf);
        let value = field.value.of(buf);
        if name.eq_ignore_ascii_case(b"transfer-encoding") {
            transfer_encoding = true;
            let mut any = false;
            for coding in list_elements(value) {
                any = true;
                if !is_token(coding) {
                    return Err(bad("invalid transfer coding"));
                }
                let is_chunked = coding.eq_ignore_ascii_case(b"chunked");
                if is_chunked && chunked_seen {
                    return Err(bad("chunked applied twice"));
                }
                chunked_seen |= is_chunked;
                chunked_last = is_chunked;
            }
            if !any {
                return Err(bad("empty Transfer-Encoding"));
            }
        } else if name.eq_ignore_ascii_case(b"connection")
            && list_elements(value).any(|o| o.eq_ignore_ascii_case(b"close"))
        {
            head.keep_alive = false;
        }
    }

    // RFC 9112 §6.3, in its order.
    head.body = if to_head || (100..200).contains(&status) || status == 204 || status == 304 {
        ResponseBody::Empty
    } else if transfer_encoding {
        // Both framings at once: the coding wins, and the connection is not
        // reused, since a server that sent both may disagree with us about
        // where this response ended.
        if content_length.is_some() {
            head.keep_alive = false;
        }
        if chunked_last {
            ResponseBody::Chunked
        } else {
            // A coding that is not chunked at the outermost layer can only be
            // ended by the connection.
            head.keep_alive = false;
            ResponseBody::UntilClose
        }
    } else {
        match content_length {
            Some(0) => ResponseBody::Empty,
            Some(n) => ResponseBody::Length(n),
            None => {
                head.keep_alive = false;
                ResponseBody::UntilClose
            }
        }
    };
    Ok(ParsedResponse::Complete(head))
}

/// `HTTP/1.1 SP 3DIGIT SP [reason]` — the reason phrase may be empty, and its
/// space with it, since servers that send no phrase commonly omit both.
fn parse_status_line(line: &[u8]) -> Result<(u16, Span), ResponseError> {
    let Some(rest) = line.strip_prefix(b"HTTP/1.1 ") else {
        return match line {
            [b'H', b'T', b'T', b'P', b'/', major, b'.', minor, b' ', ..]
                if major.is_ascii_digit() && minor.is_ascii_digit() =>
            {
                Err(ResponseError::VersionNotSupported)
            }
            _ => Err(bad("invalid status line")),
        };
    };
    let (digits, reason_at) = match rest {
        [a, b, c] => ([*a, *b, *c], rest.len()),
        [a, b, c, b' ', ..] => ([*a, *b, *c], 4),
        _ => return Err(bad("invalid status code")),
    };
    if !digits.iter().all(u8::is_ascii_digit) {
        return Err(bad("invalid status code"));
    }
    let status = digits
        .iter()
        .fold(0u16, |n, &d| n * 10 + u16::from(d - b'0'));
    if status < 100 {
        return Err(bad("invalid status code"));
    }
    let reason = &rest[reason_at..];
    if !reason.iter().all(|&b| is_field_value_byte(b)) {
        return Err(bad("invalid reason phrase"));
    }
    let start = b"HTTP/1.1 ".len() + reason_at;
    Ok((
        status,
        Span {
            start,
            end: start + reason.len(),
        },
    ))
}

/// Checks a status line that has not ended yet, as far as it goes.
fn check_status_line_prefix(line: &[u8]) -> Result<(), ResponseError> {
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    let version = &line[..line.len().min(9)];
    if b"HTTP/1.1 ".starts_with(version) {
        return Ok(());
    }
    let shape = b"HTTP/0.0 ";
    let shaped = version.iter().zip(shape).all(|(&v, &s)| {
        if s == b'0' {
            v.is_ascii_digit()
        } else {
            v == s
        }
    });
    if shaped {
        Err(ResponseError::VersionNotSupported)
    } else {
        Err(bad("invalid status line"))
    }
}
