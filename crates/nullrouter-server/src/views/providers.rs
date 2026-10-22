//! `providers [--capability]`: the loaded providers, from the home's files.

use nullrouter_registry::{CapabilityKind, OperatorHome};
use serde_json::{Value, json};

use super::{Live, View, ViewError, open_registry};

pub const NEEDS: &[&str] = &[];

/// Arguments: `capability` (a string or null), and `all` (true keeps hidden providers, for the
/// settings panel that shows them again; model lists and the default list leave them out).
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
    let all = args["all"].as_bool().unwrap_or(false);
    let reg = open_registry(home)?.snapshot();
    let rows: Vec<Value> = reg
        .providers()
        .filter(|p| all || !reg.settings(&p.id).hidden)
        .filter(|p| kind.is_none_or(|k| p.capabilities.contains_key(&k)))
        .map(|p| {
            json!({
                "id": p.id,
                "alias": p.alias,
                "category": p.category.as_str(),
                "capabilities": p.capabilities.keys().map(|k| k.as_str()).collect::<Vec<_>>(),
                "source": if p.is_bundled() { "bundled" } else { "user" },
                "signin": p.signin.is_some(),
                "terms_warning": p.signin.as_ref().is_some_and(|s| s.terms_warning),
                "enabled": reg.settings(&p.id).enabled,
                "hidden": reg.settings(&p.id).hidden,
            })
        })
        .collect();
    Ok(View::new(json!(rows)))
}
