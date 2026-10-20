//! JavaScript-compatible number formatting, and the property-key coercion
//! built on it.
//!
//! A numeric map key in a binary format has to become the property name the
//! guest would have written, and the guest spells numbers as `String(n)` — the
//! ECMAScript `Number::toString` algorithm (ECMA-262 §6.1.6.1.20). Neither of
//! Rust's float formatters matches it: `{}` never uses exponential notation,
//! `{:e}` always does, and the spec switches at its own thresholds (magnitude
//! `>= 1e21` or `< 1e-6`). The quick approximation is wrong for the values in
//! between: casting an integral `f64` to `i64` saturates at `i64::MAX`, which
//! `1e20` overshoots, so that key would spell itself `"9223372036854775807"`
//! and distinct large keys could collapse onto one property name.

use es_runtime_engine::Value;

/// Formats `n` exactly as the guest's `String(n)` would, byte for byte.
pub(crate) fn to_js_string(n: f64) -> String {
    if n == 0.0 {
        // Also covers `-0.0`, which `String(-0)` spells `"0"`.
        return "0".to_string();
    }
    if n.is_nan() {
        return "NaN".to_string();
    }
    if n.is_infinite() {
        return if n > 0.0 {
            "Infinity".to_string()
        } else {
            "-Infinity".to_string()
        };
    }

    let negative = n < 0.0;
    // `{:e}` yields the shortest decimal that round-trips, in scientific form
    // (`1.5e0`, `1e20`): the significant digits and exponent the spec's
    // algorithm is built around.
    let scientific = format!("{:e}", n.abs());
    let (mantissa, exponent) = scientific.split_once('e').expect("LowerExp writes an e");
    let exponent: i32 = exponent
        .parse()
        .expect("LowerExp writes an integer exponent");
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let k = digits.len() as i32; // significant digits (the spec's k)
    let point = exponent + 1; // decimal-point position (the spec's n)

    let mut out = if k <= point && point <= 21 {
        // Every digit sits left of the point; pad the integer part with zeros.
        let mut s = digits;
        s.push_str(&"0".repeat((point - k) as usize));
        s
    } else if 0 < point && point <= 21 {
        // The point falls inside the digits.
        format!(
            "{}.{}",
            &digits[..point as usize],
            &digits[point as usize..]
        )
    } else if -6 < point && point <= 0 {
        // `0.` followed by the zeros before the first significant digit.
        format!("0.{}{}", "0".repeat(-point as usize), digits)
    } else {
        // Exponential form: one integer digit, an optional fraction, and an
        // explicitly signed exponent (`1e+21`, `1e-7`).
        let mut s = String::from(&digits[..1]);
        if k > 1 {
            s.push('.');
            s.push_str(&digits[1..]);
        }
        s.push('e');
        s.push(if point >= 1 { '+' } else { '-' });
        s.push_str(&(point - 1).abs().to_string());
        s
    };
    if negative {
        out.insert(0, '-');
    }
    out
}

/// Coerces a decoded map key to the property name a JavaScript access would
/// use. Shared by CBOR and MessagePack so the two spell the same key the same
/// way.
///
/// Both formats allow *any* item as a key, but a JavaScript object's keys are strings,
/// so a non-string key is coerced exactly as `String(key)` would (and a property
/// access coerces its key the same way): numbers by `String(n)` (see
/// [`to_js_string`]), booleans and `null`/`undefined` by their literals,
/// arrays and byte strings as their elements joined with commas, and a nested
/// map as `"[object Object]"`. The coercions can collide (`[1, 2]` and
/// `["1", "2"]` share a name) — that is JavaScript's behaviour, not a choice
/// here.
///
/// A nested map is the one deliberate simplification: it always spells
/// `"[object Object]"`. `String(plainObject)` is the same, but a nested map that
/// carries its own `toString` is not — `String` would call it, and throw on a
/// non-callable one — and running a decoded value's accessors during a structural
/// walk is not something a codec should do. The spelling here is fixed instead.
pub(crate) fn to_property_key(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => to_js_string(*n),
        // `String(bigint)` is its exact decimal digits, with no exponent form.
        Value::BigInt(i) => i.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "null".to_string(),
        Value::Undefined => "undefined".to_string(),
        // `Uint8Array`'s `toString` is `Array.prototype.join`, so it spells as
        // its bytes, not as text.
        Value::Bytes(bytes) => join_elements(bytes.iter().map(|b| b.to_string())),
        Value::Array(items) => join_elements(items.iter().map(element_to_string)),
        Value::Object(_) => "[object Object]".to_string(),
        // Never produced by the decoder, but `Other` already carries the
        // guest's own `String(value)`.
        Value::Other(s) => s.clone(),
        // `Value` is `#[non_exhaustive]`; a future variant has no defined
        // string form here.
        _ => "[object Object]".to_string(),
    }
}

/// One element of an array key: `Array.prototype.join` spells `null` and
/// `undefined` as the empty string and every other value by `String`.
fn element_to_string(v: &Value) -> String {
    match v {
        Value::Null | Value::Undefined => String::new(),
        other => to_property_key(other),
    }
}

fn join_elements(parts: impl Iterator<Item = String>) -> String {
    let mut out = String::new();
    for (i, part) in parts.enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&part);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::to_js_string;

    /// Every expected string here was checked against the guest's own
    /// `String(n)`, including the boundary where the spec switches to
    /// exponential notation.
    #[test]
    fn matches_number_to_string() {
        let cases: &[(f64, &str)] = &[
            (0.0, "0"),
            (-0.0, "0"),
            (1.0, "1"),
            (-1.0, "-1"),
            (1.5, "1.5"),
            (100_000.0, "100000"),
            (123.456, "123.456"),
            (0.1, "0.1"),
            (0.30000000000000004, "0.30000000000000004"),
            (9_007_199_254_740_991.0, "9007199254740991"),
            // The range the `as i64` shortcut got wrong: past `i64::MAX`
            // (9.22e18) but still below the 1e21 exponential threshold.
            (1e20, "100000000000000000000"),
            (1.2345678901234568e20, "123456789012345680000"),
            (18_446_744_073_709_551_616.0, "18446744073709552000"),
            // The exponential thresholds: `>= 1e21` and `< 1e-6`.
            (1e21, "1e+21"),
            (-1e21, "-1e+21"),
            (1e22, "1e+22"),
            (1e100, "1e+100"),
            (1e-6, "0.000001"),
            (1e-7, "1e-7"),
            (5e-324, "5e-324"),
            (1.7976931348623157e308, "1.7976931348623157e+308"),
            (f64::NAN, "NaN"),
            (f64::INFINITY, "Infinity"),
            (f64::NEG_INFINITY, "-Infinity"),
        ];
        for (input, expected) in cases {
            assert_eq!(to_js_string(*input), *expected, "String({input})");
        }
    }

    /// Two distinct keys must never spell themselves the same name.
    #[test]
    fn distinct_values_keep_distinct_names() {
        assert_ne!(
            to_js_string(1e20),
            to_js_string(18_446_744_073_709_551_616.0)
        );
        assert_ne!(to_js_string(1e21), to_js_string(1.2e21));
    }
}
