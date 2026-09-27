//! Static-file HTTP semantics: validators, conditionals, ranges.
//!
//! Pure functions over a request head that is already in memory and file
//! metadata from a single `metadata()` call. Nothing here touches the
//! filesystem or a socket, so nothing here can add a syscall to the dev
//! server's hot path — the only cost of a plain `GET` is the header scan it
//! already paid to route the request. [`crate::devserver`] owns the
//! connections and the reads; this module owns the decisions.
//!
//! The scope is deliberately the full set (single ranges, suffix and open
//! ranges, multi-range bodies, `If-Range`, both conditionals): a preview that
//! answers `<video>` seeks and revalidations the way production does is
//! answering about the build rather than about itself. What stays out is
//! anything with state — no stat cache (build outputs change constantly, and
//! an ETag from size plus mtime already costs nothing to mint).

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// More ranges than this in one header and the header is ignored, answered
/// with the full representation. A server MAY ignore a `Range` header, and an
/// unbounded multipart assembly from one line of a request head is not
/// something a dev server builds.
pub const MAX_RANGES: usize = 32;

/// An inclusive byte span of a representation, `start..=end`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Span {
    pub start: u64,
    pub end: u64,
}

impl Span {
    /// The number of bytes the span covers.
    pub fn len(self) -> u64 {
        self.end - self.start + 1
    }
}

/// What a `Range` header field amounts to, once the representation length is
/// known.
#[derive(Debug, PartialEq, Eq)]
pub enum RangeOutcome {
    /// No usable ranges: absent, a unit other than `bytes`, syntactically
    /// invalid, or more than [`MAX_RANGES`]. The caller serves the full
    /// representation — ignoring the header is always legal.
    Ignored,
    /// Syntactically valid, but nothing overlaps the representation.
    Unsatisfiable,
    /// One or more spans to serve, in the order asked.
    Satisfiable(Vec<Span>),
}

/// The answer for a file request, before the body is read.
#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    /// The full representation, `200`.
    Full,
    /// A validator matched, `304` with no body.
    NotModified,
    /// Ranges asked for nothing satisfiable, `416`.
    Unsatisfiable,
    /// One span, `206`.
    Single(Span),
    /// Several spans, `206` with a `multipart/byteranges` body.
    Multi(Vec<Span>),
}

/// A weak ETag from size and mtime: `W/"len-secs.nanos"`.
///
/// Content-hashed ETags would read every file on every request to mint a
/// validator nobody asked to cache — build outputs are fingerprinted by name
/// already, so the validator only has to catch "edited since". Size plus the
/// full mtime does that, including the same-size rewrite within one second
/// that second precision alone would miss.
pub fn etag(len: u64, mtime: SystemTime) -> String {
    let (secs, nanos) = mtime
        .duration_since(UNIX_EPOCH)
        .map(|d| (d.as_secs(), d.subsec_nanos()))
        .unwrap_or((0, 0));
    format!("W/\"{len}-{secs}.{nanos:09}\"")
}

/// A file mtime as an IMF-fixdate, for `Last-Modified`.
pub fn last_modified(mtime: SystemTime) -> String {
    http_date(mtime)
}

/// Seconds since the epoch, saturating rather than panicking on the
/// pre-epoch times a filesystem can in principle report.
fn secs(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Formats a time as an IMF-fixdate (`Sun, 06 Nov 1994 08:49:37 GMT`).
///
/// By hand rather than a date crate: the dev server formats one stamp per
/// file response, and this is sixty lines with no new dependency to audit.
pub fn http_date(t: SystemTime) -> String {
    let secs = secs(t);
    let days = secs / 86_400;
    let clock = secs % 86_400;
    let (y, m, d) = ymd(days);
    // Day 0 was a Thursday.
    const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun",
        "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    format!(
        "{}, {:02} {} {:04} {:02}:{:02}:{:02} GMT",
        DAYS[(days % 7) as usize],
        d,
        MONTHS[(m - 1) as usize],
        y,
        clock / 3600,
        (clock % 3600) / 60,
        clock % 60,
    )
}

