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

use base64::{
    Engine as _, engine::general_purpose::STANDARD, engine::general_purpose::STANDARD_NO_PAD,
};

/// `btoa`: base64 of a Latin-1 string. `None` if any code point exceeds U+00FF.
fn encode(s: &str) -> Option<String> {
    // Fast path: ASCII means UTF-8 matches Latin-1 byte for byte.
    if s.is_ascii() {
        return Some(STANDARD.encode(s.as_bytes()));
    }

    let mut bytes = Vec::with_capacity(s.len());
    for c in s.chars() {
        let cp = c as u32;
        if cp > 0xFF {
            return None;
        }
        bytes.push(cp as u8);
    }
    Some(STANDARD.encode(&bytes))
}

/// `atob`: decode to a string of U+0000–U+00FF code points. `None` on invalid
/// input.
fn decode(s: &str) -> Option<String> {
    // Strip ASCII whitespace, which the spec ignores.
    let mut cleaned = Vec::with_capacity(s.len());
    for &b in s.as_bytes() {
        if !matches!(b, b'\t' | b'\n' | b'\x0c' | b'\r' | b' ') {
            cleaned.push(b);
        }
    }

    if cleaned.len() % 4 == 1 {
        return None;
    }
    let mut end = cleaned.len();
    while end > 0 && cleaned[end - 1] == b'=' {
        end -= 1;
    }

    let decoded = STANDARD_NO_PAD.decode(&cleaned[..end]).ok()?;

    // Fast path: if the output is valid UTF-8 (e.g. pure ASCII), this is zero-copy.
    match String::from_utf8(decoded) {
        Ok(s) => Some(s),
        Err(e) => {
            // Slow path: convert Latin-1 (u8 > 127) to UTF-8
            let decoded = e.into_bytes();
            let mut out = String::with_capacity(decoded.len() + decoded.len() / 4);
            for b in decoded {
                out.push(b as char);
            }
            Some(out)
        }
    }
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
