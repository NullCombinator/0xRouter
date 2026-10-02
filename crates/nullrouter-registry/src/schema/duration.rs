//! Durations written as `500ms`, `30s`, `5m`, `4h` or `1d` in plugin files.

use std::time::Duration;

use serde::de::{Deserialize, Deserializer, Error as _};

/// Parses one number followed by one unit: `ms`, `s`, `m`, `h` or `d`.
///
/// ```
/// use std::time::Duration;
/// use nullrouter_registry::schema::parse_duration;
/// assert_eq!(parse_duration("5m"), Ok(Duration::from_secs(300)));
/// assert!(parse_duration("5").is_err());
/// ```
pub fn parse_duration(s: &str) -> Result<Duration, String> {
    let bad = || format!("{s:?} is not a duration such as 30s, 5m or 4h");
    let split = s.find(|c: char| !c.is_ascii_digit()).ok_or_else(bad)?;
    let (n, unit) = s.split_at(split);
    let n: u64 = n.parse().map_err(|_| bad())?;
    let secs = |k: u64| n.checked_mul(k).map(Duration::from_secs).ok_or_else(bad);
    match unit {
        "ms" => Ok(Duration::from_millis(n)),
        "s" => secs(1),
        "m" => secs(60),
        "h" => secs(3600),
        "d" => secs(86_400),
        _ => Err(bad()),
    }
}

pub(crate) fn de_duration<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
    parse_duration(&String::deserialize(d)?).map_err(D::Error::custom)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units() {
        assert_eq!(parse_duration("500ms"), Ok(Duration::from_millis(500)));
        assert_eq!(parse_duration("30s"), Ok(Duration::from_secs(30)));
        assert_eq!(parse_duration("4h"), Ok(Duration::from_secs(4 * 3600)));
        assert_eq!(parse_duration("1d"), Ok(Duration::from_secs(86_400)));
        for bad in ["", "m", "5", "5 m", "-5m", "5min", "1.5h", "99999999999999999999h"] {
            assert!(parse_duration(bad).is_err(), "{bad}");
        }
    }
}
