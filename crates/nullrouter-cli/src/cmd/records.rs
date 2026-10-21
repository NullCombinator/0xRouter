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
use nullrouter_adapters::alerts::{Alert, AlertKind, AlertLog};
use nullrouter_adapters::store::Store;
use nullrouter_cli::routing_text;
use nullrouter_engine::clock;
use nullrouter_engine::journal::{records, state};
use nullrouter_registry::OperatorHome;
use nullrouter_server::operator::{self, CallError};
use nullrouter_server::views;
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
        /// A placement reason, such as `warm`, `cold_by_deficit` or `overflow`.
        #[arg(long)]
        reason: Option<String>,
        /// A date (`2026-10-04`) or time (`2026-10-04T09:00:00Z`), UTC.
        #[arg(long)]
        since: Option<String>,
        #[arg(long)]
        limit: Option<usize>,
        /// Only records older than this one: the next page back. The id must name a record.
        #[arg(long, value_name = "ID")]
        before: Option<String>,
        /// Only test calls (`nullrouter test`, retests, combo tests).
        #[arg(long, conflicts_with = "no_test")]
        test: bool,
        /// Only client requests, no test calls.
        #[arg(long)]
        no_test: bool,
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

fn fail(msg: impl std::fmt::Display) -> ExitCode {
    eprintln!("{msg}");
    ExitCode::from(1)
}

fn parse_when(text: &str) -> Result<SystemTime, ExitCode> {
    views::records::parse_when(text).map_err(|e| fail(e.message))
}

