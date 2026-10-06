//! `nullrouter resolve <target>`: exit 2 when the target isn't found.

use std::path::PathBuf;
use std::process::ExitCode;

use nullrouter_registry::OperatorHome;
use nullrouter_server::views;
use serde_json::json;

pub(crate) fn run(home: Option<PathBuf>, target: &str, as_json: bool) -> Result<ExitCode, ExitCode> {
    let home = home.map_or_else(OperatorHome::resolve, OperatorHome::new);
    let view = super::read(&home, views::resolve::NEEDS, &json!({"target": target}), views::resolve::build)?;
    let out = &view.json;
    let code = if out["kind"] == "not_found" { 2 } else { 0 };
    if as_json {
        println!("{out:#}");
    } else {
        match out["kind"].as_str() {
            Some("direct") => println!(
                "direct {} → {} (upstream {}{})",
                target,
                out["provider"].as_str().unwrap_or_default(),
                out["upstream_id"].as_str().unwrap_or_default(),
                if out["catalogued"] == true { "" } else { ", uncatalogued" }
            ),
            Some("unified") => {
                println!("unified {target}:");
                for (i, m) in out["members"].as_array().into_iter().flatten().enumerate() {
                    println!(
                        "  {i}. {} {} → upstream {}",
                        m["provider"].as_str().unwrap_or_default(),
                        m["requested"].as_str().unwrap_or_default(),
                        m["upstream_id"].as_str().unwrap_or_default()
                    );
                }
                for n in view.extra["notes"].as_array().into_iter().flatten().filter_map(|n| n.as_str()) {
                    println!("note: {n}");
                }
            }
            _ => eprintln!("not found: {}", out["error"].as_str().unwrap_or_default()),
        }
    }
    Ok(ExitCode::from(code))
}
