//! `nullrouter combos [NAME]`: exit 2 when NAME isn't a loaded combo.

use std::path::PathBuf;
use std::process::ExitCode;

use nullrouter_registry::OperatorHome;
use nullrouter_server::views;
use serde_json::{Value, json};

/// One line of a combo's tree: its indent and number, its name, and what it is.
fn rows(entry: &Value, indent: &str, out: &mut Vec<(String, String, &'static str)>) {
    for (i, m) in entry["members"].as_array().into_iter().flatten().enumerate() {
        let name = m["name"].as_str().unwrap_or_default().to_owned();
        let kind = if m["kind"] == "combo" { "combo" } else { "unified" };
        let lead = format!("{indent}{i}. ");
        out.push((lead.clone(), name, kind));
        if kind == "combo" {
            rows(m, &" ".repeat(lead.chars().count()), out);
        }
    }
}

/// `combo coder (llm):` and its members, nested combos expanded, the kinds in one column.
pub(crate) fn print_tree(entry: &Value) {
    let name = entry["name"].as_str().unwrap_or_default();
    match entry["model_kind"].as_str() {
        Some(kind) => println!("combo {name} ({kind}):"),
        None => println!("combo {name}:"),
    }
    let mut out = Vec::new();
    rows(entry, "  ", &mut out);
    let width = out.iter().map(|(lead, name, _)| lead.chars().count() + name.chars().count()).max().unwrap_or(0) + 2;
    for (lead, name, kind) in out {
        let pad = width - lead.chars().count();
        println!("{lead}{name:<pad$}{kind}");
    }
}

pub(crate) fn run(home: Option<PathBuf>, name: Option<&str>, as_json: bool) -> Result<ExitCode, ExitCode> {
    let home = home.map_or_else(OperatorHome::resolve, OperatorHome::new);
    let view = super::read(&home, views::combos::NEEDS, &json!({"name": name}), views::combos::build)?;
    let out = &view.json;
    let code = if out["kind"] == "not_found" { 2 } else { 0 };
    if as_json {
        println!("{out:#}");
    } else if out["kind"] == "not_found" {
        eprintln!("not found: {}", out["error"].as_str().unwrap_or_default());
    } else if name.is_some() {
        print_tree(out);
    } else {
        let (combos, dropped) = (out["combos"].as_array().unwrap(), out["dropped"].as_array().unwrap());
        if combos.is_empty() && dropped.is_empty() {
            println!("no combos; declare one with [[combo]] in config.toml");
        }
        for c in combos {
            print_tree(c);
        }
        for d in dropped {
            println!(
                "dropped combo {}: {}",
                d["name"].as_str().unwrap_or_default(),
                d["reason"].as_str().unwrap_or_default()
            );
        }
    }
    Ok(ExitCode::from(code))
}
