//! `nullrouter records` (contracts/operator-cli.md § records). History is read from the journal's
//! segments on disk, so `list` and `show` work with or without a server. A request whose `open`
//! has no `close` is `in progress` while a server runs and `interrupted` when none does.
//! `prune` and `forget` rewrite the segments under the journal lock; with a server running,
//! `forget` first has it drop the fingerprints, so the writer and the CLI never both own a file.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, SystemTime};

use clap::Subcommand;
use nullrouter_cli::routing_text;
use nullrouter_engine::clock;
use nullrouter_engine::journal::{records, state};
use nullrouter_engine::keys::{self, Keys};
use nullrouter_registry::OperatorHome;
use nullrouter_server::operator::{self, CallError};
use serde_json::{Value, json};

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Newest first, from disk.
    List {
        #[arg(long)]
        provider: Option<String>,
        /// `provider/account`.
        #[arg(long)]
        account: Option<String>,
        /// An agent key id.
        #[arg(long)]
        agent: Option<String>,
        /// The target the client named, the unified model it resolved to, or the model that served it.
        #[arg(long)]
        model: Option<String>,
        /// A placement reason, such as `warm_stay` or `cold_by_deficit`.
        #[arg(long)]
        reason: Option<String>,
        /// A date (`2026-10-04`) or time (`2026-10-04T09:00:00Z`), UTC.
        #[arg(long)]
        since: Option<String>,
        #[arg(long)]
        limit: Option<usize>,
    },
    Show {
        id: String,
    },
    /// Delete the records that arrived before a date.
    Prune {
        #[arg(long)]
        before: String,
    },
    /// Delete one account's or one agent's records (an agent's fingerprints go too).
    Forget {
        /// `provider/account`.
        #[arg(long, conflicts_with = "agent", required_unless_present = "agent")]
        account: Option<String>,
        #[arg(long)]
        agent: Option<String>,
    },
}

