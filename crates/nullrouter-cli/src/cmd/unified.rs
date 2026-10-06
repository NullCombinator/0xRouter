//! `nullrouter unified [NAME]`: exit 2 when NAME isn't a loaded unified model.

use std::path::PathBuf;
use std::process::ExitCode;

use nullrouter_registry::OperatorHome;
use nullrouter_server::views;
use serde_json::{Value, json};

fn print_model(entry: &Value, notes: &Value, indent: &str) {
    let name = entry["name"].as_str().unwrap_or_default();
    match entry["model_kind"].as_str() {
        Some(kind) => println!("{name}  {kind}"),
        None => println!("{name}"),
    }
    for (i, m) in entry["members"].as_array().into_iter().flatten().enumerate() {
        println!(
            "{indent}{i}. {} {} → upstream {}",
            m["provider"].as_str().unwrap_or_default(),
            m["requested"].as_str().unwrap_or_default(),
            m["upstream_id"].as_str().unwrap_or_default()
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
        print_model(out, &view.extra["notes"], "  ");
    } else {
        let (models, dropped) = (out["unified"].as_array().unwrap(), out["dropped"].as_array().unwrap());
        if models.is_empty() && dropped.is_empty() {
            println!("no unified models; declare one with [[unified_model]] in config.toml");
        }
        for m in models {
            print_model(m, &view.extra["notes"], "  ");
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
