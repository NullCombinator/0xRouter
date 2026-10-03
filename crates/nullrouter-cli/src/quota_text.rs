//! The `quota` text view (slice 005 contracts/operator-cli.md § `quota`): one line per
//! account, then one line per window the provider reported, every value belonging to the
//! poll time on its account line (FR-020).
//!
//! ```text
//! anthropic/max            polled 14:20 (3 min ago, every 10 min)
//!   5-hour                 62% left   resets 16:00
//! xai/main                 quota not reported
//! ```
//!
//! Input: the `accounts` of a `quota.list` answer. Times are shown at `offset_secs` from UTC.

use std::fmt::Write as _;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nullrouter_engine::clock;
use serde_json::Value;

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/// A wall-clock instant at a fixed offset.
struct Local {
    day: i64,
    year: u32,
    month: usize,
    mday: u32,
    hm: String,
}

fn local(t: SystemTime, offset_secs: i64) -> Local {
    let secs = t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64) + offset_secs;
    let shifted = UNIX_EPOCH + Duration::from_secs(secs.max(0) as u64);
    // `YYYY-MM-DDTHH:MM:SSZ`
    let s = clock::rfc3339(shifted);
    Local {
        day: secs.div_euclid(86_400),
        year: s[0..4].parse().unwrap_or(1970),
        month: s[5..7].parse::<usize>().unwrap_or(1).clamp(1, 12) - 1,
        mday: s[8..10].parse().unwrap_or(1),
        hm: s[11..16].to_owned(),
    }
}

fn weekday(day: i64) -> &'static str {
    DAYS[(day + 4).rem_euclid(7) as usize]
}

/// A poll time: `14:20` today, else `Oct 2 14:20` (`2025 Oct 2 14:20` in another year).
fn when_polled(t: SystemTime, now: SystemTime, offset: i64) -> String {
    let (l, n) = (local(t, offset), local(now, offset));
    if l.day == n.day {
        l.hm
    } else if l.year == n.year {
        format!("{} {} {}", MONTHS[l.month], l.mday, l.hm)
    } else {
        format!("{} {} {} {}", l.year, MONTHS[l.month], l.mday, l.hm)
    }
}

/// A reset time: `16:00` today, `Thu 09:00` within the week, else `Nov 1`.
fn when_reset(t: SystemTime, now: SystemTime, offset: i64) -> String {
    let (l, n) = (local(t, offset), local(now, offset));
    match l.day - n.day {
        0 => l.hm,
        1..=6 => format!("{} {}", weekday(l.day), l.hm),
        _ if l.year == n.year => format!("{} {}", MONTHS[l.month], l.mday),
        _ => format!("{} {} {}", MONTHS[l.month], l.mday, l.year),
    }
}

fn ago(t: SystemTime, now: SystemTime) -> String {
    let s = now.duration_since(t).unwrap_or_default().as_secs();
    match s {
        0..60 => format!("{s} s ago"),
        60..7200 => format!("{} min ago", s / 60),
        7200..172_800 => format!("{} h ago", s / 3600),
        _ => format!("{} d ago", s / 86_400),
    }
}

/// `10 min`, `1 h`, `90 s`.
pub fn every(secs: u64) -> String {
    if secs != 0 && secs.is_multiple_of(3600) {
        format!("{} h", secs / 3600)
    } else if secs != 0 && secs.is_multiple_of(60) {
        format!("{} min", secs / 60)
    } else {
        format!("{secs} s")
    }
}

/// `1,240`, `87.5`, `0`.
fn num(x: f64) -> String {
    let neg = x < 0.0;
    let x = (x.abs() * 10.0).round() / 10.0;
    let whole = x.trunc() as u64;
    let mut digits = whole.to_string();
    let mut grouped = String::new();
    while digits.len() > 3 {
        let tail = digits.split_off(digits.len() - 3);
        grouped = format!(",{tail}{grouped}");
    }
    let frac = ((x - x.trunc()) * 10.0).round() as u64;
    let sign = if neg { "-" } else { "" };
    if frac == 0 { format!("{sign}{digits}{grouped}") } else { format!("{sign}{digits}{grouped}.{frac}") }
}

fn window_value(w: &Value) -> String {
    let f = |k: &str| w[k].as_f64();
    let unit = w["unit"].as_str().unwrap_or("percent");
    if unit == "percent" {
        let used = f("used");
        let left = f("remaining").or(used.map(|u| (100.0 - u).max(0.0))).unwrap_or(0.0);
        return match used {
            Some(u) if u > 100.0 => format!("{}% left ({}% used)", num(left), num(u)),
            _ => format!("{}% left", num(left)),
        };
    }
    match (f("used"), f("limit"), f("remaining")) {
        (Some(u), Some(l), _) => format!("{} / {} {unit} used", num(u), num(l)),
        (Some(u), None, _) => format!("{} {unit} used", num(u)),
        (None, _, Some(r)) => format!("{} {unit} left", num(r)),
        _ => "not reported".into(),
    }
}

