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

use std::cell::RefCell;
use std::rc::Rc;

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

/// Indices into [`POSITIONS`] for the password span, which needs special
/// handling — see [`components_value`].
const PASSWORD_START: usize = 3;
const PASSWORD_END: usize = 4;

/// A small cache of already-parsed URLs, keyed by their own serialization.
///
/// Every component setter used to re-parse the whole URL from its `href`, because
/// the JS object holds nothing but that string — a deliberately stateless design
/// (the href is canonical, so re-parsing is always safe). It is also the single
/// biggest cost in a setter: 0.44µs of parse against 0.56µs of actually applying
/// the change, so a loop assigning `u.hostname` paid for a parse of a URL that
/// had just been produced one call earlier.
///
/// `href -> Url` is a pure function, which is what makes a cache safe here: a hit
/// cannot produce a different answer from a miss, only a faster one. So this
/// stores no identity and needs no handles — nothing on the JS side changes, no
/// object owns a host-side resource, and there is nothing to free. A handle
/// scheme would have bought the same speed while making every `new URL()`
/// allocate host state reclaimed only when a `FinalizationRegistry` callback
/// happened to run; a cache that cannot outgrow `CAP` has no such failure mode.
///
/// Entries are *taken* on use (setters need ownership to mutate) and the result
/// is put back under its new serialization, so the next setter on the same
/// object hits. Move-to-front keeps the URL being mutated at index 0, which is
/// the access pattern this exists for; anything colder simply re-parses.
#[derive(Default)]
struct ParsedUrls {
    /// Keyed by nothing: a `Url` already owns its serialization, so `as_str()`
    /// *is* the key. Storing a separate `String` would allocate a copy of it on
    /// every insert, which on the parse path cost more than the cache saved.
    entries: Vec<Url>,
}

impl ParsedUrls {
    /// Deliberately small. The pattern worth catching is a handful of URLs being
    /// read or mutated in a loop; a large cache would only add scan cost and hold
    /// memory for hrefs nobody returns to.
    const CAP: usize = 8;

    /// Removes and returns the parsed form of `href`, if it is cached.
    fn take(&mut self, href: &str) -> Option<Url> {
        let i = self.entries.iter().position(|u| u.as_str() == href)?;
        Some(self.entries.remove(i))
    }

    /// An href long enough that caching it would retain more than it saves. A
    /// parse of something this size is dominated by its length anyway, and eight
    /// entries of it would be held for the life of the runtime after the guest
    /// had dropped every reference.
    const MAX_HREF: usize = 8 * 1024;

    /// Caches `url`, evicting the coldest entry when full.
    fn put(&mut self, url: Url) {
        if url.as_str().len() > Self::MAX_HREF {
            return;
        }
        self.entries.insert(0, url);
        self.entries.truncate(Self::CAP);
    }
}

