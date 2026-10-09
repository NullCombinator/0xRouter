//! Times in this machine's zone (research R13, FR-023). A fact keeps the CLI's RFC 3339 UTC
//! value in `datetime`; the text is the same instant in local time, and the header names the zone.

use jiff::tz::TimeZone;
use jiff::{Timestamp, Zoned};
use maud::{Markup, html};

/// This machine's zone.
pub fn local_zone() -> TimeZone {
    TimeZone::system()
}

/// `<time datetime="<RFC 3339 UTC>">YYYY-MM-DD HH:MM:SS</time>`, the text in `tz`. Text that
/// isn't an RFC 3339 time is shown as it is, without an element.
pub fn time_element(rfc3339: &str, tz: &TimeZone) -> Markup {
    match rfc3339.parse::<Timestamp>() {
        Ok(at) => {
            let utc = at.to_string();
            let local = at.to_zoned(tz.clone()).strftime("%Y-%m-%d %H:%M:%S").to_string();
            html! { time datetime=(utc) { (local) } }
        }
        Err(_) => html! { (rfc3339) },
    }
}

/// `as of 15:04:05 CEST (Europe/Berlin) · reload to refresh`. A zone with no IANA name (a fixed
/// offset) is named by its offset.
pub fn as_of(now: Timestamp, tz: &TimeZone) -> String {
    let at: Zoned = now.to_zoned(tz.clone());
    let name = tz.iana_name().map_or_else(|| at.strftime("UTC%:z").to_string(), str::to_owned);
    format!("as of {} ({name}) · reload to refresh", at.strftime("%H:%M:%S %Z"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn berlin() -> TimeZone {
        TimeZone::get("Europe/Berlin").expect("the system has tzdata")
    }

    #[test]
    fn an_instant_keeps_its_utc_value_and_shows_local_time() {
        let html = time_element("2026-10-05T11:50:00Z", &berlin()).into_string();
        assert_eq!(html, r#"<time datetime="2026-10-05T11:50:00Z">2026-10-05 13:50:00</time>"#);
        let html = time_element("2026-10-05T11:50:00.120Z", &berlin()).into_string();
        assert!(
            html.contains(r#"datetime="2026-10-05T11:50:00.12Z""#)
                || html.contains(r#"datetime="2026-10-05T11:50:00.120Z""#),
            "{html}"
        );
        assert!(html.ends_with(">2026-10-05 13:50:00</time>"), "the page shows whole seconds: {html}");
    }

    #[test]
    fn a_non_time_is_shown_as_it_is() {
        assert_eq!(time_element("never", &berlin()).into_string(), "never");
        assert_eq!(time_element("", &berlin()).into_string(), "");
    }

    #[test]
    fn the_header_names_the_zone() {
        let at: Timestamp = "2026-10-05T13:04:05Z".parse().unwrap();
        assert_eq!(as_of(at, &berlin()), "as of 15:04:05 CEST (Europe/Berlin) · reload to refresh");
        let winter: Timestamp = "2026-12-05T13:04:05Z".parse().unwrap();
        assert_eq!(as_of(winter, &berlin()), "as of 14:04:05 CET (Europe/Berlin) · reload to refresh");
        assert_eq!(as_of(at, &TimeZone::UTC), "as of 13:04:05 UTC (UTC) · reload to refresh");
    }

    /// Clocks went forward at 01:00 UTC on 2026-03-29: the same wall-clock hour never shows twice.
    #[test]
    fn a_daylight_saving_change_is_the_same_instant() {
        let before = time_element("2026-03-29T00:59:59Z", &berlin()).into_string();
        let after = time_element("2026-03-29T01:00:00Z", &berlin()).into_string();
        assert!(before.ends_with(">2026-03-29 01:59:59</time>"), "{before}");
        assert!(after.ends_with(">2026-03-29 03:00:00</time>"), "{after}");
        assert!(
            before.contains(r#"datetime="2026-03-29T00:59:59Z""#)
                && after.contains(r#"datetime="2026-03-29T01:00:00Z""#)
        );
        let header: Timestamp = "2026-03-29T01:00:00Z".parse().unwrap();
        assert_eq!(as_of(header, &berlin()), "as of 03:00:00 CEST (Europe/Berlin) · reload to refresh");
    }
}
