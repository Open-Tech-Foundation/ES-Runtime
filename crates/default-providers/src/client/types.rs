//! The HTTP client's vocabulary — methods, status codes, versions and header
//! maps — as the runtime's own types (DECISIONS D217): what crosses between
//! [`HttpClient`](crate::client::HttpClient)'s policy and its transports,
//! and what its callers read back.
//!
//! The shapes follow the `http` crate's, which the client used until it, and
//! hyper with it, left the shipped binary: the same names, the same
//! validation, the same `Display`. Validation is `es-runtime-http`'s
//! [`fields`](es_runtime_http::fields) — one rule for what a name or value may
//! hold, shared with both protocol codecs.

use std::borrow::Cow;
use std::fmt;

use bytes::Bytes;
use es_runtime_http::fields::{is_field_value, is_token};

/// A request method: a token, case-sensitive (RFC 9110 §9.1).
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Method(Cow<'static, str>);

impl Method {
    /// `GET`.
    pub const GET: Method = Method(Cow::Borrowed("GET"));
    /// `HEAD`.
    pub const HEAD: Method = Method(Cow::Borrowed("HEAD"));
    /// `POST`.
    pub const POST: Method = Method(Cow::Borrowed("POST"));
    /// `PUT`.
    pub const PUT: Method = Method(Cow::Borrowed("PUT"));
    /// `DELETE`.
    pub const DELETE: Method = Method(Cow::Borrowed("DELETE"));
    /// `OPTIONS`.
    pub const OPTIONS: Method = Method(Cow::Borrowed("OPTIONS"));
    /// `TRACE`.
    pub const TRACE: Method = Method(Cow::Borrowed("TRACE"));
    /// `PATCH`.
    pub const PATCH: Method = Method(Cow::Borrowed("PATCH"));

    /// A method from its bytes.
    ///
    /// # Errors
    ///
    /// When `bytes` is not a token.
    pub fn from_bytes(bytes: &[u8]) -> Result<Method, InvalidHttp> {
        if !is_token(bytes) {
            return Err(InvalidHttp("a method must be a token"));
        }
        let text = std::str::from_utf8(bytes).expect("a token is ASCII");
        Ok(Method(Cow::Owned(text.to_owned())))
    }

    /// The method as written.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Method {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Display for Method {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A response status code, `100`–`999` (RFC 9110 §15).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StatusCode(u16);

impl StatusCode {
    /// `200 OK`.
    pub const OK: StatusCode = StatusCode(200);
    /// `204 No Content`.
    pub const NO_CONTENT: StatusCode = StatusCode(204);
    /// `301 Moved Permanently`.
    pub const MOVED_PERMANENTLY: StatusCode = StatusCode(301);
    /// `302 Found`.
    pub const FOUND: StatusCode = StatusCode(302);
    /// `303 See Other`.
    pub const SEE_OTHER: StatusCode = StatusCode(303);
    /// `304 Not Modified`.
    pub const NOT_MODIFIED: StatusCode = StatusCode(304);
    /// `307 Temporary Redirect`.
    pub const TEMPORARY_REDIRECT: StatusCode = StatusCode(307);
    /// `308 Permanent Redirect`.
    pub const PERMANENT_REDIRECT: StatusCode = StatusCode(308);

    /// A status code from its number.
    ///
    /// # Errors
    ///
    /// When `code` is outside `100`–`999`.
    pub fn from_u16(code: u16) -> Result<StatusCode, InvalidHttp> {
        if (100..=999).contains(&code) {
            Ok(StatusCode(code))
        } else {
            Err(InvalidHttp("a status code is three digits, from 100"))
        }
    }

    /// The number.
    #[must_use]
    pub fn as_u16(self) -> u16 {
        self.0
    }

    /// `2xx`.
    #[must_use]
    pub fn is_success(self) -> bool {
        (200..300).contains(&self.0)
    }

    /// `3xx`.
    #[must_use]
    pub fn is_redirection(self) -> bool {
        (300..400).contains(&self.0)
    }

    /// `5xx`.
    #[must_use]
    pub fn is_server_error(self) -> bool {
        (500..600).contains(&self.0)
    }

    /// The reason phrase RFC 9110 §15 gives the code, when it gives one.
    #[must_use]
    pub fn canonical_reason(self) -> Option<&'static str> {
        Some(match self.0 {
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
            511 => "Network Authentication Required",
            _ => return None,
        })
    }
}

impl fmt::Debug for StatusCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// `404 Not Found`, or the number alone for a code with no standard reason.
impl fmt::Display for StatusCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.canonical_reason() {
            Some(reason) => write!(f, "{} {reason}", self.0),
            None => write!(f, "{}", self.0),
        }
    }
}

