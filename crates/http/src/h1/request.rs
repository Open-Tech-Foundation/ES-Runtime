//! Parsing an HTTP/1.1 request head (RFC 9112 §2–§7), strictly.
//!
//! The parser finds a complete head, checks every byte of it against the
//! grammar, and then decides the message's framing — where its body ends —
//! which is the part request smuggling attacks. Anything the grammar does not
//! allow, or that two implementations could frame differently, is refused with
//! the status the server answers before closing. Nothing is copied: the head
//! is returned as [`Span`]s into the caller's buffer.
//!
//! Strictness, by design (DECISIONS D217):
//! - Lines end in CRLF. A bare LF or a bare CR is refused rather than
//!   tolerated, since leniency here is where parsers come to disagree.
//! - HTTP/1.1 only. `HTTP/1.0` and every other version get `505`.
//! - obs-fold (a field line continued on the next line) is refused.
//! - No whitespace between a field name and its colon.

use crate::fields::{
    Field, Span, is_field_value_byte, is_tchar, is_token, list_elements, trim_ows,
};

/// How much a request head may contain.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// The most bytes a head may take, request line to the blank line. A head
    /// still incomplete at this size is refused with `431`.
    pub max_head_bytes: usize,
    /// The most field lines a head may carry.
    pub max_fields: usize,
    /// The longest request-target accepted; past it, `414`.
    pub max_target_bytes: usize,
}

impl Default for Limits {
    /// The server's current limits: 100 fields, and a head as large as the
    /// connection buffer it replaces allowed.
    fn default() -> Self {
        Limits {
            max_head_bytes: 8192 + 4096 * 100,
            max_fields: 100,
            max_target_bytes: 65_534,
        }
    }
}

/// The form of the request-target (RFC 9112 §3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetForm {
    /// `/path?query` — the ordinary form.
    Origin,
    /// `scheme://authority/path` — what a request to a proxy uses.
    Absolute,
    /// `host:port` — `CONNECT` only.
    Authority,
    /// `*` — `OPTIONS` only.
    Asterisk,
}

/// Where the request's body ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Body {
    /// No body.
    Empty,
    /// Exactly this many bytes follow the head.
    Length(u64),
    /// A chunked body follows ([`super::ChunkedDecoder`]).
    Chunked,
}

/// A parsed request head. Its fields are in the `Vec` given to
/// [`parse_request`]; everything else is here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestHead {
    /// The method token.
    pub method: Span,
    /// The request-target, as sent.
    pub target: Span,
    /// Which form the target took.
    pub target_form: TargetForm,
    /// The `Host` field's value (empty is allowed, RFC 9112 §3.2).
    pub host: Span,
    /// Where the body ends.
    pub body: Body,
    /// Whether the connection may carry another request after this one.
    pub keep_alive: bool,
    /// Whether the client asked for `100 Continue` before sending its body.
    pub expect_continue: bool,
    /// Whether the client asked to switch protocols (`Connection: upgrade` and
    /// an `Upgrade` field) — a WebSocket handshake, for one.
    pub upgrade: bool,
    /// How many bytes the head took, including any blank lines before it; the
    /// body (or the next request) starts here.
    pub len: usize,
}

/// Why a head was refused. [`status`](RequestError::status) is what the server
/// answers with, after which it closes the connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestError {
    /// The head breaks the grammar somewhere.
    Malformed(&'static str),
    /// A version other than HTTP/1.1.
    VersionNotSupported,
    /// More bytes than [`Limits::max_head_bytes`] without a complete head.
    HeadTooLarge,
    /// More field lines than [`Limits::max_fields`].
    TooManyFields,
    /// A request-target longer than [`Limits::max_target_bytes`].
    TargetTooLong,
    /// A transfer coding other than `chunked` — one this server does not
    /// decode (`gzip, chunked`, say).
    UnsupportedTransferCoding,
    /// An `Expect` other than `100-continue`.
    ExpectationFailed,
}

impl RequestError {
    /// The status code to answer with.
    #[must_use]
    pub fn status(&self) -> u16 {
        match self {
            RequestError::Malformed(_) => 400,
            RequestError::VersionNotSupported => 505,
            RequestError::HeadTooLarge | RequestError::TooManyFields => 431,
            RequestError::TargetTooLong => 414,
            RequestError::UnsupportedTransferCoding => 501,
            RequestError::ExpectationFailed => 417,
        }
    }
}

