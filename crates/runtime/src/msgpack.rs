//! A direct MessagePack codec over the runtime's [`Value`] tree.
//!
//! The rest of `runtime:serialization` pivots through JSON — parse to a JSON
//! string, let JS `JSON.parse` build the object graph, which is measurably
//! faster than marshaling a `Value` tree across the FFI boundary. MessagePack
//! cannot use that pivot alone: JSON has no byte string, so the `bin` family
//! (the reason to reach for a binary format at all) was being flattened to
//! `null` on the way out and to an array of numbers on the way back.
//!
//! So the JSON pivot is kept for documents that *are* JSON-shaped — the common
//! case, and the fast one — and this module handles the rest:
//!
//! * [`scan_binary`] walks the encoded form structurally, allocating nothing,
//!   and reports whether any `bin` or `ext` value is present. Only when one is
//!   does decoding pay for a `Value` tree.
//! * [`read_value`] decodes to a `Value`, so `bin` arrives in JS as a
//!   `Uint8Array` rather than a list of integers.
//! * [`write_value`] encodes from a `Value`, emitting `bin` for
//!   [`Value::Bytes`] and *failing* on anything with no MessagePack
//!   representation rather than silently writing `nil`.
//!
//! Both directions are written against the format's marker table directly. The
//! alternative pivot (`rmpv`) is not in the dependency tree, and the table is
//! small enough that owning it beats adding a crate to carry it.

use es_runtime_engine::Value;

/// How deep a document may nest before it is rejected.
///
/// The input is guest-supplied and both walks are recursive, so this is the
/// bound that keeps a hostile document from exhausting the stack. Deeper than
/// this is not a document anyone wrote by hand.
const MAX_DEPTH: usize = 256;

/// Why a MessagePack byte string could not be read.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum DecodeError {
    /// The input ended mid-value.
    Truncated,
    /// A marker byte with no meaning in the format (`0xc1`).
    ReservedMarker,
    /// A `str` value whose payload is not UTF-8.
    InvalidUtf8,
    /// Nesting beyond [`MAX_DEPTH`].
    TooDeep,
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::Truncated => f.write_str("input ended in the middle of a value"),
            DecodeError::ReservedMarker => f.write_str("reserved marker byte 0xc1"),
            DecodeError::InvalidUtf8 => f.write_str("a str value is not valid UTF-8"),
            DecodeError::TooDeep => {
                write!(f, "nesting deeper than {MAX_DEPTH} levels")
            }
        }
    }
}

/// Why a [`Value`] could not be encoded.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct EncodeError(pub(crate) String);

impl std::fmt::Display for EncodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

// ---- cursor helpers --------------------------------------------------------

fn take<'a>(cur: &mut &'a [u8], n: usize) -> Result<&'a [u8], DecodeError> {
    if cur.len() < n {
        return Err(DecodeError::Truncated);
    }
    let (head, tail) = cur.split_at(n);
    *cur = tail;
    Ok(head)
}

fn byte(cur: &mut &[u8]) -> Result<u8, DecodeError> {
    Ok(take(cur, 1)?[0])
}

/// Reads an `n`-byte big-endian unsigned integer.
fn be(cur: &mut &[u8], n: usize) -> Result<u64, DecodeError> {
    Ok(take(cur, n)?
        .iter()
        .fold(0u64, |acc, &b| (acc << 8) | u64::from(b)))
}

/// What a marker byte introduces. Lengths are already resolved, so both walks
/// share one reading of the table.
enum Shape {
    /// A complete value carrying no payload (nil, bool, fixint).
    Atom(Value),
    /// `n` payload bytes forming a string.
    Str(usize),
    /// `n` payload bytes forming a byte string.
    Bin(usize),
    /// A big-endian unsigned integer of `n` bytes.
    Uint(usize),
    /// A big-endian two's-complement integer of `n` bytes.
    Int(usize),
    /// An IEEE-754 float of `n` bytes (4 or 8).
    Float(usize),
    /// `n` element values follow.
    Array(usize),
    /// `n` key/value pairs follow (so `2n` values).
    Map(usize),
    /// An extension: `n` payload bytes, with the type byte already consumed.
    Ext(usize),
}

