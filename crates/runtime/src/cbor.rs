//! A direct CBOR (RFC 8949) codec over the runtime's [`Value`] tree.
//!
//! Like MessagePack, CBOR is a binary format and decoding it moves straight to
//! the value tree rather than through the JSON pivot the text parsers use — the
//! pivot has no byte string *and* no `Infinity`/`NaN`, so a document using
//! either would be silently narrowed on the way in. The encoded form is walked
//! once, structurally, and every value that has a JavaScript counterpart
//! becomes one.
//!
//! The choices the format leaves open are pinned to the ones the module can
//! defend (see DECISIONS.md D170):
//!
//! * **Decoding is permissive over well-formed CBOR** — definite and
//!   indefinite-length strings, arrays and maps all read, and nesting is
//!   bounded by [`MAX_DEPTH`].
//! * **Encoding is the deterministic (canonical) form of RFC 8949 §4.2** —
//!   shortest-form integers and lengths, the shortest float that preserves the
//!   value, and map keys sorted by the bytewise lexicographic order of their
//!   encodings. So the same value always encodes to the same bytes.
//! * **Tags are refused.** Major type 6 has no JavaScript counterpart, and
//!   inventing one would make a tagged value indistinguishable from real data.
//!   `decode` reports the tag number; `encode` never writes one.
//!
//! Numbers inside JavaScript's safe range decode to a `number`; an integer past
//! 2<sup>53</sup> decodes to a `BigInt`, which CBOR can carry whole where the
//! JSON pivot of MessagePack cannot. `encode` accepts a `BigInt` in
//! `[-2^64, 2^64-1]` and refuses one beyond it, which would need a bignum tag.

use es_runtime_engine::Value;

/// How deep a document may nest before it is rejected.
///
/// The input is guest-supplied and both walks are recursive, so this is the
/// bound that keeps a hostile document from exhausting the stack. Deeper than
/// this is not a document anyone wrote by hand.
const MAX_DEPTH: usize = 256;

/// JavaScript's `Number.MAX_SAFE_INTEGER` (2<sup>53</sup> - 1): the largest
/// magnitude a `number` holds exactly.
const MAX_SAFE_INTEGER: i128 = 9_007_199_254_740_991;

/// Wraps an exact CBOR integer as a `number` while it is inside JavaScript's
/// safe range and as a `BigInt` once it is not.
fn int_to_value(i: i128) -> Value {
    if (-MAX_SAFE_INTEGER..=MAX_SAFE_INTEGER).contains(&i) {
        Value::Number(i as f64)
    } else {
        Value::BigInt(i)
    }
}