pub(crate) fn run(home: Option<PathBuf>, cmd: Command, as_json: bool) -> Result<ExitCode, ExitCode> {
    let home = home.map_or_else(OperatorHome::resolve, OperatorHome::new);
    let running = views::server_runs(&home);
    match cmd {
        Command::List { provider, account, agent, model, reason, since, limit, before, test, no_test } => {
            let test = (test || no_test).then_some(test);
            let args = json!({
                "provider": provider, "account": account, "agent": agent, "model": model,
                "reason": reason, "since": since, "limit": limit, "before": before, "test": test,
            });
            let view = super::read(&home, views::records::NEEDS, &args, views::records::build)?;
            if as_json {
                println!("{:#}", view.json);
            } else {
                let mut found = view.json;
                scrub(&mut found);
                for r in found.as_array().into_iter().flatten() {
                    println!("{}", line(r));
                }
            }
        }
        Command::Show { id } => {
            let view = super::read(&home, views::records::RECORD_NEEDS, &json!({"id": id}), views::records::record)?;
            let mut record = view.json;
            if as_json {
                println!("{record:#}");
            } else {
                scrub(&mut record);
                let names = view.extra["key_names"]
                    .as_object()
                    .map(|m| m.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned()))).collect())
                    .unwrap_or_default();
                print!("{}", show(&record, &names, &read_alerts(&home)));
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
                match &who {
                    records::Who::Account(a) => state::forget_account(home.path(), a),
                    records::Who::Agent(k) => state::forget_agent(home.path(), k),
                }
                .map_err(|e| {
                    fail(format!("{gone} records forgotten, but the routing state could not be rewritten: {e}"))
                })?;
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

/// The alert log, read once for `records show`. With no `adapters/` there are no alerts, and a read
/// creates nothing; a log that cannot be read leaves the alert ids off the guardrail lines.
fn read_alerts(home: &OperatorHome) -> Vec<Alert> {
    if !home.path().join("adapters").is_dir() {
        return Vec::new();
    }
    let Ok(store) = Store::open(home.path()) else { return Vec::new() };
    AlertLog::open(&store).list().unwrap_or_default()
}

fn ask(home: &OperatorHome, req: &Value) -> Result<Value, ExitCode> {
    match operator::call(home, req) {
        Ok(a) if a["ok"] == true => Ok(a),
        Ok(a) => Err(fail(a["error"].as_str().unwrap_or("the server refused the request"))),
        Err(e @ CallError::NoServer(_)) => Err(fail(e)),
        Err(e) => Err(fail(e)),
    }
}

/// Drops control characters from every string in `v`. A record carries text that a client or a
/// provider chose, and the terminal obeys escape sequences in it.
fn scrub(v: &mut Value) {
    match v {
        Value::String(s) if s.chars().any(char::is_control) => *s = s.chars().filter(|c| !c.is_control()).collect(),
        Value::Array(a) => a.iter_mut().for_each(scrub),
        Value::Object(m) => m.values_mut().for_each(scrub),
        _ => {}
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

/// One `records list` line; a test call ends with its `test` tag and run id.
fn line(r: &Value) -> String {
    let served = if r["served_by"].is_null() { "-".into() } else { who(&r["served_by"]) };
    let test = match r["test"]["run"].as_str() {
        Some(run) => format!("  test {run}"),
        None => String::new(),
    };
    let line = format!(
        "{}  {}  {:<10} {:<20} {:<24} {}{test}",
        s(&r["id"]),
        when(&r["arrived"]),
        s(&r["outcome"]).replace('_', " "),
        s(&r["style"]),
        r["target"].as_str().unwrap_or("-"),
        served
    );
    match slowest(r) {
        Some(text) => format!("{line}  {text}"),
        None => line,
    }
}

/// `1.2 ms`, `41.2 s`: a phase time at the size a person reads.
fn span(ms: f64) -> String {
    if ms < 1000.0 { format!("{ms:.1} ms") } else { format!("{:.1} s", ms / 1000.0) }
}

/// The `SLOWEST` column: the longest phase, its time and its side. `…` marks a phase still
/// running; `not recorded` a request from before phases were kept. Nothing for a record with
/// no attempt to time.
fn slowest(r: &Value) -> Option<String> {
    let sl = &r["slowest"];
    if let Some(phase) = sl["phase"].as_str() {
        let more = if sl["in_progress"] == true { "…" } else { "" };
        let time = sl["ms"].as_f64().map_or_else(|| "-".into(), span);
        return Some(format!("{} {time}{more} ({})", phase.replace('_', " "), s(&sl["side"])));
    }
    let attempts = r["attempts"].as_array()?;
    let unrecorded = |a: &Value| a["phases"] == "not_recorded";
    (attempts.iter().any(unrecorded)).then(|| "not recorded".to_owned())
}

/// The attempt header's second line: how the connection came, and how the attempt ended.
fn connection_line(a: &Value) -> String {
    let t = &a["timing"];
    let mut parts: Vec<String> = Vec::new();
    match t["connection"].as_str() {
        Some("new") => parts.push("new connection".into()),
        Some("reused") => parts.push("reused".into()),
        _ => {}
    }
    if let Some(v) = t["http"].as_str() {
        parts.push(format!("HTTP/{}", if v == "2" { "2" } else { "1.1" }));
    }
    if let Some(p) = t["proxy"].as_str() {
        parts.push(format!("proxy {p}"));
    }
    let mut out = parts.join(" · ");
    if let Some(p) = a["phases"]["ended_in"].as_str() {
        let mut why = format!("failed in {}", p.replace('_', " "));
        let hit = &t["timeout"];
        if let Some(which) = hit["which"].as_str() {
            let by = match s(&hit["source"]["by"]) {
                "built_in" => "built-in".to_owned(),
                other => other.to_owned(),
            };
            why += &format!(
                " (timeout: {} {} ms, {by} {})",
                which.replace('_', " "),
                hit["ms"],
                match s(&hit["source"]["level"]) {
                    l @ ("model" | "provider" | "endpoint") => format!("per {l}"),
                    other => other.to_owned(),
                }
            );
        }
        if !out.is_empty() {
            out += "   ";
        }
        out += &why;
    }
    out
}

/// The phase table of one attempt, one row a phase.
fn phase_rows(a: &Value, o: &mut String) {
    let p = &a["phases"];
    if p == "not_recorded" {
        let _ = writeln!(o, "       phases  not recorded");
        return;
    }
    let Some(map) = p.as_object() else { return };
    let t = &a["timing"];
    let note = |name: &str| -> String {
        match name {
            "connect" => t["refresh_ms"]
                .as_f64()
                .map_or_else(String::new, |r| format!("   (includes token refresh {})", span(r))),
            "delivery" => {
                t["closing_ms"].as_f64().map_or_else(String::new, |c| format!("   (of which record close {})", span(c)))
            }
            _ => String::new(),
        }
    };
    for name in [
        "router_overhead",
        "retry_wait",
        "connect",
        "headers",
        "first_token",
        "waiting_for_provider",
        "generation",
        "delivery",
    ] {
        let Some(v) = map.get(name) else { continue };
        let text = match v {
            Value::Number(n) => span(n.as_f64().unwrap_or(0.0)),
            Value::Object(o) => format!("{}…", span(o["in_progress"].as_f64().unwrap_or(0.0))),
            Value::String(x) => x.replace('_', " "),
            _ => continue,
        };
        let _ = writeln!(o, "       {:<22}{text}{}", name.replace('_', " "), note(name));
    }
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
    let parts: Vec<String> =
        [("in", "input"), ("out", "output"), ("cache r", "cache_read"), ("cache w", "cache_write")]
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
    let prefix = |w: &Value| {
        format!(
            "prefix {} · idle {:.0} s",
            routing_text::si(w["prefix_tokens"].as_f64().unwrap_or(0.0)),
            w["idle_s"].as_f64().unwrap_or(0.0)
        )
    };
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
            let _ = writeln!(
                o,
                "            warm on {} · {} · moved: {because} on {}",
                named(warm),
                prefix(warm),
                named(warm)
            );
        }
    }
    let rows = d["candidates"].as_array().cloned().unwrap_or_default();
    if rows.is_empty() {
        return;
    }
    let order: Vec<usize> =
        d["order"].as_array().into_iter().flatten().filter_map(|i| i.as_u64().map(|i| i as usize)).collect();
    // The attempt order first, then the candidates it left out.
    let mut seq: Vec<(Option<usize>, &Value)> =
        order.iter().enumerate().filter_map(|(rank, i)| rows.get(*i).map(|r| (Some(rank), r))).collect();
    seq.extend(rows.iter().enumerate().filter(|(i, _)| !order.contains(i)).map(|(_, r)| (None, r)));
    let mut cells =
        vec![["#", "account", "tier", "eligible", "pace", "share", "deficit", "price"].map(str::to_owned).to_vec()];
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

/// `hermes built-in` or `claude-code v0.1.0-1a2b3c4d`: the harness and the version that ran.
fn adapter_who(a: &Value) -> String {
    let version = match s(&a["version"]) {
        "builtin" => "built-in".to_owned(),
        v => v.to_owned(),
    };
    format!("{} {version}", s(&a["harness"]))
}

/// The adapter's outcome as the text writes it: `ran, 3 changes`, `not run (suspect); served as
/// plain client`, `failed (deadline)`, `blocked by guardrail`.
fn adapter_outcome(a: &Value) -> String {
    let o = &a["outcome"];
    match o["state"].as_str() {
        Some("ran") => {
            let n = a["changes"].as_array().map_or(0, Vec::len);
            format!("ran, {n} {}", if n == 1 { "change" } else { "changes" })
        }
        Some("not_run") => format!("not run ({}); served as plain client", s(&o["reason"])),
        Some("failed") => match o["reason"]["rule"].as_str() {
            Some(rule) => format!("failed ({}: {rule})", s(&o["reason"]["kind"])),
            None => format!("failed ({})", s(&o["reason"]["kind"])),
        },
        Some("blocked") => "blocked by guardrail".into(),
        _ => "unknown".into(),
    }
}

/// The record's adapter run for the agent line: the first attempt's, else the response's.
fn record_adapter(r: &Value) -> Option<&Value> {
    let attempt = r["attempts"].as_array()?.iter().map(|a| &a["adapter"]).find(|a| !a.is_null());
    attempt.or_else(|| Some(&r["response_adapter"]).filter(|a| !a.is_null()))
}

/// `harness hermes (built-in)` or `harness claude-code v0.1.0-1a2b3c4d`, on the agent line.
fn harness_text(a: &Value) -> String {
    match s(&a["version"]) {
        "builtin" => format!("harness {} (built-in)", s(&a["harness"])),
        v => format!("harness {} {v}", s(&a["harness"])),
    }
}

/// The id of the guardrail alert a blocked run raised: a `guardrail` alert on this record, from the
/// harness and version the guardrail names. `None` for a run without a guardrail, or with no match.
fn guardrail_alert<'a>(r: &Value, run: &Value, alerts: &'a [Alert]) -> Option<&'a str> {
    let g = &run["guardrail"];
    if g.is_null() {
        return None;
    }
    let (record, harness, version) = (s(&r["id"]), s(&g["adapter"]["harness"]), s(&g["adapter"]["version"]));
    alerts
        .iter()
        .find(|a| {
            a.kind == AlertKind::Guardrail
                && a.record.as_deref() == Some(record)
                && a.harness.as_str() == harness
                && a.version.as_str() == version
        })
        .map(|a| a.id.as_str())
}