/// How long a rewrite waits for the journal lock. Tests shorten it.
fn lock_wait() -> Duration {
    std::env::var("NULLROUTER_JOURNAL_LOCK_WAIT_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .map_or(records::LOCK_WAIT, Duration::from_millis)
}

fn server_runs(home: &OperatorHome) -> bool {
    std::os::unix::net::UnixStream::connect(operator::socket_path(home)).is_ok()
}

fn fail(msg: impl std::fmt::Display) -> ExitCode {
    eprintln!("{msg}");
    ExitCode::from(1)
}

/// `2026-10-04` or an RFC 3339 time.
fn parse_when(text: &str) -> Result<SystemTime, ExitCode> {
    let full = if text.len() == 10 { format!("{text}T00:00:00Z") } else { text.to_owned() };
    clock::parse_rfc3339(&full).ok_or_else(|| fail(format!("{text:?} is not a date; use 2026-10-04 or 2026-10-04T09:00:00Z")))
}

/// What a request with no `close` is called: in flight with a server, cut short without one.
fn settle_open(records: &mut [Value], running: bool) {
    for r in records {
        if r["outcome"] == "in_progress" && r["job"].is_null() && !running {
            r["outcome"] = json!("interrupted");
        }
    }
}

pub(crate) fn run(home: Option<PathBuf>, cmd: Command, as_json: bool) -> Result<ExitCode, ExitCode> {
    let home = home.map_or_else(OperatorHome::resolve, OperatorHome::new);
    // Records name keys by id; the operator knows them by name.
    let names = Keys::load(&home.path().join(keys::FILE))
        .map(|k| k.iter().map(|k| (k.id.clone(), k.name.clone())).collect())
        .unwrap_or_default();
    let running = server_runs(&home);
    match cmd {
        Command::List { provider, account, agent, model, reason, since, limit } => {
            let since = since.as_deref().map(parse_when).transpose()?;
            let filter = records::Filter { provider, account, agent, model, reason, since, limit, ..Default::default() };
            let mut found = records::read(home.path(), &filter);
            settle_open(&mut found, running);
            if as_json {
                println!("{:#}", Value::Array(found));
            } else {
                for r in &found {
                    println!("{}", line(r));
                }
            }
        }
        Command::Show { id } => {
            // A running server knows the freshest copy of an unfinished request.
            let live = running
                .then(|| operator::call(&home, &json!({"op": "records.get", "id": id})).ok())
                .flatten()
                .filter(|a| a["ok"] == true)
                .map(|a| a["record"].clone());
            let mut found = live.or_else(|| records::get(home.path(), &id)).into_iter().collect::<Vec<_>>();
            settle_open(&mut found, running);
            let Some(record) = found.pop() else { return Err(fail(format!("no record {id}"))) };
            if as_json {
                println!("{record:#}");
            } else {
                print!("{}", show(&record, &names));
            }
        }
        Command::Prune { before } => {
            let cut = parse_when(&before)?;
            let gone = records::prune(home.path(), cut, lock_wait()).map_err(|e| fail(lock_message(&e)))?;
            println!("pruned {gone} records that arrived before {}", clock::rfc3339(cut));
        }
        Command::Forget { account, agent } => {
            let who = match (&account, &agent) {
                (Some(a), None) if a.contains('/') => records::Who::Account(a.clone()),
                (Some(_), None) => return Err(fail("--account is provider/name, for example anthropic/max")),
                (None, Some(k)) => records::Who::Agent(k.clone()),
                _ => return Err(fail("name an account or an agent")),
            };
            // The server drops the fingerprints (and, for an account, its ledger entries) first.
            if running {
                let req = match &who {
                    records::Who::Account(a) => json!({"op": "records.forget", "account": a}),
                    records::Who::Agent(k) => json!({"op": "records.forget", "agent": k}),
                };
                ask(&home, &req)?;
            }
            let gone = records::forget(home.path(), &who, lock_wait()).map_err(|e| fail(lock_message(&e)))?;
            if !running {
                let _ = match &who {
                    records::Who::Account(a) => state::forget_account(home.path(), a),
                    records::Who::Agent(k) => state::forget_agent(home.path(), k),
                };
            }
            println!("forgot {gone} records");
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn lock_message(e: &std::io::Error) -> String {
    if e.kind() == std::io::ErrorKind::TimedOut {
        "the record journal is busy (another prune or forget holds its lock); try again".into()
    } else {
        format!("the record journal could not be rewritten: {e}")
    }
}

fn ask(home: &OperatorHome, req: &Value) -> Result<Value, ExitCode> {
    match operator::call(home, req) {
        Ok(a) if a["ok"] == true => Ok(a),
        Ok(a) => Err(fail(a["error"].as_str().unwrap_or("the server refused the request"))),
        Err(e @ CallError::NoServer(_)) => Err(fail(e)),
        Err(e) => Err(fail(e)),
    }
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or("-")
}

/// `2026-09-27T15:02:11.123Z` → `2026-09-27 15:02:11`.
fn when(v: &Value) -> String {
    s(v).get(..19).unwrap_or(s(v)).replace('T', " ")
}

/// `1234` → `1 234`: grouped as in the contract's example.
fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

fn ms(v: &Value) -> String {
    let tenths = |f: f64| (f * 10.0).round() as u64;
    v.as_f64().map_or_else(|| "-".into(), |f| format!("{}.{} ms", grouped(tenths(f) / 10), tenths(f) % 10))
}

fn count(v: &Value) -> String {
    v.as_u64().map_or_else(|| "not reported".into(), grouped)
}

/// `provider/account` or just `provider`.
fn who(v: &Value) -> String {
    match v["account"].as_str() {
        Some(a) => format!("{}/{a}", s(&v["provider"])),
        None => s(&v["provider"]).to_owned(),
    }
}

/// One `records list` line.
fn line(r: &Value) -> String {
    let served = if r["served_by"].is_null() { "-".into() } else { who(&r["served_by"]) };
    format!(
        "{}  {}  {:<10} {:<20} {:<24} {}",
        s(&r["id"]),
        when(&r["arrived"]),
        s(&r["outcome"]).replace('_', " "),
        s(&r["style"]),
        r["target"].as_str().unwrap_or("-"),
        served
    )
}

fn outcome(a: &Value) -> String {
    let o = &a["outcome"];
    match o["state"].as_str() {
        Some("ok") => "ok".into(),
        Some("failed") => {
            let status = o["status"].as_u64().map_or_else(String::new, |s| format!("{s} "));
            format!("{status}{}: {}", s(&o["class"]).replace('_', " "), s(&o["reason"]))
        }
        Some("skipped") => format!("skipped: {}", s(&o["reason"])),
        Some("cancelled") => "cancelled".into(),
        _ => "in progress".into(),
    }
}

/// `cold_by_deficit (rank 0)  ` for an attempt a placement chose; nothing for a skip.
fn placed(a: &Value) -> String {
    match (a["placement"]["reason"].as_str(), a["placement"]["rank"].as_u64()) {
        (Some(reason), Some(rank)) => format!("{reason} (rank {rank})  "),
        _ => String::new(),
    }
}

/// `  in 18,210 · out 512 · cache w 18,100`, for the counts the provider reported.
fn attempt_usage(u: &Value) -> String {
    let parts: Vec<String> = [("in", "input"), ("out", "output"), ("cache r", "cache_read"), ("cache w", "cache_write")]
        .iter()
        .filter_map(|(label, key)| u[*key].as_u64().map(|n| format!("{label} {}", grouped_comma(n))))
        .collect();
    if parts.is_empty() { String::new() } else { format!("  {}", parts.join(" · ")) }
}

fn grouped_comma(n: u64) -> String {
    grouped(n).replace(' ', ",")
}

/// The `decision` lines and candidate table of a record that a placement shaped.
fn decision(d: &Value, o: &mut String) {
    if d.is_null() {
        return;
    }
    let w = &d["amortization_window"];
    let span = match (
        w["start"].as_str().and_then(nullrouter_engine::clock::parse_rfc3339),
        w["length"].as_str().and_then(|l| nullrouter_registry::schema::parse_duration(l).ok()),
    ) {
        (Some(start), Some(len)) => format!("{}–{}", routing_text::hm(start), routing_text::hm(start + len)),
        _ => "-".into(),
    };
    let warm = &d["warm"];
    let named = |w: &Value| format!("{}/{}", s(&w["provider"]), s(&w["account"]));
    let prefix = |w: &Value| format!("prefix {} · idle {:.0} s", routing_text::si(w["prefix_tokens"].as_f64().unwrap_or(0.0)), w["idle_s"].as_f64().unwrap_or(0.0));
    if d["kind"] == "warm" && !warm.is_null() {
        let _ = writeln!(o, "decision    warm on {} · {} · stayed", named(warm), prefix(warm));
    } else {
        let _ = writeln!(
            o,
            "decision    {} · size {} · amortization {span}",
            s(&d["kind"]),
            routing_text::si(d["size_tokens"].as_f64().unwrap_or(0.0))
        );
        if !warm.is_null() {
            let because = warm["moved_because"].as_str().unwrap_or("not usable");
            let _ = writeln!(o, "            warm on {} · {} · moved: {because} on {}", named(warm), prefix(warm), named(warm));
        }
    }
    let rows = d["candidates"].as_array().cloned().unwrap_or_default();
    if rows.is_empty() {
        return;
    }
    let order: Vec<usize> = d["order"].as_array().into_iter().flatten().filter_map(|i| i.as_u64().map(|i| i as usize)).collect();
    // The attempt order first, then the candidates it left out.
    let mut seq: Vec<(Option<usize>, &Value)> = order.iter().enumerate().filter_map(|(rank, i)| rows.get(*i).map(|r| (Some(rank), r))).collect();
    seq.extend(rows.iter().enumerate().filter(|(i, _)| !order.contains(i)).map(|(_, r)| (None, r)));
    let mut cells = vec![["#", "account", "tier", "eligible", "pace", "share", "deficit", "price"].map(str::to_owned).to_vec()];
    for (rank, r) in seq {
        let dash = || String::new();
        cells.push(vec![
            rank.map_or_else(|| "-".into(), |n| n.to_string()),
            who(r),
            if r["tier"] == "payg" { "payg".into() } else { s(&r["tier"]).to_owned() },
            match r["why_not"].as_str() {
                Some(why) => format!("no: {}", why.replace('_', " ")),
                None => "yes".into(),
            },
            r["pace"].as_f64().map_or_else(dash, |p| format!("{p:.2}")),
            r["share"].as_f64().map_or_else(dash, |x| format!("{:.0}%", x * 100.0)),
            r["deficit_before"].as_i64().map_or_else(dash, routing_text::deficit),
            r["price_now"].as_f64().map_or_else(dash, |p| format!("{p:.2}")),
        ]);
    }
    let cols = cells[0].len();
    let widths: Vec<usize> = (0..cols).map(|i| cells.iter().map(|r| r[i].chars().count()).max().unwrap_or(0)).collect();
    for row in &cells {
        let mut line = String::from("  ");
        for (i, c) in row.iter().enumerate() {
            line.push_str(c);
            if i + 1 < cols {
                line.push_str(&" ".repeat(widths[i] - c.chars().count() + 2));
            }
        }
        let _ = writeln!(o, "{}", line.trim_end());
    }
}

/// The `records show` text.
fn show(r: &Value, names: &std::collections::HashMap<String, String>) -> String {
    let mut o = String::new();
    let _ = writeln!(o, "{}  {}  {}", s(&r["id"]), when(&r["arrived"]), s(&r["outcome"]).replace('_', " "));
    let agent = &r["agent"];
    if !agent.is_null() {
        let key = s(&agent["key"]);
        let name = names.get(key).map_or(key, String::as_str);
        match agent["session"].as_str() {
            Some(sess) => _ = writeln!(o, "agent       {name} / session {sess}"),
            None => _ = writeln!(o, "agent       {name}"),
        }
    }
    let _ = writeln!(
        o,
        "door        {}  {}  {}",
        s(&r["style"]),
        r["op"].as_str().unwrap_or("-"),
        r["model_type"].as_str().unwrap_or("-")
    );
    if let Some(t) = r["target"].as_str() {
        let unified = if r["unified_model"].is_null() { "" } else { " (unified)" };
        let _ = writeln!(o, "target      {t}{unified}");
    }
    if !r["served_by"].is_null() {
        let _ = writeln!(o, "served by   {}  {}", who(&r["served_by"]), s(&r["served_by"]["model"]));
    }
    let _ = writeln!(o, "ttft        {:<12} total {}", ms(&r["ttft_ms"]), ms(&r["total_ms"]));
    let u = &r["usage"];
    if u.is_null() {
        let _ = writeln!(o, "usage       not reported");
    } else {
        let estimated = if u["estimated"] == true { "  (estimated)" } else { "" };
        let _ = writeln!(
            o,
            "usage       input {}  output {}  cache-read {}  cache-write {}{estimated}",
            count(&u["input"]),
            count(&u["output"]),
            count(&u["cache_read"]),
            count(&u["cache_write"])
        );
    }
    let b = &r["break_handling"];
    let brk = match b["kind"].as_str() {
        Some("error_event") => format!("error event: {}", s(&b["reason"])),
        Some(k) => k.replace('_', " "),
        None => "none".into(),
    };
    let _ = writeln!(o, "break       {brk}");
    if let Some(job) = r["job"].as_object() {
        let _ = writeln!(o, "job         {}  upstream {}", s(&job["nullrouter_job_id"]), s(&job["upstream_id"]));
    }
    decision(&r["decision"], &mut o);
    let _ = writeln!(o, "attempts");
    for a in r["attempts"].as_array().into_iter().flatten() {
        let _ = writeln!(o, "  {}  {:<16} {:<26} {}{}", a["n"], who(a), s(&a["model"]), placed(a), outcome(a));
        if let (Some(from), Some(to)) = (a["started"].as_f64(), a["ended"].as_f64()) {
            let used = attempt_usage(&a["usage"]);
            let _ = writeln!(o, "       {:.2} s{used}", (to - from) / 1000.0);
        }
        for d in a["dropped"].as_array().into_iter().flatten() {
            let _ = writeln!(o, "       dropped {}: {}", s(&d["path"]), s(&d["reason"]));
        }
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn show_follows_the_contract_layout() {
        let r = json!({
            "id": "rq_01", "arrived": "2026-09-27T15:02:11.5Z", "outcome": "succeeded",
            "agent": {"key": "ak_1", "session": "9f1c"}, "style": "anthropic-messages", "op": "generate",
            "model_type": "text", "target": "claude-sonnet", "unified_model": "claude-sonnet",
            "served_by": {"provider": "openrouter", "account": "main", "model": "anthropic/claude-sonnet-4"},
            "ttft_ms": 412.34, "total_ms": 5804.9,
            "usage": {"input": 1204, "output": 388, "cache_read": 18432, "cache_write": null, "estimated": false},
            "break_handling": {"kind": "none"}, "job": null,
            "attempts": [
                {"n": 1, "provider": "anthropic", "account": "main", "model": "claude-sonnet-4-20250514",
                 "outcome": {"state": "failed", "status": 503, "class": "transient", "reason": "overloaded"}, "dropped": []},
                {"n": 2, "provider": "openrouter", "account": "main", "model": "anthropic/claude-sonnet-4",
                 "outcome": {"state": "ok"}, "dropped": [{"path": "metadata", "reason": "no place in openai-chat"}]}
            ]
        });
        let names = [("ak_1".to_owned(), "claude-code-laptop".to_owned())].into_iter().collect();
        let text = show(&r, &names);
        for want in [
            "rq_01  2026-09-27 15:02:11  succeeded",
            "agent       claude-code-laptop / session 9f1c",
            "door        anthropic-messages  generate  text",
            "target      claude-sonnet (unified)",
            "served by   openrouter/main  anthropic/claude-sonnet-4",
            "ttft        412.3 ms     total 5 804.9 ms",
            "usage       input 1 204  output 388  cache-read 18 432  cache-write not reported",
            "break       none",
            "503 transient: overloaded",
            "dropped metadata: no place in openai-chat",
        ] {
            assert!(text.contains(want), "{want:?} missing from:\n{text}");
        }
    }

    #[test]
    fn the_decision_table_and_placed_attempts_follow_the_contract() {
        let row = |provider: &str, account: &str, tier: &str, extra: Value| {
            let mut r = json!({"provider": provider, "account": account, "model": "m", "tier": tier});
            for (k, v) in extra.as_object().unwrap() {
                r[k] = v.clone();
            }
            r
        };
        let cold = json!({
            "id": "rq_01", "arrived": "2026-10-04T09:12:03Z", "outcome": "succeeded", "style": "anthropic-messages",
            "target": "sonnet", "ttft_ms": 640.0, "total_ms": 2210.0, "usage": null, "break_handling": {"kind": "none"},
            "decision": {"kind": "cold", "size_tokens": 18_400, "amortization_window": {"start": "2026-10-04T05:00:00.000Z", "length": "5h"},
                "warm": null, "order": [1, 0, 2], "candidates": [
                    row("anthropic", "pro", "subscription", json!({"pace": 0.88, "share": 0.39, "deficit_before": -91_200})),
                    row("anthropic", "max", "subscription", json!({"pace": 1.42, "share": 0.61, "deficit_before": 91_200})),
                    row("openrouter", "main", "payg", json!({"price_now": 3.0, "why_not": "reserve_floor"})),
                ]},
            "attempts": [{"n": 1, "provider": "anthropic", "account": "max", "model": "m", "started": 10.0, "ended": 2220.0,
                "placement": {"reason": "cold_by_deficit", "rank": 0}, "outcome": {"state": "ok"},
                "usage": {"input": 18_210, "output": 512, "cache_read": null, "cache_write": 18_100}, "dropped": []}],
        });
        let text = show(&cold, &Default::default());
        for want in [
            "decision    cold · size 18.4k · amortization 05:00–10:00",
            "#  account",
            "0  anthropic/max    subscription  yes",
            "+91.2k",
            "-91.2k",
            "2  openrouter/main  payg          no: reserve floor",
            "cold_by_deficit (rank 0)  ok",
            "2.21 s  in 18,210 · out 512 · cache w 18,100",
        ] {
            assert!(text.contains(want), "{want:?} missing from:\n{text}");
        }

        let mut warm = cold.clone();
        warm["decision"]["kind"] = json!("warm");
        warm["decision"]["warm"] = json!({"provider": "anthropic", "account": "max", "model": "m", "prefix_tokens": 41_200, "idle_s": 38.0, "stayed": true});
        assert!(show(&warm, &Default::default()).contains("decision    warm on anthropic/max · prefix 41.2k · idle 38 s · stayed"));
        warm["decision"]["kind"] = json!("cold");
        warm["decision"]["warm"]["stayed"] = json!(false);
        warm["decision"]["warm"]["moved_because"] = json!("reserve_floor");
        assert!(show(&warm, &Default::default()).contains("moved: reserve_floor on anthropic/max"));
    }
}