/// Reads one marker (and its length header) from `cur`.
fn shape(cur: &mut &[u8]) -> Result<Shape, DecodeError> {
    let m = byte(cur)?;
    Ok(match m {
        0x00..=0x7f => Shape::Atom(Value::Number(f64::from(m))),
        0xe0..=0xff => Shape::Atom(Value::Number(f64::from(m as i8))),
        0x80..=0x8f => Shape::Map(usize::from(m & 0x0f)),
        0x90..=0x9f => Shape::Array(usize::from(m & 0x0f)),
        0xa0..=0xbf => Shape::Str(usize::from(m & 0x1f)),
        0xc0 => Shape::Atom(Value::Null),
        0xc1 => return Err(DecodeError::ReservedMarker),
        0xc2 => Shape::Atom(Value::Bool(false)),
        0xc3 => Shape::Atom(Value::Bool(true)),
        0xc4 => Shape::Bin(be(cur, 1)? as usize),
        0xc5 => Shape::Bin(be(cur, 2)? as usize),
        0xc6 => Shape::Bin(be(cur, 4)? as usize),
        0xc7 => {
            let n = be(cur, 1)? as usize;
            byte(cur)?; // ext type
            Shape::Ext(n)
        }
        0xc8 => {
            let n = be(cur, 2)? as usize;
            byte(cur)?;
            Shape::Ext(n)
        }
        0xc9 => {
            let n = be(cur, 4)? as usize;
            byte(cur)?;
            Shape::Ext(n)
        }
        0xca => Shape::Float(4),
        0xcb => Shape::Float(8),
        0xcc => Shape::Uint(1),
        0xcd => Shape::Uint(2),
        0xce => Shape::Uint(4),
        0xcf => Shape::Uint(8),
        0xd0 => Shape::Int(1),
        0xd1 => Shape::Int(2),
        0xd2 => Shape::Int(4),
        0xd3 => Shape::Int(8),
        0xd4..=0xd8 => {
            byte(cur)?; // ext type
            Shape::Ext(1usize << (m - 0xd4))
        }
        0xd9 => Shape::Str(be(cur, 1)? as usize),
        0xda => Shape::Str(be(cur, 2)? as usize),
        0xdb => Shape::Str(be(cur, 4)? as usize),
        0xdc => Shape::Array(be(cur, 2)? as usize),
        0xdd => Shape::Array(be(cur, 4)? as usize),
        0xde => Shape::Map(be(cur, 2)? as usize),
        0xdf => Shape::Map(be(cur, 4)? as usize),
    })
}

// ---- scanning --------------------------------------------------------------

/// Whether the encoded value contains any `bin` or `ext`.
///
/// Exact rather than a byte search: a `0xc4` inside a string payload is not a
/// `bin` marker, and treating it as one would push every document carrying
/// arbitrary text onto the slow path. Allocates nothing and reads no payload.
pub(crate) fn scan_binary(bytes: &[u8]) -> Result<bool, DecodeError> {
    let mut cur = bytes;
    let mut found = false;
    scan_one(&mut cur, 0, &mut found)?;
    Ok(found)
}

fn scan_one(cur: &mut &[u8], depth: usize, found: &mut bool) -> Result<(), DecodeError> {
    if depth > MAX_DEPTH {
        return Err(DecodeError::TooDeep);
    }
    match shape(cur)? {
        Shape::Atom(_) => {}
        Shape::Str(n) | Shape::Uint(n) | Shape::Int(n) | Shape::Float(n) => {
            take(cur, n)?;
        }
        Shape::Bin(n) | Shape::Ext(n) => {
            *found = true;
            take(cur, n)?;
        }
        Shape::Array(n) => {
            for _ in 0..n {
                scan_one(cur, depth + 1, found)?;
            }
        }
        Shape::Map(n) => {
            for _ in 0..n {
                scan_one(cur, depth + 1, found)?;
                scan_one(cur, depth + 1, found)?;
            }
        }
    }
    Ok(())
}

// ---- decoding --------------------------------------------------------------

/// Decodes one MessagePack value into a [`Value`].
pub(crate) fn read_value(bytes: &[u8]) -> Result<Value, DecodeError> {
    let mut cur = bytes;
    read_one(&mut cur, 0)
}