/// The change lines and the guardrail lines of an adapter run, at the contract's indent. A
/// change is `kind  path  reason`, the kind and the path padded to their columns. `alert` is the
/// guardrail alert the block raised, if the log has one.
fn adapter_detail(a: &Value, alert: Option<&str>, o: &mut String) {
    for c in a["changes"].as_array().into_iter().flatten() {
        let kind = s(&c["kind"]);
        let path = s(&c["path"]);
        let kind_pad = 11usize.saturating_sub(kind.len()).max(2);
        let path_pad = 27usize.saturating_sub(path.len()).max(2);
        let _ = writeln!(o, "       {kind}{}{path}{}{}", " ".repeat(kind_pad), " ".repeat(path_pad), s(&c["reason"]));
    }
    let g = &a["guardrail"];
    if g.is_null() {
        return;
    }
    let paths: Vec<&str> = g["paths"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    let mut rule = format!("       rule {}", s(&g["rule"]));
    if !paths.is_empty() {
        rule += &format!("  paths {}", paths.join(", "));
    }
    let _ = writeln!(o, "{rule}");
    let suffix = alert.map_or_else(String::new, |id| format!(" (alert {id})"));
    let _ = writeln!(o, "       sent unmodified; adapter marked suspect{suffix}");
}

/// The `records show` text. `alerts` is the alert log, for the guardrail lines' alert ids.
fn show(r: &Value, names: &std::collections::HashMap<String, String>, alerts: &[Alert]) -> String {
    let mut o = String::new();
    let _ = writeln!(o, "{}  {}  {}", s(&r["id"]), when(&r["arrived"]), s(&r["outcome"]).replace('_', " "));
    let agent = &r["agent"];
    if !agent.is_null() {
        let key = s(&agent["key"]);
        let name = names.get(key).map_or(key, String::as_str);
        let harness = record_adapter(r).map_or_else(String::new, |a| format!("   {}", harness_text(a)));
        match agent["session"].as_str() {
            Some(sess) => _ = writeln!(o, "agent       {name} / session {sess}{harness}"),
            None => _ = writeln!(o, "agent       {name}{harness}"),
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
    if let Some(c) = r["combo"].as_str() {
        let _ = writeln!(o, "combo       {c}");
    }
    if let Some(t) = r["test"].as_object() {
        let _ = writeln!(o, "test        {}  run {}", s(&t["source"]).replace('_', " "), s(&t["run"]));
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
        if !a["adapter"].is_null() {
            let _ = writeln!(o, "     adapter {}: {}", adapter_who(&a["adapter"]), adapter_outcome(&a["adapter"]));
            adapter_detail(&a["adapter"], guardrail_alert(r, &a["adapter"], alerts), &mut o);
        }
        if let (Some(from), Some(to)) = (a["started"].as_f64(), a["ended"].as_f64()) {
            let used = attempt_usage(&a["usage"]);
            let _ = writeln!(o, "       {:.2} s{used}", (to - from) / 1000.0);
        }
        if let Some(m) = a["member"].as_str() {
            let _ = writeln!(o, "       member {m}");
        }
        for d in a["dropped"].as_array().into_iter().flatten() {
            let _ = writeln!(o, "       dropped {}: {}", s(&d["path"]), s(&d["reason"]));
        }
        let how = connection_line(a);
        if !how.is_empty() {
            let _ = writeln!(o, "       {how}");
        }
        if a["kind"] != "skipped" {
            phase_rows(a, &mut o);
        }
    }
    let sum: f64 = r["attempts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|a| a["phases"].as_object())
        .flat_map(|m| m.values())
        .filter_map(Value::as_f64)
        .sum();
    if sum > 0.0 {
        let _ = writeln!(o, "total {} = sum of phases", span(sum));
    }
    let ra = &r["response_adapter"];
    if ra.is_null() {
        let _ = writeln!(o, "response adapter: none");
    } else {
        let _ = writeln!(o, "response adapter: {}: {}", adapter_who(ra), adapter_outcome(ra));
        adapter_detail(ra, guardrail_alert(r, ra, alerts), &mut o);
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
        let text = show(&r, &names, &[]);
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
        let text = show(&cold, &Default::default(), &[]);
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
        assert!(
            show(&warm, &Default::default(), &[])
                .contains("decision    warm on anthropic/max · prefix 41.2k · idle 38 s · stayed")
        );
        warm["decision"]["kind"] = json!("cold");
        warm["decision"]["warm"]["stayed"] = json!(false);
        warm["decision"]["warm"]["moved_because"] = json!("reserve_floor");
        assert!(show(&warm, &Default::default(), &[]).contains("moved: reserve_floor on anthropic/max"));
    }

    fn timed(extra: Value) -> Value {
        let mut r = json!({
            "id": "rq_01", "arrived": "2026-10-07T14:01:55Z", "outcome": "succeeded", "style": "openai-chat",
            "target": "gpt-5", "served_by": {"provider": "openrouter", "account": "main", "model": "m"},
            "ttft_ms": 9300.0, "total_ms": 41000.6, "usage": null, "break_handling": {"kind": "none"},
            "attempts": [
                {"n": 1, "provider": "openrouter", "account": "main", "model": "m", "started": 4.1, "ended": 6046.3,
                 "outcome": {"state": "failed", "status": null, "class": "timeout", "reason": "no response headers"},
                 "dropped": [],
                 "timing": {"connection": "new", "http": "2", "proxy": "eu-exit", "refresh_ms": 0.5,
                            "timeout": {"which": "headers", "ms": 6000, "source": {"by": "operator", "level": "provider"}}},
                 "phases": {"router_overhead": 4.1, "retry_wait": "not_applicable", "connect": 41.0, "headers": 6000.2,
                            "first_token": "not_applicable", "generation": "not_applicable",
                            "delivery": "not_applicable", "ended_in": "headers"}},
                {"n": 2, "provider": "openrouter", "account": "main", "model": "m", "started": 6048.3, "ended": 41000.6,
                 "outcome": {"state": "ok"}, "dropped": [],
                 "timing": {"connection": "reused", "http": "2", "closing_ms": 1.1},
                 "phases": {"router_overhead": 0.3, "retry_wait": 2000.0, "connect": "not_applicable",
                            "waiting_for_provider": 3205.5, "generation": 31740.5, "delivery": 12.3, "ended_in": null}}
            ],
            "slowest": {"phase": "generation", "ms": 30100.0, "side": "provider", "in_progress": false}
        });
        for (k, v) in extra.as_object().unwrap() {
            r[k] = v.clone();
        }
        r
    }

    #[test]
    fn list_shows_the_slowest_phase_with_its_side() {
        let done = line(&timed(json!({})));
        assert!(done.ends_with("generation 30.1 s (provider)"), "{done}");
        let live = line(&timed(
            json!({"slowest": {"phase": "headers", "ms": 2100.0, "side": "provider", "in_progress": true}}),
        ));
        assert!(live.ends_with("headers 2.1 s… (provider)"), "{live}");
        let merged = line(&timed(
            json!({"slowest": {"phase": "waiting_for_provider", "ms": 41.2, "side": "provider", "in_progress": false}}),
        ));
        assert!(merged.ends_with("waiting for provider 41.2 ms (provider)"), "{merged}");
        let old = line(&timed(json!({"slowest": null, "attempts": [{"n": 1, "phases": "not_recorded"}]})));
        assert!(old.ends_with("not recorded"), "{old}");
        let none = line(&timed(json!({"slowest": null, "attempts": []})));
        assert!(!none.contains("not recorded") && !none.contains("provider)"), "{none}");
    }

    #[test]
    fn show_prints_the_phase_table_per_attempt() {
        let text = show(&timed(json!({})), &Default::default(), &[]);
        for want in [
            "new connection · HTTP/2 · proxy eu-exit   failed in headers (timeout: headers 6000 ms, operator per provider)",
            "router overhead       4.1 ms",
            "connect               41.0 ms   (includes token refresh 0.5 ms)",
            "headers               6.0 s",
            "first token           not applicable",
            "reused · HTTP/2",
            "retry wait            2.0 s",
            "connect               not applicable",
            "waiting for provider  3.2 s",
            "generation            31.7 s",
            "delivery              12.3 ms   (of which record close 1.1 ms)",
            "= sum of phases",
        ] {
            assert!(text.contains(want), "{want:?} missing from:\n{text}");
        }
        let old = timed(
            json!({"attempts": [{"n": 1, "provider": "p", "model": "m", "outcome": {"state": "ok"}, "dropped": [], "phases": "not_recorded"}]}),
        );
        assert!(show(&old, &Default::default(), &[]).contains("phases  not recorded"));
    }

    /// Spec 011: a test call's tag and run id, its combo and each attempt's member path.
    #[test]
    fn a_test_call_shows_its_run_combo_and_members() {
        let r = json!({
            "id": "rq_07", "arrived": "2026-10-07T09:20:00Z", "outcome": "succeeded", "style": "openai-chat",
            "op": "generate", "model_type": "text", "target": "coder", "combo": "coder",
            "test": {"run": "tr_01", "source": "combo_test"},
            "served_by": {"provider": "beta", "account": "a", "model": "m1"},
            "break_handling": {"kind": "none"},
            "attempts": [
                {"n": 1, "provider": "alpha", "account": "a", "model": "m1", "member": "coder › ua",
                 "outcome": {"state": "failed", "status": 503, "class": "transient", "reason": "overloaded"}},
                {"n": 2, "provider": "beta", "account": "a", "model": "m1", "member": "coder › chain › ub",
                 "outcome": {"state": "ok"}}
            ]
        });
        assert!(line(&r).ends_with("beta/a  test tr_01"), "{}", line(&r));
        let text = show(&r, &Default::default(), &[]);
        for want in [
            "combo       coder\n",
            "test        combo test  run tr_01\n",
            "       member coder › ua\n",
            "       member coder › chain › ub\n",
        ] {
            assert!(text.contains(want), "{want:?} missing from:\n{text}");
        }
        let mut client = r.clone();
        client.as_object_mut().unwrap().remove("test");
        assert!(line(&client).ends_with("beta/a"), "a client request has no tag");
    }

    /// Slice 004: each attempt's adapter line, its changes and guardrail, and the response adapter.
    #[test]
    fn adapter_runs_follow_the_contract() {
        let r = json!({
            "id": "rq_01", "arrived": "2026-09-28T11:04:52Z", "outcome": "succeeded", "style": "openai-chat",
            "break_handling": {"kind": "none"}, "response_adapter": null,
            "attempts": [
                {"n": 1, "provider": "openrouter", "account": "main", "model": "deepseek/deepseek-r1",
                 "outcome": {"state": "ok"}, "dropped": [],
                 "adapter": {"harness": "hermes", "version": "builtin", "outcome": {"state": "ran"}, "duration_us": 12,
                     "changes": [
                        {"path": "messages[4].images", "kind": "converted", "reason": "format_conversion"},
                        {"path": "messages[4].content", "kind": "converted", "reason": "format_conversion"},
                        {"path": "messages[3].reasoning_content", "kind": "removed", "reason": "target_rejects_field"}
                     ]}},
                {"n": 2, "provider": "anthropic", "account": "max", "model": "m",
                 "outcome": {"state": "failed", "status": null, "class": "transient", "reason": "overloaded"}, "dropped": [],
                 "adapter": {"harness": "claude-code", "version": "v0.1.0-1a2b3c4d", "outcome": {"state": "blocked"},
                     "changes": [], "duration_us": 9,
                     "guardrail": {"direction": "request", "rule": "tool_call_added", "paths": ["messages[7].content[2]"],
                         "adapter": {"harness": "claude-code", "version": "v0.1.0-1a2b3c4d"}, "at": "2026-09-28T11:04:52Z"}}},
                {"n": 3, "provider": "anthropic", "account": "max", "model": "m", "outcome": {"state": "ok"}, "dropped": [],
                 "adapter": {"harness": "claude-code", "version": "v0.1.0-1a2b3c4d", "outcome": {"state": "not_run", "reason": "suspect"},
                     "changes": [], "duration_us": 0}},
                {"n": 4, "provider": "anthropic", "account": "max", "model": "m", "outcome": {"state": "ok"}, "dropped": [],
                 "adapter": {"harness": "claude-code", "version": "v0.1.0-1a2b3c4d", "outcome": {"state": "failed", "reason": {"kind": "deadline"}},
                     "changes": [], "duration_us": 0}}
            ]
        });
        let text = show(&r, &Default::default(), &[]);
        for want in [
            "     adapter hermes built-in: ran, 3 changes\n",
            "       converted  messages[4].images         format_conversion\n",
            "       removed    messages[3].reasoning_content  target_rejects_field\n",
            "     adapter claude-code v0.1.0-1a2b3c4d: blocked by guardrail\n",
            "       rule tool_call_added  paths messages[7].content[2]\n",
            "       sent unmodified; adapter marked suspect\n",
            "     adapter claude-code v0.1.0-1a2b3c4d: not run (suspect); served as plain client\n",
            "     adapter claude-code v0.1.0-1a2b3c4d: failed (deadline)\n",
            "response adapter: none\n",
        ] {
            assert!(text.contains(want), "{want:?} missing from:\n{text}");
        }
    }
}
