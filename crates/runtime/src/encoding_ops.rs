//! Host ops backing `TextEncoder`/`TextDecoder` (SPEC §2.3). UTF-8 transcoding
//! in Rust + V8's native string conversion is faster than the pure-JS
//! code-point loop for non-trivial inputs.
//!
//! `TextEncoder` is UTF-8 by definition, so it needs nothing else. `TextDecoder`
//! takes any label the WHATWG Encoding Standard defines, resolved and decoded by
//! `encoding_rs` — the same implementation Firefox ships. UTF-8 keeps its own
//! dedicated path: it is the overwhelmingly common case and decodes in place
//! with no copy when the input is valid.
//!
//! Streaming decode needs state (a multi-byte sequence split across chunks, and
//! for ISO-2022-JP a shift state that can span any number of chunks), so a
//! `{ stream: true }` decode allocates a native decoder held in a registry —
//! the same shape as the compression contexts. A one-shot decode allocates
//! nothing: the decoder lives and dies inside the op.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use es_runtime_common::ExceptionClass;
use es_runtime_engine::{Engine, OpDecl, OpError, Value};

use crate::Result;

/// Resolves a label to its encoding, as the spec's "get an encoding" does.
fn encoding_for(label: &str) -> std::result::Result<&'static encoding_rs::Encoding, OpError> {
    encoding_rs::Encoding::for_label(label.as_bytes())
        .ok_or_else(|| OpError::range_error(format!("unsupported encoding label: {label}")))
}

/// Builds a decoder honouring the spec's `ignoreBOM`.
///
/// `with_bom_removal` strips a BOM **for this encoding only** — it does not let
/// the decoder morph into one for another encoding, which is what plain BOM
/// sniffing would do and is not `TextDecoder`'s behaviour.
fn new_decoder(enc: &'static encoding_rs::Encoding, ignore_bom: bool) -> encoding_rs::Decoder {
    if ignore_bom {
        enc.new_decoder_without_bom_handling()
    } else {
        enc.new_decoder_with_bom_removal()
    }
}

/// A `fatal: true` decoder met a byte sequence the encoding does not define.
fn malformed() -> OpError {
    OpError::new(
        ExceptionClass::TypeError,
        "the encoded data was not valid for the encoding",
    )
}

/// Decodes `bytes` through `decoder`, replacing errors with U+FFFD or — under
/// `fatal` — refusing.
///
/// `last` marks the end of the stream: a trailing incomplete sequence is only an
/// error once no more bytes can arrive.
fn decode(
    decoder: &mut encoding_rs::Decoder,
    bytes: &[u8],
    fatal: bool,
    last: bool,
) -> std::result::Result<String, OpError> {
    let capacity = if fatal {
        decoder.max_utf8_buffer_length_without_replacement(bytes.len())
    } else {
        decoder.max_utf8_buffer_length(bytes.len())
    }
    .ok_or_else(|| OpError::range_error("decoded text would be too large"))?;
    let mut out = String::with_capacity(capacity);

    if fatal {
        let (result, _read) = decoder.decode_to_string_without_replacement(bytes, &mut out, last);
        match result {
            encoding_rs::DecoderResult::InputEmpty => Ok(out),
            // The buffer was sized from `max_utf8_buffer_length_*`, so it cannot
            // be the thing that ran out.
            encoding_rs::DecoderResult::OutputFull => {
                Err(OpError::range_error("decoded text would be too large"))
            }
            encoding_rs::DecoderResult::Malformed(_, _) => Err(malformed()),
        }
    } else {
        let (_result, _read, _had_errors) = decoder.decode_to_string(bytes, &mut out, last);
        Ok(out)
    }
}

