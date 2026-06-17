//! Host ops backing the URL family (SPEC §2.4), powered by the WHATWG-ish
//! `url` crate (DECISIONS.md D18).
//!
//! The ops are pure computation (no capability): parse a URL and return its
//! canonical serialization plus component *offsets*, or apply one component
//! setter and return the same. The JS `URL`/`URLSearchParams` wrappers
//! (prelude) drive these.
//!
//! Wire shape: `[href, o0..o14]` — a JS array of the canonical href string and
//! fifteen [`url::Position`] offsets (see [`POSITIONS`]) as numbers. Every URL
//! getter in the prelude is then a lazy `href.slice(...)`; nothing is built for
//! components the script never reads. This replaced an 11-field JSON round-trip
//! (serialize in Rust, `JSON.parse` in JS), which itself had beaten per-property
//! V8 object building — slicing beats both (see bench/README.md). Offsets are
//! UTF-16 code-unit indices so JS `slice` can use them directly; canonical
//! WHATWG hrefs are ASCII in practice (non-ASCII is percent-encoded/punycoded),
//! making the byte→UTF-16 remap a never-taken safety path.
//!
//! A `Value::Null` result signals a parse/setter failure the wrapper turns into
//! a `TypeError`. `origin` is not sliceable (opaque origins serialize as
//! "null"), so it stays a separate op the prelude calls lazily.

use es_runtime_engine::{Engine, OpDecl, Value};
use url::{Position, Url};

use crate::Result;

/// The component boundaries shipped to JS, in serialization order. The prelude
/// indexes this list positionally (url.js keeps the mirror table) — append-only.
const POSITIONS: [Position; 15] = [
    Position::AfterScheme,    // 0: protocol = href[0 .. o0+1] (includes ":")
    Position::BeforeUsername, // 1
    Position::AfterUsername,  // 2: username = href[o1 .. o2]
    Position::BeforePassword, // 3
    Position::AfterPassword,  // 4: password = href[o3 .. o4]
    Position::BeforeHost,     // 5
    Position::AfterHost,      // 6: hostname = href[o5 .. o6]
    Position::BeforePort,     // 7
    Position::AfterPort,      // 8: port = href[o7 .. o8]; host = href[o5 .. o8]
    Position::BeforePath,     // 9
    Position::AfterPath,      // 10: pathname = href[o9 .. o10]
    Position::BeforeQuery,    // 11: (after the "?")
    Position::AfterQuery,     // 12: search = "?" + href[o11 .. o12] if non-empty
    Position::BeforeFragment, // 13: (after the "#")
    Position::AfterFragment,  // 14: hash = "#" + href[o13 .. o14] if non-empty
];

/// Registers `url_parse`, `url_set`, and `url_origin`.
pub(crate) fn install(engine: &mut dyn Engine) -> Result<()> {
    engine.register_op(OpDecl::sync("url_parse", |args| {
        let input = args.first().and_then(Value::as_str).unwrap_or("");
        let base = args.get(1).and_then(Value::as_str);
        Ok(match parse(input, base) {
            Some(url) => components_value(&url),
            None => Value::Null,
        })
    }))?;

    engine.register_op(OpDecl::sync("url_set", |args| {
        let href = args.first().and_then(Value::as_str).unwrap_or("");
        let component = args.get(1).and_then(Value::as_str).unwrap_or("");
        let value = args.get(2).and_then(Value::as_str).unwrap_or("");
        Ok(match set_component(href, component, value) {
            Some(url) => components_value(&url),
            None => Value::Null,
        })
    }))?;

    // Lazy `.origin` (rarely read; needs origin logic, not slicing — opaque
    // origins serialize as "null"). `href` is canonical, so re-parsing is safe.
    engine.register_op(OpDecl::sync("url_origin", |args| {
        let href = args.first().and_then(Value::as_str).unwrap_or("");
        Ok(match Url::parse(href) {
            Ok(url) => Value::String(url.origin().ascii_serialization()),
            Err(_) => Value::Null,
        })
    }))?;
    Ok(())
}