impl std::fmt::Display for RequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RequestError::Malformed(why) => write!(f, "malformed request: {why}"),
            RequestError::VersionNotSupported => f.write_str("HTTP version not supported"),
            RequestError::HeadTooLarge => f.write_str("request head too large"),
            RequestError::TooManyFields => f.write_str("too many header fields"),
            RequestError::TargetTooLong => f.write_str("request-target too long"),
            RequestError::UnsupportedTransferCoding => f.write_str("unsupported transfer coding"),
            RequestError::ExpectationFailed => f.write_str("unsupported expectation"),
        }
    }
}

impl std::error::Error for RequestError {}

/// The outcome of a parse that did not fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Parsed {
    /// A complete head.
    Complete(RequestHead),
    /// The head is not all here yet; read more and parse again from the start.
    Partial,
}

/// How many blank lines may come before a request line. RFC 9112 §2.2 asks a
/// server to ignore at least one (a client's stray CRLF after a body); a few
/// more costs nothing, and a bound keeps a stream of them from being free.
const MAX_LEADING_BLANK_LINES: usize = 4;

fn bad(why: &'static str) -> RequestError {
    RequestError::Malformed(why)
}

/// Parses the request head at the start of `buf`, filling `fields` (cleared
/// first) with its field lines.
///
/// # Errors
///
/// A [`RequestError`] when the head is refused; the connection is then
/// answered with its [`status`](RequestError::status) and closed.
pub fn parse_request(
    buf: &[u8],
    limits: &Limits,
    fields: &mut Vec<Field>,
) -> Result<Parsed, RequestError> {
    fields.clear();

    // Blank lines before the request line, CRLF each.
    let mut start = 0;
    for _ in 0..MAX_LEADING_BLANK_LINES {
        match buf.get(start..start + 2) {
            Some(b"\r\n") => start += 2,
            Some([b'\r', _]) | Some([b'\n', _]) => return Err(bad("stray line ending")),
            None if buf.len() > start => {
                // One byte left: a lone CR may be the start of a CRLF.
                if buf[start] == b'\r' {
                    return Ok(Parsed::Partial);
                }
                if buf[start] == b'\n' {
                    return Err(bad("bare LF"));
                }
                break;
            }
            _ => break,
        }
    }

    // The head ends at the first CRLFCRLF; until it arrives it is partial.
    let Some(end) = find_head_end(&buf[start..]) else {
        if buf.len() >= limits.max_head_bytes {
            return Err(RequestError::HeadTooLarge);
        }
        // A head that cannot end well is refused now rather than when it is
        // complete: a bare LF or CR is already wrong.
        check_line_endings(&buf[start..], false)?;
        return Ok(Parsed::Partial);
    };
    let head_end = start + end; // index of the blank line's CR
    let len = head_end + 4;
    if len > limits.max_head_bytes {
        return Err(RequestError::HeadTooLarge);
    }
    check_line_endings(&buf[start..head_end + 2], true)?;

    // ---- request line ----------------------------------------------------
    let line_end = start + find_crlf(&buf[start..]).expect("a head has a line");
    let line = &buf[start..line_end];
    let sp1 = line
        .iter()
        .position(|&b| b == b' ')
        .ok_or(bad("no method"))?;
    let method = Span {
        start,
        end: start + sp1,
    };
    if !is_token(method.of(buf)) {
        return Err(bad("invalid method"));
    }
    let rest = &line[sp1 + 1..];
    let sp2 = rest
        .iter()
        .position(|&b| b == b' ')
        .ok_or(bad("no version"))?;
    let target = Span {
        start: start + sp1 + 1,
        end: start + sp1 + 1 + sp2,
    };
    let version = &rest[sp2 + 1..];
    if target.is_empty() {
        return Err(bad("empty request-target"));
    }
    if target.len() > limits.max_target_bytes {
        return Err(RequestError::TargetTooLong);
    }
    // Visible ASCII only: no whitespace, controls, DEL or raw non-ASCII.
    if !target.of(buf).iter().all(|&b| (0x21..=0x7e).contains(&b)) {
        return Err(bad("invalid request-target"));
    }
    check_version(version)?;

    // ---- field lines -----------------------------------------------------
    parse_field_lines(buf, line_end + 2, head_end + 2, limits, fields)?;

    let mut head = RequestHead {
        method,
        target,
        target_form: TargetForm::Origin,
        host: Span::default(),
        body: Body::Empty,
        keep_alive: true,
        expect_continue: false,
        upgrade: false,
        len,
    };
    apply_semantics(buf, fields, &mut head)?;
    Ok(Parsed::Complete(head))
}