/// Registers `utf8_encode` / `utf8_decode` and the `TextDecoder` ops.
pub(crate) fn install(engine: &mut dyn Engine) -> Result<()> {
    // `arg 0` arrives already transcoded UTF-16 → UTF-8 by V8 (lone surrogates
    // become U+FFFD), which is exactly TextEncoder semantics — so the op is just
    // "hand the bytes back". The owned String's buffer becomes the returned
    // bytes (and ultimately the ArrayBuffer backing store) without a copy.
    engine.register_op(OpDecl::sync("utf8_encode", |args| {
        Ok(match args.into_iter().next() {
            Some(Value::String(s)) => Value::Bytes(s.into_bytes()),
            _ => Value::Bytes(Vec::new()),
        })
    }))?;

    // `(bytes, fatal, ignoreBOM)` → string. V8 builds the JS string natively.
    // The bytes are consumed: valid UTF-8 (the common case) converts in place
    // with no copy; only invalid input takes the lossy-replacement path.
    engine.register_op(OpDecl::sync("utf8_decode", |args| {
        let fatal = matches!(args.get(1), Some(Value::Bool(true)));
        let ignore_bom = matches!(args.get(2), Some(Value::Bool(true)));
        let mut bytes = match args.into_iter().next() {
            Some(Value::Bytes(b)) => b,
            _ => Vec::new(),
        };
        if !ignore_bom && bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
            bytes.drain(..3);
        }
        match String::from_utf8(bytes) {
            Ok(s) => Ok(Value::String(s)),
            Err(_) if fatal => Err(OpError::new(ExceptionClass::TypeError, "invalid UTF-8")),
            Err(e) => Ok(Value::String(
                String::from_utf8_lossy(e.as_bytes()).into_owned(),
            )),
        }
    }))?;

    // `label` → the canonical encoding name, lowercase as the spec's `encoding`
    // attribute reports it. An unknown label is a `RangeError`, which is what
    // the `TextDecoder` constructor must throw.
    engine.register_op(OpDecl::sync("encoding_for_label", |args| {
        let label = args.first().and_then(Value::as_str).unwrap_or("");
        Ok(Value::String(encoding_for(label)?.name().to_lowercase()))
    }))?;

    // One-shot decode: no state to keep, so the decoder never leaves the op.
    engine.register_op(OpDecl::sync("decode_once", |args| {
        let enc = encoding_for(args.first().and_then(Value::as_str).unwrap_or(""))?;
        let fatal = matches!(args.get(2), Some(Value::Bool(true)));
        let ignore_bom = matches!(args.get(3), Some(Value::Bool(true)));
        // Consumes the argument vector to take the byte buffer by value: the
        // UTF-8 path below turns it into the result string without copying it,
        // which borrowing would make impossible.
        let mut bytes = match args.into_iter().nth(1) {
            Some(Value::Bytes(b)) => b,
            _ => Vec::new(),
        };

        // UTF-8 is what `new TextDecoder()` gives you, so this is the path
        // almost every call takes — and for it a decode is a *validation*: the
        // bytes are already the output. `String::from_utf8` checks them in place
        // and hands back the same allocation.
        //
        // The general path cannot do that. It builds an `encoding_rs::Decoder`
        // per call and transcodes into a fresh `String` sized by
        // `max_utf8_buffer_length`, which for UTF-8 input is up to three times
        // the input length — an allocation and a copy, to produce bytes
        // identical to the ones it was given.
        if enc == encoding_rs::UTF_8 {
            if !ignore_bom && bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
                bytes.drain(..3);
            }
            return match String::from_utf8(bytes) {
                Ok(s) => Ok(Value::String(s)),
                Err(_) if fatal => Err(malformed()),
                // Same replacement convention as the general path: U+FFFD per
                // maximal subpart, which is what `utf8_decode` already relies on.
                Err(e) => Ok(Value::String(
                    String::from_utf8_lossy(e.as_bytes()).into_owned(),
                )),
            };
        }

        let mut decoder = new_decoder(enc, ignore_bom);
        Ok(Value::String(decode(&mut decoder, &bytes, fatal, true)?))
    }))?;

    // Streaming decode. The registry holds one decoder per in-flight stream;
    // `decoder_free` is called when the stream ends, and a `FinalizationRegistry`
    // in the prelude is the backstop for a decoder abandoned mid-stream.
    let decoders: Rc<RefCell<HashMap<u64, encoding_rs::Decoder>>> =
        Rc::new(RefCell::new(HashMap::new()));
    let next_id = Rc::new(RefCell::new(0u64));

    let registry = decoders.clone();
    let ids = next_id.clone();
    engine.register_op(OpDecl::sync("decoder_new", move |args| {
        let name = args.first().and_then(Value::as_str).unwrap_or("");
        let ignore_bom = matches!(args.get(1), Some(Value::Bool(true)));
        let decoder = new_decoder(encoding_for(name)?, ignore_bom);
        let mut id = ids.borrow_mut();
        *id += 1;
        registry.borrow_mut().insert(*id, decoder);
        Ok(Value::Number(*id as f64))
    }))?;

    let registry = decoders.clone();
    engine.register_op(OpDecl::sync("decoder_decode", move |args| {
        let id = args.first().and_then(Value::as_number).unwrap_or(0.0) as u64;
        let fatal = matches!(args.get(2), Some(Value::Bool(true)));
        let last = matches!(args.get(3), Some(Value::Bool(true)));
        let bytes = match args.get(1) {
            Some(Value::Bytes(b)) => b.as_slice(),
            _ => &[][..],
        };
        let mut registry = registry.borrow_mut();
        let decoder = registry
            .get_mut(&id)
            .ok_or_else(|| OpError::type_error("unknown decoder"))?;
        Ok(Value::String(decode(decoder, bytes, fatal, last)?))
    }))?;

    let registry = decoders;
    engine.register_op(OpDecl::sync("decoder_free", move |args| {
        let id = args.first().and_then(Value::as_number).unwrap_or(0.0) as u64;
        registry.borrow_mut().remove(&id);
        Ok(Value::Undefined)
    }))?;

    Ok(())
}

