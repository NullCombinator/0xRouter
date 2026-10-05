//! Routing state files: warm fingerprints and deficit ledgers, read at start and compacted
//! (spec 006, research R12, contracts/record-journal.md § Routing state lines).
//!
//! Appends go through the journal writer. This file only reads, and renders the lines a
//! compaction or a forget replaces a file with; the writer thread does the replacing, in order
//! with the appends, so a line is never lost between a snapshot and the rename.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};

use serde_json::{Value, json};

use crate::clock;
use crate::routing::fingerprint::Hash;
use crate::routing::warm::Stored;
use crate::routing::{CandidateKey, Tier};

/// One `ledger` line, parsed.
#[derive(Debug, Clone, PartialEq)]
pub struct LedgerLine {
    pub target: String,
    pub tier: Tier,
    pub window_start: SystemTime,
    pub deficits: BTreeMap<String, f64>,
}

/// What the routing files held.
#[derive(Debug, Default)]
pub struct Loaded {
    pub warm: Vec<Stored>,
    /// The last line per `(target, tier)`.
    pub ledgers: Vec<LedgerLine>,
}

fn text(home: &Path, file: &str) -> String {
    fs::read_to_string(home.join("routing").join(file)).unwrap_or_default()
}

/// Reads both files. The last `warm` line per identity and the last `ledger` line per
/// `(target, tier)` win; a line that doesn't parse is skipped.
pub fn load(home: &Path) -> Loaded {
    let mut warm: HashMap<(String, Hash, CandidateKey), Stored> = HashMap::new();
    for raw in text(home, "warm.jsonl").lines() {
        if let Some(s) = parse_warm(raw) {
            warm.insert((s.agent.clone(), s.hash, s.at.clone()), s);
        }
    }
    let mut ledgers: BTreeMap<(String, &'static str), LedgerLine> = BTreeMap::new();
    for raw in text(home, "ledger.jsonl").lines() {
        if let Some(l) = parse_ledger(raw) {
            ledgers.insert((l.target.clone(), tier_name(l.tier)), l);
        }
    }
    let mut warm: Vec<Stored> = warm.into_values().collect();
    warm.sort_by(|a, b| (&a.agent, a.hash, &a.at).cmp(&(&b.agent, b.hash, &b.at)));
    Loaded { warm, ledgers: ledgers.into_values().collect() }
}

pub fn tier_name(t: Tier) -> &'static str {
    if t == Tier::Payg { "payg" } else { "subscription" }
}

fn parse_warm(raw: &str) -> Option<Stored> {
    let v: Value = serde_json::from_str(raw).ok()?;
    if v["t"] != "warm" {
        return None;
    }
    Some(Stored {
        agent: v["agent"].as_str()?.to_owned(),
        hash: Hash::from_hex(v["hash"].as_str()?)?,
        at: CandidateKey::new(v["provider"].as_str()?, v["account"].as_str()?, v["model"].as_str()?),
        prefix_tokens: v["prefix_tokens"].as_u64()?,
        last_used: clock::parse_rfc3339(v["last_used"].as_str()?)?,
        ttl: v["ttl_s"].as_u64().map(Duration::from_secs),
    })
}

fn parse_ledger(raw: &str) -> Option<LedgerLine> {
    let v: Value = serde_json::from_str(raw).ok()?;
    if v["t"] != "ledger" {
        return None;
    }
    let tier = match v["tier"].as_str()? {
        "payg" => Tier::Payg,
        "subscription" => Tier::Subscription,
        _ => return None,
    };
    let deficits = v["deficits"].as_object()?.iter().filter_map(|(k, d)| Some((k.clone(), d.as_f64()?))).collect();
    Some(LedgerLine {
        target: v["target"].as_str()?.to_owned(),
        tier,
        window_start: clock::parse_rfc3339(v["window_start"].as_str()?)?,
        deficits,
    })
}

/// The `warm` line for a stored fingerprint.
pub fn warm_line(s: &Stored) -> Value {
    let mut line = json!({
        "v": 1,
        "t": "warm",
        "agent": s.agent,
        "hash": s.hash.hex(),
        "provider": s.at.provider,
        "account": s.at.account,
        "model": s.at.model,
        "prefix_tokens": s.prefix_tokens,
        "last_used": crate::quota::extract::rfc3339_millis(s.last_used),
    });
    if let Some(ttl) = s.ttl {
        line["ttl_s"] = json!(ttl.as_secs());
    }
    line
}

