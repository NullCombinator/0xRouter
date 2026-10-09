//! Summaries over the record journal: totals and Est. Cost for a window, latency per agent and
//! per provider (spec 010, research R3 to R6). Reads only the day segments a window needs and
//! writes nothing.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use serde::Serialize;
use serde_json::Value;

use crate::accounts::Accounts;
use crate::clock;
use crate::journal::records::{day_of, fold, segments};
use crate::routing::price::{PriceSpec, entry_at};
use nullrouter_registry::Registry;

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

/// A record's usage split the way pricing needs it: input with neither cache count in it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Split {
    plain: u64,
    read: u64,
    write: u64,
    output: u64,
}

fn split_of(usage: &Value) -> Option<Split> {
    if usage.is_null() {
        return None;
    }
    let n = |k: &str| usage[k].as_u64().unwrap_or(0);
    let (read, write) = (n("cache_read"), n("cache_write"));
    let plain =
        if usage["input_semantics"] == "includes_cache" { n("input").saturating_sub(read + write) } else { n("input") };
    Some(Split { plain, read, write, output: n("output") })
}

/// The tokens of a record's `usage` (`None` when it reported none). `includes_cache` input has
/// the cache reads and writes taken out and the writes added back, so writes stay in input;
/// `excludes_cache` input gets the writes added. Reasoning tokens are part of `output` already.
pub fn tokens_of(usage: &Value) -> Option<Tokens> {
    let s = split_of(usage)?;
    Some(Tokens { input: s.plain + s.write, cached: s.read, output: s.output })
}

fn is_skipped(attempt: &Value) -> bool {
    attempt["kind"] == "skipped"
}

/// The first attempt that wasn't skipped.
pub fn first_attempt(record: &Value) -> Option<&Value> {
    record["attempts"].as_array()?.iter().find(|a| !is_skipped(a))
}

/// The router's overhead: the first attempt's router-overhead phase (`phases::of`), which is
/// when it started less any sign-in refresh and retry wait before it. A record from before
/// phase timing has neither, so its overhead stays when that attempt started.
pub fn router_overhead(record: &Value) -> Option<f64> {
    let a = first_attempt(record)?;
    let t = &a["timing"];
    let spent = t["refresh_ms"].as_f64().unwrap_or(0.0) + t["retry_wait_ms"].as_f64().unwrap_or(0.0);
    Some((a["started"].as_f64()? - spent).max(0.0))
}

/// The attempt that was running when the first token reached the client: the last non-skipped
/// attempt with `started <= ttft_ms`. A break comes after the first token, so this holds for
/// fallbacks, continuations and restarts alike.
pub fn first_token_attempt(record: &Value) -> Option<&Value> {
    let ttft = record["ttft_ms"].as_f64()?;
    record["attempts"]
        .as_array()?
        .iter()
        .filter(|a| !is_skipped(a))
        .rfind(|a| a["started"].as_f64().is_some_and(|s| s <= ttft))
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

/// Why a finished record's tokens are left out of Est. Cost (R5).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Unpriced {
    pub no_price: u64,
    pub account_gone: u64,
    pub no_output_price: u64,
}

/// Request and token totals for a window. They add up across segments, which is what lets the
/// finished days be cached.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Totals {
    pub requests: u64,
    pub in_flight: u64,
    pub not_reported: u64,
    pub input: u64,
    pub cached: u64,
    pub output: u64,
    pub cost_usd: f64,
    pub unpriced: Unpriced,
    pub agents: BTreeMap<String, u64>,
    pub providers: BTreeMap<String, u64>,
}

impl Totals {
    pub fn add(&mut self, o: &Totals) {
        self.requests += o.requests;
        self.in_flight += o.in_flight;
        self.not_reported += o.not_reported;
        self.input += o.input;
        self.cached += o.cached;
        self.output += o.output;
        self.cost_usd += o.cost_usd;
        self.unpriced.no_price += o.unpriced.no_price;
        self.unpriced.account_gone += o.unpriced.account_gone;
        self.unpriced.no_output_price += o.unpriced.no_output_price;
        for (k, v) in &o.agents {
            *self.agents.entry(k.clone()).or_default() += v;
        }
        for (k, v) in &o.providers {
            *self.providers.entry(k.clone()).or_default() += v;
        }
    }
}

/// What an account is priced by: `None` when the account no longer exists, an empty spec when
/// it exists with no price. The second argument is the account name (`None` for a provider that
/// has none).
pub type Prices<'a> = &'a dyn Fn(&str, Option<&str>) -> Option<PriceSpec>;

