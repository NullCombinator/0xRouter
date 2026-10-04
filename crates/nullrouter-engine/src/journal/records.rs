//! Request record segments: the lines a record becomes, and reading, recovering, pruning and
//! forgetting them (spec 006, research R11, contracts/record-journal.md). Readers need no server
//! and tolerate lines that don't parse.
//!
//! A segment is one UTC day of arrivals, so every line of a request is in one file. Records are
//! read as JSON in the shape `serde_json::to_value(&RequestRecord)` gives, which is what the
//! operator socket and the CLI already print.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use serde_json::{Map, Value, json};

use crate::clock;
use crate::journal::writer::sync_dir;
use crate::records::{Outcome, RequestRecord};

/// The advisory lock the writer takes per batch and a rewrite takes for its whole run.
pub const LOCK_FILE: &str = "records.lock";
/// How long a rewrite waits for the lock (contract: 10 s).
pub const LOCK_WAIT: Duration = Duration::from_secs(10);

const OPEN_KEYS: [&str; 6] = ["style", "op", "model_type", "target", "unified_model", "agent"];

/// The lines a change to a record needs, in the order they apply. `before` is what the journal
/// already holds; `force_open` writes the `open` line even if nothing in it changed.
pub fn lines_for(before: &RequestRecord, after: &RequestRecord, force_open: bool) -> Vec<(&'static str, Value)> {
    let mut out = Vec::new();
    let opened = before.agent != after.agent
        || before.style != after.style
        || before.op != after.op
        || before.model_type != after.model_type
        || before.target != after.target
        || before.unified_model != after.unified_model;
    if force_open || opened {
        out.push(("open", open_line(after)));
    }
    if after.decision.is_some() && after.decision != before.decision {
        out.push(("decision", json!({"id": after.id, "decision": after.decision})));
    }
    for (i, a) in after.attempts.iter().enumerate() {
        if before.attempts.get(i) != Some(a) {
            out.push(("attempt", json!({"id": after.id, "attempt": a})));
        }
    }
    let closed = before.served_by != after.served_by
        || before.outcome != after.outcome
        || before.usage != after.usage
        || before.ttft_ms != after.ttft_ms
        || before.total_ms != after.total_ms
        || before.break_handling != after.break_handling
        || before.job != after.job;
    // A job that was submitted is in progress until it is done, but its reference must survive.
    if closed && (after.outcome != Outcome::InProgress || after.job.is_some()) {
        out.push(("close", close_line(after)));
    }
    out
}

fn open_line(r: &RequestRecord) -> Value {
    let mut line = json!({
        "id": r.id,
        "arrived": r.arrived,
        "style": r.style,
        "op": r.op,
        "type": r.model_type,
        "target": r.target,
        "unified_model": r.unified_model,
    });
    if let Some(a) = &r.agent {
        line["agent"] = json!(a.key);
        if let Some(s) = &a.session {
            line["session"] = json!(s);
        }
    }
    line
}

fn close_line(r: &RequestRecord) -> Value {
    json!({
        "id": r.id,
        "outcome": r.outcome,
        "served_by": r.served_by,
        "ttft_ms": r.ttft_ms,
        "total_ms": r.total_ms,
        "usage": r.usage,
        "break_handling": r.break_handling,
        "job": r.job,
    })
}

/// The segment day (`YYYY-MM-DD`) of a record that arrived at `arrived` (RFC 3339).
pub fn day_of(arrived: &str) -> &str {
    arrived.get(..10).unwrap_or(arrived)
}

// ---------------------------------------------------------------------------------------------
// Folding

/// A request being rebuilt from its lines.
struct Folding {
    record: Map<String, Value>,
    attempts: BTreeMap<u64, Value>,
}