/// Fuzz entry: decode arbitrary bytes under an arbitrary label (see
/// [`crate::fuzz`]).
#[cfg(feature = "fuzzing")]
pub(crate) fn fuzz_decode(label: &str, bytes: &[u8], fatal: bool, ignore_bom: bool) {
    if let Ok(enc) = encoding_for(label) {
        let mut decoder = new_decoder(enc, ignore_bom);
        // Two chunks then a final empty one, so the streaming path — where a
        // sequence spans the boundary — is exercised as well as the one-shot.
        let mid = bytes.len() / 2;
        let _ = decode(&mut decoder, &bytes[..mid], fatal, false);
        let _ = decode(&mut decoder, &bytes[mid..], fatal, false);
        let _ = decode(&mut decoder, &[], fatal, true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The label table is the spec's, not a hand-written subset: aliases and
    /// case/whitespace folding all resolve to the canonical name.
    #[test]
    fn labels_resolve_the_way_the_spec_says() {
        for (label, name) in [
            ("utf-8", "utf-8"),
            ("UTF8", "utf-8"),
            ("  unicode-1-1-utf-8 ", "utf-8"),
            ("latin1", "windows-1252"),
            ("ISO-8859-1", "windows-1252"),
            ("utf-16", "utf-16le"),
            ("ucs-2", "utf-16le"),
            ("shift_jis", "shift_jis"),
            ("sjis", "shift_jis"),
            ("gb2312", "gbk"),
        ] {
            let resolved = encoding_for(label.trim()).unwrap().name().to_lowercase();
            assert_eq!(resolved, name, "label {label}");
        }
        assert!(encoding_for("definitely-not-an-encoding").is_err());
    }

    #[test]
    fn decodes_the_non_utf8_encodings() {
        // "aé" in UTF-16LE, windows-1252 and UTF-16BE.
        let utf16le = encoding_for("utf-16le").unwrap();
        let mut d = new_decoder(utf16le, true);
        assert_eq!(
            decode(&mut d, &[0x61, 0x00, 0xe9, 0x00], false, true).unwrap(),
            "aé"
        );

        let cp1252 = encoding_for("windows-1252").unwrap();
        let mut d = new_decoder(cp1252, true);
        // 0x80 is the euro sign in windows-1252 — the byte that distinguishes it
        // from ISO-8859-1, which the spec deliberately maps to this encoding.
        assert_eq!(
            decode(&mut d, &[0x61, 0xe9, 0x80], false, true).unwrap(),
            "aé€"
        );

        let utf16be = encoding_for("utf-16be").unwrap();
        let mut d = new_decoder(utf16be, true);
        assert_eq!(
            decode(&mut d, &[0x00, 0x61, 0x00, 0xe9], false, true).unwrap(),
            "aé"
        );
    }

    /// The reason streaming needs a *held* decoder: a character split across
    /// chunks must survive the boundary.
    #[test]
    fn a_split_sequence_survives_the_chunk_boundary() {
        let mut d = new_decoder(encoding_for("utf-16le").unwrap(), true);
        assert_eq!(
            decode(&mut d, &[0x61, 0x00, 0xe9], false, false).unwrap(),
            "a"
        );
        assert_eq!(decode(&mut d, &[0x00], false, false).unwrap(), "é");

        // Ending the stream on a half-finished unit is where it becomes an error.
        let mut d = new_decoder(encoding_for("utf-16le").unwrap(), true);
        assert_eq!(
            decode(&mut d, &[0x61, 0x00, 0xe9], false, true).unwrap(),
            "a\u{fffd}"
        );
        let mut d = new_decoder(encoding_for("utf-16le").unwrap(), true);
        assert!(decode(&mut d, &[0x61, 0x00, 0xe9], true, true).is_err());
    }

    #[test]
    fn bom_removal_follows_ignore_bom() {
        let utf8 = encoding_for("utf-8").unwrap();
        let bytes = [0xef, 0xbb, 0xbf, 0x61];
        let mut stripping = new_decoder(utf8, false);
        assert_eq!(decode(&mut stripping, &bytes, false, true).unwrap(), "a");
        let mut keeping = new_decoder(utf8, true);
        assert_eq!(
            decode(&mut keeping, &bytes, false, true).unwrap(),
            "\u{feff}a"
        );

        // A BOM for a *different* encoding is data, not a BOM: the decoder must
        // not morph into a UTF-16 decoder the way plain sniffing would.
        let mut d = new_decoder(encoding_for("windows-1252").unwrap(), false);
        assert_eq!(
            decode(&mut d, &[0xff, 0xfe, 0x61], false, true).unwrap(),
            "ÿþa"
        );
    }

    #[test]
    fn fatal_refuses_what_lossy_replaces() {
        let utf8 = encoding_for("utf-8").unwrap();
        let mut lossy = new_decoder(utf8, true);
        assert_eq!(
            decode(&mut lossy, &[0x61, 0xff, 0x62], false, true).unwrap(),
            "a\u{fffd}b"
        );
        let mut strict = new_decoder(utf8, true);
        assert!(decode(&mut strict, &[0x61, 0xff, 0x62], true, true).is_err());
    }
}