/// The price lookup of a registry and the accounts, built as `route.rs` builds a `PriceSpec`: the
/// provider's declared schedule, replaced by the account's flat override.
pub fn prices_of<'a>(
    registry: &'a Registry,
    accounts: &'a Accounts,
) -> impl Fn(&str, Option<&str>) -> Option<PriceSpec> + 'a {
    move |provider, account| {
        let entity = registry.provider(provider).ok()?;
        let schedule = entity.routing().prices.to_vec();
        match account {
            None => Some(PriceSpec { schedule, flat: None }),
            Some(name) => {
                let a = accounts.get(provider, name)?;
                Some(PriceSpec { schedule, flat: a.routing.price })
            }
        }
    }
}

fn add_record(t: &mut Totals, r: &Value, prices: Prices<'_>, running: bool) {
    t.requests += 1;
    if let Some(a) = r["agent"]["key"].as_str() {
        *t.agents.entry(a.to_owned()).or_default() += 1;
    }
    let mut seen: Vec<&str> = Vec::new();
    for a in r["attempts"].as_array().into_iter().flatten().filter(|a| !is_skipped(a)) {
        if let Some(p) = a["provider"].as_str()
            && !seen.contains(&p)
        {
            seen.push(p);
            *t.providers.entry(p.to_owned()).or_default() += 1;
        }
    }
    // An unfinished request is in flight with a server, and cut short (not reported) without one,
    // unless it is a submitted job, which stays in progress.
    if r["outcome"] == "in_progress" && (running || !r["job"].is_null()) {
        t.in_flight += 1;
        return;
    }
    let Some(s) = split_of(&r["usage"]) else {
        t.not_reported += 1;
        return;
    };
    t.input += s.plain + s.write;
    t.cached += s.read;
    t.output += s.output;
    let served = &r["served_by"];
    let Some(provider) = served["provider"].as_str() else {
        t.unpriced.account_gone += 1;
        return;
    };
    let Some(spec) = prices(provider, served["account"].as_str()) else {
        t.unpriced.account_gone += 1;
        return;
    };
    let at = r["arrived"].as_str().and_then(clock::parse_rfc3339).unwrap_or(SystemTime::UNIX_EPOCH);
    let Some(rates) = entry_at(&spec, at) else {
        t.unpriced.no_price += 1;
        return;
    };
    if s.output > 0 && rates.output.is_none() {
        t.unpriced.no_output_price += 1;
        return;
    }
    // Cache tokens with no rate of their own cost the input rate, as 9router does.
    let usd = s.plain as f64 * rates.input
        + s.read as f64 * rates.cache_read.unwrap_or(rates.input)
        + s.write as f64 * rates.cache_write.unwrap_or(rates.input)
        + s.output as f64 * rates.output.unwrap_or(0.0);
    t.cost_usd += usd / 1e6;
}

fn segment_totals(path: &Path, w: &Window, prices: Prices<'_>, skip: &HashSet<&str>, running: bool) -> Totals {
    let mut t = Totals::default();
    let Ok(text) = fs::read_to_string(path) else { return t };
    let wanted = |r: &&Value| {
        r["arrived"].as_str().is_some_and(|a| w.holds(a)) && r["id"].as_str().is_none_or(|id| !skip.contains(id))
    };
    for r in fold(&text).iter().filter(wanted) {
        add_record(&mut t, r, prices, running);
    }
    t
}

/// What a cached day was computed from: any change recomputes it (R3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    inode: u64,
    len: u64,
    generation: u64,
}

/// Finished days' totals, one entry per segment file. Process-wide, like the segment index.
static CACHE: Mutex<Option<HashMap<PathBuf, (Stamp, Totals)>>> = Mutex::new(None);

/// Totals over the window, computed from the journal alone and from no cache. `running` says
/// whether a server runs: without one an unfinished request was cut short, not in flight.
pub fn totals(home: &Path, w: &Window, prices: Prices<'_>, running: bool) -> Totals {
    totals_with(home, w, prices, None, &[], running)
}