/// Registers `url_parse`, `url_set`, and `url_origin`.
pub(crate) fn install(engine: &mut dyn Engine) -> Result<()> {
    // Shared by the two ops that are handed an href they did not produce.
    // `url_parse` deliberately does not seed it: measured, the insert cost more
    // on that path than the one parse it would have saved for the object's first
    // setter, which is the only call that could have used it.
    let cache: Rc<RefCell<ParsedUrls>> = Rc::new(RefCell::new(ParsedUrls::default()));

    engine.register_op(OpDecl::sync("url_parse", |args| {
        let input = args.first().and_then(Value::as_str).unwrap_or("");
        let base = args.get(1).and_then(Value::as_str);
        Ok(match parse(input, base) {
            Some(url) => components_value(&url),
            None => Value::Null,
        })
    }))?;

    let c = cache.clone();
    engine.register_op(OpDecl::sync("url_set", move |args| {
        let href = args.first().and_then(Value::as_str).unwrap_or("");
        let component = args.get(1).and_then(Value::as_str).unwrap_or("");
        let value = args.get(2).and_then(Value::as_str).unwrap_or("");

        // `href` replaces the URL rather than modifying it, so the cached parse
        // of the *old* href is of no use — but the new one is worth keeping.
        if component == "href" {
            return Ok(match Url::parse(value) {
                Ok(url) => {
                    let out = components_value(&url);
                    c.borrow_mut().put(url);
                    out
                }
                Err(_) => Value::Null,
            });
        }

        let cached = c.borrow_mut().take(href);
        let url = match cached {
            Some(url) => url,
            None => match Url::parse(href) {
                Ok(url) => url,
                Err(_) => return Ok(Value::Null),
            },
        };
        Ok(match apply_component(url, component, value) {
            Some(url) => {
                let out = components_value(&url);
                c.borrow_mut().put(url);
                out
            }
            None => Value::Null,
        })
    }))?;

    // Lazy `.origin` (rarely read; needs origin logic, not slicing — opaque
    // origins serialize as "null"). `href` is canonical, so re-parsing is safe.
    let c = cache;
    engine.register_op(OpDecl::sync("url_origin", move |args| {
        let href = args.first().and_then(Value::as_str).unwrap_or("");
        let cached = c.borrow_mut().take(href);
        let url = match cached {
            Some(url) => url,
            None => match Url::parse(href) {
                Ok(url) => url,
                Err(_) => return Ok(Value::Null),
            },
        };
        let origin = url.origin().ascii_serialization();
        // Read-only: put it straight back, since `.origin` does not change the URL.
        c.borrow_mut().put(url);
        Ok(Value::String(origin))
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

/// Applies one component setter to an already-parsed URL.
///
/// Per WHATWG, an invalid component setter is a silent no-op (the URL comes back
/// unchanged). The `href` component is not handled here: it replaces the URL
/// outright rather than modifying one, so the caller parses the new value.
///
/// Takes a parsed `Url` rather than an href so a caller holding one need not
/// re-parse it from its own serialization — which is the whole point of
/// [`ParsedUrls`].
fn apply_component(mut url: Url, component: &str, value: &str) -> Option<Url> {
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

            // Per WHATWG: a host with no `:port`, or a bare trailing `:` (empty
            // port), leaves the existing port untouched; an explicit port must be
            // a valid u16 to set it, and an invalid one aborts the whole setter.
            // `valid` gates the assignment; `set_port_to` is the port to apply.
            let (valid, set_port_to) = match port_str {
                None | Some("") => (true, None),
                Some(p) => match p.parse::<u16>() {
                    Ok(num) => (true, Some(num)),
                    Err(_) => (false, None),
                },
            };

            if valid && url::Host::parse(host_str).is_ok() {
                let _ = url.set_host(Some(host_str));
                if let Some(port) = set_port_to {
                    let _ = url.set_port(Some(port));
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
    // `url` mis-slices the password positions for a URL that has a username but
    // no password (`https://foo@example.com`). Both `BeforePassword` and
    // `AfterPassword` fall into a branch that assumes there are no credentials
    // at all: its `debug_assert!(username_end == host_start)` fires in debug
    // builds, and in release the two positions straddle the "@" separator, so
    // `.password` reads back as "@" instead of "". Derive that (empty) span from
    // the end of the username instead, and never index those two positions.
    let no_password = url.password().is_none();
    let username_end = url[..Position::AfterUsername].len() as u32;
    for (i, (slot, position)) in offsets.iter_mut().zip(POSITIONS).enumerate() {
        *slot = if no_password && (i == PASSWORD_START || i == PASSWORD_END) {
            username_end
        } else {
            url[..position].len() as u32
        };
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

/// Fuzz entry: parse and read back every component (see [`crate::fuzz`]).
#[cfg(feature = "fuzzing")]
pub(crate) fn fuzz_parse(input: &str, base: Option<&str>) {
    if let Some(url) = parse(input, base) {
        let _ = components_value(&url);
    }
}

/// Fuzz entry: apply one component setter (see [`crate::fuzz`]).
#[cfg(feature = "fuzzing")]
pub(crate) fn fuzz_set(href: &str, component: &str, value: &str) {
    // Mirrors what the `url_set` op does, including its `href` branch, so the
    // fuzzer covers the path production runs rather than a parallel one.
    if component == "href" {
        if let Ok(url) = Url::parse(value) {
            let _ = components_value(&url);
        }
        return;
    }
    let Ok(url) = Url::parse(href) else { return };
    if let Some(url) = apply_component(url, component, value) {
        let _ = components_value(&url);
    }
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
    fn username_without_password_slices_to_an_empty_password() {
        // `url`'s BeforePassword/AfterPassword straddle the "@" when there is a
        // username and no password: debug builds hit its debug_assert, release
        // builds read the password back as "@". Both must give "".
        let s = slices("https://foo@example.com/p");
        assert_eq!(s[2], "foo", "username");
        assert_eq!(s[3], "", "password");
        assert_eq!(s[5], "example.com", "hostname");
        assert_eq!(s[7], "/p", "pathname");
    }

    #[test]
    fn credential_shapes_all_slice_correctly() {
        // Username only, password only, both, and neither.
        let both = slices("https://u:p@example.com/");
        assert_eq!((both[2].as_str(), both[3].as_str()), ("u", "p"));

        let user_only = slices("https://u@example.com/");
        assert_eq!((user_only[2].as_str(), user_only[3].as_str()), ("u", ""));

        let password_only = slices("https://:p@example.com/");
        assert_eq!(
            (password_only[2].as_str(), password_only[3].as_str()),
            ("", "p")
        );

        let neither = slices("https://example.com/");
        assert_eq!((neither[2].as_str(), neither[3].as_str()), ("", ""));
    }

    #[test]
    fn username_without_password_slices_with_non_ascii_host() {
        // The UTF-16 remap runs over the same offsets; the empty password span
        // must survive it.
        let s = slices("https://foo@ünïcode.example/p");
        assert_eq!(s[2], "foo", "username");
        assert_eq!(s[3], "", "password");
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