/// Why a CBOR document could not be read.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum DecodeError {
    /// The input ended mid-value.
    Truncated,
    /// Bytes remained after the top-level value.
    TrailingData,
    /// A `text` string whose payload is not UTF-8.
    InvalidUtf8,
    /// Nesting beyond [`MAX_DEPTH`].
    TooDeep,
    /// An initial byte whose additional information is reserved (`28`–`30`), or
    /// an indefinite-length argument where one is not allowed.
    ReservedHeader,
    /// A `break` (`0xff`) outside an indefinite-length item.
    UnexpectedBreak,
    /// A tag (major type 6) — refused, carrying its number.
    Tag(u64),
    /// A simple value (major type 7) with no JavaScript counterpart.
    UnsupportedSimple(u8),
    /// A simple value below 32 in the two-byte form (`0xf8 0x00`–`0xf8 0x1f`),
    /// which RFC 8949 §3.3 makes not well-formed: those values have a one-byte
    /// form, so the second spelling would give `false` two encodings.
    TwoByteSimple(u8),
    /// A chunk inside an indefinite-length string was not a definite string of
    /// the same major type.
    BadIndefiniteChunk,
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::Truncated => f.write_str("input ended in the middle of a value"),
            DecodeError::TrailingData => f.write_str("bytes remain after the top-level value"),
            DecodeError::InvalidUtf8 => f.write_str("a text string is not valid UTF-8"),
            DecodeError::TooDeep => write!(f, "nesting deeper than {MAX_DEPTH} levels"),
            DecodeError::ReservedHeader => f.write_str("a reserved initial byte or length form"),
            DecodeError::UnexpectedBreak => {
                f.write_str("a break outside an indefinite-length item")
            }
            DecodeError::Tag(n) => {
                write!(
                    f,
                    "unsupported tag {n} (tags have no JavaScript counterpart)"
                )
            }
            DecodeError::UnsupportedSimple(n) => {
                write!(f, "unsupported simple value {n}")
            }
            DecodeError::TwoByteSimple(n) => {
                write!(f, "simple value {n} in the two-byte form (not well-formed)")
            }
            DecodeError::BadIndefiniteChunk => {
                f.write_str("an indefinite-length string held a chunk of the wrong type")
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

/// Whether the cursor rests on a `break` stop code (`0xff`).
fn at_break(cur: &[u8]) -> bool {
    cur.first() == Some(&0xff)
}

/// Reads the numeric argument introduced by additional information `ai`.
///
/// `28`–`30` are reserved everywhere and `31` (indefinite length) is not a
/// number, so both are rejected here; callers that accept an indefinite length
/// check for `ai == 31` before calling.
fn read_arg(cur: &mut &[u8], ai: u8) -> Result<u64, DecodeError> {
    match ai {
        0..=23 => Ok(u64::from(ai)),
        24 => Ok(u64::from(byte(cur)?)),
        25 => be(cur, 2),
        26 => be(cur, 4),
        27 => be(cur, 8),
        _ => Err(DecodeError::ReservedHeader),
    }
}

fn read_string(cur: &mut &[u8], n: usize) -> Result<String, DecodeError> {
    let raw = take(cur, n)?;
    std::str::from_utf8(raw)
        .map(str::to_owned)
        .map_err(|_| DecodeError::InvalidUtf8)
}

// ---- decoding --------------------------------------------------------------

/// Decodes one CBOR value into a [`Value`], requiring the whole input be
/// consumed.
pub(crate) fn read_value(bytes: &[u8]) -> Result<Value, DecodeError> {
    let mut cur = bytes;
    let v = read_one(&mut cur, 0)?;
    if !cur.is_empty() {
        return Err(DecodeError::TrailingData);
    }
    Ok(v)
}

/// Checks that `bytes` is exactly one well-formed CBOR value, without building
/// the value tree.
///
/// `validate` accepts exactly what [`read_value`] accepts — same headers, same
/// depth bound, the same UTF-8 requirement on text strings, the same refusal of
/// tags — but the walk allocates nothing: a string is checked in place and a map
/// key is skipped rather than spelled. So testing untrusted input for validity
/// costs its own bytes, not a decoded array or map the caller will throw away.
pub(crate) fn validate(bytes: &[u8]) -> Result<(), DecodeError> {
    let mut cur = bytes;
    skip_one(&mut cur, 0)?;
    if !cur.is_empty() {
        return Err(DecodeError::TrailingData);
    }
    Ok(())
}

/// The value-free counterpart of [`read_one`]: walks one item and checks it is
/// well-formed, building nothing. It must accept exactly what `read_one`
/// accepts; `validate_agrees_with_decode` pins that agreement.
fn skip_one(cur: &mut &[u8], depth: usize) -> Result<(), DecodeError> {
    if depth > MAX_DEPTH {
        return Err(DecodeError::TooDeep);
    }
    let head = byte(cur)?;
    let major = head >> 5;
    let ai = head & 0x1f;
    match major {
        // Integers: the argument is read only to be consumed.
        0 | 1 => {
            read_arg(cur, ai)?;
        }
        // Byte string.
        2 => {
            if ai == 31 {
                loop {
                    if at_break(cur) {
                        byte(cur)?;
                        break;
                    }
                    let chunk = byte(cur)?;
                    if chunk >> 5 != 2 || chunk & 0x1f == 31 {
                        return Err(DecodeError::BadIndefiniteChunk);
                    }
                    let n = read_arg(cur, chunk & 0x1f)? as usize;
                    take(cur, n)?;
                }
            } else {
                let n = read_arg(cur, ai)? as usize;
                take(cur, n)?;
            }
        }
        // Text string: each chunk is checked for UTF-8 where it lies.
        3 => {
            if ai == 31 {
                loop {
                    if at_break(cur) {
                        byte(cur)?;
                        break;
                    }
                    let chunk = byte(cur)?;
                    if chunk >> 5 != 3 || chunk & 0x1f == 31 {
                        return Err(DecodeError::BadIndefiniteChunk);
                    }
                    let n = read_arg(cur, chunk & 0x1f)? as usize;
                    check_utf8(cur, n)?;
                }
            } else {
                let n = read_arg(cur, ai)? as usize;
                check_utf8(cur, n)?;
            }
        }
        // Array.
        4 => {
            if ai == 31 {
                loop {
                    if at_break(cur) {
                        byte(cur)?;
                        break;
                    }
                    skip_one(cur, depth + 1)?;
                }
            } else {
                let n = read_arg(cur, ai)? as usize;
                for _ in 0..n {
                    skip_one(cur, depth + 1)?;
                }
            }
        }
        // Map: keys and values are skipped the same way.
        5 => {
            if ai == 31 {
                loop {
                    if at_break(cur) {
                        byte(cur)?;
                        break;
                    }
                    skip_one(cur, depth + 1)?;
                    skip_one(cur, depth + 1)?;
                }
            } else {
                let n = read_arg(cur, ai)? as usize;
                for _ in 0..n {
                    skip_one(cur, depth + 1)?;
                    skip_one(cur, depth + 1)?;
                }
            }
        }
        // Tags have no JavaScript counterpart.
        6 => return Err(DecodeError::Tag(read_arg(cur, ai)?)),
        // Simple values and floats.
        7 => match ai {
            20..=23 => {}
            24 => match byte(cur)? {
                n @ 0..=31 => return Err(DecodeError::TwoByteSimple(n)),
                other => return Err(DecodeError::UnsupportedSimple(other)),
            },
            25 => {
                take(cur, 2)?;
            }
            26 => {
                take(cur, 4)?;
            }
            27 => {
                take(cur, 8)?;
            }
            28..=30 => return Err(DecodeError::ReservedHeader),
            31 => return Err(DecodeError::UnexpectedBreak),
            _ => return Err(DecodeError::UnsupportedSimple(ai)),
        },
        _ => unreachable!("major type is three bits"),
    }
    Ok(())
}

/// Validates a definite-length text string in place, allocating no `String` for
/// it.
fn check_utf8(cur: &mut &[u8], n: usize) -> Result<(), DecodeError> {
    std::str::from_utf8(take(cur, n)?).map_err(|_| DecodeError::InvalidUtf8)?;
    Ok(())
}

fn read_one(cur: &mut &[u8], depth: usize) -> Result<Value, DecodeError> {
    if depth > MAX_DEPTH {
        return Err(DecodeError::TooDeep);
    }
    let head = byte(cur)?;
    let major = head >> 5;
    let ai = head & 0x1f;
    Ok(match major {
        // Unsigned and negative integers. An integer inside JavaScript's safe
        // range is a `number`; one past it is a `BigInt`, so no CBOR integer
        // loses precision. The negative value is formed in `i128` first, so an
        // argument just past 2^53 lands on the exact integer rather than one ulp
        // off a number that is itself exactly representable.
        0 => int_to_value(i128::from(read_arg(cur, ai)?)),
        1 => int_to_value(-1 - i128::from(read_arg(cur, ai)?)),
        // Byte string.
        2 => {
            if ai == 31 {
                let mut buf = Vec::new();
                loop {
                    if at_break(cur) {
                        byte(cur)?;
                        break;
                    }
                    let chunk = byte(cur)?;
                    if chunk >> 5 != 2 || chunk & 0x1f == 31 {
                        return Err(DecodeError::BadIndefiniteChunk);
                    }
                    let n = read_arg(cur, chunk & 0x1f)? as usize;
                    buf.extend_from_slice(take(cur, n)?);
                }
                Value::Bytes(buf)
            } else {
                let n = read_arg(cur, ai)? as usize;
                Value::Bytes(take(cur, n)?.to_vec())
            }
        }
        // Text string. Each chunk of an indefinite-length string is a text
        // string in its own right, so each is validated on its own.
        3 => {
            if ai == 31 {
                let mut text = String::new();
                loop {
                    if at_break(cur) {
                        byte(cur)?;
                        break;
                    }
                    let chunk = byte(cur)?;
                    if chunk >> 5 != 3 || chunk & 0x1f == 31 {
                        return Err(DecodeError::BadIndefiniteChunk);
                    }
                    let n = read_arg(cur, chunk & 0x1f)? as usize;
                    text.push_str(&read_string(cur, n)?);
                }
                Value::String(text)
            } else {
                let n = read_arg(cur, ai)? as usize;
                Value::String(read_string(cur, n)?)
            }
        }
        // Array.
        4 => {
            let mut out = Vec::new();
            if ai == 31 {
                loop {
                    if at_break(cur) {
                        byte(cur)?;
                        break;
                    }
                    out.push(read_one(cur, depth + 1)?);
                }
            } else {
                let n = read_arg(cur, ai)? as usize;
                // A length header is untrusted, so the capacity is capped
                // rather than trusted; a short input fails on the first read.
                out.reserve(n.min(1024));
                for _ in 0..n {
                    out.push(read_one(cur, depth + 1)?);
                }
            }
            Value::Array(out)
        }
        // Map.
        5 => {
            let mut out = Vec::new();
            if ai == 31 {
                loop {
                    if at_break(cur) {
                        byte(cur)?;
                        break;
                    }
                    out.push(read_pair(cur, depth)?);
                }
            } else {
                let n = read_arg(cur, ai)? as usize;
                out.reserve(n.min(1024));
                for _ in 0..n {
                    out.push(read_pair(cur, depth)?);
                }
            }
            Value::Object(out)
        }
        // Tags have no JavaScript counterpart.
        6 => return Err(DecodeError::Tag(read_arg(cur, ai)?)),
        // Simple values and floats.
        7 => match ai {
            20 => Value::Bool(false),
            21 => Value::Bool(true),
            22 => Value::Null,
            23 => Value::Undefined,
            // The two-byte simple form carries only 32–255, none of which is
            // assigned; 0–31 here is not well-formed (RFC 8949 §3.3).
            24 => match byte(cur)? {
                n @ 0..=31 => return Err(DecodeError::TwoByteSimple(n)),
                other => return Err(DecodeError::UnsupportedSimple(other)),
            },
            25 => Value::Number(half_to_f64(be(cur, 2)? as u16)),
            26 => Value::Number(f64::from(f32::from_bits(be(cur, 4)? as u32))),
            27 => Value::Number(f64::from_bits(be(cur, 8)?)),
            28..=30 => return Err(DecodeError::ReservedHeader),
            31 => return Err(DecodeError::UnexpectedBreak),
            // The unassigned simple values (0–19); `ai` is masked to five
            // bits, so the catch-all only ever sees those.
            _ => return Err(DecodeError::UnsupportedSimple(ai)),
        },
        _ => unreachable!("major type is three bits"),
    })
}

/// Reads one key/value pair of a map.
fn read_pair(cur: &mut &[u8], depth: usize) -> Result<(String, Value), DecodeError> {
    let key = crate::js_number::to_property_key(&read_one(cur, depth + 1)?);
    let value = read_one(cur, depth + 1)?;
    Ok((key, value))
}

// ---- half-precision floats -------------------------------------------------

/// An IEEE-754 binary16 (half) bit pattern as an `f64`.
fn half_to_f64(half: u16) -> f64 {
    let sign = (half >> 15) & 1;
    let exp = (half >> 10) & 0x1f;
    let frac = half & 0x3ff;
    let magnitude = if exp == 0 {
        // Zero or a subnormal: `frac * 2^-24`.
        f64::from(frac) * 2f64.powi(-24)
    } else if exp == 31 {
        if frac == 0 { f64::INFINITY } else { f64::NAN }
    } else {
        (1.0 + f64::from(frac) / 1024.0) * 2f64.powi(i32::from(exp) - 15)
    };
    if sign == 1 { -magnitude } else { magnitude }
}

/// The binary16 bit pattern of `v` when `v` is *exactly* representable as a
/// half, else `None`.
///
/// `v` must not be NaN (the caller handles it). This is an exactness test, not
/// a rounding: a value that is not a half is left for the single- or
/// double-precision branch rather than approximated, which is what makes the
/// encoder's float form canonical.
fn half_exact(v: f64) -> Option<u16> {
    let bits = v.to_bits();
    let sign = ((bits >> 63) & 1) as u16;
    let exp = ((bits >> 52) & 0x7ff) as i32;
    let mant = bits & 0x000f_ffff_ffff_ffff;

    if exp == 0x7ff {
        // An infinity fits a half; NaN is the caller's.
        return Some((sign << 15) | 0x7c00);
    }
    if exp == 0 {
        // Zero fits; an f64 subnormal is far below a half subnormal.
        return if mant == 0 { Some(sign << 15) } else { None };
    }

    let e = exp - 1023;
    if e > 15 {
        return None; // beyond the largest half
    }
    if e >= -14 {
        // A normal half carries ten mantissa bits, so the low 42 must be zero.
        if mant & ((1u64 << 42) - 1) != 0 {
            return None;
        }
        let frac = (mant >> 42) as u16;
        return Some((sign << 15) | (((e + 15) as u16) << 10) | frac);
    }
    if e < -24 {
        return None; // below the smallest half subnormal
    }
    // A subnormal half is `k * 2^-24`. `value = (2^52 + mant) * 2^(e - 52)`,
    // so `k = (2^52 + mant) * 2^(e - 28)`, which is an integer in `1..1024`
    // exactly when `2^52 + mant` is divisible by `2^(28 - e)`.
    let m = (1u64 << 52) | mant;
    let shift = (28 - e) as u32; // e in [-24, -15] → shift in [43, 52]
    if m & ((1u64 << shift) - 1) != 0 {
        return None;
    }
    let k = m >> shift;
    if k == 0 || k >= 1024 {
        return None;
    }
    Some((sign << 15) | (k as u16))
}

// ---- encoding --------------------------------------------------------------

/// Encodes a [`Value`] as CBOR in the deterministic (canonical) form.
pub(crate) fn write_value(v: &Value) -> Result<Vec<u8>, EncodeError> {
    let mut out = Vec::new();
    write_one(v, &mut out, 0)?;
    Ok(out)
}

/// Fuzz entry: decode arbitrary bytes and, when they decode, re-encode the
/// value and decode it again. Both walks are recursive over untrusted input, so
/// the target's value is that nothing panics and that the encoder can always
/// read back what the decoder produced.
#[cfg(any(test, feature = "fuzzing"))]
pub(crate) fn fuzz_round_trip(bytes: &[u8]) {
    let Ok(value) = read_value(bytes) else {
        return;
    };
    let encoded = write_value(&value).expect("a decoded value must re-encode");
    read_value(&encoded).expect("the encoder's own output must decode");
}

/// Writes an initial byte holding `major` and the shortest additional
/// information that carries `n` (RFC 8949 §4.2.1).
fn write_head(out: &mut Vec<u8>, major: u8, n: u64) {
    let base = major << 5;
    if n < 24 {
        out.push(base | n as u8);
    } else if n <= u64::from(u8::MAX) {
        out.push(base | 24);
        out.push(n as u8);
    } else if n <= u64::from(u16::MAX) {
        out.push(base | 25);
        out.extend_from_slice(&(n as u16).to_be_bytes());
    } else if n <= u64::from(u32::MAX) {
        out.push(base | 26);
        out.extend_from_slice(&(n as u32).to_be_bytes());
    } else {
        out.push(base | 27);
        out.extend_from_slice(&n.to_be_bytes());
    }
}

fn write_text(out: &mut Vec<u8>, s: &str) {
    write_head(out, 3, s.len() as u64);
    out.extend_from_slice(s.as_bytes());
}

fn write_one(v: &Value, out: &mut Vec<u8>, depth: usize) -> Result<(), EncodeError> {
    if depth > MAX_DEPTH {
        return Err(EncodeError(format!(
            "value nests deeper than {MAX_DEPTH} levels"
        )));
    }
    match v {
        // CBOR distinguishes `undefined` (0xf7) from `null` (0xf6), and both
        // exist in JavaScript, so both survive.
        Value::Undefined => out.push(0xf7),
        Value::Null => out.push(0xf6),
        Value::Bool(false) => out.push(0xf4),
        Value::Bool(true) => out.push(0xf5),
        Value::Number(n) => write_number(*n, out),
        // A `BigInt` has no CBOR type of its own; it is an integer while it fits
        // one of the two integer major types, and beyond that a bignum tag this
        // codec refuses.
        Value::BigInt(i) => write_bigint(*i, out)?,
        Value::String(s) => write_text(out, s),
        Value::Bytes(b) => {
            write_head(out, 2, b.len() as u64);
            out.extend_from_slice(b);
        }
        Value::Array(items) => {
            write_head(out, 4, items.len() as u64);
            for item in items {
                write_one(item, out, depth + 1)?;
            }
        }
        Value::Object(entries) => {
            // Canonical CBOR sorts map keys by the bytewise lexicographic order
            // of their deterministic encodings, which is neither the insertion
            // order nor the string order — the length prefix sorts first, so
            // `"z"` precedes `"aa"`. Each key is encoded once, then reused.
            let mut pairs: Vec<(Vec<u8>, &Value)> = Vec::with_capacity(entries.len());
            for (k, val) in entries {
                let mut key = Vec::new();
                write_text(&mut key, k);
                pairs.push((key, val));
            }
            pairs.sort_by(|a, b| a.0.cmp(&b.0));
            write_head(out, 5, pairs.len() as u64);
            for (key, val) in &pairs {
                out.extend_from_slice(key);
                write_one(val, out, depth + 1)?;
            }
        }
        // Anything the marshaller could not turn into a structured value — a
        // function, a symbol, a class instance with no enumerable own data.
        // Writing a value it cannot represent would be silent loss, so it is
        // refused at the call site instead.
        other => {
            return Err(EncodeError(format!(
                "cannot encode {} as CBOR",
                describe(other)
            )));
        }
    }
    Ok(())
}

fn write_number(n: f64, out: &mut Vec<u8>) {
    // The preferred NaN is the half-precision quiet NaN (RFC 8949 §4.2.1).
    if n.is_nan() {
        out.extend_from_slice(&[0xf9, 0x7e, 0x00]);
        return;
    }
    // An integer-valued number takes an integer form, the shortest that holds
    // it. Negative zero is excluded so its sign survives as a float.
    if n.fract() == 0.0 && !(n == 0.0 && n.is_sign_negative()) {
        // `i128` holds every integral `f64` either integer major type can
        // carry, so this cast is exact where a `u64`/`i64` cast would saturate.
        let i = n as i128;
        if i as f64 == n {
            if (0..=u64::MAX as i128).contains(&i) {
                write_head(out, 0, i as u64);
                return;
            }
            // Major type 1 carries `-1 - value`, so it reaches -2^64 — one past
            // `i64::MIN`, where the old `i64` cast gave up and wrote a float.
            if (-(u64::MAX as i128) - 1..0).contains(&i) {
                write_head(out, 1, (-(i + 1)) as u64);
                return;
            }
        }
    }
    // Otherwise the shortest float that preserves the value exactly.
    if let Some(half) = half_exact(n) {
        out.push(0xf9);
        out.extend_from_slice(&half.to_be_bytes());
    } else if f64::from(n as f32) == n {
        out.push(0xfa);
        out.extend_from_slice(&(n as f32).to_bits().to_be_bytes());
    } else {
        out.push(0xfb);
        out.extend_from_slice(&n.to_bits().to_be_bytes());
    }
}

/// Writes a `BigInt` as a CBOR integer, or refuses one outside the range the two
/// integer major types cover (`[-2^64, 2^64-1]`), which would need a bignum tag
/// this codec does not write.
fn write_bigint(i: i128, out: &mut Vec<u8>) -> Result<(), EncodeError> {
    if (0..=u64::MAX as i128).contains(&i) {
        write_head(out, 0, i as u64);
        Ok(())
    } else if (-(u64::MAX as i128) - 1..0).contains(&i) {
        // Major type 1 carries `-1 - value`, so it reaches -2^64.
        write_head(out, 1, (-(i + 1)) as u64);
        Ok(())
    } else {
        Err(EncodeError(
            "cannot encode a BigInt outside -2^64..2^64-1 as CBOR (a bignum tag is refused)".into(),
        ))
    }
}

fn describe(v: &Value) -> &'static str {
    match v {
        Value::Undefined => "undefined",
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::BigInt(_) => "a BigInt",
        Value::String(_) => "a string",
        Value::Bytes(_) => "bytes",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
        _ => "a value with no CBOR representation (a function, symbol, or similar)",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round(v: Value) -> Value {
        read_value(&write_value(&v).expect("encode")).expect("decode")
    }

    fn enc(v: Value) -> Vec<u8> {
        write_value(&v).expect("encode")
    }

    #[test]
    fn integers_use_the_shortest_form() {
        assert_eq!(enc(Value::Number(0.0)), vec![0x00]);
        assert_eq!(enc(Value::Number(10.0)), vec![0x0a]);
        assert_eq!(enc(Value::Number(23.0)), vec![0x17]);
        assert_eq!(enc(Value::Number(24.0)), vec![0x18, 0x18]);
        assert_eq!(enc(Value::Number(255.0)), vec![0x18, 0xff]);
        assert_eq!(enc(Value::Number(256.0)), vec![0x19, 0x01, 0x00]);
        assert_eq!(
            enc(Value::Number(65536.0)),
            vec![0x1a, 0x00, 0x01, 0x00, 0x00]
        );
        assert_eq!(enc(Value::Number(-1.0)), vec![0x20]);
        assert_eq!(enc(Value::Number(-24.0)), vec![0x37]);
        assert_eq!(enc(Value::Number(-25.0)), vec![0x38, 0x18]);
    }

    #[test]
    fn integers_round_trip_including_the_extremes() {
        for n in [
            0.0,
            1.0,
            23.0,
            24.0,
            255.0,
            256.0,
            65535.0,
            65536.0,
            4294967296.0,
            -1.0,
            -24.0,
            -25.0,
            -256.0,
            -257.0,
            -65536.0,
            -65537.0,
            -4294967296.0,
            -4294967297.0,
            // JavaScript's safe-integer bounds stay `number`.
            9_007_199_254_740_991.0,
            -9_007_199_254_740_991.0,
        ] {
            assert_eq!(round(Value::Number(n)), Value::Number(n), "{n}");
        }
    }

    #[test]
    fn integers_past_the_safe_range_round_trip_as_bigint() {
        // Past 2^53 (positive or negative) a CBOR integer is a `BigInt`, so it
        // survives exactly instead of rounding to a nearby `number`. Includes
        // `i64::MIN` (-2^63) and the format's own -2^64.
        for i in [
            9_007_199_254_740_992_i128, // 2^53
            i128::from(u64::MAX),
            -9_007_199_254_740_994,      // just past the negative bound
            -9_223_372_036_854_775_808,  // i64::MIN
            -18_446_744_073_709_551_616, // -2^64
        ] {
            assert_eq!(round(Value::BigInt(i)), Value::BigInt(i), "{i}");
        }
        // A `BigInt` inside the safe range has no BigInt form on the wire: it
        // encodes as an integer and decodes back as a `number`.
        assert_eq!(round(Value::BigInt(5)), Value::Number(5.0));
        // A `number` holding an exactly-representable integer past the safe
        // range encodes as an integer and decodes to the matching `BigInt`.
        assert_eq!(
            round(Value::Number(-18_446_744_073_709_551_616.0)),
            Value::BigInt(-18_446_744_073_709_551_616)
        );
    }

    #[test]
    fn a_bigint_outside_the_integer_major_types_is_refused() {
        // Beyond [-2^64, 2^64-1] CBOR would need a bignum tag, which this codec
        // refuses, so encoding says so rather than writing a float.
        let too_big = u64::MAX as i128 + 1;
        let err = write_value(&Value::BigInt(too_big)).expect_err("must refuse");
        assert!(err.0.contains("BigInt outside"), "{}", err.0);
        assert!(write_value(&Value::BigInt(-too_big - 1)).is_err());
    }

    #[test]
    fn negative_integers_keep_integer_form_down_to_the_cbor_floor() {
        // Major type 1 carries `-1 - value`, so it reaches -2^64, one past
        // `i64::MIN` — where the encoder used to give up and write a float.
        assert_eq!(
            enc(Value::Number(-18_446_744_073_709_551_616.0)),
            vec![0x3b, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]
        );
        assert_eq!(
            enc(Value::Number(-9_223_372_036_854_775_808.0)),
            vec![0x3b, 0x7f, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]
        );
        // The `f64` below -2^64 has no integer form and stays a float.
        let below = -(18_446_744_073_709_551_616.0 + 4096.0);
        assert_eq!(enc(Value::Number(below))[0], 0xfb);
    }

    #[test]
    fn floats_use_the_shortest_exact_width() {
        // RFC 8949 Appendix A: the canonical encoding of each value.
        let cases = [
            (1.5_f64, vec![0xf9, 0x3e, 0x00]),
            (0.5, vec![0xf9, 0x38, 0x00]),
            (5.960464477539063e-8, vec![0xf9, 0x00, 0x01]), // 2^-24
            (0.00006103515625, vec![0xf9, 0x04, 0x00]),     // 2^-14
            // Not exactly representable as a half *or* a single, so it keeps
            // double precision (RFC 8949 Appendix A).
            (-4.1, {
                let mut v = vec![0xfb];
                v.extend_from_slice(&(-4.1_f64).to_bits().to_be_bytes());
                v
            }),
            (3.4028234663852886e38, vec![0xfa, 0x7f, 0x7f, 0xff, 0xff]),
            (
                1.0e300,
                vec![0xfb, 0x7e, 0x37, 0xe4, 0x3c, 0x88, 0x00, 0x75, 0x9c],
            ),
        ];
        for (value, expected) in cases {
            assert_eq!(enc(Value::Number(value)), expected, "{value}");
        }
    }

    #[test]
    fn special_floats_take_their_canonical_form() {
        assert_eq!(enc(Value::Number(f64::INFINITY)), vec![0xf9, 0x7c, 0x00]);
        assert_eq!(
            enc(Value::Number(f64::NEG_INFINITY)),
            vec![0xf9, 0xfc, 0x00]
        );
        assert_eq!(enc(Value::Number(f64::NAN)), vec![0xf9, 0x7e, 0x00]);
        assert_eq!(enc(Value::Number(-0.0)), vec![0xf9, 0x80, 0x00]);
    }

    #[test]
    fn floats_round_trip() {
        for n in [
            1.5,
            -4.1,
            0.1,
            f64::INFINITY,
            f64::NEG_INFINITY,
            5.960464477539063e-8,
            1.0e300,
        ] {
            assert_eq!(round(Value::Number(n)), Value::Number(n), "{n}");
        }
        match round(Value::Number(f64::NAN)) {
            Value::Number(n) => assert!(n.is_nan()),
            other => panic!("expected a number, got {other:?}"),
        }
    }

    #[test]
    fn undefined_and_null_stay_distinct() {
        assert_eq!(enc(Value::Undefined), vec![0xf7]);
        assert_eq!(enc(Value::Null), vec![0xf6]);
        assert_eq!(round(Value::Undefined), Value::Undefined);
        assert_eq!(round(Value::Null), Value::Null);
    }

    #[test]
    fn strings_and_bytes_round_trip() {
        let text = Value::String("héllo".into());
        assert_eq!(round(text.clone()), text);
        // A four-character string is definite, not a one-byte length header.
        assert_eq!(
            enc(Value::String("IETF".into())),
            vec![0x64, b'I', b'E', b'T', b'F']
        );
        let bytes = Value::Bytes(vec![1, 2, 3]);
        assert_eq!(round(bytes.clone()), bytes);
    }

    #[test]
    fn indefinite_length_strings_decode() {
        // RFC 8949: (_ "strea", "ming") is a valid indefinite-length text.
        let b = [
            0x7f, 0x65, b's', b't', b'r', b'e', b'a', 0x64, b'm', b'i', b'n', b'g', 0xff,
        ];
        assert_eq!(
            read_value(&b).expect("decode"),
            Value::String("streaming".into())
        );
        // (_ h'0102', h'0304') for a bytes string.
        let b = [0x5f, 0x42, 0x01, 0x02, 0x42, 0x03, 0x04, 0xff];
        assert_eq!(
            read_value(&b).expect("decode"),
            Value::Bytes(vec![1, 2, 3, 4])
        );
    }

    #[test]
    fn indefinite_length_containers_decode() {
        // (_ 1, 2, 3) — an indefinite-length array.
        let b = [0x9f, 0x01, 0x02, 0x03, 0xff];
        assert_eq!(
            read_value(&b).expect("decode"),
            Value::Array(vec![
                Value::Number(1.0),
                Value::Number(2.0),
                Value::Number(3.0)
            ])
        );
        // {_ "a": 1} — an indefinite-length map.
        let b = [0xbf, 0x61, b'a', 0x01, 0xff];
        assert_eq!(
            read_value(&b).expect("decode"),
            Value::Object(vec![("a".into(), Value::Number(1.0))])
        );
    }

    #[test]
    fn map_keys_are_sorted_canonically() {
        // Insertion order "b", "a", "aa" — canonical order is by encoded bytes,
        // and the length prefix sorts first, so "a" < "b" < "aa".
        let v = Value::Object(vec![
            ("b".into(), Value::Number(1.0)),
            ("a".into(), Value::Number(2.0)),
            ("aa".into(), Value::Number(3.0)),
        ]);
        assert_eq!(
            enc(v),
            vec![
                0xa3, 0x61, b'a', 0x02, 0x61, b'b', 0x01, 0x62, b'a', b'a', 0x03
            ]
        );
    }

    #[test]
    fn maps_with_integer_keys_become_property_names() {
        assert_eq!(
            read_value(&[0xa1, 0x01, 0x02]).expect("decode"),
            Value::Object(vec![("1".into(), Value::Number(2.0))])
        );
    }

    #[test]
    fn negative_integers_just_past_2_53_decode_exactly() {
        // Major type 1 with argument 2^53 + 1 encodes -9007199254740994, past
        // JavaScript's safe range, so it decodes to a `BigInt` — exactly.
        let bytes = [0x3b, 0x00, 0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01];
        assert_eq!(
            read_value(&bytes).expect("decode"),
            Value::BigInt(-9_007_199_254_740_994)
        );
        // The integer form survives a round trip.
        assert_eq!(enc(Value::BigInt(-9_007_199_254_740_994)), bytes.to_vec());
    }

    #[test]
    fn large_numeric_map_keys_do_not_saturate() {
        // A uint64 key past `i64::MAX` (9.22e18) is past the safe range, so it
        // decodes to a `BigInt` and spells its exact digits. A saturating cast
        // would have produced "9223372036854775807", and a `number` path would
        // have rounded it to "18446744073709552000".
        assert_eq!(
            read_value(&[
                0xa1, 0x1b, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x02
            ])
            .expect("decode"),
            Value::Object(vec![("18446744073709551615".into(), Value::Number(2.0))])
        );
        // A float64 key of 1e20, past `i64::MAX` but below the exponential
        // threshold, so it is written out in full.
        assert_eq!(
            read_value(&[
                0xa1, 0xfb, 0x44, 0x15, 0xaf, 0x1d, 0x78, 0xb5, 0x8c, 0x40, 0x02
            ])
            .expect("decode"),
            Value::Object(vec![("100000000000000000000".into(), Value::Number(2.0))])
        );
        // A float64 key at 1e21, where `String(n)` switches to exponential.
        assert_eq!(
            read_value(&[
                0xa1, 0xfb, 0x44, 0x4b, 0x1a, 0xe4, 0xd6, 0xe2, 0xef, 0x50, 0x02
            ])
            .expect("decode"),
            Value::Object(vec![("1e+21".into(), Value::Number(2.0))])
        );
    }

    #[test]
    fn non_string_map_keys_are_coerced_like_a_property_access() {
        // An array key spells as its elements joined by commas.
        assert_eq!(
            read_value(&[0xa1, 0x82, 0x01, 0x02, 0x03]).expect("decode"),
            Value::Object(vec![("1,2".into(), Value::Number(3.0))])
        );
        // A byte string key is a `Uint8Array`, whose `toString` joins bytes.
        assert_eq!(
            read_value(&[0xa1, 0x42, 0x01, 0x02, 0x03]).expect("decode"),
            Value::Object(vec![("1,2".into(), Value::Number(3.0))])
        );
        // `null` inside an array key is the empty string, not "null".
        assert_eq!(
            read_value(&[0xa1, 0x83, 0x01, 0xf6, 0x02, 0x03]).expect("decode"),
            Value::Object(vec![("1,,2".into(), Value::Number(3.0))])
        );
        // A nested map key is `[object Object]`, not the old `[object]`.
        assert_eq!(
            read_value(&[0xa1, 0xa1, 0x61, 0x61, 0x01, 0x03]).expect("decode"),
            Value::Object(vec![("[object Object]".into(), Value::Number(3.0))])
        );
    }

    #[test]
    fn half_precision_decodes_to_the_exact_value() {
        assert_eq!(
            read_value(&[0xf9, 0x3c, 0x00]).expect("decode"),
            Value::Number(1.0)
        );
        assert_eq!(
            read_value(&[0xf9, 0x00, 0x01]).expect("decode"),
            Value::Number(5.960464477539063e-8)
        );
        assert_eq!(
            read_value(&[0xf9, 0x7c, 0x00]).expect("decode"),
            Value::Number(f64::INFINITY)
        );
    }

    #[test]
    fn tags_are_refused_by_name() {
        // Tag 0 (a date/time string) is well-formed CBOR but unsupported: no JS
        // type corresponds to a tag.
        let err = read_value(&[0xc0, 0x61, b'a']).expect_err("must refuse");
        assert_eq!(err, DecodeError::Tag(0));
        assert!(format!("{err}").contains("tag 0"));
        // Tag 55799 (the self-described CBOR marker).
        assert_eq!(
            read_value(&[0xd9, 0xd9, 0xf7, 0x00]),
            Err(DecodeError::Tag(55799))
        );
    }

    #[test]
    fn unassigned_simple_values_are_refused() {
        assert_eq!(read_value(&[0xf0]), Err(DecodeError::UnsupportedSimple(16)));
        assert_eq!(
            read_value(&[0xf8, 0xff]),
            Err(DecodeError::UnsupportedSimple(255))
        );
    }

    #[test]
    fn a_simple_value_below_32_in_the_two_byte_form_is_not_well_formed() {
        // RFC 8949 §3.3: `0xf8` followed by a byte under 0x20 is not
        // well-formed, so `false` cannot sneak in under a second spelling.
        for n in [0u8, 20, 21, 22, 23, 31] {
            assert_eq!(
                read_value(&[0xf8, n]),
                Err(DecodeError::TwoByteSimple(n)),
                "{n}"
            );
            assert_eq!(validate(&[0xf8, n]), Err(DecodeError::TwoByteSimple(n)));
        }
        // 32 is the first value the two-byte form may carry; it is unassigned.
        assert_eq!(
            read_value(&[0xf8, 0x20]),
            Err(DecodeError::UnsupportedSimple(32))
        );
    }

    #[test]
    fn malformed_input_is_an_error_not_a_panic() {
        assert_eq!(read_value(&[]), Err(DecodeError::Truncated));
        assert_eq!(read_value(&[0x18]), Err(DecodeError::Truncated));
        assert_eq!(read_value(&[0x1c]), Err(DecodeError::ReservedHeader));
        // A break outside an indefinite-length item.
        assert_eq!(read_value(&[0xff]), Err(DecodeError::UnexpectedBreak));
        // Trailing bytes after a complete value.
        assert_eq!(read_value(&[0x00, 0x00]), Err(DecodeError::TrailingData));
        // A text string whose payload is not UTF-8.
        assert_eq!(read_value(&[0x61, 0xff]), Err(DecodeError::InvalidUtf8));
        // An indefinite-length string whose chunk is the wrong major type.
        assert_eq!(
            read_value(&[0x7f, 0x41, 0x01, 0xff]),
            Err(DecodeError::BadIndefiniteChunk)
        );
        // A length header promising more than the input holds.
        assert_eq!(
            read_value(&[0x5b, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]),
            Err(DecodeError::Truncated)
        );
    }

    #[test]
    fn validate_agrees_with_decode() {
        // Well-formed and malformed inputs alike: `validate` must return the
        // same result as a decode, since it is the same syntax without the tree.
        let inputs: &[&[u8]] = &[
            &[0x00],
            &[0x1b, 0x00, 0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01],
            &[0xa1, 0x01, 0x02],
            &[0x7f, 0x62, 0x68, 0x69, 0xff],
            &[0x9f, 0x01, 0x02, 0xff],
            &[0xa1, 0xa1, 0x61, 0x61, 0x01, 0x03],
            &[],
            &[0xff],
            &[0xc0, 0x61, b'a'],
            &[0x00, 0x00],
            &[0x61, 0xff],
            &[0x7f, 0x41, 0x01, 0xff],
            &[0x1c],
            &[0xf0],
            &[0xf8, 0x14],
        ];
        for input in inputs {
            assert_eq!(validate(input), read_value(input).map(|_| ()), "{input:?}");
        }
        let bomb = vec![0x9fu8; MAX_DEPTH + 8];
        assert_eq!(validate(&bomb), read_value(&bomb).map(|_| ()));
    }

    #[test]
    fn deep_nesting_is_refused_rather_than_overflowing_the_stack() {
        // 0x9f is an indefinite-length array: a chain of them nests without
        // bound.
        let bomb = vec![0x9fu8; MAX_DEPTH + 8];
        assert_eq!(read_value(&bomb), Err(DecodeError::TooDeep));
    }

    #[test]
    fn a_value_with_no_representation_is_refused_rather_than_lost() {
        let err = write_value(&Value::Other("function () {}".into())).expect_err("must refuse");
        assert!(err.0.contains("cannot encode"), "{}", err.0);
    }

    #[test]
    fn canonical_output_is_stable_across_key_order() {
        let a = Value::Object(vec![
            ("x".into(), Value::Number(1.0)),
            ("t".into(), Value::Number(2.0)),
        ]);
        let b = Value::Object(vec![
            ("t".into(), Value::Number(2.0)),
            ("x".into(), Value::Number(1.0)),
        ]);
        assert_eq!(enc(a), enc(b));
    }

    #[test]
    fn fuzz_entry_does_not_panic() {
        let inputs: &[&[u8]] = &[
            &[],
            &[0xff],
            &[0xc0, 0x61, 0x61],
            &[0x9f, 0x01, 0xff],
            &[0x7f, 0x62, 0x68, 0x69, 0xff],
            &[0xfb, 0x40, 0x09, 0x21, 0xfb, 0x54, 0x44, 0x2d, 0x18],
        ];
        for input in inputs {
            fuzz_round_trip(input);
        }
    }
}
