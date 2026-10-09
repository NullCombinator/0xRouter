//! `nullrouter usage [--period]`: request and token totals and Est. Cost for one period.

use std::path::PathBuf;
use std::process::ExitCode;

use nullrouter_registry::OperatorHome;
use nullrouter_server::views::{self, usage::local_text};
use serde_json::{Value, json};

/// 1234567 → `1,234,567`.
fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Exact below 1,000; `612k`; from a million, one decimal: `1.3M`.
pub(crate) fn compact(n: u64) -> String {
    if n < 1_000 {
        n.to_string()
    } else if n < 999_500 {
        format!("{}k", (n as f64 / 1_000.0).round() as u64)
    } else {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    }
}

fn count(v: &Value, k: &str) -> u64 {
    v[k].as_u64().unwrap_or(0)
}

fn table(title: &str, rows: &Value, name: impl Fn(&Value) -> String) {
    println!();
    println!("{title:<18}requests");
    for r in rows.as_array().into_iter().flatten() {
        println!("{:<18}{:>8}", name(r), grouped(count(r, "requests")));
    }
}

pub(crate) fn run(home: Option<PathBuf>, period: &str, as_json: bool) -> Result<ExitCode, ExitCode> {
    let home = home.map_or_else(OperatorHome::resolve, OperatorHome::new);
    let view = super::read(&home, views::usage::NEEDS, &json!({"period": period}), views::usage::build)?;
    let v = &view.json;
    if as_json {
        println!("{v:#}");
        return Ok(ExitCode::SUCCESS);
    }
    let zone = v["zone"].as_str().unwrap_or("local");
    let to = local_text(v["to"].as_str().unwrap_or_default());
    match v["from"].as_str() {
        Some(from) => {
            println!("period: {}, {} → {to} {zone}", v["period"].as_str().unwrap_or(period), local_text(from))
        }
        None => println!("period: {}, up to {to} {zone}", v["period"].as_str().unwrap_or(period)),
    }
    for w in v["warnings"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        println!("{w}");
    }
    let requests = count(v, "requests");
    if requests == 0 {
        println!("requests          0 (no requests in this period)");
        return Ok(ExitCode::SUCCESS);
    }
    let (in_flight, not_reported) = (count(v, "in_flight"), count(v, "not_reported"));
    let mut notes = Vec::new();
    if in_flight > 0 {
        notes.push(format!("{in_flight} in flight"));
    }
    if not_reported > 0 {
        notes.push(format!("{not_reported} not reported"));
    }
    let tail = if notes.is_empty() { String::new() } else { format!("   ({})", notes.join(", ")) };
    println!("{:<18}{}{tail}", "requests", grouped(requests));
    let t = &v["tokens"];
    println!("{:<18}{}", "input (uncached)", compact(count(t, "input")));
    println!("{:<18}{}", "cached", compact(count(t, "cached")));
    println!("{:<18}{}", "output", compact(count(t, "output")));
    let cost = &v["cost"];
    println!(
        "{:<18}~${:.2}   {}",
        "est. cost",
        cost["usd"].as_f64().unwrap_or(0.0),
        cost["label"].as_str().unwrap_or_default()
    );
    let pad = " ".repeat(18);
    let un = &cost["unpriced"];
    if count(un, "requests") > 0 {
        let reasons: Vec<String> =
            [("no_price", "no price"), ("account_gone", "account gone"), ("no_output_price", "no output price")]
                .iter()
                .filter(|(k, _)| count(un, k) > 0)
                .map(|(k, label)| format!("{} {label}", count(un, k)))
                .collect();
        println!("{pad}{} requests not priced: {}", count(un, "requests"), reasons.join(", "));
    }
    println!("{pad}{}", cost["note"].as_str().unwrap_or_default());
    table("agent", &v["agents"], |r| r["name"].as_str().or(r["id"].as_str()).unwrap_or("?").to_owned());
    table("provider", &v["providers"], |r| r["id"].as_str().unwrap_or("?").to_owned());
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_print_compact() {
        assert_eq!(compact(0), "0");
        assert_eq!(compact(999), "999");
        assert_eq!(compact(1_000), "1k");
        assert_eq!(compact(611_900), "612k");
        assert_eq!(compact(1_300_000), "1.3M");
        assert_eq!(compact(999_600), "1.0M");
        assert_eq!(grouped(1_284), "1,284");
        assert_eq!(grouped(12), "12");
        assert_eq!(grouped(1_000_000), "1,000,000");
    }
}
