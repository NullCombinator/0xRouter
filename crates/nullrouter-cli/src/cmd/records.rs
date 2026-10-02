//! `nullrouter records` (contracts/operator-cli.md § records show). Records live in the
//! running server's memory and are read over the operator socket.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Subcommand;
use nullrouter_engine::keys::{self, Keys};
use nullrouter_registry::OperatorHome;
use nullrouter_server::operator::{self, CallError};
use serde_json::{Value, json};

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Newest first.
    List {
        #[arg(long)]
        provider: Option<String>,
        /// Unified model name.
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        limit: Option<usize>,
    },
    Show {
        id: String,
    },
}

fn ask(home: &OperatorHome, req: &Value) -> Result<Value, ExitCode> {
    match operator::call(home, req) {
        Ok(a) if a["ok"] == true => Ok(a),
        Ok(a) => {
            eprintln!("{}", a["error"].as_str().unwrap_or("the server refused the request"));
            Err(ExitCode::from(1))
        }
        Err(e @ CallError::NoServer(_)) => {
            eprintln!("{e}; records are kept by the running server (start it with `nullrouter serve`)");
            Err(ExitCode::from(4))
        }
        Err(e) => {
            eprintln!("{e}");
            Err(ExitCode::from(1))
        }
    }
}

pub(crate) fn run(home: Option<PathBuf>, cmd: Command, as_json: bool) -> Result<ExitCode, ExitCode> {
    let home = home.map_or_else(OperatorHome::resolve, OperatorHome::new);
    // Records name keys by id; the operator knows them by name.
    let names = Keys::load(&home.path().join(keys::FILE))
        .map(|k| k.iter().map(|k| (k.id.clone(), k.name.clone())).collect())
        .unwrap_or_default();
    match cmd {
        Command::List { provider, model, limit } => {
            let a = ask(
                &home,
                &json!({"op": "records.list", "provider": provider, "unified_model": model, "limit": limit}),
            )?;
            let records = a["records"].as_array().cloned().unwrap_or_default();
            if as_json {
                println!("{:#}", Value::Array(records));
            } else {
                for r in &records {
                    println!("{}", line(r));
                }
            }
        }
        Command::Show { id } => {
            let a = ask(&home, &json!({"op": "records.get", "id": id}))?;
            if as_json {
                println!("{:#}", a["record"]);
            } else {
                print!("{}", show(&a["record"], &names));
            }
        }
    }
    Ok(ExitCode::SUCCESS)
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
        if i > 0 && (digits.len() - i) % 3 == 0 {
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
        s(&r["outcome"]),
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
    let _ = writeln!(o, "attempts");
    for a in r["attempts"].as_array().into_iter().flatten() {
        let _ = writeln!(o, "  {}  {:<16} {:<26} {}", a["n"], who(a), s(&a["model"]), outcome(a));
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
}