/// Days since the epoch to a civil date (Howard Hinnant's algorithm).
fn ymd(days: u64) -> (i64, u64, u64) {
    let z = days as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Parses the three HTTP date formats (IMF-fixdate, RFC 850, asctime), which
/// is what an `If-Modified-Since` or `If-Range` date can arrive as. Anything
/// else is `None`, and the caller treats the header as absent.
pub fn parse_http_date(text: &str) -> Option<SystemTime> {
    let text = text.trim();
    parse_imf(text)
        .or_else(|| parse_rfc850(text))
        .or_else(|| parse_asctime(text))
}

fn month(word: &str) -> Option<u64> {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun",
        "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    MONTHS.iter().position(|m| m.eq_ignore_ascii_case(word)).map(|i| i as u64 + 1)
}

fn days_from_civil(y: i64, m: u64, d: u64) -> Option<u64> {
    // HTTP dates name real moments: four-digit years from the epoch on.
    // Bounding first is what keeps the arithmetic below total — a header can
    // name any year at all, and an unbounded `i64` reaches overflow in the
    // era multiplication below.
    if !(1970..=9999).contains(&y) || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = (era as u64)
        .checked_mul(146_097)?
        .checked_add(doe)?
        .checked_sub(719_468)?;
    Some(days)
}

fn hms(h: &str, m: &str, s: &str) -> Option<u64> {
    let h: u64 = h.parse().ok()?;
    let m: u64 = m.parse().ok()?;
    let s: u64 = s.parse().ok()?;
    if h > 23 || m > 59 || s > 60 {
        return None;
    }
    Some(h * 3600 + m * 60 + s)
}

/// `Sun, 06 Nov 1994 08:49:37 GMT`
fn parse_imf(text: &str) -> Option<SystemTime> {
    let (day_name, rest) = text.split_once(',')?;
    if day_name.len() != 3 {
        return None;
    }
    let rest = rest.trim();
    let mut parts = rest.split_whitespace();
    let d: u64 = parts.next()?.parse().ok()?;
    let m = month(parts.next()?)?;
    let y: i64 = parts.next()?.parse().ok()?;
    let time = parts.next()?;
    // The trailing `GMT`: required by the format, and the only zone HTTP
    // dates ever name.
    if !parts.next()?.eq_ignore_ascii_case("gmt") || parts.next().is_some() {
        return None;
    }
    let mut t = time.split(':');
    let clock = hms(t.next()?, t.next()?, t.next()?)?;
    if t.next().is_some() {
        return None;
    }
    let days = days_from_civil(y, m, d)?;
    let secs = days.checked_mul(86_400)?.checked_add(clock)?;
    Some(UNIX_EPOCH + Duration::from_secs(secs))
}

/// `Sunday, 06-Nov-94 08:49:37 GMT`
fn parse_rfc850(text: &str) -> Option<SystemTime> {
    let rest = text.split_once(',')?.1.trim();
    let mut parts = rest.split_whitespace();
    let date = parts.next()?;
    let time = parts.next()?;
    if !parts.next()?.eq_ignore_ascii_case("gmt") || parts.next().is_some() {
        return None;
    }
    let mut d = date.split('-');
    let day: u64 = d.next()?.parse().ok()?;
    let m = month(d.next()?)?;
    let yy: i64 = d.next()?.parse().ok()?;
    if d.next().is_some() || !(0..=99).contains(&yy) {
        return None;
    }
    // Two-digit years land in 1969..2068, per the HTTP rule of thumb.
    let y = if yy >= 69 { 1900 + yy } else { 2000 + yy };
    let mut t = time.split(':');
    let clock = hms(t.next()?, t.next()?, t.next()?)?;
    if t.next().is_some() {
        return None;
    }
    let days = days_from_civil(y, m, day)?;
    let secs = days.checked_mul(86_400)?.checked_add(clock)?;
    Some(UNIX_EPOCH + Duration::from_secs(secs))
}

/// `Sun Nov  6 08:49:37 1994`
fn parse_asctime(text: &str) -> Option<SystemTime> {
    let mut parts = text.split_whitespace();
    if parts.next()?.len() != 3 {
        return None;
    }
    let m = month(parts.next()?)?;
    let d: u64 = parts.next()?.parse().ok()?;
    let time = parts.next()?;
    let y: i64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    let mut t = time.split(':');
    let clock = hms(t.next()?, t.next()?, t.next()?)?;
    if t.next().is_some() {
        return None;
    }
    let days = days_from_civil(y, m, d)?;
    let secs = days.checked_mul(86_400)?.checked_add(clock)?;
    Some(UNIX_EPOCH + Duration::from_secs(secs))
}

/// The value of the first header field named `name` (case-insensitive), or
/// `None`. Scans the head already in memory — no allocation, no second read.
pub fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    for line in head.lines().skip(1) {
        if line.is_empty() {
            continue;
        }
        let Some((field, value)) = line.split_once(':') else {
            continue;
        };
        if field.trim().eq_ignore_ascii_case(name) {
            return Some(value.trim());
        }
    }
    None
}

