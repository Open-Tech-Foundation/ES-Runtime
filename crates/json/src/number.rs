//! Hex integer literals as the nearest double, as ECMAScript reads them.

/// The `f64` nearest to the hex digits' value, ties to even, `Infinity` past
/// the largest double. Any number of digits: the first 64 significant bits
/// are kept and the rest only say whether anything below them is non-zero.
pub(crate) fn hex_to_f64(digits: &[u8]) -> f64 {
    let mut kept: u64 = 0;
    // Bits dropped below `kept`, and whether any of them was set.
    let mut dropped: i32 = 0;
    let mut sticky = false;
    for &digit in digits {
        let value = u64::from(char::from(digit).to_digit(16).expect("a hex digit"));
        if kept >> 60 == 0 {
            kept = (kept << 4) | value;
        } else {
            sticky |= value != 0;
            dropped = dropped.saturating_add(4);
        }
    }
    if kept == 0 {
        return 0.0;
    }
    let shift = kept.leading_zeros();
    let normalized = kept << shift;
    // 53 significant bits, rounded on the 11 below them and the sticky bit.
    let mut mantissa = normalized >> 11;
    let rest = normalized & 0x7ff;
    if rest > 0x400 || (rest == 0x400 && (sticky || mantissa & 1 == 1)) {
        mantissa += 1;
    }
    // Scaling by a power of two is exact until it overflows to infinity. The
    // exponent is negative for a value below 2^53, whose low bits are zero.
    let mut exponent = dropped - shift as i32 + 11;
    let mut value = mantissa as f64;
    while exponent != 0 && value.is_finite() {
        let step = exponent.clamp(-1000, 1000);
        value *= 2f64.powi(step);
        exponent -= step;
    }
    value
}

#[cfg(test)]
mod tests {
    use super::hex_to_f64;

    #[test]
    fn exact_below_two_to_the_fifty_three() {
        assert_eq!(hex_to_f64(b"0"), 0.0);
        assert_eq!(hex_to_f64(b"ff"), 255.0);
        assert_eq!(hex_to_f64(b"1FFFFFFFFFFFFF"), 9_007_199_254_740_991.0);
        assert_eq!(hex_to_f64(b"0000000000000000000000001"), 1.0);
    }

    #[test]
    fn rounds_to_nearest_ties_to_even() {
        // 2^53 + 1 is a tie between 2^53 and 2^53 + 2: even is 2^53.
        assert_eq!(hex_to_f64(b"20000000000001"), 9_007_199_254_740_992.0);
        // 2^53 + 3 ties between + 2 and + 4: even is + 4.
        assert_eq!(hex_to_f64(b"20000000000003"), 9_007_199_254_740_996.0);
        // Just above the tie, from a bit far below the kept 64: rounds up.
        let mut digits = b"20000000000001".to_vec();
        digits.extend_from_slice(&[b'0'; 30]);
        digits.push(b'1');
        assert_eq!(hex_to_f64(&digits), (2f64.powi(53) + 2.0) * 2f64.powi(124));
        // u64::MAX rounds up to 2^64.
        assert_eq!(hex_to_f64(b"FFFFFFFFFFFFFFFF"), 2f64.powi(64));
    }

    #[test]
    fn overflows_to_infinity() {
        // f64::MAX is (2^53 - 1) * 2^971: 53 ones, shifted 3, then 242 hex zeros.
        let largest = format!("{:x}", ((1u64 << 53) - 1) << 3) + &"0".repeat(242);
        assert_eq!(hex_to_f64(largest.as_bytes()), f64::MAX);
        assert_eq!(hex_to_f64(&[b'f'; 300]), f64::INFINITY);
    }
}