impl Folding {
    fn new(open: &Value) -> Self {
        let mut record = Map::new();
        for (k, v) in [
            ("id", open["id"].clone()),
            ("arrived", open["arrived"].clone()),
            ("agent", Value::Null),
            ("style", Value::Null),
            ("op", Value::Null),
            ("model_type", Value::Null),
            ("target", Value::Null),
            ("unified_model", Value::Null),
            ("served_by", Value::Null),
            ("attempts", json!([])),
            ("outcome", json!("in_progress")),
            ("break_handling", json!({"kind": "none"})),
            ("ttft_ms", Value::Null),
            ("total_ms", Value::Null),
            ("usage", Value::Null),
            ("job", Value::Null),
            ("decision", Value::Null),
        ] {
            record.insert(k.into(), v);
        }
        let mut f = Self { record, attempts: BTreeMap::new() };
        f.open(open);
        f
    }

    fn open(&mut self, line: &Value) {
        for key in OPEN_KEYS {
            let from = if key == "model_type" { "type" } else { key };
            let value = match key {
                "agent" => match line.get("agent").and_then(Value::as_str) {
                    Some(k) => json!({"key": k, "session": line.get("session").cloned().unwrap_or(Value::Null)}),
                    None => Value::Null,
                },
                _ => line.get(from).cloned().unwrap_or(Value::Null),
            };
            self.record.insert(key.into(), value);
        }
    }

    fn apply(&mut self, t: &str, line: &Value) {
        match t {
            "open" => self.open(line),
            "decision" => {
                self.record.insert("decision".into(), line.get("decision").cloned().unwrap_or(Value::Null));
            }
            "attempt" => {
                if let Some(a) = line.get("attempt")
                    && let Some(n) = a.get("n").and_then(Value::as_u64)
                {
                    self.attempts.insert(n, a.clone());
                }
            }
            "close" => {
                for key in ["outcome", "served_by", "ttft_ms", "total_ms", "usage", "break_handling", "job", "recovered_at"] {
                    if let Some(v) = line.get(key) {
                        self.record.insert(key.into(), v.clone());
                    }
                }
            }
            _ => {}
        }
    }

    fn finish(mut self) -> Value {
        self.record.insert("attempts".into(), Value::Array(self.attempts.into_values().collect()));
        Value::Object(self.record)
    }
}

/// Folds the lines of one segment into records, in the order their `open` lines appear. A line
/// that doesn't parse, has no id, or belongs to a request whose `open` isn't here is skipped.
pub fn fold(text: &str) -> Vec<Value> {
    let mut order: Vec<String> = Vec::new();
    let mut open: BTreeMap<String, Folding> = BTreeMap::new();
    for raw in text.lines() {
        let Ok(line) = serde_json::from_str::<Value>(raw) else { continue };
        let (Some(t), Some(id)) = (line["t"].as_str(), line["id"].as_str()) else { continue };
        if t == "open" {
            match open.get_mut(id) {
                Some(f) => f.apply(t, &line),
                None => {
                    order.push(id.to_owned());
                    open.insert(id.to_owned(), Folding::new(&line));
                }
            }
        } else if let Some(f) = open.get_mut(id) {
            f.apply(t, &line);
        }
    }
    order.into_iter().filter_map(|id| open.remove(&id)).map(Folding::finish).collect()
}

// ---------------------------------------------------------------------------------------------
// Reading

/// `records/YYYY-MM-DD.jsonl` under `home`, oldest first.
pub fn segments(home: &Path) -> Vec<(String, PathBuf)> {
    let Ok(dir) = fs::read_dir(home.join("records")) else { return Vec::new() };
    let mut out: Vec<(String, PathBuf)> = dir
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let day = name.strip_suffix(".jsonl")?;
            (day.len() == 10 && day.as_bytes()[4] == b'-' && day.as_bytes()[7] == b'-').then(|| (day.to_owned(), e.path()))
        })
        .collect();
    out.sort();
    out
}

/// What `records list` filters by; every part is optional.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    pub provider: Option<String>,
    /// `provider/account`.
    pub account: Option<String>,
    pub agent: Option<String>,
    /// The target the client named, the unified model it resolved to, or the model that served it.
    pub model: Option<String>,
    /// A placement reason (`cold_by_deficit`, `warm_stay`, …) of any attempt.
    pub reason: Option<String>,
    pub since: Option<SystemTime>,
    pub unified_model: Option<String>,
    pub limit: Option<usize>,
}