/// Parses `input`, optionally against `base` (the `new URL(input, base)` form).
fn parse(input: &str, base: Option<&str>) -> Option<Url> {
    match base {
        Some(base) => {
            let base = Url::parse(base).ok()?;
            Url::options().base_url(Some(&base)).parse(input).ok()
        }
        None => Url::parse(input).ok(),
    }
}

/// Applies one component setter to `href` and returns the resulting URL.
///
/// Per WHATWG, an invalid component setter is a silent no-op (the URL is
/// returned unchanged); only an invalid `href` assignment fails (→ `None`, which
/// the wrapper turns into a `TypeError`).
fn set_component(href: &str, component: &str, value: &str) -> Option<Url> {
    if component == "href" {
        return Url::parse(value).ok();
    }

    let mut url = Url::parse(href).ok()?;
    match component {
        "protocol" => {
            let _ = url.set_scheme(value.trim_end_matches(':'));
        }
        "username" => {
            let _ = url.set_username(value);
        }
        "password" => {
            let _ = url.set_password((!value.is_empty()).then_some(value));
        }
        "host" => {
            let (host_str, port_str) = if value.starts_with('[') {
                if let Some(closing) = value.find(']') {
                    if let Some(colon) = value[closing..].find(':') {
                        (
                            &value[..closing + colon],
                            Some(&value[closing + colon + 1..]),
                        )
                    } else {
                        (value, None)
                    }
                } else {
                    (value, None)
                }
            } else if let Some(colon) = value.rfind(':') {
                (&value[..colon], Some(&value[colon + 1..]))
            } else {
                (value, None)
            };

            let port_opt = match port_str {
                Some("") => Some(None),
                Some(p) => match p.parse::<u16>() {
                    Ok(num) => Some(Some(num)),
                    Err(_) => None, // Invalid port
                },
                None => Some(None), // Valid, but no port specified
            };

            // Only apply if BOTH host is valid AND port is valid
            if let (Ok(_), Some(port)) = (url::Host::parse(host_str), port_opt) {
                let _ = url.set_host(Some(host_str));
                if port_str.is_some() {
                    let _ = url.set_port(port);
                }
            }
        }
        "hostname" => {
            let has_colon = if value.starts_with('[') {
                if let Some(closing) = value.find(']') {
                    value[closing..].contains(':')
                } else {
                    value.contains(':')
                }
            } else {
                value.contains(':')
            };

            if !has_colon && url::Host::parse(value).is_ok() {
                let _ = url.set_host(Some(value));
            }
        }
        "port" => {
            let port = if value.is_empty() {
                None
            } else {
                match value.parse::<u16>() {
                    Ok(p) => Some(p),
                    Err(_) => return Some(url),
                }
            };
            let _ = url.set_port(port);
        }
        "pathname" => url.set_path(value),
        "search" => {
            let query = value.strip_prefix('?').unwrap_or(value);
            url.set_query((!query.is_empty()).then_some(query));
        }
        "hash" => {
            let fragment = value.strip_prefix('#').unwrap_or(value);
            url.set_fragment((!fragment.is_empty()).then_some(fragment));
        }
        _ => {}
    }
    Some(url)
}

/// Builds the `[href, o0..o14]` wire value (see the module docs).
fn components_value(url: &Url) -> Value {
    let href = url.as_str();
    let mut offsets = [0u32; 15];
    for (slot, position) in offsets.iter_mut().zip(POSITIONS) {
        *slot = url[..position].len() as u32;
    }
    // JS slices by UTF-16 code unit; the offsets above are bytes. They agree
    // exactly when the href is ASCII — always, for spec-canonical hrefs.
    if !href.is_ascii() {
        remap_to_utf16(href, &mut offsets);
    }

    let mut items = Vec::with_capacity(1 + offsets.len());
    items.push(Value::String(href.to_owned()));
    items.extend(offsets.iter().map(|&o| Value::Number(f64::from(o))));
    Value::Array(items)
}

