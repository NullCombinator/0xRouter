//! `nullrouter unified [NAME]`: exit 2 when NAME isn't a loaded unified model.

use std::path::PathBuf;
use std::process::ExitCode;

use nullrouter_engine::verdict::State;
use nullrouter_registry::OperatorHome;
use nullrouter_server::views;
use serde_json::{Value, json};

/// `  verdicts: max BROKEN, pro PASS` after member `i`, when any member of the model has a
/// verdict; a member with none reads `untested`.
fn verdicts_of(verdicts: &Value, i: usize) -> String {
    let any = verdicts.as_array().is_some_and(|all| all.iter().any(|m| m.as_array().is_some_and(|v| !v.is_empty())));
    if !any {
        return String::new();
    }
    let mine: Vec<String> = verdicts[i]
        .as_array()
        .into_iter()
        .flatten()
        .map(|v| {
            let state = v["state"].as_str().and_then(State::parse).map_or("?", State::label);
            format!("{} {state}", v["account"].as_str().unwrap_or_default())
        })
        .collect();
    if mine.is_empty() { "  verdicts: untested".to_owned() } else { format!("  verdicts: {}", mine.join(", ")) }
}

fn print_model(entry: &Value, notes: &Value, verdicts: &Value, indent: &str) {
    let name = entry["name"].as_str().unwrap_or_default();
    match entry["model_kind"].as_str() {
        Some(kind) => println!("{name}  {kind}"),
        None => println!("{name}"),
    }
    for (i, m) in entry["members"].as_array().into_iter().flatten().enumerate() {
        println!(
            "{indent}{i}. {} {} → upstream {}{}",
            m["provider"].as_str().unwrap_or_default(),
            m["requested"].as_str().unwrap_or_default(),
            m["upstream_id"].as_str().unwrap_or_default(),
            verdicts_of(verdicts, i)
        );
    }
    for n in notes[name].as_array().into_iter().flatten().filter_map(Value::as_str) {
        println!("{indent}note: {n}");
    }
}

pub(crate) fn run(home: Option<PathBuf>, name: Option<&str>, as_json: bool) -> Result<ExitCode, ExitCode> {
    let home = home.map_or_else(OperatorHome::resolve, OperatorHome::new);
    let view = super::read(&home, views::unified::NEEDS, &json!({"name": name}), views::unified::build)?;
    let out = &view.json;
    let code = if out["kind"] == "not_found" { 2 } else { 0 };
    if as_json {
        println!("{out:#}");
    } else if out["kind"] == "not_found" {
        eprintln!("not found: {}", out["error"].as_str().unwrap_or_default());
    } else if name.is_some() {
        print_model(out, &view.extra["notes"], &view.extra["verdicts"], "  ");
    } else {
        let (models, dropped) = (out["unified"].as_array().unwrap(), out["dropped"].as_array().unwrap());
        if models.is_empty() && dropped.is_empty() {
            println!("no unified models; declare one with [[unified_model]] in config.toml");
        }
        for m in models {
            print_model(m, &view.extra["notes"], &Value::Null, "  ");
        }
        for d in dropped {
            println!(
                "dropped unified model {}: {}",
                d["name"].as_str().unwrap_or_default(),
                d["reason"].as_str().unwrap_or_default()
            );
        }
    }
    Ok(ExitCode::from(code))
}