/// Whether an `If-None-Match` value matches `etag`, by the weak comparison
/// conditional `GET` uses: `*` matches everything, otherwise the opaque tags
/// match with any `W/` prefixes disregarded.
pub fn etag_matches(value: &str, etag: &str) -> bool {
    let ours = etag.strip_prefix("W/").unwrap_or(etag);
    value.split(',').any(|tag| {
        let tag = tag.trim();
        tag == "*" || tag.strip_prefix("W/").unwrap_or(tag) == ours
    })
}

/// Parses a `Range` header value against a representation of `len` bytes.
///
/// Only the `bytes` unit is honoured; anything else — and anything
/// syntactically invalid — is [`RangeOutcome::Ignored`], which the caller
/// answers with the full representation. A valid set with nothing
/// overlapping is [`RangeOutcome::Unsatisfiable`].
pub fn parse_range(value: &str, len: u64) -> RangeOutcome {
    let Some(set) = value.strip_prefix("bytes=") else {
        return RangeOutcome::Ignored;
    };
    let mut spans = Vec::new();
    for spec in set.split(',') {
        let spec = spec.trim();
        let Some((first, last)) = spec.split_once('-') else {
            return RangeOutcome::Ignored;
        };
        let span = if first.is_empty() {
            // A suffix: the last N bytes. `-0` is invalid.
            let n: u64 = match last.trim().parse() {
                Ok(n) if n > 0 => n,
                _ => return RangeOutcome::Ignored,
            };
            let n = n.min(len);
            if n == 0 {
                continue;
            }
            Span { start: len - n, end: len - 1 }
        } else {
            // Syntax before bounds: both positions parse, or the whole
            // header is invalid — even when the first already misses the
            // representation (`bytes=999999-nope` is malformed, not
            // unsatisfiable, and is answered in full rather than with 416).
            let first: u64 = match first.trim().parse() {
                Ok(n) => n,
                _ => return RangeOutcome::Ignored,
            };
            let end: u64 = if last.trim().is_empty() {
                u64::MAX
            } else {
                match last.trim().parse() {
                    Ok(n) => n,
                    _ => return RangeOutcome::Ignored,
                }
            };
            if end < first {
                // A last position before the first is not a range that misses
                // — it is not a range at all, and it poisons the whole header.
                return RangeOutcome::Ignored;
            }
            if first >= len {
                continue;
            }
            Span { start: first, end: end.min(len - 1) }
        };
        spans.push(span);
        if spans.len() > MAX_RANGES {
            return RangeOutcome::Ignored;
        }
    }
    if spans.is_empty() {
        // An empty set (`bytes=`) was never valid; anything else parsed fine
        // and simply overlapped nothing — an empty file included.
        return if set.trim().is_empty() {
            RangeOutcome::Ignored
        } else {
            RangeOutcome::Unsatisfiable
        };
    }
    RangeOutcome::Satisfiable(coalesce(spans))
}

/// Merges overlapping and adjacent spans, ascending.
///
/// Thirty-two duplicate full-file ranges would otherwise read and hold
/// thirty-two copies of the file; merged, they are one span, and no request
/// can hold more than the representation itself however it phrases the ask.
/// Parts arrive ascending rather than in the order asked — each part carries
/// its own `Content-Range`, so no consumer can misread the result.
pub fn coalesce(mut spans: Vec<Span>) -> Vec<Span> {
    spans.sort();
    let mut merged: Vec<Span> = Vec::with_capacity(spans.len());
    for span in spans {
        if let Some(last) = merged.last_mut()
            && span.start <= last.end.saturating_add(1)
        {
            last.end = last.end.max(span.end);
            continue;
        }
        merged.push(span);
    }
    merged
}

