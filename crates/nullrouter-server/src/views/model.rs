//! `model <provider> [<model>]`: what a provider declares about a model, or about every model it
//! declares.

use nullrouter_registry::OperatorHome;
use serde_json::{Value, json};

use super::{Live, View, ViewError, open_registry};

pub const NEEDS: &[&str] = &[];

/// Arguments: `provider`, and `model`. Without `model`, a list: each model the provider
/// declares, in plugin order, as `model <provider> <model>` shows it (the dashboard's provider
/// window). An unknown provider is an answer, not an error: its JSON is
/// `{"kind":"not_found","error"}` and the CLI exits 2.
pub fn build(home: &OperatorHome, args: &Value, _live: &Live) -> Result<View, ViewError> {
    let provider = args["provider"].as_str().unwrap_or_default();
    let reg = open_registry(home)?.snapshot();
    let entity = match reg.provider(provider) {
        Ok(p) => p,
        Err(e) => return Ok(View::new(json!({ "kind": "not_found", "error": e.to_string() }))),
    };
    let one = |model: &str| match reg.model(provider, model) {
        Ok(info) => json!({
            "provider": entity.id,
            "model": model,
            "declared": info.declared,
            "name": info.name,
            "kind": info.kind.map(|k| k.as_str()),
            "target_format": info.target_format.map(|f| f.as_str()),
            "supported_formats": info.supported_formats.map(|f| f.iter().map(|x| x.as_str()).collect::<Vec<_>>()),
            "quota_family": info.quota_family,
            "strip": info.strip.map(|s| s.iter().map(|x| x.as_str()).collect::<Vec<_>>()),
            "upstream_id": info.upstream_id,
        }),
        Err(e) => json!({ "kind": "not_found", "error": e.to_string() }),
    };
    Ok(View::new(match args["model"].as_str() {
        Some(model) => one(model),
        None => Value::Array(entity.models.iter().flatten().map(|m| one(m.id.as_str())).collect()),
    }))
}
