//! `nullrouter latency`: the last 24 hours, per agent and per provider.

use std::path::PathBuf;
use std::process::ExitCode;

use nullrouter_registry::OperatorHome;
use nullrouter_server::views::{self, usage::local_text};
use serde_json::{Value, json};

/// Under a second in ms, otherwise seconds with one decimal.
fn time(ms: f64) -> String {
    if ms < 1000.0 { format!("{} ms", ms.round() as u64) } else { format!("{:.1} s", ms / 1000.0) }
}

/// `p50 / p95`, or `none` when no request had a value (never 0).
fn pct(v: &Value) -> String {
    match (v["p50"].as_f64(), v["p95"].as_f64()) {
        (Some(a), Some(b)) => format!("{} / {}", time(a), time(b)),
        _ => "none".to_owned(),
    }
}

/// `resolved 14:01:58`, with the date when it isn't the read's date; `none` when there is none.
fn last(v: &Value, read_date: &str) -> String {
    let Some(at) = v["at"].as_str() else { return "none".to_owned() };
    let local = local_text(at);
    let when = match local.split_once(' ') {
        Some((date, clock)) if date == read_date => clock.to_owned(),
        _ => local,
    };
    let status = v["status"].as_u64().map_or_else(String::new, |s| format!(" ({s})"));
    format!("{} {when}{status}", v["result"].as_str().unwrap_or("?"))
}

fn name(v: &Value) -> String {
    v["name"].as_str().or(v["id"].as_str()).unwrap_or("?").to_owned()
}

/// Prints rows as columns: the first left-aligned, `requests` right-aligned, two spaces between.
fn table(head: &[&str], rows: &[Vec<String>]) {
    let width = |i: usize| rows.iter().map(|r| r[i].chars().count()).chain([head[i].len()]).max().unwrap_or(0);
    let widths: Vec<usize> = (0..head.len()).map(width).collect();
    let line = |cells: Vec<&str>| {
        let parts: Vec<String> = cells
            .iter()
            .enumerate()
            .map(|(i, c)| if i == 1 { format!("{c:>w$}", w = widths[i]) } else { format!("{c:<w$}", w = widths[i]) })
            .collect();
        println!("{}", parts.join("  ").trim_end());
    };
    line(head.to_vec());
    for r in rows {
        line(r.iter().map(String::as_str).collect());
    }
}

pub(crate) fn run(home: Option<PathBuf>, as_json: bool) -> Result<ExitCode, ExitCode> {
    let home = home.map_or_else(OperatorHome::resolve, OperatorHome::new);
    let view = super::read(&home, views::latency::NEEDS, &json!({}), views::latency::build)?;
    let v = &view.json;
    if as_json {
        println!("{v:#}");
        return Ok(ExitCode::SUCCESS);
    }
    let (from, to) =
        (local_text(v["from"].as_str().unwrap_or_default()), local_text(v["to"].as_str().unwrap_or_default()));
    let zone = v["zone"].as_str().unwrap_or("local");
    println!("{}, {from} → {to} {zone}", v["window"].as_str().unwrap_or_default());
    let (agents, providers) = (v["agents"].as_array(), v["providers"].as_array());
    if agents.is_none_or(|a| a.is_empty()) && providers.is_none_or(|p| p.is_empty()) {
        println!("no requests in the last 24 h");
        return Ok(ExitCode::SUCCESS);
    }
    let read_date = to.split(' ').next().unwrap_or_default().to_owned();
    let rows: Vec<Vec<String>> = agents
        .into_iter()
        .flatten()
        .map(|a| {
            vec![name(a), a["requests"].to_string(), pct(&a["overhead"]), pct(&a["ttft"]), last(&a["last"], &read_date)]
        })
        .collect();
    println!();
    table(&["agent", "requests", "overhead p50/p95", "ttft p50/p95", "last response"], &rows);
    let rows: Vec<Vec<String>> = providers
        .into_iter()
        .flatten()
        .map(|p| {
            let by: Vec<String> = p["agents"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|a| format!("{} {}", name(a), a["requests"]))
                .collect();
            vec![
                p["id"].as_str().unwrap_or("?").to_owned(),
                p["requests"].to_string(),
                pct(&p["own_ttft"]),
                last(&p["last"], &read_date),
                by.join(", "),
            ]
        })
        .collect();
    println!();
    table(&["provider", "requests", "own ttft p50/p95", "last response", "by agent"], &rows);
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_print_in_ms_under_a_second_and_seconds_above() {
        assert_eq!(time(4.6), "5 ms");
        assert_eq!(time(999.0), "999 ms");
        assert_eq!(time(1000.0), "1.0 s");
        assert_eq!(time(1910.0), "1.9 s");
    }

    #[test]
    fn no_values_is_none_never_zero() {
        assert_eq!(pct(&Value::Null), "none");
        assert_eq!(pct(&json!({"p50": 420.0, "p95": 1910.0, "n": 3})), "420 ms / 1.9 s");
    }
}