/// Decides a file response from the request head and the representation's
/// length, ETag and mtime (seconds, `None` when the filesystem withholds it —
/// date conditionals then never match rather than matching wrongly).
///
/// Order is the RFC's: `If-None-Match` over `If-Modified-Since`, then ranges
/// subject to `If-Range`. An `If-Range` entity-tag never matches our weak
/// ETags (strong comparison is required there), so a ranged request carrying
/// one is answered in full rather than sliced against a validator that cannot
/// promise identity.
pub fn decide(head: &str, len: u64, etag: &str, mtime_secs: Option<u64>) -> Decision {
    if let Some(inm) = header(head, "if-none-match") {
        if etag_matches(inm, etag) {
            return Decision::NotModified;
        }
    } else if let (Some(ms), Some(ims)) = (mtime_secs, header(head, "if-modified-since"))
        && let Some(t) = parse_http_date(ims)
        && ms <= secs(t)
    {
        return Decision::NotModified;
    }
    let Some(range) = header(head, "range") else {
        return Decision::Full;
    };
    let spans = match parse_range(range, len) {
        RangeOutcome::Ignored => return Decision::Full,
        RangeOutcome::Unsatisfiable => return Decision::Unsatisfiable,
        RangeOutcome::Satisfiable(spans) => spans,
    };
    if let Some(if_range) = header(head, "if-range") {
        let fresh = if let Some(t) = parse_http_date(if_range) {
            mtime_secs == Some(secs(t))
        } else {
            false
        };
        if !fresh {
            return Decision::Full;
        }
    }
    match spans.as_slice() {
        [single] => Decision::Single(*single),
        _ => Decision::Multi(spans),
    }
}

/// A multipart boundary unique to this response: process, time and a counter,
/// so one can never be a prefix of another response's while staying readable
/// in a capture.
pub fn boundary() -> String {
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("esdev-{}.{}.{}", std::process::id(), now, n)
}

/// One part's framing, without its bytes: the boundary, the part headers,
/// and the blank line the bytes follow. Shared by the builder, the length
/// computation and the streaming writer, so the three cannot disagree about
/// the shape.
pub fn part_head(boundary: &str, content_type: &str, total_len: u64, span: Span) -> String {
    format!(
        "--{boundary}\r\nContent-Type: {content_type}\r\nContent-Range: bytes {}-{}/{total_len}\r\n\r\n",
        span.start, span.end,
    )
}

