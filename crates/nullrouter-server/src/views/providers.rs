//! `providers [--capability]`: the loaded providers, from the home's files.

use nullrouter_registry::{CapabilityKind, OperatorHome};
use serde_json::{Value, json};

use super::{Live, View, ViewError, open_registry};

pub const NEEDS: &[&str] = &[];

/// Arguments: `capability` (a string or null).
pub fn build(home: &OperatorHome, args: &Value, _live: &Live) -> Result<View, ViewError> {
    let capability = args["capability"].as_str();
    let kind = match capability.map(|c| CapabilityKind::parse(c).ok_or(c)).transpose() {
        Ok(k) => k,
        Err(c) => {
            return Err(ViewError::failed(format!(
                "unknown capability {c:?}; allowed: {}",
                CapabilityKind::ALLOWED.join(", ")
            )));
        }
    };
    let reg = open_registry(home)?.snapshot();
    let rows: Vec<Value> = reg
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
    Ok(View::new(json!(rows)))
}
