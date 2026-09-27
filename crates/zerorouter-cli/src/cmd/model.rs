use std::path::PathBuf;
use std::process::ExitCode;

use serde_json::json;

pub(crate) fn run(home: Option<PathBuf>, provider: &str, model: &str, as_json: bool) -> Result<ExitCode, ExitCode> {
    let reg = crate::open(home)?.snapshot();
    let info = match reg.model(provider, model) {
        Ok(info) => info,
        Err(e) => {
            if as_json {
                println!("{:#}", json!({ "kind": "not_found", "error": e.to_string() }));
            } else {
                eprintln!("not found: {e}");
            }
            return Ok(ExitCode::from(2));
        }
    };
    let provider_id = reg.provider(provider).map(|p| p.id.as_str()).unwrap_or(provider);
    let out = json!({
        "provider": provider_id,
        "model": model,
        "declared": info.declared,
        "name": info.name,
        "kind": info.kind.map(|k| k.as_str()),
        "target_format": info.target_format.map(|f| f.as_str()),
        "supported_formats": info.supported_formats.map(|f| f.iter().map(|x| x.as_str()).collect::<Vec<_>>()),
        "quota_family": info.quota_family,
        "strip": info.strip.map(|s| s.iter().map(|x| x.as_str()).collect::<Vec<_>>()),
        "upstream_id": info.upstream_id,
    });
    if as_json {
        println!("{out:#}");
    } else {
        for (k, v) in out.as_object().into_iter().flatten() {
            println!("{k}: {v}");
        }
    }
    Ok(ExitCode::SUCCESS)
}