/// The `ledger` line for a ledger's standing.
pub fn ledger_line(
    target: &str,
    tier: Tier,
    window: SystemTime,
    deficits: &BTreeMap<String, f64>,
    at: SystemTime,
) -> Value {
    let deficits: BTreeMap<&String, i64> = deficits.iter().map(|(k, v)| (k, v.round() as i64)).collect();
    json!({
        "v": 1,
        "t": "ledger",
        "target": target,
        "tier": tier_name(tier),
        "window_start": crate::quota::extract::rfc3339_millis(window),
        "deficits": deficits,
        "at": crate::quota::extract::rfc3339_millis(at),
    })
}

/// Drops the lines of `agent` from `routing/warm.jsonl` while no server is running (the CLI's
/// `records forget --agent`). Same write-sync-rename as a compaction. Returns how many fingerprints.
pub fn forget_agent(home: &Path, agent: &str) -> std::io::Result<usize> {
    rewrite_warm(home, |s| s.agent == agent)
}

/// Drops the lines of `provider/account` from the warm file while no server is running.
pub fn forget_account(home: &Path, account: &str) -> std::io::Result<usize> {
    let gone = rewrite_warm(home, |s| s.at.account_key() == account)?;
    rewrite_ledger(home, account)?;
    Ok(gone)
}

/// Drops `account`'s deficits from `routing/ledger.jsonl`: a running server does the same in
/// memory (`route::drop_account`). A line left with no deficits goes; other lines stay as written.
fn rewrite_ledger(home: &Path, account: &str) -> std::io::Result<()> {
    let mut changed = false;
    let mut body = String::new();
    for raw in text(home, "ledger.jsonl").lines() {
        let edited = serde_json::from_str::<Value>(raw).ok().and_then(|mut v| {
            if v["t"] != "ledger" {
                return None;
            }
            v["deficits"].as_object_mut()?.remove(account)?;
            Some(v)
        });
        match edited {
            Some(v) => {
                changed = true;
                if v["deficits"].as_object().is_some_and(|d| !d.is_empty()) {
                    body += &(v.to_string() + "\n");
                }
            }
            None => body += &(raw.to_owned() + "\n"),
        }
    }
    if !changed {
        return Ok(());
    }
    let _lock = super::records::lock(home, super::records::LOCK_WAIT)?;
    super::writer::replace_file(&home.join("routing").join("ledger.jsonl"), &body)
}

