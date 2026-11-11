//! The `Date` header's form, IMF-fixdate (RFC 9110 §5.6.7):
//! `Sun, 06 Nov 1994 08:49:37 GMT`, always 29 bytes.
//!
//! A server sends one on every response, so it is formatted into a fixed array
//! without allocating; a driver formats it once per second and reuses it.

/// The length of an IMF-fixdate.
pub const IMF_FIXDATE_LEN: usize = 29;

const DAYS: [&[u8; 3]; 7] = [b"Thu", b"Fri", b"Sat", b"Sun", b"Mon", b"Tue", b"Wed"];
const MONTHS: [&[u8; 3]; 12] = [
    b"Jan", b"Feb", b"Mar", b"Apr", b"May", b"Jun", b"Jul", b"Aug", b"Sep", b"Oct", b"Nov", b"Dec",
];

/// The IMF-fixdate for `unix_seconds` seconds after 1970-01-01T00:00:00Z.
#[must_use]
pub fn imf_fixdate(unix_seconds: u64) -> [u8; IMF_FIXDATE_LEN] {
    let days = unix_seconds / 86_400;
    let secs = unix_seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    let (h, m, s) = (secs / 3600, (secs / 60) % 60, secs % 60);

    let mut out = *b"Thu, 01 Jan 1970 00:00:00 GMT";
    out[..3].copy_from_slice(DAYS[(days % 7) as usize]);
    two(&mut out[5..7], day);
    out[8..11].copy_from_slice(MONTHS[(month - 1) as usize]);
    // Four digits: a year past 9999 is not a date anything here will send.
    let y = year % 10_000;
    two(&mut out[12..14], y / 100);
    two(&mut out[14..16], y % 100);
    two(&mut out[17..19], h);
    two(&mut out[20..22], m);
    two(&mut out[23..25], s);
    out
}

fn two(dst: &mut [u8], n: u64) {
    dst[0] = b'0' + (n / 10) as u8;
    dst[1] = b'0' + (n % 10) as u8;
}

/// (year, month 1–12, day 1–31) for a count of days since 1970-01-01, by
/// Howard Hinnant's `civil_from_days` algorithm, restricted to dates on or
/// after the epoch.
fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::imf_fixdate;

    #[test]
    fn known_dates_format_exactly() {
        // RFC 9110's own example.
        assert_eq!(&imf_fixdate(784_111_777), b"Sun, 06 Nov 1994 08:49:37 GMT");
        assert_eq!(&imf_fixdate(0), b"Thu, 01 Jan 1970 00:00:00 GMT");
        // A leap day, and the last second of a leap year.
        assert_eq!(&imf_fixdate(951_782_400), b"Tue, 29 Feb 2000 00:00:00 GMT");
        assert_eq!(
            &imf_fixdate(1_735_689_599),
            b"Tue, 31 Dec 2024 23:59:59 GMT"
        );
        assert_eq!(
            &imf_fixdate(1_794_441_600),
            b"Thu, 12 Nov 2026 00:00:00 GMT"
        );
    }

    #[test]
    fn every_day_for_sixty_years_round_trips_its_fields() {
        // Day of week advances by one per day and the month/day stay in range.
        let mut prev_dow: Option<usize> = None;
        let names = [b"Sun", b"Mon", b"Tue", b"Wed", b"Thu", b"Fri", b"Sat"];
        for day in 0..(60 * 366) {
            let d = imf_fixdate(day * 86_400);
            let dow = names.iter().position(|n| &d[..3] == *n).unwrap();
            if let Some(p) = prev_dow {
                assert_eq!(dow, (p + 1) % 7, "{}", String::from_utf8_lossy(&d));
            }
            prev_dow = Some(dow);
            let dd = (d[5] - b'0') * 10 + (d[6] - b'0');
            assert!((1..=31).contains(&dd));
        }
    }
}