fn read_one(cur: &mut &[u8], depth: usize) -> Result<Value, DecodeError> {
    if depth > MAX_DEPTH {
        return Err(DecodeError::TooDeep);
    }
    Ok(match shape(cur)? {
        Shape::Atom(v) => v,
        Shape::Str(n) => {
            let raw = take(cur, n)?;
            Value::String(
                std::str::from_utf8(raw)
                    .map_err(|_| DecodeError::InvalidUtf8)?
                    .to_owned(),
            )
        }
        Shape::Bin(n) => Value::Bytes(take(cur, n)?.to_vec()),
        Shape::Uint(n) => Value::Number(be(cur, n)? as f64),
        Shape::Int(n) => {
            let raw = be(cur, n)?;
            // Sign-extend from the width actually on the wire.
            let bits = n * 8;
            let signed = if bits == 64 {
                raw as i64
            } else {
                let shift = 64 - bits;
                ((raw << shift) as i64) >> shift
            };
            Value::Number(signed as f64)
        }
        Shape::Float(4) => Value::Number(f64::from(f32::from_bits(be(cur, 4)? as u32))),
        Shape::Float(_) => Value::Number(f64::from_bits(be(cur, 8)?)),
        Shape::Ext(n) => {
            // No JS type corresponds to an extension, and inventing one would
            // make it indistinguishable from real data. The bytes are handed
            // over as-is, which at least keeps them.
            Value::Bytes(take(cur, n)?.to_vec())
        }
        Shape::Array(n) => {
            let mut out = Vec::with_capacity(n.min(1024));
            for _ in 0..n {
                out.push(read_one(cur, depth + 1)?);
            }
            Value::Array(out)
        }
        Shape::Map(n) => {
            let mut out = Vec::with_capacity(n.min(1024));
            for _ in 0..n {
                // A non-string key has no object-property equivalent, so it is
                // stringified the way a JS property access would.
                let key = match read_one(cur, depth + 1)? {
                    Value::String(s) => s,
                    Value::Number(n) => format_number(n),
                    Value::Bool(b) => b.to_string(),
                    Value::Null => "null".to_string(),
                    _ => "[object]".to_string(),
                };
                out.push((key, read_one(cur, depth + 1)?));
            }
            Value::Object(out)
        }
    })
}

