//! `usage [--period]`: request and token totals and Est. Cost for one period (spec 010, research
//! R1 to R5). A running server answers `usage.totals` from its cache and its live ring; without
//! one the view reads the journal itself, and the two give the same numbers.

use std::time::SystemTime;

use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};
use nullrouter_engine::accounts::{self, Accounts};
use nullrouter_engine::clock;
use nullrouter_engine::journal::summary::{self, Window};
use nullrouter_engine::keys::{self, Keys};
use nullrouter_registry::OperatorHome;
use serde_json::{Value, json};

use super::{Live, View, ViewError, open_registry};

/// The server's totals, and its journal health for the "records not kept" warning.
pub const NEEDS: &[&str] = &["usage.totals", "routing.health"];

pub const PERIODS: &str = "today, 24h, 7d, 30d, 60d or all";
pub const LABEL: &str = "Estimated, not actual billing";
pub const NOTE: &str = "Priced with today's declared prices; earlier price changes are not tracked.";

/// The window a period names at `at`, in `zone`. `today` starts at local midnight, so on a
/// daylight-saving change day it is 23 or 25 hours long; the others subtract elapsed time.
pub fn window(period: &str, at: Timestamp, zone: &TimeZone) -> Result<(Option<Timestamp>, Timestamp), ViewError> {
    let back = |hours: i64| {
        at.checked_sub(SignedDuration::from_hours(hours)).map_err(|e| ViewError::failed(format!("{period}: {e}")))
    };
    let from = match period {
        "today" => Some(
            at.to_zoned(zone.clone())
                .start_of_day()
                .map_err(|e| ViewError::failed(format!("today: {e}")))?
                .timestamp(),
        ),
        "24h" => Some(back(24)?),
        "7d" => Some(back(7 * 24)?),
        "30d" => Some(back(30 * 24)?),
        "60d" => Some(back(60 * 24)?),
        "all" => None,
        other => return Err(ViewError::failed(format!("unknown period {other:?}; use {PERIODS}"))),
    };
    Ok((from, at))
}

/// `at` from the arguments (RFC 3339), else the wall clock.
pub fn at_of(args: &Value) -> Result<Timestamp, ViewError> {
    let t = match args["at"].as_str() {
        Some(s) => clock::parse_rfc3339(s).ok_or_else(|| ViewError::failed(format!("{s:?} is not an RFC 3339 time")))?,
        None => clock::now(),
    };
    Timestamp::try_from(t).map_err(ViewError::failed)
}

fn system(t: Timestamp) -> SystemTime {
    SystemTime::from(t)
}

/// An RFC 3339 time as local wall-clock text, `2026-10-06 14:02:11`, in the machine's zone. The
/// text both commands print, so the zone rule lives in one place. A time that doesn't parse is
/// returned as it came.
pub fn local_text(rfc: &str) -> String {
    clock::parse_rfc3339(rfc)
        .and_then(|t| Timestamp::try_from(t).ok())
        .map_or_else(|| rfc.to_owned(), |t| t.to_zoned(TimeZone::system()).strftime("%Y-%m-%d %H:%M:%S").to_string())
}

/// `[from, to)` as RFC 3339 for the operator op, from the view's arguments. An argument that
/// names no window leaves both null; the view reports why.
pub fn op_window(args: &Value) -> (Value, Value) {
    let period = args["period"].as_str().unwrap_or("today");
    match at_of(args).and_then(|at| window(period, at, &TimeZone::system())) {
        Ok((from, to)) => (json!(from.map(|f| clock::rfc3339(system(f)))), json!(clock::rfc3339(system(to)))),
        Err(_) => (Value::Null, Value::Null),
    }
}

