//! What makes an HTTP/2 field section well formed (RFC 9113 §8.2, §8.3).
//!
//! A malformed request or response is a stream error, `PROTOCOL_ERROR`
//! (§8.1.1): the stream is reset and the connection carries on. The rules are
//! checked on the decoded list, before anything above sees it.

use super::{ErrorCode, FieldList, H2Error};

/// The pseudo-header fields of a message (§8.3), split from its fields.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pseudo {
    /// `:method` (requests).
    pub method: Option<Vec<u8>>,
    /// `:scheme` (requests).
    pub scheme: Option<Vec<u8>>,
    /// `:authority` (requests).
    pub authority: Option<Vec<u8>>,
    /// `:path` (requests).
    pub path: Option<Vec<u8>>,
    /// `:status` (responses).
    pub status: Option<u16>,
}

/// What kind of field section is being checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Section {
    Request,
    Response,
    /// Trailers carry no pseudo-header fields.
    Trailers,
}

fn malformed(stream: u32, why: &'static str) -> H2Error {
    H2Error::Stream {
        stream,
        code: ErrorCode::ProtocolError,
        why,
    }
}

/// A field name HTTP/2 accepts (§8.2.1): no uppercase, no controls, space,
/// DEL or non-ASCII, and a colon only as a pseudo-header's first byte.
fn valid_name(name: &[u8]) -> bool {
    !name.is_empty()
        && name.iter().enumerate().all(|(i, &b)| {
            crate::fields::is_tchar(b) && !b.is_ascii_uppercase() || (i == 0 && b == b':')
        })
}

/// A field value HTTP/2 accepts (§8.2.1): no NUL, CR or LF, and no
/// whitespace at either end.
fn valid_value(value: &[u8]) -> bool {
    !value.iter().any(|&b| matches!(b, 0 | b'\r' | b'\n'))
        && !value.first().is_some_and(|&b| b == b' ' || b == b'\t')
        && !value.last().is_some_and(|&b| b == b' ' || b == b'\t')
}

/// The headers HTTP/2 forbids because they describe an HTTP/1.1 connection
/// (§8.2.2).
fn connection_specific(name: &[u8]) -> bool {
    matches!(
        name,
        b"connection" | b"keep-alive" | b"proxy-connection" | b"transfer-encoding" | b"upgrade"
    )
}

/// Checks a decoded field section and splits its pseudo-header fields out.
/// Returns them, and the regular fields in order.
pub(crate) fn check(
    stream: u32,
    section: Section,
    list: Vec<(Vec<u8>, Vec<u8>)>,
) -> Result<(Pseudo, FieldList), H2Error> {
    let mut pseudo = Pseudo::default();
    let mut fields = Vec::with_capacity(list.len());
    for (name, value) in list {
        if !valid_name(&name) {
            return Err(malformed(stream, "invalid field name"));
        }
        if !valid_value(&value) {
            return Err(malformed(stream, "invalid field value"));
        }
        if name[0] == b':' {
            if !fields.is_empty() {
                return Err(malformed(stream, "a pseudo-header after a regular field"));
            }
            let slot = match (section, name.as_slice()) {
                (Section::Request, b":method") => &mut pseudo.method,
                (Section::Request, b":scheme") => &mut pseudo.scheme,
                (Section::Request, b":authority") => &mut pseudo.authority,
                (Section::Request, b":path") => &mut pseudo.path,
                (Section::Response, b":status") => {
                    if pseudo.status.is_some() {
                        return Err(malformed(stream, "a repeated pseudo-header"));
                    }
                    let digits: [u8; 3] = value
                        .as_slice()
                        .try_into()
                        .map_err(|_| malformed(stream, "invalid :status"))?;
                    if !digits.iter().all(u8::is_ascii_digit) {
                        return Err(malformed(stream, "invalid :status"));
                    }
                    let status = digits
                        .iter()
                        .fold(0u16, |n, &d| n * 10 + u16::from(d - b'0'));
                    if status < 100 {
                        return Err(malformed(stream, "invalid :status"));
                    }
                    pseudo.status = Some(status);
                    continue;
                }
                _ => {
                    return Err(malformed(
                        stream,
                        "a pseudo-header that does not belong here",
                    ));
                }
            };
            if slot.is_some() {
                return Err(malformed(stream, "a repeated pseudo-header"));
            }
            *slot = Some(value);
            continue;
        }
        if connection_specific(&name) {
            return Err(malformed(stream, "a connection-specific header field"));
        }
        if name == b"te" && value != b"trailers" {
            return Err(malformed(stream, "TE other than trailers"));
        }
        fields.push((name, value));
    }
    match section {
        Section::Request => {
            let method = pseudo
                .method
                .as_deref()
                .ok_or(malformed(stream, "no :method"))?;
            if method == b"CONNECT" {
                // §8.5: only :authority, no :scheme or :path.
                if pseudo.authority.is_none() || pseudo.scheme.is_some() || pseudo.path.is_some() {
                    return Err(malformed(stream, "CONNECT carries only :authority"));
                }
            } else {
                if pseudo.scheme.is_none() {
                    return Err(malformed(stream, "no :scheme"));
                }
                match pseudo.path.as_deref() {
                    None | Some(b"") => return Err(malformed(stream, "no :path")),
                    Some(_) => {}
                }
            }
        }
        Section::Response => {
            if pseudo.status.is_none() {
                return Err(malformed(stream, "no :status"));
            }
        }
        Section::Trailers => {}
    }
    Ok((pseudo, fields))
}

/// The `content-length` of a field section, when it has one (§8.1.1). Several
/// that disagree, or one that is not a number, make the message malformed.
pub(crate) fn content_length(
    stream: u32,
    fields: &[(Vec<u8>, Vec<u8>)],
) -> Result<Option<u64>, H2Error> {
    let mut length = None;
    for (_, value) in fields.iter().filter(|(n, _)| n == b"content-length") {
        if value.is_empty() || !value.iter().all(u8::is_ascii_digit) || value.len() > 19 {
            return Err(malformed(stream, "invalid content-length"));
        }
        let n = value
            .iter()
            .fold(0u64, |n, &d| n * 10 + u64::from(d - b'0'));
        if length.is_some_and(|prior| prior != n) {
            return Err(malformed(stream, "conflicting content-length"));
        }
        length = Some(n);
    }
    Ok(length)
}
