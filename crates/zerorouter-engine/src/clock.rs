//! Wall-clock formatting without a date crate.

use std::time::{SystemTime, UNIX_EPOCH};

/// `t` as RFC 3339 UTC with seconds, e.g. `2026-09-27T15:00:00Z`.
pub fn rfc3339(t: SystemTime) -> String {
    let secs = t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (days, rem) = (secs / 86_400, secs % 86_400);
    let (y, m, d) = civil_from_days(days as i64);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

pub fn now_rfc3339() -> String {
    rfc3339(SystemTime::now())
}

/// An RFC 3339 time (`2026-09-27T15:00:00Z`, fractions and `±hh:mm` offsets allowed).
pub fn parse_rfc3339(s: &str) -> Option<SystemTime> {
    let b = s.as_bytes();
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b't' | b' ') || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let (y, mo, d, h, mi, sec) = (num(0..4)?, num(5..7)?, num(8..10)?, num(11..13)?, num(14..16)?, num(17..19)?);
    let mut i = 19;
    let mut nanos = 0u32;
    if b.get(i) == Some(&b'.') {
        let start = i + 1;
        i = start;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        let frac = s.get(start..i.min(start + 9))?;
        nanos = format!("{frac:0<9}").parse().ok()?;
    }
    let offset = match b.get(i)? {
        b'Z' | b'z' if i + 1 == b.len() => 0,
        c @ (b'+' | b'-') if b.len() == i + 6 && b[i + 3] == b':' => {
            let o = num(i + 1..i + 3)? * 3600 + num(i + 4..i + 6)? * 60;
            if *c == b'+' { o } else { -o }
        }
        _ => return None,
    };
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    let secs = days_from_civil(y, mo as u32, d as u32) * 86_400 + h * 3600 + mi * 60 + sec - offset;
    let secs = u64::try_from(secs).ok()?;
    Some(UNIX_EPOCH + std::time::Duration::new(secs, nanos))
}

/// Howard Hinnant's civil-to-days algorithm.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = i64::from((m + 9) % 12);
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Howard Hinnant's days-to-civil algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn formats_known_instants() {
        assert_eq!(rfc3339(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(UNIX_EPOCH + Duration::from_secs(1_790_521_200)), "2026-09-27T15:00:00Z");
        assert_eq!(rfc3339(UNIX_EPOCH + Duration::from_secs(951_782_400)), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn parses_what_it_formats_and_offsets() {
        let t = UNIX_EPOCH + Duration::from_secs(1_790_521_200);
        assert_eq!(parse_rfc3339("2026-09-27T15:00:00Z"), Some(t));
        assert_eq!(parse_rfc3339("2026-09-27T17:00:00+02:00"), Some(t));
        assert_eq!(parse_rfc3339("2026-09-27T15:00:00.5Z"), Some(t + Duration::from_millis(500)));
        assert_eq!(parse_rfc3339("2000-02-29T00:00:00Z"), Some(UNIX_EPOCH + Duration::from_secs(951_782_400)));
        assert_eq!(parse_rfc3339("2026-09-27"), None);
        assert_eq!(parse_rfc3339("2026-13-27T15:00:00Z"), None);
    }
}
