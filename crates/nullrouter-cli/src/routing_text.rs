//! The `routing` text view (slice 006 contracts/operator-cli.md § `routing`): a header with the
//! amortization window and the journal's sync age, then each target's accounts in tiers, one line
//! per account with its pace, share, deficit, priority, cache lifetime and windows.
//!
//! ```text
//! amortization 5h (05:00–10:00 UTC, 2h 48m left) · records kept, last sync 0.4 s ago
//!
//! sonnet                              subscription tier
//!   account          source     pace  share  deficit  priority  cache  windows
//!   anthropic/max    polled     1.42  61%    +91.2k   1         5m     5-hour 5.6M/9.0M wtok · floor 5% · rst 10:00
//! ```
//!
//! Input: the answer of a `routing.view` request. Times are UTC.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nullrouter_engine::clock;
use serde_json::Value;

const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/// Where the second tier's label starts, in the contract's example.
const LABEL_AT: usize = 36;

fn time_of(v: &Value) -> Option<SystemTime> {
    v.as_str().and_then(clock::parse_rfc3339)
}

/// `1234.5` → `1.2k`, `5_600_000` → `5.6M`, `250` → `250`.
pub fn si(x: f64) -> String {
    let a = x.abs();
    if a >= 1e6 {
        format!("{:.1}M", x / 1e6)
    } else if a >= 1e3 {
        format!("{:.1}k", x / 1e3)
    } else {
        format!("{x:.0}")
    }
}

/// A deficit with its sign: `+91.2k`, `-250`, `0`.
pub fn deficit(n: i64) -> String {
    match n {
        0 => "0".into(),
        n if n > 0 => format!("+{}", si(n as f64)),
        n => si(n as f64),
    }
}

fn number(x: f64) -> String {
    if x.fract() == 0.0 { format!("{x:.0}") } else { format!("{x}") }
}

pub fn hm(t: SystemTime) -> String {
    clock::rfc3339(t).get(11..16).unwrap_or("??:??").to_owned()
}

fn weekday(t: SystemTime) -> &'static str {
    let days = t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() / 86_400);
    // 1970-01-01 was a Thursday.
    DAYS[((days + 4) % 7) as usize]
}

/// `10:00` within a day of `now`, else `Thu 09:00`.
fn reset(t: SystemTime, now: SystemTime) -> String {
    match t.duration_since(now) {
        Ok(d) if d > Duration::from_secs(86_400) => format!("{} {}", weekday(t), hm(t)),
        _ => hm(t),
    }
}

fn left(d: Duration) -> String {
    let m = d.as_secs() / 60;
    match (m / 60, m % 60) {
        (0, m) => format!("{m}m"),
        (h, m) => format!("{h}h {m:02}m"),
    }
}

fn length_text(v: &Value) -> String {
    v.as_str().unwrap_or("?").to_owned()
}

fn header(answer: &Value, now: SystemTime) -> String {
    let w = &answer["amortization"];
    let mut out = format!("amortization {}", length_text(&w["length"]));
    if let (Some(start), Some(len)) =
        (time_of(&w["start"]), w["length"].as_str().and_then(|l| nullrouter_registry::schema::parse_duration(l).ok()))
    {
        let end = start + len;
        let span = if len >= Duration::from_secs(86_400) {
            format!("{} {}–{} {}", weekday(start), hm(start), weekday(end), hm(end))
        } else {
            format!("{}–{}", hm(start), hm(end))
        };
        let remaining = end.duration_since(now).unwrap_or_default();
        out += &format!(" ({span} UTC, {} left)", left(remaining));
    }
    let j = &answer["journal"];
    if j["kept"] == false {
        out += &format!(" · records not kept since {}", j["since"].as_str().unwrap_or("?"));
    } else {
        match j["last_sync_age_s"].as_f64() {
            Some(age) => out += &format!(" · records kept, last sync {age:.1} s ago"),
            None => out += " · records kept",
        }
    }
    out
}

fn window(w: &Value, now: SystemTime) -> String {
    let unit = match w["unit"].as_str() {
        Some("requests") => "req",
        _ => "wtok",
    };
    let mut s = format!(
        "{} {}/{} {unit} · floor {}%",
        w["name"].as_str().unwrap_or("?"),
        si(w["remaining_now"].as_f64().unwrap_or(0.0)),
        si(w["capacity"].as_f64().unwrap_or(0.0)),
        number((w["reserve"].as_f64().unwrap_or(0.0) * 1000.0).round() / 10.0),
    );
    if let Some(t) = time_of(&w["resets_at"]) {
        s += &format!(" · rst {}", reset(t, now));
    }
    s
}