/// Parses the field lines in `buf[from..to]` — whole CRLF-ended lines whose
/// line endings are already checked — into `fields`. Shared by request heads
/// and the trailer section of a chunked body, which follow the same grammar.
pub(crate) fn parse_field_lines(
    buf: &[u8],
    from: usize,
    to: usize,
    limits: &Limits,
    fields: &mut Vec<Field>,
) -> Result<(), RequestError> {
    let mut pos = from;
    while pos < to {
        let line_end = pos + find_crlf(&buf[pos..to]).ok_or(bad("unterminated field line"))?;
        if line_end == pos {
            break; // the blank line
        }
        if matches!(buf[pos], b' ' | b'\t') {
            return Err(bad("obs-fold"));
        }
        if fields.len() == limits.max_fields {
            return Err(RequestError::TooManyFields);
        }
        fields.push(parse_field(buf, pos, line_end)?);
        pos = line_end + 2;
    }
    Ok(())
}

/// Checks that every line ending in `buf` is a CRLF (see
/// [`check_line_endings`]); `buf` ends on a line boundary.
pub(crate) fn check_complete_line_endings(buf: &[u8]) -> Result<(), RequestError> {
    check_line_endings(buf, true)
}

/// The offset of the first CRLFCRLF in `buf`.
fn find_head_end(buf: &[u8]) -> Option<usize> {
    let mut from = 0;
    while let Some(i) = buf[from..].iter().position(|&b| b == b'\n') {
        let lf = from + i;
        // `\r\n\r\n` ends at this LF when the three bytes before it are `\r\n\r`.
        if lf >= 3 && &buf[lf - 3..lf] == b"\r\n\r" {
            return Some(lf - 3);
        }
        from = lf + 1;
    }
    None
}

fn find_crlf(buf: &[u8]) -> Option<usize> {
    buf.windows(2).position(|w| w == b"\r\n")
}

/// Every LF must follow a CR and every CR must precede an LF. `complete` says
/// whether `buf` ends on a line boundary; when it does not, a trailing CR may
/// still be waiting for its LF.
fn check_line_endings(buf: &[u8], complete: bool) -> Result<(), RequestError> {
    for (i, &b) in buf.iter().enumerate() {
        match b {
            b'\n' if i == 0 || buf[i - 1] != b'\r' => return Err(bad("bare LF")),
            b'\r' => match buf.get(i + 1) {
                Some(b'\n') => {}
                None if !complete => {}
                _ => return Err(bad("bare CR")),
            },
            _ => {}
        }
    }
    Ok(())
}

fn check_version(version: &[u8]) -> Result<(), RequestError> {
    if version == b"HTTP/1.1" {
        return Ok(());
    }
    // A well-formed version that is not 1.1 is a version we refuse to speak;
    // anything else is not a version at all.
    match version {
        [b'H', b'T', b'T', b'P', b'/', major, b'.', minor]
            if major.is_ascii_digit() && minor.is_ascii_digit() =>
        {
            Err(RequestError::VersionNotSupported)
        }
        _ => Err(bad("invalid HTTP version")),
    }
}

fn parse_field(buf: &[u8], start: usize, end: usize) -> Result<Field, RequestError> {
    let line = &buf[start..end];
    let colon = line
        .iter()
        .position(|&b| !is_tchar(b))
        .ok_or(bad("field line without a colon"))?;
    if colon == 0 {
        return Err(bad("empty field name"));
    }
    if line[colon] != b':' {
        // Whitespace before the colon included (RFC 9112 §5.1: MUST reject).
        return Err(bad("invalid field name"));
    }
    let raw = &line[colon + 1..];
    if !raw.iter().all(|&b| is_field_value_byte(b)) {
        return Err(bad("invalid field value"));
    }
    let trimmed = trim_ows(raw);
    let lead = raw.iter().take_while(|&&b| b == b' ' || b == b'\t').count();
    let value_start = start + colon + 1 + lead;
    Ok(Field {
        name: Span {
            start,
            end: start + colon,
        },
        value: Span {
            start: value_start,
            end: value_start + trimmed.len(),
        },
    })
}

fn named(buf: &[u8], field: &Field, name: &[u8]) -> bool {
    field.name.of(buf).eq_ignore_ascii_case(name)
}