/// Rewrites ascending byte `offsets` into `s` as UTF-16 code-unit indices, in
/// one pass. Component boundaries always fall on char boundaries.
fn remap_to_utf16(s: &str, offsets: &mut [u32; 15]) {
    let mut remapped = [0u32; 15];
    let mut next = 0;
    let (mut byte_idx, mut utf16_idx) = (0u32, 0u32);
    for c in s.chars() {
        while next < offsets.len() && offsets[next] == byte_idx {
            remapped[next] = utf16_idx;
            next += 1;
        }
        byte_idx += c.len_utf8() as u32;
        utf16_idx += c.len_utf16() as u32;
    }
    while next < offsets.len() {
        remapped[next] = utf16_idx;
        next += 1;
    }
    *offsets = remapped;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mirrors the prelude's slicing (url.js) over the wire shape, so the
    /// Rust-side contract is pinned without an engine.
    fn slices(input: &str) -> Vec<String> {
        let url = Url::parse(input).expect("parse");
        let Value::Array(items) = components_value(&url) else {
            panic!("expected array");
        };
        let href = items[0].as_str().expect("href").to_string();
        let o: Vec<usize> = items[1..]
            .iter()
            .map(|v| v.as_number().expect("offset") as usize)
            .collect();
        let slice = |a: usize, b: usize| href[a..b].to_string();
        vec![
            href.clone(),
            slice(0, o[0] + 1), // protocol
            slice(o[1], o[2]),  // username
            slice(o[3], o[4]),  // password
            slice(o[5], o[8]),  // host
            slice(o[5], o[6]),  // hostname
            slice(o[7], o[8]),  // port
            slice(o[9], o[10]), // pathname
            if o[11] < o[12] {
                format!("?{}", slice(o[11], o[12]))
            } else {
                String::new()
            },
            if o[13] < o[14] {
                format!("#{}", slice(o[13], o[14]))
            } else {
                String::new()
            },
        ]
    }

    #[test]
    fn full_url_slices_to_whatwg_components() {
        let s = slices("https://user:pw@example.com:8080/a/b?x=1&y=2#frag");
        assert_eq!(
            s,
            [
                "https://user:pw@example.com:8080/a/b?x=1&y=2#frag",
                "https:",
                "user",
                "pw",
                "example.com:8080",
                "example.com",
                "8080",
                "/a/b",
                "?x=1&y=2",
                "#frag",
            ]
        );
    }

    #[test]
    fn sparse_url_slices_to_empty_components() {
        let s = slices("https://example.com/");
        assert_eq!(
            s,
            [
                "https://example.com/",
                "https:",
                "",
                "",
                "example.com",
                "example.com",
                "",
                "/",
                "",
                ""
            ]
        );
    }

    #[test]
    fn no_authority_url_slices() {
        let s = slices("mailto:joe@example.com");
        assert_eq!(
            s,
            [
                "mailto:joe@example.com",
                "mailto:",
                "",
                "",
                "",
                "",
                "",
                "joe@example.com",
                "",
                ""
            ]
        );
    }

    #[test]
    fn empty_query_and_fragment_are_empty_strings() {
        // WHATWG: a present-but-empty query/fragment reads back as "".
        let s = slices("https://example.com/p?#");
        assert_eq!(s[8], "");
        assert_eq!(s[9], "");
    }

    #[test]
    fn remap_rewrites_byte_offsets_as_utf16_indices() {
        // rust-url percent-encodes non-ASCII everywhere today, so this path is
        // a safety net rather than a live one — pin the pure function directly.
        // "a😀b": 'a'=1 byte/1 unit, '😀'=4 bytes/2 units, 'b'=1 byte/1 unit.
        let s = "a😀b";
        let mut offsets = [0u32; 15];
        // Byte offsets: start, after 'a', after '😀', after 'b', rest at end.
        offsets[1] = 1;
        offsets[2] = 5;
        for slot in offsets.iter_mut().skip(3) {
            *slot = 6;
        }
        remap_to_utf16(s, &mut offsets);
        assert_eq!(offsets[0], 0);
        assert_eq!(offsets[1], 1); // after 'a'
        assert_eq!(offsets[2], 3); // after the surrogate pair
        assert!(offsets[3..].iter().all(|&o| o == 4)); // end of string
    }
}