/// Assembles a `multipart/byteranges` body from already-read spans: each
/// part carries its own `Content-Type` and `Content-Range`, closed by the
/// terminating boundary.
///
/// Test oracle only: the server streams parts instead of assembling them
/// (see below), and this is what the length computation is checked against.
#[cfg(test)]
pub fn multipart_body(
    boundary: &str,
    content_type: &str,
    total_len: u64,
    parts: &[(Span, Vec<u8>)],
) -> Vec<u8> {
    let mut out = Vec::new();
    for (span, bytes) in parts {
        out.extend_from_slice(part_head(boundary, content_type, total_len, *span).as_bytes());
        out.extend_from_slice(bytes);
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    out
}

/// The exact length a `multipart/byteranges` body will have for these spans,
/// without building it.
///
/// What lets a `HEAD` answer the length its `GET` would send, without
/// reading spans nobody asked to receive.
pub fn multipart_content_length(
    boundary: &str,
    content_type: &str,
    total_len: u64,
    spans: &[Span],
) -> u64 {
    let mut len = 0u64;
    for span in spans {
        len += part_head(boundary, content_type, total_len, *span).len() as u64;
        len += span.len();
        len += 2;
    }
    len += format!("--{boundary}--\r\n").len() as u64;
    len
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head(lines: &[&str]) -> String {
        let mut h = String::from("GET /a.bin HTTP/1.1\r\n");
        for l in lines {
            h.push_str(l);
            h.push_str("\r\n");
        }
        h.push_str("\r\n");
        h
    }

    #[test]
    fn the_etag_is_weak_and_stable() {
        let t = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        assert_eq!(etag(100, t), "W/\"100-1700000000.000000000\"");
        assert_eq!(etag(100, t), etag(100, t));
        // Same size, same second, different instants: still different tags,
        // so a rapid rebuild cannot keep a stale one.
        assert_ne!(
            etag(100, t + Duration::from_nanos(1)),
            etag(100, t + Duration::from_nanos(2))
        );
    }

    #[test]
    fn dates_round_trip() {
        let t = UNIX_EPOCH + Duration::from_secs(1_787_345_377);
        assert_eq!(http_date(t), "Fri, 21 Aug 2026 20:49:37 GMT");
        assert_eq!(parse_http_date(&http_date(t)), Some(t));
        // The obsolete forms still parse when a client sends one.
        assert_eq!(
            parse_http_date("Friday, 21-Aug-26 20:49:37 GMT"),
            Some(t)
        );
        assert_eq!(
            parse_http_date("Fri Aug 21 20:49:37 2026"),
            Some(t)
        );
        assert_eq!(parse_http_date("not a date"), None);
        // Extreme years are rejected, not computed: the arithmetic below
        // overflows an `i64` long before a year with ten digits.
        assert_eq!(parse_http_date("Sun, 06 Nov 9999999999 08:49:37 GMT"), None);
        assert_eq!(parse_http_date("Sun, 06 Nov 10000 08:49:37 GMT"), None);
        assert_eq!(parse_http_date("Sun, 06 Nov 1969 08:49:37 GMT"), None);
        assert!(parse_http_date("Sun, 06 Nov 9999 08:49:37 GMT").is_some());
    }

    #[test]
    fn headers_are_found_case_insensitively() {
        let h = head(&["Range: bytes=0-1", "X-Other: 2"]);
        assert_eq!(header(&h, "range"), Some("bytes=0-1"));
        assert_eq!(header(&h, "RANGE"), Some("bytes=0-1"));
        assert_eq!(header(&h, "missing"), None);
    }

    #[test]
    fn etag_comparison_is_weak() {
        assert!(etag_matches("*", "W/\"1-2\""));
        assert!(etag_matches("W/\"1-2\"", "W/\"1-2\""));
        assert!(etag_matches("\"1-2\"", "W/\"1-2\""));
        assert!(etag_matches("W/\"9-9\", W/\"1-2\"", "W/\"1-2\""));
        assert!(!etag_matches("W/\"3-4\"", "W/\"1-2\""));
    }

    #[test]
    fn ranges_parse() {
        use RangeOutcome::*;
        assert_eq!(
            parse_range("bytes=0-99", 1000),
            Satisfiable(vec![Span { start: 0, end: 99 }])
        );
        assert_eq!(
            parse_range("bytes=950-", 1000),
            Satisfiable(vec![Span { start: 950, end: 999 }])
        );
        assert_eq!(
            parse_range("bytes=-50", 1000),
            Satisfiable(vec![Span { start: 950, end: 999 }])
        );
        // Clamped, not rejected.
        assert_eq!(
            parse_range("bytes=0-99999", 1000),
            Satisfiable(vec![Span { start: 0, end: 999 }])
        );
        assert_eq!(
            parse_range("bytes=-99999", 1000),
            Satisfiable(vec![Span { start: 0, end: 999 }])
        );
        // Several, in the order asked.
        assert_eq!(
            parse_range("bytes=0-1, 10-11", 1000),
            Satisfiable(vec![
                Span { start: 0, end: 1 },
                Span { start: 10, end: 11 },
            ])
        );
        // Valid but past the end.
        assert_eq!(parse_range("bytes=999-1999", 500), Unsatisfiable);
        assert_eq!(parse_range("bytes=500-", 500), Unsatisfiable);
        // Anything else is ignored, answered in full.
        assert_eq!(parse_range("items=0-1", 500), Ignored);
        assert_eq!(parse_range("bytes=", 500), Ignored);
        assert_eq!(parse_range("bytes=abc", 500), Ignored);
        assert_eq!(parse_range("bytes=-0", 500), Ignored);
        assert_eq!(parse_range("bytes=5-3", 500), Ignored);
        // Malformed stays malformed even past the end: the positions are
        // validated before the bounds are consulted.
        assert_eq!(parse_range("bytes=999999-nope", 500), Ignored);
        assert_eq!(parse_range("bytes=999-nope", 500), Ignored);
        assert_eq!(parse_range("bytes=nope-999", 500), Ignored);
        // An empty file satisfies nothing.
        assert_eq!(parse_range("bytes=0-", 0), Unsatisfiable);
    }

    #[test]
    fn too_many_ranges_are_ignored() {
        let many = (0..MAX_RANGES + 1)
            .map(|i| format!("{}-{}", i * 2, i * 2))
            .collect::<Vec<_>>()
            .join(", ");
        assert_eq!(
            parse_range(&format!("bytes={many}"), 100_000),
            RangeOutcome::Ignored
        );
    }

    #[test]
    fn decisions_follow_the_rfc_order() {
        let et = "W/\"10-100\"";
        // No conditionals: the file.
        assert_eq!(decide(&head(&[]), 10, et, Some(100)), Decision::Full);
        // A matching validator short-circuits everything, ranges included.
        assert_eq!(
            decide(&head(&["If-None-Match: W/\"10-100\"", "Range: bytes=0-1"]), 10, et, Some(100)),
            Decision::NotModified
        );
        assert_eq!(
            decide(&head(&["If-None-Match: *"]), 10, et, Some(100)),
            Decision::NotModified
        );
        // A miss falls through to the ranges.
        assert_eq!(
            decide(&head(&["If-None-Match: W/\"1-1\"", "Range: bytes=0-1"]), 10, et, Some(100)),
            Decision::Single(Span { start: 0, end: 1 })
        );
        // Without an mtime, date conditionals never match rather than
        // matching wrongly.
        assert_eq!(
            decide(&head(&["If-Modified-Since: Sun, 06 Nov 1994 08:49:37 GMT"]), 10, et, None),
            Decision::Full
        );
        assert_eq!(
            decide(&head(&["If-Modified-Since: Sun, 06 Nov 1994 08:49:37 GMT"]), 10, et, Some(100)),
            Decision::NotModified
        );
        assert_eq!(
            decide(
                &head(&["If-None-Match: W/\"1-1\"", "If-Modified-Since: Sun, 06 Nov 1994 08:49:37 GMT"]),
                10, et, Some(100)
            ),
            Decision::Full
        );
        // Ranges, single and multiple.
        assert_eq!(
            decide(&head(&["Range: bytes=0-1"]), 10, et, Some(100)),
            Decision::Single(Span { start: 0, end: 1 })
        );
        match decide(&head(&["Range: bytes=0-1, 4-5"]), 10, et, Some(100)) {
            Decision::Multi(spans) => assert_eq!(spans.len(), 2),
            d => panic!("expected multi, got {d:?}"),
        }
        assert_eq!(
            decide(&head(&["Range: bytes=99-100"]), 10, et, Some(100)),
            Decision::Unsatisfiable
        );
        // A stale If-Range restores the full file; a fresh date keeps the slice.
        assert_eq!(
            decide(&head(&["Range: bytes=0-1", "If-Range: W/\"10-100\""]), 10, et, Some(100)),
            Decision::Full
        );
        assert_eq!(
            decide(&head(&["Range: bytes=0-1", "If-Range: Thu, 01 Jan 1970 00:01:40 GMT"]), 10, et, Some(100)),
            Decision::Single(Span { start: 0, end: 1 })
        );
    }

    #[test]
    fn multipart_bodies_carry_each_part() {
        let parts = [
            (Span { start: 0, end: 1 }, b"ab".to_vec()),
            (Span { start: 4, end: 5 }, b"ef".to_vec()),
        ];
        let body = multipart_body("b", "text/plain", 10, &parts);
        let text = String::from_utf8(body).unwrap();
        assert!(text.starts_with("--b\r\n"));
        assert!(text.contains("Content-Range: bytes 0-1/10\r\n\r\nab\r\n"));
        assert!(text.contains("Content-Range: bytes 4-5/10\r\n\r\nef\r\n"));
        assert!(text.ends_with("--b--\r\n"));
    }

    #[test]
    fn overlapping_ranges_coalesce() {
        let s = |a, b| Span { start: a, end: b };
        // Duplicates collapse: thirty-two full-file asks become one span.
        assert_eq!(
            parse_range("bytes=0-1023, 0-1023", 1024),
            RangeOutcome::Satisfiable(vec![s(0, 1023)])
        );
        // Overlap and adjacency merge; order normalises ascending.
        assert_eq!(
            parse_range("bytes=200-300, 0-100, 50-250", 1000),
            RangeOutcome::Satisfiable(vec![s(0, 300)])
        );
        assert_eq!(
            parse_range("bytes=0-99, 100-199", 1000),
            RangeOutcome::Satisfiable(vec![s(0, 199)])
        );
        // Disjoint spans stay disjoint.
        assert_eq!(
            parse_range("bytes=0-1, 10-11", 1000),
            RangeOutcome::Satisfiable(vec![s(0, 1), s(10, 11)])
        );
    }

    #[test]
    fn the_head_length_is_the_get_length() {
        // Empty, single, multiple, and large offsets: the arithmetic and the
        // builder agree everywhere, so a HEAD never under-reports.
        for spans in [
            vec![],
            vec![Span { start: 0, end: 1 }],
            vec![Span { start: 0, end: 1 }, Span { start: 4, end: 5 }],
            vec![Span { start: 9_999_950, end: 10_000_000 }],
        ] {
            let parts: Vec<(Span, Vec<u8>)> = spans
                .iter()
                .map(|s| (*s, vec![0u8; s.len() as usize]))
                .collect();
            let body = multipart_body("boundary-1", "application/octet-stream", 10_000_001, &parts);
            assert_eq!(
                multipart_content_length("boundary-1", "application/octet-stream", 10_000_001, &spans),
                body.len() as u64,
            );
        }
    }
}