/// Framing, `Host`, `Connection`, `Expect`, upgrades and the target's form —
/// what the fields mean together, once each is known to be well formed.
fn apply_semantics(
    buf: &[u8],
    fields: &[Field],
    head: &mut RequestHead,
) -> Result<(), RequestError> {
    let mut content_length: Option<u64> = None;
    let mut transfer_encoding = false;
    let mut chunked_seen = false;
    let mut chunked_last = false;
    let mut other_coding = false;
    let mut hosts = 0;
    let mut connection_upgrade = false;
    let mut upgrade_field = false;

    for field in fields {
        let value = field.value.of(buf);
        if named(buf, field, b"content-length") {
            // Digits only: no sign, no list, no whitespace inside. Repeats are
            // allowed only when they agree (RFC 9110 §8.6).
            if value.is_empty() || !value.iter().all(u8::is_ascii_digit) {
                return Err(bad("invalid Content-Length"));
            }
            let mut n: u64 = 0;
            for &d in value {
                n = n
                    .checked_mul(10)
                    .and_then(|n| n.checked_add(u64::from(d - b'0')))
                    .ok_or(bad("Content-Length overflows"))?;
            }
            if content_length.is_some_and(|prior| prior != n) {
                return Err(bad("conflicting Content-Length"));
            }
            content_length = Some(n);
        } else if named(buf, field, b"transfer-encoding") {
            transfer_encoding = true;
            let mut any = false;
            for coding in list_elements(value) {
                any = true;
                if chunked_last {
                    // Something after `chunked`: the body is not chunk-framed
                    // at its outermost layer, so its end cannot be found.
                    return Err(bad("chunked is not the final transfer coding"));
                }
                if coding.eq_ignore_ascii_case(b"chunked") {
                    if chunked_seen {
                        return Err(bad("chunked applied twice"));
                    }
                    chunked_seen = true;
                    chunked_last = true;
                } else if is_token(coding) {
                    other_coding = true;
                } else {
                    return Err(bad("invalid transfer coding"));
                }
            }
            if !any {
                return Err(bad("empty Transfer-Encoding"));
            }
        } else if named(buf, field, b"host") {
            hosts += 1;
            if !value.iter().all(|&b| is_authority_byte(b)) {
                return Err(bad("invalid Host"));
            }
            head.host = field.value;
        } else if named(buf, field, b"connection") {
            for option in list_elements(value) {
                if option.eq_ignore_ascii_case(b"close") {
                    head.keep_alive = false;
                } else if option.eq_ignore_ascii_case(b"upgrade") {
                    connection_upgrade = true;
                }
            }
        } else if named(buf, field, b"upgrade") {
            upgrade_field = !value.is_empty();
        } else if named(buf, field, b"expect") {
            if value.eq_ignore_ascii_case(b"100-continue") {
                head.expect_continue = true;
            } else {
                return Err(RequestError::ExpectationFailed);
            }
        }
    }

    // Framing (RFC 9112 §6.3). Both present is a message two parsers can
    // frame two ways — the smuggling case — so it is refused, not resolved.
    head.body = match (transfer_encoding, content_length) {
        (true, Some(_)) => return Err(bad("both Transfer-Encoding and Content-Length")),
        (true, None) if !chunked_last => {
            return Err(bad("chunked is not the final transfer coding"));
        }
        (true, None) if other_coding => return Err(RequestError::UnsupportedTransferCoding),
        (true, None) => Body::Chunked,
        (false, Some(0) | None) => Body::Empty,
        (false, Some(n)) => Body::Length(n),
    };

    match hosts {
        0 => return Err(bad("missing Host")),
        1 => {}
        _ => return Err(bad("more than one Host")),
    }

    head.upgrade = connection_upgrade && upgrade_field;
    if head.upgrade && head.body != Body::Empty {
        // After the switch the connection speaks another protocol; a body
        // in between would leave the two ends disagreeing where it starts.
        return Err(bad("an upgrade request with a body"));
    }

    let method = head.method.of(buf);
    let target = head.target.of(buf);
    head.target_form = if target == b"*" {
        TargetForm::Asterisk
    } else if target[0] == b'/' {
        TargetForm::Origin
    } else if method == b"CONNECT" {
        TargetForm::Authority
    } else if is_absolute(target) {
        TargetForm::Absolute
    } else {
        return Err(bad("invalid request-target"));
    };
    match (method, head.target_form) {
        (b"OPTIONS", TargetForm::Asterisk) => {}
        (_, TargetForm::Asterisk) => return Err(bad("* is only for OPTIONS")),
        (b"CONNECT", TargetForm::Authority) => {}
        (b"CONNECT", _) => return Err(bad("CONNECT needs an authority")),
        _ => {}
    }
    Ok(())
}

/// `scheme ":" …` (RFC 3986 §3.1): a letter, then letters, digits, `+-.`.
fn is_absolute(target: &[u8]) -> bool {
    let Some(colon) = target.iter().position(|&b| b == b':') else {
        return false;
    };
    let scheme = &target[..colon];
    !scheme.is_empty()
        && scheme[0].is_ascii_alphabetic()
        && scheme
            .iter()
            .all(|&b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
}

/// A byte an authority (`host[:port]`, RFC 3986 §3.2) may contain: unreserved,
/// sub-delims, `%` for pct-encoding, `:` and the IP-literal brackets.
fn is_authority_byte(b: u8) -> bool {
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
}