impl Filter {
    pub fn matches(&self, r: &Value) -> bool {
        let attempts = r["attempts"].as_array().map(Vec::as_slice).unwrap_or_default();
        let touches = |want: &dyn Fn(&str, Option<&str>) -> bool| {
            attempts
                .iter()
                .any(|a| want(a["provider"].as_str().unwrap_or(""), a["account"].as_str()))
                || r["served_by"]["provider"].as_str().is_some_and(|p| want(p, r["served_by"]["account"].as_str()))
        };
        if let Some(p) = &self.provider
            && !touches(&|prov, _| prov == p)
        {
            return false;
        }
        if let Some(want) = &self.account {
            let (p, n) = want.split_once('/').map_or((None, want.as_str()), |(p, n)| (Some(p), n));
            if !touches(&|prov, acct| acct == Some(n) && p.is_none_or(|p| p == prov)) {
                return false;
            }
        }
        if let Some(k) = &self.agent
            && r["agent"]["key"].as_str() != Some(k)
        {
            return false;
        }
        if let Some(m) = &self.model {
            let named = r["target"].as_str() == Some(m)
                || r["unified_model"].as_str() == Some(m)
                || r["served_by"]["model"].as_str() == Some(m)
                || attempts.iter().any(|a| a["model"].as_str() == Some(m));
            if !named {
                return false;
            }
        }
        if let Some(u) = &self.unified_model
            && r["unified_model"].as_str() != Some(u)
        {
            return false;
        }
        if let Some(reason) = &self.reason
            && !attempts.iter().any(|a| a["placement"]["reason"].as_str() == Some(reason))
        {
            return false;
        }
        if let Some(since) = self.since {
            let at = r["arrived"].as_str().and_then(clock::parse_rfc3339);
            if at.is_none_or(|at| at < since) {
                return false;
            }
        }
        true
    }
}

/// The records matching `filter`, newest first, read from the segments newest first. Reading
/// stops at the limit.
pub fn read(home: &Path, filter: &Filter) -> Vec<Value> {
    let limit = filter.limit.unwrap_or(usize::MAX);
    let mut out = Vec::new();
    for (_, path) in segments(home).into_iter().rev() {
        if out.len() >= limit {
            break;
        }
        let Ok(text) = fs::read_to_string(&path) else { continue };
        let mut records = fold(&text);
        records.sort_by(|a, b| b["id"].as_str().cmp(&a["id"].as_str()));
        out.extend(records.into_iter().filter(|r| filter.matches(r)).take(limit - out.len()));
    }
    out
}

/// One record by id; the id's ULID gives no day, so the segments are searched newest first.
pub fn get(home: &Path, id: &str) -> Option<Value> {
    for (_, path) in segments(home).into_iter().rev() {
        let Ok(text) = fs::read_to_string(&path) else { continue };
        // Cheap test before folding the whole segment.
        if !text.contains(id) {
            continue;
        }
        if let Some(r) = fold(&text).into_iter().find(|r| r["id"].as_str() == Some(id)) {
            return Some(r);
        }
    }
    None
}

// ---------------------------------------------------------------------------------------------
// Recovery

/// Makes the newest two segments whole after a crash: a torn final line is cut off, and every
/// request that has no `close` gets one saying it was interrupted. Returns how many. Takes the
/// journal lock; run it before the writer takes requests.
pub fn recover(home: &Path, now: SystemTime) -> io::Result<usize> {
    let all = segments(home);
    let newest = &all[all.len().saturating_sub(2)..];
    if newest.is_empty() {
        return Ok(0);
    }
    let _lock = lock(home, LOCK_WAIT)?;
    let mut interrupted = 0;
    for (_, path) in newest {
        make_whole(path)?;
        let text = fs::read_to_string(path)?;
        let lines: Vec<String> = fold(&text)
            .into_iter()
            .filter(|r| r["outcome"] == "in_progress" && r["job"].is_null())
            .map(|r| {
                json!({"v": 1, "t": "close", "id": r["id"], "outcome": "interrupted", "recovered_at": clock::rfc3339(now)})
                    .to_string()
            })
            .collect();
        if lines.is_empty() {
            continue;
        }
        interrupted += lines.len();
        let mut f = OpenOptions::new().append(true).open(path)?;
        f.write_all((lines.join("\n") + "\n").as_bytes())?;
        f.sync_data()?;
    }
    Ok(interrupted)
}