/// Totals over the window with the requests the server still holds in memory. `live` records
/// replace their copies in the journal (they are the fresher ones), so they are left out of the
/// segment reads and added from memory; a day that has one is read, not cached.
///
/// A day that lies wholly inside the window is served from the cache, keyed by the file's inode
/// and length and by `generation` (the engine's reload generation, since a reload can change
/// prices or accounts). `generation: None` reads everything.
pub fn totals_with(
    home: &Path,
    w: &Window,
    prices: Prices<'_>,
    generation: Option<u64>,
    live: &[Value],
    running: bool,
) -> Totals {
    let live: Vec<&Value> = live.iter().filter(|r| r["arrived"].as_str().is_some_and(|a| w.holds(a))).collect();
    let skip: HashSet<&str> = live.iter().filter_map(|r| r["id"].as_str()).collect();
    let mut out = Totals::default();
    for (day, path) in segments_for(home, w) {
        let start = clock::parse_rfc3339(&format!("{day}T00:00:00Z"));
        let whole = start.is_some_and(|s| w.from.is_none_or(|f| f <= s) && s + Duration::from_secs(86_400) <= w.to);
        let has_live = live.iter().any(|r| r["arrived"].as_str().is_some_and(|a| day_of(a) == day));
        let stamp = generation.filter(|_| whole && !has_live).and_then(|generation| {
            let m = fs::metadata(&path).ok()?;
            Some(Stamp { inode: m.ino(), len: m.len(), generation })
        });
        let Some(stamp) = stamp else {
            out.add(&segment_totals(&path, w, prices, &skip, running));
            continue;
        };
        let hit =
            CACHE.lock().ok().and_then(|c| c.as_ref()?.get(&path).filter(|(s, _)| *s == stamp).map(|(_, t)| t.clone()));
        let day_totals = hit.unwrap_or_else(|| {
            let t = segment_totals(&path, w, prices, &skip, running);
            if let Ok(mut c) = CACHE.lock() {
                c.get_or_insert_with(HashMap::new).insert(path.clone(), (stamp, t.clone()));
            }
            t
        });
        out.add(&day_totals);
    }
    let mut live_totals = Totals::default();
    for r in live {
        add_record(&mut live_totals, r, prices, running);
    }
    out.add(&live_totals);
    out
}

/// Percentiles of one set of values, in milliseconds (R6). `n` is how many values there were.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Pct {
    pub p50: f64,
    pub p95: f64,
    pub n: u64,
}

/// `None` when there are no values: a row says so, and never shows 0.
fn pct(mut v: Vec<f64>) -> Option<Pct> {
    v.sort_by(f64::total_cmp);
    Some(Pct { p50: nearest_rank(&v, 0.5)?, p95: nearest_rank(&v, 0.95)?, n: v.len() as u64 })
}

