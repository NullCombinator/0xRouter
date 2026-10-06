use std::path::PathBuf;
use std::process::ExitCode;

use nullrouter_registry::OperatorHome;
use nullrouter_server::views;
use serde_json::json;

pub(crate) fn run(home: Option<PathBuf>, capability: Option<&str>, as_json: bool) -> Result<ExitCode, ExitCode> {
    let home = home.map_or_else(OperatorHome::resolve, OperatorHome::new);
    let view =
        super::read(&home, views::providers::NEEDS, &json!({"capability": capability}), views::providers::build)?;
    let rows = view.json.as_array().map_or(&[][..], Vec::as_slice);
    if as_json {
        println!("{:#}", view.json);
    } else {
        for r in rows {
            println!(
                "{:<24} {:<10} {:<10} {}",
                r["id"].as_str().unwrap_or_default(),
                r["alias"].as_str().unwrap_or("-"),
                r["category"].as_str().unwrap_or_default(),
                r["capabilities"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|v| v.as_str())
                    .collect::<Vec<_>>()
                    .join(",")
            );
        }
    }
    Ok(ExitCode::SUCCESS)
}
