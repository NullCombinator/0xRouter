//! `model <provider> <model>`: what a provider declares about a model.

use nullrouter_registry::OperatorHome;
use serde_json::{Value, json};

use super::{Live, View, ViewError, open_registry};

pub const NEEDS: &[&str] = &[];

/// Arguments: `provider`, `model`. An unknown provider is an answer, not an error: its JSON is
/// `{"kind":"not_found","error"}` and the CLI exits 2.
pub fn build(home: &OperatorHome, args: &Value, _live: &Live) -> Result<View, ViewError> {
    let (provider, model) = (args["provider"].as_str().unwrap_or_default(), args["model"].as_str().unwrap_or_default());
    let reg = open_registry(home)?.snapshot();
    let info = match reg.model(provider, model) {
        Ok(info) => info,
        Err(e) => return Ok(View::new(json!({ "kind": "not_found", "error": e.to_string() }))),
    };
    let provider_id = reg.provider(provider).map(|p| p.id.as_str()).unwrap_or(provider);
    Ok(View::new(json!({
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
    })))
}