fn row(a: &Value, now: SystemTime) -> Vec<String> {
    let payg = a["tier"] == "payg";
    let dash = || "—".to_owned();
    let pace = a["pace"].as_f64().map_or_else(dash, |p| format!("{p:.2}"));
    let share = a["share"].as_f64().map_or_else(dash, |s| format!("{:.0}%", s * 100.0));
    let source = match a["source"].as_str() {
        Some("pay-as-you-go") => "payg".to_owned(),
        Some("polled") if a["pending_first_poll"] == true => "polled (pending first poll)".to_owned(),
        Some("polled") if a["stale"] == true => "polled (stale)".to_owned(),
        Some(s) => s.to_owned(),
        None => "?".to_owned(),
    };
    let mut windows = if payg {
        match a["price_now"].as_f64() {
            Some(p) => format!("price now {p:.2}/Mtok in"),
            None => "price not declared".into(),
        }
    } else {
        a["windows"].as_array().into_iter().flatten().map(|w| window(w, now)).collect::<Vec<_>>().join(" | ")
    };
    let note = match a["why_not"].as_str() {
        Some("priority_zero") => Some("cold work off (priority 0)".to_owned()),
        Some(why) => Some(format!("not an option: {}", why.replace('_', " "))),
        None => None,
    };
    if let Some(n) = note {
        windows = if windows.is_empty() { n } else { format!("{windows} · {n}") };
    }
    vec![
        format!("{}/{}", a["provider"].as_str().unwrap_or("?"), a["account"].as_str().unwrap_or("?")),
        source,
        pace,
        share,
        deficit(a["deficit"].as_i64().unwrap_or(0)),
        number(a["priority"].as_f64().unwrap_or(0.0)),
        a["cache_lifetime"].as_str().unwrap_or("?").to_owned(),
        windows,
    ]
}

/// Cells padded to their column's widest, two spaces apart; the last column isn't padded.
fn pad(rows: &[Vec<String>]) -> Vec<String> {
    let cols = rows.first().map_or(0, Vec::len);
    let widths: Vec<usize> = (0..cols).map(|i| rows.iter().map(|r| r[i].chars().count()).max().unwrap_or(0)).collect();
    rows.iter()
        .map(|r| {
            let mut line = String::new();
            for (i, cell) in r.iter().enumerate() {
                line.push_str(cell);
                if i + 1 < cols {
                    line.push_str(&" ".repeat(widths[i] - cell.chars().count() + 2));
                }
            }
            line
        })
        .collect()
}

fn target(t: &Value, now: SystemTime, out: &mut String) {
    let accounts: Vec<&Value> = t["accounts"].as_array().into_iter().flatten().collect();
    let (payg, subs): (Vec<&Value>, Vec<&Value>) = accounts.into_iter().partition(|a| a["tier"] == "payg");
    let mut cells = vec![
        ["account", "source", "pace", "share", "deficit", "priority", "cache", "windows"].map(str::to_owned).to_vec(),
    ];
    cells.extend(subs.iter().chain(&payg).map(|a| row(a, now)));
    let lines = pad(&cells);
    let name = t["target"].as_str().unwrap_or("?");
    let first = if subs.is_empty() { "pay-as-you-go tier" } else { "subscription tier" };
    out.push_str(&format!("{name:<LABEL_AT$}{first}\n"));
    out.push_str(&format!("  {}\n", lines[0]));
    for (i, line) in lines[1..].iter().enumerate() {
        if i == subs.len() && !subs.is_empty() && !payg.is_empty() {
            out.push_str(&format!("{:<LABEL_AT$}pay-as-you-go tier\n", ""));
        }
        out.push_str(&format!("  {line}\n"));
    }
}