/// How the newest response ended and when; `status` is the HTTP status of a failed provider
/// attempt, when it had one.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Last {
    pub result: &'static str,
    pub at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct AgentLatency {
    pub requests: u64,
    pub overhead: Option<Pct>,
    pub ttft: Option<Pct>,
    pub last: Option<Last>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ProviderLatency {
    pub requests: u64,
    pub own_ttft: Option<Pct>,
    pub agents: BTreeMap<String, u64>,
    pub last: Option<Last>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Latency {
    pub agents: BTreeMap<String, AgentLatency>,
    pub providers: BTreeMap<String, ProviderLatency>,
}

/// `arrived` plus `ms` milliseconds.
fn after(arrived: &str, ms: f64) -> Option<SystemTime> {
    Some(clock::parse_rfc3339(arrived)? + Duration::from_secs_f64(ms.max(0.0) / 1000.0))
}

/// Keeps the newer of two responses.
fn newer(slot: &mut Option<(SystemTime, Last)>, at: SystemTime, result: &'static str, status: Option<u64>) {
    if slot.as_ref().is_none_or(|(t, _)| at > *t) {
        *slot = Some((at, Last { result, at: clock::rfc3339(at), status }));
    }
}

#[derive(Default)]
struct AgentAcc {
    requests: u64,
    overhead: Vec<f64>,
    ttft: Vec<f64>,
    last: Option<(SystemTime, Last)>,
}

#[derive(Default)]
struct ProviderAcc {
    requests: u64,
    own: Vec<f64>,
    agents: BTreeMap<String, u64>,
    last: Option<(SystemTime, Last)>,
}

/// Latency per agent and per provider over the window's records that have an agent (FR-012),
/// the journal's merged with the server's live ones (a live record replaces its disk copy).
/// Never cached. `running` says whether a server runs, as for [`totals`].
pub fn latency(home: &Path, w: &Window, live: &[Value], running: bool) -> Latency {
    let skip: HashSet<&str> = live.iter().filter_map(|r| r["id"].as_str()).collect();
    let mut records = records_in(home, w);
    records.retain(|r| r["id"].as_str().is_none_or(|id| !skip.contains(id)));
    records.extend(live.iter().filter(|r| r["arrived"].as_str().is_some_and(|a| w.holds(a))).cloned());

    let mut agents: BTreeMap<String, AgentAcc> = BTreeMap::new();
    let mut providers: BTreeMap<String, ProviderAcc> = BTreeMap::new();
    for r in &records {
        let Some(agent) = r["agent"]["key"].as_str() else { continue };
        let arrived = r["arrived"].as_str().unwrap_or_default();
        let a = agents.entry(agent.to_owned()).or_default();
        a.requests += 1;
        if let Some(overhead) = router_overhead(r) {
            a.overhead.push(overhead);
        }
        if let Some(t) = r["ttft_ms"].as_f64() {
            a.ttft.push(t);
        }
        // A request cut short without a server is interrupted; a job in progress isn't.
        let outcome = match r["outcome"].as_str() {
            Some("in_progress") if !running && r["job"].is_null() => Some("interrupted"),
            o => o,
        };
        let result = match outcome {
            Some("succeeded") => Some("resolved"),
            Some("failed" | "refused" | "interrupted") => Some("failed"),
            _ => None,
        };
        if let Some(result) = result
            && let Some(at) = after(arrived, r["total_ms"].as_f64().unwrap_or(0.0))
        {
            newer(&mut a.last, at, result, None);
        }

        let mut seen: Vec<&str> = Vec::new();
        for att in r["attempts"].as_array().into_iter().flatten().filter(|x| !is_skipped(x)) {
            let Some(p) = att["provider"].as_str() else { continue };
            let acc = providers.entry(p.to_owned()).or_default();
            if !seen.contains(&p) {
                seen.push(p);
                acc.requests += 1;
                *acc.agents.entry(agent.to_owned()).or_default() += 1;
            }
            let (result, status) = match att["outcome"]["state"].as_str() {
                Some("ok") => ("resolved", None),
                Some("failed") => ("failed", att["outcome"]["status"].as_u64()),
                _ => continue,
            };
            if let Some(at) = att["ended"].as_f64().and_then(|e| after(arrived, e)) {
                newer(&mut acc.last, at, result, status);
            }
        }
        if let Some((att, own)) = own_ttft(r)
            && let Some(p) = att["provider"].as_str()
        {
            providers.entry(p.to_owned()).or_default().own.push(own);
        }
    }
    Latency {
        agents: agents
            .into_iter()
            .map(|(k, a)| {
                let row = AgentLatency {
                    requests: a.requests,
                    overhead: pct(a.overhead),
                    ttft: pct(a.ttft),
                    last: a.last.map(|(_, l)| l),
                };
                (k, row)
            })
            .collect(),
        providers: providers
            .into_iter()
            .map(|(k, p)| {
                let row = ProviderLatency {
                    requests: p.requests,
                    own_ttft: pct(p.own),
                    agents: p.agents,
                    last: p.last.map(|(_, l)| l),
                };
                (k, row)
            })
            .collect(),
    }
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
        let r =
            rec(0.0, json!([{"n": 0, "kind": "skipped", "started": 1.0}, {"n": 1, "kind": "initial", "started": 6.0}]));
        assert_eq!(first_attempt(&r).unwrap()["n"], 1);
        assert!(first_attempt(&rec(0.0, json!([{"kind": "skipped", "started": 1.0}]))).is_none());
        assert!(first_attempt(&json!({})).is_none());
    }

    #[test]
    fn router_overhead_leaves_out_a_refresh_and_a_retry_wait() {
        let r = rec(0.0, json!([{"kind": "skipped", "started": 1.0}, {"kind": "initial", "started": 9.0}]));
        assert_eq!(router_overhead(&r), Some(9.0));
        let timed = json!([{"kind": "initial", "started": 9.0, "timing": {"refresh_ms": 3.0, "retry_wait_ms": 4.0}}]);
        assert_eq!(router_overhead(&rec(0.0, timed)), Some(2.0));
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
        assert_eq!(
            own_ttft(&r).map(|(a, o)| (a["provider"].as_str().unwrap().to_owned(), o)),
            Some(("a".into(), 295.0))
        );
        // A restart before any token, with a skipped attempt in between.
        let r = rec(
            2500.0,
            json!([{"n": 1, "kind": "initial", "started": 5.0, "provider": "a"},
                   {"n": 2, "kind": "skipped", "started": 2000.0, "provider": "c"},
                   {"n": 3, "kind": "restart", "started": 2100.0, "provider": "b"}]),
        );
        assert_eq!(
            own_ttft(&r).map(|(a, o)| (a["provider"].as_str().unwrap().to_owned(), o)),
            Some(("b".into(), 400.0))
        );
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
        let ids: Vec<String> =
            records_in(home.path(), &w).iter().map(|r| r["id"].as_str().unwrap().to_owned()).collect();
        // `to` is exclusive, `from` inclusive.
        assert_eq!(ids, ["rq_2026-10-04_1", "rq_2026-10-05_0"]);
        let all = Window { from: None, to: at("2026-10-06T00:00:00Z") };
        assert_eq!(records_in(home.path(), &all).len(), 4);
    }
}
