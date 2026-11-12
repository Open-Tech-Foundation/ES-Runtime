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

/// The byte classes as lookup tables: a parser asks for every byte of a head,
/// and one load beats a chain of range comparisons.
const fn class_table(tchar: bool) -> [bool; 256] {
    let mut t = [false; 256];
    let mut b = 0usize;
    while b < 256 {
        let c = b as u8;
        t[b] = if tchar {
            matches!(c,
                b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+' | b'-' | b'.'
                | b'^' | b'_' | b'`' | b'|' | b'~'
                | b'0'..=b'9' | b'a'..=b'z' | b'A'..=b'Z')
        } else {
            matches!(c, b'\t' | b' ' | 0x21..=0x7e | 0x80..=0xff)
        };
        b += 1;
    }
    t
}

static TCHAR: [bool; 256] = class_table(true);
static FIELD_VALUE: [bool; 256] = class_table(false);

/// Whether `b` may appear in a token (RFC 9110 §5.6.2 `tchar`).
#[must_use]
#[inline]
pub fn is_tchar(b: u8) -> bool {
    TCHAR[b as usize]
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
#[inline]
pub fn is_field_value_byte(b: u8) -> bool {
    FIELD_VALUE[b as usize]
}

/// Whether every byte of `s` may appear in a field value.
#[must_use]
pub fn is_field_value(s: &[u8]) -> bool {
    s.iter().all(|&b| is_field_value_byte(b))
}

/// Whether `s` is exactly an authority a `Host` field (RFC 9110 §7.2) or an
/// HTTP/2 `:authority` may carry: a host and an optional port, nothing more
/// (RFC 3986 §3.2, without userinfo).
///
/// The host is an IPv6 address in brackets, or a name or IPv4 address made of
/// unreserved characters and sub-delims. Everything that would change what a
/// URL built from it means is refused — `/`, `?`, `#`, `@` and whitespace — and
/// so is `%`: a URL parser percent-decodes a host, so an escape here would be
/// checked as one string and used as another. The port is digits, at most
/// 65535, written without leading zeros, so that it reads one way only.
#[must_use]
pub fn is_authority(s: &str) -> bool {
    let (host, port) = if let Some(rest) = s.strip_prefix('[') {
        let Some((v6, after)) = rest.split_once(']') else {
            return false;
        };
        if v6.parse::<std::net::Ipv6Addr>().is_err() {
            return false;
        }
        match after {
            "" => return true,
            _ => match after.strip_prefix(':') {
                Some(port) => ("[ok]", Some(port)),
                None => return false,
            },
        }
    } else {
        match s.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (s, None),
        }
    };
    let host_ok = host == "[ok]"
        || (!host.is_empty()
            && host.bytes().all(|b| {
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
                    )
            }));
    let port_ok = port.is_none_or(|p| {
        !p.is_empty()
            && p.bytes().all(|b| b.is_ascii_digit())
            && p.parse::<u16>().is_ok_and(|n| n.to_string() == p)
    });
    host_ok && port_ok
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

    #[test]
    fn an_authority_is_a_host_and_a_port_and_nothing_else() {
        for good in [
            "example.com",
            "example.com:8080",
            "127.0.0.1:80",
            "[::1]:8080",
            "[::1]",
            "[2001:db8::7]:443",
            "sub.example.com.",
            "localhost:0",
            "xn--bcher-kva.example",
            "a_b.example:65535",
        ] {
            assert!(is_authority(good), "{good} was refused");
        }
        for bad in [
            // The forgeries: everything after the host is a different request.
            "example.com/admin",
            "example.com/admin?x=1",
            "example.com?x=1",
            "example.com#f",
            // Userinfo — illegal in `Host`, and the visible half of the URL.
            "evil.com@real.host",
            "evil.com@real.host:443",
            // Decoded by a URL parser into something this check never saw.
            "a%2Fb.example",
            // Not an authority at all.
            "exa mple.com",
            "example.com:notaport",
            "example.com:",
            "example.com:65536",
            "example.com:080",
            "example.com:+80",
            ":80",
            "[::1",
            "[::1]x",
            "[::1]:",
            "[zz::1]",
            "[fe80::1%25eth0]",
            "a:b:c",
            "",
        ] {
            assert!(!is_authority(bad), "{bad} was accepted");
        }
    }
}