/// The whole view for a `routing.view` answer, at `now`.
pub fn render(answer: &Value, now: SystemTime) -> String {
    let mut out = header(answer, now);
    out.push('\n');
    let targets = answer["targets"].as_array().cloned().unwrap_or_default();
    if targets.is_empty() {
        out.push_str("\nno target has accounts to route over\n");
    }
    for t in &targets {
        out.push('\n');
        target(t, now, &mut out);
    }
    let warnings: Vec<&str> = answer["warnings"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    if !warnings.is_empty() {
        out.push('\n');
        for w in warnings {
            out.push_str(w);
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn now() -> SystemTime {
        clock::parse_rfc3339("2026-10-04T07:12:00Z").unwrap()
    }

    fn account(provider: &str, name: &str, tier: &str, source: &str, extra: Value) -> Value {
        let mut a = json!({"provider": provider, "account": name, "tier": tier, "source": source, "priority": 1.0,
            "pace": null, "share": null, "deficit": 0, "cache_lifetime": "5m", "price_now": null, "why_not": null, "windows": []});
        for (k, v) in extra.as_object().unwrap() {
            a[k] = v.clone();
        }
        a
    }

    fn answer() -> Value {
        let w = |name: &str, rem: f64, cap: f64, reserve: f64, resets: &str| json!({"name": name, "unit": "weighted_tokens", "remaining_now": rem, "capacity": cap, "reserve": reserve, "resets_at": resets});
        json!({
            "amortization": {"start": "2026-10-04T05:00:00.000Z", "length": "5h"},
            "journal": {"kept": true, "last_sync_age_s": 0.4},
            "warnings": ["opencode-go/main: window rolling capacity assumed"],
            "targets": [{"target": "sonnet", "accounts": [
                account("anthropic", "max", "subscription", "polled", json!({"pace": 1.42, "share": 0.61, "deficit": 91_200,
                    "windows": [w("5-hour", 5.6e6, 9e6, 0.05, "2026-10-04T10:00:00.000Z"), w("weekly", 72.9e6, 90e6, 0.05, "2026-10-09T09:00:00.000Z")]})),
                account("opencode-go", "main", "subscription", "estimated", json!({"pace": 1.05, "priority": 0.0, "why_not": "priority_zero",
                    "windows": [w("rolling", 4.2e6, 6e6, 0.05, "2026-10-04T12:00:00.000Z")]})),
                account("openrouter", "main", "payg", "pay-as-you-go", json!({"share": 1.0, "price_now": 3.0})),
            ]}],
        })
    }

    #[test]
    fn the_view_follows_the_contract() {
        let text = render(&answer(), now());
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "amortization 5h (05:00–10:00 UTC, 2h 48m left) · records kept, last sync 0.4 s ago");
        assert_eq!(lines[1], "");
        assert_eq!(lines[2], format!("{:<36}subscription tier", "sonnet"));
        assert!(lines[3].trim_start().starts_with("account") && lines[3].ends_with("windows"), "{}", lines[3]);
        let max = lines[4];
        for want in [
            "anthropic/max",
            "polled",
            "1.42",
            "61%",
            "+91.2k",
            "5m",
            "5-hour 5.6M/9.0M wtok · floor 5% · rst 10:00",
            " | weekly 72.9M/90.0M wtok · floor 5% · rst Fri 09:00",
        ] {
            assert!(max.contains(want), "{want:?} missing from {max:?}");
        }
        assert!(
            lines[5].contains("estimated") && lines[5].ends_with("rst 12:00 · cold work off (priority 0)"),
            "{}",
            lines[5]
        );
        assert_eq!(lines[6], format!("{:<36}pay-as-you-go tier", ""));
        assert!(
            lines[7].contains("openrouter/main")
                && lines[7].contains("payg")
                && lines[7].ends_with("price now 3.00/Mtok in"),
            "{}",
            lines[7]
        );
        assert_eq!(lines[9], "opencode-go/main: window rolling capacity assumed");
    }

    #[test]
    fn a_journal_that_lost_writes_is_said_in_the_header() {
        let mut a = answer();
        a["journal"] = json!({"kept": false, "since": "2026-10-04T09:01:00Z"});
        assert!(render(&a, now()).lines().next().unwrap().ends_with("records not kept since 2026-10-04T09:01:00Z"));
    }

    #[test]
    fn numbers() {
        assert_eq!((si(5.6e6), si(1234.0), si(250.0)), ("5.6M".into(), "1.2k".into(), "250".into()));
        assert_eq!((deficit(-91_200), deficit(250), deficit(0)), ("-91.2k".into(), "+250".into(), "0".into()));
    }
}
