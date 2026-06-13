//! Host ops backing `atob`/`btoa` (SPEC §2.3) — base64 over Latin-1 strings.
//!
//! Pure computation (no capability). The previous pure-JS implementation built
//! the result with per-character string concatenation, ~36× slower than the
//! native paths in Node/Bun/Deno on the bench's base64 workload; one op call
//! per `atob`/`btoa` with the loop in Rust closes most of that.
//!
//! Semantics mirror the JS implementation they replace (and WHATWG `atob`'s
//! forgiving-base64, with one recorded looseness: *all* trailing `=` are
//! stripped, not just one or two). A `Value::Null` result signals invalid
//! input, which the prelude wrapper turns into an `InvalidCharacterError`
//! `DOMException`.

use es_runtime_engine::{Engine, OpDecl, Value};

use crate::Result;

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// `ALPHABET` inverted: sextet value per byte, or -1 for non-alphabet bytes.
const LOOKUP: [i8; 256] = {
    let mut table = [-1i8; 256];
    let mut i = 0;
    while i < ALPHABET.len() {
        table[ALPHABET[i] as usize] = i as i8;
        i += 1;
    }
    table
};

/// Registers `base64_encode` / `base64_decode`.
pub(crate) fn install(engine: &mut dyn Engine) -> Result<()> {
    engine.register_op(OpDecl::sync("base64_encode", |args| {
        let s = args.first().and_then(Value::as_str).unwrap_or("");
        Ok(match encode(s) {
            Some(out) => Value::String(out),
            None => Value::Null,
        })
    }))?;

    engine.register_op(OpDecl::sync("base64_decode", |args| {
        let s = args.first().and_then(Value::as_str).unwrap_or("");
        Ok(match decode(s) {
            Some(out) => Value::String(out),
            None => Value::Null,
        })
    }))?;
    Ok(())
}

/// `btoa`: base64 of a Latin-1 string. `None` if any code point exceeds U+00FF.
fn encode(s: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(s.len());
    for c in s.chars() {
        let cp = c as u32;
        if cp > 0xFF {
            return None;
        }
        bytes.push(cp as u8);
    }

    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b1 = u32::from(*chunk.get(1).unwrap_or(&0));
        let b2 = u32::from(*chunk.get(2).unwrap_or(&0));
        let triple = (u32::from(chunk[0]) << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(triple >> 18) as usize & 0x3f] as char);
        out.push(ALPHABET[(triple >> 12) as usize & 0x3f] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(triple >> 6) as usize & 0x3f] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[triple as usize & 0x3f] as char
        } else {
            '='
        });
    }
    Some(out)
}

/// `atob`: decode to a string of U+0000–U+00FF code points. `None` on invalid
/// input.
///
/// Whitespace stripping and the `len % 4 == 1` check operate on *bytes* where
/// the spec speaks of code units; they only disagree for non-ASCII input, which
/// always fails (here as an invalid character, in the spec sometimes as a
/// length error) — same exception either way, so the difference is unobservable.
fn decode(s: &str) -> Option<String> {
    // Strip ASCII whitespace, which the spec ignores.
    let cleaned: Vec<u8> = s
        .bytes()
        .filter(|b| !matches!(b, b'\t' | b'\n' | b'\x0c' | b'\r' | b' '))
        .collect();
    if cleaned.len() % 4 == 1 {
        return None;
    }
    let mut end = cleaned.len();
    while end > 0 && cleaned[end - 1] == b'=' {
        end -= 1;
    }

    let mut out = String::with_capacity(end * 3 / 4 + 1);
    let (mut acc, mut bits) = (0u32, 0u32);
    for &b in &cleaned[..end] {
        let v = LOOKUP[b as usize];
        if v < 0 {
            return None;
        }
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xff) as u8 as char);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_with_padding() {
        assert_eq!(encode("").as_deref(), Some(""));
        assert_eq!(encode("f").as_deref(), Some("Zg=="));
        assert_eq!(encode("fo").as_deref(), Some("Zm8="));
        assert_eq!(encode("foo").as_deref(), Some("Zm9v"));
        assert_eq!(encode("foobar").as_deref(), Some("Zm9vYmFy"));
    }

    #[test]
    fn encodes_full_latin1_range_and_rejects_beyond() {
        let latin1: String = (0u8..=255).map(char::from).collect();
        let encoded = encode(&latin1).expect("latin1 encodes");
        assert_eq!(decode(&encoded).as_deref(), Some(latin1.as_str()));
        assert_eq!(encode("héllo"), Some("aOlsbG8=".into())); // é = U+00E9, in range
        assert_eq!(encode("✓"), None);
        assert_eq!(encode("😀"), None);
    }

    #[test]
    fn decodes_ignoring_whitespace() {
        assert_eq!(decode("Zm9v").as_deref(), Some("foo"));
        assert_eq!(decode(" Zm 9\tv\n").as_deref(), Some("foo"));
        assert_eq!(decode("Zg==").as_deref(), Some("f"));
        assert_eq!(decode("Zg").as_deref(), Some("f")); // forgiving: padding optional
    }

    #[test]
    fn decode_rejects_invalid() {
        assert_eq!(decode("Zm9vv"), None); // len % 4 == 1
        assert_eq!(decode("Zm.v"), None); // non-alphabet char
        assert_eq!(decode("Zm9é"), None); // non-ASCII
        assert_eq!(decode("=Zm9v"), None); // interior padding
    }

    #[test]
    fn decode_strips_all_trailing_padding_like_the_js_it_replaces() {
        // Looser than WHATWG (which allows at most two "="); recorded in the
        // module docs. "AAA" decodes to two bytes.
        assert_eq!(decode("AAA=====").as_deref(), Some("\0\0"));
    }
}