/// Arguments: `period` (default `today`), `at` (RFC 3339, default now).
pub fn build(home: &OperatorHome, args: &Value, live: &Live) -> Result<View, ViewError> {
    let period = args["period"].as_str().unwrap_or("today");
    let zone = TimeZone::system();
    let (from, to) = window(period, at_of(args)?, &zone)?;
    let (from, to) = (from.map(system), system(to));

    let served = live.ok("usage.totals")?;
    let totals = match served {
        Some(a) => a["totals"].clone(),
        None => {
            let reg = open_registry(home)?.snapshot();
            let accounts = Accounts::load(&home.path().join(accounts::FILE)).map_err(ViewError::failed)?;
            let prices = summary::prices_of(&reg, &accounts);
            let own = summary::totals(home.path(), &Window { from, to }, &prices, live.running);
            serde_json::to_value(own).map_err(ViewError::failed)?
        }
    };

    let names: std::collections::BTreeMap<String, String> = Keys::load(&home.path().join(keys::FILE))
        .map(|k| k.iter().map(|k| (k.id.clone(), k.name.clone())).collect())
        .unwrap_or_default();
    let n = |v: &Value, k: &str| v[k].as_u64().unwrap_or(0);
    let unpriced = &totals["unpriced"];
    let rows = |map: &Value, named: bool| {
        let mut rows: Vec<(String, u64)> =
            map.as_object().into_iter().flatten().map(|(k, v)| (k.clone(), v.as_u64().unwrap_or(0))).collect();
        rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        rows.into_iter()
            .map(|(id, requests)| {
                let mut row = json!({"id": id, "requests": requests});
                if named {
                    row["name"] = names.get(&id).map_or(Value::Null, |n| json!(n));
                }
                row
            })
            .collect::<Vec<_>>()
    };
    let mut warnings = Vec::new();
    if let Some(j) = live.answer("routing.health").filter(|a| a["ok"] == true).map(|a| &a["journal"])
        && j["kept"] == false
        && j["since"].as_str().and_then(clock::parse_rfc3339).is_some_and(|since| since < to)
    {
        warnings.push(json!(format!(
            "warning: records not kept since {} (disk full): {} requests",
            j["since"].as_str().unwrap_or("?"),
            j["unkept_requests"]
        )));
    }
    Ok(View::new(json!({
        "period": period,
        "at": clock::rfc3339(to),
        "from": from.map(clock::rfc3339),
        "to": clock::rfc3339(to),
        "zone": zone.iana_name().unwrap_or("local"),
        "requests": n(&totals, "requests"),
        "in_flight": n(&totals, "in_flight"),
        "not_reported": n(&totals, "not_reported"),
        "tokens": {"input": n(&totals, "input"), "cached": n(&totals, "cached"), "output": n(&totals, "output")},
        "cost": {
            "usd": totals["cost_usd"].as_f64().unwrap_or(0.0),
            "label": LABEL,
            "note": NOTE,
            "unpriced": {
                "requests": n(unpriced, "no_price") + n(unpriced, "account_gone") + n(unpriced, "no_output_price"),
                "no_price": n(unpriced, "no_price"),
                "account_gone": n(unpriced, "account_gone"),
                "no_output_price": n(unpriced, "no_output_price"),
            },
        },
        "agents": rows(&totals["agents"], true),
        "providers": rows(&totals["providers"], false),
        "warnings": warnings,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    fn berlin() -> TimeZone {
        TimeZone::get("Europe/Berlin").unwrap()
    }

    #[test]
    fn today_starts_at_local_midnight() {
        let (from, to) = window("today", ts("2026-10-06T14:02:11Z"), &berlin()).unwrap();
        // Berlin is UTC+2 in October: midnight on the 6th is 22:00 UTC on the 5th.
        assert_eq!(from, Some(ts("2026-10-05T22:00:00Z")));
        assert_eq!(to, ts("2026-10-06T14:02:11Z"));
    }

    #[test]
    fn today_follows_the_clock_change_days() {
        // 2026-03-29: clocks go forward at 02:00, so the day is 23 h long; midnight is still +01:00.
        let (from, _) = window("today", ts("2026-03-29T12:00:00Z"), &berlin()).unwrap();
        assert_eq!(from, Some(ts("2026-03-28T23:00:00Z")));
        // 2026-10-25: clocks go back at 03:00, a 25 h day; midnight is +02:00.
        let (from, _) = window("today", ts("2026-10-25T12:00:00Z"), &berlin()).unwrap();
        assert_eq!(from, Some(ts("2026-10-24T22:00:00Z")));
        // After the change the next day starts at +01:00.
        let (from, _) = window("today", ts("2026-10-26T12:00:00Z"), &berlin()).unwrap();
        assert_eq!(from, Some(ts("2026-10-25T23:00:00Z")));
    }

    #[test]
    fn the_rolling_periods_subtract_elapsed_time_and_all_has_no_start() {
        let at = ts("2026-10-06T14:02:11Z");
        for (p, secs) in [("24h", 86_400), ("7d", 7 * 86_400), ("30d", 30 * 86_400), ("60d", 60 * 86_400)] {
            let (from, to) = window(p, at, &berlin()).unwrap();
            assert_eq!(to.as_second() - from.unwrap().as_second(), secs, "{p}");
        }
        assert_eq!(window("all", at, &berlin()).unwrap().0, None);
    }

    #[test]
    fn an_unknown_period_names_the_six() {
        let e = window("week", ts("2026-10-06T14:02:11Z"), &berlin()).unwrap_err();
        assert_eq!(e.code, 1);
        assert_eq!(e.message, "unknown period \"week\"; use today, 24h, 7d, 30d, 60d or all");
    }

    #[test]
    fn the_view_has_the_data_model_shape_with_names_joined_and_sorted() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join("keys.toml"),
            "schema = 1\n\n[[key]]\nid = \"ak_a\"\nname = \"alice\"\ndigest = \"sha256:00\"\nlast4 = \"0000\"\ncreated = \"2026-09-01T00:00:00Z\"\n",
        )
        .unwrap();
        let mut live = Live::none();
        live.answers.insert(
            "usage.totals",
            Some(json!({"ok": true, "totals": {
                "requests": 5, "in_flight": 1, "not_reported": 2, "input": 10, "cached": 20, "output": 30,
                "cost_usd": 1.5, "unpriced": {"no_price": 1, "account_gone": 0, "no_output_price": 2},
                "agents": {"ak_a": 2, "ak_gone": 3, "ak_b": 3}, "providers": {"xai": 1, "anthropic": 4}}})),
        );
        let args = json!({"period": "all", "at": "2026-10-06T14:02:11Z"});
        let v = build(&OperatorHome::new(home.path()), &args, &live).unwrap().json;
        assert_eq!(v["period"], "all");
        assert_eq!(v["from"], Value::Null);
        assert_eq!(v["to"], "2026-10-06T14:02:11Z");
        assert_eq!(v["tokens"], json!({"input": 10, "cached": 20, "output": 30}));
        assert_eq!(v["cost"]["label"], "Estimated, not actual billing");
        assert_eq!(v["cost"]["note"], NOTE);
        assert_eq!(v["cost"]["unpriced"], json!({"requests": 3, "no_price": 1, "account_gone": 0, "no_output_price": 2}));
        // Requests descending, then id; a deleted key's name is null.
        assert_eq!(
            v["agents"],
            json!([{"id": "ak_b", "name": null, "requests": 3}, {"id": "ak_gone", "name": null, "requests": 3}, {"id": "ak_a", "name": "alice", "requests": 2}])
        );
        assert_eq!(v["providers"], json!([{"id": "anthropic", "requests": 4}, {"id": "xai", "requests": 1}]));
        assert_eq!(v["warnings"], json!([]));
    }
}