/// Cuts a segment back to its last whole line. A final line without its newline that is still a
/// whole JSON object only gets the newline.
fn make_whole(path: &Path) -> io::Result<()> {
    let bytes = fs::read(path)?;
    if bytes.is_empty() || bytes.last() == Some(&b'\n') {
        return Ok(());
    }
    let start = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
    let whole = serde_json::from_slice::<Value>(&bytes[start..]).is_ok_and(|v| v.is_object());
    let f = OpenOptions::new().write(true).append(false).open(path)?;
    if whole {
        drop(f);
        let mut f = OpenOptions::new().append(true).open(path)?;
        f.write_all(b"\n")?;
        f.sync_data()
    } else {
        f.set_len(start as u64)?;
        f.sync_data()
    }
}

// ---------------------------------------------------------------------------------------------
// Prune and forget

/// The advisory lock, waiting up to `wait`. `TimedOut` when someone else holds it.
pub fn lock(home: &Path, wait: Duration) -> io::Result<File> {
    let f = OpenOptions::new().append(true).create(true).mode(0o600).open(home.join(LOCK_FILE))?;
    let deadline = Instant::now() + wait;
    loop {
        match f.try_lock() {
            Ok(()) => return Ok(f),
            Err(fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(fs::TryLockError::WouldBlock) => {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "the record journal is busy"));
            }
            Err(fs::TryLockError::Error(e)) => return Err(e),
        }
    }
}

/// Who `forget` removes the records of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Who {
    /// `provider/account`.
    Account(String),
    /// An agent key.
    Agent(String),
}

/// Deletes the records that arrived before `before`: older segments whole, the one `before`
/// falls in rewritten. Returns how many records went.
pub fn prune(home: &Path, before: SystemTime, wait: Duration) -> io::Result<usize> {
    let _lock = lock(home, wait)?;
    let cut = clock::rfc3339(before);
    let mut gone = 0;
    for (day, path) in segments(home) {
        let boundary = day.as_str() == day_of(&cut);
        if day.as_str() > day_of(&cut) {
            break;
        }
        if boundary {
            gone += rewrite(&path, |r| r["arrived"].as_str().is_some_and(|a| clock::parse_rfc3339(a).is_some_and(|t| t < before)))?;
        } else {
            let text = fs::read_to_string(&path).unwrap_or_default();
            gone += fold(&text).len();
            fs::remove_file(&path)?;
        }
    }
    if gone > 0 {
        sync_dir(&home.join("records"))?;
    }
    Ok(gone)
}

/// Deletes every record of `who`, from every segment. Returns how many.
pub fn forget(home: &Path, who: &Who, wait: Duration) -> io::Result<usize> {
    let _lock = lock(home, wait)?;
    let mut gone = 0;
    for (_, path) in segments(home) {
        gone += rewrite(&path, |r| belongs(r, who))?;
    }
    Ok(gone)
}

fn belongs(r: &Value, who: &Who) -> bool {
    match who {
        Who::Agent(k) => r["agent"]["key"].as_str() == Some(k),
        Who::Account(a) => {
            Filter { account: Some(a.clone()), ..Filter::default() }.matches(r)
        }
    }
}

