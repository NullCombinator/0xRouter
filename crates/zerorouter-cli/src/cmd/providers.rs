use std::path::PathBuf;
use std::process::ExitCode;

use serde_json::json;
use zerorouter_registry::CapabilityKind;

pub(crate) fn run(home: Option<PathBuf>, capability: Option<&str>, as_json: bool) -> Result<ExitCode, ExitCode> {
    let kind = match capability.map(|c| CapabilityKind::parse(c).ok_or(c)).transpose() {
        Ok(k) => k,
        Err(c) => {
            eprintln!("unknown capability {c:?}; allowed: {}", CapabilityKind::ALLOWED.join(", "));
            return Err(ExitCode::from(1));
        }
    };
    let reg = crate::open(home)?.snapshot();
    let rows: Vec<_> = reg
        .providers()
        .filter(|p| kind.is_none_or(|k| p.capabilities.contains_key(&k)))
        .map(|p| {
            json!({
                "id": p.id,
                "alias": p.alias,
                "category": p.category.as_str(),
                "capabilities": p.capabilities.keys().map(|k| k.as_str()).collect::<Vec<_>>(),
                "source": if p.is_bundled() { "bundled" } else { "user" },
            })
        })
        .collect();
    if as_json {
        println!("{:#}", json!(rows));
    } else {
        for r in &rows {
            println!(
                "{:<24} {:<10} {:<10} {}",
                r["id"].as_str().unwrap_or_default(),
                r["alias"].as_str().unwrap_or("-"),
                r["category"].as_str().unwrap_or_default(),
                r["capabilities"].as_array().into_iter().flatten().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(",")
            );
        }
    }
    Ok(ExitCode::SUCCESS)
}
