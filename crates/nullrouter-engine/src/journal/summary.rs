//! Summaries over the record journal: totals and Est. Cost for a window, latency per agent and
//! per provider (spec 010, research R3 to R6). Reads only the day segments a window needs and
//! writes nothing.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;

use crate::clock;
use crate::journal::records::{day_of, fold, segments};

/// Nearest-rank percentile of `sorted` (ascending): `v[ceil(q·n) − 1]`. One value gives itself
/// for every `q`; no values give `None` (R6).
pub fn nearest_rank(sorted: &[f64], q: f64) -> Option<f64> {
    let n = sorted.len();
    if n == 0 {
        return None;
    }
    let rank = (q * n as f64).ceil() as usize;
    sorted.get(rank.clamp(1, n) - 1).copied()
}

/// One record's tokens as the cards count them (R4): `input` is uncached, `cached` is cache
/// reads, so `input + cached` is every prompt token and nothing is counted twice.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tokens {
    pub input: u64,
    pub cached: u64,
    pub output: u64,
}

/// The tokens of a record's `usage` (`None` when it reported none). `includes_cache` input has
/// the cache reads and writes taken out and the writes added back, so writes stay in input;
/// `excludes_cache` input gets the writes added. Reasoning tokens are part of `output` already.
pub fn tokens_of(usage: &Value) -> Option<Tokens> {
    if usage.is_null() {
        return None;
    }
    let n = |k: &str| usage[k].as_u64().unwrap_or(0);
    let (read, write) = (n("cache_read"), n("cache_write"));
    let input = if usage["input_semantics"] == "includes_cache" {
        n("input").saturating_sub(read + write) + write
    } else {
        n("input") + write
    };
    Some(Tokens { input, cached: read, output: n("output") })
}

fn is_skipped(attempt: &Value) -> bool {
    attempt["kind"] == "skipped"
}

/// The first attempt that wasn't skipped: when it started is the router's overhead.
pub fn first_attempt(record: &Value) -> Option<&Value> {
    record["attempts"].as_array()?.iter().find(|a| !is_skipped(a))
}

/// The attempt that was running when the first token reached the client: the last non-skipped
/// attempt with `started <= ttft_ms`. A break comes after the first token, so this holds for
/// fallbacks, continuations and restarts alike.
pub fn first_token_attempt(record: &Value) -> Option<&Value> {
    let ttft = record["ttft_ms"].as_f64()?;
    record["attempts"].as_array()?.iter().filter(|a| !is_skipped(a)).rfind(|a| a["started"].as_f64().is_some_and(|s| s <= ttft))
}

/// The provider's own wait for its first token: the record's `ttft_ms` less when that attempt
/// started, with the attempt (for its provider).
pub fn own_ttft(record: &Value) -> Option<(&Value, f64)> {
    let attempt = first_token_attempt(record)?;
    Some((attempt, record["ttft_ms"].as_f64()? - attempt["started"].as_f64()?))
}

/// A read window: arrivals in `[from, to)`; `from` is `None` for all time (R2, R3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub from: Option<SystemTime>,
    pub to: SystemTime,
}

impl Window {
    /// Whether a record that arrived at `arrived` (RFC 3339) lies in the window. A time that
    /// doesn't parse is outside every window.
    pub fn holds(&self, arrived: &str) -> bool {
        clock::parse_rfc3339(arrived).is_some_and(|t| self.from.is_none_or(|f| f <= t) && t < self.to)
    }
}

/// The segment files a window can touch: every `records/YYYY-MM-DD.jsonl` whose UTC day lies in
/// `[day_of(from), day_of(to)]`, oldest first.
pub fn segments_for(home: &Path, w: &Window) -> Vec<(String, PathBuf)> {
    let first = w.from.map(clock::rfc3339);
    let last = clock::rfc3339(w.to);
    segments(home)
        .into_iter()
        .filter(|(day, _)| first.as_deref().is_none_or(|f| day.as_str() >= day_of(f)) && day.as_str() <= day_of(&last))
        .collect()
}

