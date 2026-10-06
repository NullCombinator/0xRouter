//! `unified [NAME]`: the unified models the registry loaded, and those it dropped.
//!
//! `--json` without NAME: `{"unified":[<entry>…],"dropped":[{"name","reason"}]}`, where each entry
//! is exactly what `resolve <name> --json` prints for that model. With NAME it is that one entry,
//! or the `{"kind":"not_found","error"}` object `resolve` prints; the CLI exits 2 on it. A model
//! `extra.notes` maps each model's name to its limits notes as the text prints them. A model dropped at startup isn't loaded, so its name is not found like any other.

use nullrouter_registry::{OperatorHome, Registry};
use serde_json::{Value, json};

use super::{Live, View, ViewError, open_registry, resolve};

pub const NEEDS: &[&str] = &[];

/// Arguments: `name`, or null for the list.
pub fn build(home: &OperatorHome, args: &Value, _live: &Live) -> Result<View, ViewError> {
    let reg = open_registry(home)?.snapshot();
    if let Some(name) = args["name"].as_str() {
        let json = match reg.unified_model(name) {
            Ok(u) => resolve::unified_json(&reg, u),
            Err(e) => json!({ "kind": "not_found", "error": e.to_string() }),
        };
        return Ok(View { json, extra: json!({ "notes": notes(&reg, Some(name)) }) });
    }
    let dropped = reg
        .report()
        .dropped_unified_models
        .iter()
        .map(|d| json!({ "name": d.name, "reason": format!("member provider {} was skipped", d.provider) }));
    let unified = reg.unified_models().map(|u| resolve::unified_json(&reg, u));
    Ok(View {
        json: json!({ "unified": unified.collect::<Vec<_>>(), "dropped": dropped.collect::<Vec<_>>() }),
        extra: json!({ "notes": notes(&reg, None) }),
    })
}

/// The limits notes as the text prints them, by unified model name: those of `only`, or all.
fn notes(reg: &Registry, only: Option<&str>) -> Value {
    let mut by_name = serde_json::Map::new();
    for n in reg.report().notes.iter().filter(|n| only.is_none_or(|o| o == n.unified)) {
        by_name
            .entry(n.unified.clone())
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .unwrap()
            .push(json!(n.to_string()));
    }
    Value::Object(by_name)
}
