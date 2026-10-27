//! Decimal128 to and from text, as the BSON Decimal128 specification defines
//! it over IEEE 754-2008's binary integer decimal encoding.
//!
//! Text is parsed **exactly or not at all**: a value that would need rounding to
//! fit 34 digits or the exponent range is an error, never a silent change.
//! Trailing zeros may move into the exponent, and a zero's exponent is clamped,
//! because neither changes the value.

use crate::{Error, Result};

const MAX_DIGITS: usize = 34;
const EXPONENT_MIN: i64 = -6176;
const EXPONENT_MAX: i64 = 6111;
const EXPONENT_BIAS: i64 = 6176;
/// 10^34 - 1, the largest canonical coefficient.
const COEFFICIENT_MAX: u128 = 9_999_999_999_999_999_999_999_999_999_999_999;

const SIGN: u64 = 1 << 63;
const INFINITY: u64 = 0x7800_0000_0000_0000;
const NAN: u64 = 0x7C00_0000_0000_0000;

/// The canonical text of a Decimal128, given its 16 bytes as stored
/// (little-endian). A non-canonical encoding reads as zero, as the
/// specification requires.
pub fn decimal128_to_string(bytes: [u8; 16]) -> String {
    let (low, high) = split(bytes);
    let negative = high & SIGN != 0;
    let combination = (high >> 58) & 0x1f;

    let (biased, coefficient) = if combination >> 3 == 0b11 {
        match combination {
            0b11110 => return if negative { "-Infinity" } else { "Infinity" }.to_string(),
            0b11111 => return "NaN".to_string(),
            // The large form encodes a coefficient of at least 2^113, past the
            // largest canonical one.
            _ => ((high >> 47) & 0x3fff, 0),
        }
    } else {
        let coefficient = (u128::from(high & 0x0001_ffff_ffff_ffff) << 64) | u128::from(low);
        let coefficient = if coefficient > COEFFICIENT_MAX {
            0
        } else {
            coefficient
        };
        ((high >> 49) & 0x3fff, coefficient)
    };

    // 14 bits, so the cast is exact.
    #[expect(clippy::cast_possible_wrap, reason = "a 14-bit field")]
    let exponent = biased as i64 - EXPONENT_BIAS;
    let mut out = String::new();
    if negative {
        out.push('-');
    }
    format_finite(&mut out, &coefficient.to_string(), exponent);
    out
}

/// The IEEE to-scientific-string rule: plain notation when the exponent is not
/// positive and the adjusted exponent is at least -6, scientific otherwise.
fn format_finite(out: &mut String, digits: &str, exponent: i64) {
    let len = i64::try_from(digits.len()).unwrap_or(i64::MAX);
    let adjusted = exponent + len - 1;
    if exponent <= 0 && adjusted >= -6 {
        if exponent == 0 {
            out.push_str(digits);
            return;
        }
        let point = len + exponent;
        if point > 0 {
            let point = usize::try_from(point).unwrap_or(0);
            out.push_str(&digits[..point]);
            out.push('.');
            out.push_str(&digits[point..]);
        } else {
            out.push_str("0.");
            for _ in 0..-point {
                out.push('0');
            }
            out.push_str(digits);
        }
        return;
    }
    out.push_str(&digits[..1]);
    if digits.len() > 1 {
        out.push('.');
        out.push_str(&digits[1..]);
    }
    out.push('E');
    if adjusted >= 0 {
        out.push('+');
    }
    out.push_str(&adjusted.to_string());
}