/// The records of the window's segments, folded, keeping those that arrived inside it.
pub fn records_in(home: &Path, w: &Window) -> Vec<Value> {
    let mut out = Vec::new();
    for (_, path) in segments_for(home, w) {
        let Ok(text) = fs::read_to_string(&path) else { continue };
        out.extend(fold(&text).into_iter().filter(|r| r["arrived"].as_str().is_some_and(|a| w.holds(a))));
    }
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn nearest_rank_is_a_real_observed_value() {
        let v = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0];
        assert_eq!(nearest_rank(&v, 0.5), Some(5.0));
        assert_eq!(nearest_rank(&v, 0.95), Some(10.0));
        let v: Vec<f64> = (1..=20).map(f64::from).collect();
        assert_eq!(nearest_rank(&v, 0.5), Some(10.0));
        assert_eq!(nearest_rank(&v, 0.95), Some(19.0));
        assert_eq!(nearest_rank(&[7.0], 0.5), Some(7.0));
        assert_eq!(nearest_rank(&[7.0], 0.95), Some(7.0));
        assert_eq!(nearest_rank(&[], 0.5), None);
    }

    #[test]
    fn tokens_take_cache_out_of_included_input_and_keep_writes_in_input() {
        let u = json!({"input": 1000, "output": 50, "cache_read": 600, "cache_write": 100, "reasoning": 20,
                       "input_semantics": "includes_cache"});
        assert_eq!(tokens_of(&u), Some(Tokens { input: 400, cached: 600, output: 50 }));
        let u = json!({"input": 300, "output": 50, "cache_read": 600, "cache_write": 100,
                       "input_semantics": "excludes_cache"});
        assert_eq!(tokens_of(&u), Some(Tokens { input: 400, cached: 600, output: 50 }));
        let u = json!({"input": 10, "output": null, "cache_read": null, "cache_write": null,
                       "input_semantics": "excludes_cache"});
        assert_eq!(tokens_of(&u), Some(Tokens { input: 10, cached: 0, output: 0 }));
        assert_eq!(tokens_of(&Value::Null), None);
    }

    fn rec(ttft: f64, attempts: Value) -> Value {
        json!({"ttft_ms": ttft, "attempts": attempts})
    }

    #[test]
    fn first_attempt_skips_skipped_ones() {
        let r = rec(0.0, json!([{"n": 0, "kind": "skipped", "started": 1.0}, {"n": 1, "kind": "initial", "started": 6.0}]));
        assert_eq!(first_attempt(&r).unwrap()["n"], 1);
        assert!(first_attempt(&rec(0.0, json!([{"kind": "skipped", "started": 1.0}]))).is_none());
        assert!(first_attempt(&json!({})).is_none());
    }

    #[test]
    fn the_first_token_attempt_is_the_one_running_when_it_arrived() {
        // A failed, B served: B's attempt started at 900 ms and the first token came at 1300.
        let r = rec(
            1300.0,
            json!([{"n": 1, "kind": "initial", "started": 5.0, "provider": "a"},
                   {"n": 2, "kind": "next_account", "started": 900.0, "provider": "b"}]),
        );
        let (a, own) = own_ttft(&r).unwrap();
        assert_eq!((a["provider"].as_str(), own), (Some("b"), 400.0));
        // A continuation after the first token: the first attempt keeps the credit.
        let r = rec(
            300.0,
            json!([{"n": 1, "kind": "initial", "started": 5.0, "provider": "a"},
                   {"n": 2, "kind": "continuation", "started": 4000.0, "provider": "b"}]),
        );
        assert_eq!(own_ttft(&r).map(|(a, o)| (a["provider"].as_str().unwrap().to_owned(), o)), Some(("a".into(), 295.0)));
        // A restart before any token, with a skipped attempt in between.
        let r = rec(
            2500.0,
            json!([{"n": 1, "kind": "initial", "started": 5.0, "provider": "a"},
                   {"n": 2, "kind": "skipped", "started": 2000.0, "provider": "c"},
                   {"n": 3, "kind": "restart", "started": 2100.0, "provider": "b"}]),
        );
        assert_eq!(own_ttft(&r).map(|(a, o)| (a["provider"].as_str().unwrap().to_owned(), o)), Some(("b".into(), 400.0)));
        // No first token, no value.
        assert!(own_ttft(&json!({"ttft_ms": null, "attempts": [{"kind": "initial", "started": 5.0}]})).is_none());
    }

    fn at(rfc: &str) -> SystemTime {
        clock::parse_rfc3339(rfc).unwrap()
    }

    fn home_with(days: &[(&str, &[&str])]) -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        fs::create_dir(home.path().join("records")).unwrap();
        for (day, arrivals) in days {
            let mut text = String::new();
            for (i, arrived) in arrivals.iter().enumerate() {
                text += &format!("{{\"t\":\"open\",\"id\":\"rq_{day}_{i}\",\"arrived\":\"{arrived}\"}}\n");
            }
            fs::write(home.path().join(format!("records/{day}.jsonl")), text).unwrap();
        }
        home
    }

    #[test]
    fn segments_for_returns_exactly_the_days_the_window_touches() {
        let home = home_with(&[("2026-10-03", &[]), ("2026-10-04", &[]), ("2026-10-05", &[]), ("2026-10-06", &[])]);
        let days = |w: Window| segments_for(home.path(), &w).into_iter().map(|(d, _)| d).collect::<Vec<_>>();
        let w = Window { from: Some(at("2026-10-04T23:00:00Z")), to: at("2026-10-05T01:00:00Z") };
        assert_eq!(days(w), ["2026-10-04", "2026-10-05"]);
        let w = Window { from: None, to: at("2026-10-05T00:00:00Z") };
        assert_eq!(days(w), ["2026-10-03", "2026-10-04", "2026-10-05"]);
        let w = Window { from: Some(at("2026-10-06T00:00:00Z")), to: at("2026-10-06T12:00:00Z") };
        assert_eq!(days(w), ["2026-10-06"]);
    }

    #[test]
    fn records_in_keeps_arrivals_in_the_window_across_a_utc_midnight() {
        let home = home_with(&[
            ("2026-10-04", &["2026-10-04T22:00:00Z", "2026-10-04T23:30:00Z"]),
            ("2026-10-05", &["2026-10-05T00:10:00Z", "2026-10-05T02:00:00Z"]),
        ]);
        let w = Window { from: Some(at("2026-10-04T23:00:00Z")), to: at("2026-10-05T02:00:00Z") };
        let ids: Vec<String> = records_in(home.path(), &w).iter().map(|r| r["id"].as_str().unwrap().to_owned()).collect();
        // `to` is exclusive, `from` inclusive.
        assert_eq!(ids, ["rq_2026-10-04_1", "rq_2026-10-05_0"]);
        let all = Window { from: None, to: at("2026-10-06T00:00:00Z") };
        assert_eq!(records_in(home.path(), &all).len(), 4);
    }
}