fn time_of(v: &Value) -> Option<SystemTime> {
    v.as_str().and_then(clock::parse_rfc3339)
}

/// The text for `accounts` (a `quota.list` answer's) at `now`.
pub fn render(accounts: &[Value], now: SystemTime, offset_secs: i64) -> String {
    let mut out = String::new();
    for a in accounts {
        let label = format!("{}/{}", a["provider"].as_str().unwrap_or("?"), a["name"].as_str().unwrap_or("?"));
        let mut head = if a["reported"] != true {
            "quota not reported".to_owned()
        } else {
            match time_of(&a["latest"]["at"]) {
                Some(t) => format!(
                    "polled {} ({}, every {})",
                    when_polled(t, now, offset_secs),
                    ago(t, now),
                    every(a["interval_s"].as_u64().unwrap_or(600))
                ),
                None => "not polled yet".to_owned(),
            }
        };
        if a["reported"] == true
            && let Some(t) = time_of(&a["last_failure"]["at"])
        {
            let why = a["last_failure"]["error"]["summary"].as_str().unwrap_or("failed");
            let _ = write!(head, "; last poll failed {} ({why})", when_polled(t, now, offset_secs));
        }
        let _ = writeln!(out, "{label:<24} {head}");
        if a["reported"] != true {
            continue;
        }
        for w in a["latest"]["windows"].as_array().into_iter().flatten() {
            let name = w["name"].as_str().unwrap_or("?");
            let mut line = format!("  {name:<22} {}", window_value(w));
            if let Some(r) = time_of(&w["resets_at"]) {
                let _ = write!(line, "   resets {}", when_reset(r, now, offset_secs));
            }
            let _ = writeln!(out, "{line}");
        }
    }
    out
}

/// `1 request`, `3 requests`.
fn requests(n: u64) -> String {
    if n == 1 { "1 request".into() } else { format!("{} requests", num(n as f64)) }
}

/// A failed entry's short reason: `HTTP 500`, `timeout`, `rate limited`, ….
fn entry_failure(e: &Value) -> String {
    match (e["class"].as_str().unwrap_or("failed"), e["status"].as_u64()) {
        ("rate_limited", _) => "rate limited".into(),
        ("no_windows", _) => "no quota in the answer".into(),
        ("network", _) => "network error".into(),
        (_, Some(s)) => format!("HTTP {s}"),
        (class, None) => class.replace('_', " "),
    }
}

/// The `quota history` text for one account's entries (oldest first, as stored): a line
/// per poll with its windows or failure, then a line per model of the traffic tally.
///
/// ```text
/// Oct 2 14:20   ok       rolling 75% left
///   m1                     2 requests   input 15   output 3   cache read 0   cache write 0
/// Oct 2 14:30   failed   HTTP 500   no traffic
/// ```
pub fn history(entries: &[Value], now: SystemTime, offset_secs: i64) -> String {
    let mut out = String::new();
    for e in entries {
        let when = time_of(&e["at"]).map_or_else(|| "?".to_owned(), |t| when_polled(t, now, offset_secs));
        let mut line = if e["ok"] == true {
            let windows: Vec<String> = e["windows"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|w| format!("{} {}", w["name"].as_str().unwrap_or("?"), window_value(w)))
                .collect();
            format!("{when:<13} ok       {}", windows.join(", "))
        } else {
            format!("{when:<13} failed   {}", entry_failure(&e["error"]))
        };
        let tally = e["tally"].as_object().filter(|t| !t.is_empty());
        if tally.is_none() {
            line.push_str("   no traffic");
        }
        let _ = writeln!(out, "{}", line.trim_end());
        for (model, t) in tally.into_iter().flatten() {
            let n = |k: &str| t[k].as_u64().unwrap_or(0);
            let mut reqs = requests(n("requests"));
            if n("requests_usage_unreported") > 0 {
                let _ = write!(reqs, " ({} usage unreported)", num(n("requests_usage_unreported") as f64));
            }
            let _ = writeln!(
                out,
                "  {model:<22} {reqs}   input {}   output {}   cache read {}   cache write {}",
                num(n("input") as f64),
                num(n("output") as f64),
                num(n("cache_read") as f64),
                num(n("cache_write") as f64),
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_and_intervals() {
        assert_eq!(num(1240.0), "1,240");
        assert_eq!(num(1_234_567.0), "1,234,567");
        assert_eq!(num(87.5), "87.5");
        assert_eq!(num(0.0), "0");
        assert_eq!(every(600), "10 min");
        assert_eq!(every(3600), "1 h");
        assert_eq!(every(90), "90 s");
    }
}