fn rewrite_warm(home: &Path, drop: impl Fn(&Stored) -> bool) -> std::io::Result<usize> {
    let loaded = load(home);
    let (gone, kept): (Vec<_>, Vec<_>) = loaded.warm.into_iter().partition(|s| drop(s));
    if gone.is_empty() {
        return Ok(0);
    }
    let _lock = super::records::lock(home, super::records::LOCK_WAIT)?;
    let body: String = kept.iter().map(|s| warm_line(s).to_string() + "\n").collect();
    super::writer::replace_file(&home.join("routing").join("warm.jsonl"), &body)?;
    Ok(gone.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored(agent: &str, hash: u8, account: &str, used: &str) -> Stored {
        Stored {
            agent: agent.into(),
            hash: Hash([hash; 16]),
            at: CandidateKey::new("anthropic", account, "m"),
            prefix_tokens: 100,
            last_used: clock::parse_rfc3339(used).unwrap(),
            ttl: None,
        }
    }

    #[test]
    fn the_last_line_per_identity_wins_and_bad_lines_are_skipped() {
        let home = tempfile::tempdir().unwrap();
        fs::create_dir_all(home.path().join("routing")).unwrap();
        let a1 = stored("k", 1, "max", "2026-10-04T09:00:00Z");
        let a2 = stored("k", 1, "max", "2026-10-04T09:30:00Z");
        let b = stored("k", 2, "pro", "2026-10-04T09:10:00Z");
        let body = format!(
            "{}\nnot json\n{{\"v\":1,\"t\":\"warm\"}}\n{}\n{}\n",
            warm_line(&a1),
            warm_line(&b),
            warm_line(&a2)
        );
        fs::write(home.path().join("routing/warm.jsonl"), body).unwrap();
        let loaded = load(home.path());
        assert_eq!(loaded.warm.len(), 2);
        let max = loaded.warm.iter().find(|s| s.at.account == "max").unwrap();
        assert_eq!(max.last_used, a2.last_used);
    }

    #[test]
    fn ledger_lines_round_trip_and_the_last_per_target_and_tier_wins() {
        let home = tempfile::tempdir().unwrap();
        fs::create_dir_all(home.path().join("routing")).unwrap();
        let w = clock::parse_rfc3339("2026-10-04T05:00:00Z").unwrap();
        let at = clock::parse_rfc3339("2026-10-04T09:00:00Z").unwrap();
        let d = |a: f64| BTreeMap::from([("anthropic/max".to_owned(), a), ("anthropic/pro".to_owned(), -a)]);
        let body = [
            ledger_line("sonnet", Tier::Subscription, w, &d(10.0), at),
            ledger_line("sonnet", Tier::Subscription, w, &d(8800.4), at),
            ledger_line("sonnet", Tier::Payg, w, &d(1.0), at),
        ]
        .iter()
        .map(|l| l.to_string() + "\n")
        .collect::<String>();
        fs::write(home.path().join("routing/ledger.jsonl"), body).unwrap();
        let loaded = load(home.path());
        assert_eq!(loaded.ledgers.len(), 2);
        let sub = loaded.ledgers.iter().find(|l| l.tier == Tier::Subscription).unwrap();
        assert_eq!(sub.window_start, w);
        assert_eq!(sub.deficits["anthropic/max"], 8800.0);
        assert_eq!(sub.deficits["anthropic/pro"], -8800.0);
    }

    #[test]
    fn forgetting_an_agent_or_account_rewrites_the_warm_file() {
        let home = tempfile::tempdir().unwrap();
        fs::create_dir_all(home.path().join("routing")).unwrap();
        let body: String = [
            stored("k1", 1, "max", "2026-10-04T09:00:00Z"),
            stored("k2", 2, "max", "2026-10-04T09:00:00Z"),
            stored("k2", 3, "pro", "2026-10-04T09:00:00Z"),
        ]
        .iter()
        .map(|s| warm_line(s).to_string() + "\n")
        .collect();
        fs::write(home.path().join("routing/warm.jsonl"), body).unwrap();
        assert_eq!(forget_agent(home.path(), "k1").unwrap(), 1);
        assert_eq!(forget_account(home.path(), "anthropic/pro").unwrap(), 1);
        let left = load(home.path()).warm;
        assert_eq!(left.len(), 1);
        assert_eq!((left[0].agent.as_str(), left[0].at.account.as_str()), ("k2", "max"));
        assert_eq!(forget_agent(home.path(), "nobody").unwrap(), 0);
    }

    #[test]
    fn forgetting_an_account_also_drops_its_ledger_entries() {
        let home = tempfile::tempdir().unwrap();
        fs::create_dir_all(home.path().join("routing")).unwrap();
        let w = clock::parse_rfc3339("2026-10-04T05:00:00Z").unwrap();
        let at = clock::parse_rfc3339("2026-10-04T09:00:00Z").unwrap();
        let both = BTreeMap::from([("anthropic/max".to_owned(), 5.0), ("anthropic/pro".to_owned(), -5.0)]);
        let only = BTreeMap::from([("anthropic/pro".to_owned(), 3.0)]);
        let body: String = [
            ledger_line("sonnet", Tier::Subscription, w, &both, at),
            ledger_line("haiku", Tier::Subscription, w, &only, at),
        ]
        .iter()
        .map(|l| l.to_string() + "\n")
        .collect();
        fs::write(home.path().join("routing/ledger.jsonl"), body).unwrap();
        forget_account(home.path(), "anthropic/pro").unwrap();
        let left = load(home.path()).ledgers;
        assert_eq!(left.len(), 1, "a line with no deficits left goes");
        assert_eq!(left[0].target, "sonnet");
        assert_eq!(left[0].deficits.keys().collect::<Vec<_>>(), ["anthropic/max"]);
        assert!(!fs::read_to_string(home.path().join("routing/ledger.jsonl")).unwrap().contains("anthropic/pro"));
    }
}
