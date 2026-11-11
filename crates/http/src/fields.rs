//! Header fields as every HTTP version shares them (RFC 9110 §5).
//!
//! One validator decides what a field may contain, so a field refused on one
//! version is refused on all of them, and the server and the client agree.

/// A byte range into a message buffer. Parsers return these rather than copies,
/// so a head is parsed without allocating its contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    /// The first byte.
    pub start: usize,
    /// One past the last byte.
    pub end: usize,
}

impl Span {
    /// The bytes this span covers in `buf`.
    ///
    /// # Panics
    ///
    /// If the span is not within `buf` — a span is only ever applied to the
    /// buffer that produced it.
    #[must_use]
    pub fn of<'a>(&self, buf: &'a [u8]) -> &'a [u8] {
        &buf[self.start..self.end]
    }

    /// The number of bytes covered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.end - self.start
    }

    /// Whether the span covers nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }
}

/// One field line: where its name and its value are.
///
/// The value excludes the optional whitespace around it (RFC 9110 §5.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Field {
    /// The field name, as sent (case preserved).
    pub name: Span,
    /// The field value, without leading or trailing whitespace.
    pub value: Span,
}

/// Whether `b` may appear in a token (RFC 9110 §5.6.2 `tchar`).
#[must_use]
pub const fn is_tchar(b: u8) -> bool {
    matches!(b,
        b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+' | b'-' | b'.'
        | b'^' | b'_' | b'`' | b'|' | b'~'
        | b'0'..=b'9' | b'a'..=b'z' | b'A'..=b'Z')
}

/// Whether `s` is a token: one or more `tchar`. Field names and methods are
/// tokens.
#[must_use]
pub fn is_token(s: &[u8]) -> bool {
    !s.is_empty() && s.iter().all(|&b| is_tchar(b))
}

/// Whether `b` may appear inside a field value (RFC 9110 §5.5): visible ASCII,
/// space, horizontal tab and obs-text. Everything else — NUL, CR, LF and the
/// other controls — is what request and response splitting is made of, and is
/// refused.
#[must_use]
pub const fn is_field_value_byte(b: u8) -> bool {
    matches!(b, b'\t' | b' ' | 0x21..=0x7e | 0x80..=0xff)
}

/// Whether every byte of `s` may appear in a field value.
#[must_use]
pub fn is_field_value(s: &[u8]) -> bool {
    s.iter().all(|&b| is_field_value_byte(b))
}

/// `s` without leading and trailing optional whitespace (space and tab).
#[must_use]
pub fn trim_ows(s: &[u8]) -> &[u8] {
    let start = s
        .iter()
        .position(|&b| b != b' ' && b != b'\t')
        .unwrap_or(s.len());
    let end = s
        .iter()
        .rposition(|&b| b != b' ' && b != b'\t')
        .map_or(start, |i| i + 1);
    &s[start..end]
}

/// The elements of a comma-separated list value (RFC 9110 §5.6.1), each with
/// its whitespace trimmed. Empty elements are skipped, as the grammar allows.
pub fn list_elements(value: &[u8]) -> impl Iterator<Item = &[u8]> {
    value
        .split(|&b| b == b',')
        .map(trim_ows)
        .filter(|e| !e.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_the_tchar_set() {
        assert!(is_token(b"Content-Type"));
        assert!(is_token(b"x!#$%&'*+-.^_`|~9"));
        for bad in [
            &b""[..],
            b"a b",
            b"a:b",
            b"a\"b",
            b"a(b",
            b"a\x7fb",
            b"\xc3\xa9",
        ] {
            assert!(!is_token(bad), "{bad:?}");
        }
    }

    #[test]
    fn field_values_refuse_controls_but_keep_obs_text() {
        assert!(is_field_value(b"text/plain; charset=utf-8"));
        assert!(is_field_value(b"a\tb \x80\xff"));
        for bad in [&b"a\rb"[..], b"a\nb", b"a\0b", b"a\x01b", b"a\x7fb"] {
            assert!(!is_field_value(bad), "{bad:?}");
        }
    }

    #[test]
    fn whitespace_is_trimmed_from_both_ends_only() {
        assert_eq!(trim_ows(b" \t a b \t"), b"a b");
        assert_eq!(trim_ows(b"   "), b"");
        assert_eq!(trim_ows(b""), b"");
    }

    #[test]
    fn list_elements_skip_empties() {
        let got: Vec<&[u8]> = list_elements(b" gzip , ,chunked,").collect();
        assert_eq!(got, [&b"gzip"[..], b"chunked"]);
    }
}