impl PartialEq<u16> for StatusCode {
    fn eq(&self, other: &u16) -> bool {
        self.0 == *other
    }
}

/// The HTTP version a response arrived over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Version {
    /// HTTP/1.1.
    Http11,
    /// HTTP/2.
    Http2,
}

/// A field name: a token, kept lowercase (RFC 9110 §5.1: case-insensitive).
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct HeaderName(Cow<'static, str>);

impl HeaderName {
    /// A name from its bytes, lowercased.
    ///
    /// # Errors
    ///
    /// When `bytes` is not a token.
    pub fn from_bytes(bytes: &[u8]) -> Result<HeaderName, InvalidHttp> {
        if !is_token(bytes) {
            return Err(InvalidHttp("a field name must be a token"));
        }
        let text = std::str::from_utf8(bytes).expect("a token is ASCII");
        Ok(HeaderName(Cow::Owned(text.to_ascii_lowercase())))
    }

    /// A name known at compile time. It must be a lowercase token; this is
    /// checked, and a wrong one is a bug, so it panics.
    #[must_use]
    pub const fn from_static(name: &'static str) -> HeaderName {
        let bytes = name.as_bytes();
        assert!(!bytes.is_empty(), "a field name must not be empty");
        let mut i = 0;
        while i < bytes.len() {
            let b = bytes[i];
            assert!(
                b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_' || b == b'.',
                "a static field name is a lowercase token"
            );
            i += 1;
        }
        HeaderName(Cow::Borrowed(name))
    }

    /// The name, lowercase.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for HeaderName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for HeaderName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Display for HeaderName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The field names the client names, lowercase, as the `http` crate's
/// `header` module had them.
pub mod header {
    use super::HeaderName;

    /// `accept-encoding`.
    pub const ACCEPT_ENCODING: HeaderName = HeaderName::from_static("accept-encoding");
    /// `authorization`.
    pub const AUTHORIZATION: HeaderName = HeaderName::from_static("authorization");
    /// `content-encoding`.
    pub const CONTENT_ENCODING: HeaderName = HeaderName::from_static("content-encoding");
    /// `content-language`.
    pub const CONTENT_LANGUAGE: HeaderName = HeaderName::from_static("content-language");
    /// `content-length`.
    pub const CONTENT_LENGTH: HeaderName = HeaderName::from_static("content-length");
    /// `content-location`.
    pub const CONTENT_LOCATION: HeaderName = HeaderName::from_static("content-location");
    /// `content-type`.
    pub const CONTENT_TYPE: HeaderName = HeaderName::from_static("content-type");
    /// `cookie`.
    pub const COOKIE: HeaderName = HeaderName::from_static("cookie");
    /// `location`.
    pub const LOCATION: HeaderName = HeaderName::from_static("location");
    /// `proxy-authorization`.
    pub const PROXY_AUTHORIZATION: HeaderName = HeaderName::from_static("proxy-authorization");
    /// `range`.
    pub const RANGE: HeaderName = HeaderName::from_static("range");
    /// `te`.
    pub const TE: HeaderName = HeaderName::from_static("te");
    /// `user-agent`.
    pub const USER_AGENT: HeaderName = HeaderName::from_static("user-agent");
    /// `www-authenticate`.
    pub const WWW_AUTHENTICATE: HeaderName = HeaderName::from_static("www-authenticate");
}

/// A field value: visible ASCII, space, tab and obs-text — never NUL, CR, LF
/// or another control, which is what request and response splitting is made
/// of (RFC 9110 §5.5).
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct HeaderValue(Bytes);

impl HeaderValue {
    /// A value from its bytes.
    ///
    /// # Errors
    ///
    /// When `bytes` holds a byte a field value may not.
    pub fn from_bytes(bytes: &[u8]) -> Result<HeaderValue, InvalidHttp> {
        if !is_field_value(bytes) {
            return Err(InvalidHttp("a field value holds a control character"));
        }
        Ok(HeaderValue(Bytes::copy_from_slice(bytes)))
    }

    /// A value known at compile time. A control character in it is a bug, so
    /// it panics.
    #[must_use]
    pub fn from_static(text: &'static str) -> HeaderValue {
        assert!(
            is_field_value(text.as_bytes()),
            "a static field value holds a control character"
        );
        HeaderValue(Bytes::from_static(text.as_bytes()))
    }

    /// The value as text, when it is visible ASCII, space and tab only.
    ///
    /// # Errors
    ///
    /// When it holds obs-text (bytes from 0x80), which has no one reading.
    pub fn to_str(&self) -> Result<&str, InvalidHttp> {
        if self
            .0
            .iter()
            .all(|&b| b == b'\t' || (0x20..0x7f).contains(&b))
        {
            Ok(std::str::from_utf8(&self.0).expect("ASCII"))
        } else {
            Err(InvalidHttp("a field value that is not visible ASCII"))
        }
    }

    /// The value's bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Its length in bytes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether it is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// A value from text, as [`HeaderValue::from_bytes`] checks it.
impl std::str::FromStr for HeaderValue {
    type Err = InvalidHttp;

    fn from_str(text: &str) -> Result<HeaderValue, InvalidHttp> {
        Self::from_bytes(text.as_bytes())
    }
}

impl fmt::Debug for HeaderValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", String::from_utf8_lossy(&self.0))
    }
}

impl PartialEq<str> for HeaderValue {
    fn eq(&self, other: &str) -> bool {
        self.0 == other.as_bytes()
    }
}

impl PartialEq<&str> for HeaderValue {
    fn eq(&self, other: &&str) -> bool {
        self.0 == other.as_bytes()
    }
}

/// Header fields in the order they were added, repeats kept; looked up by
/// name case-insensitively (names are stored lowercase).
#[derive(Clone, Default, PartialEq, Eq)]
pub struct HeaderMap(Vec<(HeaderName, HeaderValue)>);

impl HeaderMap {
    /// An empty map.
    #[must_use]
    pub fn new() -> Self {
        HeaderMap(Vec::new())
    }

    /// An empty map with room for `n` fields.
    #[must_use]
    pub fn with_capacity(n: usize) -> Self {
        HeaderMap(Vec::with_capacity(n))
    }

    fn matches(name: &HeaderName, wanted: &str) -> bool {
        name.as_str().eq_ignore_ascii_case(wanted)
    }

    /// The first value of `name`.
    pub fn get(&self, name: impl AsRef<str>) -> Option<&HeaderValue> {
        let name = name.as_ref();
        self.0
            .iter()
            .find(|(n, _)| Self::matches(n, name))
            .map(|(_, v)| v)
    }

    /// Every value of `name`, in order.
    pub fn get_all(&self, name: impl AsRef<str>) -> impl Iterator<Item = &HeaderValue> {
        let name = name.as_ref().to_ascii_lowercase();
        self.0
            .iter()
            .filter(move |(n, _)| n.as_str() == name)
            .map(|(_, v)| v)
    }

    /// Whether `name` is present.
    pub fn contains_key(&self, name: impl AsRef<str>) -> bool {
        self.get(name).is_some()
    }

    /// Sets `name` to `value` alone: every earlier value is removed, and the
    /// first of them returned. The field keeps its place if it had one.
    pub fn insert(&mut self, name: HeaderName, value: HeaderValue) -> Option<HeaderValue> {
        match self.0.iter().position(|(n, _)| *n == name) {
            Some(at) => {
                let old = std::mem::replace(&mut self.0[at].1, value);
                let mut i = at + 1;
                while i < self.0.len() {
                    if self.0[i].0 == name {
                        self.0.remove(i);
                    } else {
                        i += 1;
                    }
                }
                Some(old)
            }
            None => {
                self.0.push((name, value));
                None
            }
        }
    }

    /// Adds a value for `name` after any it already has.
    pub fn append(&mut self, name: HeaderName, value: HeaderValue) {
        self.0.push((name, value));
    }

    /// Removes every value of `name`, returning the first.
    pub fn remove(&mut self, name: impl AsRef<str>) -> Option<HeaderValue> {
        let name = name.as_ref();
        let mut first = None;
        self.0.retain(|(n, v)| {
            if Self::matches(n, name) {
                if first.is_none() {
                    first = Some(v.clone());
                }
                false
            } else {
                true
            }
        });
        first
    }

    /// Every field, in order.
    pub fn iter(&self) -> impl Iterator<Item = (&HeaderName, &HeaderValue)> {
        self.0.iter().map(|(n, v)| (n, v))
    }

    /// Every distinct name, in the order each first appears.
    pub fn keys(&self) -> impl Iterator<Item = &HeaderName> {
        let mut seen: Vec<&HeaderName> = Vec::new();
        self.0.iter().filter_map(move |(n, _)| {
            if seen.contains(&n) {
                None
            } else {
                seen.push(n);
                Some(n)
            }
        })
    }

    /// How many fields, repeats counted.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether there are none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl<'a> IntoIterator for &'a HeaderMap {
    type Item = (&'a HeaderName, &'a HeaderValue);
    type IntoIter = std::iter::Map<
        std::slice::Iter<'a, (HeaderName, HeaderValue)>,
        fn(&'a (HeaderName, HeaderValue)) -> (&'a HeaderName, &'a HeaderValue),
    >;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter().map(|(n, v)| (n, v))
    }
}

impl fmt::Debug for HeaderMap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

/// A name, value, method or status the HTTP grammar does not allow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidHttp(&'static str);

impl fmt::Display for InvalidHttp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for InvalidHttp {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_tokens_kept_lowercase() {
        assert_eq!(
            HeaderName::from_bytes(b"Content-Type").unwrap().as_str(),
            "content-type"
        );
        for bad in [&b""[..], b"bad name", b"x:y", b"caf\xc3\xa9"] {
            assert!(HeaderName::from_bytes(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn values_refuse_what_splits_a_message() {
        assert!(HeaderValue::from_bytes(b"text/html; charset=utf-8").is_ok());
        assert!(HeaderValue::from_bytes(b"tab\tok").is_ok());
        // obs-text is a value, though not text.
        let obs = HeaderValue::from_bytes(b"caf\xc3\xa9").unwrap();
        assert!(obs.to_str().is_err());
        for bad in [&b"a\r\nb"[..], b"nul\0", b"del\x7f", b"bell\x07"] {
            assert!(HeaderValue::from_bytes(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_map_keeps_order_and_repeats_and_looks_up_without_case() {
        let mut map = HeaderMap::new();
        let cookie = HeaderName::from_bytes(b"Set-Cookie").unwrap();
        map.append(cookie.clone(), HeaderValue::from_static("a=1"));
        map.append(header::CONTENT_TYPE, HeaderValue::from_static("text/plain"));
        map.append(cookie.clone(), HeaderValue::from_static("b=2"));
        assert_eq!(map.get("SET-COOKIE").unwrap(), "a=1");
        assert_eq!(
            map.get_all("set-cookie").collect::<Vec<_>>(),
            ["a=1", "b=2"]
        );
        assert_eq!(
            map.keys().map(HeaderName::as_str).collect::<Vec<_>>(),
            ["set-cookie", "content-type"]
        );
        // insert replaces every value and keeps the field's place.
        assert_eq!(
            map.insert(cookie.clone(), HeaderValue::from_static("c=3"))
                .unwrap(),
            "a=1"
        );
        assert_eq!(
            map.iter()
                .map(|(n, v)| (n.as_str(), v.to_str().unwrap()))
                .collect::<Vec<_>>(),
            [("set-cookie", "c=3"), ("content-type", "text/plain")]
        );
        assert_eq!(map.remove("Content-Type").unwrap(), "text/plain");
        assert_eq!(map.len(), 1);
        assert!(!map.contains_key(header::CONTENT_TYPE));
    }

    #[test]
    fn statuses_display_as_the_http_crate_did() {
        assert_eq!(
            StatusCode::from_u16(404).unwrap().to_string(),
            "404 Not Found"
        );
        assert_eq!(StatusCode::from_u16(599).unwrap().to_string(), "599");
        assert!(StatusCode::from_u16(99).is_err());
        assert!(StatusCode::from_u16(1000).is_err());
        let s = StatusCode::from_u16(503).unwrap();
        assert!(s.is_server_error() && !s.is_success());
    }

    #[test]
    fn methods_are_case_sensitive_tokens() {
        assert_eq!(Method::from_bytes(b"GET").unwrap(), Method::GET);
        assert_ne!(Method::from_bytes(b"get").unwrap(), Method::GET);
        assert!(Method::from_bytes(b"G T").is_err());
    }
}
