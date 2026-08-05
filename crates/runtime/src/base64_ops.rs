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
    const fn is_ws(b: u8) -> bool {
        matches!(b, b'\t' | b'\n' | b'\x0c' | b'\r' | b' ')
    }

    // Strip ASCII whitespace, which the spec ignores — but only when there is
    // some. Copying every byte through a `Vec::push` cost more than the base64
    // decode it was preparing for (2.2µs against Node's 0.36µs on the bench's
    // 1 KiB row); a base64 string with whitespace in it is the rare shape, and
    // the common one now borrows the input untouched.
    let borrowed: Vec<u8>;
    let cleaned: &[u8] = if s.as_bytes().iter().copied().any(is_ws) {
        borrowed = s.as_bytes().iter().copied().filter(|&b| !is_ws(b)).collect();
        &borrowed
    } else {
        s.as_bytes()
    };

    if cleaned.len() % 4 == 1 {
        return None;
    }
    let mut end = cleaned.len();
    while end > 0 && cleaned[end - 1] == b'=' {
        end -= 1;
    }

    let decoded = STANDARD_NO_PAD.decode(&cleaned[..end]).ok()?;

    // Fast path: if the output is valid UTF-8 (e.g. pure ASCII), this is
    // zero-copy, and `v8::String::new` then recognizes the ASCII and builds a
    // one-byte string directly. Returning the raw bytes for V8 to adopt as
    // Latin-1 was tried and measured no faster, because that ASCII fast path
    // already exists — so this keeps the simpler type.
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

/// Fuzz entry: `atob`'s decoder (see [`crate::fuzz`]).
#[cfg(feature = "fuzzing")]
pub(crate) fn fuzz_decode(input: &str) {
    let _ = decode(input);
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
        assert_eq!(encode("h\u{e9}llo"), Some("aOlsbG8=".into())); // \u{e9} = U+00E9, in range
        assert_eq!(encode("\u{2713}"), None);
        assert_eq!(encode("\u{1f600}"), None);
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
        assert_eq!(decode("Zm9\u{e9}"), None); // non-ASCII
        assert_eq!(decode("=Zm9v"), None); // interior padding
    }

    /// Encoder/decoder throughput on its own, away from the op boundary — the
    /// number that decides whether the `base64` bench row is limited by the
    /// crate or by the JS↔Rust crossing around it. Reported, not asserted.
    #[test]
    #[ignore = "measurement: cargo test -p es-runtime --lib --release -- --ignored --nocapture base64_throughput"]
    #[allow(clippy::print_stdout)]
    fn base64_throughput() {
        for size in [1024usize, 65536] {
            let input: String = "a".repeat(size);
            let encoded = encode(&input).expect("ascii encodes");
            let n = (16 * 1024 * 1024 / size).max(16);

            let start = std::time::Instant::now();
            for _ in 0..n {
                std::hint::black_box(encode(std::hint::black_box(&input)));
            }
            let enc_gbs = (size * n) as f64 / start.elapsed().as_secs_f64() / 1e9;

            let start = std::time::Instant::now();
            for _ in 0..n {
                std::hint::black_box(decode(std::hint::black_box(&encoded)));
            }
            let dec_gbs = (encoded.len() * n) as f64 / start.elapsed().as_secs_f64() / 1e9;

            println!("{size:>6} B: encode {enc_gbs:5.2} GB/s   decode {dec_gbs:5.2} GB/s");
        }
    }

    #[test]
    fn decode_strips_all_trailing_padding_like_the_js_it_replaces() {
        // Looser than WHATWG (which allows at most two "="); recorded in the
        // module docs. "AAA" decodes to two bytes.
        assert_eq!(decode("AAA=====").as_deref(), Some("\0\0"));
    }
}