/// Parses decimal text into the 16 bytes a Decimal128 is stored as.
///
/// Accepts an optional sign, digits with at most one decimal point, and an
/// optional exponent (`1.5E+3`), or `Infinity`, `Inf` and `NaN` in any case.
/// Anything that cannot be represented exactly is an error.
pub fn decimal128_from_str(text: &str) -> Result<[u8; 16]> {
    let invalid = || Error::new(format!("{text:?} is not a decimal number"));
    let (negative, rest) = match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    if rest.eq_ignore_ascii_case("inf") || rest.eq_ignore_ascii_case("infinity") {
        return Ok(join(0, INFINITY | if negative { SIGN } else { 0 }));
    }
    if rest.eq_ignore_ascii_case("nan") {
        return Ok(join(0, NAN));
    }

    let (mantissa, exponent_text) = match rest.find(['e', 'E']) {
        Some(at) => (&rest[..at], Some(&rest[at + 1..])),
        None => (rest, None),
    };
    let (whole, fraction) = match mantissa.find('.') {
        Some(at) => (&mantissa[..at], &mantissa[at + 1..]),
        None => (mantissa, ""),
    };
    let all_digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    if (whole.is_empty() && fraction.is_empty()) || !all_digits(whole) || !all_digits(fraction) {
        return Err(invalid());
    }
    let mut exponent = match exponent_text {
        Some(t) => parse_exponent(t).ok_or_else(invalid)?,
        None => 0,
    };
    exponent -= i64::try_from(fraction.len()).map_err(|_| invalid())?;

    // The significant digits, leading zeros removed.
    let mut digits: Vec<u8> = whole
        .bytes()
        .chain(fraction.bytes())
        .skip_while(|&b| b == b'0')
        .collect();

    if digits.is_empty() {
        let exponent = exponent.clamp(EXPONENT_MIN, EXPONENT_MAX);
        return Ok(encode(negative, 0, exponent));
    }

    let inexact = || {
        Error::new(format!(
            "{text:?} cannot be represented exactly as a Decimal128"
        ))
    };
    // Too many digits: only trailing zeros may move into the exponent.
    while digits.len() > MAX_DIGITS {
        if digits.last() != Some(&b'0') {
            return Err(inexact());
        }
        digits.pop();
        exponent += 1;
    }
    // Too large an exponent: pad the coefficient with zeros while it has room.
    while exponent > EXPONENT_MAX {
        if digits.len() >= MAX_DIGITS {
            return Err(Error::new(format!(
                "{text:?} is too large for a Decimal128"
            )));
        }
        digits.push(b'0');
        exponent -= 1;
    }
    // Too small an exponent: only trailing zeros may be dropped.
    while exponent < EXPONENT_MIN {
        if digits.last() != Some(&b'0') {
            return Err(inexact());
        }
        digits.pop();
        exponent += 1;
    }

    let coefficient = digits
        .iter()
        .fold(0u128, |acc, &d| acc * 10 + u128::from(d - b'0'));
    Ok(encode(negative, coefficient, exponent))
}

/// An exponent's digits, with an optional sign. Saturates far beyond any
/// representable exponent rather than overflowing, so `1E+99999999999` reaches
/// the range checks instead of wrapping.
fn parse_exponent(text: &str) -> Option<i64> {
    let (negative, digits) = match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let magnitude = digits.bytes().fold(0i64, |acc, d| {
        acc.saturating_mul(10)
            .saturating_add(i64::from(d - b'0'))
            .min(1 << 40)
    });
    Some(if negative { -magnitude } else { magnitude })
}

fn encode(negative: bool, coefficient: u128, exponent: i64) -> [u8; 16] {
    // The callers keep the exponent in range, so the biased value is 14 bits.
    #[expect(
        clippy::cast_sign_loss,
        reason = "the exponent is in range, so this is non-negative"
    )]
    let biased = (exponent + EXPONENT_BIAS) as u64;
    #[expect(clippy::cast_possible_truncation, reason = "taking the low 64 bits")]
    let low = coefficient as u64;
    // A coefficient is below 2^113, so its high half is below 2^49.
    let high_coefficient = (coefficient >> 64) as u64;
    let mut high = (biased << 49) | high_coefficient;
    if negative {
        high |= SIGN;
    }
    join(low, high)
}

fn split(bytes: [u8; 16]) -> (u64, u64) {
    let mut low = [0u8; 8];
    let mut high = [0u8; 8];
    low.copy_from_slice(&bytes[..8]);
    high.copy_from_slice(&bytes[8..]);
    (u64::from_le_bytes(low), u64::from_le_bytes(high))
}

fn join(low: u64, high: u64) -> [u8; 16] {
    let mut out = [0u8; 16];
    out[..8].copy_from_slice(&low.to_le_bytes());
    out[8..].copy_from_slice(&high.to_le_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round(text: &str) -> String {
        decimal128_to_string(decimal128_from_str(text).unwrap())
    }

    #[test]
    fn canonical_text_round_trips() {
        for text in [
            "0",
            "-0",
            "1",
            "-1.5",
            "0.001",
            "1.000",
            "1E+3",
            "1.23E-7",
            "0E+6111",
            "0E-6176",
            "Infinity",
            "-Infinity",
            "NaN",
            "9.999999999999999999999999999999999E+6144",
            "1E-6176",
        ] {
            assert_eq!(round(text), text);
        }
    }

    #[test]
    fn spellings_normalize() {
        assert_eq!(round("+inf"), "Infinity");
        assert_eq!(round("-NaN"), "NaN");
        assert_eq!(round(".5"), "0.5");
        assert_eq!(round("0.0000005"), "5E-7");
        assert_eq!(round("1e09"), "1E+9");
        assert_eq!(round("0E+2147483647"), "0E+6111");
        // Trailing zeros move into the exponent rather than rounding.
        assert_eq!(
            round("1.1111111111111111111111111111123450"),
            "1.111111111111111111111111111112345"
        );
    }

    #[test]
    fn inexact_and_malformed_text_is_refused() {
        for text in [
            "",
            ".",
            "1e",
            "E01",
            "1..3",
            "+-1",
            " 1",
            "1 ",
            "Infi",
            "NaNq",
            "1.23abc",
            "1.11111111111111111111111111111234549",
            "1E-6177",
            "7e10000",
        ] {
            assert!(decimal128_from_str(text).is_err(), "{text:?} parsed");
        }
    }
}