/// `String(n)` for a JS number, so a numeric map key round-trips as JS spells
/// it (`1`, not `1.0`).
fn format_number(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e21 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

// ---- encoding --------------------------------------------------------------

/// Encodes a [`Value`] as MessagePack.
pub(crate) fn write_value(v: &Value) -> Result<Vec<u8>, EncodeError> {
    let mut out = Vec::new();
    write_one(v, &mut out, 0)?;
    Ok(out)
}

fn write_len(
    out: &mut Vec<u8>,
    fix_base: u8,
    fix_max: usize,
    m8: Option<u8>,
    m16: u8,
    m32: u8,
    n: usize,
) {
    if n <= fix_max {
        out.push(fix_base | (n as u8));
    } else if n <= u8::MAX as usize && m8.is_some() {
        out.push(m8.expect("checked"));
        out.push(n as u8);
    } else if n <= u16::MAX as usize {
        out.push(m16);
        out.extend_from_slice(&(n as u16).to_be_bytes());
    } else {
        out.push(m32);
        out.extend_from_slice(&(n as u32).to_be_bytes());
    }
}

fn write_one(v: &Value, out: &mut Vec<u8>, depth: usize) -> Result<(), EncodeError> {
    if depth > MAX_DEPTH {
        return Err(EncodeError(format!(
            "value nests deeper than {MAX_DEPTH} levels"
        )));
    }
    match v {
        // `undefined` has no MessagePack form; nil is what JSON.stringify's
        // nearest equivalent would produce and what every encoder emits.
        Value::Undefined | Value::Null => out.push(0xc0),
        Value::Bool(false) => out.push(0xc2),
        Value::Bool(true) => out.push(0xc3),
        Value::Number(n) => write_number(*n, out),
        Value::String(s) => {
            write_len(out, 0xa0, 0x1f, Some(0xd9), 0xda, 0xdb, s.len());
            out.extend_from_slice(s.as_bytes());
        }
        // The whole point of a binary format, and previously written as nil.
        Value::Bytes(b) => {
            if b.len() <= u8::MAX as usize {
                out.push(0xc4);
                out.push(b.len() as u8);
            } else if b.len() <= u16::MAX as usize {
                out.push(0xc5);
                out.extend_from_slice(&(b.len() as u16).to_be_bytes());
            } else {
                out.push(0xc6);
                out.extend_from_slice(&(b.len() as u32).to_be_bytes());
            }
            out.extend_from_slice(b);
        }
        Value::Array(items) => {
            write_len(out, 0x90, 0x0f, None, 0xdc, 0xdd, items.len());
            for item in items {
                write_one(item, out, depth + 1)?;
            }
        }
        Value::Object(entries) => {
            write_len(out, 0x80, 0x0f, None, 0xde, 0xdf, entries.len());
            for (k, val) in entries {
                write_len(out, 0xa0, 0x1f, Some(0xd9), 0xda, 0xdb, k.len());
                out.extend_from_slice(k.as_bytes());
                write_one(val, out, depth + 1)?;
            }
        }
        // Anything the marshaller could not turn into a structured value — a
        // function, a symbol, a class instance with no enumerable own data.
        // Writing nil for these is how a `Map` used to encode as an empty
        // object and a `Uint8Array` as null: silent, total data loss in a
        // format chosen for fidelity. Refusing says so at the call site.
        other => {
            return Err(EncodeError(format!(
                "cannot encode {} as MessagePack",
                describe(other)
            )));
        }
    }
    Ok(())
}

fn write_number(n: f64, out: &mut Vec<u8>) {
    if n.fract() == 0.0 && n >= -(2f64.powi(63)) && n < 2f64.powi(63) {
        let i = n as i64;
        if i >= 0 {
            let u = i as u64;
            if u < 128 {
                out.push(u as u8);
            } else if u <= u8::MAX as u64 {
                out.push(0xcc);
                out.push(u as u8);
            } else if u <= u16::MAX as u64 {
                out.push(0xcd);
                out.extend_from_slice(&(u as u16).to_be_bytes());
            } else if u <= u32::MAX as u64 {
                out.push(0xce);
                out.extend_from_slice(&(u as u32).to_be_bytes());
            } else {
                out.push(0xcf);
                out.extend_from_slice(&u.to_be_bytes());
            }
        } else if i >= -32 {
            out.push(i as i8 as u8);
        } else if i >= i64::from(i8::MIN) {
            out.push(0xd0);
            out.push(i as i8 as u8);
        } else if i >= i64::from(i16::MIN) {
            out.push(0xd1);
            out.extend_from_slice(&(i as i16).to_be_bytes());
        } else if i >= i64::from(i32::MIN) {
            out.push(0xd2);
            out.extend_from_slice(&(i as i32).to_be_bytes());
        } else {
            out.push(0xd3);
            out.extend_from_slice(&i.to_be_bytes());
        }
    } else {
        // Non-integral, out of i64 range, or non-finite: float64 carries all of
        // them exactly, NaN and the infinities included.
        out.push(0xcb);
        out.extend_from_slice(&n.to_bits().to_be_bytes());
    }
}

fn describe(v: &Value) -> &'static str {
    match v {
        Value::Undefined => "undefined",
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Bytes(_) => "bytes",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
        _ => "a value with no MessagePack representation (a function, symbol, or similar)",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round(v: Value) -> Value {
        read_value(&write_value(&v).expect("encode")).expect("decode")
    }

    #[test]
    fn bytes_survive_a_round_trip() {
        // The headline defect: a `Uint8Array` used to encode as `nil` (0xc0),
        // so every binary payload was silently destroyed.
        let encoded = write_value(&Value::Bytes(vec![1, 2, 3])).expect("encode");
        assert_eq!(
            encoded,
            vec![0xc4, 0x03, 1, 2, 3],
            "must be the bin8 family"
        );
        assert_eq!(
            round(Value::Bytes(vec![1, 2, 3])),
            Value::Bytes(vec![1, 2, 3])
        );
    }

    #[test]
    fn nested_bytes_survive_too() {
        let v = Value::Object(vec![(
            "k".into(),
            Value::Array(vec![Value::Bytes(vec![9, 8]), Value::String("s".into())]),
        )]);
        assert_eq!(round(v.clone()), v);
    }

    #[test]
    fn bin_decodes_as_bytes_not_an_array_of_numbers() {
        // Foreign MessagePack, hand-built: bin8 of three bytes.
        assert_eq!(
            read_value(&[0xc4, 0x03, 1, 2, 3]).expect("decode"),
            Value::Bytes(vec![1, 2, 3]),
        );
    }

    #[test]
    fn every_length_class_round_trips() {
        for n in [0usize, 1, 31, 32, 255, 256, 65535, 65536] {
            let bytes = Value::Bytes(vec![7u8; n]);
            assert_eq!(round(bytes.clone()), bytes, "bin of {n}");
            let s = Value::String("x".repeat(n));
            assert_eq!(round(s.clone()), s, "str of {n}");
            let arr = Value::Array(vec![Value::Null; n.min(70000)]);
            assert_eq!(round(arr.clone()), arr, "array of {n}");
        }
    }

    #[test]
    fn integers_use_their_narrowest_form_and_survive() {
        for n in [
            0.0,
            1.0,
            127.0,
            128.0,
            255.0,
            256.0,
            65535.0,
            65536.0,
            4294967296.0,
            -1.0,
            -32.0,
            -33.0,
            -128.0,
            -129.0,
            -32768.0,
            -32769.0,
            -2147483648.0,
            -2147483649.0,
        ] {
            assert_eq!(round(Value::Number(n)), Value::Number(n), "{n}");
        }
        assert_eq!(
            write_value(&Value::Number(1.0)).expect("encode"),
            vec![0x01]
        );
        assert_eq!(
            write_value(&Value::Number(-1.0)).expect("encode"),
            vec![0xff]
        );
    }

    #[test]
    fn non_integral_and_non_finite_numbers_survive() {
        assert_eq!(round(Value::Number(1.5)), Value::Number(1.5));
        assert_eq!(
            round(Value::Number(f64::INFINITY)),
            Value::Number(f64::INFINITY)
        );
        let nan = round(Value::Number(f64::NAN));
        match nan {
            Value::Number(n) => assert!(n.is_nan()),
            other => panic!("expected a number, got {other:?}"),
        }
    }

    #[test]
    fn a_value_with_no_representation_is_refused_rather_than_nil() {
        // Silently writing nil is what made a Map encode as `{}` and a function
        // as null — the failure mode this refusal exists to replace.
        let err = write_value(&Value::Other("function () {}".into())).expect_err("must refuse");
        assert!(err.0.contains("cannot encode"), "{}", err.0);
    }

    #[test]
    fn the_binary_scan_is_exact() {
        // No bin anywhere: the JSON fast path stays available.
        let plain = write_value(&Value::Object(vec![
            ("a".into(), Value::Number(1.0)),
            ("b".into(), Value::Array(vec![Value::String("x".into())])),
        ]))
        .expect("encode");
        assert!(!scan_binary(&plain).expect("scan"));

        // A 0xc4 byte *inside a string* is not a bin marker; treating it as one
        // would push ordinary text documents onto the slow path.
        let text = write_value(&Value::String("\u{0104}".into())).expect("encode");
        assert!(
            text.contains(&0xc4),
            "the fixture needs a 0xc4 payload byte"
        );
        assert!(!scan_binary(&text).expect("scan"));

        // …and a real bin, however deeply nested, is found.
        let nested = write_value(&Value::Array(vec![Value::Object(vec![(
            "k".into(),
            Value::Bytes(vec![1]),
        )])]))
        .expect("encode");
        assert!(scan_binary(&nested).expect("scan"));
    }

    #[test]
    fn malformed_input_is_an_error_not_a_panic() {
        assert_eq!(read_value(&[]), Err(DecodeError::Truncated));
        assert_eq!(read_value(&[0xc1]), Err(DecodeError::ReservedMarker));
        assert_eq!(read_value(&[0xc4, 0x05, 1]), Err(DecodeError::Truncated));
        // A str whose payload is not UTF-8.
        assert_eq!(read_value(&[0xa1, 0xff]), Err(DecodeError::InvalidUtf8));
        // A length header promising more than the input holds must not
        // pre-allocate it either.
        assert_eq!(
            read_value(&[0xdd, 0xff, 0xff, 0xff, 0xff]),
            Err(DecodeError::Truncated)
        );
        assert_eq!(scan_binary(&[0xc4, 0x05, 1]), Err(DecodeError::Truncated));
    }

    #[test]
    fn deep_nesting_is_refused_rather_than_overflowing_the_stack() {
        // 0x91 is a one-element array: a chain of them nests without bound.
        let bomb = vec![0x91u8; MAX_DEPTH + 8];
        assert_eq!(read_value(&bomb), Err(DecodeError::TooDeep));
        assert_eq!(scan_binary(&bomb), Err(DecodeError::TooDeep));
    }

    #[test]
    fn ext_values_keep_their_payload() {
        // fixext1, type 5, payload 0x42 — decoded as bytes rather than dropped.
        assert_eq!(
            read_value(&[0xd4, 0x05, 0x42]),
            Ok(Value::Bytes(vec![0x42]))
        );
        assert!(scan_binary(&[0xd4, 0x05, 0x42]).expect("scan"));
    }

    #[test]
    fn map_keys_become_property_names() {
        // Integer keys are legal MessagePack and become the string a JS
        // property access would use.
        assert_eq!(
            read_value(&[0x81, 0x01, 0x02]).expect("decode"),
            Value::Object(vec![("1".into(), Value::Number(2.0))]),
        );
    }
}