/// Rewrites a segment without the requests `drop` selects: write a temporary file, `fdatasync`
/// it, rename it into place, `fsync` the directory. Lines that don't parse stay. An emptied
/// segment is removed. Returns how many requests went.
fn rewrite(path: &Path, drop: impl Fn(&Value) -> bool) -> io::Result<usize> {
    let text = fs::read_to_string(path)?;
    let ids: BTreeSet<String> =
        fold(&text).into_iter().filter(|r| drop(r)).filter_map(|r| r["id"].as_str().map(str::to_owned)).collect();
    if ids.is_empty() {
        return Ok(0);
    }
    let kept: Vec<&str> = text
        .lines()
        .filter(|l| {
            serde_json::from_str::<Value>(l).ok().and_then(|v| v["id"].as_str().map(|id| ids.contains(id))) != Some(true)
        })
        .collect();
    if kept.is_empty() {
        fs::remove_file(path)?;
    } else {
        let tmp = path.with_extension("jsonl.tmp");
        let mut f = OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
        f.write_all((kept.join("\n") + "\n").as_bytes())?;
        f.sync_data()?;
        fs::rename(&tmp, path)?;
    }
    if let Some(dir) = path.parent() {
        sync_dir(dir)?;
    }
    Ok(ids.len())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::keys::AgentId;
    use crate::records::{Attempt, AttemptKind, AttemptOutcome, ServedBy};

    fn record(id: &str, arrived: &str) -> RequestRecord {
        let mut r = RequestRecord::new(id.into(), arrived.into(), "anthropic-messages");
        r.agent = Some(AgentId::new("key_a", Some("s1")));
        r.target = Some("sonnet".into());
        r
    }

    fn attempt(n: u32, account: &str) -> Attempt {
        Attempt {
            n,
            provider: "anthropic".into(),
            account: Some(account.into()),
            model: "m".into(),
            kind: AttemptKind::Initial,
            started: 0.5,
            ended: Some(9.0),
            outcome: Some(AttemptOutcome::Ok),
            usage: None,
            dropped: Vec::new(),
            forced: Vec::new(),
            placement: None,
        }
    }

    fn text(lines: &[(&str, Value)]) -> String {
        lines
            .iter()
            .map(|(t, v)| {
                let mut v = v.clone();
                v["v"] = 1.into();
                v["t"] = (*t).into();
                v.to_string() + "\n"
            })
            .collect()
    }

    fn lifecycle(id: &str, arrived: &str, account: &str) -> Vec<(&'static str, Value)> {
        let start = record(id, arrived);
        let mut done = start.clone();
        done.attempts.push(attempt(1, account));
        done.served_by = Some(ServedBy { provider: "anthropic".into(), account: Some(account.into()), model: "m".into() });
        done.outcome = Outcome::Succeeded;
        done.total_ms = Some(12.0);
        let mut lines = lines_for(&RequestRecord::new(id.into(), arrived.into(), "anthropic-messages"), &start, true);
        lines.extend(lines_for(&start, &done, false));
        lines
    }

    #[test]
    fn a_folded_record_is_the_live_record() {
        let start = record("rq_1", "2026-10-04T09:12:03.120Z");
        let mut done = start.clone();
        done.attempts.push(attempt(1, "max"));
        done.served_by = Some(ServedBy { provider: "anthropic".into(), account: Some("max".into()), model: "m".into() });
        done.outcome = Outcome::Succeeded;
        done.ttft_ms = Some(4.0);
        done.total_ms = Some(12.0);
        let blank = RequestRecord::new("rq_1".into(), start.arrived.clone(), "anthropic-messages");
        let mut lines = lines_for(&blank, &start, true);
        lines.extend(lines_for(&start, &done, false));
        let folded = fold(&text(&lines));
        assert_eq!(folded.len(), 1);
        assert_eq!(folded[0], serde_json::to_value(&done).unwrap());
    }

    #[test]
    fn later_lines_replace_earlier_ones_and_strays_are_skipped() {
        let mut lines = lifecycle("rq_1", "2026-10-04T09:00:00Z", "max");
        let mut again = lines.last().unwrap().clone();
        again.1["total_ms"] = json!(99.0);
        lines.push(again);
        lines.push(("attempt", json!({"id": "rq_missing", "attempt": {"n": 1}})));
        let body = text(&lines) + "not json\n{\"v\":1}\n";
        let folded = fold(&body);
        assert_eq!(folded.len(), 1, "a line without its open, or without an id, is skipped");
        assert_eq!(folded[0]["total_ms"], 99.0);
        assert_eq!(folded[0]["attempts"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn no_secret_or_prompt_field_exists_in_the_lines() {
        fn keys(v: &Value, out: &mut Vec<String>) {
            match v {
                Value::Object(m) => {
                    for (k, v) in m {
                        out.push(k.clone());
                        keys(v, out);
                    }
                }
                Value::Array(a) => a.iter().for_each(|v| keys(v, out)),
                _ => {}
            }
        }
        let mut found = Vec::new();
        for line in text(&lifecycle("rq_1", "2026-10-04T09:00:00Z", "max")).lines() {
            keys(&serde_json::from_str::<Value>(line).unwrap(), &mut found);
        }
        for banned in ["secret", "token", "authorization", "content", "messages", "prompt", "headers", "body"] {
            assert!(!found.iter().any(|k| k == banned), "{banned}");
        }
    }

    #[test]
    fn filters_select_by_account_agent_model_reason_and_time() {
        let home = tempfile::tempdir().unwrap();
        fs::create_dir_all(home.path().join("records")).unwrap();
        let mut body = text(&lifecycle("rq_1", "2026-10-04T09:00:00Z", "max"));
        body += &text(&lifecycle("rq_2", "2026-10-04T10:00:00Z", "pro"));
        fs::write(home.path().join("records/2026-10-04.jsonl"), body).unwrap();
        let ids = |f: Filter| read(home.path(), &f).iter().map(|r| r["id"].as_str().unwrap().to_owned()).collect::<Vec<_>>();
        assert_eq!(ids(Filter::default()), ["rq_2", "rq_1"], "newest first");
        assert_eq!(ids(Filter { account: Some("anthropic/pro".into()), ..Filter::default() }), ["rq_2"]);
        assert_eq!(ids(Filter { account: Some("max".into()), ..Filter::default() }), ["rq_1"]);
        assert_eq!(ids(Filter { agent: Some("key_a".into()), limit: Some(1), ..Filter::default() }), ["rq_2"]);
        assert!(ids(Filter { agent: Some("key_b".into()), ..Filter::default() }).is_empty());
        assert_eq!(ids(Filter { model: Some("sonnet".into()), ..Filter::default() }).len(), 2);
        assert_eq!(ids(Filter { since: clock::parse_rfc3339("2026-10-04T09:30:00Z"), ..Filter::default() }), ["rq_2"]);
        assert!(get(home.path(), "rq_1").is_some() && get(home.path(), "rq_9").is_none());
    }

    #[test]
    fn recovery_cuts_a_torn_line_and_closes_what_was_open() {
        let home = tempfile::tempdir().unwrap();
        fs::create_dir_all(home.path().join("records")).unwrap();
        let mut body = text(&lifecycle("rq_1", "2026-10-04T09:00:00Z", "max"));
        let open = lines_for(
            &RequestRecord::new("rq_2".into(), "2026-10-04T09:05:00Z".into(), "anthropic-messages"),
            &record("rq_2", "2026-10-04T09:05:00Z"),
            true,
        );
        body += &text(&open);
        body += "{\"v\":1,\"t\":\"attempt\",\"id\":\"rq_2\",\"attem";
        let path = home.path().join("records/2026-10-04.jsonl");
        fs::write(&path, body).unwrap();

        let now = clock::parse_rfc3339("2026-10-04T09:10:00Z").unwrap();
        assert_eq!(recover(home.path(), now).unwrap(), 1);
        let after = fs::read_to_string(&path).unwrap();
        assert!(after.ends_with('\n') && !after.contains("\"attem\n"), "the torn line is gone");
        let records = read(home.path(), &Filter::default());
        let by = |id: &str| records.iter().find(|r| r["id"] == id).unwrap().clone();
        assert_eq!(by("rq_1")["outcome"], "succeeded", "a finished request is untouched");
        assert_eq!(by("rq_2")["outcome"], "interrupted");
        assert_eq!(by("rq_2")["recovered_at"], "2026-10-04T09:10:00Z");
        assert_eq!(recover(home.path(), now).unwrap(), 0, "and a second recovery changes nothing");
    }

    #[test]
    fn a_whole_final_line_without_its_newline_only_gets_the_newline() {
        let home = tempfile::tempdir().unwrap();
        fs::create_dir_all(home.path().join("records")).unwrap();
        let path = home.path().join("records/2026-10-04.jsonl");
        fs::write(&path, text(&lifecycle("rq_1", "2026-10-04T09:00:00Z", "max")).trim_end()).unwrap();
        recover(home.path(), SystemTime::now()).unwrap();
        assert_eq!(read(home.path(), &Filter::default())[0]["outcome"], "succeeded");
    }

    #[test]
    fn prune_removes_older_days_whole_and_rewrites_the_boundary_day() {
        let home = tempfile::tempdir().unwrap();
        fs::create_dir_all(home.path().join("records")).unwrap();
        let day = |d: &str, lines: Vec<(&str, Value)>| fs::write(home.path().join(format!("records/{d}.jsonl")), text(&lines)).unwrap();
        day("2026-10-02", lifecycle("rq_a", "2026-10-02T09:00:00Z", "max"));
        let mut mid = lifecycle("rq_b", "2026-10-03T08:00:00Z", "max");
        mid.extend(lifecycle("rq_c", "2026-10-03T20:00:00Z", "max"));
        day("2026-10-03", mid);
        day("2026-10-04", lifecycle("rq_d", "2026-10-04T09:00:00Z", "max"));
        let cut = clock::parse_rfc3339("2026-10-03T12:00:00Z").unwrap();
        assert_eq!(prune(home.path(), cut, Duration::from_secs(1)).unwrap(), 2);
        let ids: Vec<_> = read(home.path(), &Filter::default()).iter().map(|r| r["id"].as_str().unwrap().to_owned()).collect();
        assert_eq!(ids, ["rq_d", "rq_c"]);
        assert!(!home.path().join("records/2026-10-02.jsonl").exists());
    }

    #[test]
    fn forget_removes_one_accounts_or_agents_records_only() {
        let home = tempfile::tempdir().unwrap();
        fs::create_dir_all(home.path().join("records")).unwrap();
        let mut lines = lifecycle("rq_1", "2026-10-04T09:00:00Z", "max");
        lines.extend(lifecycle("rq_2", "2026-10-04T10:00:00Z", "pro"));
        fs::write(home.path().join("records/2026-10-04.jsonl"), text(&lines)).unwrap();
        assert_eq!(forget(home.path(), &Who::Account("anthropic/max".into()), Duration::from_secs(1)).unwrap(), 1);
        let left = read(home.path(), &Filter::default());
        assert_eq!(left.len(), 1);
        assert_eq!(left[0]["id"], "rq_2");
        assert_eq!(forget(home.path(), &Who::Agent("key_a".into()), Duration::from_secs(1)).unwrap(), 1);
        assert!(read(home.path(), &Filter::default()).is_empty());
        assert!(segments(home.path()).is_empty(), "an emptied segment is removed");
    }

    #[test]
    fn a_rewrite_waits_for_the_lock_and_gives_up() {
        let home = tempfile::tempdir().unwrap();
        fs::create_dir_all(home.path().join("records")).unwrap();
        let held = lock(home.path(), Duration::from_secs(1)).unwrap();
        let err = forget(home.path(), &Who::Agent("k".into()), Duration::from_millis(100)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        drop(held);
        assert!(forget(home.path(), &Who::Agent("k".into()), Duration::from_millis(100)).is_ok());
    }
}
