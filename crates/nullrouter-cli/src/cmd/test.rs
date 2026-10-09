//! `nullrouter test` (spec 011 contracts/cli.md § `nullrouter test`): real, billed calls through
//! the running server, one per pair, each giving PASS, BROKEN or UNKNOWN.
//!
//! `test.plan` names the pairs and the calls first; more than one call asks before it runs.
//! `test.run` then streams one result per pair as it finishes. Ctrl-C closes the socket: the
//! server sends no more calls, and the finished results stay saved.
//!
//! A combo's test is one call through the combo; its answer is one nested result, printed as a
//! tree of the members the walk reached.

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, SystemTime};

use clap::Args as ClapArgs;
use nullrouter_engine::clock;
use nullrouter_engine::verdict::{Rejection, State};
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
    let mut combo = None;
    let done = answer(operator::call_stream(&home, &req, |line| {
        let r = &line["result"];
        if line["event"] == "combo" {
            combo = Some(r.clone());
        } else {
            if !as_json {
                println!("{}", result_line(r, SystemTime::now()));
            }
            results.push(r.clone());
        }
    }))?;
    let done = &done["done"];
    if let Some(c) = combo {
        if as_json {
            println!("{}", json!({"combo": c, "done": done}));
        } else {
            for l in combo_lines(&c, SystemTime::now()) {
                println!("{l}");
            }
        }
    } else if as_json {
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

/// A combo result as its output tree (contracts/cli.md § Combo output). An UNKNOWN attempt
/// changes no verdict, so it reads `(not saved)`.
pub(crate) fn combo_lines(r: &Value, now: SystemTime) -> Vec<String> {
    let s = |k: &str| r[k].as_str().unwrap_or_default();
    let mut head = format!("combo {}: {}", s("combo"), label(r));
    match r["answered_by"].as_str() {
        Some(by) => head += &format!(", answered by {by}"),
        None if !s("reason").is_empty() => head += &format!(": {}", s("reason")),
        None => {}
    }
    if let Some(next) = r["next"].as_str().and_then(clock::parse_rfc3339) {
        head += &format!("; retest in {}", short(next.duration_since(now).unwrap_or_default()));
    }
    let mut out = vec![head];
    tree(&r["tried"], 1, &mut out);
    out
}

fn label(v: &Value) -> &'static str {
    v["state"].as_str().and_then(State::parse).map_or("SKIPPED", State::label)
}

fn tree(tried: &Value, depth: usize, out: &mut Vec<String>) {
    let pad = "  ".repeat(depth);
    for t in tried.as_array().into_iter().flatten() {
        let name = format!("{pad}{}", t["member"].as_str().unwrap_or_default());
        let attempts = t["attempts"].as_array().map_or(&[][..], Vec::as_slice);
        let every_skip = !attempts.is_empty() && attempts.iter().all(|a| a["skipped"].is_string());
        let text = match attempts {
            _ if t["kind"] == "combo" => String::new(),
            _ if every_skip && t["state"] == "broken" => t["reason"].as_str().unwrap_or_default().to_owned(),
            [one] => attempt_text(one),
            _ => String::new(),
        };
        out.push(format!("{name:<18} {:<8} {text}", label(t)).trim_end().to_owned());
        if t["kind"] == "combo" {
            tree(&t["tried"], depth + 1, out);
        } else if attempts.len() > 1 && text.is_empty() {
            for a in attempts {
                out.push(format!("{pad}  {:<16} {:<8} {}", "", label(a), attempt_text(a)).trim_end().to_owned());
            }
        }
    }
}

/// `openrouter/main 503 upstream overloaded (not saved)`, `opencode-go/main 2.1 s`.
fn attempt_text(a: &Value) -> String {
    let s = |k: &str| a[k].as_str().unwrap_or_default();
    let who = format!("{}/{}", s("provider"), s("account"));
    match (a["skipped"].as_str(), s("state")) {
        (Some(why), _) => format!("{who} skipped: {why}"),
        (None, "pass") => format!("{who} {}", timing(a)),
        (None, "broken") => {
            let what = Rejection::parse(s("rejection")).map_or("rejected", |x| x.describe());
            format!("{who} {what}: {}", s("reason"))
        }
        (None, _) => format!("{who} {} (not saved)", s("reason")),
    }
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
        assert!(
            result_line(&broken, now)
                .ends_with("model not available: 403: model claude-opus-4-1 is not available on your plan")
        );
        let unknown = json!({"provider": "openrouter", "account": "main", "model": "x", "state": "unknown", "reason": "503: upstream overloaded", "ms": 300, "next": "2026-10-07T09:01:00Z"});
        assert!(
            result_line(&unknown, now).ends_with("503: upstream overloaded; retest in 1 min"),
            "{}",
            result_line(&unknown, now)
        );
        let skipped = json!({"provider": "xai", "account": "backup", "model": "grok-4", "state": null, "reason": "rate-limited until 09:14 UTC", "skipped": "rate-limited until 09:14 UTC", "ms": 0});
        assert!(result_line(&skipped, now).starts_with("SKIPPED  xai/backup"));
    }

    #[test]
    fn a_combo_prints_as_the_contract_shows() {
        let now = clock::parse_rfc3339("2026-10-07T09:00:00Z").unwrap();
        let skip = "BROKEN since 2026-10-07 08:00: 404: no such model";
        let r = json!({
            "combo": "coder", "state": "pass", "answered_by": "glm", "reason": "", "ms": 2400,
            "tried": [
                {"member": "sonnet", "kind": "unified", "state": "broken", "reason": "skipped: BROKEN on every account",
                 "attempts": [{"provider": "anthropic", "account": "max", "model": "s", "state": "broken",
                               "reason": skip, "skipped": skip, "ms": 0}]},
                {"member": "fallback-chain", "kind": "combo", "state": "pass", "reason": "", "tried": [
                    {"member": "gpt", "kind": "unified", "state": "unknown", "reason": "503: upstream overloaded",
                     "attempts": [{"provider": "openrouter", "account": "main", "model": "g", "state": "unknown",
                                   "reason": "503: upstream overloaded", "ms": 300}]},
                    {"member": "glm", "kind": "unified", "state": "pass", "reason": "",
                     "attempts": [{"provider": "opencode-go", "account": "main", "model": "z", "state": "pass",
                                   "reason": "", "ms": 2100}]}
                ]}
            ]
        });
        assert_eq!(
            combo_lines(&r, now),
            [
                "combo coder: PASS, answered by glm",
                "  sonnet           BROKEN   skipped: BROKEN on every account",
                "  fallback-chain   PASS",
                "    gpt            UNKNOWN  openrouter/main 503: upstream overloaded (not saved)",
                "    glm            PASS     opencode-go/main 2.1 s",
            ]
        );
        let unknown = json!({"combo": "writer", "state": "unknown", "reason": "503: busy", "ms": 1,
                             "next": "2026-10-07T09:05:00Z", "tried": []});
        assert_eq!(combo_lines(&unknown, now), ["combo writer: UNKNOWN: 503: busy; retest in 5 min"]);
    }
}
