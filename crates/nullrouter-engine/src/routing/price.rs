//! The price schedule in effect now, from the plugin's declaration or the operator's override (research R10).

use std::time::SystemTime;

use nullrouter_registry::schema::{PriceDecl, PriceWhen, Weekday, parse_clock, parse_offset};

use crate::accounts::PriceOverride;

/// What an account is priced by: the plugin's schedule, or the operator's flat price, which
/// replaces the whole schedule.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PriceSpec {
    pub schedule: Vec<PriceDecl>,
    pub flat: Option<PriceOverride>,
}

/// What one input token costs now, per million: the operator's flat price, else the first
/// schedule entry whose `when` holds, else the entry without one. `None` when nothing is priced,
/// which ranks as 1 (R10).
pub fn price_now(spec: &PriceSpec, now: SystemTime) -> Option<f64> {
    if let Some(flat) = spec.flat {
        return Some(flat.input);
    }
    spec.schedule.iter().find(|p| p.when.as_ref().is_none_or(|w| holds(w, now))).map(|p| p.input)
}

/// The price a ranking divides by: `price_now`, or 1 for an account with no price anywhere.
pub fn rank_price(spec: &PriceSpec, now: SystemTime) -> f64 {
    price_now(spec, now).filter(|p| *p > 0.0).unwrap_or(1.0)
}

fn holds(w: &PriceWhen, now: SystemTime) -> bool {
    let offset = w.offset.as_deref().and_then(|o| parse_offset(o).ok()).unwrap_or(0);
    let utc_min = now.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| (d.as_secs() / 60) as i64);
    let local = utc_min + i64::from(offset);
    let (day, minute) = (local.div_euclid(1440), local.rem_euclid(1440) as u32);
    // 1970-01-01 was a Thursday; Monday is 0.
    let weekday = (day + 3).rem_euclid(7);
    if !w.days.is_empty() && !w.days.iter().any(|d| weekday_index(*d) == weekday) {
        return false;
    }
    let clock = |s: &Option<String>| s.as_deref().and_then(|s| parse_clock(s).ok());
    match (clock(&w.from), clock(&w.to)) {
        (Some(from), Some(to)) if from <= to => minute >= from && minute < to,
        // `to` before `from` wraps past midnight.
        (Some(from), Some(to)) => minute >= from || minute < to,
        (Some(from), None) => minute >= from,
        (None, Some(to)) => minute < to,
        (None, None) => true,
    }
}

fn weekday_index(d: Weekday) -> i64 {
    match d {
        Weekday::Mon => 0,
        Weekday::Tue => 1,
        Weekday::Wed => 2,
        Weekday::Thu => 3,
        Weekday::Fri => 4,
        Weekday::Sat => 5,
        Weekday::Sun => 6,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    /// 2026-10-05 is a Monday.
    fn at(day: u64, h: u64, m: u64) -> SystemTime {
        let monday = 1_791_158_400; // 2026-10-05T00:00:00Z
        SystemTime::UNIX_EPOCH + Duration::from_secs(monday + day * 86_400 + h * 3600 + m * 60)
    }

    fn entry(input: f64, when: Option<PriceWhen>) -> PriceDecl {
        PriceDecl { when, input, output: None, cache_read: None, cache_write: None }
    }

    fn when(days: &[Weekday], from: Option<&str>, to: Option<&str>, offset: Option<&str>) -> Option<PriceWhen> {
        Some(PriceWhen {
            days: days.to_vec(),
            from: from.map(Into::into),
            to: to.map(Into::into),
            offset: offset.map(Into::into),
        })
    }

    fn spec(schedule: Vec<PriceDecl>) -> PriceSpec {
        PriceSpec { schedule, flat: None }
    }

    #[test]
    fn the_first_matching_entry_wins_and_the_entry_without_when_is_the_default() {
        let s = spec(vec![
            entry(1.0, when(&[], Some("16:30"), Some("23:59"), None)),
            entry(2.0, when(&[], Some("16:00"), Some("23:59"), None)),
            entry(5.0, None),
        ]);
        assert_eq!(price_now(&s, at(0, 17, 0)), Some(1.0));
        assert_eq!(price_now(&s, at(0, 16, 10)), Some(2.0));
        assert_eq!(price_now(&s, at(0, 9, 0)), Some(5.0));
    }

    #[test]
    fn days_limit_an_entry() {
        let s = spec(vec![entry(1.0, when(&[Weekday::Sat, Weekday::Sun], None, None, None)), entry(4.0, None)]);
        assert_eq!(price_now(&s, at(0, 12, 0)), Some(4.0), "Monday");
        assert_eq!(price_now(&s, at(5, 12, 0)), Some(1.0), "Saturday");
        assert_eq!(price_now(&s, at(6, 23, 59)), Some(1.0), "Sunday");
        assert_eq!(price_now(&s, at(7, 0, 0)), Some(4.0), "Monday again");
    }

    #[test]
    fn to_before_from_wraps_past_midnight() {
        let s = spec(vec![entry(1.0, when(&[], Some("22:00"), Some("06:00"), None)), entry(3.0, None)]);
        assert_eq!(price_now(&s, at(0, 23, 0)), Some(1.0));
        assert_eq!(price_now(&s, at(1, 5, 59)), Some(1.0));
        assert_eq!(price_now(&s, at(1, 6, 0)), Some(3.0));
        assert_eq!(price_now(&s, at(0, 21, 59)), Some(3.0));
    }

    #[test]
    fn the_offset_moves_the_clock_and_the_day() {
        // 22:00 to 06:00 at +08:00 is 14:00 to 22:00 UTC.
        let s = spec(vec![entry(1.0, when(&[], Some("22:00"), Some("06:00"), Some("+08:00"))), entry(3.0, None)]);
        assert_eq!(price_now(&s, at(0, 14, 0)), Some(1.0));
        assert_eq!(price_now(&s, at(0, 21, 59)), Some(1.0));
        assert_eq!(price_now(&s, at(0, 22, 0)), Some(3.0));
        // Monday only, at -05:00: Monday 01:00 UTC is still Sunday there.
        let s = spec(vec![entry(1.0, when(&[Weekday::Mon], None, None, Some("-05:00"))), entry(3.0, None)]);
        assert_eq!(price_now(&s, at(0, 1, 0)), Some(3.0));
        assert_eq!(price_now(&s, at(0, 5, 0)), Some(1.0));
    }

    #[test]
    fn the_flat_override_replaces_the_whole_schedule() {
        let flat = PriceOverride { input: 9.0, output: None, cache_read: None, cache_write: None };
        let s = PriceSpec { schedule: vec![entry(1.0, None)], flat: Some(flat) };
        assert_eq!(price_now(&s, at(0, 12, 0)), Some(9.0));
    }

    #[test]
    fn no_price_anywhere_ranks_as_one() {
        let s = PriceSpec::default();
        assert_eq!(price_now(&s, at(0, 0, 0)), None);
        assert_eq!(rank_price(&s, at(0, 0, 0)), 1.0);
        // A schedule that doesn't cover the moment, with no default, is the same.
        let s = spec(vec![entry(2.0, when(&[Weekday::Sat], None, None, None))]);
        assert_eq!(rank_price(&s, at(0, 0, 0)), 1.0);
        // A free price can't divide.
        assert_eq!(rank_price(&spec(vec![entry(0.0, None)]), at(0, 0, 0)), 1.0);
    }
}
