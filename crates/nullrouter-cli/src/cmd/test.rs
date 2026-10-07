//! `nullrouter test` (spec 011 contracts/cli.md § `nullrouter test`): real, billed calls through
//! the running server, one per pair, each giving PASS, BROKEN or UNKNOWN.
//!
//! `test.plan` names the pairs and the calls first; more than one call asks before it runs.
//! `test.run` then streams one result per pair as it finishes. Ctrl-C closes the socket: the
//! server sends no more calls, and the finished results stay saved.

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, SystemTime};

use clap::Args as ClapArgs;
use nullrouter_engine::clock;
use nullrouter_engine::verdict::Rejection;
use nullrouter_registry::OperatorHome;
use nullrouter_server::operator::{self, CallError};
use serde_json::{Value, json};

const NO_SERVER: &str = "no running server; start it with nullrouter serve";

#[derive(ClapArgs)]
pub(crate) struct Args {
    /// `provider/model`, a unified model, or a combo.
    #[arg(required_unless_present = "all", conflicts_with = "all")]
    target: Option<String>,
    /// Only this account of the model's provider.
    #[arg(long, value_name = "NAME")]
    account: Option<String>,
    /// Every pair a unified model or combo can reach.
    #[arg(long)]
    all: bool,
    /// Run without asking first.
    #[arg(long)]
    yes: bool,
}

fn fail(e: impl std::fmt::Display) -> ExitCode {
    eprintln!("{e}");
    ExitCode::from(1)
}

/// The server's closing answer, or the exit for its error.
fn answer(got: Result<Value, CallError>) -> Result<Value, ExitCode> {
    match got {
        Ok(a) if a["ok"] == true => Ok(a),
        Ok(a) => Err(fail(a["error"].as_str().unwrap_or("the server refused the request"))),
        Err(CallError::NoServer(_)) => Err(fail(NO_SERVER)),
        Err(e) => Err(fail(e)),
    }
}

pub(crate) fn run(home: Option<PathBuf>, args: Args, as_json: bool) -> Result<ExitCode, ExitCode> {
    let home = home.map_or_else(OperatorHome::resolve, OperatorHome::new);
    let mut req = json!({"op": "test.plan", "target": args.target, "account": args.account, "all": args.all});
    let plan = answer(operator::call(&home, &req))?;
    let calls: Vec<(String, u64)> = plan["calls"]
        .as_object()
        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.as_u64().unwrap_or(0))).collect())
        .unwrap_or_default();
    let n: u64 = calls.iter().map(|(_, c)| c).sum();
    if n > 1 && !args.yes && !confirm(n, &calls) {
        return Err(fail("not run"));
    }

    req["op"] = "test.run".into();
    let mut results = Vec::new();
    let done = answer(operator::call_stream(&home, &req, |line| {
        let r = &line["result"];
        if !as_json {
            println!("{}", result_line(r, SystemTime::now()));
        }
        results.push(r.clone());
    }))?;
    let done = &done["done"];
    if as_json {
        println!("{}", json!({"results": results, "done": done}));
    } else {
        let count = |k: &str| done[k].as_u64().unwrap_or(0);
        println!(
            "{} pairs: {} pass, {} broken, {} unknown, {} skipped",
            results.len(),
            count("pass"),
            count("broken"),
            count("unknown"),
            count("skipped")
        );
    }
    Ok(ExitCode::SUCCESS)
}

/// `12 billed calls: 8 text, 2 embeddings, … Continue? [y/N]`, on stderr so `--json` stays clean.
fn confirm(n: u64, calls: &[(String, u64)]) -> bool {
    let parts: Vec<String> = calls.iter().filter(|(_, c)| *c > 0).map(|(k, c)| format!("{c} {k}")).collect();
    eprint!("{n} billed calls: {}. Continue? [y/N] ", parts.join(", "));
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    if std::io::stdin().lock().read_line(&mut line).is_err() {
        return false;
    }
    matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// One result as its output line (contracts/cli.md).
pub(crate) fn result_line(r: &Value, now: SystemTime) -> String {
    let s = |k: &str| r[k].as_str().unwrap_or_default();
    let who = format!("{}/{}", s("provider"), s("account"));
    let (label, text) = match (r["skipped"].as_str(), s("state")) {
        (Some(why), _) => ("SKIPPED", why.to_owned()),
        (None, "pass") => ("PASS", timing(r)),
        (None, "broken") => {
            let what = Rejection::parse(s("rejection")).map_or("rejected", |x| x.describe());
            ("BROKEN", format!("{what}: {}", s("reason")))
        }
        (None, _) => {
            let retest = r["next"].as_str().and_then(clock::parse_rfc3339).map(|t| {
                let left = t.duration_since(now).unwrap_or_default();
                format!("; retest in {}", short(left))
            });
            ("UNKNOWN", format!("{}{}", s("reason"), retest.unwrap_or_default()))
        }
    };
    format!("{label:<8} {who:<20} {:<26} {text}", s("model"))
}

fn timing(r: &Value) -> String {
    let secs = |ms: &Value| ms.as_f64().unwrap_or(0.0) / 1000.0;
    match r.get("ttft_ms") {
        Some(t) if !t.is_null() => format!("{:.1} s (first output {:.1} s)", secs(&r["ms"]), secs(t)),
        _ => format!("{:.1} s", secs(&r["ms"])),
    }
}

/// `40 s`, `5 min`, `6 h`.
fn short(d: Duration) -> String {
    match d.as_secs() {
        s if s < 60 => format!("{s} s"),
        s if s < 3600 => format!("{} min", (s + 30) / 60),
        s => format!("{} h", (s + 1800) / 3600),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn result_lines() {
        let now = clock::parse_rfc3339("2026-10-07T09:00:00Z").unwrap();
        let pass = json!({"provider": "anthropic", "account": "max", "model": "claude-sonnet-4-5", "state": "pass", "reason": "", "ms": 1800, "ttft_ms": 900});
        assert_eq!(
            result_line(&pass, now),
            "PASS     anthropic/max        claude-sonnet-4-5          1.8 s (first output 0.9 s)"
        );
        let broken = json!({"provider": "anthropic", "account": "max", "model": "claude-opus-4-1", "state": "broken", "rejection": "model_not_available", "reason": "403: model claude-opus-4-1 is not available on your plan", "ms": 300});
        assert!(result_line(&broken, now).ends_with("model not available: 403: model claude-opus-4-1 is not available on your plan"));
        let unknown = json!({"provider": "openrouter", "account": "main", "model": "x", "state": "unknown", "reason": "503: upstream overloaded", "ms": 300, "next": "2026-10-07T09:01:00Z"});
        assert!(result_line(&unknown, now).ends_with("503: upstream overloaded; retest in 1 min"), "{}", result_line(&unknown, now));
        let skipped = json!({"provider": "xai", "account": "backup", "model": "grok-4", "state": null, "reason": "rate-limited until 09:14 UTC", "skipped": "rate-limited until 09:14 UTC", "ms": 0});
        assert!(result_line(&skipped, now).starts_with("SKIPPED  xai/backup"));
    }
}
